from collections import deque
from dataclasses import dataclass
from threading import RLock


@dataclass
class CacheStats:
    total_blocks: int
    allocated_blocks: int
    free_blocks: int
    total_allocated_bytes: int
    total_free_bytes: int
    fragmentation_ratio: float
    hit_rate: float = 0.0


class KVCacheAllocator:
    def __init__(self, total_bytes: int, block_size: int) -> None:
        if total_bytes < block_size:
            raise ValueError("total cache size must be >= block size")
        self.total_bytes = total_bytes
        self.block_size = block_size
        self.total_blocks = total_bytes // block_size
        self._free = deque(range(self.total_blocks))
        self._owners: dict[int, str | None] = {}
        self._lock = RLock()

    def allocate(self, request_id: str, num_blocks: int, owner: str | None = None) -> list[int]:
        if num_blocks <= 0:
            raise ValueError("num_blocks must be positive")
        with self._lock:
            if len(self._free) < num_blocks:
                raise RuntimeError(
                    f"insufficient free blocks: requested={num_blocks} available={len(self._free)}"
                )
            block_owner = owner or request_id
            block_ids = [self._free.popleft() for _ in range(num_blocks)]
            for block_id in block_ids:
                self._owners[block_id] = block_owner
            return block_ids

    def deallocate(self, block_ids: list[int]) -> int:
        with self._lock:
            released = 0
            for block_id in block_ids:
                if block_id in self._owners:
                    del self._owners[block_id]
                    self._free.append(block_id)
                    released += 1
            return released

    def stats(self) -> CacheStats:
        with self._lock:
            allocated = len(self._owners)
            free = len(self._free)
            return CacheStats(
                total_blocks=self.total_blocks,
                allocated_blocks=allocated,
                free_blocks=free,
                total_allocated_bytes=allocated * self.block_size,
                total_free_bytes=free * self.block_size,
                fragmentation_ratio=free / self.total_blocks if self.total_blocks else 0.0,
            )


class Scheduler:
    def __init__(self, cache_bytes: int, block_size: int, node_id: str = "node-1") -> None:
        self.node_id = node_id
        self.allocator = KVCacheAllocator(cache_bytes, block_size)

    def allocate(self, request_id: str, num_blocks: int, owner: str | None = None) -> tuple[list[int], str]:
        return self.allocator.allocate(request_id, num_blocks, owner), self.node_id

    def deallocate(self, block_ids: list[int]) -> int:
        return self.allocator.deallocate(block_ids)

    def stats(self) -> CacheStats:
        return self.allocator.stats()

    def cluster_health(self) -> dict:
        return {
            "healthy": True,
            "total_nodes": 1,
            "healthy_nodes": 1,
            "leader_id": self.node_id,
        }

