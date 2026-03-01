// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! Vector-compatible gRPC sink.
//!
//! Sends events to a Vector source via the `vector.Vector/PushEvents` RPC.
//! Use this to push DFE events to a downstream Vector pipeline.

use super::convert::json_to_event_wrapper;
use super::proto::vector;
use crate::transport::error::{TransportError, TransportResult};

/// Client that sends events to a Vector source via gRPC.
///
/// Converts JSON values to Vector's protobuf `EventWrapper` format
/// and sends them via `PushEvents`.
pub struct VectorCompatClient {
    client: vector::vector_client::VectorClient<tonic::transport::Channel>,
}

impl VectorCompatClient {
    /// Connect to a Vector source endpoint.
    ///
    /// Uses lazy connection — the actual TCP connection is established
    /// on the first RPC call.
    ///
    /// # Errors
    ///
    /// Returns error if the endpoint URI is invalid.
    pub fn connect_lazy(endpoint: &str) -> TransportResult<Self> {
        let channel = tonic::transport::Channel::from_shared(endpoint.to_string())
            .map_err(|e| TransportError::Config(format!("invalid Vector endpoint: {e}")))?
            .connect_lazy();

        let client = vector::vector_client::VectorClient::new(channel)
            .max_decoding_message_size(usize::MAX)
            .accept_compressed(tonic::codec::CompressionEncoding::Gzip)
            .send_compressed(tonic::codec::CompressionEncoding::Gzip);

        Ok(Self { client })
    }

    /// Send JSON values as Vector log events.
    ///
    /// Each JSON value is wrapped as a Vector `Log` event inside an `EventWrapper`.
    ///
    /// # Errors
    ///
    /// Returns error if the gRPC call fails.
    pub async fn send_events(&self, values: &[serde_json::Value]) -> TransportResult<()> {
        let events: Vec<_> = values.iter().map(json_to_event_wrapper).collect();

        let request = vector::PushEventsRequest { events };

        self.client
            .clone()
            .push_events(request)
            .await
            .map_err(|e| TransportError::Send(format!("Vector PushEvents failed: {e}")))?;

        Ok(())
    }

    /// Check if the remote Vector source is healthy.
    ///
    /// # Errors
    ///
    /// Returns error if the health check RPC fails.
    pub async fn health_check(&self) -> TransportResult<bool> {
        let response = self
            .client
            .clone()
            .health_check(vector::HealthCheckRequest {})
            .await
            .map_err(|e| TransportError::Connection(format!("Vector health check failed: {e}")))?;

        Ok(response.into_inner().status == vector::ServingStatus::Serving as i32)
    }
}
