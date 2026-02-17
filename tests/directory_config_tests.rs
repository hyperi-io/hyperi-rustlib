// Project:   hyperi-rustlib
// File:      tests/directory_config_tests.rs
// Purpose:   Integration tests for DirectoryConfigStore
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

#![cfg(feature = "directory-config")]

use std::path::PathBuf;
use std::time::Duration;

use hyperi_rustlib::directory_config::{
    ChangeOperation, DirectoryConfigError, DirectoryConfigStore, DirectoryConfigStoreConfig,
    WriteMode,
};

#[cfg(feature = "directory-config-git")]
use git2::Repository;

/// Helper to create a config pointing at a temp directory.
fn test_config(dir: &std::path::Path) -> DirectoryConfigStoreConfig {
    DirectoryConfigStoreConfig {
        directory: dir.to_path_buf(),
        refresh_interval: Duration::from_millis(100),
        git_enabled: false,
        git_push: false,
        ..Default::default()
    }
}

/// Write a YAML file into the given directory.
/// Supports subdirectory table names (e.g. `loaders/dfe-loader`).
fn write_yaml(dir: &std::path::Path, name: &str, content: &str) {
    let path = dir.join(format!("{name}.yaml"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

// --- Construction and initialisation ---

#[tokio::test]
async fn test_new_with_empty_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    assert!(store.list_tables().await.is_empty());
    assert_eq!(store.write_mode(), WriteMode::DirectWrite);
}

#[tokio::test]
async fn test_new_with_nonexistent_directory() {
    let config = DirectoryConfigStoreConfig {
        directory: PathBuf::from("/nonexistent/path"),
        ..Default::default()
    };
    let result = DirectoryConfigStore::new(config).await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        DirectoryConfigError::DirectoryNotFound(_)
    ));
}

#[tokio::test]
async fn test_new_loads_yaml_files() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "service-a", "host: localhost\nport: 8080\n");
    write_yaml(tmp.path(), "service-b", "name: test\n");

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let tables = store.list_tables().await;
    assert_eq!(tables, vec!["service-a", "service-b"]);
}

#[tokio::test]
async fn test_non_yaml_files_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "valid", "key: value\n");
    std::fs::write(tmp.path().join("readme.txt"), "not yaml").unwrap();
    std::fs::write(tmp.path().join("data.json"), "{}").unwrap();

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let tables = store.list_tables().await;
    assert_eq!(tables, vec!["valid"]);
}

// --- Read API ---

#[tokio::test]
async fn test_get_returns_full_table() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(
        tmp.path(),
        "app",
        "database:\n  host: db.local\n  port: 5432\n",
    );

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let value = store.get("app").await.unwrap();
    assert!(value.is_mapping());
}

#[tokio::test]
async fn test_get_table_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let result = store.get("missing").await;
    assert!(matches!(
        result.unwrap_err(),
        DirectoryConfigError::TableNotFound(_)
    ));
}

#[tokio::test]
async fn test_get_key_top_level() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "app", "name: myapp\nversion: 2\n");

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let value = store.get_key("app", "name").await.unwrap();
    assert_eq!(value, serde_yaml_ng::Value::String("myapp".to_string()));
}

#[tokio::test]
async fn test_get_key_nested() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(
        tmp.path(),
        "app",
        "database:\n  host: db.local\n  port: 5432\n",
    );

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let value = store.get_key("app", "database.host").await.unwrap();
    assert_eq!(value, serde_yaml_ng::Value::String("db.local".to_string()));
}

#[tokio::test]
async fn test_get_key_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "app", "name: myapp\n");

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let result = store.get_key("app", "missing.key").await;
    assert!(matches!(
        result.unwrap_err(),
        DirectoryConfigError::KeyNotFound { .. }
    ));
}

#[tokio::test]
async fn test_get_as_typed() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "db", "host: localhost\nport: 5432\nssl: true\n");

    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct DbConfig {
        host: String,
        port: u16,
        ssl: bool,
    }

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let config: DbConfig = store.get_as("db").await.unwrap();
    assert_eq!(
        config,
        DbConfig {
            host: "localhost".to_string(),
            port: 5432,
            ssl: true,
        }
    );
}

// --- Write API ---

