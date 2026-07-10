"""Retry handler with exponential backoff and optional jitter."""
import asyncio
import random
from typing import Callable, Coroutine, Any

class RetryHandler:
    def __init__(self, max_attempts: int = 5, initial_backoff_ms: int = 100,
                 max_backoff_ms: int = 2000, backoff_multiplier: float = 2.0,
                 enable_jitter: bool = True):
        self.max_attempts = max_attempts
        self.initial_backoff_ms = initial_backoff_ms
        self.max_backoff_ms = max_backoff_ms
        self.backoff_multiplier = backoff_multiplier
        self.enable_jitter = enable_jitter
        self.transient_statuses = {500, 502, 503, 504}
        self.non_transient_statuses = {401, 403, 404}

    async def run(self, func: Callable[..., Coroutine[Any, Any, Any]], *args, **kwargs) -> Any:
        attempt = 0
        backoff = self.initial_backoff_ms
        while attempt < self.max_attempts:
            try:
                return await func(*args, **kwargs)
            except Exception as exc:
                # If exc has a status_code attribute (e.g., HTTPException), inspect it
                status = getattr(exc, "status_code", None)
                if status in self.non_transient_statuses:
                    raise
                attempt += 1
                if attempt >= self.max_attempts:
                    raise
                # compute backoff with jitter
                delay_ms = backoff
                if self.enable_jitter:
                    jitter_factor = random.uniform(0.8, 1.2)
                    delay_ms = int(delay_ms * jitter_factor)
                await asyncio.sleep(delay_ms / 1000.0)
                backoff = min(int(backoff * self.backoff_multiplier), self.max_backoff_ms)
