from typing import Literal

from pydantic import BaseModel, Field


class InferenceRequest(BaseModel):
    model: str
    prompt: str
    max_tokens: int = Field(ge=1, le=32000)
    temperature: float | None = Field(default=None, ge=0.0, le=2.0)
    top_p: float | None = Field(default=None, ge=0.0, le=1.0)
    backend: Literal["ollama", "huggingface"] = "ollama"


class InferenceResponse(BaseModel):
    success: bool
    output: str | None = None
    tokens_generated: int
    latency_ms: int
    backend: str | None = None
    error: str | None = None


class ChatMessage(BaseModel):
    role: str
    content: str


class ChatCompletionRequest(BaseModel):
    model: str
    messages: list[ChatMessage]
    max_tokens: int | None = Field(default=1024, ge=1, le=32000)
    temperature: float | None = Field(default=None, ge=0.0, le=2.0)
    top_p: float | None = Field(default=None, ge=0.0, le=1.0)
    stream: bool | None = False


class AllocateRequest(BaseModel):
    request_id: str
    num_blocks: int = Field(gt=0)
    owner: str | None = None


class DeallocateRequest(BaseModel):
    request_id: str
    block_ids: list[int]

