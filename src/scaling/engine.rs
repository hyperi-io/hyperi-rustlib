// Project:   hyperi-rustlib
// File:      src/scaling/engine.rs
// Purpose:   Horizontal scaling-pressure engine (CEL over local metrics)
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! The horizontal scaling-pressure ENGINE: CEL expressions evaluated over local,
//! correlated signals on the periodic scaling tick, producing N named
//! `scaling_pressure` gauges for any horizontal autoscaler (KEDA is the prime
//! tool; the engine is autoscaler-neutral).
//!
//! ## The correlated composite (rustlib's edge)
//!
//! An autoscaler sees only coarse, top-level, single metrics. An APP has rich
//! LOCAL context and can COMBINE + CORRELATE it -- CPU, the compound transport
//! pressure, and any domain signal -- into one **correlated composite**
//! pressure. That is the whole point of this engine (scaling ACR principle 4).
//!
//! ## Precedence
//!
//! `config.scaling.pressures` (CEL) > app-plumbed default > rustlib's
//! context-aware smart default (computed in Rust; used when no expressions are
//! configured, and as the per-pressure fallback when one fails).
//!
//! ## Smart default (context-aware, per spec section 4)
//!
//! ```text
//! circuit_open ? 0 : 100 * min(1, max(cpu_utilisation_ratio / cpu_target,
//!                                     transport_inbound_pressure_ratio))
//! ```
//!
//! Outbound pressure is EMITTED (gratis) but not in the default max -- it is
//! downstream-bound (scaling ACR). Memory is excluded (self-regulation's job).

use std::collections::HashMap;

use cel::{ExecutionError, Program, Value};
use parking_lot::Mutex;
use serde_json::json;

use super::config::ScalingEngineConfig;
use super::transport_pressure::{
    PressureTargets, ScalingTransport, TransportSignals, inbound_pressure, outbound_pressure,
};

/// One configured pressure output. `program` is `None` for the rustlib smart
/// default (computed in Rust) and for any user expression that failed to
/// compile/validate (it falls back to the smart default, loudly).
struct CompiledPressure {
    name: String,
    program: Option<Program>,
    enabled: bool,
}

/// The CEL-over-local-metrics horizontal scaling-pressure engine.
///
/// Construct with [`ScalingEngine::new`], then call [`ScalingEngine::tick`] on
/// the periodic scaling interval (NOT the data hot-path). Evaluation errors hold
/// the last-good value (fail-safe), missing signals contribute 0 (never NaN).
pub struct ScalingEngine {
    #[cfg_attr(not(feature = "metrics"), allow(dead_code))]
    namespace: String,
    enabled: bool,
    cpu_target: f64,
    targets: PressureTargets,
    inbound_kind: ScalingTransport,
    outbound_kind: ScalingTransport,
    params: std::collections::BTreeMap<String, f64>,
    pressures: Vec<CompiledPressure>,
    /// Last successfully-evaluated value per pressure name (fail-safe hold).
    last_good: Mutex<HashMap<String, f64>>,
}

impl ScalingEngine {
    /// Build the engine from config + the resolved inbound/outbound transport
    /// kinds. Returns the engine plus a list of friendly, operator-facing
    /// validation errors (each failing expression falls back to the smart
    /// default; the caller should log these LOUDLY).
    ///
    /// `inbound`/`outbound` come from config (`scaling.transport.*`) or are
    /// auto-derived by the runtime from the transports it built.
    #[must_use]
    pub fn new(
        namespace: &str,
        config: &ScalingEngineConfig,
        inbound: ScalingTransport,
        outbound: ScalingTransport,
    ) -> (Self, Vec<String>) {
        let targets = PressureTargets::from_params(&config.params);
        let cpu_target = config.cpu_target();
        let mut errors = Vec::new();

        let pressures: Vec<CompiledPressure> = if config.pressures.is_empty() {
            // No expressions configured -> the rustlib smart default.
            vec![CompiledPressure {
                name: "default".to_string(),
                program: None,
                enabled: true,
            }]
        } else {
            config
                .pressures
                .iter()
                .map(|p| {
                    let program = if p.enabled {
                        match compile_and_check(&p.expression, &config.params) {
                            Ok(prog) => Some(prog),
                            Err(msg) => {
                                errors.push(format!(
                                    "scaling pressure '{}' is invalid -- falling back to the \
                                     rustlib smart default. {msg}",
                                    p.name
                                ));
                                None
                            }
                        }
                    } else {
                        None
                    };
                    CompiledPressure {
                        name: p.name.clone(),
                        program,
                        enabled: p.enabled,
                    }
                })
                .collect()
        };

        // Duplicate names would collide on one gauge series + share a last_good
        // slot -- flag at load, consistent with the rest of the validation.
        {
            let mut seen = std::collections::HashSet::new();
            for p in &pressures {
                if !seen.insert(p.name.as_str()) {
                    errors.push(format!(
                        "duplicate scaling pressure name '{}' -- names must be unique",
                        p.name
                    ));
                }
            }
        }

        let engine = Self {
            namespace: namespace.to_string(),
            enabled: config.enabled,
            cpu_target,
            targets,
            inbound_kind: inbound,
            outbound_kind: outbound,
            params: config.params.clone(),
            pressures,
            last_good: Mutex::new(HashMap::new()),
        };
        (engine, errors)
    }

