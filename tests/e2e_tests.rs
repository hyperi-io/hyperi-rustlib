// Project:   hyperi-rustlib
// File:      tests/e2e_tests.rs
// Purpose:   Single-binary e2e test entry point (external infrastructure required)
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! End-to-end tests requiring external infrastructure.
//!
//! These tests are `#[ignore]` by default and require running services.
//! Run with: `cargo nextest run --test e2e -- --ignored`

mod common;

mod e2e;
