// Project:   hs-rustlib
// File:      src/clickhouse_arrow/error.rs
// Purpose:   ClickHouse-specific error types
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! ClickHouse error types.

use std::fmt;

/// Errors that can occur during ClickHouse operations.
#[derive(Debug)]
pub enum ClickHouseError {
    /// Connection or configuration error.
    Connection(String),
    /// Query execution error.
    Query(String),
    /// Insert operation error.
    Insert(String),
    /// Schema-related error.
    Schema(String),
    /// Arrow format/conversion error.
    Arrow(String),
}

impl fmt::Display for ClickHouseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(msg) => write!(f, "ClickHouse connection error: {msg}"),
            Self::Query(msg) => write!(f, "ClickHouse query error: {msg}"),
            Self::Insert(msg) => write!(f, "ClickHouse insert error: {msg}"),
            Self::Schema(msg) => write!(f, "ClickHouse schema error: {msg}"),
            Self::Arrow(msg) => write!(f, "Arrow format error: {msg}"),
        }
    }
}

impl std::error::Error for ClickHouseError {}

impl From<clickhouse_arrow::Error> for ClickHouseError {
    fn from(err: clickhouse_arrow::Error) -> Self {
        // Convert clickhouse-arrow errors to appropriate variants
        let msg = err.to_string();
        if msg.contains("connect") || msg.contains("connection") {
            Self::Connection(msg)
        } else if msg.contains("schema") || msg.contains("column") {
            Self::Schema(msg)
        } else {
            Self::Query(msg)
        }
    }
}

impl From<arrow::error::ArrowError> for ClickHouseError {
    fn from(err: arrow::error::ArrowError) -> Self {
        Self::Arrow(err.to_string())
    }
}
