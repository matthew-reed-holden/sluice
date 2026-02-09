//! Connection management for Sluice gRPC client.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};

use crate::error::{Result, SluiceError};

use sluice_proto::sluice::v1::sluice_client::SluiceClient as ProtoClient;
use sluice_proto::sluice::v1::{
    DeleteConsumerGroupRequest, DeleteTopicRequest, GetTopicStatsRequest, InitialPosition,
    ListTopicsRequest, PublishRequest, PublishResponse, ResetCursorRequest, SubscriptionMode,
    Topic, TopicStats,
};

use super::subscription::Subscription;

/// Configuration for retry logic with exponential backoff.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (0 means no retries).
    pub max_retries: u32,
    /// Initial backoff duration.
    pub initial_backoff: Duration,
    /// Maximum backoff duration.
    pub max_backoff: Duration,
    /// Backoff multiplier (exponential factor).
    pub multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(10),
            multiplier: 2.0,
        }
    }
}

impl RetryConfig {
    /// Create a retry config with no retries.
    pub fn no_retry() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Create a retry config with custom max retries.
    pub fn with_max_retries(max_retries: u32) -> Self {
        Self {
            max_retries,
            ..Default::default()
        }
    }

    /// Calculate the backoff duration for a given attempt.
    pub fn backoff_for_attempt(&self, attempt: u32) -> Duration {
        let backoff = self.initial_backoff.as_secs_f64() * self.multiplier.powi(attempt as i32);
        let backoff_secs = backoff.min(self.max_backoff.as_secs_f64());
        Duration::from_secs_f64(backoff_secs)
    }
}

/// Configuration for connecting to a Sluice server.
#[derive(Debug, Clone)]
pub struct ConnectConfig {
    /// Server endpoint (e.g., "http://localhost:50051" or "https://...")
    pub endpoint: String,
    /// Optional path to CA certificate for TLS verification
    pub tls_ca: Option<String>,
    /// Optional TLS domain name override
    pub tls_domain: Option<String>,
    /// Retry configuration for connection attempts.
    pub retry: RetryConfig,
}

impl ConnectConfig {
    /// Create a new connection config for a plaintext endpoint.
    pub fn plaintext(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            tls_ca: None,
            tls_domain: None,
            retry: RetryConfig::default(),
        }
    }

    /// Create a new connection config for a TLS endpoint.
    pub fn tls(endpoint: impl Into<String>, ca_path: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            tls_ca: Some(ca_path.into()),
            tls_domain: None,
            retry: RetryConfig::default(),
        }
    }

    /// Set the TLS domain name override.
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.tls_domain = Some(domain.into());
        self
    }

    /// Set the retry configuration.
    pub fn with_retry(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    /// Disable retries.
    pub fn without_retry(mut self) -> Self {
        self.retry = RetryConfig::no_retry();
        self
    }
}

/// A gRPC client for interacting with Sluice servers.
pub struct SluiceClient {
    inner: ProtoClient<Channel>,
}

