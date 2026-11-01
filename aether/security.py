import time
import uuid
from pathlib import Path

import jwt
from fastapi import HTTPException, Request
from redis.asyncio import Redis
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.responses import JSONResponse


class ApiKeyValidator:
    def __init__(
        self,
        fallback_keys: set[str],
        jwt_secret: str,
        database=None,
    ) -> None:
        self.fallback_keys = fallback_keys
        self.jwt_secret = jwt_secret
        self.database = database

    async def validate(self, request: Request) -> None:
        if request.url.path in {
            "/health",
            "/health/live",
            "/health/ready",
            "/health/startup",
            "/ready",
            "/metrics",
        }:
            return

        api_key = request.headers.get("x-api-key")

        if api_key:
            if api_key in self.fallback_keys:
                return

            if self.database and await self.database.validate_api_key(api_key):
                return

        authorization = request.headers.get("authorization", "")

        if authorization.lower().startswith("bearer "):
            token = authorization.split(" ", 1)[1]

            try:
                jwt.decode(
                    token,
                    self.jwt_secret,
                    algorithms=["HS256"],
                )
                return
            except jwt.PyJWTError as exc:
                raise HTTPException(
                    status_code=401,
                    detail="invalid bearer token",
                ) from exc

        raise HTTPException(
            status_code=401,
            detail="missing or invalid API key",
        )


class RateLimitMiddleware(BaseHTTPMiddleware):
    def __init__(
        self,
        app,
        redis: Redis,
        rps: int,
        window_seconds: int = 1,
        fail_closed: bool = True,
        metrics=None,
    ) -> None:
        super().__init__(app)

        self.redis = redis
        self.limit = max(1, rps)
        self.window_seconds = max(1, window_seconds)
        self.fail_closed = fail_closed
        self.metrics = metrics

        script_path = Path(__file__).with_name("rate_limit.lua")
        self.script = script_path.read_text(encoding="utf-8")

    def _client_identity(self, request: Request) -> str:
        api_key = request.headers.get("x-api-key")

        if api_key:
            return f"api-key:{api_key}"

        client_id = request.headers.get("x-client-id")

        if client_id:
            return f"client-id:{client_id}"

        client_ip = request.client.host if request.client else "unknown"
        return f"ip:{client_ip}"

    async def dispatch(self, request: Request, call_next):
        identity = self._client_identity(request)
        route = request.url.path

        redis_key = (
            f"aether:rate-limit:{identity}:{route}"
        )

        now = time.time()
        request_id = f"{now}:{uuid.uuid4().hex}"

        try:
            result = await self.redis.eval(
                self.script,
                1,
                redis_key,
                now,
                self.window_seconds,
                self.limit,
                request_id,
            )

            allowed = int(result[0])
            used = int(result[1])
            retry_after = int(result[2])

        except Exception:
            if self.fail_closed:
                return JSONResponse(
                    {
                        "error": "rate limiter unavailable",
                    },
                    status_code=503,
                )

            return await call_next(request)

        if not allowed:
            if self.metrics:
                self.metrics.rate_limited_total.inc()

            return JSONResponse(
                {
                    "error": "rate limit exceeded",
                },
                status_code=429,
                headers={
                    "Retry-After": str(max(1, retry_after)),
                    "X-RateLimit-Limit": str(self.limit),
                    "X-RateLimit-Remaining": "0",
                },
            )

        response = await call_next(request)

        response.headers["X-RateLimit-Limit"] = str(self.limit)
        response.headers["X-RateLimit-Remaining"] = str(
            max(0, self.limit - used)
        )

        return response


class SecurityHeadersMiddleware(BaseHTTPMiddleware):
    async def dispatch(self, request: Request, call_next):
        response = await call_next(request)

        response.headers["x-content-type-options"] = "nosniff"
        response.headers["x-frame-options"] = "DENY"
        response.headers["x-xss-protection"] = "1; mode=block"

        return response