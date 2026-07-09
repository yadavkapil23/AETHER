import pytest
from pydantic import ValidationError

from aether.models import InferenceRequest


def test_inference_request_accepts_ollama_model_name():
    req = InferenceRequest(
        model="qwen2.5:0.5b",
        prompt="hello",
        max_tokens=10,
        temperature=0.7,
        top_p=0.9,
    )

    assert req.model == "qwen2.5:0.5b"


def test_inference_request_rejects_invalid_token_count():
    with pytest.raises(ValidationError):
        InferenceRequest(model="qwen", prompt="hello", max_tokens=0)
