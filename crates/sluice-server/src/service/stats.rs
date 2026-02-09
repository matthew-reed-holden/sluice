//! GetTopicStats RPC handler implementation.
//!
//! Provides statistics about topics including message counts, sequence ranges,
//! timestamps, and consumer group cursor positions.

use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::proto::sluice::v1::{
    ConsumerGroupStats, GetTopicStatsRequest, GetTopicStatsResponse, TopicStats,
};
use crate::server::ServerState;
use crate::storage::schema::{
    get_all_topics, get_consumer_groups_for_topic, get_topic_by_name, get_topic_message_stats,
};

/// Handle a GetTopicStats RPC request.
///
/// Returns statistics for the requested topics, or all topics if none specified.
#[tracing::instrument(skip(state, request))]
pub async fn handle_get_topic_stats(
    state: &Arc<ServerState>,
    request: Request<GetTopicStatsRequest>,
) -> Result<Response<GetTopicStatsResponse>, Status> {
    let req = request.into_inner();

    let conn = state
        .reader_pool
        .get()
        .map_err(|e| Status::internal(format!("database error: {e}")))?;

    // Get list of topics to query
    let topics = if req.topics.is_empty() {
        // Return all topics
        get_all_topics(&conn).map_err(|e| Status::internal(format!("database error: {e}")))?
    } else {
        // Return only requested topics
        let mut topics = Vec::with_capacity(req.topics.len());
        for topic_name in &req.topics {
            if let Some(topic) = get_topic_by_name(&conn, topic_name)
                .map_err(|e| Status::internal(format!("database error: {e}")))?
            {
                topics.push(topic);
            }
            // Silently skip topics that don't exist
        }
        topics
    };

    // Build stats for each topic
    let mut stats = Vec::with_capacity(topics.len());
    for topic in topics {
        // Get message statistics
        let msg_stats = get_topic_message_stats(&conn, topic.id)
            .map_err(|e| Status::internal(format!("database error: {e}")))?;

        // Get consumer groups
        let consumer_groups = get_consumer_groups_for_topic(&conn, topic.id)
            .map_err(|e| Status::internal(format!("database error: {e}")))?;

        // Build consumer group stats with pending count
        let consumer_group_stats: Vec<ConsumerGroupStats> = consumer_groups
            .into_iter()
            .map(|cg| {
                let pending = if msg_stats.last_sequence > cg.cursor_seq {
                    msg_stats.last_sequence - cg.cursor_seq
                } else {
                    0
                };
                ConsumerGroupStats {
                    group_name: cg.group_name,
                    cursor_sequence: cg.cursor_seq,
                    pending_count: pending,
                    last_ack_timestamp: cg.updated_at.unwrap_or(0),
                }
            })
            .collect();

        stats.push(TopicStats {
            topic: topic.name,
            total_messages: msg_stats.total_messages,
            first_sequence: msg_stats.first_sequence,
            last_sequence: msg_stats.last_sequence,
            first_message_timestamp: msg_stats.first_timestamp.unwrap_or(0),
            last_message_timestamp: msg_stats.last_timestamp.unwrap_or(0),
            created_at: topic.created_at,
            consumer_groups: consumer_group_stats,
        });
    }

    tracing::debug!(topic_count = stats.len(), "Returning topic stats");

    Ok(Response::new(GetTopicStatsResponse { stats }))
}
