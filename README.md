# AETHER - Python LLM Gateway

AETHER is a Python/FastAPI LLM gateway and inference orchestration service. It sits between client applications and model backends, adding authentication, rate limiting, backend fallback, KV-cache block allocation, audit logging, and Prometheus metrics.

## What It Includes

- FastAPI gateway with `/infer`, `/infer/stream`, and OpenAI-style `/v1/chat/completions`
- Backend fallback order: `vLLM -> llama.cpp HTTP -> Ollama -> HuggingFace`
- API key and JWT authentication
- Per-client token-bucket rate limiting
- In-process KV-cache block scheduler
- Optional standalone scheduler API
- BLAKE3 audit hash chain
- Optional PostgreSQL logging for API keys, inference logs, and audit logs
- Prometheus metrics at `/metrics`
- Docker Compose stack for gateway, Postgres, Redis, Prometheus, and Grafana

## Project Structure

```text
aether/
  audit.py          BLAKE3 audit hash chain
  backends.py       async HTTP clients and fallback routing
  config.py         environment-based settings
  database.py       optional async PostgreSQL integration
  gateway.py        FastAPI gateway application
  metrics.py        Prometheus metrics
  models.py         Pydantic request/response models
  scheduler.py      KV-cache block allocator
  scheduler_api.py  standalone scheduler HTTP API
  security.py       API key/JWT auth, rate limiting, headers

tests_python/       Python unit tests
Dockerfile          Python gateway container
docker-compose.yml  Python service stack
pyproject.toml      Python package metadata
requirements.txt    Runtime dependencies
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

VLLM_ENDPOINT=http://localhost:8000
LLAMACPP_ENDPOINT=http://localhost:8001
OLLAMA_ENDPOINT=http://localhost:11434
HUGGINGFACE_API_KEY=
HUGGINGFACE_ENDPOINT=https://api-inference.huggingface.co/models

AETHER_CACHE_BYTES=67108864
AETHER_BLOCK_SIZE=16384
```

PostgreSQL is optional at boot. If the database is unavailable, the gateway still starts and accepts keys from `API_KEYS`.

## API Examples

Synchronous inference:

```powershell
curl -X POST http://localhost:8080/infer `
  -H "Content-Type: application/json" `
  -H "X-API-Key: sk-demo123" `
  -d '{ "model": "qwen2.5:0.5b", "prompt": "Explain KV cache reuse.", "max_tokens": 100, "temperature": 0.7, "top_p": 0.9 }'
```

Streaming inference:

```powershell
curl -N -X POST http://localhost:8080/infer/stream `
  -H "Content-Type: application/json" `
  -H "X-API-Key: sk-demo123" `
  -d '{ "model": "qwen2.5:0.5b", "prompt": "Tell me a story.", "max_tokens": 200 }'
```

KV-cache allocation:

```powershell
curl -X POST http://localhost:8080/v1/allocate `
  -H "Content-Type: application/json" `
  -H "X-API-Key: sk-demo123" `
  -d '{ "request_id": "req-1", "num_blocks": 10 }'
```

## Endpoints

| Method | Path | Description |
| --- | --- | --- |
| POST | `/infer` | Synchronous LLM inference |
| POST | `/infer/stream` | SSE streaming inference via vLLM-compatible backend |
| POST | `/v1/chat/completions` | OpenAI-style chat completion response |
| POST | `/v1/allocate` | Allocate KV-cache blocks |
| POST | `/v1/deallocate` | Release KV-cache blocks |
| GET | `/v1/stats` | Cache statistics |
| GET | `/v1/cluster` | Scheduler health |
| GET | `/backends/status` | Backend health and circuit states |
| GET | `/health` | Full health report |
| GET | `/health/live` | Liveness probe |
| GET | `/health/ready` | Backend readiness probe |
| GET | `/ready` | Database readiness probe |
| GET | `/metrics` | Prometheus metrics |
| POST | `/api/keys` | Create API key |
| GET | `/api/keys` | List API keys |
| DELETE | `/api/keys/{key}` | Revoke API key |

## Testing

```powershell
python -m pytest tests_python
python -m compileall aether tests_python
```

## License

Internal project.