    /// Whether the engine is enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The resolved inbound transport kind (drives the compound inbound term).
    #[must_use]
    pub fn inbound_kind(&self) -> ScalingTransport {
        self.inbound_kind
    }

    /// The resolved outbound transport kind.
    #[must_use]
    pub fn outbound_kind(&self) -> ScalingTransport {
        self.outbound_kind
    }

    /// The rustlib context-aware smart default (0-100). Pure; no CEL.
    #[must_use]
    fn smart_default(&self, cpu_ratio: f64, inbound: f64, circuit_open: bool) -> f64 {
        if circuit_open {
            return 0.0;
        }
        let cpu_term = if self.cpu_target > 0.0 {
            cpu_ratio / self.cpu_target
        } else {
            0.0
        };
        let composite = cpu_term.max(inbound);
        100.0 * composite.clamp(0.0, 1.0)
    }

    /// Evaluate all configured pressures for the current tick.
    ///
    /// Returns `(name, value)` per enabled pressure. Pure (no metric emission);
    /// [`tick`](Self::tick) wraps this and publishes the gauges.
    #[must_use]
    pub fn evaluate(
        &self,
        signals: &TransportSignals,
        cpu_ratio: f64,
        memory_ratio: f64,
    ) -> Vec<(String, f64)> {
        let inbound = inbound_pressure(self.inbound_kind, signals, &self.targets);
        let outbound = outbound_pressure(signals, &self.targets);

        // Build the CEL context ONCE per tick (only if a pressure needs it).
        let ctx = if self
            .pressures
            .iter()
            .any(|p| p.enabled && p.program.is_some())
        {
            Some(self.eval_context(signals, cpu_ratio, inbound, outbound, memory_ratio))
        } else {
            None
        };

        let mut out = Vec::with_capacity(self.pressures.len());
        for p in &self.pressures {
            if !p.enabled {
                continue;
            }
            let value = match &p.program {
                // Smart default (no expression, or failed compile).
                None => self.smart_default(cpu_ratio, inbound, signals.circuit_open),
                Some(program) => {
                    let evaluated = ctx.as_ref().and_then(|m| eval_program(program, m));
                    match evaluated {
                        Some(v) if v.is_finite() => {
                            self.last_good.lock().insert(p.name.clone(), v);
                            v
                        }
                        // Eval error / non-numeric -> hold last-good, else fall
                        // back to the smart default (fail-safe; never panic/NaN).
                        _ => self
                            .last_good
                            .lock()
                            .get(&p.name)
                            .copied()
                            .unwrap_or_else(|| {
                                self.smart_default(cpu_ratio, inbound, signals.circuit_open)
                            }),
                    }
                }
            };
            out.push((p.name.clone(), value));
        }
        out
    }

