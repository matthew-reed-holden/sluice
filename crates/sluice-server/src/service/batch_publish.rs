//! BatchPublish RPC handler implementation.
//!
//! Handles batch publish requests that atomically persist multiple messages.

use std::sync::Arc;
use std::time::Instant;
use tonic::{Request, Response, Status};

use crate::generate_message_id;
use crate::proto::sluice::v1::{
    BatchPublishRequest, BatchPublishResponse, PublishResult as ProtoPublishResult,
};
use crate::server::ServerState;
use crate::storage::writer::{BatchMessageInput, WriterError};

/// Maximum payload size per message (4MB, gRPC default limit).
const MAX_PAYLOAD_SIZE: usize = 4 * 1024 * 1024;

/// Maximum number of messages per batch.
const MAX_BATCH_SIZE: usize = 1000;

/// Handle a BatchPublish RPC request.
///
/// Persists all messages atomically in a single transaction.
#[tracing::instrument(skip(state, request), fields(topic, batch_size))]
pub async fn handle_batch_publish(
    state: &Arc<ServerState>,
    request: Request<BatchPublishRequest>,
) -> Result<Response<BatchPublishResponse>, Status> {
    let start = Instant::now();
    let req = request.into_inner();

    // Validate topic
    if req.topic.is_empty() {
        return Err(Status::invalid_argument("topic cannot be empty"));
    }

    if req.topic.len() > 255 {
        return Err(Status::invalid_argument(
            "topic name too long (max 255 characters)",
        ));
    }

    // Validate topic name characters (alphanumeric, dash, underscore, dot)
    if !req
        .topic
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(Status::invalid_argument(
            "topic name must contain only alphanumeric characters, dashes, underscores, or dots",
        ));
    }

    // Validate batch size
    if req.messages.is_empty() {
        return Err(Status::invalid_argument(
            "batch must contain at least one message",
        ));
    }

    if req.messages.len() > MAX_BATCH_SIZE {
        return Err(Status::invalid_argument(format!(
            "batch too large: {} messages (max {} messages)",
            req.messages.len(),
            MAX_BATCH_SIZE
        )));
    }

    tracing::Span::current().record("topic", &req.topic);
    tracing::Span::current().record("batch_size", req.messages.len());

    // Clone topic for metrics before moving to writer
    let topic_for_metrics = req.topic.clone();

    // Prepare messages for writer
    let mut messages = Vec::with_capacity(req.messages.len());
    for msg in req.messages {
        // Validate payload size
        if msg.payload.len() > MAX_PAYLOAD_SIZE {
            return Err(Status::resource_exhausted(format!(
                "payload too large: {} bytes (max {} bytes)",
                msg.payload.len(),
                MAX_PAYLOAD_SIZE
            )));
        }

        // Serialize attributes to JSON
        let attributes = if msg.attributes.is_empty() {
            None
        } else {
            Some(
                serde_json::to_string(&msg.attributes)
                    .map_err(|e| Status::invalid_argument(format!("invalid attributes: {e}")))?,
            )
        };

        // Generate message ID
        let message_id = generate_message_id();

        messages.push(BatchMessageInput {
            message_id,
            payload: if msg.payload.is_empty() {
                None
            } else {
                Some(msg.payload)
            },
            attributes,
        });
    }

    let batch_size = messages.len();

    // Submit to writer
    let (results, timestamp) = state
        .writer
        .batch_publish(req.topic, messages)
        .await
        .map_err(|e| match e {
            WriterError::ChannelClosed => Status::unavailable("server is shutting down"),
            WriterError::Database(msg) if msg.contains("disk") || msg.contains("full") => {
                Status::unavailable(format!("storage error: {msg}"))
            }
            WriterError::Database(msg) => Status::internal(format!("database error: {msg}")),
            WriterError::ThreadPanic => Status::internal("internal error"),
        })?;

    // Record metrics
    let latency = start.elapsed().as_secs_f64();

    tracing::debug!(
        topic = %topic_for_metrics,
        batch_size,
        latency_ms = latency * 1000.0,
        "Batch published"
    );

    // Convert results to proto format
    let proto_results: Vec<ProtoPublishResult> = results
        .into_iter()
        .map(|r| ProtoPublishResult {
            message_id: r.message_id,
            sequence: r.sequence as u64,
        })
        .collect();

    Ok(Response::new(BatchPublishResponse {
        results: proto_results,
        timestamp,
    }))
}
