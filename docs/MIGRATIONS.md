# MIGRATIONS

API surface changes that require consumer adjustment. Indexed by the
rustlib version where the change first ships. Used by the
[rebuild-consumers skill](../.claude/skills/rebuild-consumers/SKILL.md)
when `cargo check` flags breakage on a downstream bump.

Pre-GA discipline: no `BREAKING CHANGE:` footer, no major bump. All
six core DFE apps migrate in lockstep.

---

## 2.8.12 -- silent-footgun fixes (scaling feature, MemoryGuard cascade, KEDA trigger)

A `fix:`. Three "set it, nothing happens" footguns surfaced by the
2.8.10/2.8.11 scaling rollout. All additive -- existing apps recompile
unchanged; two carry a behaviour change worth reviewing on the bump.

### `scaling` feature now pulls `expression`

The horizontal `ScalingEngine` is `#[cfg(all(scaling, expression))]`. With
`scaling` enabled but not `expression`, the engine -- and the
`{ns}_scaling_pressure` gauge + smart default -- silently compiled OUT:
signals were pushed to a cell nobody read. 4/6 DFE apps hit this on the
2.8.10 adoption. `scaling` now implies `expression`, so enabling `scaling`
(or `cli-service`, which pulls it) always builds the engine.

- **Action:** none required. Apps that manually added `expression` to work
  around this can drop it (harmless to keep). The `cel` crate cost is now
  always paid with `scaling`/`cli-service` -- in practice it always was,
  since every DFE app needs the engine.

### MemoryGuard reads the cascade `memory:` section (BEHAVIOUR CHANGE)

`ServiceRuntime` built the live `MemoryGuard` via
`MemoryGuardConfig::from_env`, which reads ONLY flat `{PREFIX}_MEMORY_*`
env vars and IGNORED the cascade `memory:` section -- setting `memory:` in
YAML did nothing for the guard. It now uses the new
`MemoryGuardConfig::from_cascade_with_env(prefix)`: the cascade `memory:`
section is the base, flat `{PREFIX}_MEMORY_*` env overlaid on top
(back-compat, the flat env still wins).

- **Action required on the bump:** REVIEW any `memory:` section in app
  config. `limit_bytes` / `pressure_threshold` / `cgroup_headroom` set in
  YAML now take effect (were silently defaulted). `from_env` is unchanged
  and still available for non-cascade callers.

### `KedaContract` gains a scaling-pressure Prometheus trigger (additive)

`KedaConfig` + `KedaContract` gain `scaling_pressure_enabled` (default
`false`) and `scaling_pressure_threshold` (default `70`). The generated
Helm chart now emits a `keda.scalingPressure` values block and a
runtime-gated Prometheus trigger querying
`avg({metric_prefix}_scaling_pressure)` with `metricType: Value` -- the
composite is a capped per-pod 0-100 score, so KEDA scales proportionally to
hold the average at the threshold (the `avg()+Value` consumption the KEDA
doc prescribes; never `sum()` a ratio). Opt-in: the Prometheus
`serverAddress` is cluster-specific and MUST be set in `values.yaml` before
enabling. `#[serde(default)]` on `KedaContract` keeps older contract
artefacts deserialising across the bump.

- **Action:** to wire KEDA to the engine gauge, set
  `keda.scalingPressure.enabled: true` + `serverAddress` in the deployed
  `values.yaml`. Existing kafka-lag + CPU triggers are untouched.

---

## 2.8.11 -- config cascade ACTUALLY applies on the run_app path

A `fix:`. Before this release, [`run_app`](../src/cli/app.rs) built the
`ServiceRuntime` (governor, worker pool, batch engine, scaling) entirely
from `*::from_cascade()` WITHOUT ever calling `config::setup()`, so the
global config singleton was empty and every platform subsystem silently
took its hard-coded defaults regardless of the app's config file. A
`--config <file>` was also never ingested (the cascade discovers config
in DIRECTORIES; a file path pushed into the directory list is never
found). Both are now fixed.

### Platform sections are now HONOURED (BEHAVIOUR CHANGE)

Apps' platform sections in their config file -- `worker_pool`,
`self_regulation`, `batch_processing`, `scaling`, `expression`, and any
other `from_cascade()` section -- are now APPLIED. They were silently
DEFAULTED before.

- **Action required on the bump:** REVIEW these sections in every app's
  config. Any stale or unintended value that was harmlessly ignored will
  now take effect. e.g. a leftover `worker_pool: { min_threads: 64 }`
  that previously did nothing will now actually size the pool.
- No API removal, no signature change -- existing apps recompile
  unchanged. The change is purely that config you set now does what it
  says.

### `run_app` populates the cascade (guarded)

The `Run` arm of `run_app` now calls `config::setup()` with the app's
`env_prefix`, `app_name`, and the `--config` file BEFORE `load_config()`
and the `ServiceRuntime` build. Guarded by `config::try_get().is_none()`
so apps that already call `setup()` / `setup_async()` themselves (e.g.
for the Postgres config layer) are NOT double-initialised.

