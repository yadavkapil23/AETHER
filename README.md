# AETHER - LLM Gateway

AETHER is a Python/FastAPI reverse proxy for LLM inference. It sits between client applications and model backends (Ollama, HuggingFace), adding backend choice per request, resilience (circuit breaker, retry with backoff, timeouts), authentication, per-client rate limiting, KV-cache block allocation, audit logging, and Prometheus metrics.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how the pieces fit together and the use cases this is built for.

## What It Includes

- FastAPI gateway with `/infer`, `/infer/stream` (SSE), and OpenAI-style `/v1/chat/completions`
- Per-request backend choice: `"ollama"` (default, no auth required) or `"huggingface"` (auth required)
- 3-state circuit breaker per backend (closed / open / half-open)
- Retry with exponential backoff and jitter on transient HTTP failures
- Configurable timeouts per operation (`GATEWAY_TIMEOUT`, `HEALTH_CHECK_TIMEOUT`, `STREAM_TIMEOUT`) plus a reusable `with_timeout()` helper
- API key and JWT verification (JWT issuance is not implemented — see [ARCHITECTURE.md](ARCHITECTURE.md))
- Per-client token-bucket rate limiting, keyed on `X-API-Key`, then `X-Client-Id`, then falling back to IP
- A minimal local web UI at `/` for sending prompts without curl
- In-process KV-cache block scheduler (single machine; not distributed)
- Optional standalone scheduler API (`scheduler_api.py`) — not wired into the main gateway by default
- BLAKE3 audit hash chain (in-memory only unless PostgreSQL is connected)
- Optional PostgreSQL logging for API keys, inference logs, and audit logs
- Prometheus metrics at `/metrics`
- Docker Compose stack for gateway, Postgres, Redis, Prometheus, and Grafana
- Kubernetes manifests in `k8s/` (written, not yet deployed/verified against a real cluster)

## Project Structure

```text
aether/
  audit.py                       BLAKE3 audit hash chain (in-memory)
  backends.py                    async HTTP clients, per-request backend choice, circuit breaker + retry integration
  config.py                      environment-based settings
  database.py                    optional async PostgreSQL integration
  gateway.py                     FastAPI gateway application
  metrics.py                     Prometheus metrics
  models.py                      Pydantic request/response models
  scheduler.py                   KV-cache block allocator (single-node)
  scheduler_api.py               standalone scheduler HTTP API (separate process, not auto-used by gateway.py)
  security.py                    API key/JWT validation, rate limiting, security headers
  static/index.html              local web UI served at "/"
  resilience/
    circuit_breaker.py           3-state circuit breaker
    retry_handler.py             exponential backoff + jitter
    timeout_handler.py           with_timeout() / timeout_decorator

k8s/                             Kubernetes manifests (untested against a real cluster)
tests_python/                    Python tests (unit + live endpoint integration tests)
Dockerfile                       Python gateway container
docker-compose.yml               Python service stack
pyproject.toml                   Python package metadata
requirements.txt                 Runtime dependencies
```

## Quick Start

```powershell
python -m venv .venv
.\\.venv\\Scripts\\Activate.ps1
pip install -r requirements.txt
uvicorn aether.gateway:app --host 0.0.0.0 --port 8080
```

The gateway starts on `http://localhost:8080`.

## Docker

```powershell
docker compose up --build
```

This starts:

- Gateway: `http://localhost:8080`
- PostgreSQL: `localhost:5433`
- Prometheus: `http://localhost:9090`
- Grafana: `http://localhost:3000`
- Redis: `localhost:6379`

## Configuration

Create a `.env` file or export environment variables:

```env
DATABASE_URL=postgresql://postgres:password@localhost:5433/aether_gateway
JWT_SECRET=dev-secret-for-local-testing
API_KEYS=sk-demo123

GATEWAY_HOST=0.0.0.0
GATEWAY_PORT=8080
RATE_LIMIT_RPS=100
GATEWAY_TIMEOUT=30
HEALTH_CHECK_TIMEOUT=5
STREAM_TIMEOUT=120

OLLAMA_ENDPOINT=http://localhost:11434
HUGGINGFACE_API_KEY=
HUGGINGFACE_ENDPOINT=https://api-inference.huggingface.co/models

AETHER_CACHE_BYTES=67108864
AETHER_BLOCK_SIZE=16384
```

