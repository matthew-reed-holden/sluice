//! Connection management for Sluice gRPC client.

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};

use crate::proto::sluice::v1::sluice_client::SluiceClient as ProtoClient;
use crate::proto::sluice::v1::{ListTopicsRequest, PublishRequest, Topic};

use super::ops::PublishResult;
use super::subscription::Subscription;
use crate::proto::sluice::v1::InitialPosition;

/// Configuration for connecting to a Sluice server.
#[derive(Debug, Clone)]
pub struct ConnectConfig {
    /// Server endpoint (e.g., "http://localhost:50051" or "https://...")
    pub endpoint: String,
    /// Optional path to CA certificate for TLS verification
    pub tls_ca: Option<String>,
    /// Optional TLS domain name override
    pub tls_domain: Option<String>,
}

impl ConnectConfig {
    /// Create a new connection config for a plaintext endpoint.
    pub fn plaintext(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            tls_ca: None,
            tls_domain: None,
        }
    }

    /// Create a new connection config for a TLS endpoint.
    pub fn tls(endpoint: impl Into<String>, ca_path: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            tls_ca: Some(ca_path.into()),
            tls_domain: None,
        }
    }

    /// Set the TLS domain name override.
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.tls_domain = Some(domain.into());
        self
    }
}

/// A gRPC client for interacting with Sluice servers.
pub struct SluiceClient {
    inner: ProtoClient<Channel>,
}

impl SluiceClient {
    /// Connect to a Sluice server using the provided configuration.
    pub async fn connect(config: ConnectConfig) -> Result<Self> {
        let endpoint = &config.endpoint;
        let is_https = endpoint.starts_with("https://");
        let is_http = endpoint.starts_with("http://");

        if !is_https && !is_http {
            return Err(anyhow!("endpoint must start with http:// or https://"));
        }

        if is_http {
            if config.tls_ca.is_some() || config.tls_domain.is_some() {
                return Err(anyhow!(
                    "TLS options are only valid with https:// endpoints"
                ));
            }
            let channel = Endpoint::from_shared(endpoint.to_string())?
                .connect_timeout(Duration::from_secs(5))
                .connect()
                .await
                .context("failed to connect to server")?;
            return Ok(Self {
                inner: ProtoClient::new(channel),
            });
        }

        // HTTPS connection with TLS
        let ca_path = config
            .tls_ca
            .as_ref()
            .ok_or_else(|| anyhow!("TLS CA certificate path is required for https:// endpoints"))?;

        let ca_pem = std::fs::read(Path::new(ca_path))
            .with_context(|| format!("failed to read TLS CA file: {}", ca_path))?;
        let ca_cert = Certificate::from_pem(ca_pem);

        let mut tls = ClientTlsConfig::new().ca_certificate(ca_cert);
        if let Some(domain) = &config.tls_domain {
            tls = tls.domain_name(domain);
        }

        let channel = Endpoint::from_shared(endpoint.to_string())?
            .tls_config(tls)?
            .connect_timeout(Duration::from_secs(5))
            .connect()
            .await
            .context("failed to connect to server with TLS")?;

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
        };
        Self::connect(config).await
    }

    /// List all topics on the server.
    pub async fn list_topics(&mut self) -> Result<Vec<Topic>> {
        let resp = self
            .inner
            .list_topics(ListTopicsRequest {})
            .await
            .context("list_topics RPC failed")?
            .into_inner();
        Ok(resp.topics)
    }

    /// Publish a message to a topic.
    pub async fn publish(&mut self, topic: &str, payload: Vec<u8>) -> Result<PublishResult> {
        let resp = self
            .inner
            .publish(PublishRequest {
                topic: topic.to_string(),
                payload,
                attributes: Default::default(),
            })
            .await
            .context("publish RPC failed")?
            .into_inner();

        Ok(PublishResult {
            message_id: resp.message_id,
            sequence: resp.sequence,
        })
    }

    /// Publish a message with string payload (convenience method).
    pub async fn publish_str(&mut self, topic: &str, payload: &str) -> Result<PublishResult> {
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
            credits_window,
        )
        .await
    }
}
