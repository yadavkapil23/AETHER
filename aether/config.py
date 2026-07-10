from functools import lru_cache
from typing import Literal

from pydantic import Field
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file=".env", env_file_encoding="utf-8", extra="ignore")

    gateway_host: str = Field("0.0.0.0", alias="GATEWAY_HOST")
    gateway_port: int = Field(8080, alias="GATEWAY_PORT")
    database_url: str = Field(
        "postgresql://postgres:password@localhost:5433/aether_gateway",
        alias="DATABASE_URL",
    )
    jwt_secret: str = Field("change-me-in-production", alias="JWT_SECRET")
    api_keys: str = Field("sk-demo123", alias="API_KEYS")
    rate_limit_rps: int = Field(100, alias="RATE_LIMIT_RPS")
    gateway_timeout: float = Field(30.0, alias="GATEWAY_TIMEOUT")
    health_check_timeout: float = Field(5.0, alias="HEALTH_CHECK_TIMEOUT")
    stream_timeout: float = Field(120.0, alias="STREAM_TIMEOUT")
    gateway_cache_size: int = Field(1000, alias="GATEWAY_CACHE_SIZE")

    ollama_endpoint: str = Field("http://localhost:11434", alias="OLLAMA_ENDPOINT")
    huggingface_endpoint: str = Field(
        "https://api-inference.huggingface.co/models",
        alias="HUGGINGFACE_ENDPOINT",
    )
    huggingface_api_key: str | None = Field(None, alias="HUGGINGFACE_API_KEY")

    scheduler_url: str = Field("http://localhost:50052", alias="SCHEDULER_URL")
    scheduler_mode: Literal["inprocess", "remote"] = Field("inprocess", alias="SCHEDULER_MODE")
    cache_bytes: int = Field(64 * 1024 * 1024, alias="AETHER_CACHE_BYTES")
    block_size: int = Field(16 * 1024, alias="AETHER_BLOCK_SIZE")

    log_level: str = Field("info", alias="LOG_LEVEL")

    @property
    def fallback_api_keys(self) -> set[str]:
        return {key.strip() for key in self.api_keys.split(",") if key.strip()}


@lru_cache
def get_settings() -> Settings:
    return Settings()