    /// Evaluate and publish the gauges for this tick.
    ///
    /// Emits `{ns}_scaling_pressure{name=...}` per pressure, plus the gratis
    /// compound `{ns}_transport_inbound_pressure_ratio` /
    /// `{ns}_transport_outbound_pressure_ratio` and `{ns}_scaling_circuit_open`.
    #[allow(unused_variables)]
    pub fn tick(&self, signals: &TransportSignals, cpu_ratio: f64, memory_ratio: f64) {
        if !self.enabled {
            return;
        }
        let inbound = inbound_pressure(self.inbound_kind, signals, &self.targets);
        let outbound = outbound_pressure(signals, &self.targets);
        let values = self.evaluate(signals, cpu_ratio, memory_ratio);

        #[cfg(feature = "metrics")]
        {
            let ns = &self.namespace;
            for (name, value) in &values {
                metrics::gauge!(format!("{ns}_scaling_pressure"), "name" => name.clone())
                    .set(*value);
            }
            // Gratis compound transport pressure (IN and OUT) -- observability +
            // KEDA-direct. Ratios floored at 0, unclamped above for proportional
            // scale-up.
            metrics::gauge!(format!("{ns}_transport_inbound_pressure_ratio")).set(inbound);
            metrics::gauge!(format!("{ns}_transport_outbound_pressure_ratio")).set(outbound);
            // 0/1 state gauge (NOT a bool type) -- the only default gate.
            metrics::gauge!(format!("{ns}_scaling_circuit_open")).set(if signals.circuit_open {
                1.0
            } else {
                0.0
            });
        }
    }

    /// Build the CEL evaluation context (derived vars + `params` + `metrics`).
    fn eval_context(
        &self,
        signals: &TransportSignals,
        cpu_ratio: f64,
        inbound: f64,
        outbound: f64,
        memory_ratio: f64,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        m.insert("cpu_utilisation_ratio".into(), json!(cpu_ratio));
        m.insert("circuit_open".into(), json!(signals.circuit_open));
        m.insert("transport_inbound_pressure_ratio".into(), json!(inbound));
        m.insert("transport_outbound_pressure_ratio".into(), json!(outbound));
        m.insert("memory_ratio".into(), json!(memory_ratio));

        let params: serde_json::Map<String, serde_json::Value> = self
            .params
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        m.insert("params".into(), serde_json::Value::Object(params));

        m.insert(
            "metrics".into(),
            serde_json::Value::Object(signal_metrics(signals)),
        );

        // App-pushed DOMAIN signals under a DEDICATED `custom` map (so
        // expressions use `custom.<name>`), SEPARATE from the strict `metrics`
        // map of fixed transport signals. Names are not known at config-load,
        // so this map is whatever the app has pushed via `set_custom`.
        let custom: serde_json::Map<String, serde_json::Value> = signals
            .custom
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        m.insert("custom".into(), serde_json::Value::Object(custom));
        m
    }

    /// Operator-facing list of identifiers available to expressions.
    #[must_use]
    pub fn available_surface(&self) -> String {
        let params: Vec<&str> = self.params.keys().map(String::as_str).collect();
        format!(
            "top-level: cpu_utilisation_ratio, circuit_open, \
             transport_inbound_pressure_ratio, transport_outbound_pressure_ratio, memory_ratio; \
             params.{{{}}}; metrics.{{kafka_assigned_lag, redis_pending, inflight, shed_rate, \
             send_backpressure_rate, refused_rate, produce_queue_depth}}; \
             custom.<app-pushed domain signals, validated at runtime not load>",
            params.join(", ")
        )
    }
}

/// The curated `metrics` map exposed to expressions (the scaling-relevant local
/// signals, by name). Absent signals are simply omitted.
fn signal_metrics(s: &TransportSignals) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    let mut put = |k: &str, v: Option<f64>| {
        if let Some(v) = v {
            m.insert(k.to_string(), json!(v));
        }
    };
    put("kafka_assigned_lag", s.kafka_assigned_lag);
    put("redis_pending", s.redis_pending);
    put("inflight", s.inflight);
    put("shed_rate", s.shed_rate);
    put("send_backpressure_rate", s.send_backpressure_rate);
    put("refused_rate", s.refused_rate);
    put("produce_queue_depth", s.produce_queue_depth);
    m
}

