// Project:   hyperi-rustlib
// File:      tests/sync_in_async.rs
// Purpose:   Mechanical lint enforcing the "no blocking I/O in async" rule
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Grep-based lint: scan every `.rs` file under `src/` for `async fn`
//! bodies containing synchronous blocking calls. Each match is a
//! latent runtime stall — the call pins a tokio worker thread for the
//! duration of the I/O, starving every other task on that worker.
//!
//! The lint is intentionally simple and conservative:
//!
//! - Walks `src/` recursively.
//! - Tracks brace depth to identify lines inside `async fn` bodies.
//! - Reports a violation if any forbidden pattern appears.
//!
//! False positives are addressed with `// allow-sync-in-async:` line
//! suppressors. Use sparingly, with a justification comment.
//!
//! See `src/concurrency/mod.rs` "The hard rule" for the policy and
//! the three async primitives that should replace these violations.

use std::fs;
use std::path::{Path, PathBuf};

/// Forbidden patterns. Match is byte-substring (not regex) so the test
/// stays fast and doesn't drag in a regex dep.
///
/// `parking_lot::*::lock()` held across `.await` is NOT detected here —
/// `clippy::await_holding_lock` catches it more reliably and is
/// enabled crate-wide via `#![warn(clippy::pedantic)]`.
const FORBIDDEN: &[(&str, &str)] = &[
    ("std::fs::", "blocking filesystem call — use tokio::fs"),
    (
        "std::io::Write::write",
        "blocking write — use AsyncWriteExt or BackgroundSink",
    ),
    (
        "std::thread::sleep",
        "blocking sleep — use tokio::time::sleep",
    ),
    (
        "reqwest::blocking",
        "blocking HTTP client — use reqwest::Client",
    ),
];

/// Lines containing this marker are exempt from the lint. Reserve for
/// genuine false positives (e.g. setup before the runtime is hot).
const ALLOW_MARKER: &str = "allow-sync-in-async";

// Load-bearing at zero. Any new violation fails CI. If you need to add
// one, justify it inline with `// allow-sync-in-async: <reason>` — the
// marker is reviewable and grep-able.

fn collect_rs_files(root: &Path, acc: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, acc);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            acc.push(path);
        }
    }
}

/// Walk the file's tokens just enough to know which lines lie inside an
/// `async fn` body **outside** a `#[cfg(test)] mod` block. Tests are
/// exempt — they run under per-test runtimes and don't risk starving
/// production tasks.
fn scan_file(path: &Path) -> Vec<String> {
    let Ok(source) = fs::read_to_string(path) else {
        return Vec::new();
    };

    let mut violations = Vec::new();
    let mut depth: i32 = 0;
    let mut async_depths: Vec<i32> = Vec::new();
    let mut test_mod_depths: Vec<i32> = Vec::new();
    let mut pending_async = false;
    let mut pending_test_mod = false;
    let mut last_was_cfg_test = false;

    for (lineno, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        let code = match trimmed.find("//") {
            Some(i) => &trimmed[..i],
            None => trimmed,
        };

        // `#[cfg(test)]` on its own line; the next `mod` opens a test block.
        let cfg_test_here = code.contains("#[cfg(test)]");
        if cfg_test_here {
            last_was_cfg_test = true;
        }

        // `mod <name> {` immediately after `#[cfg(test)]` enters a test mod.
        if last_was_cfg_test && code.starts_with("mod ") {
            pending_test_mod = true;
            last_was_cfg_test = false;
        }

        // `async fn` (skip trait declarations ending in `;`).
        if code.contains("async fn") && !code.trim_end().ends_with(';') {
            pending_async = true;
        }

        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    if pending_test_mod {
                        test_mod_depths.push(depth);
                        pending_test_mod = false;
                    }
                    if pending_async {
                        async_depths.push(depth);
                        pending_async = false;
                    }
                }
                '}' => {
                    if async_depths.last() == Some(&depth) {
                        async_depths.pop();
                    }
                    if test_mod_depths.last() == Some(&depth) {
                        test_mod_depths.pop();
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }

        // Reset cfg(test) tracking if we passed a non-mod, non-attr line.
        if !cfg_test_here && !code.is_empty() && !code.starts_with("#[") {
            last_was_cfg_test = false;
        }

        // Inside an async fn body and NOT inside a test mod?
        if async_depths.is_empty() || !test_mod_depths.is_empty() {
            continue;
        }

        if line.contains(ALLOW_MARKER) {
            continue;
        }

        for (needle, reason) in FORBIDDEN {
            if code.contains(needle) {
                violations.push(format!(
                    "{}:{}: forbidden `{}` ({})",
                    path.display(),
                    lineno + 1,
                    needle,
                    reason,
                ));
            }
        }
    }

    violations
}

#[test]
fn no_sync_in_async() {
    let mut files = Vec::new();
    collect_rs_files(Path::new("src"), &mut files);

    let violations: Vec<String> = files.iter().flat_map(|p| scan_file(p)).collect();

    if !violations.is_empty() {
        eprintln!("\n== sync-in-async violations ==");
        for v in &violations {
            eprintln!("  {v}");
        }
        eprintln!("== total: {} ==\n", violations.len());
    }

    assert!(
        violations.is_empty(),
        "found {} sync-in-async violation(s) — see eprintln output above. \
         Migrate to BackgroundSink / PeriodicWorker / tokio::fs, or add \
         an `// allow-sync-in-async: <reason>` marker if genuinely safe.",
        violations.len(),
    );
}

#[test]
fn lint_finds_its_own_violations() {
    // Self-test: the scanner correctly identifies a planted violation
    // and ignores the same pattern outside an async fn or inside an
    // allow-marker line.
    let tmp = std::env::temp_dir().join("sync_in_async_lint_test.rs");
    let source = "\
fn sync_ok() {
    std::fs::write(\"/tmp/x\", b\"\").unwrap();
}

async fn bad() {
    std::fs::write(\"/tmp/x\", b\"\").unwrap();
}

async fn allowed() {
    std::fs::write(\"/tmp/x\", b\"\").unwrap(); // allow-sync-in-async: test
}
";
    fs::write(&tmp, source).expect("write temp");
    let violations = scan_file(&tmp);
    let _ = fs::remove_file(&tmp);

    assert_eq!(
        violations.len(),
        1,
        "expected exactly one violation (the bad() body), got: {violations:?}",
    );
    assert!(
        violations[0].contains("std::fs::"),
        "violation should be std::fs::, got {}",
        violations[0],
    );
}
