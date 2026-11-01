import time

import pytest
from redis.asyncio import Redis


@pytest.mark.asyncio
async def test_redis_rate_limit_script():
    redis = Redis.from_url(
        "redis://localhost:6379/15",
        encoding="utf-8",
        decode_responses=True,
    )

    try:
        if not await redis.ping():
            pytest.skip("Redis is not running")
    except Exception:
        pytest.skip("Redis is not running")

    with open(
        "aether/rate_limit.lua",
        encoding="utf-8",
    ) as file:
        script = file.read()

    key = f"test:rate-limit:{time.time_ns()}"

    try:
        results = []

        for index in range(5):
            result = await redis.eval(
                script,
                1,
                key,
                time.time(),
                1,
                2,
                f"request-{index}",
            )

            results.append(int(result[0]))

        assert results[:2] == [1, 1]
        assert results[2:] == [0, 0, 0]

    finally:
        await redis.delete(key)
        await redis.aclose()