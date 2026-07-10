"""Bulkhead pattern implementation using asyncio.Semaphore per resource."""

import asyncio
from collections import defaultdict
from typing import Dict, NamedTuple

class BulkheadMetrics(NamedTuple):
    current_concurrent: int
    max_concurrent: int
    rejected_count: int

class Bulkhead:
    """
    Limits concurrent executions for a named resource (e.g., a backend service).

    Each resource gets its own asyncio.Semaphore. Attempts to acquire when
    the semaphore is exhausted are rejected and counted.
    """

    def __init__(self, max_concurrent_per_resource: int = 100) -> None:
        self._max_concurrent = max_concurrent_per_resource
        self._semaphores: dict[str, asyncio.Semaphore] = defaultdict(
            lambda: asyncio.Semaphore(self._max_concurrent)
        )
        self._rejected: dict[str, int] = defaultdict(int)
        self._lock = asyncio.Lock()

    async def acquire(self, resource: str) -> None:
        """
        Acquire a permit for the given resource.

        Raises RuntimeError if the resource is exhausted (i.e., no permits left).
        """
        async with self._lock:
            sem = self._semaphores[resource]
        if sem.locked() and sem._value == 0:
            # No permits available
            self._rejected[resource] += 1
            raise RuntimeError(f"Bulkhead limit exceeded for resource '{resource}'")
        await sem.acquire()

    def release(self, resource: str) -> None:
        """Release a permit for the given resource."""
        sem = self._semaphores[resource]
        sem.release()

    def metrics(self, resource: str | None = None) -> dict[str, BulkheadMetrics]:
        """
        Return metrics for all resources or a specific one.

        If ``resource`` is provided, returns a dict with that single entry.
        """
        result: dict[str, BulkheadMetrics] = {}
        resources = [resource] if resource is not None else list(self._semaphores.keys())
        for res in resources:
            sem = self._semaphores[res]
            current = self._max_concurrent - sem._value
            result[res] = BulkheadMetrics(
                current_concurrent=current,
                max_concurrent=self._max_concurrent,
                rejected_count=self._rejected[res],
            )
        return result