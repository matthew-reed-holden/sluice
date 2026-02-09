//! Subscribe RPC handler implementation.
//!
//! Handles bidirectional streaming for message consumption with credit-based flow control.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};

use crate::flow::credit::CreditBalance;
use crate::generate_message_id;
use crate::observability::metrics::{record_backpressure, record_subscription_lag};
use crate::proto::sluice::v1::subscribe_downstream::Response as DownstreamResponse;
use crate::proto::sluice::v1::subscribe_upstream::Request as UpstreamRequest;
use crate::proto::sluice::v1::{
    Heartbeat, InitialPosition, MessageDelivery, SubscribeDownstream, SubscribeUpstream,
    SubscriptionMode,
};
use crate::server::ServerState;
use crate::service::ConsumerGroupKey;
use crate::storage::schema::{
    fetch_messages_from_seq, get_message_seq_by_id, get_topic_by_name, get_topic_max_seq,
};

type SubscribeStream =
    Pin<Box<dyn Stream<Item = Result<SubscribeDownstream, Status>> + Send + 'static>>;

/// Handle a Subscribe RPC request.
///
/// Establishes a bidirectional stream for message consumption.
#[tracing::instrument(skip(state, request))]
pub async fn handle_subscribe(
    state: &Arc<ServerState>,
    request: Request<Streaming<SubscribeUpstream>>,
) -> Result<Response<SubscribeStream>, Status> {
    let mut inbound = request.into_inner();

    // Wait for SubscriptionInit as first message
    let init = match inbound.message().await? {
        Some(msg) => match msg.request {
            Some(UpstreamRequest::Init(init)) => init,
            _ => {
                return Err(Status::invalid_argument(
                    "first message must be SubscriptionInit",
                ))
            }
        },
        None => return Err(Status::invalid_argument("stream closed before init")),
    };

    // Validate init
    if init.topic.is_empty() {
        return Err(Status::invalid_argument("topic cannot be empty"));
    }
    if init.topic.len() > 255 {
        return Err(Status::invalid_argument(
            "topic name exceeds 255 characters",
        ));
    }
    // Only allow alphanumeric, dash, underscore, and dot
    if !init
        .topic
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(Status::invalid_argument(
            "topic name contains invalid characters (only alphanumeric, dash, underscore, dot allowed)",
        ));
    }

    // Extract fields early before moving
    let initial_position = init.initial_position();
    let subscription_mode = init.mode();
    let browse_mode = subscription_mode == SubscriptionMode::Browse;
    let topic_name = init.topic.clone();

    let consumer_group = if init.consumer_group.is_empty() {
        "default".to_string()
    } else {
        init.consumer_group
    };

    let consumer_id = if init.consumer_id.is_empty() {
        generate_message_id()
    } else {
        init.consumer_id
    };

    tracing::info!(
        topic = %topic_name,
        consumer_group = %consumer_group,
        consumer_id = %consumer_id,
        browse_mode = browse_mode,
        "Subscription init"
    );

    // Look up topic using reader pool
    let topic = {
        let conn = state
            .reader_pool
            .get()
            .map_err(|e| Status::internal(format!("database error: {e}")))?;

        get_topic_by_name(&conn, &topic_name)
            .map_err(|e| Status::internal(format!("database error: {e}")))?
            .ok_or_else(|| {
                if initial_position == InitialPosition::Earliest {
                    Status::not_found(format!("topic '{}' does not exist", topic_name))
                } else {
                    // For LATEST, we'll wait for the topic to be created
                    Status::not_found(format!("topic '{}' does not exist", topic_name))
                }
            })?
    };

    // Determine starting cursor based on mode
    let start_cursor = if browse_mode {
        // Browse mode: always start from beginning, ignore cursor
        0
    } else {
        // Consumer group mode: get or create subscription
        let subscription = state
            .writer
            .get_or_create_subscription(topic.id, consumer_group.clone())
            .await
            .map_err(|e| Status::internal(format!("database error: {e}")))?;

        match initial_position {
            InitialPosition::Latest => {
                // Start from max sequence (new messages only)
                if subscription.cursor_seq == 0 {
                    let conn = state
                        .reader_pool
                        .get()
                        .map_err(|e| Status::internal(format!("database error: {e}")))?;
                    get_topic_max_seq(&conn, topic.id)
                        .map_err(|e| Status::internal(format!("database error: {e}")))?
                } else {
                    subscription.cursor_seq
                }
            }
            InitialPosition::Earliest => {
                // Use existing cursor or start from 0
                subscription.cursor_seq
            }
            InitialPosition::Offset => {
                // Start from specific offset provided in init message
                if init.offset == 0 {
                    return Err(Status::invalid_argument(
                        "offset must be provided and > 0 for OFFSET position",
                    ));
                }
                // Use the offset as cursor position (will start reading from offset + 1)
                init.offset as i64
            }
        }
    };

    // Register connection for takeover handling (only in consumer group mode)
    let (consumer_group_key, cancel_rx) = if browse_mode {
        // Browse mode: no takeover handling needed
        (None, None)
    } else {
        let key = ConsumerGroupKey {
            topic_id: topic.id,
            consumer_group: consumer_group.clone(),
        };
        let rx = state.connection_registry.register(key.clone());
        (Some(key), Some(rx))
    };

    // Create response channel
    let (tx, rx) = mpsc::channel(100);

    // Create credit balance
    let credits = Arc::new(CreditBalance::new());

    // Clone state for spawned task
    let state = Arc::clone(state);
    let credits_clone = Arc::clone(&credits);

    // Spawn subscription handler task
    tokio::spawn(async move {
        let result = subscription_loop(
            Arc::clone(&state),
            inbound,
            tx,
            topic.id,
            topic_name.clone(),
            consumer_group.clone(),
            consumer_id,
            start_cursor,
            credits_clone,
            cancel_rx,
            browse_mode,
        )
        .await;

        // Unregister connection when done (only if registered)
        if let Some(key) = consumer_group_key {
            state.connection_registry.unregister(&key);
        }

        if let Err(e) = result {
            tracing::warn!(error = %e, "Subscription ended with error");
        }
    });

    Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
}

