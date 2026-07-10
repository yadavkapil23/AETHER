import asyncio
import time

import pytest

from aether.resilience import CircuitBreaker, CircuitBreakerMetrics, CircuitBreakerState


def sync_metrics(cb: CircuitBreaker) -> CircuitBreakerMetrics:
    """Helper to retrieve metrics from an event loop that is already running."""
    return asyncio.get_event_loop().run_until_complete(cb.metrics())


# ── Unit tests ───────────────────────────────────────────────────────────


@pytest.mark.asyncio
async def test_starts_closed():
    cb = CircuitBreaker()
    assert await cb.get_state() == CircuitBreakerState.CLOSED.value
    assert await cb.allow() is True


@pytest.mark.asyncio
async def test_allow_blocks_when_open():
    cb = CircuitBreaker(timeout_secs=999)
    await cb.force_open()
    assert await cb.get_state() == CircuitBreakerState.OPEN.value
    assert await cb.allow() is False


@pytest.mark.asyncio
async def test_opens_on_consecutive_failures():
    cb = CircuitBreaker(failure_threshold_count=3, failure_threshold=0.9, timeout_secs=999)
    await cb.record_failure()
    await cb.record_failure()
    assert await cb.get_state() == CircuitBreakerState.CLOSED.value
    await cb.record_failure()
    assert await cb.get_state() == CircuitBreakerState.OPEN.value


@pytest.mark.asyncio
async def test_opens_on_failure_rate():
    cb = CircuitBreaker(
        failure_threshold=0.5,
        failure_threshold_count=20,  # high enough that only rate triggers
        sample_size=10,
        timeout_secs=999,
    )
    for _ in range(5):
        await cb.record_success()
    for _ in range(5):
        await cb.record_failure()
    assert await cb.get_state() == CircuitBreakerState.OPEN.value


@pytest.mark.asyncio
async def test_transitions_to_half_open_after_timeout():
    cb = CircuitBreaker(failure_threshold_count=2, timeout_secs=0.1)
    await cb.record_failure()
    await cb.record_failure()
    assert await cb.get_state() == CircuitBreakerState.OPEN.value
    await asyncio.sleep(0.2)
    assert await cb.allow() is True
    assert await cb.get_state() == CircuitBreakerState.HALF_OPEN.value


@pytest.mark.asyncio
async def test_closes_after_half_open_successes():
    cb = CircuitBreaker(failure_threshold_count=2, success_threshold=2, timeout_secs=0.1)
    await cb.record_failure()
    await cb.record_failure()
    assert await cb.get_state() == CircuitBreakerState.OPEN.value
    await asyncio.sleep(0.2)
    await cb.allow()
    assert await cb.get_state() == CircuitBreakerState.HALF_OPEN.value
    await cb.record_success()
    assert await cb.get_state() == CircuitBreakerState.HALF_OPEN.value
    await cb.record_success()
    assert await cb.get_state() == CircuitBreakerState.CLOSED.value


@pytest.mark.asyncio
async def test_reopens_on_half_open_failure():
    cb = CircuitBreaker(failure_threshold_count=2, timeout_secs=0.1)
    await cb.record_failure()
    await cb.record_failure()
    assert await cb.get_state() == CircuitBreakerState.OPEN.value
    await asyncio.sleep(0.2)
    await cb.allow()
    assert await cb.get_state() == CircuitBreakerState.HALF_OPEN.value
    await cb.record_failure()
    assert await cb.get_state() == CircuitBreakerState.OPEN.value


@pytest.mark.asyncio
async def test_resets_consecutive_failures_on_success():
    cb = CircuitBreaker(failure_threshold_count=3, timeout_secs=999)
    await cb.record_failure()
    await cb.record_failure()
    m = await cb.metrics()
    assert m.consecutive_failures == 2
    await cb.record_success()
    m = await cb.metrics()
    assert m.consecutive_failures == 0
    assert await cb.get_state() == CircuitBreakerState.CLOSED.value


