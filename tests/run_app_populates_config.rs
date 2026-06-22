// Project:   hyperi-rustlib
// File:      tests/run_app_populates_config.rs
// Purpose:   Prove run_app's Run path populates the global config cascade
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Wiring test for the 2.8.11 cascade-applies fix.
//!
//! Drives a minimal [`DfeApp`] through [`run_app`] with `--config <file>` and
//! asserts that, by the time `run_service` is called, the global config
//! singleton is populated AND carries the value from the explicit file. Before
//! the fix, `run_app` never called `config::setup()`, so `try_get()` returned
//! `None` inside `run_service`.
//!
//! Own integration binary: the metrics recorder + config singleton + logger are
//! process-global, so this file runs in its own process with one setup.
#![cfg(feature = "cli-service")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use hyperi_rustlib::cli::{CliError, CommonArgs, DfeApp, ServiceRuntime, VersionInfo, run_app};

/// Minimal app whose `run_service` records what it observed about the cascade,
/// then returns immediately (no real service loop).
struct ProbeApp {
    common: CommonArgs,
    cascade_was_populated: Arc<AtomicBool>,
    log_level_seen: Arc<std::sync::Mutex<Option<String>>>,
}

impl DfeApp for ProbeApp {
    type Config = ();

    // The trait fixes the `-> &str` return; for a literal that trips
    // `clippy::unnecessary_literal_bound`. The signature is not ours to change.
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "cascade-probe"
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn env_prefix(&self) -> &str {
        "CASCADE_PROBE"
    }

    fn version_info(&self) -> VersionInfo {
        VersionInfo::new("cascade-probe", "0.0.0-test")
    }

    fn common_args(&self) -> &CommonArgs {
        &self.common
    }

    fn load_config(&self, _path: Option<&str>) -> Result<Self::Config, CliError> {
        Ok(())
    }

    async fn run_service(
        &self,
        _config: Self::Config,
        runtime: ServiceRuntime,
    ) -> Result<(), CliError> {
        // The whole point: by the time we're here, run_app must have populated
        // the global cascade from our --config file.
        let cfg = hyperi_rustlib::config::try_get();
        self.cascade_was_populated
            .store(cfg.is_some(), Ordering::SeqCst);
        if let Some(cfg) = cfg {
            *self.log_level_seen.lock().unwrap() = cfg.get_string("log_level");
        }
        // Cancel immediately so we don't spin a real loop.
        runtime.shutdown.cancel();
        Ok(())
    }
}

#[tokio::test]
async fn run_app_populates_cascade_from_config_file() {
    // A config file whose log_level differs from the hard-coded default (info).
    let dir = std::env::temp_dir().join(format!("rustlib-runapp-cfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("config.yaml");
    std::fs::write(&file, "log_level: warn\n").expect("write config");

    let cascade_was_populated = Arc::new(AtomicBool::new(false));
    let log_level_seen = Arc::new(std::sync::Mutex::new(None));

    let app = ProbeApp {
        common: CommonArgs {
            config: Some(file.to_string_lossy().into_owned()),
            log_level: "info".to_string(),
            log_format: "json".to_string(),
            // Ephemeral port so the metrics server never collides with anything.
            metrics_addr: "127.0.0.1:0".to_string(),
            verbose: false,
            quiet: true,
        },
        cascade_was_populated: Arc::clone(&cascade_was_populated),
        log_level_seen: Arc::clone(&log_level_seen),
    };

    run_app(app).await.expect("run_app should succeed");

    assert!(
        cascade_was_populated.load(Ordering::SeqCst),
        "run_app must populate the global config cascade before run_service"
    );
    assert_eq!(
        *log_level_seen.lock().unwrap(),
        Some("warn".to_string()),
        "the cascade run_app built must carry the --config file's log_level (warn), \
         proving the explicit file was actually ingested"
    );

    std::fs::remove_dir_all(&dir).ok();
}
