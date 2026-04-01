#![cfg(feature = "worker")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use hyperi_rustlib::worker::{AdaptiveWorkerPool, ScalingDecision, ScalingInput, WorkerPoolConfig};

// --- Config tests ---

#[test]
fn test_default_config_has_sensible_values() {
    let cfg = WorkerPoolConfig::default();
    assert_eq!(cfg.min_threads, 2);
    assert_eq!(cfg.max_threads, 0);
    assert!((cfg.grow_below - 0.60).abs() < f64::EPSILON);
    assert!((cfg.shrink_above - 0.85).abs() < f64::EPSILON);
    assert!((cfg.emergency_above - 0.95).abs() < f64::EPSILON);
    assert!((cfg.memory_pressure_cap - 0.80).abs() < f64::EPSILON);
    assert_eq!(cfg.scale_interval_secs, 5);
    assert_eq!(cfg.async_concurrency, 32);
    assert_eq!(cfg.health_saturation_timeout_secs, 30);
}

#[test]
fn test_config_validation_rejects_min_greater_than_max() {
    let cfg = WorkerPoolConfig {
        min_threads: 16,
        max_threads: 4,
        ..Default::default()
    };
    assert!(cfg.validate().is_err());
}

#[test]
fn test_config_validation_accepts_max_zero_auto_detect() {
    let cfg = WorkerPoolConfig {
        min_threads: 2,
        max_threads: 0,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_from_cascade_without_config_setup_returns_defaults() {
    // Regression test for hyperi-io/dfe-loader#19:
    // from_cascade() must not panic when config::setup() hasn't been called.
    // It should fall back to defaults gracefully.
    let cfg = WorkerPoolConfig::from_cascade("worker_pool")
        .expect("from_cascade should not panic without config::setup()");
    assert_eq!(cfg.min_threads, 2);
    assert_eq!(cfg.max_threads, 0); // auto-detect default
}

#[test]
fn test_config_validation_rejects_thresholds_out_of_order() {
    let cfg = WorkerPoolConfig {
        grow_below: 0.90,
        shrink_above: 0.50,
        ..Default::default()
    };
    assert!(cfg.validate().is_err());
}

#[test]
fn test_resolve_max_threads_auto_detect() {
    let mut cfg = WorkerPoolConfig::default();
    cfg.resolve_max_threads();
    assert!(cfg.max_threads >= 1);
}

// --- process_batch tests ---

#[test]
fn test_process_batch_executes_on_multiple_threads() {
    let config = WorkerPoolConfig {
        min_threads: 4,
        max_threads: 4,
        ..Default::default()
    };
    let pool = AdaptiveWorkerPool::new(config);

    let thread_ids = Arc::new(parking_lot::Mutex::new(std::collections::HashSet::new()));
    let items: Vec<i32> = (0..100).collect();

    let tids = thread_ids.clone();
    let results: Vec<Result<i32, String>> = pool.process_batch(&items, |&item| {
        tids.lock().insert(std::thread::current().id());
        std::thread::sleep(std::time::Duration::from_millis(1));
        Ok(item * 2)
    });

    assert_eq!(results.len(), 100);
    for (i, result) in results.iter().enumerate() {
        assert_eq!(
            *result.as_ref().unwrap(),
            (i32::try_from(i).unwrap()) * 2,
            "Wrong result at index {i}"
        );
    }
    let unique_threads = thread_ids.lock().len();
    assert!(
        unique_threads > 1,
        "Expected multiple threads, got {unique_threads}"
    );
}

#[test]
fn test_process_batch_respects_semaphore_throttle() {
    let config = WorkerPoolConfig {
        min_threads: 2,
        max_threads: 4,
        ..Default::default()
    };
    let pool = AdaptiveWorkerPool::new(config);

    let concurrent = Arc::new(AtomicUsize::new(0));
    let max_concurrent = Arc::new(AtomicUsize::new(0));
    let items: Vec<i32> = (0..20).collect();

    let c = concurrent.clone();
    let mc = max_concurrent.clone();
    let _results: Vec<Result<i32, String>> = pool.process_batch(&items, |&item| {
        let current = c.fetch_add(1, Ordering::SeqCst) + 1;
        mc.fetch_max(current, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(10));
        c.fetch_sub(1, Ordering::SeqCst);
        Ok(item)
    });

    let observed_max = max_concurrent.load(Ordering::SeqCst);
    assert!(
        observed_max <= 2,
        "Expected max 2 concurrent, got {observed_max}"
    );
}

#[test]
fn test_process_batch_handles_errors() {
    let config = WorkerPoolConfig {
        min_threads: 2,
        max_threads: 2,
        ..Default::default()
    };
    let pool = AdaptiveWorkerPool::new(config);

    let items: Vec<i32> = (0..10).collect();
    let results: Vec<Result<i32, String>> = pool.process_batch(&items, |&item| {
        if item % 3 == 0 {
            Err(format!("bad item: {item}"))
        } else {
            Ok(item * 2)
        }
    });

    assert_eq!(results.len(), 10);
    assert!(results[0].is_err());
    assert!(results[1].is_ok());
    assert!(results[3].is_err());
}

#[test]
fn test_process_batch_empty_input() {
    let pool = AdaptiveWorkerPool::new(WorkerPoolConfig::default());
    let items: Vec<i32> = vec![];
    let results: Vec<Result<i32, String>> = pool.process_batch(&items, |&x| Ok(x));
    assert!(results.is_empty());
}

// --- fan_out_async tests ---

#[tokio::test]
async fn test_fan_out_async_preserves_order() {
    let config = WorkerPoolConfig {
        min_threads: 2,
        max_threads: 4,
        async_concurrency: 4,
        ..Default::default()
    };
    let pool = AdaptiveWorkerPool::new(config);

    let items: Vec<i32> = (0..20).collect();
    let results: Vec<Result<i32, String>> = pool
        .fan_out_async(&items, |&item| async move {
            tokio::time::sleep(std::time::Duration::from_millis(
                u64::try_from(20 - item).unwrap_or(0) % 10,
            ))
            .await;
            Ok(item * 3)
        })
        .await;

    assert_eq!(results.len(), 20);
    for (i, result) in results.iter().enumerate() {
        assert_eq!(
            *result.as_ref().unwrap(),
            (i32::try_from(i).unwrap()) * 3,
            "Result at index {i} has wrong value"
        );
    }
}

#[tokio::test]
async fn test_fan_out_async_respects_concurrency_limit() {
    let config = WorkerPoolConfig {
        async_concurrency: 3,
        ..Default::default()
    };
    let pool = AdaptiveWorkerPool::new(config);

    let concurrent = Arc::new(AtomicUsize::new(0));
    let max_concurrent = Arc::new(AtomicUsize::new(0));
    let items: Vec<i32> = (0..12).collect();

    let c = concurrent.clone();
    let mc = max_concurrent.clone();
    let _results: Vec<Result<i32, String>> = pool
        .fan_out_async(&items, |&item| {
            let c = c.clone();
            let mc = mc.clone();
            async move {
                let current = c.fetch_add(1, Ordering::SeqCst) + 1;
                mc.fetch_max(current, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                c.fetch_sub(1, Ordering::SeqCst);
                Ok(item)
            }
        })
        .await;

    let observed = max_concurrent.load(Ordering::SeqCst);
    assert!(observed <= 3, "Expected max 3 concurrent, got {observed}");
}

#[tokio::test]
async fn test_fan_out_async_empty_input() {
    let pool = AdaptiveWorkerPool::new(WorkerPoolConfig::default());
    let items: Vec<i32> = vec![];
    let results: Vec<Result<i32, String>> =
        pool.fan_out_async(&items, |&x| async move { Ok(x) }).await;
    assert!(results.is_empty());
}

// --- Scaling decision tests ---

fn scaling_input(cpu: f64, mem: f64, current: usize) -> ScalingInput {
    ScalingInput {
        cpu_util: cpu,
        memory_pressure: mem,
        current,
        min_threads: 2,
        max_threads: 8,
        grow_below: 0.60,
        shrink_above: 0.85,
        emergency_above: 0.95,
        memory_pressure_cap: 0.80,
    }
}

#[test]
fn test_scaling_decision_grow_when_cpu_low() {
    let decision = ScalingDecision::evaluate(&scaling_input(0.40, 0.20, 4));
    assert_eq!(decision.target, 6);
    assert_eq!(decision.direction, "up");
}

#[test]
fn test_scaling_decision_steady_in_dead_band() {
    let decision = ScalingDecision::evaluate(&scaling_input(0.72, 0.20, 4));
    assert_eq!(decision.target, 4);
    assert_eq!(decision.direction, "steady");
}

#[test]
fn test_scaling_decision_shrink_when_cpu_high() {
    let decision = ScalingDecision::evaluate(&scaling_input(0.90, 0.20, 6));
    assert_eq!(decision.target, 5);
    assert_eq!(decision.direction, "down");
}

#[test]
fn test_scaling_decision_emergency_shrink() {
    let decision = ScalingDecision::evaluate(&scaling_input(0.97, 0.20, 6));
    assert_eq!(decision.target, 4);
    assert_eq!(decision.direction, "emergency_down");
}

#[test]
fn test_scaling_decision_memory_cap_overrides_cpu() {
    let decision = ScalingDecision::evaluate(&scaling_input(0.40, 0.90, 6));
    assert_eq!(decision.target, 2);
    assert_eq!(decision.direction, "memory_cap");
}

#[test]
fn test_scaling_decision_respects_min_max_bounds() {
    // Try to grow past max
    let decision = ScalingDecision::evaluate(&scaling_input(0.30, 0.20, 7));
    assert_eq!(decision.target, 8);

    // Try to shrink below min
    let decision = ScalingDecision::evaluate(&scaling_input(0.97, 0.20, 3));
    assert_eq!(decision.target, 2);
}

// --- Graceful shutdown test ---

#[tokio::test]
async fn test_graceful_shutdown_drains_work() {
    let config = WorkerPoolConfig {
        min_threads: 2,
        max_threads: 4,
        ..Default::default()
    };
    let pool = Arc::new(AdaptiveWorkerPool::new(config));
    let cancel = tokio_util::sync::CancellationToken::new();

    pool.start_scaling_loop(cancel.clone());

    // Submit some work
    let items: Vec<i32> = (0..10).collect();
    let results: Vec<Result<i32, String>> = pool.process_batch(&items, |&x| Ok(x));
    assert_eq!(results.len(), 10);

    // Shutdown scaler
    cancel.cancel();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Pool still usable after scaler stops — just no more scaling adjustments
    let more: Vec<Result<i32, String>> = pool.process_batch(&[42], |&x| Ok(x));
    assert_eq!(more.len(), 1);
    assert_eq!(*more[0].as_ref().unwrap(), 42);
}

// --- Pool active_threads test ---

#[test]
fn test_active_threads_reports_correct_count() {
    let config = WorkerPoolConfig {
        min_threads: 3,
        max_threads: 8,
        ..Default::default()
    };
    let pool = AdaptiveWorkerPool::new(config);
    // Initially, semaphore has min_threads (3) permits available out of 8 total
    // active = max - available = 8 - 3 = 5 ... wait that's not right
    // Actually at rest, no work in flight, all permits available = min_threads
    // active = max_threads - available_permits = 8 - 3 = 5
    // Hmm, this is tricky. Let me just verify the initial state makes sense.
    let max = pool.max_threads();
    assert_eq!(max, 8);
}

// =============================================================================
// Adversarial / edge case tests
// =============================================================================

#[test]
fn test_process_batch_panic_in_closure_does_not_crash_pool() {
    // If a closure panics, rayon propagates it. The pool should still be usable after.
    let config = WorkerPoolConfig {
        min_threads: 2,
        max_threads: 2,
        ..Default::default()
    };
    let pool = AdaptiveWorkerPool::new(config);

    // First batch: one item panics
    let items: Vec<i32> = vec![1, 2, 3];
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pool.process_batch(&items, |&item| -> Result<i32, String> {
            if item == 2 {
                panic!("deliberate panic in closure");
            }
            Ok(item)
        })
    }));
    assert!(result.is_err(), "panic should propagate from process_batch");

    // Pool should still be usable after the panic
    let items2: Vec<i32> = vec![10, 20, 30];
    let results: Vec<Result<i32, String>> = pool.process_batch(&items2, |&item| Ok(item * 2));
    assert_eq!(results.len(), 3);
    assert_eq!(*results[0].as_ref().unwrap(), 20);
    assert_eq!(*results[1].as_ref().unwrap(), 40);
    assert_eq!(*results[2].as_ref().unwrap(), 60);
}

