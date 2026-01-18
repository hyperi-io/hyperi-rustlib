// Project:   hs-rustlib
// File:      src/clickhouse_arrow/client.rs
// Purpose:   ClickHouse Arrow client wrapper
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! ClickHouse Arrow client for native protocol operations.
//!
//! Wraps `clickhouse-arrow` with a simplified API for common use cases.

use std::sync::Arc;

use arrow::array::RecordBatch;
use arrow::datatypes::{DataType, SchemaRef};
use clickhouse_arrow::{ArrowFormat, Client, ClientBuilder};
use futures_util::StreamExt;
use futures_util::TryStreamExt;

use super::config::ClickHouseConfig;
use super::error::ClickHouseError;
use super::types::{ColumnInfo, ParsedType, TableSchema};
use super::Result;

/// Type alias for the underlying Arrow client.
pub type ArrowClient = Client<ArrowFormat>;

/// ClickHouse client using native Arrow protocol.
///
/// Provides efficient columnar data transfer for inserts and queries.
/// Uses the native ClickHouse protocol (port 9000) with Arrow format.
///
/// ## Example
///
/// ```rust,no_run
/// use hs_rustlib::clickhouse::{ArrowClickHouseClient, ClickHouseConfig};
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let config = ClickHouseConfig::new("localhost:9000", "default");
/// let client = ArrowClickHouseClient::new(&config).await?;
///
/// // Run a query
/// let batches = client.select("SELECT 1 as x").await?;
/// # Ok(())
/// # }
/// ```
pub struct ArrowClickHouseClient {
    client: ArrowClient,
    database: String,
}

impl ArrowClickHouseClient {
    /// Create a new Arrow client from config.
    ///
    /// # Errors
    ///
    /// Returns an error if connection fails.
    pub async fn new(config: &ClickHouseConfig) -> Result<Self> {
        let host = config.primary_host().ok_or_else(|| {
            ClickHouseError::Connection("No ClickHouse hosts configured".into())
        })?;

        // Parse host:port
        let addr = if host.contains(':') {
            host.to_string()
        } else {
            format!("{host}:9000")
        };

        let client = ClientBuilder::new()
            .with_endpoint(&addr)
            .with_username(&config.username)
            .with_password(&config.password)
            .with_database(&config.database)
            .build_arrow()
            .await
            .map_err(|e| ClickHouseError::Connection(format!("Arrow client connect failed: {e}")))?;

        Ok(Self {
            client,
            database: config.database.clone(),
        })
    }

    /// Get the database name.
    #[must_use]
    pub fn database(&self) -> &str {
        &self.database
    }

    /// Insert an Arrow `RecordBatch` into a table.
    ///
    /// The table can be specified as "db.table" or just "table" (uses default database).
    ///
    /// # Errors
    ///
    /// Returns an error if the insert fails.
    pub async fn insert(&self, table: &str, batch: RecordBatch) -> Result<usize> {
        if batch.num_rows() == 0 {
            return Ok(0);
        }

        let row_count = batch.num_rows();
        let (db, tbl) = parse_db_table(table, &self.database);
        let insert_query = format!("INSERT INTO {db}.{tbl} VALUES");

        let mut stream = self.client
            .insert(&insert_query, batch, None)
            .await
            .map_err(|e| ClickHouseError::Insert(format!("Arrow insert failed: {e}")))?;

        // Consume the stream to complete the insert
        while let Some(result) = stream.next().await {
            result.map_err(|e| ClickHouseError::Insert(format!("Arrow insert stream error: {e}")))?;
        }

        Ok(row_count)
    }

    /// Insert multiple Arrow `RecordBatch`es into a table.
    ///
    /// # Errors
    ///
    /// Returns an error if any insert fails.
    pub async fn insert_many(&self, table: &str, batches: Vec<RecordBatch>) -> Result<usize> {
        if batches.is_empty() {
            return Ok(0);
        }

        let total_rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
        let (db, tbl) = parse_db_table(table, &self.database);
        let insert_query = format!("INSERT INTO {db}.{tbl} VALUES");

        let mut stream = self.client
            .insert_many(&insert_query, batches, None)
            .await
            .map_err(|e| ClickHouseError::Insert(format!("Arrow insert_many failed: {e}")))?;

        while let Some(result) = stream.next().await {
            result.map_err(|e| ClickHouseError::Insert(format!("Arrow insert_many stream error: {e}")))?;
        }

        Ok(total_rows)
    }

    /// Fetch table schema as Arrow schema.
    ///
    /// # Errors
    ///
    /// Returns an error if the table doesn't exist or schema fetch fails.
    pub async fn fetch_schema(&self, table: &str) -> Result<SchemaRef> {
        let (db, tbl) = parse_db_table(table, &self.database);

        let schemas = self.client
            .fetch_schema(Some(&db), &[tbl.as_str()], None)
            .await
            .map_err(|e| ClickHouseError::Schema(format!("Failed to fetch schema: {e}")))?;

        schemas.get(&tbl)
            .cloned()
            .ok_or_else(|| ClickHouseError::Schema(format!("Table '{table}' not found")))
    }

    /// Execute a query (for DDL, schema queries, etc.).
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub async fn query(&self, sql: &str) -> Result<()> {
        self.client
            .execute(sql, None)
            .await
            .map_err(|e| ClickHouseError::Query(format!("Query failed: {e}")))
    }

