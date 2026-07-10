import time

import pytest

from aether.circuit_breaker import CircuitBreaker, CircuitBreakerState


def test_circuit_breaker_starts_closed():
    cb = CircuitBreaker()
    assert cb.get_state() == CircuitBreakerState.CLOSED.value
    assert cb.allow() is True


def test_circuit_breaker_opens_on_consecutive_failures():
    cb = CircuitBreaker(failure_threshold=3)
    assert cb.allow() is True

    cb.record_failure()
    cb.record_failure()
    assert cb.get_state() == CircuitBreakerState.CLOSED.value

    cb.record_failure()
    assert cb.get_state() == CircuitBreakerState.OPEN.value
    assert cb.allow() is False


def test_circuit_breaker_transitions_to_half_open():
    cb = CircuitBreaker(failure_threshold=2, timeout_secs=0.1)

    cb.record_failure()
    cb.record_failure()
    assert cb.get_state() == CircuitBreakerState.OPEN.value

    time.sleep(0.2)
    assert cb.allow() is True
    assert cb.get_state() == CircuitBreakerState.HALF_OPEN.value


def test_circuit_breaker_closes_after_half_open_successes():
    cb = CircuitBreaker(failure_threshold=2, success_threshold=2, timeout_secs=0.1)

    cb.record_failure()
    cb.record_failure()
    assert cb.get_state() == CircuitBreakerState.OPEN.value

    time.sleep(0.2)
    cb.allow()
    assert cb.get_state() == CircuitBreakerState.HALF_OPEN.value

    cb.record_success()
    assert cb.get_state() == CircuitBreakerState.HALF_OPEN.value

    cb.record_success()
    assert cb.get_state() == CircuitBreakerState.CLOSED.value


def test_circuit_breaker_reopens_on_half_open_failure():
    cb = CircuitBreaker(failure_threshold=2, timeout_secs=0.1)

    cb.record_failure()
    cb.record_failure()
    assert cb.get_state() == CircuitBreakerState.OPEN.value

    time.sleep(0.2)
    cb.allow()
    assert cb.get_state() == CircuitBreakerState.HALF_OPEN.value

    cb.record_failure()
    assert cb.get_state() == CircuitBreakerState.OPEN.value


def test_circuit_breaker_failure_rate_tracking():
    cb = CircuitBreaker(failure_threshold=10, failure_rate_threshold=0.5, sample_size=10)

    for _ in range(5):
        cb.record_success()
    for _ in range(5):
        cb.record_failure()

    failure_rate = cb.metrics.total_failures / cb.metrics.total_requests
    assert failure_rate == 0.5
    assert cb.get_state() == CircuitBreakerState.OPEN.value


def test_circuit_breaker_reset_on_success():
    cb = CircuitBreaker(failure_threshold=3)

    cb.record_failure()
    cb.record_failure()
    assert cb.metrics.consecutive_failures == 2

    cb.record_success()
    assert cb.metrics.consecutive_failures == 0
    assert cb.get_state() == CircuitBreakerState.CLOSED.value


def test_circuit_breaker_metrics():
    cb = CircuitBreaker()

    cb.record_success()
    cb.record_failure()
    cb.record_failure()

    metrics = cb.get_metrics()
    assert metrics["total_requests"] == 3
    assert metrics["total_failures"] == 2
    assert metrics["consecutive_failures"] == 2
    assert metrics["state"] == CircuitBreakerState.CLOSED.value
