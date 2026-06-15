# KEDA

KEDA (Kubernetes Event-driven Autoscaling) scales pods on triggers
the standard HPA can't see -- Kafka consumer-group lag, Prometheus
queries, cron schedules, queue depth. rustlib is autoscaler-NEUTRAL in
code; KEDA is the prime tool and the worked example here.

Two layers:

- `KedaContract` -- the deployment-side declaration (generated
  `ScaledObject`).
- The **horizontal scaling-pressure engine** (`ScalingEngine`, rustlib
  2.8.10) -- the runtime-side signal source. It computes a **correlated
  composite** pressure from the app's rich LOCAL context (CPU, transport
  backlog, in-flight, domain signals) that a bare top-level KEDA trigger
  cannot see together. That correlation is rustlib's edge.

The older weighted `ScalingPressure` is retained (worker-pool feedback)
but superseded by the engine for the scale-out signal -- see the legacy
section below.

---

## `KedaContract`

The contract subset that lands in `values.yaml.keda` and the generated
`ScaledObject`. Built from `KedaConfig` (cascade-loaded runtime config)
via `KedaContract::from_config(&cfg)`.

```rust
pub struct KedaContract {
    pub min_replicas: u32,
    pub max_replicas: u32,
    pub polling_interval: u32,        // seconds between KEDA polls
    pub cooldown_period: u32,         // seconds before scale-down after load drops
    pub kafka_lag_threshold: u64,     // scale when lag > N per partition
    pub activation_lag_threshold: u64,// wake from zero when lag > N
    pub cpu_enabled: bool,
    pub cpu_threshold: u32,           // % utilisation
}
```

`KedaContract::default()` shape:

| Field | Default |
|-------|---------|
| `min_replicas` | `1` |
| `max_replicas` | `10` |
| `polling_interval` | `15` |
| `cooldown_period` | `300` |
| `kafka_lag_threshold` | `1000` |
| `activation_lag_threshold` | `0` |
| `cpu_enabled` | `true` |
| `cpu_threshold` | `80` |

`min_replicas: 0` enables scale-to-zero -- pods spin down entirely
when there's nothing to do, KEDA spins them back up when lag exceeds
`activation_lag_threshold`.

---

## When templates are generated

`generate_chart()` writes `keda-scaledobject.yaml` and
`keda-triggerauth.yaml` **only when `contract.keda.is_some()`**.
Non-autoscaling services (one-shot jobs, singleton coordinators) set
`keda: None` and get just the HPA fallback.

`hpa.yaml` is always written. It guards itself with
`{{- if and .Values.autoscaling.enabled (not .Values.keda.enabled) }}`
-- mutually exclusive with KEDA at runtime, so clusters without the
KEDA operator still scale on CPU by setting `autoscaling.enabled: true`
and `keda.enabled: false`.

---

## Generated `ScaledObject`

Triggers KEDA gets:

1. **Kafka** -- `lagThreshold` per partition, `activationLagThreshold`
   for wake-from-zero. Topic and consumer group default to
   `values.yaml.config.kafka`, overridable per-deployment.
2. **CPU** (optional, when `keda.cpu.enabled`) -- utilisation
   percentage via `metricType: Utilization`.
3. **Scaling pressure** (optional, when `keda.scalingPressure.enabled`) --
   a Prometheus trigger on `avg({metric_prefix}_scaling_pressure)`
   (`metricType: Value`), the correlated-composite engine gauge (below).
   Opt-in: `serverAddress` is cluster-specific and must be set in
   `values.yaml` before enabling. rustlib 2.8.12.

