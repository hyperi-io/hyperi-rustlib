// Project:   hyperi-rustlib
// File:      src/directory_config/store.rs
// Purpose:   Core DirectoryConfigStore implementation
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{RwLock, broadcast};

use crate::directory_config::error::{DirectoryConfigError, DirectoryConfigResult};
use crate::directory_config::refresh::refresh_loop;
use crate::directory_config::types::{
    ChangeEvent, ChangeOperation, DirectoryConfigStoreConfig, WriteMode, WriteResult,
};

/// Table cache: table name -> parsed YAML value.
pub(crate) type TableCache = Arc<RwLock<HashMap<String, serde_yaml_ng::Value>>>;

/// Timestamp cache: table name -> last modified time (for change detection).
pub(crate) type TimestampCache = Arc<RwLock<HashMap<String, std::time::SystemTime>>>;

/// Directory-based config store with YAML files as tables.
///
/// Each YAML file in the configured directory represents a "table".
/// Files are cached in memory and refreshed by background polling.
/// Write operations use advisory file locking and optionally commit
/// changes via git.
#[derive(Debug)]
pub struct DirectoryConfigStore {
    config: DirectoryConfigStoreConfig,
    cache: TableCache,
    timestamps: TimestampCache,
    write_mode: WriteMode,
    change_tx: broadcast::Sender<ChangeEvent>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl DirectoryConfigStore {
    /// Create a new store. Validates the directory exists and detects write mode.
    /// Does NOT start background refresh — call `start()` for that.
    pub async fn new(config: DirectoryConfigStoreConfig) -> DirectoryConfigResult<Self> {
        let dir = &config.directory;

        if !dir.exists() || !dir.is_dir() {
            return Err(DirectoryConfigError::DirectoryNotFound(
                dir.display().to_string(),
            ));
        }

        let write_mode = detect_write_mode(dir, config.git_enabled);
        let cache: TableCache = Arc::new(RwLock::new(HashMap::new()));
        let timestamps: TimestampCache = Arc::new(RwLock::new(HashMap::new()));

        // Broadcast channel for change events (capacity 64, dropped events are OK)
        let (change_tx, _) = broadcast::channel(64);

        let store = Self {
            config,
            cache: cache.clone(),
            timestamps: timestamps.clone(),
            write_mode,
            change_tx,
            shutdown_tx: None,
        };

        // Initial load
        load_all_tables(&store.config.directory, &cache, &timestamps).await?;

        tracing::info!(
            directory = %store.config.directory.display(),
            write_mode = ?store.write_mode,
            tables = store.cache.read().await.len(),
            "DirectoryConfigStore initialised"
        );

        Ok(store)
    }

    /// Start background polling refresh.
    pub async fn start(&mut self) -> DirectoryConfigResult<()> {
        if self.shutdown_tx.is_some() {
            return Err(DirectoryConfigError::AlreadyRunning);
        }

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        let cache = self.cache.clone();
        let timestamps = self.timestamps.clone();
        let directory = self.config.directory.clone();
        let interval = self.config.refresh_interval;
        let change_tx = self.change_tx.clone();

        tokio::spawn(async move {
            refresh_loop(
                cache,
                timestamps,
                directory,
                interval,
                change_tx,
                shutdown_rx,
            )
            .await;
        });

        tracing::info!(
            interval_secs = self.config.refresh_interval.as_secs(),
            "Background refresh started"
        );

        Ok(())
    }

    /// Stop background polling refresh.
    pub async fn stop(&mut self) -> DirectoryConfigResult<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
            tracing::info!("Background refresh stopped");
            Ok(())
        } else {
            Err(DirectoryConfigError::NotStarted)
        }
    }

    /// List all table names (YAML filenames without extension).
    pub async fn list_tables(&self) -> Vec<String> {
        let cache = self.cache.read().await;
        let mut tables: Vec<String> = cache.keys().cloned().collect();
        tables.sort();
        tables
    }

    /// Get the entire contents of a table as a YAML value.
    ///
    /// Table names may include subdirectory prefixes (e.g. `loaders/dfe-loader`).
    pub async fn get(&self, table: &str) -> DirectoryConfigResult<serde_yaml_ng::Value> {
        validate_table_name(table)?;
        let table = normalize_table_name(table);
        let cache = self.cache.read().await;
        cache
            .get(table)
            .cloned()
            .ok_or_else(|| DirectoryConfigError::TableNotFound(table.to_string()))
    }

    /// Get a specific key from a table using dot-notation path.
    ///
    /// For example, `get_key("loaders/dfe-loader", "kafka.brokers")` navigates
    /// into the nested YAML structure. Dot separates keys, slash separates
    /// subdirectory path components in the table name.
    pub async fn get_key(
        &self,
        table: &str,
        key: &str,
    ) -> DirectoryConfigResult<serde_yaml_ng::Value> {
        let value = self.get(table).await?;
        let table = normalize_table_name(table);
        navigate_yaml(&value, key).ok_or_else(|| DirectoryConfigError::KeyNotFound {
            table: table.to_string(),
            key: key.to_string(),
        })
    }

    /// Deserialise a table into a typed struct.
    ///
    /// Table names may include subdirectory prefixes (e.g. `loaders/dfe-loader`).
    pub async fn get_as<T: serde::de::DeserializeOwned>(
        &self,
        table: &str,
    ) -> DirectoryConfigResult<T> {
        let value = self.get(table).await?;
        let table = normalize_table_name(table);
        serde_yaml_ng::from_value(value).map_err(|e| DirectoryConfigError::ParseError {
            file: table.to_string(),
            message: e.to_string(),
        })
    }

    /// Set a key in a table. Creates the table file if it doesn't exist.
    ///
    /// Returns `ReadOnly` error if the store is not writable.
    pub async fn set(
        &self,
        table: &str,
        key: &str,
        value: serde_yaml_ng::Value,
        message: Option<&str>,
    ) -> DirectoryConfigResult<WriteResult> {
        self.check_writable()?;
        validate_table_name(table)?;

        let table = normalize_table_name(table);
        let file_path = self.table_path(table);

        // Auto-create parent directories for subdirectory tables
        if let Some(parent) = file_path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)?;
        }

        // Load current content or start with empty mapping
        let mut doc = if file_path.exists() {
            load_yaml_file(&file_path)?
        } else {
            serde_yaml_ng::Value::Mapping(serde_yaml_ng::Mapping::new())
        };

        // Set the key (supports dot-notation)
        set_yaml_key(&mut doc, key, value);

        // Write with advisory lock
        write_yaml_locked(&file_path, &doc)?;

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(table.to_string(), doc);
        }

        // Git commit if applicable
        let (branch, commit) = self.maybe_git_commit(table, message).await?;

        let result = WriteResult {
            table: table.to_string(),
            operation: ChangeOperation::Updated,
            branch,
            commit,
        };

        // Broadcast change
        let _ = self.change_tx.send(ChangeEvent {
            table: table.to_string(),
            operation: ChangeOperation::Updated,
        });

        Ok(result)
    }

    /// Delete a key from a table.
    ///
    /// Returns `ReadOnly` error if the store is not writable.
    /// Returns `TableNotFound` if the table doesn't exist.
    /// Returns `KeyNotFound` if the key doesn't exist.
    pub async fn delete_key(
        &self,
        table: &str,
        key: &str,
        message: Option<&str>,
    ) -> DirectoryConfigResult<WriteResult> {
        self.check_writable()?;
        validate_table_name(table)?;

        let table = normalize_table_name(table);
        let file_path = self.table_path(table);
        if !file_path.exists() {
            return Err(DirectoryConfigError::TableNotFound(table.to_string()));
        }

        let mut doc = load_yaml_file(&file_path)?;

        if !remove_yaml_key(&mut doc, key) {
            return Err(DirectoryConfigError::KeyNotFound {
                table: table.to_string(),
                key: key.to_string(),
            });
        }

        write_yaml_locked(&file_path, &doc)?;

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(table.to_string(), doc);
        }

        let (branch, commit) = self.maybe_git_commit(table, message).await?;

        let result = WriteResult {
            table: table.to_string(),
            operation: ChangeOperation::Deleted,
            branch,
            commit,
        };

        let _ = self.change_tx.send(ChangeEvent {
            table: table.to_string(),
            operation: ChangeOperation::Deleted,
        });

        Ok(result)
    }

    /// Subscribe to change events for a specific table (or all tables).
    ///
    /// Returns a broadcast receiver that receives `ChangeEvent`s.
    /// Pass an empty string to receive events for all tables.
    #[must_use]
    pub fn on_change(&self) -> broadcast::Receiver<ChangeEvent> {
        self.change_tx.subscribe()
    }

    /// Get the current write mode.
    #[must_use]
    pub fn write_mode(&self) -> WriteMode {
        self.write_mode
    }

    /// Check if this store is backed by a git repository.
    #[must_use]
    pub fn is_git(&self) -> bool {
        self.write_mode == WriteMode::GitCommit
    }

    /// Get the current git branch name (requires `directory-config-git` feature).
    #[cfg(feature = "directory-config-git")]
    #[must_use]
    pub fn current_branch(&self) -> Option<String> {
        crate::directory_config::git::git_current_branch(&self.config.directory)
    }

    /// List all git branches (requires `directory-config-git` feature).
    #[cfg(feature = "directory-config-git")]
    pub fn list_branches(&self) -> DirectoryConfigResult<Vec<String>> {
        if !self.is_git() {
            return Err(DirectoryConfigError::NotGitRepo);
        }
        crate::directory_config::git::git_list_branches(&self.config.directory)
    }

    /// Switch to a git branch, optionally creating it (requires `directory-config-git` feature).
    #[cfg(feature = "directory-config-git")]
    pub fn switch_branch(&self, branch: &str, create: bool) -> DirectoryConfigResult<()> {
        if !self.is_git() {
            return Err(DirectoryConfigError::NotGitRepo);
        }
        crate::directory_config::git::git_switch_branch(&self.config.directory, branch, create)
    }

    // --- Internal helpers ---

    fn check_writable(&self) -> DirectoryConfigResult<()> {
        if self.write_mode == WriteMode::ReadOnly {
            return Err(DirectoryConfigError::ReadOnly);
        }
        Ok(())
    }

    /// Resolve the on-disk path for a table name.
    ///
    /// Supports subdirectory table names (e.g. `loaders/dfe-loader`).
    /// Checks for `.yaml` first, falls back to `.yml` if it exists.
    /// For new files (writes), always uses `.yaml`.
    fn table_path(&self, table: &str) -> PathBuf {
        let table = normalize_table_name(table);
        let yaml_path = self.config.directory.join(format!("{table}.yaml"));
        if yaml_path.exists() {
            return yaml_path;
        }
        let yml_path = self.config.directory.join(format!("{table}.yml"));
        if yml_path.exists() {
            return yml_path;
        }
        // Default to .yaml for new files
        yaml_path
    }

    #[allow(unused_variables)]
    async fn maybe_git_commit(
        &self,
        table: &str,
        message: Option<&str>,
    ) -> DirectoryConfigResult<(Option<String>, Option<String>)> {
        if self.write_mode != WriteMode::GitCommit {
            return Ok((None, None));
        }

        #[cfg(feature = "directory-config-git")]
        {
            let table = normalize_table_name(table);
            let default_msg = format!("config: update {table}");
            let msg = message.unwrap_or(&default_msg);

            // Resolve actual filename on disk (may be .yml)
            let file_path = self.table_path(table);
            let filename = file_path
                .strip_prefix(&self.config.directory)
                .unwrap_or(&file_path)
                .to_string_lossy()
                .to_string();

            let commit_hash = crate::directory_config::git::git_add_and_commit(
                &self.config.directory,
                &filename,
                msg,
                &self.config.git_author_name,
                &self.config.git_author_email,
            )?;

            if self.config.git_push {
                crate::directory_config::git::git_push(&self.config.directory)?;
            }

            let branch = crate::directory_config::git::git_current_branch(&self.config.directory);

            Ok((branch, Some(commit_hash)))
        }

        #[cfg(not(feature = "directory-config-git"))]
        Ok((None, None))
    }
}