    /// Execute a SELECT query and return Arrow `RecordBatch`es.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub async fn select(&self, sql: &str) -> Result<Vec<RecordBatch>> {
        let response = self.client
            .query(sql, None)
            .await
            .map_err(|e| ClickHouseError::Query(format!("SELECT query failed: {e}")))?;

        let batches: Vec<RecordBatch> = response
            .try_collect()
            .await
            .map_err(|e| ClickHouseError::Query(format!("Failed to collect query results: {e}")))?;

        Ok(batches)
    }

    /// Check connection health.
    ///
    /// # Errors
    ///
    /// Returns an error if the health check fails.
    pub async fn health_check(&self) -> Result<()> {
        self.client
            .health_check(true)
            .await
            .map_err(|e| ClickHouseError::Connection(format!("Health check failed: {e}")))
    }

    /// Check if a table exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the check fails (other than table not found).
    pub async fn table_exists(&self, table: &str) -> Result<bool> {
        match self.fetch_schema(table).await {
            Ok(_) => Ok(true),
            Err(ClickHouseError::Schema(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Get list of all table names in the database.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub async fn list_tables(&self) -> Result<Vec<String>> {
        let schemas = self.client
            .fetch_schema(Some(&self.database), &[], None)
            .await
            .map_err(|e| ClickHouseError::Schema(format!("Failed to list tables: {e}")))?;

        Ok(schemas.keys().cloned().collect())
    }

    /// Fetch table schema as `TableSchema` (includes parsed types).
    ///
    /// # Errors
    ///
    /// Returns an error if the schema fetch fails.
    pub async fn fetch_table_schema(&self, table: &str) -> Result<TableSchema> {
        let (db, tbl) = parse_db_table(table, &self.database);
        let arrow_schema = self.fetch_schema(table).await?;

        let columns: Vec<ColumnInfo> = arrow_schema
            .fields()
            .iter()
            .enumerate()
            .map(|(i, field)| {
                let type_name = arrow_type_to_ch_name(field.data_type());
                ColumnInfo {
                    name: field.name().clone(),
                    type_name: type_name.clone(),
                    parsed_type: ParsedType::parse(&type_name),
                    position: (i as u64) + 1,
                    default_kind: String::new(),
                    default_expression: String::new(),
                    comment: String::new(),
                    is_in_primary_key: false,
                    is_in_sorting_key: false,
                }
            })
            .collect();

        Ok(TableSchema {
            database: db,
            table: tbl,
            columns,
            comment: String::new(),
        })
    }

    /// Get the underlying Arrow client for advanced operations.
    #[must_use]
    pub fn inner(&self) -> &ArrowClient {
        &self.client
    }
}

/// Convert Arrow `DataType` to ClickHouse type name (best effort).
fn arrow_type_to_ch_name(dt: &DataType) -> String {
    match dt {
        DataType::Int8 => "Int8".to_string(),
        DataType::Int16 => "Int16".to_string(),
        DataType::Int32 => "Int32".to_string(),
        DataType::Int64 => "Int64".to_string(),
        DataType::UInt8 => "UInt8".to_string(),
        DataType::UInt16 => "UInt16".to_string(),
        DataType::UInt32 => "UInt32".to_string(),
        DataType::UInt64 => "UInt64".to_string(),
        DataType::Float32 => "Float32".to_string(),
        DataType::Float64 => "Float64".to_string(),
        DataType::Boolean => "Bool".to_string(),
        DataType::Utf8 | DataType::LargeUtf8 => "String".to_string(),
        DataType::Binary | DataType::LargeBinary => "String".to_string(),
        DataType::Date32 | DataType::Date64 => "Date".to_string(),
        DataType::FixedSizeBinary(16) => "UUID".to_string(),
        DataType::FixedSizeBinary(4) => "IPv4".to_string(),
        DataType::FixedSizeBinary(n) => format!("FixedString({n})"),
        DataType::List(inner) => format!("Array({})", arrow_type_to_ch_name(inner.data_type())),
        DataType::Timestamp(_, _) => "DateTime64(3)".to_string(),
        DataType::Time32(_) => "DateTime".to_string(),
        DataType::Time64(_) => "DateTime64(6)".to_string(),
        _ => "String".to_string(), // Default fallback
    }
}

/// Parse "db.table" format, falling back to default database.
fn parse_db_table(table: &str, default_db: &str) -> (String, String) {
    if let Some((db, tbl)) = table.split_once('.') {
        (db.to_string(), tbl.to_string())
    } else {
        (default_db.to_string(), table.to_string())
    }
}

/// Thread-safe reference to Arrow ClickHouse client.
pub type SharedArrowClient = Arc<ArrowClickHouseClient>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_db_table() {
        let (db, tbl) = parse_db_table("mydb.events", "default");
        assert_eq!(db, "mydb");
        assert_eq!(tbl, "events");

        let (db, tbl) = parse_db_table("events", "default");
        assert_eq!(db, "default");
        assert_eq!(tbl, "events");
    }

    #[test]
    fn test_arrow_type_to_ch_name() {
        assert_eq!(arrow_type_to_ch_name(&DataType::Int64), "Int64");
        assert_eq!(arrow_type_to_ch_name(&DataType::Utf8), "String");
        assert_eq!(arrow_type_to_ch_name(&DataType::Boolean), "Bool");
        assert_eq!(arrow_type_to_ch_name(&DataType::FixedSizeBinary(16)), "UUID");
        assert_eq!(arrow_type_to_ch_name(&DataType::FixedSizeBinary(4)), "IPv4");
    }
}
