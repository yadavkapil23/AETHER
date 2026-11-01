from prometheus_client import CollectorRegistry, Counter, Gauge, Histogram, generate_latest


class Metrics:
    def __init__(self) -> None:
        self.registry = CollectorRegistry()
        self.inference_total = Counter(
            "aether_inference_total",
            "Total inference requests",
            ["model", "backend", "status"],
            registry=self.registry,
        )
        self.inference_latency = Histogram(
            "aether_inference_latency_ms",
            "Inference latency in milliseconds",
            ["model", "backend"],
            registry=self.registry,
        )
        self.tokens_generated = Counter(
            "aether_tokens_generated_total",
            "Generated tokens",
            ["model", "backend"],
            registry=self.registry,
        )
        self.rate_limited_total = Counter(
            "aether_rate_limited_total",
            "Requests rejected by token-bucket limiter",
            registry=self.registry,
        )
        self.cache_allocated_blocks = Gauge(
            "aether_cache_allocated_blocks",
            "KV-cache allocated blocks",
            registry=self.registry,
        )

    def record_inference_success(
        self, model: str, backend: str, latency_ms: int, tokens: int
    ) -> None:
        self.inference_total.labels(model, backend, "success").inc()
        self.inference_latency.labels(model, backend).observe(latency_ms)
        self.tokens_generated.labels(model, backend).inc(tokens)

    def record_inference_error(self, model: str, backend: str, reason: str) -> None:
        self.inference_total.labels(model, backend or "unknown", reason).inc()

    def export(self) -> bytes:
        return generate_latest(self.registry)
