# KEDA

KEDA (Kubernetes Event-driven Autoscaling) scales pods on triggers
the standard HPA can't see -- Kafka consumer-group lag, Prometheus
queries, cron schedules, queue depth. `KedaContract` is the
deployment-side declaration; `ScalingPressure` is the runtime-side
signal source. Together they make scale-out track pipeline pressure,
not container CPU.

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
```

KEDA scales to the **max** of all triggers -- high lag OR high CPU
grows the pool; both must subside to shrink.

The `TriggerAuthentication` template wires SASL credentials from the
`kafka` secret group. With no `kafka` group, no `TriggerAuthentication`
is written and `authenticationRef` is omitted -- useful where Kafka
auth is bypassed via mesh mTLS.

---

## `ScalingPressure` -- app-level signal

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
