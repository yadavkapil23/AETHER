import asyncio
import logging
import time
import uuid
from contextlib import asynccontextmanager

import httpx
import uvicorn
from fastapi import Depends, FastAPI, HTTPException, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse, Response, StreamingResponse

from aether.audit import AuditTrail, new_audit_event
from aether.backends import LLMBackend
from aether.config import Settings, get_settings
from aether.database import Database
from aether.metrics import Metrics
from aether.models import (
    AllocateRequest,
    ChatCompletionRequest,
    DeallocateRequest,
    InferenceRequest,
    InferenceResponse,
)
from aether.scheduler import Scheduler
from aether.security import ApiKeyValidator, RateLimitMiddleware, SecurityHeadersMiddleware

LOGGER = logging.getLogger(__name__)


class GatewayState:
    def __init__(self, settings: Settings) -> None:
        self.settings = settings
        self.metrics = Metrics()
        self.database = Database(settings.database_url)
        self.audit = AuditTrail()
        self.scheduler = Scheduler(settings.cache_bytes, settings.block_size)
        self.llm = LLMBackend(
            settings.vllm_endpoint,
            settings.llamacpp_endpoint,
            settings.ollama_endpoint,
            settings.huggingface_endpoint,
            settings.huggingface_api_key,
            settings.gateway_timeout,
        )
        self.auth = ApiKeyValidator(settings.fallback_api_keys, settings.jwt_secret, self.database)