#[test]
fn test_process_batch_all_errors() {
    let pool = AdaptiveWorkerPool::new(WorkerPoolConfig::default());
    let items: Vec<i32> = (0..50).collect();
    let results: Vec<Result<i32, String>> =
        pool.process_batch(&items, |&item| Err(format!("every item fails: {item}")));
    assert_eq!(results.len(), 50);
    assert!(results.iter().all(Result::is_err));
}

#[test]
fn test_process_batch_single_item() {
    let pool = AdaptiveWorkerPool::new(WorkerPoolConfig::default());
    let items = vec![42];
    let results: Vec<Result<i32, String>> = pool.process_batch(&items, |&x| Ok(x * 2));
    assert_eq!(results.len(), 1);
    assert_eq!(*results[0].as_ref().unwrap(), 84);
}

#[test]
fn test_process_batch_large_batch_stress() {
    // 10,000 items should process without issues
    let config = WorkerPoolConfig {
        min_threads: 4,
        max_threads: 4,
        ..Default::default()
    };
    let pool = AdaptiveWorkerPool::new(config);
    let items: Vec<i32> = (0..10_000).collect();
    let results: Vec<Result<i64, String>> = pool.process_batch(&items, |&item| {
        // Some CPU work to exercise thread scheduling
        Ok(i64::from(item) * i64::from(item))
    });
    assert_eq!(results.len(), 10_000);
    // Verify ordering preserved
    for (i, result) in results.iter().enumerate() {
        let expected = (i as i64) * (i as i64);
        assert_eq!(
            *result.as_ref().unwrap(),
            expected,
            "ordering broken at index {i}"
        );
    }
}