/// Main subscription loop handling bidirectional communication.
#[allow(clippy::too_many_arguments)]
async fn subscription_loop(
    state: Arc<ServerState>,
    mut inbound: Streaming<SubscribeUpstream>,
    tx: mpsc::Sender<Result<SubscribeDownstream, Status>>,
    topic_id: i64,
    topic_name: String,
    consumer_group: String,
    consumer_id: String,
    initial_cursor: i64,
    credits: Arc<CreditBalance>,
    cancel_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    browse_mode: bool,
) -> Result<(), Status> {
    let mut cursor = initial_cursor;
    let mut notify_rx = state.notify_bus.subscribe();

    // In-memory delivery count tracking (per message_id).
    // Starts at 0; incremented to 1 on first delivery, 2 on redelivery after nack, etc.
    let mut delivery_counts: HashMap<String, u32> = HashMap::new();

    // Heartbeat interval (30 seconds)
    let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(30));
    // Skip first immediate tick
    heartbeat_interval.tick().await;

    tracing::debug!(
        topic_id,
        consumer_group = %consumer_group,
        cursor,
        browse_mode,
        "Subscription loop started"
    );

    // Wrap cancel_rx in a fuse so we can poll it optionally
    let mut cancel_rx = cancel_rx;

    loop {
        // Build the cancel future - either wait for signal or never resolve
        let cancel_fut = async {
            if let Some(ref mut rx) = cancel_rx {
                let _ = rx.await;
            } else {
                // In browse mode, never cancel via takeover
                std::future::pending::<()>().await;
            }
        };

        tokio::select! {
            // Handle consumer group takeover (cancellation) - only in consumer group mode
            _ = cancel_fut => {
                tracing::info!(
                    consumer_id = %consumer_id,
                    consumer_group = %consumer_group,
                    "Connection terminated due to consumer group takeover"
                );
                // Send ABORTED status to client
                let _ = tx.send(Err(Status::aborted("consumer group takeover"))).await;
                return Err(Status::aborted("consumer group takeover"));
            }

            // Handle inbound messages (CreditGrant, Ack, Nack, BatchAck)
            msg = inbound.message() => {
                match msg {
                    Ok(Some(upstream)) => {
                        match upstream.request {
                            Some(UpstreamRequest::Credit(grant)) => {
                                if grant.credits > 0 {
                                    credits.add(grant.credits);
                                    tracing::debug!(credits = grant.credits, "Credits granted");
                                }
                            }
                            Some(UpstreamRequest::Ack(ack)) => {
                                handle_ack(&state, topic_id, &consumer_group, &ack.message_id, &mut cursor, browse_mode).await?;
                            }
                            Some(UpstreamRequest::Nack(nack)) => {
                                handle_nack(&state, topic_id, &consumer_group, &nack.message_id, &mut cursor, browse_mode).await?;
                            }
                            Some(UpstreamRequest::BatchAck(batch_ack)) => {
                                handle_batch_ack(&state, topic_id, &consumer_group, &batch_ack.message_ids, &mut cursor, browse_mode).await?;
                            }
                            Some(UpstreamRequest::Init(_)) => {
                                return Err(Status::invalid_argument("unexpected SubscriptionInit"));
                            }
                            None => {}
                        }
                    }
                    Ok(None) => {
                        tracing::info!(consumer_id = %consumer_id, "Client disconnected");
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Inbound stream error");
                        return Err(e);
                    }
                }
            }

            // Send heartbeat periodically
            _ = heartbeat_interval.tick() => {
                // Get latest sequence for the topic
                let max_seq = {
                    let conn = state
                        .reader_pool
                        .get()
                        .map_err(|e| Status::internal(format!("database error: {e}")))?;
                    get_topic_max_seq(&conn, topic_id)
                        .map_err(|e| Status::internal(format!("database error: {e}")))?
                };

                let heartbeat = Heartbeat {
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64,
                    latest_seq: max_seq as u64,
                };

                if tx.send(Ok(SubscribeDownstream {
                    response: Some(DownstreamResponse::Heartbeat(heartbeat)),
                })).await.is_err() {
                    return Err(Status::cancelled("client disconnected"));
                }

                tracing::trace!(topic_id, max_seq, "Heartbeat sent");
            }

            // Handle notifications about new data
            notification = notify_rx.recv() => {
                match notification {
                    Ok(notif) if notif.topic_id == topic_id => {
                        // New data available, try to deliver
                        deliver_messages(&state, &tx, topic_id, &topic_name, &consumer_group, &mut cursor, &credits, &mut delivery_counts).await?;
                    }
                    Ok(_) => {
                        // Notification for different topic, ignore
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(lagged = n, "Notification receiver lagged");
                        // Try to deliver anyway
                        deliver_messages(&state, &tx, topic_id, &topic_name, &consumer_group, &mut cursor, &credits, &mut delivery_counts).await?;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::info!("Notification bus closed");
                        return Ok(());
                    }
                }
            }
        }

        // Try to deliver messages if we have credits
        if credits.available() > 0 {
            deliver_messages(
                &state,
                &tx,
                topic_id,
                &topic_name,
                &consumer_group,
                &mut cursor,
                &credits,
                &mut delivery_counts,
            )
            .await?;
        }
    }
}

