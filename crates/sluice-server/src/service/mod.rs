//! gRPC service handlers for Sluice.

pub mod batch_publish;
pub mod publish;
pub mod registry;
pub mod stats;
pub mod subscribe;
pub mod topics;

pub use registry::{ConnectionRegistry, ConsumerGroupKey};

use std::pin::Pin;
use std::sync::Arc;
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};

use crate::proto::sluice::v1::sluice_server::Sluice;
use crate::proto::sluice::v1::{
    BatchPublishRequest, BatchPublishResponse, GetTopicStatsRequest, GetTopicStatsResponse,
    ListTopicsRequest, ListTopicsResponse, PublishRequest, PublishResponse, SubscribeDownstream,
    SubscribeUpstream,
};
use crate::server::ServerState;

/// Sluice gRPC service implementation.
pub struct SluiceService {
    state: Arc<ServerState>,
}

impl SluiceService {
    /// Create a new Sluice service with shared state.
    pub fn new(state: Arc<ServerState>) -> Self {
        Self { state }
    }
}

type SubscribeStream =
    Pin<Box<dyn Stream<Item = Result<SubscribeDownstream, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl Sluice for SluiceService {
    async fn publish(
        &self,
        request: Request<PublishRequest>,
    ) -> Result<Response<PublishResponse>, Status> {
        publish::handle_publish(&self.state, request).await
    }

    async fn batch_publish(
        &self,
        request: Request<BatchPublishRequest>,
    ) -> Result<Response<BatchPublishResponse>, Status> {
        batch_publish::handle_batch_publish(&self.state, request).await
    }

    type SubscribeStream = SubscribeStream;

    async fn subscribe(
        &self,
        request: Request<Streaming<SubscribeUpstream>>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        subscribe::handle_subscribe(&self.state, request).await
    }

    async fn list_topics(
        &self,
        request: Request<ListTopicsRequest>,
    ) -> Result<Response<ListTopicsResponse>, Status> {
        topics::handle_list_topics(&self.state, request).await
    }

    async fn get_topic_stats(
        &self,
        request: Request<GetTopicStatsRequest>,
    ) -> Result<Response<GetTopicStatsResponse>, Status> {
        stats::handle_get_topic_stats(&self.state, request).await
    }
}
