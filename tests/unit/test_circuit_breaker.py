"""Unit tests for aether.resilience.circuit_breaker.CircuitBreaker.

Covers:
- state transitions (CLOSED → OPEN → HALF_OPEN → CLOSED)
- failure handling and failure‑rate threshold
- timeout recovery from OPEN to HALF_OPEN
- success threshold to close from HALF_OPEN
- metric reporting
"""

import asyncio
import time
import pytest

from aether.resilience.circuit_breaker import CircuitBreaker, State

# Helper coroutine that simply returns a value
async def successful_coro():
    return "ok"

# Helper coroutine that raises an exception
async def failing_coro():
    raise RuntimeError("failure")

@pytest.mark.asyncio
async def test_initial_state():
    cb = CircuitBreaker()
    assert cb.state == State.CLOSED
    metrics = await cb.metrics()
    assert metrics["state"] == "closed"
    assert metrics["total_requests"] == 0
    assert metrics["failures"] == 0

@pytest.mark.asyncio
async def test_failure_threshold_triggers_open():
    # Set low sample size and low threshold to trigger quickly
    cb = CircuitBreaker(failure_threshold=0.5, sample_size=4)
    # First three failures -> failure_rate = 0.75 (>0.5)
    for _ in range(3):
        with pytest.raises(RuntimeError):
            await cb.call(failing_coro())
    # After third failure, breaker should be OPEN
    assert cb.state == State.OPEN
    # Further calls should raise immediately without executing the coroutine
    with pytest.raises(RuntimeError):
        await cb.call(successful_coro())
    metrics = await cb.metrics()
    assert metrics["state"] == "open"
    assert metrics["total_requests"] >= 3
    assert metrics["failures"] >= 3

@pytest.mark.asyncio
async def test_timeout_recovery_to_half_open():
    cb = CircuitBreaker(failure_threshold=0.0, sample_size=1, timeout_secs=1)
    # Immediate failure opens the breaker
    with pytest.raises(RuntimeError):
        await cb.call(failing_coro())
    assert cb.state == State.OPEN
    # Wait longer than timeout
    await asyncio.sleep(1.1)
    # Next call should attempt transition to HALF_OPEN before executing
    # Use a successful coroutine – should succeed and move to HALF_OPEN during call
    result = await cb.call(successful_coro())
    assert result == "ok"
    assert cb.state == State.HALF_OPEN

@pytest.mark.asyncio
async def test_half_open_success_threshold_closes():
    cb = CircuitBreaker(failure_threshold=0.0, sample_size=1, timeout_secs=0, success_threshold=2)
    # Trigger open immediately
    with pytest.raises(RuntimeError):
        await cb.call(failing_coro())
    assert cb.state == State.OPEN
    # Move to HALF_OPEN manually (timeout 0).
    await asyncio.sleep(0.01)
    # First successful call in HALF_OPEN increments success counter but stays HALF_OPEN
    await cb.call(successful_coro())
    assert cb.state == State.HALF_OPEN
    # Second successful call should close the breaker
    await cb.call(successful_coro())
    assert cb.state == State.CLOSED

@pytest.mark.asyncio
async def test_half_open_failure_returns_to_open():
    cb = CircuitBreaker(failure_threshold=0.0, sample_size=1, timeout_secs=0, success_threshold=1)
    # Open the breaker
    with pytest.raises(RuntimeError):
        await cb.call(failing_coro())
    # Transition to HALF_OPEN
    await asyncio.sleep(0.01)
    # Failure in HALF_OPEN should revert to OPEN
    with pytest.raises(RuntimeError):
        await cb.call(failing_coro())
    assert cb.state == State.OPEN

@pytest.mark.asyncio
async def test_metrics_reflect_state_changes():
    cb = CircuitBreaker(failure_threshold=0.0, sample_size=1, timeout_secs=0)
    # Initially closed
    m1 = await cb.metrics()
    assert m1["state"] == "closed"
    # Cause failure -> open
    with pytest.raises(RuntimeError):
        await cb.call(failing_coro())
    m2 = await cb.metrics()
    assert m2["state"] == "open"
    # Wait for half‑open transition
    await asyncio.sleep(0.01)
    await cb.call(successful_coro())
    m3 = await cb.metrics()
    # After successful call in half‑open, should be closed again
    assert m3["state"] == "closed"