/// Deliver available messages to the client.
#[allow(clippy::too_many_arguments)]
async fn deliver_messages(
    state: &Arc<ServerState>,
    tx: &mpsc::Sender<Result<SubscribeDownstream, Status>>,
    topic_id: i64,
    topic_name: &str,
    consumer_group: &str,
    cursor: &mut i64,
    credits: &Arc<CreditBalance>,
    delivery_counts: &mut HashMap<String, u32>,
) -> Result<(), Status> {
    let available_credits = credits.available();
    if available_credits == 0 {
        return Ok(());
    }

    // Fetch messages from database
    let conn = state
        .reader_pool
        .get()
        .map_err(|e| Status::internal(format!("database error: {e}")))?;

    let messages = fetch_messages_from_seq(&conn, topic_id, *cursor, available_credits as i64)
        .map_err(|e| Status::internal(format!("database error: {e}")))?;

    // Get max sequence for lag calculation
    let max_seq = get_topic_max_seq(&conn, topic_id)
        .map_err(|e| Status::internal(format!("database error: {e}")))?;

    drop(conn);

    // Record subscription lag
    let lag = max_seq - *cursor;
    record_subscription_lag(topic_name, consumer_group, lag);

    // Record backpressure state (active if no credits and there's lag)
    let has_backpressure = available_credits == 0 && lag > 0;
    record_backpressure(topic_name, consumer_group, has_backpressure);

    for msg in messages {
        // Try to consume a credit
        if !credits.try_consume() {
            break;
        }

        // Parse attributes from JSON
        let attributes = msg
            .attributes
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        // Track delivery count: increment for this message_id
        let count = delivery_counts.entry(msg.message_id.clone()).or_insert(0);
        *count += 1;
        let delivery_count = *count;

        let delivery = MessageDelivery {
            message_id: msg.message_id,
            sequence: msg.global_seq as u64,
            payload: msg.payload.unwrap_or_default(),
            attributes,
            timestamp: msg.created_at,
            delivery_count,
        };

        // Send to client
        if tx
            .send(Ok(SubscribeDownstream {
                response: Some(DownstreamResponse::Delivery(delivery)),
            }))
            .await
            .is_err()
        {
            return Err(Status::cancelled("client disconnected"));
        }

        // Update local cursor (but don't persist until ACK)
        *cursor = msg.global_seq;
    }

    Ok(())
}