def build_app(settings: Settings | None = None) -> FastAPI:
    settings = settings or get_settings()
    state = GatewayState(settings)

    @asynccontextmanager
    async def lifespan(app: FastAPI):
        app.state.aether = state
        try:
            await state.database.connect()
        except Exception:
            LOGGER.exception("PostgreSQL unavailable; running with env API keys only")
        yield
        await state.database.close()
        await state.llm.close()

    app = FastAPI(title="AETHER Python Gateway", version="0.1.0", lifespan=lifespan)
    app.add_middleware(
        CORSMiddleware,
        allow_origins=["*"],
        allow_methods=["*"],
        allow_headers=["*"],
    )
    app.add_middleware(SecurityHeadersMiddleware)
    app.add_middleware(RateLimitMiddleware, rps=settings.rate_limit_rps, metrics=state.metrics)

    async def require_auth(request: Request) -> None:
        await request.app.state.aether.auth.validate(request)

    def aether_state(request: Request) -> GatewayState:
        return request.app.state.aether

    @app.exception_handler(HTTPException)
    async def http_exception_handler(_: Request, exc: HTTPException):
        return JSONResponse({"error": exc.detail}, status_code=exc.status_code)

    @app.post("/infer", response_model=InferenceResponse, dependencies=[Depends(require_auth)])
    async def infer(req: InferenceRequest, app_state: GatewayState = Depends(aether_state)):
        start = time.perf_counter()
        try:
            result = await app_state.llm.infer(
                req.model, req.prompt, req.max_tokens, req.temperature, req.top_p
            )
            latency_ms = int((time.perf_counter() - start) * 1000)
            app_state.metrics.record_inference_success(
                req.model, result.backend, latency_ms, result.tokens_generated
            )
            asyncio.create_task(
                app_state.database.log_inference(
                    req.model, "success", latency_ms, result.tokens_generated, result.backend
                )
            )
            event_hash = app_state.audit.append(
                new_audit_event(req.model, "inference", f"{result.backend}:{result.tokens_generated}")
            )
            asyncio.create_task(
                app_state.database.log_audit(
                    "inference",
                    "success",
                    "model",
                    req.model,
                    f"backend={result.backend},audit_hash={event_hash}",
                )
            )
            return InferenceResponse(
                success=True,
                output=result.output,
                tokens_generated=result.tokens_generated,
                latency_ms=latency_ms,
                backend=result.backend,
            )
        except Exception as exc:
            latency_ms = int((time.perf_counter() - start) * 1000)
            app_state.metrics.record_inference_error(req.model, "unknown", "failure")
            asyncio.create_task(
                app_state.database.log_inference(
                    req.model, "failure", latency_ms, error_message=str(exc)
                )
            )
            raise HTTPException(status_code=502, detail=f"inference failed: {exc}") from exc

    @app.post("/infer/stream", dependencies=[Depends(require_auth)])
    async def infer_stream(req: InferenceRequest, app_state: GatewayState = Depends(aether_state)):
        url = f"{app_state.settings.vllm_endpoint.rstrip('/')}/v1/completions"
        payload = {
            "model": req.model,
            "prompt": req.prompt,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
            "top_p": req.top_p,
            "stream": True,
        }

        async def stream():
            async with httpx.AsyncClient(timeout=120) as client:
                async with client.stream("POST", url, json=payload) as response:
                    response.raise_for_status()
                    async for line in response.aiter_lines():
                        line = line.strip()
                        if line.startswith("data:"):
                            yield f"{line}\n\n"

        return StreamingResponse(stream(), media_type="text/event-stream")

    @app.post("/v1/chat/completions", dependencies=[Depends(require_auth)])
    async def chat_completions(
        req: ChatCompletionRequest, app_state: GatewayState = Depends(aether_state)
    ):
        if not req.messages:
            raise HTTPException(status_code=400, detail="messages cannot be empty")
        prompt = "\n".join(f"{message.role}: {message.content}" for message in req.messages)
        result = await app_state.llm.infer(
            req.model,
            prompt,
            req.max_tokens or 1024,
            req.temperature,
            req.top_p,
        )
        return {
            "id": f"chatcmpl-{uuid.uuid4()}",
            "object": "chat.completion",
            "created": int(time.time()),
            "model": req.model,
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": result.output},
                    "finish_reason": "stop",
                }
            ],
            "usage": {
                "prompt_tokens": result.prompt_tokens,
                "completion_tokens": result.tokens_generated,
                "total_tokens": result.total_tokens,
            },
        }

    @app.post("/v1/allocate", dependencies=[Depends(require_auth)])
    async def allocate(req: AllocateRequest, app_state: GatewayState = Depends(aether_state)):
        start = time.perf_counter()
        try:
            block_ids, node_id = app_state.scheduler.allocate(req.request_id, req.num_blocks, req.owner)
            stats = app_state.scheduler.stats()
            app_state.metrics.cache_allocated_blocks.set(stats.allocated_blocks)
            return {
                "success": True,
                "block_ids": block_ids,
                "latency_ms": int((time.perf_counter() - start) * 1000),
                "node_id": node_id,
                "error": None,
            }
        except Exception as exc:
            raise HTTPException(status_code=500, detail=str(exc)) from exc

    @app.post("/v1/deallocate", dependencies=[Depends(require_auth)])
    async def deallocate(req: DeallocateRequest, app_state: GatewayState = Depends(aether_state)):
        start = time.perf_counter()
        count = app_state.scheduler.deallocate(req.block_ids)
        stats = app_state.scheduler.stats()
        app_state.metrics.cache_allocated_blocks.set(stats.allocated_blocks)
        return {
            "success": True,
            "count": count,
            "latency_ms": int((time.perf_counter() - start) * 1000),
            "error": None,
        }

    @app.get("/v1/stats", dependencies=[Depends(require_auth)])
    async def stats(app_state: GatewayState = Depends(aether_state)):
        return app_state.scheduler.stats().__dict__

    @app.get("/v1/cluster", dependencies=[Depends(require_auth)])
    async def cluster(app_state: GatewayState = Depends(aether_state)):
        return app_state.scheduler.cluster_health()

    @app.get("/backends/status", dependencies=[Depends(require_auth)])
    async def backends_status(app_state: GatewayState = Depends(aether_state)):
        return await app_state.llm.health()

    @app.post("/api/keys", dependencies=[Depends(require_auth)])
    async def create_api_key(body: dict, app_state: GatewayState = Depends(aether_state)):
        key = body.get("key") or f"sk-{uuid.uuid4().hex}"
        row = await app_state.database.add_api_key(key, body.get("name"))
        row["key"] = mask_key(row["key"])
        return row

    @app.get("/api/keys", dependencies=[Depends(require_auth)])
    async def list_api_keys(app_state: GatewayState = Depends(aether_state)):
        rows = await app_state.database.list_api_keys()
        for row in rows:
            row["key"] = mask_key(row["key"])
        return rows

    @app.delete("/api/keys/{key}", dependencies=[Depends(require_auth)])
    async def revoke_api_key(key: str, app_state: GatewayState = Depends(aether_state)):
        return {"revoked": await app_state.database.revoke_api_key(key)}

    @app.get("/health/live")
    async def health_live():
        return {"status": "alive"}

    @app.get("/health/startup")
    async def health_startup():
        return {"status": "started", "timestamp": int(time.time())}

    @app.get("/health/ready")
    async def health_ready(app_state: GatewayState = Depends(aether_state)):
        backend_status = await app_state.llm.health()
        ready = any(item["healthy"] for item in backend_status.values())
        return JSONResponse(
            {"status": "ready" if ready else "not_ready", "backends": backend_status},
            status_code=200 if ready else 503,
        )

    @app.get("/ready")
    async def ready(app_state: GatewayState = Depends(aether_state)):
        db_ok = await app_state.database.health()
        return {"ready": db_ok, "database": db_ok}

    @app.get("/health")
    async def health(app_state: GatewayState = Depends(aether_state)):
        return {
            "status": "healthy",
            "database": await app_state.database.health(),
            "audit_verified": app_state.audit.verify(),
            "backends": await app_state.llm.health(),
            "scheduler": app_state.scheduler.cluster_health(),
        }

    @app.get("/metrics")
    async def metrics(app_state: GatewayState = Depends(aether_state)):
        return Response(app_state.metrics.export(), media_type="text/plain; version=0.0.4")

    return app


def mask_key(key: str) -> str:
    if len(key) <= 8:
        return "*" * len(key)
    return f"{key[:4]}...{key[-4:]}"


app = build_app()


def main() -> None:
    settings = get_settings()
    uvicorn.run("aether.gateway:app", host=settings.gateway_host, port=settings.gateway_port)


if __name__ == "__main__":
    main()