```yaml
apiVersion: keda.sh/v1alpha1
kind: ScaledObject
metadata:
  name: {{ include "dfe-loader.fullname" . }}
spec:
  scaleTargetRef:
    name: {{ include "dfe-loader.fullname" . }}
  minReplicaCount: {{ .Values.keda.minReplicaCount }}
  maxReplicaCount: {{ .Values.keda.maxReplicaCount }}
  pollingInterval: {{ .Values.keda.pollingInterval }}
  cooldownPeriod:  {{ .Values.keda.cooldownPeriod }}
  triggers:
    - type: kafka
      authenticationRef:
        name: {{ include "dfe-loader.fullname" . }}-kafka-auth
      metadata:
        bootstrapServers: {{ .Values.config.kafka.brokers | quote }}
        consumerGroup:    {{ .Values.keda.kafka.consumerGroup | default .Values.config.kafka.group_id | quote }}
        topic:            {{ .Values.keda.kafka.topic | default (index .Values.config.kafka.topics 0) | quote }}
        lagThreshold:           {{ .Values.keda.kafka.lagThreshold | quote }}
        activationLagThreshold: {{ .Values.keda.kafka.activationLagThreshold | quote }}
        saslType: scram_sha512
        tls: disable
    {{- if .Values.keda.cpu.enabled }}
    - type: cpu
      metricType: Utilization
      metadata:
        value: {{ .Values.keda.cpu.threshold | quote }}
    {{- end }}
    {{- if .Values.keda.scalingPressure.enabled }}
    - type: prometheus
      metricType: Value
      metadata:
        serverAddress: {{ .Values.keda.scalingPressure.serverAddress | quote }}
        query:         {{ .Values.keda.scalingPressure.query | quote }}
        threshold:     {{ .Values.keda.scalingPressure.threshold | quote }}
    {{- end }}
```

KEDA scales to the **max** of all triggers -- high lag OR high CPU OR
high correlated-composite pressure grows the pool; all must subside to
shrink. The scaling-pressure trigger is OFF by default (`serverAddress`
is cluster-specific) -- the chart wires it; the operator enables it.

The `TriggerAuthentication` template wires SASL credentials from the
`kafka` secret group. With no `kafka` group, no `TriggerAuthentication`
is written and `authenticationRef` is omitted -- useful where Kafka
auth is bypassed via mesh mTLS.

---

## Horizontal scaling pressure -- the engine (correlated composite)

> rustlib 2.8.10. Autoscaler-neutral in code; KEDA is the prime tool.

An autoscaler scales on coarse, top-level, SINGLE metrics. An app has
rich LOCAL context it can COMBINE and CORRELATE -- CPU, transport
backlog, in-flight, domain signals -- into one **correlated composite**
pressure. That correlated composite is rustlib's edge over a bare KEDA
trigger, and the job of `ScalingEngine`.

### What you get, by effort (tiered)