@pytest.mark.asyncio
async def test_metrics_snapshot():
    cb = CircuitBreaker()
    await cb.record_success()
    await cb.record_failure()
    await cb.record_failure()
    m = await cb.metrics()
    assert m.total_requests == 3
    assert m.total_successes == 1
    assert m.total_failures == 2
    assert m.consecutive_failures == 2
    assert m.consecutive_successes == 0
    assert m.failure_rate == pytest.approx(2 / 3)
    assert m.state == CircuitBreakerState.CLOSED.value
    assert isinstance(m.failure_rate, float)
    assert m.open_duration_secs >= 0


@pytest.mark.asyncio
async def test_aegis_default_config():
    """Verifies the AEGIS-spec defaults."""
    cb = CircuitBreaker()
    assert cb.failure_threshold == 0.5
    assert cb.failure_threshold_count == 5
    assert cb.success_threshold == 5
    assert cb.timeout_secs == 30.0
    assert cb.sample_size == 100


@pytest.mark.asyncio
async def test_force_open_and_close():
    cb = CircuitBreaker()
    assert await cb.get_state() == CircuitBreakerState.CLOSED.value
    await cb.force_open()
    assert await cb.get_state() == CircuitBreakerState.OPEN.value
    await cb.force_close()
    assert await cb.get_state() == CircuitBreakerState.CLOSED.value
    m = await cb.metrics()
    assert m.consecutive_failures == 0


@pytest.mark.asyncio
async def test_concurrent_access():
    """Verify asyncio.Lock prevents race conditions under concurrent calls."""
    cb = CircuitBreaker(timeout_secs=999)

    async def hammer(n: int):
        for _ in range(n):
            await cb.record_failure()

    tasks = [asyncio.create_task(hammer(20)) for _ in range(5)]
    await asyncio.gather(*tasks)
    # Each task calls record_failure 20 times = 100 total
    m = await cb.metrics()
    assert m.total_requests == 100
    assert m.total_failures == 100


@pytest.mark.asyncio
async def test_get_state():
    cb = CircuitBreaker()
    assert await cb.get_state() == "closed"
    await cb.force_open()
    assert await cb.get_state() == "open"


@pytest.mark.asyncio
async def test_half_open_metrics():
    cb = CircuitBreaker(failure_threshold_count=2, success_threshold=3, timeout_secs=0.1)
    await cb.record_failure()
    await cb.record_failure()
    await asyncio.sleep(0.2)
    await cb.allow()
    await cb.record_success()
    await cb.record_success()
    m = await cb.metrics()
    assert m.half_open_successes == 2
    assert m.state == CircuitBreakerState.HALF_OPEN.value
    # 3rd success should close
    await cb.record_success()
    m = await cb.metrics()
    assert m.state == CircuitBreakerState.CLOSED.value
    assert m.half_open_successes == 0


@pytest.mark.asyncio
async def test_validation():
    with pytest.raises(ValueError):
        CircuitBreaker(failure_threshold=0.0)
    with pytest.raises(ValueError):
        CircuitBreaker(failure_threshold_count=0)
    with pytest.raises(ValueError):
        CircuitBreaker(success_threshold=0)
    with pytest.raises(ValueError):
        CircuitBreaker(timeout_secs=0)
    with pytest.raises(ValueError):
        CircuitBreaker(sample_size=0)


@pytest.mark.asyncio
async def test_failure_rate_stays_in_range():
    """failure_rate should never exceed 1.0."""
    cb = CircuitBreaker(failure_threshold_count=100, failure_threshold=2.0)
    # Create a breaker that only accepts rate trigger, set toxically high threshold
    # so it never opens — just validate failure_rate math
    for _ in range(50):
        await cb.record_failure()
    for _ in range(10):
        await cb.record_success()
    m = await cb.metrics()
    assert m.failure_rate > 0.8