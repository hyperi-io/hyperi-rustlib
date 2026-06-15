// Project:   hyperi-rustlib
// File:      src/scaling/transport_pressure.rs
// Purpose:   Compound, per-pod transport scaling pressure (gratis default)
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Compound transport scaling pressure -- the "works most of the time" inbound
//! (and outbound) signal (scaling ACR + spec 5c). rustlib provides the COMPUTOR,
//! the compound gauge, and the CPU term gratis; the per-pod transport SIGNALS
//! are fed by the app (or a governed receiver) via `ScalingSignalsCell` -- they
//! are NOT auto-collected, so an app that pushes nothing gets a CPU-only default.
//!
//! rustlib computes ONE normalised inbound pressure (and one outbound) from the
//! signals a pod can know LOCALLY -- no peer/replica count. The conditional
//! "which signal for which transport" lives HERE, once, instead of in every
//! app's CEL expression:
//!
//! - **Kafka** -> lag over THIS instance's assigned partitions / `lag_target`
//!   (inherently per-pod; falls as the group grows).
//! - **Redis** -> this consumer's pending / `redis_lag_target`.
//! - **HTTP / gRPC** -> this pod's in-flight / concurrency target, `max`'d with
//!   its shed rate / `shed_target`.
//! - **file / pipe / memory** -> 0 (not horizontally scalable).
//!
//! Ratios are floored at 0 and left UNCLAMPED above 1.0 so a KEDA Prometheus
//! scaler (Value/AverageValue target) gets proportional scale-up; the smart
//! default composite applies the `min(1, ...)` bound when producing the 0-100
//! `scaling_pressure`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Inbound/outbound transport kind, for scaling-signal selection.
///
/// Deliberately separate from [`crate::metrics::TransportKind`] -- this carries
/// the *scaling* semantics (is-horizontally-scalable, which signal to read).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScalingTransport {
    /// Apache Kafka consumer (pull, durable backlog).
    Kafka,
    /// Redis Streams consumer (pull, durable backlog).
    Redis,
    /// HTTP server (push originator).
    Http,
    /// gRPC server (push originator).
    Grpc,
    /// File source (single sequential reader).
    File,
    /// Pipe / stdin (forward-only).
    Pipe,
    /// In-process memory transport (test / loopback).
    Memory,
    /// Anything else / not classified.
    Other,
}

impl ScalingTransport {
    /// Parse from a transport label (case-insensitive). Unknown -> [`Self::Other`].
    #[must_use]
    pub fn from_label(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "kafka" => Self::Kafka,
            "redis" | "redis_stream" | "redis-streams" | "redisstream" => Self::Redis,
            "http" => Self::Http,
            "grpc" => Self::Grpc,
            "file" => Self::File,
            "pipe" | "stdin" => Self::Pipe,
            "memory" => Self::Memory,
            _ => Self::Other,
        }
    }

    /// Whether adding pods can relieve load on this inbound transport -- i.e. it
    /// has a durable external backlog (kafka/redis) or LB-distributed load
    /// (http/grpc). file/pipe/memory cannot be scaled out.
    #[must_use]
    pub fn is_horizontally_scalable_inbound(self) -> bool {
        matches!(self, Self::Kafka | Self::Redis | Self::Http | Self::Grpc)
    }
}

/// Per-pod, locally-knowable scaling signals, sampled each scaling tick.
///
/// EVERY field is what THIS instance knows from its own state or its
/// broker/coordinator session -- no peer/replica assumptions (scaling ACR
/// principle 1). `None` means "not applicable / not yet known" and contributes
/// 0 to the pressure (never NaN).
#[derive(Debug, Clone, Default)]
pub struct TransportSignals {
    /// Kafka: summed lag over THIS instance's ASSIGNED partitions (messages).
    pub kafka_assigned_lag: Option<f64>,
    /// Redis: this consumer's pending / per-consumer group lag (messages).
    pub redis_pending: Option<f64>,
    /// HTTP/gRPC: this pod's in-flight request count.
    pub inflight: Option<f64>,
    /// HTTP/gRPC: this pod's shed/reject rate (events per second).
    pub shed_rate: Option<f64>,
    /// Outbound: send-backpressure rate (events per second).
    pub send_backpressure_rate: Option<f64>,
    /// Outbound: refused/dropped rate (events per second).
    pub refused_rate: Option<f64>,
    /// Outbound: producer/sink queue depth (messages).
    pub produce_queue_depth: Option<f64>,
    /// Outbound circuit breaker open (sink dead -> the composite gate).
    pub circuit_open: bool,
}

