// Project:   hyperi-rustlib
// File:      tests/e2e/mod.rs
// Purpose:   E2E test module declarations
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

#[cfg(feature = "transport-grpc")]
mod grpc_transport;

#[cfg(feature = "transport-kafka")]
mod kafka;

#[cfg(feature = "transport-grpc-vector-compat")]
mod vector_compat;

#[cfg(feature = "deployment-test-support")]
mod contract_artefacts;
