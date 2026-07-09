import logging
from typing import Any

LOGGER = logging.getLogger(__name__)


class Database:
    def __init__(self, database_url: str) -> None:
        self.database_url = database_url
        self.pool: Any | None = None

    async def connect(self) -> None:
        try:
            import asyncpg
        except ImportError as exc:
            raise RuntimeError(
                "asyncpg is not installed; run `pip install -r requirements.txt` to enable Postgres"
            ) from exc
        self.pool = await asyncpg.create_pool(self.database_url, min_size=1, max_size=10)
        await self.migrate()

    async def close(self) -> None:
        if self.pool:
            await self.pool.close()

    async def migrate(self) -> None:
        if not self.pool:
            return
        async with self.pool.acquire() as conn:
            await conn.execute(
                """
                CREATE EXTENSION IF NOT EXISTS pgcrypto;
                CREATE TABLE IF NOT EXISTS api_keys (
                    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                    key TEXT UNIQUE NOT NULL,
                    name TEXT,
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                    last_used TIMESTAMPTZ,
                    is_active BOOLEAN NOT NULL DEFAULT TRUE,
                    created_by TEXT
                );
                CREATE TABLE IF NOT EXISTS inference_logs (
                    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                    model TEXT NOT NULL,
                    status TEXT NOT NULL,
                    latency_ms INTEGER NOT NULL,
                    tokens_generated INTEGER,
                    backend TEXT,
                    error_message TEXT,
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
                );
                CREATE TABLE IF NOT EXISTS audit_logs (
                    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                    action TEXT NOT NULL,
                    resource_type TEXT,
                    resource_id TEXT,
                    user_id TEXT,
                    details TEXT,
                    status TEXT NOT NULL,
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
                );
                """
            )

    async def health(self) -> bool:
        if not self.pool:
            return False
        try:
            async with self.pool.acquire() as conn:
                await conn.fetchval("SELECT 1")
            return True
        except Exception:
            LOGGER.exception("database health check failed")
            return False

    async def validate_api_key(self, key: str) -> bool:
        if not self.pool:
            return False
        async with self.pool.acquire() as conn:
            row = await conn.fetchrow(
                "SELECT key FROM api_keys WHERE key = $1 AND is_active = TRUE", key
            )
        return row is not None

    async def add_api_key(self, key: str, name: str | None = None) -> dict[str, Any]:
        if not self.pool:
            raise RuntimeError("database not connected")
        async with self.pool.acquire() as conn:
            row = await conn.fetchrow(
                """
                INSERT INTO api_keys (key, name)
                VALUES ($1, $2)
                ON CONFLICT (key) DO UPDATE SET is_active = TRUE, name = EXCLUDED.name
                RETURNING id::text, key, name, created_at, last_used, is_active, created_by
                """,
                key,
                name,
            )
        return dict(row)

    async def list_api_keys(self) -> list[dict[str, Any]]:
        if not self.pool:
            return []
        async with self.pool.acquire() as conn:
            rows = await conn.fetch(
                """
                SELECT id::text, key, name, created_at, last_used, is_active, created_by
                FROM api_keys
                ORDER BY created_at DESC
                """
            )
        return [dict(row) for row in rows]

    async def revoke_api_key(self, key: str) -> bool:
        if not self.pool:
            return False
        async with self.pool.acquire() as conn:
            result = await conn.execute("UPDATE api_keys SET is_active = FALSE WHERE key = $1", key)
        return not result.endswith("0")

    async def log_inference(
        self,
        model: str,
        status: str,
        latency_ms: int,
        tokens_generated: int | None = None,
        backend: str | None = None,
        error_message: str | None = None,
    ) -> None:
        if not self.pool:
            return
        async with self.pool.acquire() as conn:
            await conn.execute(
                """
                INSERT INTO inference_logs
                (model, status, latency_ms, tokens_generated, backend, error_message)
                VALUES ($1, $2, $3, $4, $5, $6)
                """,
                model,
                status,
                latency_ms,
                tokens_generated,
                backend,
                error_message,
            )

    async def log_audit(
        self,
        action: str,
        status: str,
        resource_type: str | None = None,
        resource_id: str | None = None,
        details: str | None = None,
        user_id: str | None = None,
    ) -> None:
        if not self.pool:
            return
        async with self.pool.acquire() as conn:
            await conn.execute(
                """
                INSERT INTO audit_logs
                (action, resource_type, resource_id, user_id, details, status)
                VALUES ($1, $2, $3, $4, $5, $6)
                """,
                action,
                resource_type,
                resource_id,
                user_id,
                details,
                status,
            )