/// Per-pod normalisation targets (KEDA `lagThreshold`-style: "what ONE pod
/// tolerates"). Pulled from the config `params` map with researched defaults.
#[derive(Debug, Clone)]
pub struct PressureTargets {
    /// Kafka per-pod lag target. NO universal default (spec) -- absent => the
    /// kafka term contributes 0 (never NaN).
    pub lag_target: Option<f64>,
    /// Redis per-pod lag target. Absent => the redis term contributes 0.
    pub redis_lag_target: Option<f64>,
    /// HTTP per-pod in-flight concurrency target (KEDA http-add-on ref: 100).
    pub http_concurrency_target: f64,
    /// gRPC per-pod in-flight concurrency target.
    pub grpc_concurrency_target: f64,
    /// Shed/reject rate that counts as full overload (events/sec).
    pub shed_target: f64,
    /// Outbound producer/sink queue-depth target. Absent => term 0.
    pub produce_queue_target: Option<f64>,
}

impl PressureTargets {
    /// Build from the config `params` map. Transport targets without a
    /// universally-safe default are left `None` (their term contributes 0 until
    /// the operator sizes them); concurrency/shed get researched defaults.
    #[must_use]
    pub fn from_params(params: &std::collections::BTreeMap<String, f64>) -> Self {
        let get = |k: &str| params.get(k).copied();
        Self {
            lag_target: get("lag_target"),
            redis_lag_target: get("redis_lag_target"),
            http_concurrency_target: get("http_concurrency_target").unwrap_or(100.0),
            grpc_concurrency_target: get("grpc_concurrency_target").unwrap_or(100.0),
            shed_target: get("shed_target").unwrap_or(10.0),
            produce_queue_target: get("produce_queue_target"),
        }
    }
}

/// `value / target` floored at 0; 0 when the target is unset/<=0 or the value is
/// non-finite (never NaN/Inf into the pressure).
fn ratio_opt(value: f64, target: Option<f64>) -> f64 {
    match target {
        Some(t) if t > 0.0 && value.is_finite() => (value / t).max(0.0),
        _ => 0.0,
    }
}

/// `value / target` floored at 0, for targets that always have a default.
fn ratio(value: f64, target: f64) -> f64 {
    if target > 0.0 && value.is_finite() {
        (value / target).max(0.0)
    } else {
        0.0
    }
}

/// Compound INBOUND pressure ratio (per-pod; >=0, unclamped above 1.0 for
/// proportional scale-up). Picks the signal by the configured inbound kind.
#[must_use]
pub fn inbound_pressure(kind: ScalingTransport, s: &TransportSignals, t: &PressureTargets) -> f64 {
    match kind {
        ScalingTransport::Kafka => ratio_opt(s.kafka_assigned_lag.unwrap_or(0.0), t.lag_target),
        ScalingTransport::Redis => ratio_opt(s.redis_pending.unwrap_or(0.0), t.redis_lag_target),
        ScalingTransport::Http => {
            let conc = ratio(s.inflight.unwrap_or(0.0), t.http_concurrency_target);
            let shed = ratio(s.shed_rate.unwrap_or(0.0), t.shed_target);
            conc.max(shed)
        }
        ScalingTransport::Grpc => {
            let conc = ratio(s.inflight.unwrap_or(0.0), t.grpc_concurrency_target);
            let shed = ratio(s.shed_rate.unwrap_or(0.0), t.shed_target);
            conc.max(shed)
        }
        // file/pipe/memory/other: not horizontally scalable -> CPU + gate only.
        _ => 0.0,
    }
}

/// Compound OUTBOUND pressure ratio (per-pod). EMIT-ONLY by default -- NOT in
/// the smart-default composite (downstream-bound; more pods rarely relieve a
/// saturated sink -- scaling ACR). A dead sink surfaces as the circuit gate in
/// the composite, not here.
#[must_use]
pub fn outbound_pressure(s: &TransportSignals, t: &PressureTargets) -> f64 {
    let bp = ratio(s.send_backpressure_rate.unwrap_or(0.0), t.shed_target);
    let refused = ratio(s.refused_rate.unwrap_or(0.0), t.shed_target);
    let queue = ratio_opt(s.produce_queue_depth.unwrap_or(0.0), t.produce_queue_target);
    bp.max(refused).max(queue)
}