/// Handle an ACK message.
async fn handle_ack(
    state: &Arc<ServerState>,
    topic_id: i64,
    consumer_group: &str,
    message_id: &str,
    cursor: &mut i64,
    browse_mode: bool,
) -> Result<(), Status> {
    // In browse mode, ACKs don't update the cursor
    if browse_mode {
        tracing::debug!(message_id, "Browse mode: ACK ignored (cursor not updated)");
        return Ok(());
    }

    // Look up message sequence using reader pool
    let seq = {
        let conn = state
            .reader_pool
            .get()
            .map_err(|e| Status::internal(format!("database error: {e}")))?;

        get_message_seq_by_id(&conn, message_id)
            .map_err(|e| Status::internal(format!("database error: {e}")))?
    };

    match seq {
        Some(seq) => {
            // Update cursor via writer (requires write access)
            state
                .writer
                .update_cursor(topic_id, consumer_group.to_string(), seq)
                .await
                .map_err(|e| Status::internal(format!("database error: {e}")))?;

            *cursor = seq;
            tracing::debug!(message_id, seq, "Cursor updated");
        }
        None => {
            tracing::warn!(message_id, "ACK for unknown message");
        }
    }

    Ok(())
}

/// Handle a Nack message.
///
/// Resets the cursor to just before the nacked message's sequence,
/// so that message (and any after it) will be redelivered.
async fn handle_nack(
    state: &Arc<ServerState>,
    _topic_id: i64,
    _consumer_group: &str,
    message_id: &str,
    cursor: &mut i64,
    browse_mode: bool,
) -> Result<(), Status> {
    // In browse mode, Nacks are ignored
    if browse_mode {
        tracing::debug!(message_id, "Browse mode: NACK ignored");
        return Ok(());
    }

    // Look up message sequence
    let seq = {
        let conn = state
            .reader_pool
            .get()
            .map_err(|e| Status::internal(format!("database error: {e}")))?;

        get_message_seq_by_id(&conn, message_id)
            .map_err(|e| Status::internal(format!("database error: {e}")))?
    };

    match seq {
        Some(seq) => {
            // Reset cursor to just before this message so it will be redelivered.
            // We move the local cursor backwards; the persisted cursor only advances
            // (via update_cursor's WHERE clause), so no write is needed here.
            let new_cursor = seq - 1;
            *cursor = new_cursor;
            tracing::debug!(
                message_id,
                seq,
                new_cursor,
                "Nack processed, cursor reset for redelivery"
            );
        }
        None => {
            tracing::warn!(message_id, "NACK for unknown message");
        }
    }

    Ok(())
}

/// Handle a BatchAck message.
///
/// Advances the cursor to the maximum sequence among the acknowledged messages.
async fn handle_batch_ack(
    state: &Arc<ServerState>,
    topic_id: i64,
    consumer_group: &str,
    message_ids: &[String],
    cursor: &mut i64,
    browse_mode: bool,
) -> Result<(), Status> {
    // In browse mode, ACKs don't update the cursor
    if browse_mode {
        tracing::debug!(count = message_ids.len(), "Browse mode: BatchAck ignored");
        return Ok(());
    }

    if message_ids.is_empty() {
        return Ok(());
    }

    // Find the maximum sequence among all acknowledged messages
    let conn = state
        .reader_pool
        .get()
        .map_err(|e| Status::internal(format!("database error: {e}")))?;

    let mut max_seq: Option<i64> = None;
    for message_id in message_ids {
        if let Some(seq) = get_message_seq_by_id(&conn, message_id)
            .map_err(|e| Status::internal(format!("database error: {e}")))?
        {
            max_seq = Some(max_seq.map_or(seq, |current: i64| current.max(seq)));
        } else {
            tracing::warn!(message_id = %message_id, "BatchAck: unknown message_id, skipping");
        }
    }

    drop(conn);

    if let Some(seq) = max_seq {
        // Update cursor via writer
        state
            .writer
            .update_cursor(topic_id, consumer_group.to_string(), seq)
            .await
            .map_err(|e| Status::internal(format!("database error: {e}")))?;

        *cursor = seq;
        tracing::debug!(
            count = message_ids.len(),
            max_seq = seq,
            "BatchAck processed, cursor advanced"
        );
    }

    Ok(())
}