impl SluiceClient {
    /// Connect to a Sluice server using the provided configuration.
    ///
    /// Uses exponential backoff retry logic based on the retry configuration.
    pub async fn connect(config: ConnectConfig) -> Result<Self> {
        let retry_config = config.retry.clone();
        let mut last_error: Option<SluiceError> = None;

        for attempt in 0..=retry_config.max_retries {
            if attempt > 0 {
                let backoff = retry_config.backoff_for_attempt(attempt - 1);
                tracing::debug!(
                    attempt,
                    backoff_ms = backoff.as_millis(),
                    "Retrying connection"
                );
                tokio::time::sleep(backoff).await;
            }

            match Self::try_connect(&config).await {
                Ok(client) => return Ok(client),
                Err(e) => {
                    tracing::warn!(
                        attempt,
                        max_retries = retry_config.max_retries,
                        error = %e,
                        "Connection attempt failed"
                    );
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| SluiceError::InvalidConfig {
            message: "connection failed with no attempts".into(),
        }))
    }

    /// Try to connect once without retry.
    async fn try_connect(config: &ConnectConfig) -> Result<Self> {
        let endpoint = &config.endpoint;
        let is_https = endpoint.starts_with("https://");
        let is_http = endpoint.starts_with("http://");

        if !is_https && !is_http {
            return Err(SluiceError::InvalidConfig {
                message: "endpoint must start with http:// or https://".into(),
            });
        }

        if is_http {
            if config.tls_ca.is_some() || config.tls_domain.is_some() {
                return Err(SluiceError::InvalidConfig {
                    message: "TLS options are only valid with https:// endpoints".into(),
                });
            }
            let channel = Endpoint::from_shared(endpoint.to_string())
                .map_err(|e| SluiceError::InvalidConfig {
                    message: e.to_string(),
                })?
                .connect_timeout(Duration::from_secs(5))
                .connect()
                .await
                .map_err(|source| SluiceError::ConnectionFailed {
                    endpoint: endpoint.clone(),
                    source,
                })?;
            return Ok(Self {
                inner: ProtoClient::new(channel),
            });
        }

        // HTTPS connection with TLS
        let ca_path = config
            .tls_ca
            .as_ref()
            .ok_or_else(|| SluiceError::InvalidConfig {
                message: "TLS CA certificate path is required for https:// endpoints".into(),
            })?;

        let ca_pem = std::fs::read(Path::new(ca_path)).map_err(|source| SluiceError::Io {
            context: format!("failed to read TLS CA file: {}", ca_path),
            source,
        })?;
        let ca_cert = Certificate::from_pem(ca_pem);

        let mut tls = ClientTlsConfig::new().ca_certificate(ca_cert);
        if let Some(domain) = &config.tls_domain {
            tls = tls.domain_name(domain);
        }

        let channel = Endpoint::from_shared(endpoint.to_string())
            .map_err(|e| SluiceError::InvalidConfig {
                message: e.to_string(),
            })?
            .tls_config(tls)
            .map_err(|source| SluiceError::ConnectionFailed {
                endpoint: endpoint.clone(),
                source,
            })?
            .connect_timeout(Duration::from_secs(5))
            .connect()
            .await
            .map_err(|source| SluiceError::ConnectionFailed {
                endpoint: endpoint.clone(),
                source,
            })?;

        Ok(Self {
            inner: ProtoClient::new(channel),
        })
    }

    /// Connect using individual parameters (convenience method).
    pub async fn connect_with(
        endpoint: &str,
        tls_ca: Option<&Path>,
        tls_domain: Option<&str>,
    ) -> Result<Self> {
        let config = ConnectConfig {
            endpoint: endpoint.to_string(),
            tls_ca: tls_ca.map(|p| p.to_string_lossy().to_string()),
            tls_domain: tls_domain.map(String::from),
            retry: RetryConfig::default(),
        };
        Self::connect(config).await
    }

    /// List all topics on the server.
    pub async fn list_topics(&mut self) -> Result<Vec<Topic>> {
        let resp = self
            .inner
            .list_topics(ListTopicsRequest {})
            .await
            .map_err(|source| SluiceError::RpcFailed {
                method: "list_topics",
                source,
            })?
            .into_inner();
        Ok(resp.topics)
    }

    /// Publish a message to a topic.
    pub async fn publish(&mut self, topic: &str, payload: Vec<u8>) -> Result<PublishResponse> {
        self.publish_with_attributes(topic, payload, HashMap::new())
            .await
    }

    /// Publish a message to a topic with custom attributes.
    ///
    /// Attributes are key-value string pairs attached to the message,
    /// useful for routing, filtering, or carrying metadata without
    /// modifying the payload.
    pub async fn publish_with_attributes(
        &mut self,
        topic: &str,
        payload: Vec<u8>,
        attributes: HashMap<String, String>,
    ) -> Result<PublishResponse> {
        let resp = self
            .inner
            .publish(PublishRequest {
                topic: topic.to_string(),
                payload,
                attributes,
            })
            .await
            .map_err(|source| SluiceError::RpcFailed {
                method: "publish",
                source,
            })?
            .into_inner();

        Ok(PublishResponse {
            timestamp: resp.timestamp,
            message_id: resp.message_id,
            sequence: resp.sequence,
        })
    }

    /// Publish a message with string payload (convenience method).
    pub async fn publish_str(&mut self, topic: &str, payload: &str) -> Result<PublishResponse> {
        self.publish(topic, payload.as_bytes().to_vec()).await
    }

    /// Start a subscription to a topic.
    ///
    /// Returns a `Subscription` handle for receiving messages and sending acks.
    pub async fn subscribe(
        &mut self,
        topic: &str,
        consumer_group: Option<&str>,
        consumer_id: Option<&str>,
        initial_position: InitialPosition,
        credits_window: u32,
    ) -> Result<Subscription> {
        Subscription::start(
            &mut self.inner,
            topic.to_string(),
            consumer_group.map(String::from),
            consumer_id.map(String::from),
            initial_position,
            SubscriptionMode::ConsumerGroup,
            credits_window,
        )
        .await
    }

    /// Start a browse subscription to a topic.
    ///
    /// Browse mode reads all messages from the beginning without affecting
    /// the consumer group cursor. ACKs are ignored in browse mode.
    /// This is useful for debugging and inspecting topic contents.
    pub async fn subscribe_browse(
        &mut self,
        topic: &str,
        credits_window: u32,
    ) -> Result<Subscription> {
        Subscription::start(
            &mut self.inner,
            topic.to_string(),
            None,                      // consumer_group not used in browse mode
            None,                      // consumer_id
            InitialPosition::Earliest, // Always start from beginning
            SubscriptionMode::Browse,
            credits_window,
        )
        .await
    }

    /// Get statistics for topics.
    ///
    /// If `topics` is empty, returns stats for all topics.
    /// Otherwise, returns stats only for the specified topics.
    pub async fn get_topic_stats(&mut self, topics: Vec<String>) -> Result<Vec<TopicStats>> {
        let resp = self
            .inner
            .get_topic_stats(GetTopicStatsRequest { topics })
            .await
            .map_err(|source| SluiceError::RpcFailed {
                method: "get_topic_stats",
                source,
            })?
            .into_inner();
        Ok(resp.stats)
    }

    /// Delete a topic and all its messages and subscriptions.
    ///
    /// Returns true if the topic existed and was deleted.
    pub async fn delete_topic(&mut self, topic: &str) -> Result<bool> {
        let resp = self
            .inner
            .delete_topic(DeleteTopicRequest {
                topic: topic.to_string(),
            })
            .await
            .map_err(|source| SluiceError::RpcFailed {
                method: "delete_topic",
                source,
            })?
            .into_inner();
        Ok(resp.deleted)
    }

    /// Delete a consumer group's subscription for a topic.
    ///
    /// Returns true if the subscription existed and was deleted.
    pub async fn delete_consumer_group(
        &mut self,
        topic: &str,
        consumer_group: &str,
    ) -> Result<bool> {
        let resp = self
            .inner
            .delete_consumer_group(DeleteConsumerGroupRequest {
                topic: topic.to_string(),
                consumer_group: consumer_group.to_string(),
            })
            .await
            .map_err(|source| SluiceError::RpcFailed {
                method: "delete_consumer_group",
                source,
            })?
            .into_inner();
        Ok(resp.deleted)
    }

    /// Reset a consumer group's cursor to a specific sequence number.
    ///
    /// This allows reprocessing messages from a specific point. The cursor
    /// can be set to any value, including backwards (unlike normal ACK
    /// processing which only advances).
    ///
    /// Returns true if the subscription existed and was updated.
    pub async fn reset_cursor(
        &mut self,
        topic: &str,
        consumer_group: &str,
        sequence: u64,
    ) -> Result<bool> {
        let resp = self
            .inner
            .reset_cursor(ResetCursorRequest {
                topic: topic.to_string(),
                consumer_group: consumer_group.to_string(),
                sequence,
            })
            .await
            .map_err(|source| SluiceError::RpcFailed {
                method: "reset_cursor",
                source,
            })?
            .into_inner();
        Ok(resp.updated)
    }
}
