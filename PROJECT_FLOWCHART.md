# AETHER Complete Project Flowchart

This document maps the complete AETHER system: startup, request handling, authentication, rate limiting, inference, resilience, scheduling, persistence, metrics, and deployment.

## 1. Complete system overview

```mermaid
flowchart TB
    Client[Client application or browser UI]
    Gateway[AETHER FastAPI Gateway\naether/gateway.py]
    Redis[(Redis\ndistributed rate-limit state)]
    Postgres[(PostgreSQL\nAPI keys, inference logs, audit logs)]
    Ollama[Ollama\nlocal model backend]
    HF[Hugging Face\nexternal model backend]
    Scheduler[In-process KV-cache scheduler]
    Prometheus[Prometheus\nmetrics collection]
    Grafana[Grafana\nmetrics dashboard]

    Client --> Gateway
    Gateway --> Redis
    Gateway --> Postgres
    Gateway --> Ollama
    Gateway --> HF
    Gateway --> Scheduler
    Prometheus -->|GET /metrics| Gateway
    Grafana --> Prometheus
```

## 2. Startup and application lifecycle

```mermaid
sequenceDiagram
    participant OS as Windows / Docker
    participant App as AETHER Gateway
    participant Redis
    participant DB as PostgreSQL
    participant Model as Ollama / Hugging Face

    OS->>App: Start uvicorn aether.gateway:app
    App->>App: Load Settings from environment/.env
    App->>App: Create GatewayState
    App->>App: Create Metrics, AuditTrail, Scheduler, LLMBackend
    App->>Redis: Create Redis client and ping
    Redis-->>App: PONG
    App->>DB: Connect and run migrations
    DB-->>App: Connection ready
    App->>Model: Backend health checks when requested
    App-->>OS: Application ready
```

If PostgreSQL is unavailable, the gateway can still start using `API_KEYS`, but database-backed API-key management returns `503`.

## 3. Normal request flow

```mermaid
flowchart TD
    Start[Incoming HTTP request]
    Headers[Security headers middleware]
    Limit[Redis rate-limit middleware]
    Identity[Choose identity:\nAPI key -> client ID -> IP]
    Lua[Atomic Redis Lua sliding-window check]
    Allowed{Request allowed?}
    Reject[Return HTTP 429\nRetry-After and quota headers]
    Route[FastAPI route handler]
    Auth{Route requires authentication?}
    Verify[Validate API key or JWT]
    AuthOK{Credentials valid?}
    Unauthorized[Return HTTP 401]
    Handler[Execute route logic]
    Response[Return response]

    Start --> Headers --> Limit --> Identity --> Lua --> Allowed
    Allowed -->|No| Reject
    Allowed -->|Yes| Route --> Auth
    Auth -->|No| Handler
    Auth -->|Yes| Verify --> AuthOK
    AuthOK -->|No| Unauthorized
    AuthOK -->|Yes| Handler
    Handler --> Response
```

## 4. Rate-limiting flow

```mermaid
flowchart TD
    Request[Incoming request]
    Key[Build Redis key:\naether:rate-limit:{identity}:{route}]
    Remove[Remove entries older than window]
    Count[Count current requests]
    Full{Count >= limit?}
    Denied[Return 429\nRetry-After header]
    Add[Add request to Redis sorted set]
    TTL[Set Redis key expiration]
    Continue[Continue to FastAPI route]
    RedisError{Redis error?}
    FailClosed[Return 503\nrate limiter unavailable]
    FailOpen[Continue without limit\nonly if configured]

    Request --> Key
    Key --> RedisError
    RedisError -->|Yes| FailClosed
    RedisError -->|Yes and fail-open| FailOpen
    RedisError -->|No| Remove --> Count --> Full
    Full -->|Yes| Denied
    Full -->|No| Add --> TTL --> Continue
```

The rate limiter is distributed because all gateway instances use the same Redis keys.

## 5. Authentication flow

```mermaid
flowchart TD
    Request[Protected request]
    APIKey{X-API-Key header?}
    Static[Compare against API_KEYS environment values]
    Database[Check PostgreSQL api_keys table]
    Bearer{Bearer token?}
    JWT[Verify JWT signature with JWT_SECRET]
    Valid{Valid credentials?}
    Deny[HTTP 401]
    Allow[Route continues]

    Request --> APIKey
    APIKey -->|Yes| Static
    Static -->|Valid| Allow
    Static -->|Invalid| Database
    Database -->|Valid| Allow
    Database -->|Invalid| Bearer
    APIKey -->|No| Bearer
    Bearer -->|Yes| JWT --> Valid
    Bearer -->|No| Deny
    Valid -->|Yes| Allow
    Valid -->|No| Deny
```