// --- Free functions ---

/// Validate a table name.
///
/// Table names may contain forward slashes for subdirectory access
/// (e.g. `loaders/dfe-loader`). Rejects path traversal (`..`),
/// leading slashes, backslashes, and empty segments.
pub(crate) fn validate_table_name(table: &str) -> DirectoryConfigResult<()> {
    let trimmed = table.trim_matches('/');
    if trimmed.is_empty() {
        return Err(DirectoryConfigError::InvalidTableName(
            "table name is empty".to_string(),
        ));
    }
    if table.contains('\\') {
        return Err(DirectoryConfigError::InvalidTableName(
            "backslash not allowed".to_string(),
        ));
    }
    for segment in trimmed.split('/') {
        if segment.is_empty() {
            return Err(DirectoryConfigError::InvalidTableName(
                "empty path segment".to_string(),
            ));
        }
        if segment == ".." {
            return Err(DirectoryConfigError::InvalidTableName(
                "path traversal not allowed".to_string(),
            ));
        }
        if segment == "." {
            return Err(DirectoryConfigError::InvalidTableName(
                "current directory reference not allowed".to_string(),
            ));
        }
    }
    Ok(())
}

/// Normalise a table name by stripping leading/trailing slashes.
fn normalize_table_name(table: &str) -> &str {
    table.trim_matches('/')
}