#[tokio::test]
async fn test_set_creates_new_table() {
    let tmp = tempfile::tempdir().unwrap();
    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();

    let result = store
        .set(
            "new-service",
            "host",
            serde_yaml_ng::Value::String("localhost".to_string()),
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.table, "new-service");
    assert_eq!(result.operation, ChangeOperation::Updated);

    // Verify in cache
    let value = store.get_key("new-service", "host").await.unwrap();
    assert_eq!(value, serde_yaml_ng::Value::String("localhost".to_string()));

    // Verify on disk
    let on_disk = std::fs::read_to_string(tmp.path().join("new-service.yaml")).unwrap();
    assert!(on_disk.contains("localhost"));
}

#[tokio::test]
async fn test_set_updates_existing_key() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "app", "host: old-host\nport: 8080\n");

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();

    store
        .set(
            "app",
            "host",
            serde_yaml_ng::Value::String("new-host".to_string()),
            None,
        )
        .await
        .unwrap();

    let value = store.get_key("app", "host").await.unwrap();
    assert_eq!(value, serde_yaml_ng::Value::String("new-host".to_string()));

    // Port should be unchanged
    let port = store.get_key("app", "port").await.unwrap();
    assert_eq!(port, serde_yaml_ng::Value::Number(8080.into()));
}

#[tokio::test]
async fn test_set_nested_key() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "app", "database:\n  host: old\n");

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();

    store
        .set(
            "app",
            "database.host",
            serde_yaml_ng::Value::String("new-host".to_string()),
            None,
        )
        .await
        .unwrap();

    let value = store.get_key("app", "database.host").await.unwrap();
    assert_eq!(value, serde_yaml_ng::Value::String("new-host".to_string()));
}

#[tokio::test]
async fn test_delete_key() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(
        tmp.path(),
        "app",
        "host: localhost\nport: 8080\ndebug: true\n",
    );

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();

    let result = store.delete_key("app", "debug", None).await.unwrap();
    assert_eq!(result.operation, ChangeOperation::Deleted);

    // Key should be gone
    let err = store.get_key("app", "debug").await.unwrap_err();
    assert!(matches!(err, DirectoryConfigError::KeyNotFound { .. }));

    // Other keys remain
    let host = store.get_key("app", "host").await.unwrap();
    assert_eq!(host, serde_yaml_ng::Value::String("localhost".to_string()));
}

#[tokio::test]
async fn test_delete_key_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "app", "host: localhost\n");

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let result = store.delete_key("app", "missing", None).await;
    assert!(matches!(
        result.unwrap_err(),
        DirectoryConfigError::KeyNotFound { .. }
    ));
}

#[tokio::test]
async fn test_delete_table_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let result = store.delete_key("missing", "key", None).await;
    assert!(matches!(
        result.unwrap_err(),
        DirectoryConfigError::TableNotFound(_)
    ));
}

// --- Write mode ---

#[tokio::test]
async fn test_read_only_rejects_writes() {
    // Create a temp directory with YAML, then make it read-only
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "existing", "key: value\n");

    // Remove write permission
    let mut perms = std::fs::metadata(tmp.path()).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    {
        perms.set_readonly(true);
    }
    std::fs::set_permissions(tmp.path(), perms.clone()).unwrap();

    let config = DirectoryConfigStoreConfig {
        directory: tmp.path().to_path_buf(),
        git_enabled: false,
        ..Default::default()
    };

    let store = DirectoryConfigStore::new(config).await.unwrap();
    assert_eq!(store.write_mode(), WriteMode::ReadOnly);

    let result = store
        .set(
            "test",
            "key",
            serde_yaml_ng::Value::String("val".to_string()),
            None,
        )
        .await;
    assert!(matches!(
        result.unwrap_err(),
        DirectoryConfigError::ReadOnly
    ));

    // Restore permissions so tempdir cleanup works
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

// --- Lifecycle ---

#[tokio::test]
async fn test_start_stop() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();

    store.start().await.unwrap();
    store.stop().await.unwrap();
}

#[tokio::test]
async fn test_double_start_error() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();

    store.start().await.unwrap();
    let result = store.start().await;
    assert!(matches!(
        result.unwrap_err(),
        DirectoryConfigError::AlreadyRunning
    ));

    store.stop().await.unwrap();
}