### New `ConfigOptions.config_file` (additive)

`ConfigOptions` gains `config_file: Option<PathBuf>` (default `None`).
When set, the named YAML file merges ABOVE the discovered
`defaults`/`settings`/`settings.{env}` files but BELOW the PostgreSQL
layer (async path) and BELOW environment variables -- an explicit
override that still yields to ENV. `CommonArgs::to_config_options` now
sets this from `--config` instead of (wrongly) pushing it into
`config_paths`.

### Observable startup + deser WARN

- `ServiceRuntime::build` now emits one startup `tracing::info!` line
  summarising which platform sections were found in the cascade vs
  defaulted (`cascade_initialised`, `self_regulation`, `worker_pool`,
  `batch_processing`, `scaling`, `expression`). The previously-silent
  "defaulted everything" failure is now visible.
- New `Config::unmarshal_key_or_warn` /
  `unmarshal_key_registered_or_warn`: a config section that is PRESENT
  but malformed (typo / type mismatch) now logs a `tracing::warn!` and
  falls back to the default, instead of silently swallowing the error.
  Wired into the four `ServiceRuntime`-build readers: governor,
  worker-pool, batch-engine, scaling. An ABSENT key is unchanged --
  still silent default (the default-ON governor relies on this).
  (`ScalingEngineConfig::from_cascade` shares the `scaling` key with
  `ScalingPressureConfig`, which already warns for that section, so it
  was left as-is to avoid a double WARN.)

### Custom / domain scaling signals (additive)

The scaling engine's CEL pressures could previously reference ONLY the
8 FIXED transport signals. Apps can now push DOMAIN signals so a
`scaling.pressures` expression can scale on them -- essential for
non-rustlib-inbound apps (e.g. the fetcher, whose smart default is
otherwise CPU-only).

- **New `ScalingSignalsCell::set_custom(&self, name: &str, value: f64)`**
  -- insert/overwrite a named domain signal (e.g. cloud-API pending
  fetch backlog / API throttle, ClickHouse insert backlog). Pushed from
  the app at runtime; no setter per signal.
- **New `TransportSignals.custom: BTreeMap<String, f64>`** (default
  empty), populated by `ScalingSignalsCell::snapshot()`. Existing
  `TransportSignals { .. }` struct literals that enumerate all fields
  without `..Default::default()` must add the `custom` field (or switch
  to `..Default::default()`); all setters and the typed fields are
  unchanged.
- **CEL surface:** domain signals are exposed under a DEDICATED
  `custom.<name>` map, SEPARATE from the strict fixed-signal `metrics`
  map. Scale on them like `custom.clickhouse_backlog / params.ch_target`.
- **Validation contract (no API change, behaviour note):** custom names
  are unknown at config-load, so the load-time dry-run cannot
  pre-populate them. Syntax errors and unknown TOP-LEVEL identifiers are
  still HARD-rejected at load. A reference to a `custom.<name>` (or any
  map key absent at load) is downgraded to a load `warn!` and KEPT; the
  runtime guard falls back to last-good / smart-default if it errors at
  tick time. Startup never fails for a `custom.*` reference.
- No API removal, no signature change -- apps recompile unchanged unless
  they hit the `TransportSignals` struct-literal note above.

---

## 2.8.10 -- horizontal scaling pressure + metrics overhaul

A `fix:` (the existing scaling was not fit for purpose). Adds the
CEL-over-local-metrics scaling-pressure ENGINE + the compound transport
pressure, and overhauls metric conventions. The 6 core apps adopt in
lockstep. Full design: [deployment/KEDA.md](deployment/KEDA.md), specs +
the scaling ACR under `docs/superpowers/`.

### CPU metric: gauge-of-percentage -> cumulative COUNTER (BEHAVIOUR)

`{ns}_process_cpu_seconds_total` was a GAUGE holding `cpu_usage()` (an
instantaneous percentage) under a `_total` counter name -- `rate()` and
utilisation were meaningless. Now a proper monotonic COUNTER of
cumulative CPU seconds (Linux `utime+stime` from `/proc/self/stat`,
USER_HZ 100, whole-second granularity).

- Dashboards reading it as a gauge must switch to
  `rate({ns}_process_cpu_seconds_total[5m])`.
- CPU utilisation = `rate(...) / {ns}_container_cpu_limit_cores`.
- Non-Linux: not emitted (production is Linux).

### Histogram unit no longer hard-coded to seconds (additive)

New `MetricsManager::histogram_with_unit(name, desc, unit)` and
`histogram_count(name, desc)`. `histogram()` stays seconds (latency
common case). `batch_engine_chunk_size` is now a count histogram.

### Metric renames -- DUAL-EMITTED this release, OLD dropped next