JWT verification exists, but the project does not currently issue JWTs. API keys are the practical authentication method.

## 6. Synchronous inference flow

```mermaid
flowchart TD
    Request[POST /infer]
    Validate[Pydantic validate request]
    Backend{backend field}
    OllamaAuth[No authentication required by current policy]
    HFAuth[Require API key or JWT]
    BackendCheck[Select backend client]
    Circuit[Check backend circuit breaker]
    Open{Circuit open?}
    FastFail[Fail quickly with backend error]
    Retry[Retry transient HTTP failures\nwith exponential backoff and jitter]
    Timeout[Apply gateway timeout]
    ModelCall[HTTP request to model backend]
    Success{Inference successful?}
    Result[Build InferenceResponse]
    Metrics[Record latency, tokens, backend]
    Audit[Append BLAKE3 audit event]
    DBLog[Async PostgreSQL inference/audit log]
    Error[Record error and return HTTP 502]

    Request --> Validate --> Backend
    Backend -->|ollama| OllamaAuth --> BackendCheck
    Backend -->|huggingface| HFAuth --> BackendCheck
    BackendCheck --> Circuit --> Open
    Open -->|Yes| FastFail
    Open -->|No| Retry --> Timeout --> ModelCall --> Success
    Success -->|Yes| Result --> Metrics --> Audit --> DBLog --> Result
    Success -->|No| Error
```

## 7. Ollama inference flow

```mermaid
sequenceDiagram
    participant Client
    participant Gateway
    participant Breaker as Ollama Circuit Breaker
    participant Retry as Retry Handler
    participant Ollama

    Client->>Gateway: POST /infer backend=ollama
    Gateway->>Breaker: Check and execute call
    Breaker->>Retry: Run backend request
    Retry->>Ollama: POST /v1/completions
    Ollama-->>Retry: Model output and usage
    Retry-->>Breaker: Success
    Breaker-->>Gateway: InferenceResult
    Gateway-->>Client: InferenceResponse
```

## 8. Hugging Face inference flow

```mermaid
sequenceDiagram
    participant Client
    participant Gateway
    participant Auth as API-key validator
    participant Breaker as HF Circuit Breaker
    participant Retry as Retry Handler
    participant HF as Hugging Face API

    Client->>Gateway: POST /infer backend=huggingface
    Gateway->>Auth: Validate X-API-Key or JWT
    Auth-->>Gateway: Authorized
    Gateway->>Breaker: Check and execute call
    Breaker->>Retry: Run backend request
    Retry->>HF: POST /models/{model}
    HF-->>Retry: Generated text
    Retry-->>Breaker: Success
    Breaker-->>Gateway: InferenceResult
    Gateway-->>Client: InferenceResponse
```

## 9. Streaming flow

```mermaid
flowchart TD
    Request[POST /infer/stream]
    Validate[Validate request]
    HF{Hugging Face backend?}
    Reject[HTTP 400\nstreaming is Ollama-only]
    OllamaURL[Build Ollama /v1/completions request\nstream=true]
    Upstream[Open streaming HTTP connection]
    Lines[Read upstream lines]
    SSE[Forward data lines as SSE events]
    Client[Streaming client]

    Request --> Validate --> HF
    HF -->|Yes| Reject
    HF -->|No| OllamaURL --> Upstream --> Lines --> SSE --> Client
```

## 10. OpenAI-compatible chat flow

```mermaid
flowchart TD
    Request[POST /v1/chat/completions]
    Validate[Validate ChatCompletionRequest]
    Messages{Messages present?}
    Bad[HTTP 400]
    Prompt[Convert role/content messages into prompt text]
    Backend[Call selected LLM backend]
    Format[Format OpenAI-compatible response]
    Client[Return chat.completion response]

    Request --> Validate --> Messages
    Messages -->|No| Bad
    Messages -->|Yes| Prompt --> Backend --> Format --> Client
```

## 11. KV-cache scheduler flow

```mermaid
flowchart TD
    Client[Authenticated scheduler request]
    Allocate[POST /v1/allocate]
    Validate[Validate request ID and block count]
    Free[Find free fixed-size blocks]
    Enough{Enough blocks?}
    Error[Return allocation error]
    Reserve[Reserve blocks under thread lock]
    Stats[Update cache metrics and statistics]
    Return[Return block IDs and node ID]
    Deallocate[POST /v1/deallocate]
    Release[Release block IDs]

    Client --> Allocate --> Validate --> Free --> Enough
    Enough -->|No| Error
    Enough -->|Yes| Reserve --> Stats --> Return
    Client --> Deallocate --> Release --> Stats
```

The scheduler is single-process and in-memory. It is not a distributed multi-machine cache allocator.

