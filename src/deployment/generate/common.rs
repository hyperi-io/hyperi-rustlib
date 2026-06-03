// Project:   hyperi-rustlib
// File:      src/deployment/generate/common.rs
// Purpose:   Shared template helpers (camel-case, go-ident, file write)
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

#![allow(clippy::format_push_string)]

use std::path::Path;

use crate::deployment::error::DeploymentError;

// ============================================================================
// Helpers
// ============================================================================

/// Convert a group name to camelCase suffix (e.g., "kafka" -> "kafka", "click_house" -> "clickHouse").
pub(super) fn to_camel_suffix(name: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = false;

    for ch in name.chars() {
        if ch == '_' || ch == '-' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(ch.to_ascii_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
        }
    }

    result
}

/// True if `s` is a valid Go-template identifier (matches
/// `[A-Za-z_][A-Za-z0-9_]*`). Go templates accept the dot-walked
/// `.foo.bar` syntax only for keys matching this shape; any other
/// key (hyphens, dots, digit-leading, etc.) must use the
/// `(index .foo "key")` form or `helm lint` fails with
/// `bad character U+002D '-'` (and similar).
pub(super) fn is_go_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Render a Go-template lookup expression that's safe for any key.
///
/// `base` must be a dot-prefixed path that itself only contains
/// Go-safe identifiers (e.g. `.Values.auth.secretKeys`). `key` may
/// contain anything; the function picks the dot-walked form when
/// it's safe and the `(index ... "key")` form otherwise.
///
/// Examples:
///   - `safe_template_lookup(".Values.auth", "username")`
///     → `.Values.auth.username`
///   - `safe_template_lookup(".Values.auth", "bearer-tokens")`
///     → `(index .Values.auth "bearer-tokens")`
pub(super) fn safe_template_lookup(base: &str, key: &str) -> String {
    if is_go_identifier(key) {
        format!("{base}.{key}")
    } else {
        format!("(index {base} \"{key}\")")
    }
}

pub(super) fn write_file(path: impl AsRef<Path>, content: &str) -> Result<(), DeploymentError> {
    let path = path.as_ref();
    std::fs::write(path, content).map_err(|e| DeploymentError::WriteFile {
        path: path.display().to_string(),
        source: e,
    })
}
