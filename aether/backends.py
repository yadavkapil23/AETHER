import asyncio
import time
from dataclasses import dataclass

import httpx


@dataclass
class InferenceResult:
    output: str
    tokens_generated: int
    prompt_tokens: int
    total_tokens: int
    backend: str
    latency_ms: int


class CircuitBreaker:
    def __init__(self, failure_threshold: int = 5) -> None:
        self.failure_threshold = failure_threshold
        self.failure_count = 0
        self.state = "closed"

    def allow(self) -> bool:
        return self.state != "open"

    def success(self) -> None:
        self.failure_count = 0
        self.state = "closed"

    def failure(self) -> None:
        self.failure_count += 1
        if self.failure_count >= self.failure_threshold:
            self.state = "open"


class LLMBackend:
    def __init__(
        self,
        ollama_endpoint: str,
        huggingface_endpoint: str,
        huggingface_api_key: str | None,
        timeout: float,
    ) -> None:
        self.ollama_endpoint = ollama_endpoint.rstrip("/")
        self.huggingface_endpoint = huggingface_endpoint.rstrip("/")
        self.huggingface_api_key = huggingface_api_key
        self.timeout = timeout
        self.client = httpx.AsyncClient(timeout=timeout)
        self.breakers = {
            "Ollama": CircuitBreaker(),
            "HuggingFace": CircuitBreaker(),
        }

    async def close(self) -> None:
        await self.client.aclose()

    async def infer(
        self,
        model: str,
        prompt: str,
        max_tokens: int,
        temperature: float | None,
        top_p: float | None,
    ) -> InferenceResult:
        attempts = [
            ("Ollama", self._ollama_infer),
            ("HuggingFace", self._hf_infer),
        ]
        last_error: Exception | None = None
        for name, method in attempts:
            if name == "HuggingFace" and not self.huggingface_api_key:
                continue
            if not self.breakers[name].allow():
                continue
            try:
                result = await method(model, prompt, max_tokens, temperature, top_p)
                self.breakers[name].success()
                return result
            except Exception as exc:
                self.breakers[name].failure()
                last_error = exc
        raise RuntimeError(f"all inference backends failed: {last_error}")

    async def _post_json(self, url: str, payload: dict, headers: dict | None = None) -> dict | list:
        last_error: Exception | None = None
        for attempt in range(3):
            try:
                response = await self.client.post(url, json=payload, headers=headers)
                response.raise_for_status()
                return response.json()
            except Exception as exc:
                last_error = exc
                await asyncio.sleep(min(0.1 * (2**attempt), 1.0))
        raise RuntimeError(str(last_error))



    async def _ollama_infer(
        self, model: str, prompt: str, max_tokens: int, temperature: float | None, top_p: float | None
    ) -> InferenceResult:
        start = time.perf_counter()
        data = await self._post_json(
            f"{self.ollama_endpoint}/v1/completions",
            {
                "model": model,
                "prompt": prompt,
                "max_tokens": max_tokens,
                "temperature": temperature,
                "top_p": top_p,
                "stream": False,
            },
        )
        choice = data["choices"][0]
        usage = data.get("usage", {})
        return InferenceResult(
            output=choice.get("text") or choice.get("message", {}).get("content", ""),
            tokens_generated=int(usage.get("completion_tokens", 0)),
            prompt_tokens=int(usage.get("prompt_tokens", 0)),
            total_tokens=int(usage.get("total_tokens", 0)),
            backend="Ollama",
            latency_ms=int((time.perf_counter() - start) * 1000),
        )

    async def _hf_infer(
        self, model: str, prompt: str, max_tokens: int, temperature: float | None, top_p: float | None
    ) -> InferenceResult:
        start = time.perf_counter()
        headers = {"Authorization": f"Bearer {self.huggingface_api_key}"}
        data = await self._post_json(
            f"{self.huggingface_endpoint}/{model}",
            {
                "inputs": prompt,
                "parameters": {
                    "max_length": max_tokens,
                    "temperature": temperature,
                    "top_p": top_p,
                },
            },
            headers=headers,
        )
        first = data[0] if isinstance(data, list) and data else {}
        output = first.get("generated_text") or first.get("summary_text") or ""
        completion_tokens = max(1, len(output) // 4)
        prompt_tokens = max(1, len(prompt) // 4)
        return InferenceResult(
            output=output,
            tokens_generated=completion_tokens,
            prompt_tokens=prompt_tokens,
            total_tokens=prompt_tokens + completion_tokens,
            backend="HuggingFace",
            latency_ms=int((time.perf_counter() - start) * 1000),
        )

    async def health(self) -> dict:
        async def ok(url: str) -> bool:
            try:
                response = await self.client.get(url, timeout=5)
                return response.is_success
            except Exception:
                return False

        return {
            "ollama": {
                "endpoint": self.ollama_endpoint,
                "healthy": await ok(f"{self.ollama_endpoint}/api/tags"),
                "circuit_breaker_state": self.breakers["Ollama"].state,
            },
            "huggingface": {
                "endpoint": self.huggingface_endpoint,
                "healthy": bool(self.huggingface_api_key),
                "circuit_breaker_state": self.breakers["HuggingFace"].state,
            },
        }