/// Detect write mode for the directory.
fn detect_write_mode(dir: &Path, git_enabled: bool) -> WriteMode {
    // Check writability by attempting to create a temp file
    let probe = dir.join(".dcs_write_probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
        }
        Err(_) => return WriteMode::ReadOnly,
    }

    // Check for git repo
    if git_enabled && dir.join(".git").exists() {
        WriteMode::GitCommit
    } else {
        WriteMode::DirectWrite
    }
}

/// Load all YAML files from directory (recursively) into cache.
///
/// Table names are derived from relative paths within the root directory.
/// Files at the root are named by their stem (e.g. `dfe-loader.yaml` → `dfe-loader`).
/// Files in subdirectories include the path prefix (e.g. `loaders/dfe-loader.yaml` → `loaders/dfe-loader`).
pub(crate) async fn load_all_tables(
    dir: &Path,
    cache: &TableCache,
    timestamps: &TimestampCache,
) -> DirectoryConfigResult<()> {
    let mut new_cache = HashMap::new();
    let mut new_timestamps = HashMap::new();

    load_tables_recursive(
        dir,
        dir,
        cache,
        timestamps,
        &mut new_cache,
        &mut new_timestamps,
    )
    .await?;

    // Swap cache contents
    {
        let mut cache_w = cache.write().await;
        *cache_w = new_cache;
    }
    {
        let mut ts_w = timestamps.write().await;
        *ts_w = new_timestamps;
    }

    Ok(())
}

