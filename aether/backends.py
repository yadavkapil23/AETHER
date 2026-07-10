import time
from dataclasses import dataclass

import httpx

from aether.resilience import CircuitBreaker, RetryError, retry


@dataclass
class InferenceResult:
    output: str
    tokens_generated: int
    prompt_tokens: int
    total_tokens: int
    backend: str
    latency_ms: int


class LLMBackend:
    def __init__(
        self,
        ollama_endpoint: str,
        huggingface_endpoint: str,
        huggingface_api_key: str | None,
        timeout: float,
        health_check_timeout: float = 5.0,
    ) -> None:
        self.ollama_endpoint = ollama_endpoint.rstrip("/")
        self.huggingface_endpoint = huggingface_endpoint.rstrip("/")
        self.huggingface_api_key = huggingface_api_key
        self.timeout = timeout
        self.health_check_timeout = health_check_timeout
        self.client = httpx.AsyncClient(timeout=timeout)
        self.breakers: dict[str, CircuitBreaker] = {
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
        backend: str = "ollama",
    ) -> InferenceResult:
        if backend == "huggingface":
            if not self.huggingface_api_key:
                raise RuntimeError("HuggingFace backend requested but HUGGINGFACE_API_KEY is not configured")
            name, method = "HuggingFace", self._hf_infer
        else:
            name, method = "Ollama", self._ollama_infer

        try:
            return await self.breakers[name].call(
                method(model, prompt, max_tokens, temperature, top_p)
            )
        except Exception as exc:
            raise RuntimeError(f"{name} backend failed: {exc}") from exc

    async def _post_json(self, url: str, payload: dict, headers: dict | None = None) -> dict | list:
        async def _do_post() -> dict | list:
            response = await self.client.post(url, json=payload, headers=headers)
            response.raise_for_status()
            return response.json()

        try:
            return await retry(
                _do_post,
                max_attempts=3,
                initial_backoff=0.1,
                max_backoff=1.0,
                backoff_multiplier=2.0,
                jitter=True,
            )
        except RetryError as exc:
            raise RuntimeError(str(exc.last_exception)) from exc



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
                response = await self.client.get(url, timeout=self.health_check_timeout)
                return response.is_success
            except Exception:
                return False

        return {
            "ollama": {
                "endpoint": self.ollama_endpoint,
                "healthy": await ok(f"{self.ollama_endpoint}/api/tags"),
                "circuit_breaker": await self.breakers["Ollama"].metrics(),
            },
            "huggingface": {
                "endpoint": self.huggingface_endpoint,
                "healthy": bool(self.huggingface_api_key),
                "circuit_breaker": await self.breakers["HuggingFace"].metrics(),
            },
        }

