import time

import uvicorn
from fastapi import FastAPI, HTTPException

from aether.config import get_settings
from aether.models import AllocateRequest, DeallocateRequest
from aether.scheduler import Scheduler

settings = get_settings()
scheduler = Scheduler(settings.cache_bytes, settings.block_size)
app = FastAPI(title="AETHER Python Scheduler", version="0.1.0")


@app.post("/v1/allocate")
async def allocate(req: AllocateRequest):
    start = time.perf_counter()
    try:
        block_ids, node_id = scheduler.allocate(req.request_id, req.num_blocks, req.owner)
        return {
            "success": True,
            "block_ids": block_ids,
            "latency_ms": int((time.perf_counter() - start) * 1000),
            "node_id": node_id,
            "error": None,
        }
    except Exception as exc:
        raise HTTPException(status_code=500, detail=str(exc)) from exc


@app.post("/v1/deallocate")
async def deallocate(req: DeallocateRequest):
    start = time.perf_counter()
    return {
        "success": True,
        "count": scheduler.deallocate(req.block_ids),
        "latency_ms": int((time.perf_counter() - start) * 1000),
        "error": None,
    }


@app.get("/v1/stats")
async def stats():
    return scheduler.stats().__dict__


@app.get("/v1/cluster")
async def cluster():
    return scheduler.cluster_health()


@app.get("/health")
async def health():
    return {"status": "healthy", **scheduler.cluster_health()}


def main() -> None:
    uvicorn.run("aether.scheduler_api:app", host="0.0.0.0", port=50052)


if __name__ == "__main__":
    main()