/// Recursively walk a directory tree loading YAML files.
async fn load_tables_recursive(
    root: &Path,
    current: &Path,
    cache: &TableCache,
    timestamps: &TimestampCache,
    new_cache: &mut HashMap<String, serde_yaml_ng::Value>,
    new_timestamps: &mut HashMap<String, std::time::SystemTime>,
) -> DirectoryConfigResult<()> {
    let mut entries = tokio::fs::read_dir(current).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let file_type = entry.file_type().await?;

        // Skip hidden files/directories (e.g. .git)
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }

        if file_type.is_dir() {
            Box::pin(load_tables_recursive(
                root,
                &path,
                cache,
                timestamps,
                new_cache,
                new_timestamps,
            ))
            .await?;
            continue;
        }

        let ext = path.extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("yaml" | "yml")) {
            continue;
        }

        // Derive table name from relative path (minus extension)
        let rel = path.strip_prefix(root).unwrap_or(&path);
        let table_name = rel.with_extension("").to_string_lossy().replace('\\', "/");

        if table_name.is_empty() {
            continue;
        }

        let modified = entry
            .metadata()
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => match serde_yaml_ng::from_str(&contents) {
                Ok(value) => {
                    new_cache.insert(table_name.clone(), value);
                    new_timestamps.insert(table_name, modified);
                }
                Err(e) => {
                    tracing::warn!(
                        file = %path.display(),
                        error = %e,
                        "Corrupt YAML, keeping last known good"
                    );
                    let existing = cache.read().await;
                    if let Some(existing_value) = existing.get(&table_name) {
                        new_cache.insert(table_name.clone(), existing_value.clone());
                    }
                    if let Some(ts) = timestamps.read().await.get(&table_name) {
                        new_timestamps.insert(table_name, *ts);
                    }
                }
            },
            Err(e) => {
                tracing::warn!(
                    file = %path.display(),
                    error = %e,
                    "Failed to read file"
                );
            }
        }
    }

    Ok(())
}

