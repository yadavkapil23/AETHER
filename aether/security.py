import time
from collections import defaultdict
from dataclasses import dataclass

import jwt
from fastapi import HTTPException, Request
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.responses import JSONResponse


class ApiKeyValidator:
    def __init__(self, fallback_keys: set[str], jwt_secret: str, database=None) -> None:
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

        auth = request.headers.get("authorization", "")
        if auth.lower().startswith("bearer "):
            token = auth.split(" ", 1)[1]
            try:
                jwt.decode(token, self.jwt_secret, algorithms=["HS256"])
                return
            except jwt.PyJWTError as exc:
                raise HTTPException(status_code=401, detail="invalid bearer token") from exc

        raise HTTPException(status_code=401, detail="missing or invalid API key")


@dataclass
class Bucket:
    tokens: float
    updated_at: float


class RateLimitMiddleware(BaseHTTPMiddleware):
    def __init__(self, app, rps: int, metrics=None) -> None:
        super().__init__(app)
        self.rps = max(1, rps)
        self.metrics = metrics
        self.buckets: dict[str, Bucket] = defaultdict(lambda: Bucket(self.rps, time.monotonic()))

    async def dispatch(self, request: Request, call_next):
        api_key = request.headers.get("x-api-key")
        client_id = request.headers.get("x-client-id")
        client = api_key or client_id or (request.client.host if request.client else "unknown")
        now = time.monotonic()
        bucket = self.buckets[client]
        elapsed = now - bucket.updated_at
        bucket.tokens = min(self.rps, bucket.tokens + elapsed * self.rps)
        bucket.updated_at = now

        if bucket.tokens < 1:
            if self.metrics:
                self.metrics.rate_limited_total.inc()
            return JSONResponse({"error": "rate limit exceeded"}, status_code=429)

        bucket.tokens -= 1
        return await call_next(request)


class SecurityHeadersMiddleware(BaseHTTPMiddleware):
    async def dispatch(self, request: Request, call_next):
        response = await call_next(request)
        response.headers["x-content-type-options"] = "nosniff"
        response.headers["x-frame-options"] = "DENY"
        response.headers["x-xss-protection"] = "1; mode=block"
        return response

