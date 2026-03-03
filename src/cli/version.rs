// Project:   hyperi-rustlib
// File:      src/cli/version.rs
// Purpose:   Version information types
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Version information for DFE services.

use std::fmt;

/// Service version information.
///
/// Populated at build time via `env!()` macros or passed by the application.
#[derive(Debug, Clone)]
pub struct VersionInfo {
    /// Service name (e.g. "dfe-loader").
    pub name: String,
    /// Semantic version (e.g. "1.9.7").
    pub version: String,
    /// Git commit SHA (short).
    pub commit: Option<String>,
    /// Build date (RFC 3339).
    pub build_date: Option<String>,
    /// Rust compiler version.
    pub rustc_version: Option<String>,
    /// Target triple (e.g. "x86_64-unknown-linux-gnu").
    pub target: Option<String>,
    /// rustlib version.
    pub rustlib_version: String,
}

impl VersionInfo {
    /// Create with required fields, using rustlib version from this crate.
    #[must_use]
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            commit: None,
            build_date: None,
            rustc_version: None,
            target: None,
            rustlib_version: crate::VERSION.to_string(),
        }
    }

    /// Set git commit SHA.
    #[must_use]
    pub fn with_commit(mut self, commit: impl Into<String>) -> Self {
        self.commit = Some(commit.into());
        self
    }

    /// Set build date.
    #[must_use]
    pub fn with_build_date(mut self, date: impl Into<String>) -> Self {
        self.build_date = Some(date.into());
        self
    }

    /// Set Rust compiler version.
    #[must_use]
    pub fn with_rustc(mut self, version: impl Into<String>) -> Self {
        self.rustc_version = Some(version.into());
        self
    }

    /// Set target triple.
    #[must_use]
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Short version string: "name version (commit)".
    #[must_use]
    pub fn short(&self) -> String {
        match &self.commit {
            Some(c) => format!("{} {} ({})", self.name, self.version, c),
            None => format!("{} {}", self.name, self.version),
        }
    }
}

impl fmt::Display for VersionInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} {}", self.name, self.version)?;
        if let Some(ref c) = self.commit {
            writeln!(f, "  commit:  {c}")?;
        }
        if let Some(ref d) = self.build_date {
            writeln!(f, "  built:   {d}")?;
        }
        if let Some(ref r) = self.rustc_version {
            writeln!(f, "  rustc:   {r}")?;
        }
        if let Some(ref t) = self.target {
            writeln!(f, "  target:  {t}")?;
        }
        write!(f, "  rustlib: {}", self.rustlib_version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_info_new() {
        let v = VersionInfo::new("dfe-loader", "1.9.7");
        assert_eq!(v.name, "dfe-loader");
        assert_eq!(v.version, "1.9.7");
        assert!(v.commit.is_none());
        assert_eq!(v.rustlib_version, crate::VERSION);
    }

    #[test]
    fn test_version_info_builder() {
        let v = VersionInfo::new("dfe-loader", "1.9.7")
            .with_commit("abc1234")
            .with_build_date("2026-03-03")
            .with_rustc("1.85.0")
            .with_target("x86_64-unknown-linux-gnu");

        assert_eq!(v.commit.as_deref(), Some("abc1234"));
        assert_eq!(v.build_date.as_deref(), Some("2026-03-03"));
        assert_eq!(v.rustc_version.as_deref(), Some("1.85.0"));
        assert_eq!(v.target.as_deref(), Some("x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn test_version_info_short() {
        let v = VersionInfo::new("dfe-loader", "1.9.7").with_commit("abc1234");
        assert_eq!(v.short(), "dfe-loader 1.9.7 (abc1234)");

        let v2 = VersionInfo::new("dfe-loader", "1.9.7");
        assert_eq!(v2.short(), "dfe-loader 1.9.7");
    }

    #[test]
    fn test_version_info_display() {
        let v = VersionInfo::new("dfe-loader", "1.9.7")
            .with_commit("abc1234")
            .with_target("x86_64-unknown-linux-gnu");

        let output = v.to_string();
        assert!(output.contains("dfe-loader 1.9.7"));
        assert!(output.contains("commit:  abc1234"));
        assert!(output.contains("target:  x86_64-unknown-linux-gnu"));
        assert!(output.contains(&format!("rustlib: {}", crate::VERSION)));
    }
}
