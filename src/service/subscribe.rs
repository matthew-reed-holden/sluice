//! Subscribe RPC handler implementation.
//!
//! Handles bidirectional streaming for message consumption with credit-based flow control.

use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};

use crate::flow::credit::CreditBalance;
use crate::observability::metrics::{record_backpressure, record_subscription_lag};
use crate::proto::sluice::v1::subscribe_downstream::Response as DownstreamResponse;
use crate::proto::sluice::v1::subscribe_upstream::Request as UpstreamRequest;
use crate::proto::sluice::v1::{
    InitialPosition, MessageDelivery, SubscribeDownstream, SubscribeUpstream,
};
use crate::server::ServerState;
use crate::service::ConsumerGroupKey;
use crate::storage::schema::{
    fetch_messages_from_seq, get_message_seq_by_id, get_topic_by_name, get_topic_max_seq,
};
use crate::generate_message_id;

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

    // Extract initial_position early before moving other fields
    let initial_position = init.initial_position();
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

    // Get or create subscription via writer (requires write access)
    let subscription = state
        .writer
        .get_or_create_subscription(topic.id, consumer_group.clone())
        .await
        .map_err(|e| Status::internal(format!("database error: {e}")))?;

    // Determine starting cursor
    let start_cursor = match initial_position {
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
    };

    // Register connection for takeover handling
    let consumer_group_key = ConsumerGroupKey {
        topic_id: topic.id,
        consumer_group: consumer_group.clone(),
    };
    let cancel_rx = state
        .connection_registry
        .register(consumer_group_key.clone());

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
        )
        .await;

        // Unregister connection when done
        state.connection_registry.unregister(&consumer_group_key);

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
    mut cancel_rx: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), Status> {
    let mut cursor = initial_cursor;
    let mut notify_rx = state.notify_bus.subscribe();

    tracing::debug!(
        topic_id,
        consumer_group = %consumer_group,
        cursor,
        "Subscription loop started"
    );

    loop {
        tokio::select! {
            // Handle consumer group takeover (cancellation)
            _ = &mut cancel_rx => {
                tracing::info!(
                    consumer_id = %consumer_id,
                    consumer_group = %consumer_group,
                    "Connection terminated due to consumer group takeover"
                );
                // Send ABORTED status to client
                let _ = tx.send(Err(Status::aborted("consumer group takeover"))).await;
                return Err(Status::aborted("consumer group takeover"));
            }

            // Handle inbound messages (CreditGrant, Ack)
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
                                handle_ack(&state, topic_id, &consumer_group, &ack.message_id, &mut cursor).await?;
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

            // Handle notifications about new data
            notification = notify_rx.recv() => {
                match notification {
                    Ok(notif) if notif.topic_id == topic_id => {
                        // New data available, try to deliver
                        deliver_messages(&state, &tx, topic_id, &topic_name, &consumer_group, &mut cursor, &credits).await?;
                    }
                    Ok(_) => {
                        // Notification for different topic, ignore
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(lagged = n, "Notification receiver lagged");
                        // Try to deliver anyway
                        deliver_messages(&state, &tx, topic_id, &topic_name, &consumer_group, &mut cursor, &credits).await?;
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

        let delivery = MessageDelivery {
            message_id: msg.message_id,
            sequence: msg.global_seq as u64,
            payload: msg.payload.unwrap_or_default(),
            attributes,
            timestamp: msg.created_at,
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
) -> Result<(), Status> {
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
