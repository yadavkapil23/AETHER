# AETHER Project Runbook

This guide explains how to start, verify, use, and stop the AETHER gateway on Windows PowerShell.

## Requirements

- Python 3.10 or newer
- Docker Desktop running
- Ollama installed and running if real local-model inference is required
- A model installed in Ollama, for example `qwen2.5:0.5b`

Open PowerShell and move to the project directory:

```powershell
cd C:\Users\ky805\OneDrive\Desktop\AETHER\AETHER-main
```

## Install Python dependencies

Create and activate a virtual environment:

```powershell
python -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install -r requirements.txt
```

If PowerShell blocks activation, run the project with the system Python after installing the requirements.

## Start Ollama

Check that Ollama is available:

```powershell
ollama list
```

If the required model is missing, install it:

```powershell
ollama pull qwen2.5:0.5b
```

Test Ollama directly:

```powershell
$body = '{ "model": "qwen2.5:0.5b", "prompt": "Say hello.", "stream": false }'
Invoke-RestMethod http://127.0.0.1:11434/api/generate `
  -Method Post `
  -ContentType "application/json" `
  -Body $body
```

## Start infrastructure with Docker Compose

Make sure Docker Desktop is running, then start the stack:

```powershell
docker compose up -d --build
```

Do not also start a local gateway on port `8080` while the Compose gateway is running. Choose either the Compose gateway or the local Python gateway section below.

The stack contains:

- Gateway: `http://localhost:8080`
- PostgreSQL: `localhost:5433`
- Redis: `localhost:6379`
- Prometheus: `http://localhost:9090`
- Grafana: `http://localhost:3000`

Check container status:

```powershell
docker compose ps
```

Check Redis:

```powershell
docker compose exec redis redis-cli ping
```

Expected output:

```text
PONG
```

Check PostgreSQL:

```powershell
docker compose exec postgres pg_isready -U postgres -d aether_gateway
```

Expected output includes:

```text
accepting connections
```

Check Prometheus:

```powershell
Invoke-WebRequest http://127.0.0.1:9090/-/ready
```

Open Prometheus at `http://localhost:9090`.

Open Grafana at `http://localhost:3000`.

The default local Grafana credentials from `docker-compose.yml` are:

```text
username: admin
password: admin
```

## Start the gateway locally instead of in Docker

Use this option when Ollama is installed on Windows and the gateway should connect directly to it.

Set the project on Python's import path:

```powershell
$env:PYTHONPATH = (Get-Location).Path
```

Set local service configuration:

```powershell
$env:REDIS_URL = "redis://127.0.0.1:6379/0"
$env:DATABASE_URL = "postgresql://postgres:password@127.0.0.1:5433/aether_gateway"
$env:API_KEYS = "sk-demo123"
$env:OLLAMA_ENDPOINT = "http://127.0.0.1:11434"
```

Start the gateway:

```powershell
python -m uvicorn aether.gateway:app --host 0.0.0.0 --port 8080
```

Leave this PowerShell window running. Open a second PowerShell window for verification and API requests.

## Verify the gateway

Liveness:

```powershell
Invoke-WebRequest http://127.0.0.1:8080/health/live
```

Full health status:

```powershell
Invoke-RestMethod http://127.0.0.1:8080/health
```

Metrics:

```powershell
Invoke-WebRequest http://127.0.0.1:8080/metrics
```

## Run Ollama inference through AETHER

```powershell
$body = '{
  "model": "qwen2.5:0.5b",
  "prompt": "Explain what a KV cache is in one sentence.",
  "max_tokens": 50,
  "backend": "ollama"
}'

Invoke-RestMethod http://127.0.0.1:8080/infer `
  -Method Post `
  -ContentType "application/json" `
  -Body $body
```

## API-key management

Use the configured administrative key from `API_KEYS`:

```powershell
$headers = @{ "X-API-Key" = "sk-demo123" }
```

Create a database-backed API key:

```powershell
$body = '{
  "key": "sk-team-member-1",
  "name": "Team Member 1"
}'

Invoke-RestMethod http://127.0.0.1:8080/api/keys `
  -Method Post `
  -Headers $headers `
  -ContentType "application/json" `
  -Body $body
```

List API keys:

```powershell
Invoke-RestMethod http://127.0.0.1:8080/api/keys `
  -Headers $headers
```

Keys are masked in list responses.

## KV-cache scheduler

Allocate blocks:

```powershell
$body = '{
  "request_id": "request-1",
  "num_blocks": 2
}'

Invoke-RestMethod http://127.0.0.1:8080/v1/allocate `
  -Method Post `
  -Headers $headers `
  -ContentType "application/json" `
  -Body $body
```

View scheduler statistics:

```powershell
Invoke-RestMethod http://127.0.0.1:8080/v1/stats `
  -Headers $headers
```

## Run tests

From the project directory:

```powershell
$env:PYTHONPATH = (Get-Location).Path
python -m compileall -q aether tests_python
python -m pytest -q
```

The Redis integration test requires Redis to be running. If Redis is unavailable, that test is skipped.

## Stop the project

If using Docker Compose:

```powershell
docker compose down
```

To stop the containers and delete the PostgreSQL data volume:

```powershell
docker compose down -v
```

The `-v` option deletes stored database data and should only be used when that data is no longer needed.

If containers were started individually, stop only the containers that exist:

```powershell
docker ps --format "{{.Names}}"
docker stop <container-name>
```

## Important configuration

For local development, these values are sufficient:

```env
REDIS_URL=redis://localhost:6379/0
DATABASE_URL=postgresql://postgres:password@localhost:5433/aether_gateway
API_KEYS=sk-demo123
OLLAMA_ENDPOINT=http://localhost:11434
RATE_LIMIT_RPS=7
RATE_LIMIT_WINDOW_SECONDS=1
RATE_LIMIT_FAIL_CLOSED=true
```

When the gateway runs inside Docker Compose, use the service names instead:

```env
REDIS_URL=redis://redis:6379/0
DATABASE_URL=postgresql://postgres:password@postgres:5432/aether_gateway
OLLAMA_ENDPOINT=http://host.docker.internal:11434
```

For production, replace the default passwords, API keys, and JWT secret. Use a managed PostgreSQL instance, a protected Redis instance, TLS, secure secret storage, and a reverse proxy or load balancer.
