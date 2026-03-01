// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! Vector-compatible gRPC source.
//!
//! Implements the `vector.Vector` gRPC service so that legacy Vector sinks
//! can push events to a DFE service. Incoming `EventWrapper` messages are
//! converted to JSON and fed into the same receive channel as native DFE traffic.

use super::convert::event_wrapper_to_json;
use super::proto::vector;
use crate::transport::grpc::GrpcToken;
use crate::transport::types::{Message, PayloadFormat};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tonic::{Request, Response, Status};

/// gRPC service that accepts `PushEvents` RPCs from Vector sinks.
///
/// Converts Vector's protobuf events to JSON and forwards them into
/// the transport's receive channel alongside native DFE messages.
pub struct VectorCompatService {
    sender: mpsc::Sender<Message<GrpcToken>>,
    sequence: Arc<AtomicU64>,
}

impl VectorCompatService {
    /// Create a new Vector compat service.
    ///
    /// Uses the same sender/sequence as the DFE transport server so
    /// both native and Vector-compat events arrive in the same channel.
    pub fn new(sender: mpsc::Sender<Message<GrpcToken>>, sequence: Arc<AtomicU64>) -> Self {
        Self { sender, sequence }
    }
}

#[tonic::async_trait]
impl vector::vector_server::Vector for VectorCompatService {
    async fn push_events(
        &self,
        request: Request<vector::PushEventsRequest>,
    ) -> Result<Response<vector::PushEventsResponse>, Status> {
        let req = request.into_inner();

        for event_wrapper in &req.events {
            // Convert Vector event to JSON (skip metrics)
            let Some(json_value) = event_wrapper_to_json(event_wrapper) else {
                continue;
            };

            let payload = serde_json::to_vec(&json_value)
                .map_err(|e| Status::internal(format!("json serialise failed: {e}")))?;

            let seq = self.sequence.fetch_add(1, Ordering::Relaxed);

            let msg = Message {
                key: None, // Vector events don't carry a topic key
                payload,
                token: GrpcToken::new(seq),
                timestamp_ms: None,
                format: PayloadFormat::Json,
            };

            self.sender
                .send(msg)
                .await
                .map_err(|_| Status::unavailable("receiver buffer full"))?;
        }

        Ok(Response::new(vector::PushEventsResponse {}))
    }

    async fn health_check(
        &self,
        _request: Request<vector::HealthCheckRequest>,
    ) -> Result<Response<vector::HealthCheckResponse>, Status> {
        Ok(Response::new(vector::HealthCheckResponse {
            status: vector::ServingStatus::Serving.into(),
        }))
    }
}
