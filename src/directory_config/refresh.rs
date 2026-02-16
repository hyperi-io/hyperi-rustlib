// Project:   hyperi-rustlib
// File:      src/directory_config/refresh.rs
// Purpose:   Background polling refresh for DirectoryConfigStore
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::{broadcast, oneshot};

use crate::directory_config::store::{load_all_tables, TableCache, TimestampCache};
use crate::directory_config::types::{ChangeEvent, ChangeOperation};

/// Background polling loop that detects file changes and refreshes the cache.
///
/// Runs until a shutdown signal is received. On each tick it reloads all YAML
/// files, compares timestamps, and broadcasts change events for any tables
/// that have been modified, added, or removed.
pub(crate) async fn refresh_loop(
    cache: TableCache,
    timestamps: TimestampCache,
    directory: PathBuf,
    interval: Duration,
    change_tx: broadcast::Sender<ChangeEvent>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    let mut ticker = tokio::time::interval(interval);
    // First tick fires immediately — skip it since we loaded on init
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                // Snapshot current table names before refresh
                let old_tables: HashMap<String, std::time::SystemTime> = {
                    timestamps.read().await.clone()
                };

                // Reload everything from disk
                if let Err(e) = load_all_tables(&directory, &cache, &timestamps).await {
                    tracing::warn!(error = %e, "Background refresh failed");
                    continue;
                }

                // Detect changes by comparing timestamps
                let new_timestamps = timestamps.read().await.clone();

                for (table, new_ts) in &new_timestamps {
                    let changed = match old_tables.get(table) {
                        Some(old_ts) => new_ts != old_ts,
                        None => true, // New table
                    };

                    if changed {
                        tracing::debug!(table = %table, "Table refreshed from disk");
                        let _ = change_tx.send(ChangeEvent {
                            table: table.clone(),
                            operation: ChangeOperation::Refreshed,
                        });
                    }
                }

                // Detect removed tables
                for table in old_tables.keys() {
                    if !new_timestamps.contains_key(table) {
                        tracing::debug!(table = %table, "Table removed from disk");
                        let _ = change_tx.send(ChangeEvent {
                            table: table.clone(),
                            operation: ChangeOperation::Deleted,
                        });
                    }
                }
            }
            _ = &mut shutdown_rx => {
                tracing::debug!("Refresh loop shutting down");
                break;
            }
        }
    }
}
