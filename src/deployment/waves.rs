// Project:   hyperi-rustlib
// File:      src/deployment/waves.rs
// Purpose:   Shared ArgoCD sync-wave constants
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! ArgoCD sync-wave constants.
//!
//! Convention for the order in which ArgoCD applies resources during a
//! sync. Lower waves run first. Used as the `argocd.argoproj.io/sync-wave`
//! annotation on Application resources and as the default for
//! [`crate::deployment::ArgocdConfig::sync_wave`].
//!
//! The numeric values are gaps wide enough that consumer projects can
//! slot custom waves (e.g. `-15` for "between operators and CRDs", `-3`
//! for "before topics but after CRDs"). Stick to the canonical bands
//! where possible — operators install order is genuinely
//! dependency-driven.

/// Operators that must install before everything else (e.g.
/// Strimzi Kafka Operator, External Secrets Operator).
/// Their CRDs are prerequisites for later waves.
pub const WAVE_OPERATORS: i32 = -20;

/// Custom Resource Definitions that other resources depend on.
/// Runs after operators (which often install their own CRDs).
pub const WAVE_CRDS: i32 = -10;

/// Cross-application Kafka topology: `KafkaTopic`, `KafkaUser`,
/// and similar CRs that DFE apps consume.
pub const WAVE_TOPICS: i32 = -5;

/// DFE apps themselves (loader, receiver, archiver, ...).
/// The default for any Application without an explicit sync wave.
pub const WAVE_APPS: i32 = 0;

/// Post-deployment work: smoke tests, notification webhooks,
/// observability registrations.
pub const WAVE_POST: i32 = 10;

// Compile-time invariants for the wave constants. Each leaves room for
// consumer-specific slots between the canonical waves (e.g. -15 between
// OPERATORS and CRDS). WAVE_APPS=0 is the documented default for plain
// Application resources without a more specific wave.
const _: () = assert!(WAVE_OPERATORS < WAVE_CRDS);
const _: () = assert!(WAVE_CRDS < WAVE_TOPICS);
const _: () = assert!(WAVE_TOPICS < WAVE_APPS);
const _: () = assert!(WAVE_APPS < WAVE_POST);
const _: () = assert!(WAVE_APPS == 0);
const _: () = assert!(WAVE_CRDS - WAVE_OPERATORS >= 5);
const _: () = assert!(WAVE_TOPICS - WAVE_CRDS >= 5);
const _: () = assert!(WAVE_APPS - WAVE_TOPICS >= 5);
const _: () = assert!(WAVE_POST - WAVE_APPS >= 5);