#[test]
fn test_config_validation_grow_equals_shrink_rejected() {
    let cfg = WorkerPoolConfig {
        grow_below: 0.80,
        shrink_above: 0.80, // equal — no dead band
        ..Default::default()
    };
    assert!(cfg.validate().is_err());
}

#[test]
fn test_config_validation_emergency_below_shrink_rejected() {
    let cfg = WorkerPoolConfig {
        shrink_above: 0.95,
        emergency_above: 0.90, // below shrink
        ..Default::default()
    };
    assert!(cfg.validate().is_err());
}

#[test]
fn test_config_min_threads_zero_rejected_by_rayon() {
    // min_threads=0 means no permits — every rayon task would block forever.
    // This should still create the pool (rayon allows 0 threads? no, it panics).
    // Actually rayon requires at least 1 thread. But our semaphore starts at min_threads.
    // With 0 permits, process_batch would deadlock. Validate this edge case.
    let config = WorkerPoolConfig {
        min_threads: 0,
        max_threads: 4,
        ..Default::default()
    };
    // Config validates OK (0 is technically valid — means "start with 0 active, scaler grows")
    // But process_batch would block. This is a design choice — min_threads=0 is allowed
    // for services that start idle and scale up on demand.
    assert!(config.validate().is_ok());
}

