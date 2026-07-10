"""Bulkhead concurrency control using asyncio.Semaphore per backend."""
import asyncio
from collections import defaultdict

class Bulkhead:
    def __init__(self, max_concurrent_per_backend: int = 100):
        self.max_concurrent = max_concurrent_per_backend
        self.semaphores = defaultdict(lambda: asyncio.Semaphore(self.max_concurrent))
        self.rejected_counts = defaultdict(int)

    async def acquire(self, backend_name: str):
        sem = self.semaphores[backend_name]
        if sem.locked() and sem._value == 0:
            self.rejected_counts[backend_name] += 1
            raise RuntimeError("Too Many Requests: bulkhead limit reached for {}".format(backend_name))
        await sem.acquire()
        return sem

    def release(self, backend_name: str):
        sem = self.semaphores[backend_name]
        sem.release()

    def metrics(self):
        return {
            name: {
                "current_concurrent": self.max_concurrent - sem._value,
                "max_concurrent": self.max_concurrent,
                "rejected_count": self.rejected_counts[name],
            }
            for name, sem in self.semaphores.items()
        }