No native Prometheus rename, so both names emit for 2.8.10; move
dashboards to NEW now, OLD is removed next release.

| Old (still emitted) | New |
|---|---|
| `worker_pool_cpu_utilisation` | `..._cpu_utilisation_ratio` |
| `worker_pool_memory_utilisation` | `..._memory_utilisation_ratio` |
| `worker_pool_saturation` | `..._saturation_ratio` |
| `worker_pool_grow_below` / `_shrink_above` / `_emergency_above` / `_memory_pressure_cap` | `..._ratio` |
| `worker_pool_scale_interval_secs` | `..._scale_interval_seconds` |
| `worker_pool_health_saturation_timeout_secs` | `..._seconds` |
| `self_regulation_byte_budget` | `..._byte_budget_bytes` |
| `transport_filtered_total` | `dfe_transport_filtered_total` |
| `dfe_scaling_memory_pressure` | `dfe_scaling_memory_pressure_ratio` |
| `dfe_spool_disk_available` | `dfe_spool_disk_available_bytes` |

### New default metrics (the "pre-supply" RULE)

Scaling: `{ns}_scaling_pressure{name}`,
`{ns}_transport_inbound_pressure_ratio`,
`{ns}_transport_outbound_pressure_ratio`, `{ns}_scaling_circuit_open`.
Kafka: `kafka_consumer_group_lag` (summed over THIS pod's ASSIGNED
partitions -- inherently per-pod). http-server:
`dfe_http_server_requests_total{method,status}`,
`dfe_http_server_inflight_requests`,
`dfe_http_server_request_duration_seconds`, `dfe_http_server_shed_total`.
gRPC: `dfe_grpc_server_inflight_requests`, `dfe_grpc_server_shed_total`,
`dfe_transport_received_total`. spool/dlq:
`dfe_spool_{enqueue,dequeue}_total`, `dfe_spool_disk_usage_ratio`,
`dfe_dlq_queue_depth`, `dfe_dlq_{admitted,retried}_total`. version-check:
`version_check_total{result}`.

### New scaling config + API (additive)

`scaling` key gains `interval_secs`, `params`, `pressures`, `transport`
(read by the new `ScalingEngineConfig`; the legacy `ScalingPressureConfig`
still reads `enabled`/`memory_gate_threshold` from the same section).
New API: `ScalingEngine`, `ScalingEngineConfig`, `PressureExpr`,
`ScalingTransport`, `TransportSignals`, `ScalingSignalsCell`,
`PressureTargets`, `inbound_pressure`, `outbound_pressure`.
`ServiceRuntime` gains `scaling_engine` + `scaling_signals` (push per-pod
signals there). Legacy `ScalingPressure` unchanged (worker-pool feedback).

**App adoption:** set `scaling.transport.inbound` (so the compound inbound
signal is picked) + `scaling.params.lag_target` (PER-POD), and push signals
via `runtime.scaling_signals.set_*` from receive/send loops. If inbound is
NOT a rustlib transport, the default is CPU-only -- add a domain pressure
expression. Consume per [deployment/KEDA.md](deployment/KEDA.md)
(`sum()+AverageValue` for raw lag, `avg()+Value` for ratios, cap
`maxReplicas` at partition count).

---

## Unreleased -- WorkBatch data-plane spine + self-regulation (Phase 0)

The data plane flips onto a single zero-copy currency -- `WorkBatch` (a block
of `Record`s) -- driven `get -> process -> send -> commit` by ONE unified
engine driver, with self-regulation (memory guard + inbound/byte-budget
backpressure) ON by default. See [SELF-REGULATION.md](SELF-REGULATION.md),
[BACKPRESSURE.md](BACKPRESSURE.md), [KAFKA-PATH.md](KAFKA-PATH.md).

The six core DFE apps migrate in lockstep (Phase 6); items below are the
consumer-facing surface changes.

### `WorkBatch` / `Record` -- the canonical currency (BREAKING)

A new `WorkBatch<T>` (a `Vec<Record>` + the block's `commit_tokens` + any
inline `dlq_entries`) collapses the old `Message` / `RawMessage` / `RecvBatch`
trio into ONE block type. `Record` is payload (`bytes::Bytes`, zero-copy) +
routing key + headers + lean `RecordMeta` (timestamp + format), with **no**
commit token.

The headline contract: **commit tokens live on the BATCH, not the record**,
and `commit_tokens.len()` is decoupled from `records.len()`. A transform that
fans `N` records out to `2N` does NOT multiply the source acks -- the driver
commits EXACTLY the `N` input tokens after the `2N`-record block is sent
(at-least-once). Use `WorkBatch::map_records` to transform records while the
commit tokens and DLQ entries flow through untouched.