/// Lock-free cell the app / transport updates with the current per-pod signals;
/// the engine tick reads a [`snapshot`](Self::snapshot). A NaN bit-pattern means
/// "absent" -> `None` in the snapshot (contributes 0 to the pressure).
///
/// CPU is sampled by the engine tick itself (process cumulative / cores), so it
/// is NOT in this cell -- the cell carries only the transport-side signals an
/// app pushes from its receive/send loops.
#[derive(Debug)]
pub struct ScalingSignalsCell {
    kafka_assigned_lag: AtomicU64,
    redis_pending: AtomicU64,
    inflight: AtomicU64,
    shed_rate: AtomicU64,
    send_backpressure_rate: AtomicU64,
    refused_rate: AtomicU64,
    produce_queue_depth: AtomicU64,
    circuit_open: AtomicBool,
}

impl Default for ScalingSignalsCell {
    fn default() -> Self {
        let absent = || AtomicU64::new(f64::NAN.to_bits());
        Self {
            kafka_assigned_lag: absent(),
            redis_pending: absent(),
            inflight: absent(),
            shed_rate: absent(),
            send_backpressure_rate: absent(),
            refused_rate: absent(),
            produce_queue_depth: absent(),
            circuit_open: AtomicBool::new(false),
        }
    }
}

impl ScalingSignalsCell {
    /// Create an empty cell (all signals absent, circuit closed).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set Kafka per-pod assigned-partition lag (messages).
    pub fn set_kafka_assigned_lag(&self, v: f64) {
        self.kafka_assigned_lag
            .store(v.to_bits(), Ordering::Relaxed);
    }
    /// Set Redis per-consumer pending / lag (messages).
    pub fn set_redis_pending(&self, v: f64) {
        self.redis_pending.store(v.to_bits(), Ordering::Relaxed);
    }
    /// Set this pod's in-flight request count (http/grpc).
    pub fn set_inflight(&self, v: f64) {
        self.inflight.store(v.to_bits(), Ordering::Relaxed);
    }
    /// Set this pod's shed/reject rate (events/sec).
    pub fn set_shed_rate(&self, v: f64) {
        self.shed_rate.store(v.to_bits(), Ordering::Relaxed);
    }
    /// Set outbound send-backpressure rate (events/sec).
    pub fn set_send_backpressure_rate(&self, v: f64) {
        self.send_backpressure_rate
            .store(v.to_bits(), Ordering::Relaxed);
    }
    /// Set outbound refused/dropped rate (events/sec).
    pub fn set_refused_rate(&self, v: f64) {
        self.refused_rate.store(v.to_bits(), Ordering::Relaxed);
    }
    /// Set outbound producer/sink queue depth (messages).
    pub fn set_produce_queue_depth(&self, v: f64) {
        self.produce_queue_depth
            .store(v.to_bits(), Ordering::Relaxed);
    }
    /// Set the outbound circuit-breaker state (the only default gate).
    pub fn set_circuit_open(&self, open: bool) {
        self.circuit_open.store(open, Ordering::Relaxed);
    }