/// Load a single YAML file (blocking, for write operations).
fn load_yaml_file(path: &Path) -> DirectoryConfigResult<serde_yaml_ng::Value> {
    let contents = std::fs::read_to_string(path)?;
    serde_yaml_ng::from_str(&contents).map_err(|e| DirectoryConfigError::ParseError {
        file: path.display().to_string(),
        message: e.to_string(),
    })
}

/// Write YAML to file with advisory file lock.
fn write_yaml_locked(path: &Path, value: &serde_yaml_ng::Value) -> DirectoryConfigResult<()> {
    let yaml_str = serde_yaml_ng::to_string(value)
        .map_err(|e| DirectoryConfigError::SerializationError(e.to_string()))?;

    // Open/create the file, acquire exclusive lock, write, release on drop
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;

    file.lock().map_err(DirectoryConfigError::IoError)?;

    std::fs::write(path, yaml_str.as_bytes())?;

    file.unlock().map_err(DirectoryConfigError::IoError)?;

    Ok(())
}

/// Navigate a YAML value using dot-notation key path.
fn navigate_yaml(value: &serde_yaml_ng::Value, key: &str) -> Option<serde_yaml_ng::Value> {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current = value;

    for part in &parts {
        match current {
            serde_yaml_ng::Value::Mapping(map) => {
                let yaml_key = serde_yaml_ng::Value::String((*part).to_string());
                current = map.get(&yaml_key)?;
            }
            _ => return None,
        }
    }

    Some(current.clone())
}

/// Set a value at a dot-notation key path in a YAML document.
fn set_yaml_key(doc: &mut serde_yaml_ng::Value, key: &str, value: serde_yaml_ng::Value) {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current = doc;

    for (i, part) in parts.iter().enumerate() {
        let yaml_key = serde_yaml_ng::Value::String((*part).to_string());

        if i == parts.len() - 1 {
            // Last part — set the value
            if let serde_yaml_ng::Value::Mapping(map) = current {
                map.insert(yaml_key, value);
                return;
            }
        } else {
            // Intermediate part — navigate or create mapping
            if !current.is_mapping() {
                *current = serde_yaml_ng::Value::Mapping(serde_yaml_ng::Mapping::new());
            }
            let map = current.as_mapping_mut().unwrap();
            if !map.contains_key(&yaml_key) {
                map.insert(
                    yaml_key.clone(),
                    serde_yaml_ng::Value::Mapping(serde_yaml_ng::Mapping::new()),
                );
            }
            current = map.get_mut(&yaml_key).unwrap();
        }
    }
}