`WorkBatch`, `Record`, `RecordMeta`, `FramingError` are re-exported as
`hyperi_rustlib::transport::*`. Zero-copy framing helpers: `WorkBatch::single`
(whole blob), `WorkBatch::from_ndjson`, `WorkBatch::from_json_array` (each
slices one inbound `Bytes` into per-record views -- no payload copy).

### `TransportReceiver::recv` returns `WorkBatch<Token>` (BREAKING)

| Old | `recv(max) -> TransportResult<RecvBatch<Token>>` (messages + dlq_entries) |
| New | `recv(max) -> TransportResult<WorkBatch<Token>>` |

`recv` now yields a `WorkBatch` natively -- no `RecvBatch` round-trip. The
inbound-filter DLQ entries arrive on `WorkBatch.dlq_entries` alongside the
passing `WorkBatch.records`; the source acks are on `WorkBatch.commit_tokens`.

Filter-dropped records still commit: a record an inbound `drop`/`dlq` filter
removes produces no passing record but WAS handled, so its commit token is
carried into `WorkBatch.commit_tokens`. Drop it and an all-filtered stretch
freezes the Kafka offset / leaks the Redis consumer-group PEL. (`RecvBatch`
survives internally as the build-helper that carries these `filtered_tokens`
into the `WorkBatch` -- see KEPT-but-deferred.)

**Consumer adjustment** -- anywhere you call `recv()`:

```rust
let batch = transport.recv(100).await?;
for entry in batch.dlq_entries {
    dlq.send(DlqEntry::new("filter", entry.reason, entry.payload)).await?;
}
for record in batch.records { /* process */ }
```

Custom `TransportReceiver` impls: change the `recv` signature to return
`WorkBatch` (build via `WorkBatch::new(records, tokens)` /
`WorkBatch::from_records(records)` / `WorkBatch::empty()`).

### Unified driver: `run_governed` / `run_workbatch*` replace the four run loops (BREAKING)

The four legacy `BatchEngine` run loops (`run` / `run_raw` / `run_async` /
`run_raw_async`) are DELETED. One unified driver family replaces them
(`src/worker/engine/driver.rs`):

| New method | Use |
|---|---|
| `run_governed` | The default for a self-regulating app. Streams in byte-budget sub-blocks when the governor is on; delegates to `run_workbatch` when off (byte-identical). |
| `run_workbatch` | On-demand parse (default). The driver does not pre-parse; a transform calls `codec::parse` when it needs a field. Pass-through apps pay zero parse. |
| `run_workbatch_parsed` | Opt-in hot path. The driver pre-parses the whole block (SIMD JSON / native MsgPack) on the pool and hands the closure a `ParsedBatch` (records + aligned `ParsedPayload`s + shared `FieldInterner`). Parse failures route to DLQ, no silent drop. |
| `run_workbatch_streaming` | Explicit sub-block streaming with a caller-supplied byte size (peak memory bounded to one sub-block). |

`process` is now `Fn(WorkBatch<Token>) -> Result<WorkBatch<Token>, EngineError>`
(or `Fn(ParsedBatch<'_, Token>) -> Result<WorkBatch<Token>, EngineError>` for
the parsed path). It MUST preserve `commit_tokens` -- use
`WorkBatch::map_records`, which does so automatically. `CommitMode::Auto`
(engine commits after sink `Ok`) vs `CommitMode::SinkManaged` (sink owns the
commit) selects who fires the acks.

Custom in-process callers: `process_mid_tier` / `process_raw` now take a
`Record` (not a `Message`); only the four run LOOPS were removed.

### `TransportSender::send_batch` (additive, default provided)

New trait method `send_batch(&self, records: &[Record]) -> SendResult`. The
default loops `send` per record (using each record's own key + payload
`Bytes`); transports with a native batch RPC (gRPC `RouteBatch`) override it
to send the whole block in one serde-less call. Commit tokens + DLQ entries
are NOT sent -- they are the sender's local concern; pass `&workbatch.records`
and fire the commit tokens locally after `SendResult::Ok`. Existing `send`
callers are unaffected; the default is non-atomic (a mid-block failure leaves
the already-sent prefix on the wire -- at-least-once, retried by the caller).

### Codec consolidation -- native rmpv, JSON bridge removed (BREAKING for codec users)

`src/transport/codec.rs` is the parse-on-demand codec for the WorkBatch spine.
`parse(&Bytes, PayloadFormat) -> ParsedPayload` decodes JSON via `sonic_rs`
(SIMD) and MsgPack via **native `rmpv`** -- NOT the old
`rmp_serde -> serde_json::Value -> serde_json::to_vec -> sonic_rs` bridge
(two parses + a re-serialise per MsgPack record). `ParsedPayload` keeps its
native value (`sonic_rs::Value` / `rmpv::Value`); `field_str` / `field` are
the format-agnostic routing-field accessors; `to_json_bytes` / `to_msgpack_bytes`
/ `ParsedPayload::to_bytes` serialise back to the OWN wire format (no
cross-format bridge). Pass-through contract: an UNMODIFIED record must reuse
its original `Record.payload` -- `to_bytes` is only for a record a transform
actually mutated.