## 12. API-key management flow

```mermaid
sequenceDiagram
    participant Admin
    participant Gateway
    participant Auth
    participant DB as PostgreSQL

    Admin->>Gateway: POST /api/keys
    Gateway->>Auth: Validate admin API key
    Auth-->>Gateway: Authorized
    Gateway->>DB: INSERT api_keys row
    DB-->>Gateway: Created key metadata
    Gateway-->>Admin: Masked key metadata

    Admin->>Gateway: GET /api/keys
    Gateway->>DB: SELECT active keys
    DB-->>Gateway: Key records
    Gateway-->>Admin: Masked key list
```

## 13. PostgreSQL data flow

```mermaid
flowchart LR
    Gateway[AETHER Gateway]
    Connect[Database.connect]
    Migrate[Create tables if missing]
    Keys[api_keys]
    Inference[inference_logs]
    Audit[audit_logs]
    Health[Database health check]
    DB[(PostgreSQL)]

    Gateway --> Connect --> Migrate --> DB
    Gateway --> Keys
    Gateway --> Inference
    Gateway --> Audit
    Gateway --> Health --> DB
```

## 14. Monitoring flow

```mermaid
flowchart TD
    Gateway[AETHER Gateway]
    Metrics[Metrics collector]
    Endpoint[GET /metrics]
    Prometheus[Prometheus scrape]
    Storage[Prometheus time-series storage]
    Grafana[Grafana dashboards]

    Gateway --> Metrics --> Endpoint
    Prometheus -->|every scrape interval| Endpoint
    Prometheus --> Storage
    Grafana --> Storage
```

Metrics include inference totals, latency, generated tokens, errors, rate-limited requests, and allocated cache blocks.

## 15. Docker deployment flow

```mermaid
flowchart TB
    Compose[docker compose up -d --build]
    Postgres[PostgreSQL container]
    Redis[Redis container]
    Gateway[Gateway container]
    Prometheus[Prometheus container]
    Grafana[Grafana container]
    HealthDB[PostgreSQL health check]
    HealthRedis[Redis health check]
    Ready[Gateway starts after dependencies are ready]

    Compose --> Postgres --> HealthDB
    Compose --> Redis --> HealthRedis
    Compose --> Gateway
    Compose --> Prometheus
    Compose --> Grafana
    HealthDB --> Ready
    HealthRedis --> Ready
```

The gateway container uses `host.docker.internal` for Ollama running on the Windows host. PostgreSQL and Redis are reached by their Compose service names.

## 16. Endpoint map

| Endpoint | Purpose | Auth | Main dependencies |
|---|---|---:|---|
| `GET /` | Local web UI | No | Static HTML |
| `POST /infer` | Synchronous inference | HF only | Ollama or Hugging Face |
| `POST /infer/stream` | Ollama SSE streaming | HF rejected | Ollama |
| `POST /v1/chat/completions` | OpenAI-compatible chat | HF only | Ollama or Hugging Face |
| `POST /v1/allocate` | Allocate KV-cache blocks | Yes | In-process scheduler |
| `POST /v1/deallocate` | Release KV-cache blocks | Yes | In-process scheduler |
| `GET /v1/stats` | Scheduler statistics | Yes | In-process scheduler |
| `GET /v1/cluster` | Scheduler node health | Yes | In-process scheduler |
| `GET /backends/status` | Backend health and breakers | Yes | Ollama/Hugging Face |
| `POST /api/keys` | Create database API key | Yes | PostgreSQL |
| `GET /api/keys` | List database API keys | Yes | PostgreSQL |
| `DELETE /api/keys/{key}` | Revoke database API key | Yes | PostgreSQL |
| `GET /health/live` | Liveness | No | None |
| `GET /health/ready` | Backend readiness | No | Backend health |
| `GET /health` | Full health report | No | DB, backends, audit, scheduler |
| `GET /metrics` | Prometheus metrics | No | Metrics collector |

## 17. Main limitations

```mermaid
flowchart TD
    AETHER[AETHER current architecture]
    Single[Single gateway process for scheduler state]
    RedisShared[Redis shared rate-limit state]
    DBShared[PostgreSQL shared persistence]
    NoJWTIssue[JWT verification exists\nJWT issuance does not]
    NoHF[Hugging Face requires configured API key]
    NoDistScheduler[KV-cache scheduler is not distributed]

    AETHER --> Single
    AETHER --> RedisShared
    AETHER --> DBShared
    AETHER --> NoJWTIssue
    AETHER --> NoHF
    AETHER --> NoDistScheduler
```

The production-grade rate limiter is distributed through Redis, while the KV-cache scheduler and audit trail remain process-local unless their persistence paths are explicitly used.
