// Project:   hyperi-rustlib
// File:      src/worker/engine/mod.rs
// Purpose:   SIMD-optimised batch processing engine for DFE pipelines
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

pub mod config;
pub mod types;

pub use config::{BatchProcessingConfig, ParseErrorAction, PreRouteFilterConfig};
pub use types::{MessageMetadata, ParsedMessage, PreRouteResult, RawMessage};