- The read-side `transport/payload.rs` is removed.
- The MsgPack-via-`serde_json` bridge is removed.
- `rmpv` is a new dependency (`transport` feature).
- `ParsedMessage -> ParsedPayload` rename is DEFERRED (see KEPT-but-deferred).

Parse now bounds nesting at depth 64 (`parse_guard::MAX_PARSE_DEPTH`), for
JSON (cheap iterative pre-scan before the recursive SIMD parser) and MsgPack
(`read_value_with_max_depth`). A deeper payload is a per-record `TooDeep`
parse error (routed to DLQ, not a process abort) -- it stops a hostile
deeply-nested payload exhausting the worker stack. Legitimate payloads rarely
nest past a handful of levels, so this is a security floor, not a tuning knob.

`CodecError`, `FieldRef`, `ParsedPayload`, `parse` are re-exported as
`hyperi_rustlib::transport::*`.

### Self-regulation default-ON (BEHAVIOUR CHANGE, opt-out)

A new `self_regulation` config section turns the data-plane governor ON by
default. When the `governor` feature is compiled in, the runtime builds the
pressure governor (memory HARD source), the inbound gate, and the byte-budget
controller, and threads them into the transports + driver. Memory pressure
brakes inbound intake; the byte budget sizes streaming sub-blocks. To opt out
(byte-identical to pre-governor behaviour -- nothing is constructed):

```yaml
self_regulation:
  enabled: false
```

Full tuning surface (profile / pause_above / resume_below / target_rho /
md_factor) in [SELF-REGULATION.md](SELF-REGULATION.md). Off-pressure cost is
near zero: the budget sits at its big start value so a block is one sub-block
with no per-record overhead.

### Originator brake / token wiring (BEHAVIOUR CHANGE)

Data-originator stages get the inbound brake wired into the receive transport:
Kafka pauses ASSIGNED partitions (member stays in group, no rebalance);
HTTP/gRPC returns 503 / `UNAVAILABLE`; the fetcher pauses its poll. Each pairs
with the at-least-once commit token (offset / responder / cursor) so a paused
intake never advances the source position. `SelfRegulationGovernor::attach_kafka_gate`
is the one-call form of the gate dance. See [BACKPRESSURE.md](BACKPRESSURE.md).

**App adoption is TWO steps, not one (Phase 6).** The default-on governor only
engages end-to-end if the app adopts BOTH the driver method AND the
governed-receiver constructor. `run_governed` alone wires the byte-budget lever
(streaming sub-blocks) but does NOT brake intake; the inbound brake lives on the
receive transport, which the plain factory constructors
(`AnyReceiver::from_config` / `from_transport_config`) do NOT wire. Each of the
six core apps MUST:

1. Drive the engine with `run_governed` (not the legacy run loops); AND
2. Build the receive transport through a governor-aware constructor so the
   inbound brake is actually attached.

The one-call path inside a `ServiceRuntime` app (the governor already exists,
built before transports in `ServiceRuntime::build`):

```rust
// run_service(): governor + pressure already constructed by the runtime.
let receiver = runtime.governed_receiver("transport.input").await?;
// Kafka -> pause-partitions gate attached; HTTP/gRPC -> 503/UNAVAILABLE shed;
// brakeless backends (memory/pipe/file/redis) construct as before.
// Falls back to the plain receiver when self_regulation.enabled = false.
```

Outside `ServiceRuntime` (holding a `SelfRegulationGovernor` directly):

```rust
let receiver = AnyReceiver::from_config_with_governor("transport.input", &governor).await?;
// or from_transport_config_with_governor(&cfg, &governor) for an explicit config.
```

Using `from_config` / `from_transport_config` (no governor) is still valid and
byte-identical to before -- but a factory-built receiver wired that way gets NO
inbound brake even when the governor is on. Adopt the `*_with_governor` /
`governed_receiver` path so the default-on governor is not a silent no-op on the
receive side.

### KEPT but deferred

- `Message` / `RecvBatch` remain as internal build-helpers (with
  `From<Message>` / `From<RecvBatch>` conversions into `WorkBatch`). Fully
  retiring them needs a filter-layer rework -- deferred.
- `ParsedMessage -> ParsedPayload` rename deferred (the engine still uses
  `ParsedMessage` for the in-process callers).

### `BatchEngine` filter-DLQ policy (BEHAVIOUR CHANGE)

The generic `BatchEngine` run loops (`run`/`run_raw`/`run_async`/
`run_raw_async`) previously **silently dropped** inbound-filter DLQ entries
after incrementing a metric. They now apply a `FilterDlqPolicy`, defaulting to
`Reject`: if an inbound `action: dlq` filter produces entries and no policy is
set, the run loop returns `EngineError::FilterDlqUnrouted` instead of dropping
data. (Metrics are not delivery.)

