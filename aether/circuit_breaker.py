import time
from dataclasses import dataclass, field
from enum import Enum
from typing import Optional


class CircuitBreakerState(str, Enum):
    CLOSED = "closed"
    OPEN = "open"
    HALF_OPEN = "half_open"


@dataclass
class CircuitBreakerMetrics:
    total_requests: int = 0
    total_failures: int = 0
    consecutive_failures: int = 0
    last_failure_time: Optional[float] = None
    last_state_change_time: float = field(default_factory=time.monotonic)


class CircuitBreaker:
    def __init__(
        self,
        failure_threshold: int = 5,
        success_threshold: int = 2,
        timeout_secs: float = 60.0,
        failure_rate_threshold: float = 0.5,
        sample_size: int = 10,
    ) -> None:
        self.failure_threshold = failure_threshold
        self.success_threshold = success_threshold
        self.timeout_secs = timeout_secs
        self.failure_rate_threshold = failure_rate_threshold
        self.sample_size = sample_size

        self.state = CircuitBreakerState.CLOSED
        self.metrics = CircuitBreakerMetrics()
        self.half_open_successes = 0

    def allow(self) -> bool:
        now = time.monotonic()

        if self.state == CircuitBreakerState.CLOSED:
            return True

        if self.state == CircuitBreakerState.OPEN:
            time_since_open = now - self.metrics.last_state_change_time
            if time_since_open >= self.timeout_secs:
                self._transition_to(CircuitBreakerState.HALF_OPEN, now)
                return True
            return False

        if self.state == CircuitBreakerState.HALF_OPEN:
            return True

        return False

    def record_success(self) -> None:
        now = time.monotonic()
        self.metrics.total_requests += 1
        self.metrics.consecutive_failures = 0

        if self.state == CircuitBreakerState.CLOSED:
            pass
        elif self.state == CircuitBreakerState.HALF_OPEN:
            self.half_open_successes += 1
            if self.half_open_successes >= self.success_threshold:
                self._transition_to(CircuitBreakerState.CLOSED, now)
                self.half_open_successes = 0

    def record_failure(self) -> None:
        now = time.monotonic()
        self.metrics.total_requests += 1
        self.metrics.total_failures += 1
        self.metrics.consecutive_failures += 1
        self.metrics.last_failure_time = now

        if self.state == CircuitBreakerState.CLOSED:
            if self._should_open(now):
                self._transition_to(CircuitBreakerState.OPEN, now)
        elif self.state == CircuitBreakerState.HALF_OPEN:
            self.half_open_successes = 0
            self._transition_to(CircuitBreakerState.OPEN, now)

    def _should_open(self, now: float) -> bool:
        if self.metrics.consecutive_failures >= self.failure_threshold:
            return True

        if self.metrics.total_requests >= self.sample_size:
            failure_rate = self.metrics.total_failures / max(1, self.metrics.total_requests)
            if failure_rate >= self.failure_rate_threshold:
                return True

        return False

    def _transition_to(self, new_state: CircuitBreakerState, now: float) -> None:
        self.state = new_state
        self.metrics.last_state_change_time = now
        if new_state == CircuitBreakerState.HALF_OPEN:
            self.half_open_successes = 0

    def get_state(self) -> str:
        return self.state.value

    def get_metrics(self) -> dict:
        return {
            "state": self.state.value,
            "total_requests": self.metrics.total_requests,
            "total_failures": self.metrics.total_failures,
            "consecutive_failures": self.metrics.consecutive_failures,
            "last_failure_time": self.metrics.last_failure_time,
            "half_open_successes": self.half_open_successes,
        }