- **Tier 0 -- raw signals (zero config):** rustlib emits the
  per-transport scale signals it can know LOCALLY (Kafka consumer-group
  lag over THIS pod's assigned partitions, http/grpc in-flight + shed).
- **Tier 1 -- gratis compound (zero CEL):**
  `{ns}_transport_inbound_pressure_ratio` (and `_outbound_`) -- a
  normalised 0-1 signal that picks the right inbound metric by transport
  kind. Point KEDA's Prometheus scaler straight at it.
- **Tier 2 -- the smart default (zero app code):** the engine emits
  `{ns}_scaling_pressure{name="default"}` = `max(CPU, inbound)` gated by
  circuit-open, on a periodic tick.
- **Tier 3 -- your correlated composite (config):** define CEL
  expression(s) over the local context, including app-pushed DOMAIN
  signals (`custom.<name>`, see Wiring) -- e.g. a fetcher's cloud-API
  pending-fetch backlog, a loader's ClickHouse insert backlog.

### The smart default

```text
circuit_open ? 0 : 100 * min(1, max(cpu_utilisation_ratio / cpu_target,
                                    transport_inbound_pressure_ratio))
```

`cpu_target` defaults to 0.70. Outbound pressure is EMITTED but not in
the default (downstream-bound -- more pods rarely relieve a saturated
sink; a dead sink is the circuit gate). Memory is excluded -- it is
self-regulation's (vertical) job and reaches scale-out only indirectly
via lag.

### Config (cascade, `scaling` key)

```yaml
scaling:
  enabled: true
  interval_secs: 15
  transport:
    inbound: kafka          # picks the inbound compound signal
    outbound: kafka
  params:
    cpu_target: 0.70
    lag_target: 50000       # PER-POD (see sizing)
  pressures: []             # empty => the smart default
```

CEL context: top-level `cpu_utilisation_ratio`, `circuit_open`,
`transport_inbound_pressure_ratio`, `transport_outbound_pressure_ratio`,
`memory_ratio`; `params.<key>`; the FIXED transport `metrics.<signal>`
(`kafka_assigned_lag`, `inflight`, `shed_rate`, ...); and app-pushed
DOMAIN signals under a SEPARATE `custom.<name>` map (rustlib 2.8.11).
CEL has no `max()` -- use a ternary.

Validation at LOAD: the expression is COMPILED (syntax errors and
unknown TOP-LEVEL identifiers are hard-rejected -> fall back to the
smart default with a loud, operator-facing error). A reference to a
`custom.<name>` (or any map key not present at load) is NOT a hard
error -- those domain signals are pushed at RUNTIME, so a load-time
dry-run cannot pre-populate them. Such a reference is kept with a load
`warn!`; if it still errors at tick time (signal never pushed), that
single pressure falls back to its last-good value, then to the smart
default. The runtime guard is the safety net -- startup never fails for
a `custom.*` reference.

### Multi-output

`pressures` may list N expressions -> N `{ns}_scaling_pressure{name=...}`
gauges. HPA/KEDA evaluate every trigger and scale to the MAX (a failed
metric never forces scale-down). Emitting several is fine -- current
best practice, not the old "one metric only".

### Per-pod sizing + consumption (IMPORTANT)

rustlib emits ONLY what a pod can know locally -- NO peer/replica count.
Kafka lag is summed over THIS pod's ASSIGNED partitions, so it is
inherently PER-POD and scale-invariant: as the group grows, each pod's
lag falls. Size `lag_target` as "messages one pod tolerates" =
`per_pod_throughput * tolerable_backlog_seconds` (KEDA `lagThreshold`
semantics; the toy default of 10 is almost always wrong).

Push the "divide by replicas" to the autoscaler:

- raw lag (messages) -> `sum() + AverageValue`
  (== `ceil(total / per-pod-target)`; immune to idle-pod dilution).
- normalised ratios + the composite -> `avg() + Value` (or
  AverageValue). NEVER `sum()` a ratio.

Cap `maxReplicas` at the partition count -- beyond it pods sit idle and
dilute an `avg()`.

### Wiring (apps)

`ServiceRuntime` builds the engine, runs its tick (CPU sampled
internally), and exposes `scaling_signals` -- a lock-free cell. Push
your per-pod signals from your receive/send loops:

```rust
runtime.scaling_signals.set_kafka_assigned_lag(lag as f64);
runtime.scaling_signals.set_circuit_open(breaker.is_open());
```

For the 8 FIXED transport signals there is a typed setter (above). For a
DOMAIN signal -- anything rustlib cannot know (a cloud-API backlog, an
upstream throttle, a downstream insert backlog) -- push it by name with
`set_custom` and reference it in a pressure as `custom.<name>`:

```rust
// fetcher: cloud-API pending-fetch backlog + provider throttle
runtime.scaling_signals.set_custom("pending_fetch", queue.len() as f64);
runtime.scaling_signals.set_custom("api_throttle", throttle_ratio);
```

```yaml
scaling:
  params:
    fetch_target: 500        # PER-POD backlog one pod tolerates
  pressures:
    - name: fetch
      # CEL has no max() -- ternary picks the worse of backlog vs throttle
      expression: >
        custom.pending_fetch / params.fetch_target > custom.api_throttle
        ? custom.pending_fetch / params.fetch_target
        : custom.api_throttle
```

If your inbound is NOT a rustlib transport (e.g. cloud-API polling), the
compound inbound is 0 and the default reduces to CPU-only -- add a
DOMAIN term (Tier 3) via `set_custom` for your real backlog signal. A
loader with a ClickHouse sink is the same story: push the insert backlog
with `set_custom("clickhouse_backlog", n)` and scale on
`custom.clickhouse_backlog / params.ch_target`.

### Emit your scaling signals as metrics

The engine can only correlate what exists. If anything in YOUR app could
factor into scaling -- a queue depth, an upstream rate-limit, a
cache-miss storm -- emit it as a metric and reference it in a pressure
expression. See [../core-pillars/METRICS.md](../core-pillars/METRICS.md)
and [MIGRATIONS.md](../MIGRATIONS.md).

---

## `ScalingPressure` -- app-level signal (legacy weighted model)

> Superseded by the engine above for the scale-out signal; retained for
> worker-pool saturation feedback. New apps should use `ScalingEngine`.

KEDA's built-in scalers see infrastructure metrics (Kafka lag, CPU),
not *internal* pipeline state -- buffer depth, batch formation rate,
memory headroom, circuit-breaker status. `ScalingPressure` lets the
app publish a composite 0.0-100.0 score that a Prometheus-trigger KEDA
scaler reads.

```rust
use hyperi_rustlib::scaling::{ScalingPressure, ScalingPressureConfig, ScalingComponent};

let pressure = ScalingPressure::new(
    ScalingPressureConfig::default(),
    vec![
        ScalingComponent::new("kafka_lag",    0.35, 100_000.0),
        ScalingComponent::new("buffer_depth", 0.25,  10_000.0),
        ScalingComponent::new("memory",       0.40,        1.0),
    ],
);

// Lock-free updates from anywhere
pressure.set_component("kafka_lag", 50_000.0);
pressure.set_memory(400_000_000, 1_000_000_000);
```

Two **hard gates** short-circuit the weighted composite. They are
checked in order -- circuit breaker first, so it wins over the memory
gate when both fire:

| Gate | Trigger | Result |
|------|---------|--------|
| Circuit-breaker open | Downstream sink unreachable | `0.0` -- scaling won't help |
| Memory >= threshold | Pod approaching OOM | `100.0` -- scale before kill |

Outside the gates, components are weighted (sum to 1.0) and each
saturates at its configured ceiling. The app surfaces the score as a
Prometheus gauge (the `DfeMetrics` helper exposes it as
`dfe_scaling_pressure`) for KEDA's Prometheus trigger to consume.

---

## `/scaling/pressure` endpoint

Attach `ScalingPressure` to the metrics manager via
`MetricsManager::set_scaling_pressure(...)`. The metrics HTTP server
(started with `start_server_with_routes`) then mounts a
`/scaling/pressure` route returning the current value as plain text --
useful as a KEDA `metrics-api` trigger source without standing up a
Prometheus query.

See [../../src/metrics/mod.rs](../../src/metrics/mod.rs) for the attach
API and [../../src/scaling/mod.rs](../../src/scaling/mod.rs) for the
pressure pipeline.

---

## CPU split

CPU is **not** part of the `ScalingPressure` composite. KEDA's native
CPU trigger reads container-level CPU from the K8s metrics-server --
the right source, since the app shouldn't measure its own CPU.
Configure both triggers independently in the `ScaledObject`:

- pressure gauge -> Prometheus scaler (app-level signals)
- CPU utilisation -> CPU scaler (container-level, via metrics-server)

KEDA takes the max; either fires scale-out independently.

---

## API surface

| Item | Purpose |
|------|---------|
| `KedaConfig` | Cascade-loaded runtime config (`enabled`, thresholds) |
| `KedaContract` | Deployment-time subset (no `enabled` field -- presence implies enabled) |
| `KedaContract::from_config(&cfg)` | Build contract from config |
| `KedaContract::default()` | Defaults table above |
| `ScalingPressure` | Composite pressure source -- see `scaling/` module |
| `ScalingPressureConfig` | Gate thresholds (`memory_gate_threshold`, `enabled`) |
| `ScalingComponent` | Single weighted component |
| `PressureSnapshot` / `ComponentSnapshot` / `GateType` | Introspection types |

---

## Related

- [CONTRACT.md](CONTRACT.md) -- `keda: Option<KedaContract>` field
- [ARTEFACTS.md](ARTEFACTS.md) -- when KEDA templates are written
- [../core-pillars/METRICS.md](../core-pillars/METRICS.md) -- metric
  exposition pipeline that powers Prometheus triggers
- Source: [../../src/deployment/keda.rs](../../src/deployment/keda.rs),
  [../../src/scaling/mod.rs](../../src/scaling/mod.rs),
  [../../src/scaling/pressure.rs](../../src/scaling/pressure.rs)
