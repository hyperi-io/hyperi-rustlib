// Project:   hs-rustlib
// File:      tests/common/mod.rs
// Purpose:   Shared test fixtures and utilities
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Shared test fixtures and utilities.

use std::path::PathBuf;
use tempfile::TempDir;

/// Create a temporary directory with config files for testing.
pub fn create_test_config_dir() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path().to_path_buf();

    // Create defaults.yaml
    std::fs::write(
        path.join("defaults.yaml"),
        r#"
log_level: debug
database:
  host: localhost
  port: 5432
"#,
    )
    .expect("failed to write defaults.yaml");

    // Create settings.yaml
    std::fs::write(
        path.join("settings.yaml"),
        r#"
app_name: test_app
database:
  username: testuser
"#,
    )
    .expect("failed to write settings.yaml");

    // Create settings.development.yaml
    std::fs::write(
        path.join("settings.development.yaml"),
        r#"
debug: true
database:
  password: devpassword
"#,
    )
    .expect("failed to write settings.development.yaml");

    (dir, path)
}

/// Set environment variables for testing, returning a guard that clears them on drop.
pub struct EnvGuard {
    vars: Vec<String>,
}

impl EnvGuard {
    /// Create a new environment guard with the given variables.
    pub fn new(vars: &[(&str, &str)]) -> Self {
        let var_names: Vec<String> = vars.iter().map(|(k, v)| {
            std::env::set_var(k, v);
            k.to_string()
        }).collect();

        Self { vars: var_names }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for var in &self.vars {
            std::env::remove_var(var);
        }
    }
}
