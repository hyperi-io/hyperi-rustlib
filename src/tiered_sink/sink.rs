// Project:   hs-rustlib
// File:      src/tiered_sink/sink.rs
// Purpose:   Sink trait for async message delivery
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Sink trait for async message delivery.

use async_trait::async_trait;
use std::error::Error as StdError;

/// A sink that can receive messages asynchronously.
///
/// Implement this trait for your message backend (Kafka, S3, HTTP, etc.)
/// to use with `TieredSink`.
///
/// # Example
///
/// ```rust,ignore
/// use hs_rustlib::tiered_sink::{Sink, SinkError};
///
/// struct MyKafkaSink {
///     producer: KafkaProducer,
/// }
///
/// #[async_trait::async_trait]
/// impl Sink for MyKafkaSink {
///     type Error = KafkaError;
///
///     async fn try_send(&self, data: &[u8]) -> Result<(), SinkError<Self::Error>> {
///         match self.producer.send(data).await {
///             Ok(()) => Ok(()),
///             Err(e) if e.is_queue_full() => Err(SinkError::Full),
///             Err(e) if e.is_broker_unavailable() => Err(SinkError::Unavailable),
///             Err(e) => Err(SinkError::Fatal(e)),
///         }
///     }
/// }
/// ```
#[async_trait]
pub trait Sink: Send + Sync + 'static {
    /// The error type returned by this sink.
    type Error: StdError + Send + Sync + 'static;

    /// Try to send data to the sink.
    ///
    /// This should be non-blocking or have a short timeout.
    /// Return appropriate `SinkError` variant based on the failure mode:
    ///
    /// - `SinkError::Full` - Sink is backpressuring, try again later
    /// - `SinkError::Unavailable` - Sink is down, circuit break
    /// - `SinkError::Fatal(e)` - Unrecoverable error, don't spool
    async fn try_send(&self, data: &[u8]) -> Result<(), SinkError<Self::Error>>;

    /// Check if the sink is healthy.
    ///
    /// Used by circuit breaker to probe if sink has recovered.
    /// Default implementation returns Ok (assumes healthy).
    async fn health_check(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Error returned by `Sink::try_send`.
///
/// The error variant determines how `TieredSink` handles the failure:
/// - `Full` and `Unavailable` → spool to disk
/// - `Fatal` → propagate error to caller, don't spool
#[derive(Debug)]
pub enum SinkError<E> {
    /// Sink is backpressuring (queue full, rate limited).
    /// Message should be spooled and retried later.
    Full,

    /// Sink is unavailable (connection failed, broker down).
    /// Message should be spooled, circuit breaker should open.
    Unavailable,

    /// Fatal error - do not retry, do not spool.
    /// Examples: invalid message format, authentication failure.
    Fatal(E),
}

impl<E: StdError> std::fmt::Display for SinkError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "sink is full"),
            Self::Unavailable => write!(f, "sink is unavailable"),
            Self::Fatal(e) => write!(f, "fatal sink error: {e}"),
        }
    }
}

impl<E: StdError + 'static> StdError for SinkError<E> {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Fatal(e) => Some(e),
            _ => None,
        }
    }
}

impl<E> SinkError<E> {
    /// Returns true if this is a retryable error (Full or Unavailable).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Full | Self::Unavailable)
    }

    /// Returns true if this is a fatal (non-retryable) error.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Fatal(_))
    }

    /// Returns true if this error should trigger circuit breaker.
    #[must_use]
    pub fn should_circuit_break(&self) -> bool {
        matches!(self, Self::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Debug)]
    struct TestError(String);

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl StdError for TestError {}

    struct CountingSink {
        count: Arc<AtomicUsize>,
        fail_after: Option<usize>,
    }

    #[async_trait]
    impl Sink for CountingSink {
        type Error = TestError;

        async fn try_send(&self, _data: &[u8]) -> Result<(), SinkError<Self::Error>> {
            let n = self.count.fetch_add(1, Ordering::SeqCst);
            if let Some(fail_after) = self.fail_after {
                if n >= fail_after {
                    return Err(SinkError::Unavailable);
                }
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_sink_success() {
        let count = Arc::new(AtomicUsize::new(0));
        let sink = CountingSink {
            count: Arc::clone(&count),
            fail_after: None,
        };

        sink.try_send(b"test").await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_sink_unavailable() {
        let sink = CountingSink {
            count: Arc::new(AtomicUsize::new(0)),
            fail_after: Some(0),
        };

        let result = sink.try_send(b"test").await;
        assert!(matches!(result, Err(SinkError::Unavailable)));
    }

    #[test]
    fn test_sink_error_properties() {
        let full: SinkError<TestError> = SinkError::Full;
        assert!(full.is_retryable());
        assert!(!full.is_fatal());
        assert!(!full.should_circuit_break());

        let unavailable: SinkError<TestError> = SinkError::Unavailable;
        assert!(unavailable.is_retryable());
        assert!(!unavailable.is_fatal());
        assert!(unavailable.should_circuit_break());

        let fatal: SinkError<TestError> = SinkError::Fatal(TestError("oops".into()));
        assert!(!fatal.is_retryable());
        assert!(fatal.is_fatal());
        assert!(!fatal.should_circuit_break());
    }
}