**Who is affected:** only apps whose transport has inbound `action: dlq`
filters AND use the generic run loops. Apps with no inbound DLQ filters are
unaffected (the policy never triggers).

**Consumer adjustment** — pick a policy explicitly:

```rust
use hyperi_rustlib::worker::engine::FilterDlqPolicy;

// Route dead-letters onward (recommended). The sink is FALLIBLE: return Ok on
// success; an Err is a terminal ack-barrier failure (commit skipped, block
// re-delivered) so dead-letters are never silently lost. The SAME route point
// handles inbound-filter entries AND parse/process-generated entries.
let engine = BatchEngine::new(cfg).with_filter_dlq_policy(
    FilterDlqPolicy::Route(std::sync::Arc::new(move |entries| {
        // enqueue / tokio::spawn a DLQ send -- keep it cheap
        Ok(())
    })),
);

// Or deliberately drop with a metric (the old behaviour, now explicit):
let engine = BatchEngine::new(cfg)
    .with_filter_dlq_policy(FilterDlqPolicy::DiscardWithMetric);
```

The metric `dfe_engine_filter_dlq_unrouted_total` is replaced by
`dfe_engine_filter_dlq_discarded_total` (emitted only under `DiscardWithMetric`).

### `TransportSender::send` takes owned `Bytes` (BREAKING)

| Old | `send(&self, key: &str, payload: &[u8]) -> SendResult` |
| New | `send(&self, key: &str, payload: bytes::Bytes) -> SendResult` |

Owned-bytes send (Phase 4.1) removes the per-send `payload.to_vec()` copy on
the HTTP path (reqwest `Body::from(Bytes)` is zero-copy) and lets a caller that
already holds `Bytes` (the `BatchEngine`) flow it through without re-copying.

**Consumer adjustment** — at each `send` call, pass owned `Bytes`:

```rust
// Old:
sender.send("topic", &payload_slice).await;
sender.send("topic", b"literal").await;

// New (all conversions from Vec<u8>/String/&'static [u8] are cheap):
sender.send("topic", bytes::Bytes::from(payload_vec)).await;        // Vec<u8>  (free)
sender.send("topic", bytes::Bytes::from_static(b"literal")).await;  // &'static [u8]
sender.send("topic", bytes::Bytes::copy_from_slice(slice)).await;   // &[u8]    (copies)
```

A caller holding `&[u8]` now copies once at the call site (`copy_from_slice`)
instead of inside the transport — net-neutral. Callers holding `Vec<u8>` or
`Bytes` (the hot path) are now zero-copy. `bytes` is a `transport`-feature dep.

### `transport::FromCascade` trait (additive, non-breaking)

New `transport::FromCascade` trait with a default `from_cascade_key(key)`
consolidates the byte-identical `from_cascade()` bodies the 5 transport configs
(grpc/http/file/pipe/redis) each repeated. Each config's inherent
`from_cascade()` is unchanged in signature — it just delegates — so this is
**not** a consumer migration; it only removes internal duplication.

### `AdaptiveWorkerPool::fan_out_async` return type

| Old | `Vec<Result<R, E>>` |
| New | `Vec<Option<Result<R, E>>>` |

Panicked tasks now surface as `None` instead of being silently dropped
(the old `.flatten()` shortened the output and broke the documented
input-order contract). Caller adjustment:

```rust
// Drop panicked slots (old behaviour):
let results: Vec<Result<R, E>> = pool.fan_out_async(items, f)
    .await
    .into_iter()
    .flatten()
    .collect();

// Or destructure explicitly:
for (i, slot) in pool.fan_out_async(items, f).await.into_iter().enumerate() {
    match slot {
        Some(Ok(r)) => { /* success */ }
        Some(Err(e)) => { /* task returned Err */ }
        None => tracing::error!(idx = i, "task panicked"),
    }
}
```

### `DbConnection.password` type

| Old | `String` |
| New | `SensitiveString` |

Construction via `DbConnection { password: "x".into(), ... }` keeps
working (`From<&str> for SensitiveString` exists). Sites that clone
the plaintext need `.expose()`:

```rust
// Before:
let url = format!("postgres://{}:{}@{}/{}", c.user, c.password, c.host, c.db);
// After:
let url = format!("postgres://{}:{}@{}/{}", c.user, c.password.expose(), c.host, c.db);
```

In-crate URL builders already do this. The change exists so `Debug`
+ `serde` round-trips redact by default.

### `Cache::set` signature

| Old | `fn set(&self, ...) -> ()` |
| New | `fn set(&self, ...) -> Result<(), serde_json::Error>` |

The old form swallowed serialise failures via `Err(_) => return`.
Callers now propagate or `.expect("cache set")` if a panic is the
right escalation:

