// Project:   scalo
// File:      tests/integration/mod.rs
// Purpose:   Integration test module declarations
// Language:  Rust
//
// License:   Apache-2.0
// Copyright: (c) 2026 HYPERI PTY LIMITED

// Always-compiled integration tests
mod config_parity;
mod env;
mod env_parity;
mod logger_output;
mod metrics;

// Feature-gated integration tests
#[cfg(feature = "directory-config")]
mod directory_config;

#[cfg(feature = "expression")]
mod expression;