#[tokio::test]
async fn test_stop_without_start_error() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let result = store.stop().await;
    assert!(matches!(
        result.unwrap_err(),
        DirectoryConfigError::NotStarted
    ));
}

// --- Background refresh ---

#[tokio::test]
async fn test_background_refresh_picks_up_changes() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "app", "version: 1\n");

    let mut store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    store.start().await.unwrap();

    // Modify the file on disk
    write_yaml(tmp.path(), "app", "version: 2\n");

    // Wait for refresh to pick it up (interval is 100ms)
    tokio::time::sleep(Duration::from_millis(350)).await;

    let value = store.get_key("app", "version").await.unwrap();
    assert_eq!(value, serde_yaml_ng::Value::Number(2.into()));

    store.stop().await.unwrap();
}

#[tokio::test]
async fn test_background_refresh_detects_new_table() {
    let tmp = tempfile::tempdir().unwrap();

    let mut store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    assert!(store.list_tables().await.is_empty());

    store.start().await.unwrap();

    // Add a new file on disk
    write_yaml(tmp.path(), "new-service", "enabled: true\n");

    tokio::time::sleep(Duration::from_millis(350)).await;

    let tables = store.list_tables().await;
    assert!(tables.contains(&"new-service".to_string()));

    store.stop().await.unwrap();
}

#[tokio::test]
async fn test_background_refresh_detects_removed_table() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "temporary", "data: test\n");

    let mut store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    assert_eq!(store.list_tables().await, vec!["temporary"]);

    store.start().await.unwrap();

    // Remove the file
    std::fs::remove_file(tmp.path().join("temporary.yaml")).unwrap();

    tokio::time::sleep(Duration::from_millis(350)).await;

    assert!(store.list_tables().await.is_empty());

    store.stop().await.unwrap();
}

// --- Change events ---

#[tokio::test]
async fn test_on_change_receives_write_events() {
    let tmp = tempfile::tempdir().unwrap();
    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();

    let mut rx = store.on_change();

    store
        .set(
            "app",
            "key",
            serde_yaml_ng::Value::String("value".to_string()),
            None,
        )
        .await
        .unwrap();

    let event = rx.try_recv().unwrap();
    assert_eq!(event.table, "app");
    assert_eq!(event.operation, ChangeOperation::Updated);
}

#[tokio::test]
async fn test_on_change_receives_delete_events() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "app", "host: localhost\nport: 8080\n");

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let mut rx = store.on_change();

    store.delete_key("app", "host", None).await.unwrap();

    let event = rx.try_recv().unwrap();
    assert_eq!(event.table, "app");
    assert_eq!(event.operation, ChangeOperation::Deleted);
}

// --- Corrupt YAML ---

#[tokio::test]
async fn test_corrupt_yaml_keeps_last_good() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "app", "host: localhost\n");

    let mut store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let original = store.get_key("app", "host").await.unwrap();
    assert_eq!(
        original,
        serde_yaml_ng::Value::String("localhost".to_string())
    );

    store.start().await.unwrap();

    // Corrupt the file on disk
    std::fs::write(tmp.path().join("app.yaml"), "{{{{invalid yaml!!!!").unwrap();

    tokio::time::sleep(Duration::from_millis(350)).await;

    // Should still have the last good value
    let value = store.get_key("app", "host").await.unwrap();
    assert_eq!(value, serde_yaml_ng::Value::String("localhost".to_string()));

    store.stop().await.unwrap();
}

// --- .yml extension ---

#[tokio::test]
async fn test_yml_extension_supported() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("service.yml");
    std::fs::write(path, "name: test-service\n").unwrap();

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let tables = store.list_tables().await;
    assert_eq!(tables, vec!["service"]);

    let value = store.get_key("service", "name").await.unwrap();
    assert_eq!(
        value,
        serde_yaml_ng::Value::String("test-service".to_string())
    );
}

// --- Subdirectory support ---

#[tokio::test]
async fn test_subdirectory_tables_loaded() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "root-config", "name: root\n");
    write_yaml(tmp.path(), "loaders/dfe-loader", "host: dfe\n");
    write_yaml(tmp.path(), "loaders/csv-loader", "host: csv\n");
    write_yaml(tmp.path(), "sinks/kafka/primary", "brokers: b1\n");

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let tables = store.list_tables().await;

    assert_eq!(
        tables,
        vec![
            "loaders/csv-loader",
            "loaders/dfe-loader",
            "root-config",
            "sinks/kafka/primary",
        ]
    );
}