```rust
cache.set(&key, &value, source).expect("cache set");
// or
cache.set(&key, &value, source)?;
```

### `expose_during` (additive)

New crate-root helper `hyperi_rustlib::expose_during<F, R>(f: F) -> R`
flips a thread-local flag so `SensitiveString` serialises its real
value inside the closure. Wrap any figment / serde round-trip that
must preserve secrets:

```rust
let cfg: Config = expose_during(|| {
    Figment::from(Serialized::defaults(&defaults))
        .merge(Env::prefixed("MYAPP_"))
        .extract()
})?;
```

Required for any consumer using `Figment::from(Serialized::defaults(&Config))`
where `Config` contains `SensitiveString` fields. Symptom of missing
this: secrets land as literal `***REDACTED***` post round-trip and
auth fails.

### `memory::set_heap_source` — total-heap backpressure (additive, opt-in)

New crate hook `hyperi_rustlib::memory::set_heap_source(fn() -> usize)`.
When registered, every `MemoryGuard` switches its read path
(`current_bytes`, pressure checks, `try_reserve` admission) from the
per-batch reservation counter to a true **total-process heap** figure --
catching growth the reservations never see (e.g. a transform ballooning a
`Vec`). Not registering it keeps the existing per-batch behaviour, so this
is **optional**, not a required migration.

To adopt in a DFE app, install a tracking allocator and wire it once at
startup. Prefer an actively-maintained allocator -- `tikv-jemalloc-ctl`
(`cap` works but is unmaintained since 2023):

```rust
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    hyperi_rustlib::memory::set_heap_source(|| {
        tikv_jemalloc_ctl::epoch::advance().ok();
        tikv_jemalloc_ctl::stats::allocated::read().unwrap_or(0)
    });
    // ... ServiceRuntime / MemoryGuard built afterwards pick it up ...
}
```

