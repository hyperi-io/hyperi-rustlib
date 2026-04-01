// Project:   hyperi-rustlib
// File:      src/shutdown.rs
// Purpose:   Unified graceful shutdown with global CancellationToken
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Unified graceful shutdown manager.
//!
//! Provides a global [`CancellationToken`] that all modules can listen on
//! for coordinated graceful shutdown. One place handles SIGTERM/SIGINT,
//! all modules drain gracefully.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use hyperi_rustlib::shutdown;
//!
//! #[tokio::main]
//! async fn main() {
//!     // Install the signal handler once at startup
//!     let token = shutdown::install_signal_handler();
//!
//!     // Pass token to workers, or they can call shutdown::token() directly
//!     tokio::spawn(async move {
//!         loop {
//!             tokio::select! {
//!                 _ = token.cancelled() => {
//!                     // drain and exit
//!                     break;
//!                 }
//!                 _ = do_work() => {}
//!             }
//!         }
//!     });
//! }
//!
//! async fn do_work() {
//!     tokio::time::sleep(std::time::Duration::from_secs(1)).await;
//! }
//! ```

use std::sync::OnceLock;
use tokio_util::sync::CancellationToken;

static TOKEN: OnceLock<CancellationToken> = OnceLock::new();

/// Get the global shutdown token.
///
/// All modules should clone this token and listen for cancellation
/// in their main loops via `token.cancelled().await`.
///
/// The token is created lazily on first access.
pub fn token() -> CancellationToken {
    TOKEN.get_or_init(CancellationToken::new).clone()
}

/// Check if shutdown has been requested.
pub fn is_shutdown() -> bool {
    TOKEN.get().is_some_and(CancellationToken::is_cancelled)
}

/// Trigger shutdown programmatically.
///
/// Cancels the global token. All modules listening on it will
/// begin their drain/cleanup sequence.
pub fn trigger() {
    if let Some(t) = TOKEN.get() {
        t.cancel();
    }
}

/// Wait for SIGTERM or SIGINT, then trigger shutdown.
///
/// Call this once at application startup. It spawns a background
/// task that waits for the OS signal, then cancels the global token.
///
/// **K8s pre-stop compliance:** When running in Kubernetes (detected via
/// [`crate::env::runtime_context`]), sleeps for `PRESTOP_DELAY_SECS`
/// (default 5) before cancelling the token. This gives K8s time to
/// remove the pod from Service endpoints before the app starts draining.
/// On bare metal / Docker, the delay is 0 (immediate shutdown).
///
/// Returns the token for use in `tokio::select!` or other async
/// shutdown coordination.
#[must_use]
pub fn install_signal_handler() -> CancellationToken {
    let t = token();
    let cancel = t.clone();

    tokio::spawn(async move {
        wait_for_signal().await;

        // K8s pre-stop: delay before draining to allow endpoint removal.
        // Without this, K8s routes traffic to a pod that's already shutting down.
        let prestop_delay = prestop_delay_secs();
        if prestop_delay > 0 {
            #[cfg(feature = "logger")]
            tracing::info!(
                delay_secs = prestop_delay,
                "Pre-stop delay: waiting for K8s endpoint removal"
            );
            tokio::time::sleep(std::time::Duration::from_secs(prestop_delay)).await;
        }

        cancel.cancel();

        #[cfg(feature = "logger")]
        tracing::info!("Shutdown signal received, cancelling all tasks");
    });

    t
}

/// Determine the pre-stop delay in seconds.
///
/// - `PRESTOP_DELAY_SECS` env var overrides (for tuning in deployment manifests)
/// - K8s detected: default 5 seconds
/// - Bare metal / Docker: default 0 (no delay)
fn prestop_delay_secs() -> u64 {
    if let Ok(val) = std::env::var("PRESTOP_DELAY_SECS")
        && let Ok(secs) = val.parse::<u64>()
    {
        return secs;
    }
    if crate::env::runtime_context().is_kubernetes() {
        5
    } else {
        0
    }
}

/// Wait for SIGTERM or SIGINT.
async fn wait_for_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %e, "Failed to install Ctrl+C handler");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "Failed to install SIGTERM handler, only Ctrl+C will trigger shutdown"
                );
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(unix)]
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    #[cfg(not(unix))]
    ctrl_c.await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_not_cancelled_initially() {
        // Use a fresh token (not the global) to avoid test pollution
        let t = CancellationToken::new();
        assert!(!t.is_cancelled());
    }

    #[test]
    fn trigger_cancels_token() {
        let t = CancellationToken::new();
        assert!(!t.is_cancelled());
        t.cancel();
        assert!(t.is_cancelled());
    }

    #[test]
    fn token_is_cloneable_and_shared() {
        let t = CancellationToken::new();
        let c1 = t.clone();
        let c2 = t.clone();

        assert!(!c1.is_cancelled());
        assert!(!c2.is_cancelled());

        t.cancel();

        assert!(c1.is_cancelled());
        assert!(c2.is_cancelled());
    }

    #[test]
    fn multiple_triggers_are_idempotent() {
        let t = CancellationToken::new();
        t.cancel();
        t.cancel(); // second cancel should not panic
        assert!(t.is_cancelled());
    }

    #[tokio::test]
    async fn cancelled_future_resolves_after_cancel() {
        let t = CancellationToken::new();
        let c = t.clone();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            c.cancel();
        });

        // This should resolve once the token is cancelled
        t.cancelled().await;
        assert!(t.is_cancelled());
    }

    #[tokio::test]
    async fn child_token_cancelled_by_parent() {
        let parent = CancellationToken::new();
        let child = parent.child_token();

        assert!(!child.is_cancelled());
        parent.cancel();
        assert!(child.is_cancelled());
    }
}
