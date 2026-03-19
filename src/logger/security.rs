// Project:   hyperi-rustlib
// File:      src/logger/security.rs
// Purpose:   Structured security event logging following OWASP Logging Vocabulary
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Structured security event logging following OWASP Logging Vocabulary.
//!
//! All security events are emitted with `target: "security"` so operators can
//! route them separately via `RUST_LOG=security=info` or a dedicated tracing
//! `Layer` with per-layer filtering.
//!
//! ## Example
//!
//! ```rust
//! use hyperi_rustlib::logger::security::{SecurityEvent, SecurityOutcome, auth_failure};
//! use std::net::{IpAddr, Ipv4Addr};
//!
//! // Builder pattern for full control
//! SecurityEvent::new("auth.failure", "bearer_validate", SecurityOutcome::Failure)
//!     .actor("svc-collector")
//!     .source_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))
//!     .reason("expired_token")
//!     .emit();
//!
//! // Convenience function for common cases
//! auth_failure("bearer_validate", "expired_token", Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
//! ```

use std::net::IpAddr;

/// Outcome of a security event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityOutcome {
    /// Operation succeeded.
    Success,
    /// Operation failed (e.g. bad credentials).
    Failure,
    /// Access was denied (authorisation).
    Denied,
    /// Internal error during security operation.
    Error,
}

impl SecurityOutcome {
    /// Return the outcome as a static string for structured logging.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Denied => "denied",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for SecurityOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Standard security event types following OWASP Logging Vocabulary.
///
/// See: <https://cheatsheetseries.owasp.org/cheatsheets/Logging_Vocabulary_Cheat_Sheet.html>
///
/// Uses a builder pattern with mandatory fields in [`SecurityEvent::new`] and
/// optional context via chained methods. Call [`SecurityEvent::emit`] to write
/// the event to the `"security"` tracing target.
pub struct SecurityEvent<'a> {
    /// Event type (e.g. "auth.login", "access.denied", "config.changed").
    event_type: &'a str,
    /// Specific action (e.g. "bearer_validate", "tls_handshake").
    action: &'a str,
    /// Whether the action succeeded, failed, was denied, or errored.
    outcome: SecurityOutcome,
    /// User or service identity (if known).
    actor: Option<&'a str>,
    /// Source IP address of the request.
    source_ip: Option<IpAddr>,
    /// Resource that was accessed or modified.
    resource: Option<&'a str>,
    /// Reason for failure or denial.
    reason: Option<&'a str>,
    /// Additional context.
    detail: Option<&'a str>,
}

impl<'a> SecurityEvent<'a> {
    /// Create a new security event with the required fields.
    #[must_use]
    pub fn new(event_type: &'a str, action: &'a str, outcome: SecurityOutcome) -> Self {
        Self {
            event_type,
            action,
            outcome,
            actor: None,
            source_ip: None,
            resource: None,
            reason: None,
            detail: None,
        }
    }

    /// Set the actor (user or service identity).
    #[must_use]
    pub fn actor(mut self, actor: &'a str) -> Self {
        self.actor = Some(actor);
        self
    }

    /// Set the source IP address.
    #[must_use]
    pub fn source_ip(mut self, ip: IpAddr) -> Self {
        self.source_ip = Some(ip);
        self
    }

    /// Set the resource that was accessed or modified.
    #[must_use]
    pub fn resource(mut self, resource: &'a str) -> Self {
        self.resource = Some(resource);
        self
    }

    /// Set the reason for failure or denial.
    #[must_use]
    pub fn reason(mut self, reason: &'a str) -> Self {
        self.reason = Some(reason);
        self
    }

    /// Set additional context detail.
    #[must_use]
    pub fn detail(mut self, detail: &'a str) -> Self {
        self.detail = Some(detail);
        self
    }