/// Compile a user expression and dry-run it against a representative context so
/// unknown identifiers / type errors surface at LOAD, not first tick. Returns a
/// friendly error string on a HARD failure.
///
/// ## Validation contract (custom-signal aware)
///
/// `custom.<name>` domain signals are pushed by the app at RUNTIME and so are
/// NOT present in the load-time dry-run context. We therefore distinguish (via
/// the typed `cel::ExecutionError`):
///
/// - `Program::compile` failure (syntax) -> HARD reject.
/// - dry-run `NoSuchKey` (a missing MAP key, e.g. `custom.<name>` not yet
///   pushed, or any not-yet-present `metrics.*`/`params.*`) -> DOWNGRADE to a
///   load `warn!` and KEEP the program. The runtime guard falls back to
///   last-good / smart-default if it really is broken at tick time.
/// - dry-run `UndeclaredReference` (an unknown TOP-LEVEL identifier -- a typo in
///   a closed set) -> HARD reject.
/// - any other eval error (type mismatch, bad overload, ...) or a non-numeric
///   result -> HARD reject.
fn compile_and_check(
    expr: &str,
    params: &std::collections::BTreeMap<String, f64>,
) -> Result<Program, String> {
    let program = Program::compile(expr).map_err(|e| format!("compile error: {e}"))?;

    // Representative zero-context: all derived vars present, every param key,
    // and the full metrics surface, so a reference to a KNOWN name succeeds and
    // only genuine typos/type errors fail. `custom.*` is deliberately NOT
    // pre-populated -- it is runtime-validated (see the contract above).
    let mut m = serde_json::Map::new();
    m.insert("cpu_utilisation_ratio".into(), json!(0.0));
    m.insert("circuit_open".into(), json!(false));
    m.insert("transport_inbound_pressure_ratio".into(), json!(0.0));
    m.insert("transport_outbound_pressure_ratio".into(), json!(0.0));
    m.insert("memory_ratio".into(), json!(0.0));
    let pmap: serde_json::Map<String, serde_json::Value> =
        params.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    m.insert("params".into(), serde_json::Value::Object(pmap));
    let mut metrics = serde_json::Map::new();
    for k in [
        "kafka_assigned_lag",
        "redis_pending",
        "inflight",
        "shed_rate",
        "send_backpressure_rate",
        "refused_rate",
        "produce_queue_depth",
    ] {
        metrics.insert(k.to_string(), json!(0.0));
    }
    m.insert("metrics".into(), serde_json::Value::Object(metrics));
    // An EMPTY custom map: a reference to `custom.<name>` produces a `NoSuchKey`,
    // which we downgrade below (apps push the real keys at runtime).
    m.insert(
        "custom".into(),
        serde_json::Value::Object(serde_json::Map::new()),
    );

    let surface = || {
        format!(
            "Available -- top-level: cpu_utilisation_ratio, circuit_open, \
             transport_inbound_pressure_ratio, transport_outbound_pressure_ratio, memory_ratio; \
             params.{{{}}}; metrics.{{kafka_assigned_lag, redis_pending, inflight, shed_rate, \
             send_backpressure_rate, refused_rate, produce_queue_depth}}; \
             custom.<app-pushed at runtime>",
            params.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    };

    // Build the dry-run context, then execute capturing the TYPED execution
    // error so we can branch on `NoSuchKey` (a missing map key -- downgrade) vs
    // everything else (hard reject). A `build_context` failure is itself a hard
    // reject (it realistically never errs for these JSON maps).
    let ctx = crate::expression::build_context(m.iter())
        .map_err(|e| format!("context build error: {e}. {}", surface()))?;

    match program.execute(&ctx) {
        Ok(Value::Float(_) | Value::Int(_) | Value::UInt(_)) => Ok(program),
        Ok(other) => Err(format!(
            "expression must evaluate to a number, got {other:?}"
        )),
        // A missing MAP key -- almost always a `custom.<name>` the app pushes at
        // runtime (or a not-yet-present metrics/params field). Do NOT hard-reject:
        // warn and keep the program; the runtime guard is the safety net.
        Err(ExecutionError::NoSuchKey(key)) => {
            tracing::warn!(
                missing_key = %key,
                expression = expr,
                "scaling pressure references a map key not present at load (likely a \
                 custom.<name> domain signal pushed at runtime) -- keeping the expression; \
                 it will be validated on each scaling tick and fall back to the smart \
                 default if it errors."
            );
            Ok(program)
        }
        // Unknown TOP-LEVEL identifier (a typo in the closed set) -> hard reject;
        // ditto any other eval error (type mismatch, bad overload, ...).
        Err(e) => Err(format!("evaluation error: {e}. {}", surface())),
    }
}

/// Execute a compiled program against a JSON context map; returns the f64 value
/// (coercing int/uint/bool) or `None` on any error / non-numeric result.
fn eval_program(
    program: &Program,
    map: &serde_json::Map<String, serde_json::Value>,
) -> Option<f64> {
    let ctx = crate::expression::build_context(map.iter()).ok()?;
    match program.execute(&ctx).ok()? {
        Value::Float(f) => Some(f),
        Value::Int(i) => Some(i as f64),
        Value::UInt(u) => Some(u as f64),
        Value::Bool(b) => Some(if b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scaling::config::{PressureExpr, ScalingEngineConfig};

    fn cfg(pressures: Vec<PressureExpr>, params: &[(&str, f64)]) -> ScalingEngineConfig {
        let mut c = ScalingEngineConfig {
            pressures,
            ..Default::default()
        };
        for (k, v) in params {
            c.params.insert((*k).to_string(), *v);
        }
        c
    }

    #[test]
    fn smart_default_cpu_only_when_no_transport() {
        let (eng, errs) = ScalingEngine::new(
            "t",
            &cfg(vec![], &[("cpu_target", 0.70)]),
            ScalingTransport::File, // non-scalable inbound
            ScalingTransport::Kafka,
        );
        assert!(errs.is_empty());
        // CPU at target -> 100 * (0.70/0.70) = 100.
        let v = eng.evaluate(&TransportSignals::default(), 0.70, 0.0);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "default");
        assert!((v[0].1 - 100.0).abs() < 1e-6);
    }

    #[test]
    fn smart_default_takes_max_of_cpu_and_inbound_kafka() {
        let (eng, _) = ScalingEngine::new(
            "t",
            &cfg(vec![], &[("cpu_target", 0.70), ("lag_target", 100_000.0)]),
            ScalingTransport::Kafka,
            ScalingTransport::Kafka,
        );
        // CPU 0.35 -> cpu_term 0.5; lag 80k/100k = 0.8 -> max 0.8 -> 80.
        let s = TransportSignals {
            kafka_assigned_lag: Some(80_000.0),
            ..Default::default()
        };
        let v = eng.evaluate(&s, 0.35, 0.0);
        assert!((v[0].1 - 80.0).abs() < 1e-6);
    }

    #[test]
    fn circuit_open_gates_to_zero() {
        let (eng, _) = ScalingEngine::new(
            "t",
            &cfg(vec![], &[("cpu_target", 0.70), ("lag_target", 1.0)]),
            ScalingTransport::Kafka,
            ScalingTransport::Kafka,
        );
        let s = TransportSignals {
            kafka_assigned_lag: Some(1_000_000.0),
            circuit_open: true,
            ..Default::default()
        };
        assert!(eng.evaluate(&s, 0.99, 0.0)[0].1.abs() < f64::EPSILON);
    }

    #[test]
    fn user_expression_evaluated() {
        let p = PressureExpr {
            name: "cpu".into(),
            expression: "cpu_utilisation_ratio * 100.0".into(),
            enabled: true,
        };
        let (eng, errs) = ScalingEngine::new(
            "t",
            &cfg(vec![p], &[]),
            ScalingTransport::Kafka,
            ScalingTransport::Kafka,
        );
        assert!(errs.is_empty(), "errors: {errs:?}");
        let v = eng.evaluate(&TransportSignals::default(), 0.42, 0.0);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "cpu");
        assert!((v[0].1 - 42.0).abs() < 1e-6);
    }

    #[test]
    fn user_expression_can_read_params_and_metrics() {
        let p = PressureExpr {
            name: "lag".into(),
            expression: "metrics.kafka_assigned_lag / params.lag_target".into(),
            enabled: true,
        };
        let (eng, errs) = ScalingEngine::new(
            "t",
            &cfg(vec![p], &[("lag_target", 1000.0)]),
            ScalingTransport::Kafka,
            ScalingTransport::Kafka,
        );
        assert!(errs.is_empty(), "errors: {errs:?}");
        let s = TransportSignals {
            kafka_assigned_lag: Some(500.0),
            ..Default::default()
        };
        assert!((eng.evaluate(&s, 0.0, 0.0)[0].1 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn custom_domain_signal_flows_end_to_end() {
        // An app pushes a DOMAIN signal at runtime; a pressure expression scales
        // on it under the dedicated `custom` map. The custom name is NOT known at
        // config-load, so it must NOT be a hard load error (warn-and-keep), and
        // it must evaluate to the expected value once the signal is present.
        let p = PressureExpr {
            name: "ch".into(),
            expression: "custom.clickhouse_backlog / params.ch_target".into(),
            enabled: true,
        };
        let (eng, errs) = ScalingEngine::new(
            "t",
            &cfg(vec![p], &[("ch_target", 1000.0)]),
            ScalingTransport::File, // CPU-only smart default for the fallback
            ScalingTransport::Kafka,
        );
        // custom.* is runtime-validated -> no hard load error.
        assert!(errs.is_empty(), "custom.* must not hard-reject: {errs:?}");

        // App pushes the signal: backlog 2500 / target 1000 = 2.5.
        let mut signals = TransportSignals::default();
        signals.custom.insert("clickhouse_backlog".into(), 2500.0);
        let v = eng.evaluate(&signals, 0.0, 0.0);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "ch");
        assert!(
            (v[0].1 - 2.5).abs() < 1e-9,
            "custom signal should flow end-to-end, got {}",
            v[0].1
        );
    }

    #[test]
    fn custom_signal_absent_at_runtime_falls_back() {
        // Same expression, but the app never pushes the signal -> the runtime
        // eval errors (NoSuchKey) -> fall back to the smart default (no last-good
        // yet). With File inbound + CPU 0.70/0.70 the smart default is 100.
        let p = PressureExpr {
            name: "ch".into(),
            expression: "custom.never_pushed / params.ch_target".into(),
            enabled: true,
        };
        let (eng, errs) = ScalingEngine::new(
            "t",
            &cfg(vec![p], &[("ch_target", 1000.0), ("cpu_target", 0.70)]),
            ScalingTransport::File,
            ScalingTransport::Kafka,
        );
        assert!(errs.is_empty(), "errors: {errs:?}");
        let v = eng.evaluate(&TransportSignals::default(), 0.70, 0.0);
        assert!(
            (v[0].1 - 100.0).abs() < 1e-6,
            "absent custom signal must fall back to smart default, got {}",
            v[0].1
        );
    }

    #[test]
    fn syntax_error_falls_back_with_friendly_message() {
        let p = PressureExpr {
            name: "broken".into(),
            expression: "cpu_utilisation_ratio +".into(), // syntax error
            enabled: true,
        };
        let (eng, errs) = ScalingEngine::new(
            "t",
            &cfg(vec![p], &[("cpu_target", 0.70)]),
            ScalingTransport::Kafka,
            ScalingTransport::Kafka,
        );
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("broken"), "msg: {}", errs[0]);
        // Falls back to smart default (still produces a value, no panic).
        let v = eng.evaluate(&TransportSignals::default(), 0.70, 0.0);
        assert!((v[0].1 - 100.0).abs() < 1e-6);
    }

    #[test]
    fn unknown_identifier_caught_at_load() {
        let p = PressureExpr {
            name: "typo".into(),
            expression: "cpu_utilisation_ratoi * 100".into(), // typo'd ident
            enabled: true,
        };
        let (_eng, errs) = ScalingEngine::new(
            "t",
            &cfg(vec![p], &[]),
            ScalingTransport::Kafka,
            ScalingTransport::Kafka,
        );
        assert_eq!(errs.len(), 1, "should catch the unknown identifier at load");
        assert!(errs[0].contains("typo"));
    }

    #[test]
    fn multi_output_independent_gauges() {
        let ps = vec![
            PressureExpr {
                name: "a".into(),
                expression: "cpu_utilisation_ratio * 100.0".into(),
                enabled: true,
            },
            PressureExpr {
                name: "b".into(),
                expression: "transport_inbound_pressure_ratio * 100.0".into(),
                enabled: true,
            },
        ];
        let (eng, errs) = ScalingEngine::new(
            "t",
            &cfg(ps, &[("lag_target", 100.0)]),
            ScalingTransport::Kafka,
            ScalingTransport::Kafka,
        );
        assert!(errs.is_empty(), "errors: {errs:?}");
        let s = TransportSignals {
            kafka_assigned_lag: Some(50.0),
            ..Default::default()
        };
        let v = eng.evaluate(&s, 0.30, 0.0);
        assert_eq!(v.len(), 2);
        assert!((v[0].1 - 30.0).abs() < 1e-6); // a: cpu
        assert!((v[1].1 - 50.0).abs() < 1e-6); // b: inbound 50/100=0.5 *100
    }
}