rustlib intentionally takes **no allocator dependency** (the global
allocator is the binary's choice, and rustlib is `#![forbid(unsafe_code)]`).

### `SinkDrain::flush_durable` (additive, default no-op)

New trait method; existing impls compile unchanged. Custom drains
that need true durability override (file flushes the writer, Kafka
calls `producer.flush()`).

### `Dlq` struct (internal layout)

Gained a `cancel: CancellationToken` field. Consumers construct via
`Dlq::spawn` or `Dlq::disabled` and don't touch the struct fields —
no visible change.

### `TransportFilterTierConfig.budget`

New optional field defaulting to permissive values
(`max_ast_nodes=200`, `max_iteration_depth=2`,
`max_payload_bytes=1MiB`). YAML configs without the field continue
to deserialise. To tighten:

```yaml
transport:
  filter_tiers:
    budget:
      max_ast_nodes: 100
      max_iteration_depth: 1
      max_payload_bytes: 524288   # 512 KiB
```

### `StrMatcherSet` (no surface change)

Internal partition into merged-AC + individual matchers. Public API
unchanged (`is_match`, `find`, `find_iter`, `earliest_match`,
`tier_counts`, `len`, `is_empty`).

### `HttpServerConfig.max_connections`

Now actually wired (was silently inert). Default 10,000. Consumers
that set `max_connections: 1` to "disable" filtering will now hit
a hard cap; raise to a realistic number or document the throttling
intent.

### Telemetry / `version-check`

`CheckPayload` no longer includes `instance_id` or `deployment`.
`HYPERI_TELEMETRY=off` opt-out env var. No consumer code change
required; the field rename only affects log lines from rustlib
itself.

### Wave 1 — Tier-3 single-knob

`transport.filter_tiers.allow_complex_filters_in/out: true` now
implies `expression.allow_regex / allow_iteration / allow_time =
true` for the transport's compile path. Previously operators had
to flip both knobs and they could disagree (filter passes the
transport gate, fails the expression profile). One source of truth.

### Wave 1 — `WorkerPoolConfig::validate`

`async_concurrency == 0` now rejected at config-load. Previously
passed validation and panicked at `step_by(0)` inside
`fan_out_async`.

### Wave 2 — `BackgroundSink::flush()` surfaces drain errors

`flush()` now returns `Err(SinkError::Drain(_))` when the underlying
drain's `write_batch` or `flush_durable` failed. Previously acked
`Ok(())` regardless — callers thought messages were durable when
they were lost. Caller adjustment: handle `Err(SinkError::Drain)`
on `flush().await`.

### Wave 2 — Kafka DLQ `flush_durable` Err on outstanding

`Dlq::flush_durable` (Kafka backend) returns `DlqError::Kafka`
when the producer flush timeout expires with messages still in
flight. Previously logged at debug and returned `Ok(())`. Shutdown
paths that assumed Ok = drained must now treat Err as "DLQ entries
may be lost".

### Wave 3 — `CacheConfig.dir_mode` / `.file_mode`

Two new optional fields default to `Some(0o700)` and `Some(0o600)`.
`None` disables chmod entirely — required on S3-FUSE / root-
squashed NFS / similar mounts that reject chmod. Operators on
those mounts must own upstream perms.

### Wave 3 — `dangerous-diagnostics` feature

`config::registry::dump_effective_unredacted()` is now gated by
the `dangerous-diagnostics` cargo feature. Not included in `full`.
Compile with `--features dangerous-diagnostics` only for one-off
operator-driven debugging.

### Wave 3 — strict CEL `has(<single>)`

Tier-1 `has(<single-field>)` now only matches at JSON depth 1
(immediate child of the root). Previously matched at any depth.
Operators relying on the nested-match behaviour must switch to a
dotted path (`has(some.path.field)`) or to a Tier-2 CEL filter.

### Wave 5 — bounded metric labels (F7)

`DfeMetrics` methods that took free-form `&str` for metric labels
now take typed enums. The labels are bounded; cardinality is
fixed at the enum variant count.

| Method | Old | New |
|---|---|---|
| `transport_sent` | `(transport: &str, count)` | `(transport: TransportKind, count)` |
| `transport_send_errors` | `(transport: &str, count)` | `(transport: TransportKind, count)` |
| `auth_failure` | `(reason: &str)` | `(reason: AuthFailureReason)` |
| `validation_failure` | `(reason: &str)` | `(reason: ValidationFailureReason)` |
| `BufferGroup::record_flush` | `(duration, trigger: &str)` | `(duration, trigger: FlushTrigger)` |

```rust
// Before
dfe.auth_failure("token-expired");
dfe.record_flush(0.012, "size");
dfe.transport_sent("kafka", 1);

// After
use hyperi_rustlib::metrics::{AuthFailureReason, FlushTrigger, TransportKind};
dfe.auth_failure(AuthFailureReason::Expired);
dfe.record_flush(0.012, FlushTrigger::Size);
dfe.transport_sent(TransportKind::Kafka, 1);
```

Variant lists:
- `TransportKind`: `Kafka`, `Grpc`, `Memory`, `File`, `Pipe`, `Http`, `Redis`, `Routed`
- `FlushTrigger`: `Size`, `Records`, `Age`, `Eviction`, `Shutdown`, `Manual`
- `AuthFailureReason`: RFC 6749 codes + JWT failure modes
  (`Expired`, `InvalidSignature`, `InvalidClient`, `InvalidGrant`,
  `InvalidScope`, `MalformedToken`, `RevokedToken`, `RateLimited`,
  `Unauthorized`, `AccessDenied`)
- `ValidationFailureReason`: JSON Schema 2020-12 categories
  (`SchemaInvalid`, `FieldMissing`, `TypeMismatch`, `OutOfRange`,
  `PatternMismatch`, `FormatInvalid`, `EnumViolation`,
  `AdditionalProperties`, `NullValue`, `EncodingError`)

No `Other` catch-all. New failure modes require a rustlib release
that adds a variant; consumers then bump and recompile. The compiler
flags every site needing the new variant.

### Wave 5 — `RoutedSender` metric label

`dfe_transport_sent_total{transport="routed",route=...}` now
carries the **configured route name** (or `"default"` for the
fallback), not the inbound message key. Cardinality is bounded by
the routing table size. No consumer code change required — only
the metric label values change. Dashboards keyed on per-message
keys need rewiring.

---

## Known open issues (not fixed on this branch)

Tracked upstream; each needs its own focused commit. Workarounds
applied at the consumer level until then.

### #35 — Kafka topic auto-discovery race

`KafkaAdmin::list_topics` returns empty when the admin consumer
hasn't finished its bootstrap handshake. Symptom: "Auto-discovery
found no matching topics" at startup even though the topic exists.

**Workaround:** drop `topic_regex` from the config and list topics
explicitly under `topics:`. The explicit-subscribe path bypasses
the resolver.

### #36 — `KafkaTransport` always allocates both roles

`KafkaTransport::new` builds BOTH a `BaseConsumer` and a
`FutureProducer` (the producer from its own `ClientConfig`).
Producer-only callers with empty `group.id` get "rdkafka consumer
queue not available" because the consumer half still can't construct
without a group.

**Workaround:** set `librdkafka_options.group.id` to a dummy
non-empty value on producer-side configs. The dummy group is
never used; only present so `BaseConsumer` doesn't fail to
construct.

### #37 — `TransportSender::send(key, payload)` overloads `key` as topic

The Kafka impl passes `key` to `FutureRecord::to(key)`, so the
"key" arg is the destination topic, not a partition key. Callers
can't route to a configured topic AND set a partition key in one
call.

**Workaround:** none. Sites needing partition keys must bypass
the trait and use rdkafka directly.

---

## Older releases

Historical migrations live in agent memory at `project_dfe_*_migration.md` (referenced from `MEMORY.md`) until they graduate here.