#[tokio::test]
async fn test_subdirectory_get() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(
        tmp.path(),
        "loaders/dfe-loader",
        "host: dfe-host\nport: 9090\n",
    );

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();

    let value = store.get_key("loaders/dfe-loader", "host").await.unwrap();
    assert_eq!(value, serde_yaml_ng::Value::String("dfe-host".to_string()));
}

#[tokio::test]
async fn test_subdirectory_get_normalises_slashes() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "loaders/dfe-loader", "host: dfe-host\n");

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();

    // Leading/trailing slashes should be stripped
    let value = store.get_key("/loaders/dfe-loader/", "host").await.unwrap();
    assert_eq!(value, serde_yaml_ng::Value::String("dfe-host".to_string()));
}

#[tokio::test]
async fn test_subdirectory_set_creates_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();

    // Write to a subdirectory table that doesn't exist yet
    store
        .set(
            "new-group/my-service",
            "enabled",
            serde_yaml_ng::Value::Bool(true),
            None,
        )
        .await
        .unwrap();

    // Verify in cache
    let value = store
        .get_key("new-group/my-service", "enabled")
        .await
        .unwrap();
    assert_eq!(value, serde_yaml_ng::Value::Bool(true));

    // Verify on disk
    let on_disk = tmp.path().join("new-group/my-service.yaml");
    assert!(on_disk.exists());
}

#[tokio::test]
async fn test_subdirectory_set_deep_nesting() {
    let tmp = tempfile::tempdir().unwrap();
    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();

    store
        .set(
            "a/b/c/deep",
            "key",
            serde_yaml_ng::Value::String("deep-value".to_string()),
            None,
        )
        .await
        .unwrap();

    let value = store.get_key("a/b/c/deep", "key").await.unwrap();
    assert_eq!(
        value,
        serde_yaml_ng::Value::String("deep-value".to_string())
    );
}

#[tokio::test]
async fn test_subdirectory_delete_key() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "loaders/dfe-loader", "host: dfe\nport: 9090\n");

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();

    store
        .delete_key("loaders/dfe-loader", "port", None)
        .await
        .unwrap();

    // Key should be gone
    let result = store.get_key("loaders/dfe-loader", "port").await;
    assert!(matches!(
        result.unwrap_err(),
        DirectoryConfigError::KeyNotFound { .. }
    ));

    // Other key remains
    let host = store.get_key("loaders/dfe-loader", "host").await.unwrap();
    assert_eq!(host, serde_yaml_ng::Value::String("dfe".to_string()));
}

#[tokio::test]
async fn test_subdirectory_yml_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let subdir = tmp.path().join("configs");
    std::fs::create_dir_all(&subdir).unwrap();
    std::fs::write(subdir.join("service.yml"), "name: yml-service\n").unwrap();

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();

    let tables = store.list_tables().await;
    assert_eq!(tables, vec!["configs/service"]);

    let value = store.get_key("configs/service", "name").await.unwrap();
    assert_eq!(
        value,
        serde_yaml_ng::Value::String("yml-service".to_string())
    );
}

#[tokio::test]
async fn test_subdirectory_hidden_dirs_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "visible", "ok: true\n");

    // Create hidden directory with YAML file (should be skipped)
    let hidden = tmp.path().join(".git");
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(hidden.join("config.yaml"), "internal: true\n").unwrap();

    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    let tables = store.list_tables().await;
    assert_eq!(tables, vec!["visible"]);
}

#[tokio::test]
async fn test_invalid_table_name_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();

    // Path traversal
    let result = store.get("../etc/passwd").await;
    assert!(matches!(
        result.unwrap_err(),
        DirectoryConfigError::InvalidTableName(_)
    ));

    // Backslash
    let result = store.get("foo\\bar").await;
    assert!(matches!(
        result.unwrap_err(),
        DirectoryConfigError::InvalidTableName(_)
    ));

    // Empty
    let result = store.get("").await;
    assert!(matches!(
        result.unwrap_err(),
        DirectoryConfigError::InvalidTableName(_)
    ));
}

