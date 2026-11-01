from redis.asyncio import Redis

class RedisClient:
    def __init__(self, url: str) -> None:
        self.redis = Redis.from_url(
            url,
            encoding="utf-8",
            decode_responses=True,
        )

    async def ping(self) -> bool:
        try:
            await self.redis.ping()
            return True
        except Exception:
            return False

    async def close(self) -> None:
        await self.redis.aclose()