"""Graceful degradation utilities."""

from enum import Enum
from typing import Callable, Awaitable, Any
import asyncio

class DegradationLevel(Enum):
    HEALTHY = "healthy"
    DEGRADED = "degraded"
    CRITICAL = "critical"

class DegradationManager:
    """
    Manages service degradation state and provides fallback execution.

    The manager tracks whether the system is healthy, degraded, or critical,
    and can execute a primary operation with a fallback fallback.
    """

    def __init__(self) -> None:
        self._level = DegradationLevel.HEALTHY
        self._reason: str = ""
        self._fallback_enabled = True
        self._lock = asyncio.Lock()

    async def execute_with_fallback(
        self,
        primary: Callable[[], Awaitable[Any]],
        fallback: Callable[[], Awaitable[Any]],
    ) -> Any:
        """
        Try to execute ``primary``; if it raises an exception, execute ``fallback``
        (if fallback is enabled) and update degradation state.

        Returns the result of whichever coroutine succeeded.

        Raises the last exception if both primary and fallback fail and fallback
        is disabled, or if fallback is enabled but also fails.
        """
        try:
            result = await primary()
            # Success: reset degradation if we were degraded due to previous failures
            if self._level != DegradationLevel.HEALTHY:
                async with self._lock:
                    self._level = DegradationLevel.HEALTHY
                    self._reason = ""
            return result
        except Exception as exc:
            # Update degradation state
            async with self._lock:
                self._level = DegradationLevel.DEGRADED if self._level == DegradationLevel.HEALTHY else self._level
                self._reason = str(exc)
            if not self._fallback_enabled:
                raise
            try:
                return await fallback()
            except Exception as fallback_exc:
                # Both primary and fallback failed
                async with self._lock:
                    self._level = DegradationLevel.CRITICAL
                    self._reason = f"Primary: {exc}; Fallback: {fallback_exc}"
                raise fallback_exc

    def is_degraded(self) -> bool:
        """Return True if the system is in DEGRADED or CRITICAL state."""
        return self._level in (DegradationLevel.DEGRADED, DegradationLevel.CRITICAL)

    def reason(self) -> str:
        """Return a human‑readable explanation of the current degradation."""
        return self._reason

    @property
    def level(self) -> DegradationLevel:
        return self._level

    def enable_fallback(self) -> None:
        self._fallback_enabled = True

    def disable_fallback(self) -> None:
        self._fallback_enabled = False