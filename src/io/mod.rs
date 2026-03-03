// Project:   hyperi-rustlib
// File:      src/io/mod.rs
// Purpose:   Shared NDJSON file I/O module
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Shared NDJSON file I/O primitives.
//!
//! Provides [`NdjsonWriter`] — a rotating file writer for newline-delimited JSON.
//! Used by both the DLQ file backend and the file output sink.
//!
//! ## Design
//!
//! `NdjsonWriter` is a thin wrapper around `file-rotate` that handles:
//! - Rotating NDJSON files by time (hourly/daily)
//! - Optional gzip compression of rotated files
//! - Age-based cleanup of old files
//! - Atomic write counters for metrics
//!
//! It knows nothing about DLQ or output semantics — callers serialise their
//! own types and hand raw `&[u8]` lines to the writer.

mod config;
mod ndjson_writer;

pub use config::{FileWriterConfig, RotationPeriod};
pub use ndjson_writer::NdjsonWriter;