#[tokio::test]
async fn test_subdirectory_background_refresh() {
    let tmp = tempfile::tempdir().unwrap();
    write_yaml(tmp.path(), "loaders/dfe", "version: 1\n");

    let mut store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    store.start().await.unwrap();

    // Modify file on disk
    write_yaml(tmp.path(), "loaders/dfe", "version: 2\n");

    tokio::time::sleep(Duration::from_millis(350)).await;

    let value = store.get_key("loaders/dfe", "version").await.unwrap();
    assert_eq!(value, serde_yaml_ng::Value::Number(2.into()));

    store.stop().await.unwrap();
}

#[tokio::test]
async fn test_subdirectory_background_refresh_new_subdir() {
    let tmp = tempfile::tempdir().unwrap();

    let mut store = DirectoryConfigStore::new(test_config(tmp.path()))
        .await
        .unwrap();
    assert!(store.list_tables().await.is_empty());

    store.start().await.unwrap();

    // Add a new subdirectory and file
    write_yaml(tmp.path(), "sinks/kafka", "brokers: b1\n");

    tokio::time::sleep(Duration::from_millis(350)).await;

    let tables = store.list_tables().await;
    assert!(tables.contains(&"sinks/kafka".to_string()));

    store.stop().await.unwrap();
}

// --- Git integration tests ---

#[cfg(feature = "directory-config-git")]
mod git_tests {
    use super::*;

    /// Helper: initialise a git repo in a temp dir with an initial commit.
    fn init_git_repo(dir: &std::path::Path) -> Repository {
        let repo = Repository::init(dir).unwrap();

        // Configure user for commits
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test User").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();
        drop(config);

        // Create initial commit (empty tree) so HEAD exists
        {
            let sig = git2::Signature::now("Test User", "test@example.com").unwrap();
            let tree_oid = repo.index().unwrap().write_tree().unwrap();
            let tree = repo.find_tree(tree_oid).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
                .unwrap();
        }

        repo
    }

    /// Helper: config for a git-enabled store.
    fn git_config(dir: &std::path::Path) -> DirectoryConfigStoreConfig {
        DirectoryConfigStoreConfig {
            directory: dir.to_path_buf(),
            refresh_interval: Duration::from_millis(100),
            git_enabled: true,
            git_push: false,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_git_write_mode_detected() {
        let tmp = tempfile::tempdir().unwrap();
        init_git_repo(tmp.path());

        let store = DirectoryConfigStore::new(git_config(tmp.path()))
            .await
            .unwrap();
        assert_eq!(store.write_mode(), WriteMode::GitCommit);
        assert!(store.is_git());
    }

    #[tokio::test]
    async fn test_git_current_branch() {
        let tmp = tempfile::tempdir().unwrap();
        init_git_repo(tmp.path());

        let store = DirectoryConfigStore::new(git_config(tmp.path()))
            .await
            .unwrap();
        let branch = store.current_branch();
        // Default branch for git init is typically "main" or "master"
        assert!(branch.is_some());
    }

    #[tokio::test]
    async fn test_git_list_branches() {
        let tmp = tempfile::tempdir().unwrap();
        init_git_repo(tmp.path());

        let store = DirectoryConfigStore::new(git_config(tmp.path()))
            .await
            .unwrap();
        let branches = store.list_branches().unwrap();
        assert!(!branches.is_empty());
    }

    #[tokio::test]
    async fn test_git_write_creates_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_git_repo(tmp.path());

        let store = DirectoryConfigStore::new(git_config(tmp.path()))
            .await
            .unwrap();

        // Count commits before
        let head_before = repo.head().unwrap().peel_to_commit().unwrap();
        let count_before = {
            let mut revwalk = repo.revwalk().unwrap();
            revwalk.push(head_before.id()).unwrap();
            revwalk.count()
        };

        // Write a key
        store
            .set(
                "app",
                "host",
                serde_yaml_ng::Value::String("localhost".to_string()),
                Some("test: add app config"),
            )
            .await
            .unwrap();

        // Count commits after — should have one more
        let head_after = repo.head().unwrap().peel_to_commit().unwrap();
        let count_after = {
            let mut revwalk = repo.revwalk().unwrap();
            revwalk.push(head_after.id()).unwrap();
            revwalk.count()
        };
        assert_eq!(count_after, count_before + 1);

        // Verify commit message
        let latest = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(latest.message().unwrap(), "test: add app config");
    }

    #[tokio::test]
    async fn test_git_delete_creates_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_git_repo(tmp.path());
        write_yaml(tmp.path(), "app", "host: localhost\nport: 8080\n");

        let store = DirectoryConfigStore::new(git_config(tmp.path()))
            .await
            .unwrap();

        // First: stage the initial file via a set operation so git tracks it
        store
            .set(
                "app",
                "debug",
                serde_yaml_ng::Value::Bool(true),
                Some("add debug flag"),
            )
            .await
            .unwrap();

        // Now delete the key
        store
            .delete_key("app", "debug", Some("remove debug flag"))
            .await
            .unwrap();

        let latest = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(latest.message().unwrap(), "remove debug flag");
    }

