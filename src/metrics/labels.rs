// Project:   hyperi-rustlib
// File:      src/metrics/labels.rs
// Purpose:   Bounded enum types for low-cardinality Prometheus labels
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Bounded label values for Prometheus / OTel metrics.
//!
//! Each enum represents a label whose value set is fixed at compile
//! time. Wrapping label arguments in these types stops free-form
//! strings from sliding into metric labels and blowing up TSDB
//! cardinality.
//!
//! Variant choice aligns with industry conventions:
//!
//! - [`TransportKind`] -- one variant per `transport-*` cargo
//!   feature; label values match OTel `messaging.system`.
//! - [`FlushTrigger`] -- standard buffer-flush triggers seen across
//!   Kafka producers and batch processors.
//! - [`AuthFailureReason`] -- RFC 6749 OAuth 2.0 + JWT-specific
//!   failure codes; label values are the RFC strings.
//! - [`ValidationFailureReason`] -- JSON Schema 2020-12 validator
//!   error categories.
//!
//! No `Other` catch-all: every code path that names a failure must
//! pick a variant. Add new variants here when a real new failure
//! mode appears; never widen at the call site.

/// Transport backend kind. One variant per `transport-*` cargo
/// feature plus `routed`. Aligns with OTel `messaging.system`
/// attribute values where they overlap (kafka, redis, http).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransportKind {
    Kafka,
    Grpc,
    Memory,
    File,
    Pipe,
    Http,
    Redis,
    Routed,
}

impl TransportKind {
    /// Snake-case label value suitable for Prometheus / OTel exporters.
    #[must_use]
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Kafka => "kafka",
            Self::Grpc => "grpc",
            Self::Memory => "memory",
            Self::File => "file",
            Self::Pipe => "pipe",
            Self::Http => "http",
            Self::Redis => "redis",
            Self::Routed => "routed",
        }
    }
}

impl std::fmt::Display for TransportKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_label())
    }
}

/// Buffer-flush trigger. Bounded set covering the patterns in every
/// batch processor I've seen.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlushTrigger {
    /// Byte threshold crossed.
    Size,
    /// Record-count threshold crossed.
    Records,
    /// Time / idle threshold crossed.
    Age,
    /// Cache eviction forced a flush.
    Eviction,
    /// Graceful drain at shutdown.
    Shutdown,
    /// Operator / test invoked.
    Manual,
}

impl FlushTrigger {
    #[must_use]
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Records => "records",
            Self::Age => "age",
            Self::Eviction => "eviction",
            Self::Shutdown => "shutdown",
            Self::Manual => "manual",
        }
    }
}

impl std::fmt::Display for FlushTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_label())
    }
}

/// Authentication failure reason. RFC 6749 / OAuth 2.0 error codes
/// plus the JWT-specific failure modes most apps observe.
///
/// Label values match the RFC strings exactly so OAuth-aware
/// dashboards and OTel collectors work without translation.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthFailureReason {
    /// Token TTL expired (JWT exp / session timeout).
    Expired,
    /// JWT / HMAC signature verification failed.
    InvalidSignature,
    /// RFC 6749 `invalid_client` — client_id or secret wrong.
    InvalidClient,
    /// RFC 6749 `invalid_grant` — grant / code / refresh wrong.
    InvalidGrant,
    /// RFC 6749 `invalid_scope` — requested scope rejected.
    InvalidScope,
    /// Token couldn't be parsed (structurally malformed).
    MalformedToken,
    /// Token explicitly revoked.
    RevokedToken,
    /// Too many attempts (operator-policy rate limit).
    RateLimited,
    /// RFC 6749 `unauthorized_client`.
    Unauthorized,
    /// RFC 6749 `access_denied`.
    AccessDenied,
}

impl AuthFailureReason {
    #[must_use]
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::InvalidSignature => "invalid_signature",
            Self::InvalidClient => "invalid_client",
            Self::InvalidGrant => "invalid_grant",
            Self::InvalidScope => "invalid_scope",
            Self::MalformedToken => "malformed_token",
            Self::RevokedToken => "revoked_token",
            Self::RateLimited => "rate_limited",
            Self::Unauthorized => "unauthorized",
            Self::AccessDenied => "access_denied",
        }
    }
}

