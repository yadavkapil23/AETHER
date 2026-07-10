"""Timeout handler utility using asyncio.wait_for."""
import asyncio
from typing import Any, Coroutine

DEFAULT_TIMEOUT_MS = 5000

async def with_timeout(coro: Coroutine[Any, Any, Any], timeout_ms: int = DEFAULT_TIMEOUT_MS) -> Any:
    """Run coroutine with a timeout. Raises asyncio.TimeoutError on exceed."""
    timeout_sec = timeout_ms / 1000.0
    return await asyncio.wait_for(coro, timeout=timeout_sec)