    /// Emit the security event via tracing.
    ///
    /// Uses `target: "security"` so operators can route security events
    /// separately via `RUST_LOG=security=info` or a dedicated `Layer` with
    /// per-layer filtering.
    ///
    /// Level mapping:
    /// - `Success` → `info!`
    /// - `Failure` / `Denied` → `warn!`
    /// - `Error` → `error!`
    pub fn emit(&self) {
        let source_ip_str = self.source_ip.map(|ip| ip.to_string());
        let source_ip_ref = source_ip_str.as_deref().unwrap_or("-");

        match self.outcome {
            SecurityOutcome::Success => {
                tracing::info!(
                    target: "security",
                    event_type = self.event_type,
                    action = self.action,
                    outcome = self.outcome.as_str(),
                    actor = self.actor.unwrap_or("-"),
                    source_ip = source_ip_ref,
                    resource = self.resource.unwrap_or("-"),
                    "security event"
                );
            }
            SecurityOutcome::Failure | SecurityOutcome::Denied => {
                tracing::warn!(
                    target: "security",
                    event_type = self.event_type,
                    action = self.action,
                    outcome = self.outcome.as_str(),
                    actor = self.actor.unwrap_or("-"),
                    source_ip = source_ip_ref,
                    resource = self.resource.unwrap_or("-"),
                    reason = self.reason.unwrap_or("-"),
                    "security event"
                );
            }
            SecurityOutcome::Error => {
                tracing::error!(
                    target: "security",
                    event_type = self.event_type,
                    action = self.action,
                    outcome = self.outcome.as_str(),
                    actor = self.actor.unwrap_or("-"),
                    source_ip = source_ip_ref,
                    resource = self.resource.unwrap_or("-"),
                    reason = self.reason.unwrap_or("-"),
                    detail = self.detail.unwrap_or("-"),
                    "security event"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience functions for common event types
// ---------------------------------------------------------------------------

/// Log an authentication success.
pub fn auth_success(action: &str, actor: &str, source_ip: Option<IpAddr>) {
    let mut event =
        SecurityEvent::new("auth.success", action, SecurityOutcome::Success).actor(actor);
    if let Some(ip) = source_ip {
        event = event.source_ip(ip);
    }
    event.emit();
}

/// Log an authentication failure.
pub fn auth_failure(action: &str, reason: &str, source_ip: Option<IpAddr>) {
    let mut event =
        SecurityEvent::new("auth.failure", action, SecurityOutcome::Failure).reason(reason);
    if let Some(ip) = source_ip {
        event = event.source_ip(ip);
    }
    event.emit();
}

/// Log an access denial.
pub fn access_denied(action: &str, actor: &str, resource: &str, source_ip: Option<IpAddr>) {
    let mut event = SecurityEvent::new("access.denied", action, SecurityOutcome::Denied)
        .actor(actor)
        .resource(resource);
    if let Some(ip) = source_ip {
        event = event.source_ip(ip);
    }
    event.emit();
}

/// Log a configuration change.
pub fn config_changed(action: &str, actor: &str, detail: &str) {
    SecurityEvent::new("config.changed", action, SecurityOutcome::Success)
        .actor(actor)
        .detail(detail)
        .emit();
}

/// Log a TLS/certificate event.
pub fn tls_event(
    action: &str,
    outcome: SecurityOutcome,
    reason: Option<&str>,
    source_ip: Option<IpAddr>,
) {
    let mut event = SecurityEvent::new("tls.event", action, outcome);
    if let Some(r) = reason {
        event = event.reason(r);
    }
    if let Some(ip) = source_ip {
        event = event.source_ip(ip);
    }
    event.emit();
}

/// Log a rate limit trigger.
pub fn rate_limit_triggered(actor: &str, resource: &str, source_ip: Option<IpAddr>) {
    let mut event = SecurityEvent::new(
        "rate_limit.triggered",
        "rate_limit",
        SecurityOutcome::Denied,
    )
    .actor(actor)
    .resource(resource);
    if let Some(ip) = source_ip {
        event = event.source_ip(ip);
    }
    event.emit();
}

/// Log a token rotation event.
pub fn token_rotated(action: &str, detail: &str) {
    SecurityEvent::new("token.rotated", action, SecurityOutcome::Success)
        .detail(detail)
        .emit();
}

/// Log an input validation failure (potential attack indicator per OWASP).
pub fn input_validation_failure(action: &str, reason: &str, source_ip: Option<IpAddr>) {
    let mut event =
        SecurityEvent::new("input.validation_failure", action, SecurityOutcome::Failure)
            .reason(reason);
    if let Some(ip) = source_ip {
        event = event.source_ip(ip);
    }
    event.emit();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_security_event_builder() {
        let event = SecurityEvent::new("auth.failure", "bearer_validate", SecurityOutcome::Failure)
            .actor("user@example.com")
            .source_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)))
            .resource("/api/data")
            .reason("invalid_token");
        // Should not panic
        event.emit();
    }

    #[test]
    fn test_convenience_functions() {
        let ip = Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        auth_success("bearer_validate", "admin", ip);
        auth_failure("bearer_validate", "expired_token", ip);
        access_denied("read", "guest", "/admin", ip);
        config_changed("reload", "system", "auth config updated");
        tls_event(
            "handshake",
            SecurityOutcome::Failure,
            Some("cert_expired"),
            ip,
        );
        rate_limit_triggered("client_abc", "/api/ingest", ip);
        token_rotated("bearer_refresh", "3 tokens loaded from vault");
        input_validation_failure("json_parse", "invalid_json", ip);
    }

    #[test]
    fn test_outcome_as_str() {
        assert_eq!(SecurityOutcome::Success.as_str(), "success");
        assert_eq!(SecurityOutcome::Failure.as_str(), "failure");
        assert_eq!(SecurityOutcome::Denied.as_str(), "denied");
        assert_eq!(SecurityOutcome::Error.as_str(), "error");
    }

    #[test]
    fn test_outcome_display() {
        assert_eq!(format!("{}", SecurityOutcome::Success), "success");
        assert_eq!(format!("{}", SecurityOutcome::Error), "error");
    }

    #[test]
    fn test_minimal_event() {
        // Only required fields, no optionals
        SecurityEvent::new("test.event", "test_action", SecurityOutcome::Success).emit();
    }

    #[test]
    fn test_error_outcome_event() {
        SecurityEvent::new("auth.error", "token_validate", SecurityOutcome::Error)
            .reason("backend_unavailable")
            .detail("vault connection timed out after 5s")
            .source_ip(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)))
            .emit();
    }

    #[test]
    fn test_ipv6_source() {
        let ipv6: IpAddr = "::1".parse().unwrap();
        SecurityEvent::new("auth.success", "login", SecurityOutcome::Success)
            .source_ip(ipv6)
            .actor("admin")
            .emit();
    }

    #[test]
    fn test_no_source_ip() {
        auth_success("api_key", "svc-internal", None);
        auth_failure("bearer_validate", "malformed", None);
    }
}