impl std::fmt::Display for AuthFailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_label())
    }
}

/// Payload validation failure reason. Categories mirror the
/// JSON Schema 2020-12 validator error taxonomy (Ajv,
/// python-jsonschema, et al.).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValidationFailureReason {
    /// Combinator failure: `oneOf` / `anyOf` / `allOf` couldn't pick.
    SchemaInvalid,
    /// Required property absent (`required` keyword).
    FieldMissing,
    /// Wrong JSON type (`type` keyword).
    TypeMismatch,
    /// Numeric / length / array bound violated.
    OutOfRange,
    /// Regex `pattern` keyword failed.
    PatternMismatch,
    /// `format` keyword (email, date, uuid, ...).
    FormatInvalid,
    /// `enum` keyword: value not in allowed set.
    EnumViolation,
    /// Strict-mode unexpected property.
    AdditionalProperties,
    /// `null` where non-null required.
    NullValue,
    /// Bytes wouldn't decode (UTF-8 / base64 / hex).
    EncodingError,
}

impl ValidationFailureReason {
    #[must_use]
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::SchemaInvalid => "schema_invalid",
            Self::FieldMissing => "field_missing",
            Self::TypeMismatch => "type_mismatch",
            Self::OutOfRange => "out_of_range",
            Self::PatternMismatch => "pattern_mismatch",
            Self::FormatInvalid => "format_invalid",
            Self::EnumViolation => "enum_violation",
            Self::AdditionalProperties => "additional_properties",
            Self::NullValue => "null_value",
            Self::EncodingError => "encoding_error",
        }
    }
}

impl std::fmt::Display for ValidationFailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All label values are snake_case ASCII, no spaces, no upper.
    /// Cheap belt-and-braces check that the strings are stable
    /// Prometheus / OTel label material.
    #[test]
    fn label_values_are_snake_case_ascii() {
        let all: &[&str] = &[
            TransportKind::Kafka.as_label(),
            TransportKind::Grpc.as_label(),
            TransportKind::Memory.as_label(),
            TransportKind::File.as_label(),
            TransportKind::Pipe.as_label(),
            TransportKind::Http.as_label(),
            TransportKind::Redis.as_label(),
            TransportKind::Routed.as_label(),
            FlushTrigger::Size.as_label(),
            FlushTrigger::Records.as_label(),
            FlushTrigger::Age.as_label(),
            FlushTrigger::Eviction.as_label(),
            FlushTrigger::Shutdown.as_label(),
            FlushTrigger::Manual.as_label(),
            AuthFailureReason::Expired.as_label(),
            AuthFailureReason::InvalidSignature.as_label(),
            AuthFailureReason::InvalidClient.as_label(),
            AuthFailureReason::InvalidGrant.as_label(),
            AuthFailureReason::InvalidScope.as_label(),
            AuthFailureReason::MalformedToken.as_label(),
            AuthFailureReason::RevokedToken.as_label(),
            AuthFailureReason::RateLimited.as_label(),
            AuthFailureReason::Unauthorized.as_label(),
            AuthFailureReason::AccessDenied.as_label(),
            ValidationFailureReason::SchemaInvalid.as_label(),
            ValidationFailureReason::FieldMissing.as_label(),
            ValidationFailureReason::TypeMismatch.as_label(),
            ValidationFailureReason::OutOfRange.as_label(),
            ValidationFailureReason::PatternMismatch.as_label(),
            ValidationFailureReason::FormatInvalid.as_label(),
            ValidationFailureReason::EnumViolation.as_label(),
            ValidationFailureReason::AdditionalProperties.as_label(),
            ValidationFailureReason::NullValue.as_label(),
            ValidationFailureReason::EncodingError.as_label(),
        ];
        for s in all {
            assert!(
                s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'_' || b.is_ascii_digit()),
                "non-snake-case label: {s:?}"
            );
            assert!(!s.is_empty(), "empty label");
        }
    }
}
