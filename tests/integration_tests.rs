// Project:   hyperi-rustlib
// File:      tests/integration_tests.rs
// Purpose:   Single-binary integration test entry point
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Integration tests — consolidated into a single binary for compile-time efficiency.
//!
//! Each submodule is feature-gated to match its library dependency.
//! Run with: `cargo nextest run --test integration`

mod common;

mod integration;
