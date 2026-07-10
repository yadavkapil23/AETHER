# Secrets & Configuration

Copy `.env.example` to `.env` before running. This file explains what each value is, and which ones are safe to leave as defaults for local development versus which ones must be changed before any real deployment.

## Must set before deploying anywhere reachable

These ship with placeholder defaults in `.env.example` that work fine on your own machine, but must be replaced before the gateway is exposed outside localhost (including the Kubernetes manifests in `k8s/`).

| Variable | Purpose | Default (dev only) |
|---|---|---|
| `JWT_SECRET` | Signs and verifies JWT bearer tokens. Anyone who knows this value can forge valid tokens. | `dev-secret-for-local-testing` |
| `API_KEYS` | Comma-separated list of valid API keys clients send via the `X-API-Key` header. | `sk-demo123` |
| `DATABASE_URL` | Postgres connection string; embeds the DB password. Only relevant if you're actually running Postgres. | `postgresql://postgres:password@localhost:5433/aether_gateway` |

`k8s/secret.yaml` holds Kubernetes-side placeholders for the same three values (plus `POSTGRES_PASSWORD`) — it has a comment warning not to commit real values there. Override with `kubectl create secret generic aether-secrets --from-literal=...` or a proper secrets manager for real deployments.

## Optional — only needed for that specific backend

| Variable | Purpose |
|---|---|
| `HUGGINGFACE_API_KEY` | Required only if you want the HuggingFace fallback backend to work. Leave blank if you're relying purely on Ollama — `/health` will correctly report HuggingFace as unhealthy without it, which is expected, not a bug. |

## Not secrets — plain configuration, safe as shipped

`GATEWAY_HOST`, `GATEWAY_PORT`, `RATE_LIMIT_RPS`, `GATEWAY_TIMEOUT`, `GATEWAY_CACHE_SIZE`, `OLLAMA_ENDPOINT`, `HUGGINGFACE_ENDPOINT`, `AETHER_CACHE_BYTES`, `AETHER_BLOCK_SIZE`, `PROMETHEUS_PORT`, `GRAFANA_PORT` — no credentials, defaults are fine.

## Known dead config

`REDIS_URL` exists in `.env.example` and `docker-compose.yml` but nothing in the codebase actually uses it — the rate limiter is in-memory, not Redis-backed. Not a secret to worry about, just leftover.

## Local dev quick start

For running on your own machine, the defaults in `.env.example` are enough — copy it to `.env` and go. Nothing needs to change until you deploy somewhere reachable by anyone other than you.
