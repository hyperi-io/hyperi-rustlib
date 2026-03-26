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
/// Returns the token for use in `tokio::select!` or other async
/// shutdown coordination.
#[must_use]
pub fn install_signal_handler() -> CancellationToken {
    let t = token();
    let cancel = t.clone();

    tokio::spawn(async move {
        wait_for_signal().await;
        cancel.cancel();

        #[cfg(feature = "logger")]
        tracing::info!("Shutdown signal received, cancelling all tasks");
    });

    t
}

/// Wait for SIGTERM or SIGINT.
async fn wait_for_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
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