PostgreSQL is optional at boot. If the database is unavailable, the gateway still starts and accepts keys from `API_KEYS`, but `POST /api/keys` returns `503`, and `GET /api/keys` / `DELETE /api/keys/{key}` silently return empty/false rather than erroring. See [SECRETS.md](SECRETS.md) for which values are real secrets versus safe local-dev defaults.

## API Examples

Synchronous inference — Ollama backend needs no API key:

```powershell
curl -X POST http://localhost:8080/infer `
  -H "Content-Type: application/json" `
  -d '{ "model": "qwen2.5:0.5b", "prompt": "Explain KV cache reuse.", "max_tokens": 100, "temperature": 0.7, "top_p": 0.9, "backend": "ollama" }'
```

Synchronous inference — HuggingFace backend requires an API key:

```powershell
curl -X POST http://localhost:8080/infer `
  -H "Content-Type: application/json" `
  -H "X-API-Key: sk-demo123" `
  -d '{ "model": "gpt2", "prompt": "Explain KV cache reuse.", "max_tokens": 100, "backend": "huggingface" }'
```

Streaming inference (SSE, Ollama backend only — HuggingFace does not support streaming in this gateway):

```powershell
curl -N -X POST http://localhost:8080/infer/stream `
  -H "Content-Type: application/json" `
  -d '{ "model": "qwen2.5:0.5b", "prompt": "Tell me a story.", "max_tokens": 200, "backend": "ollama" }'
```

Optional `X-Client-Id` header to keep rate limits separate per teammate when no API key is used (e.g. multiple teammates behind the same office IP on the free Ollama path):

```powershell
curl -X POST http://localhost:8080/infer `
  -H "Content-Type: application/json" `
  -H "X-Client-Id: alice" `
  -d '{ "model": "qwen2.5:0.5b", "prompt": "hi", "max_tokens": 20 }'
```

KV-cache allocation (always requires an API key):

```powershell
curl -X POST http://localhost:8080/v1/allocate `
  -H "Content-Type: application/json" `
  -H "X-API-Key: sk-demo123" `
  -d '{ "request_id": "req-1", "num_blocks": 10 }'
```

## Endpoints

| Method | Path | Auth | Description |
| --- | --- | --- | --- |
| GET | `/` | none | Local web UI for sending prompts |
| POST | `/infer` | required only if `backend: "huggingface"` | Synchronous LLM inference |
| POST | `/infer/stream` | required only if `backend: "huggingface"` (and then rejected with 400 — streaming is Ollama-only) | SSE streaming inference |
| POST | `/v1/chat/completions` | required only if `backend: "huggingface"` | OpenAI-style chat completion |
| POST | `/v1/allocate` | required | Allocate KV-cache blocks |
| POST | `/v1/deallocate` | required | Release KV-cache blocks |
| GET | `/v1/stats` | required | Cache statistics |
| GET | `/v1/cluster` | required | Scheduler health (single-node; not a real cluster) |
| GET | `/backends/status` | required | Backend health and circuit breaker state |
| GET | `/health` | none | Full health report |
| GET | `/health/live` | none | Liveness probe |
| GET | `/health/ready` | none | Backend readiness probe |
| GET | `/ready` | none | Database readiness probe |
| GET | `/metrics` | none | Prometheus metrics |
| POST | `/api/keys` | required | Create API key (returns `503` if PostgreSQL is not connected) |
| GET | `/api/keys` | required | List API keys (returns `[]` if PostgreSQL is not connected) |
| DELETE | `/api/keys/{key}` | required | Revoke API key (returns `revoked: false` if PostgreSQL is not connected) |

Auth accepts either `X-API-Key` or a `Bearer` JWT in the `Authorization` header. There is currently no endpoint that issues a JWT — only `.env`'s `API_KEYS` or database-created keys work in practice. See [ARCHITECTURE.md](ARCHITECTURE.md) for why.

Rate limiting applies to every request regardless of backend, keyed on `X-API-Key` first, then `X-Client-Id`, then falling back to source IP.

## Testing

```powershell
python -m pytest tests_python -v
python -m compileall aether tests_python
```

The test suite includes live integration tests (`test_gateway_endpoints.py`) that call the real routes end-to-end, including actual inference against a local Ollama instance. Ollama must be running for those tests to pass — the rest of the suite does not require it.

## License

[LICENSE.md](LICENSE.md)