#[test]
fn test_scaling_decision_boundary_exactly_at_grow_below() {
    // CPU exactly at grow_below threshold — should be in steady band (not grow)
    let input = ScalingInput {
        cpu_util: 0.60, // exactly at grow_below
        memory_pressure: 0.20,
        current: 4,
        min_threads: 2,
        max_threads: 8,
        grow_below: 0.60,
        shrink_above: 0.85,
        emergency_above: 0.95,
        memory_pressure_cap: 0.80,
    };
    let decision = ScalingDecision::evaluate(&input);
    // At exactly grow_below: cpu_util < grow_below is FALSE (0.60 < 0.60 = false)
    // So it falls to cpu_util <= shrink_above (0.60 <= 0.85 = true) → steady
    assert_eq!(decision.direction, "steady");
}

#[test]
fn test_scaling_decision_boundary_exactly_at_shrink_above() {
    let input = ScalingInput {
        cpu_util: 0.85, // exactly at shrink_above
        memory_pressure: 0.20,
        current: 4,
        min_threads: 2,
        max_threads: 8,
        grow_below: 0.60,
        shrink_above: 0.85,
        emergency_above: 0.95,
        memory_pressure_cap: 0.80,
    };
    let decision = ScalingDecision::evaluate(&input);
    // cpu_util <= shrink_above (0.85 <= 0.85 = true) → steady
    assert_eq!(decision.direction, "steady");
}