    #[tokio::test]
    async fn test_git_switch_branch() {
        let tmp = tempfile::tempdir().unwrap();
        init_git_repo(tmp.path());

        let store = DirectoryConfigStore::new(git_config(tmp.path()))
            .await
            .unwrap();

        // Create and switch to a new branch
        store.switch_branch("feature-test", true).unwrap();
        assert_eq!(store.current_branch(), Some("feature-test".to_string()));

        // Branch should appear in list
        let branches = store.list_branches().unwrap();
        assert!(branches.contains(&"feature-test".to_string()));
    }

    #[tokio::test]
    async fn test_git_switch_back_to_original_branch() {
        let tmp = tempfile::tempdir().unwrap();
        init_git_repo(tmp.path());

        let store = DirectoryConfigStore::new(git_config(tmp.path()))
            .await
            .unwrap();
        let original = store.current_branch().unwrap();

        // Create new branch and switch
        store.switch_branch("other-branch", true).unwrap();
        assert_eq!(store.current_branch(), Some("other-branch".to_string()));

        // Switch back
        store.switch_branch(&original, false).unwrap();
        assert_eq!(store.current_branch(), Some(original));
    }

    #[tokio::test]
    async fn test_git_switch_nonexistent_branch_fails() {
        let tmp = tempfile::tempdir().unwrap();
        init_git_repo(tmp.path());

        let store = DirectoryConfigStore::new(git_config(tmp.path()))
            .await
            .unwrap();
        let result = store.switch_branch("nonexistent", false);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_git_list_branches_on_non_git_errors() {
        let tmp = tempfile::tempdir().unwrap();
        // No git init — DirectWrite mode
        let config = DirectoryConfigStoreConfig {
            directory: tmp.path().to_path_buf(),
            refresh_interval: Duration::from_millis(100),
            git_enabled: false,
            ..Default::default()
        };
        let store = DirectoryConfigStore::new(config).await.unwrap();
        let result = store.list_branches();
        assert!(matches!(
            result.unwrap_err(),
            DirectoryConfigError::NotGitRepo
        ));
    }

    #[tokio::test]
    async fn test_git_subdirectory_write_creates_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_git_repo(tmp.path());

        let store = DirectoryConfigStore::new(git_config(tmp.path()))
            .await
            .unwrap();

        let result = store
            .set(
                "loaders/dfe-loader",
                "host",
                serde_yaml_ng::Value::String("dfe-host".to_string()),
                Some("add dfe-loader config"),
            )
            .await
            .unwrap();

        // Should have git metadata
        assert!(result.commit.is_some());

        // Verify commit message
        let latest = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(latest.message().unwrap(), "add dfe-loader config");

        // Verify file exists on disk
        assert!(tmp.path().join("loaders/dfe-loader.yaml").exists());
    }

    #[tokio::test]
    async fn test_git_write_result_includes_commit() {
        let tmp = tempfile::tempdir().unwrap();
        init_git_repo(tmp.path());

        let store = DirectoryConfigStore::new(git_config(tmp.path()))
            .await
            .unwrap();

        let result = store
            .set(
                "svc",
                "port",
                serde_yaml_ng::Value::Number(9090.into()),
                Some("set port"),
            )
            .await
            .unwrap();

        // WriteResult should have git metadata
        assert!(result.commit.is_some());
        let hash = result.commit.unwrap();
        assert!(!hash.is_empty());
        assert!(hash.len() <= 7);
    }
}
