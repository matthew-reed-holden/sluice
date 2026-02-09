//! Admin service handlers (DeleteTopic, DeleteConsumerGroup, ResetCursor).

use std::sync::Arc;

use tonic::{Request, Response, Status};

use crate::proto::sluice::v1::{
    DeleteConsumerGroupRequest, DeleteConsumerGroupResponse, DeleteTopicRequest,
    DeleteTopicResponse, ResetCursorRequest, ResetCursorResponse,
};
use crate::server::ServerState;

pub async fn handle_delete_topic(
    state: &Arc<ServerState>,
    request: Request<DeleteTopicRequest>,
) -> Result<Response<DeleteTopicResponse>, Status> {
    let req = request.into_inner();

    if req.topic.is_empty() {
        return Err(Status::invalid_argument("topic name is required"));
    }

    let deleted = state
        .writer
        .delete_topic(req.topic)
        .await
        .map_err(|e| Status::internal(format!("failed to delete topic: {e}")))?;

    Ok(Response::new(DeleteTopicResponse { deleted }))
}

pub async fn handle_delete_consumer_group(
    state: &Arc<ServerState>,
    request: Request<DeleteConsumerGroupRequest>,
) -> Result<Response<DeleteConsumerGroupResponse>, Status> {
    let req = request.into_inner();

    if req.topic.is_empty() {
        return Err(Status::invalid_argument("topic name is required"));
    }
    if req.consumer_group.is_empty() {
        return Err(Status::invalid_argument("consumer_group is required"));
    }

    let deleted = state
        .writer
        .delete_consumer_group(req.topic, req.consumer_group)
        .await
        .map_err(|e| Status::internal(format!("failed to delete consumer group: {e}")))?;

    Ok(Response::new(DeleteConsumerGroupResponse { deleted }))
}

pub async fn handle_reset_cursor(
    state: &Arc<ServerState>,
    request: Request<ResetCursorRequest>,
) -> Result<Response<ResetCursorResponse>, Status> {
    let req = request.into_inner();

    if req.topic.is_empty() {
        return Err(Status::invalid_argument("topic name is required"));
    }
    if req.consumer_group.is_empty() {
        return Err(Status::invalid_argument("consumer_group is required"));
    }

    let updated = state
        .writer
        .reset_cursor(req.topic, req.consumer_group, req.sequence)
        .await
        .map_err(|e| Status::internal(format!("failed to reset cursor: {e}")))?;

    Ok(Response::new(ResetCursorResponse { updated }))
}
