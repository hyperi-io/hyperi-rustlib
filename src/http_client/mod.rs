// Project:   hyperi-rustlib
// File:      src/http_client/mod.rs
// Purpose:   Production HTTP client with retry middleware
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Production HTTP client with automatic retries and timeouts.
//!
//! Wraps [`reqwest`] with [`reqwest_middleware`] and [`reqwest_retry`] to
//! provide exponential backoff for transient errors (5xx, timeouts,
//! connection failures). Non-retryable errors (4xx) return immediately.
//!
//! # Config Cascade
//!
//! When the `config` feature is enabled, config is auto-loaded from the
//! cascade under the `http_client` key:
//!
//! ```yaml
//! http_client:
//!   timeout_secs: 30
//!   connect_timeout_secs: 10
//!   max_retries: 3
//!   min_retry_interval_ms: 100
//!   max_retry_interval_ms: 30000
//!   user_agent: "dfe-fetcher/1.0"
//! ```

pub mod config;

pub use config::HttpClientConfig;

use reqwest::Response;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::RetryTransientMiddleware;
use reqwest_retry::policies::ExponentialBackoff;

/// Production HTTP client with retry middleware.
pub struct HttpClient {
    inner: ClientWithMiddleware,
    config: HttpClientConfig,
}

impl HttpClient {
    /// Create a new HTTP client with the given config.
    #[must_use]
    pub fn new(config: HttpClientConfig) -> Self {
        let retry_policy = ExponentialBackoff::builder()
            .retry_bounds(
                std::time::Duration::from_millis(config.min_retry_interval_ms),
                std::time::Duration::from_millis(config.max_retry_interval_ms),
            )
            .build_with_max_retries(config.max_retries);

        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .connect_timeout(std::time::Duration::from_secs(config.connect_timeout_secs));

        if let Some(ref ua) = config.user_agent {
            builder = builder.user_agent(ua.clone());
        }

        let reqwest_client = builder.build().expect("failed to build reqwest client");

        let client = ClientBuilder::new(reqwest_client)
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();

        Self {
            inner: client,
            config,
        }
    }

    /// Create a client from the config cascade (or defaults).
    #[must_use]
    pub fn from_cascade() -> Self {
        Self::new(HttpClientConfig::from_cascade())
    }

    /// Send a GET request.
    pub async fn get(&self, url: &str) -> Result<Response, reqwest_middleware::Error> {
        self.inner.get(url).send().await
    }

    /// Send a POST request with a JSON body.
    pub async fn post_json<T: serde::Serialize + ?Sized>(
        &self,
        url: &str,
        body: &T,
    ) -> Result<Response, reqwest_middleware::Error> {
        self.inner
            .post(url)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(body).unwrap_or_default())
            .send()
            .await
    }

    /// Send a PUT request with a JSON body.
    pub async fn put_json<T: serde::Serialize + ?Sized>(
        &self,
        url: &str,
        body: &T,
    ) -> Result<Response, reqwest_middleware::Error> {
        self.inner
            .put(url)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(body).unwrap_or_default())
            .send()
            .await
    }

    /// Send a DELETE request.
    pub async fn delete(&self, url: &str) -> Result<Response, reqwest_middleware::Error> {
        self.inner.delete(url).send().await
    }

    /// Access the underlying middleware client for custom requests.
    #[must_use]
    pub fn client(&self) -> &ClientWithMiddleware {
        &self.inner
    }

    /// Access the current config.
    #[must_use]
    pub fn config(&self) -> &HttpClientConfig {
        &self.config
    }
}