    /// Read a consistent-enough snapshot for this tick (Relaxed -- a tick is a
    /// periodic best-effort sample, not a linearisation point).
    #[must_use]
    pub fn snapshot(&self) -> TransportSignals {
        let read = |a: &AtomicU64| -> Option<f64> {
            let v = f64::from_bits(a.load(Ordering::Relaxed));
            if v.is_nan() { None } else { Some(v) }
        };
        TransportSignals {
            kafka_assigned_lag: read(&self.kafka_assigned_lag),
            redis_pending: read(&self.redis_pending),
            inflight: read(&self.inflight),
            shed_rate: read(&self.shed_rate),
            send_backpressure_rate: read(&self.send_backpressure_rate),
            refused_rate: read(&self.refused_rate),
            produce_queue_depth: read(&self.produce_queue_depth),
            circuit_open: self.circuit_open.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn targets(pairs: &[(&str, f64)]) -> PressureTargets {
        let mut m = BTreeMap::new();
        for (k, v) in pairs {
            m.insert((*k).to_string(), *v);
        }
        PressureTargets::from_params(&m)
    }

    #[test]
    fn kind_from_label() {
        assert_eq!(
            ScalingTransport::from_label("Kafka"),
            ScalingTransport::Kafka
        );
        assert_eq!(
            ScalingTransport::from_label("redis-streams"),
            ScalingTransport::Redis
        );
        assert_eq!(ScalingTransport::from_label("grpc"), ScalingTransport::Grpc);
        assert_eq!(
            ScalingTransport::from_label("nonsense"),
            ScalingTransport::Other
        );
    }

    #[test]
    fn horizontally_scalable_classification() {
        for k in [
            ScalingTransport::Kafka,
            ScalingTransport::Redis,
            ScalingTransport::Http,
            ScalingTransport::Grpc,
        ] {
            assert!(k.is_horizontally_scalable_inbound(), "{k:?}");
        }
        for k in [
            ScalingTransport::File,
            ScalingTransport::Pipe,
            ScalingTransport::Memory,
            ScalingTransport::Other,
        ] {
            assert!(!k.is_horizontally_scalable_inbound(), "{k:?}");
        }
    }

    #[test]
    fn kafka_lag_needs_a_target_else_zero() {
        let s = TransportSignals {
            kafka_assigned_lag: Some(50_000.0),
            ..Default::default()
        };
        // No lag_target -> term is 0 (never NaN), per spec.
        let t = targets(&[]);
        assert!(inbound_pressure(ScalingTransport::Kafka, &s, &t).abs() < f64::EPSILON);
        // With a per-pod target it normalises.
        let t = targets(&[("lag_target", 100_000.0)]);
        assert!((inbound_pressure(ScalingTransport::Kafka, &s, &t) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn kafka_lag_unclamped_above_one() {
        let s = TransportSignals {
            kafka_assigned_lag: Some(250_000.0),
            ..Default::default()
        };
        let t = targets(&[("lag_target", 100_000.0)]);
        // 2.5 -- left unclamped for proportional scale-up.
        assert!((inbound_pressure(ScalingTransport::Kafka, &s, &t) - 2.5).abs() < 1e-9);
    }

    #[test]
    fn http_takes_max_of_inflight_and_shed() {
        // in-flight 50/100 = 0.5; shed 8/10 = 0.8 -> max 0.8 (confirmed overload outvotes).
        let s = TransportSignals {
            inflight: Some(50.0),
            shed_rate: Some(8.0),
            ..Default::default()
        };
        let t = targets(&[("http_concurrency_target", 100.0), ("shed_target", 10.0)]);
        assert!((inbound_pressure(ScalingTransport::Http, &s, &t) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn non_scalable_inbound_is_zero() {
        let s = TransportSignals {
            kafka_assigned_lag: Some(999.0),
            inflight: Some(999.0),
            ..Default::default()
        };
        let t = targets(&[("lag_target", 1.0), ("http_concurrency_target", 1.0)]);
        for k in [
            ScalingTransport::File,
            ScalingTransport::Pipe,
            ScalingTransport::Memory,
        ] {
            assert!(inbound_pressure(k, &s, &t).abs() < f64::EPSILON, "{k:?}");
        }
    }

    #[test]
    fn nan_inputs_never_propagate() {
        let s = TransportSignals {
            kafka_assigned_lag: Some(f64::NAN),
            inflight: Some(f64::INFINITY),
            ..Default::default()
        };
        let t = targets(&[("lag_target", 100.0), ("http_concurrency_target", 100.0)]);
        assert!(inbound_pressure(ScalingTransport::Kafka, &s, &t).abs() < f64::EPSILON);
        assert!(inbound_pressure(ScalingTransport::Http, &s, &t).abs() < f64::EPSILON);
    }

    #[test]
    fn outbound_composes_but_defaults_zero() {
        let s = TransportSignals::default();
        let t = targets(&[]);
        assert!(outbound_pressure(&s, &t).abs() < f64::EPSILON);
        let s = TransportSignals {
            refused_rate: Some(20.0),
            ..Default::default()
        };
        let t = targets(&[("shed_target", 10.0)]);
        assert!((outbound_pressure(&s, &t) - 2.0).abs() < 1e-9);
    }
}
