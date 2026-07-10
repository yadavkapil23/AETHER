"""Graceful degradation utilities."""
from enum import Enum
from typing import Callable, Awaitable, Any

class Level(Enum):
    HEALTHY = "healthy"
    DEGRADED = "degraded"
    CRITICAL = "critical"

class DegradationManager:
    def __init__(self):
        self.level = Level.HEALTHY
        self._reason = ""
        self.fallback_enabled = True

    async def execute_with_fallback(self, primary: Callable[[], Awaitable[Any]], fallback: Callable[[], Awaitable[Any]]) -> Any:
        try:
            return await primary()
        except Exception as exc:
            self.level = Level.DEGRADED
            self._reason = str(exc)
            if self.fallback_enabled:
                return await fallback()
            raise

    def is_degraded(self) -> bool:
        return self.level != Level.HEALTHY

    def reason(self) -> str:
        return self._reason
