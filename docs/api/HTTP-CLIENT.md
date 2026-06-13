# HTTP client

`HttpClient` is the wrapping `reqwest::Client` you should use for every
outbound HTTP call from a service. It pre-wires `reqwest-middleware` +
`reqwest-retry` with exponential backoff and jitter, owns a connection
pool, and reads its config from the cascade.

Use this rather than rolling a `reqwest::Client` per call site --
the extra middleware (retry, optional auth header, tracing span)
matters at production scale.

---

## Usage

```rust
use hyperi_rustlib::http_client::HttpClient;

let client = HttpClient::from_cascade()?;

let resp = client.get("https://api.example/v1/things").await?;
let body: ThingList = resp.json().await?;

// JSON helpers handle serialise + content-type:
let created: Thing = client.post_json("https://api.example/v1/things", &payload).await?.json().await?;
```

`from_cascade()` reads the `http_client.*` config section and is the
canonical way to build the client. Pass an explicit `HttpClientConfig`
only when you need a per-call-site variant (different timeout,
different auth).

---

## What the middleware stack gives you

- **Retry** with exponential backoff and jitter -- only retries on
  network errors and 5xx responses (4xx never retries).
- **Configurable timeout** at the request level (default 30s).
- **Connection pooling** -- one pool per `HttpClient` instance. Don't
  rebuild the client per call.
- **`User-Agent` header** identifying the service + version
  automatically.
- **Tracing span** per request -- propagates the current `traceparent`
  if `transport-trace` is on.

Per-host concurrency cap (bulkhead) -- set via config so one slow
downstream can't saturate the connection pool.

---

## Config shape

```yaml
http_client:
  timeout: 30s
  connect_timeout: 5s
  pool_max_per_host: 32
  pool_idle_timeout: 90s
  retry:
    max_attempts: 3
    initial_backoff: 100ms
    max_backoff: 5s
    jitter: true
  default_headers:
    "X-Service": "dfe-loader"
```

`http_client.retry.max_attempts: 0` disables retries (use the underlying
`reqwest::Client` directly for that -- `.client()` exposes it).

---

## API surface

| Item | Purpose |
|------|---------|
| `HttpClient::new(config)` | Build from explicit config |
| `HttpClient::from_cascade()` | Build from the `http_client` config section |
| `.get(url)` | GET request |
| `.post_json(url, &body)` | POST with JSON body and content-type |
| `.put_json(url, &body)` | PUT with JSON body |
| `.delete(url)` | DELETE request |
| `.client() -> &ClientWithMiddleware` | Access the middleware-wrapped reqwest client for custom requests |
| `.config() -> &HttpClientConfig` | Read back the effective config |

For requests that need more than the helpers cover (custom headers,
streaming bodies, multipart), reach through `.client()` and use the
middleware-wrapped reqwest API directly -- you still get retry, timeout,
tracing.

---

## When to use which

| Need | Use |
|------|-----|
| Outbound HTTP from a service | `HttpClient` -- always |
| Outbound HTTP from a one-shot CLI tool | `HttpClient` with smaller pool config, or plain `reqwest` for trivial cases |
| Streaming download | `.client().get(...).send().await?.bytes_stream()` |
| Webhook receiver | Different concern -- that's [HTTP-SERVER](HTTP-SERVER.md) |
| gRPC | Different concern -- see [../transport/BACKENDS.md](../transport/BACKENDS.md) |

---

## Related

- [HTTP-SERVER.md](HTTP-SERVER.md) -- sibling for inbound HTTP
- [../core-pillars/TRACING.md](../core-pillars/TRACING.md) -- span / traceparent propagation
- [../core-pillars/METRICS.md](../core-pillars/METRICS.md) -- per-host request metrics
- [../AUTO-WIRING.md](../AUTO-WIRING.md) -- singleton model
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) -- `http`
- Source: [../../src/http_client/](../../src/http_client/)