/// Remove a key at a dot-notation path. Returns true if the key existed.
fn remove_yaml_key(doc: &mut serde_yaml_ng::Value, key: &str) -> bool {
    let parts: Vec<&str> = key.split('.').collect();

    if parts.len() == 1 {
        if let serde_yaml_ng::Value::Mapping(map) = doc {
            let yaml_key = serde_yaml_ng::Value::String(parts[0].to_string());
            return map.remove(&yaml_key).is_some();
        }
        return false;
    }

    // Navigate to parent, then remove last key
    let parent_parts = &parts[..parts.len() - 1];
    let last_key = parts[parts.len() - 1];
    let mut current = &mut *doc;

    for part in parent_parts {
        let yaml_key = serde_yaml_ng::Value::String((*part).to_string());
        match current {
            serde_yaml_ng::Value::Mapping(map) => {
                if let Some(next) = map.get_mut(&yaml_key) {
                    current = next;
                } else {
                    return false;
                }
            }
            _ => return false,
        }
    }

    if let serde_yaml_ng::Value::Mapping(map) = current {
        let yaml_key = serde_yaml_ng::Value::String(last_key.to_string());
        map.remove(&yaml_key).is_some()
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_navigate_yaml() {
        let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            r"
            database:
              host: localhost
              port: 5432
            name: test
            ",
        )
        .unwrap();

        assert_eq!(
            navigate_yaml(&yaml, "name"),
            Some(serde_yaml_ng::Value::String("test".to_string()))
        );
        assert_eq!(
            navigate_yaml(&yaml, "database.host"),
            Some(serde_yaml_ng::Value::String("localhost".to_string()))
        );
        assert!(navigate_yaml(&yaml, "missing").is_none());
        assert!(navigate_yaml(&yaml, "database.missing").is_none());
    }

    #[test]
    fn test_set_yaml_key() {
        let mut doc = serde_yaml_ng::Value::Mapping(serde_yaml_ng::Mapping::new());

        set_yaml_key(
            &mut doc,
            "name",
            serde_yaml_ng::Value::String("test".to_string()),
        );
        assert_eq!(
            navigate_yaml(&doc, "name"),
            Some(serde_yaml_ng::Value::String("test".to_string()))
        );

        // Nested key
        set_yaml_key(
            &mut doc,
            "database.host",
            serde_yaml_ng::Value::String("localhost".to_string()),
        );
        assert_eq!(
            navigate_yaml(&doc, "database.host"),
            Some(serde_yaml_ng::Value::String("localhost".to_string()))
        );
    }

    #[test]
    fn test_remove_yaml_key() {
        let mut doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            r"
            database:
              host: localhost
              port: 5432
            name: test
            ",
        )
        .unwrap();

        assert!(remove_yaml_key(&mut doc, "name"));
        assert!(navigate_yaml(&doc, "name").is_none());

        assert!(remove_yaml_key(&mut doc, "database.host"));
        assert!(navigate_yaml(&doc, "database.host").is_none());
        // port should still exist
        assert!(navigate_yaml(&doc, "database.port").is_some());

        // Non-existent key
        assert!(!remove_yaml_key(&mut doc, "missing"));
    }

    #[test]
    fn test_detect_write_mode_readonly() {
        // A directory that doesn't exist should be caught before detect_write_mode,
        // but /proc is a good read-only test
        let mode = detect_write_mode(Path::new("/proc"), false);
        assert_eq!(mode, WriteMode::ReadOnly);
    }

    #[test]
    fn test_detect_write_mode_writable() {
        let tmp = tempfile::tempdir().unwrap();
        let mode = detect_write_mode(tmp.path(), false);
        assert_eq!(mode, WriteMode::DirectWrite);
    }

    #[test]
    fn test_validate_table_name_valid() {
        assert!(validate_table_name("dfe-loader").is_ok());
        assert!(validate_table_name("loaders/dfe-loader").is_ok());
        assert!(validate_table_name("a/b/c").is_ok());
        assert!(validate_table_name("my_table").is_ok());
    }

    #[test]
    fn test_validate_table_name_rejects_traversal() {
        assert!(validate_table_name("../etc/passwd").is_err());
        assert!(validate_table_name("foo/../../bar").is_err());
        assert!(validate_table_name("..").is_err());
    }

    #[test]
    fn test_validate_table_name_rejects_backslash() {
        assert!(validate_table_name("foo\\bar").is_err());
    }

    #[test]
    fn test_validate_table_name_rejects_empty() {
        assert!(validate_table_name("").is_err());
        assert!(validate_table_name("/").is_err());
        assert!(validate_table_name("//").is_err());
    }

    #[test]
    fn test_validate_table_name_rejects_empty_segments() {
        assert!(validate_table_name("foo//bar").is_err());
    }

    #[test]
    fn test_validate_table_name_rejects_single_dot() {
        assert!(validate_table_name(".").is_err());
        assert!(validate_table_name("./foo").is_err());
        assert!(validate_table_name("foo/./bar").is_err());
    }

    #[test]
    fn test_normalize_table_name() {
        assert_eq!(normalize_table_name("foo"), "foo");
        assert_eq!(normalize_table_name("/foo/"), "foo");
        assert_eq!(normalize_table_name("loaders/dfe"), "loaders/dfe");
        assert_eq!(normalize_table_name("/loaders/dfe/"), "loaders/dfe");
    }
}
