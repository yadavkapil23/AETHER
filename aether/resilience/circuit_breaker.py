# Updated circuit breaker implementation with timeout handling and metrics
"""Circuit breaker implementation (3‑state) for production use.

Features:
- CLOSED, OPEN, HALF‑OPEN states
- Failure‑rate based transition to OPEN
- Timeout‑driven transition from OPEN to HALF‑OPEN
- Success‑threshold based transition to CLOSED
- Thread‑safe via ``asyncio.Lock``
- Metrics method exposing current state and counters
"""

from __future__ import annotations

import asyncio
import time
from enum import Enum
from typing import Awaitable, Callable, Any


class State(Enum):
    CLOSED = "closed"
    OPEN = "open"
    HALF_OPEN = "half_open"


class CircuitBreaker:
    """Async circuit breaker supporting 3‑state logic.

    Parameters
    ----------
    failure_threshold: float, optional
        Ratio of failures above which the breaker opens (default 0.5).
    sample_size: int, optional
        Number of recent requests to consider for the failure ratio (default 100).
    timeout_secs: int, optional
        Seconds to stay OPEN before moving to HALF‑OPEN (default 30).
    success_threshold: int, optional
        Consecutive successful calls required in HALF‑OPEN to close the breaker (default 5).
    """

    def __init__(
        self,
        failure_threshold: float = 0.5,
        sample_size: int = 100,
        timeout_secs: int = 30,
        success_threshold: int = 5,
    ) -> None:
        self.failure_threshold = failure_threshold
        self.sample_size = sample_size
        self.timeout_secs = timeout_secs
        self.success_threshold = success_threshold

        self.state: State = State.CLOSED
        self._opened_at: float | None = None  # timestamp when entered OPEN
        self._lock = asyncio.Lock()
        self._request_count = 0
        self._failure_count = 0
        self._successes_in_half_open = 0

    # ---------------------------------------------------------------------
    # Internal helpers
    # ---------------------------------------------------------------------
    async def _transition(self, new_state: State) -> None:
        """Switch state and reset appropriate counters.

        Called with the internal lock held to guarantee thread‑safety.
        """
        async with self._lock:
            self.state = new_state
            if new_state == State.CLOSED:
                self._reset_counters()
                self._opened_at = None
            elif new_state == State.OPEN:
                self._opened_at = time.monotonic()
                # keep existing counters for diagnostics
            elif new_state == State.HALF_OPEN:
                self._successes_in_half_open = 0

    def _reset_counters(self) -> None:
        self._request_count = 0
        self._failure_count = 0
        self._successes_in_half_open = 0

    async def _maybe_half_open(self) -> None:
        """If the breaker is OPEN and the timeout has elapsed, move to HALF‑OPEN.
        """
        if self.state == State.OPEN and self._opened_at is not None:
            elapsed = time.monotonic() - self._opened_at
            if elapsed >= self.timeout_secs:
                await self._transition(State.HALF_OPEN)

    # ---------------------------------------------------------------------
    # Public API
    # ---------------------------------------------------------------------
    async def call(self, coro: Awaitable[Any]) -> Any:
        """Execute ``coro`` respecting circuit‑breaker state.

        Raises
        ------
        RuntimeError
            If the breaker is OPEN and the timeout has not yet expired.
        """
        # Possibly move from OPEN → HALF_OPEN before proceeding
        await self._maybe_half_open()
        if self.state == State.OPEN:
            raise RuntimeError("Circuit breaker is open")

        try:
            result = await coro
        except Exception:
            await self.record_failure()
            raise
        else:
            await self.record_success()
            return result

    async def record_failure(self) -> None:
        """Record a failed request and trigger state transitions if needed.
        """
        async with self._lock:
            self._request_count += 1
            self._failure_count += 1
            if self.state == State.CLOSED:
                if self._request_count >= self.sample_size:
                    failure_rate = self._failure_count / self._request_count
                    if failure_rate > self.failure_threshold:
                        await self._transition(State.OPEN)
            elif self.state == State.HALF_OPEN:
                # any failure during HALF‑OPEN forces a revert to OPEN
                await self._transition(State.OPEN)

    async def record_success(self) -> None:
        """Record a successful request and transition states when appropriate.
        """
        async with self._lock:
            self._request_count += 1
            if self.state == State.HALF_OPEN:
                self._successes_in_half_open += 1
                if self._successes_in_half_open >= self.success_threshold:
                    await self._transition(State.CLOSED)

    async def metrics(self) -> dict[str, Any]:
        """Return a snapshot of breaker metrics.

        The dictionary includes:
        - ``state``: current state name
        - ``total_requests``: number of calls observed
        - ``failures``: number of recorded failures
        - ``opened_at``: timestamp (monotonic) when entered OPEN (or ``None``)
        """
        async with self._lock:
            return {
                "state": self.state.value,
                "total_requests": self._request_count,
                "failures": self._failure_count,
                "opened_at": self._opened_at,
            }

    # ---------------------------------------------------------------------
    # Convenience wrapper
    # ---------------------------------------------------------------------
    async def protect(self, func: Callable[..., Awaitable[Any]], *args, **kwargs) -> Any:
        """Execute ``func`` with the breaker applied.

        ``func`` is called as ``await func(*args, **kwargs)``.
        """
        return await self.call(func(*args, **kwargs))


from enum import Enum
import asyncio

class State(Enum):
    CLOSED = "closed"
    OPEN = "open"
    HALF_OPEN = "half_open"

class CircuitBreaker:
    def __init__(self, failure_threshold: float = 0.5, sample_size: int = 100,
                 timeout_secs: int = 30, success_threshold: int = 5):
        self.failure_threshold = failure_threshold
        self.sample_size = sample_size
        self.timeout_secs = timeout_secs
        self.success_threshold = success_threshold
        self.state = State.CLOSED
        self._lock = asyncio.Lock()
        self._request_count = 0
        self._failure_count = 0
        self._successes_in_half_open = 0

    async def _transition(self, new_state: State):
        async with self._lock:
            self.state = new_state
            if new_state == State.CLOSED:
                self._reset_counters()
            elif new_state == State.HALF_OPEN:
                self._successes_in_half_open = 0

    def _reset_counters(self):
        self._request_count = 0
        self._failure_count = 0
        self._successes_in_half_open = 0

    async def call(self, coro):
        """Execute coroutine respecting circuit breaker state."""
        if self.state == State.OPEN:
            raise RuntimeError("Circuit breaker is open")
        try:
            result = await coro
        except Exception:
            await self.record_failure()
            raise
        else:
            await self.record_success()
            return result

    async def record_failure(self):
        async with self._lock:
            self._request_count += 1
            self._failure_count += 1
            if self.state == State.CLOSED:
                if self._request_count >= self.sample_size:
                    failure_rate = self._failure_count / self._request_count
                    if failure_rate > self.failure_threshold:
                        await self._transition(State.OPEN)
            elif self.state == State.HALF_OPEN:
                await self._transition(State.OPEN)

    async def record_success(self):
        async with self._lock:
            self._request_count += 1
            if self.state == State.HALF_OPEN:
                self._successes_in_half_open += 1
                if self._successes_in_half_open >= self.success_threshold:
                    await self._transition(State.CLOSED)

    async def metrics(self):
        async with self._lock:
            return {
                "state": self.state.value,
                "total_requests": self._request_count,
                "failures": self._failure_count,
            }