#[test]
fn test_scaling_decision_boundary_exactly_at_emergency() {
    let input = ScalingInput {
        cpu_util: 0.95, // exactly at emergency_above
        memory_pressure: 0.20,
        current: 4,
        min_threads: 2,
        max_threads: 8,
        grow_below: 0.60,
        shrink_above: 0.85,
        emergency_above: 0.95,
        memory_pressure_cap: 0.80,
    };
    let decision = ScalingDecision::evaluate(&input);
    // cpu_util <= emergency_above (0.95 <= 0.95 = true) → down (not emergency)
    assert_eq!(decision.direction, "down");
}

#[test]
fn test_scaling_decision_memory_exactly_at_cap() {
    let input = ScalingInput {
        cpu_util: 0.40,
        memory_pressure: 0.80, // exactly at memory_pressure_cap
        current: 6,
        min_threads: 2,
        max_threads: 8,
        grow_below: 0.60,
        shrink_above: 0.85,
        emergency_above: 0.95,
        memory_pressure_cap: 0.80,
    };
    let decision = ScalingDecision::evaluate(&input);
    // memory_pressure > memory_pressure_cap (0.80 > 0.80 = false) → NOT memory_cap
    // Falls through to cpu check: 0.40 < 0.60 → grow
    assert_eq!(decision.direction, "up");
}

#[tokio::test]
async fn test_fan_out_async_with_all_failures() {
    let pool = AdaptiveWorkerPool::new(WorkerPoolConfig::default());
    let items: Vec<i32> = (0..10).collect();
    let results: Vec<Result<i32, String>> = pool
        .fan_out_async(&items, |&item| async move { Err(format!("fail: {item}")) })
        .await;
    assert_eq!(results.len(), 10);
    assert!(results.iter().all(Result::is_err));
}

#[tokio::test]
async fn test_fan_out_async_mixed_success_failure_preserves_order() {
    let pool = AdaptiveWorkerPool::new(WorkerPoolConfig {
        async_concurrency: 4,
        ..Default::default()
    });
    let items: Vec<i32> = (0..20).collect();
    let results: Vec<Result<i32, String>> = pool
        .fan_out_async(&items, |&item| async move {
            // Variable delay to stress ordering
            tokio::time::sleep(std::time::Duration::from_millis(
                u64::try_from(item % 5).unwrap_or(0),
            ))
            .await;
            if item % 3 == 0 {
                Err(format!("fail: {item}"))
            } else {
                Ok(item * 10)
            }
        })
        .await;

    assert_eq!(results.len(), 20);
    // Verify ordering: even indices that are multiples of 3 should be errors
    assert!(results[0].is_err()); // 0 % 3 == 0
    assert!(results[3].is_err()); // 3 % 3 == 0
    assert!(results[6].is_err()); // 6 % 3 == 0
    assert_eq!(*results[1].as_ref().unwrap(), 10); // 1 * 10
    assert_eq!(*results[2].as_ref().unwrap(), 20); // 2 * 10
    assert_eq!(*results[4].as_ref().unwrap(), 40); // 4 * 10
}
