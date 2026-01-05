use std::path::Path;

use anyhow::{anyhow, Context, Result};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};
use tonic::Streaming;

use crate::proto::sluice::v1::sluice_client::SluiceClient;
use crate::proto::sluice::v1::{
    subscribe_upstream, Ack, CreditGrant, InitialPosition, ListTopicsRequest, MessageDelivery,
    PublishRequest, PublishResponse, SubscribeDownstream, SubscribeUpstream, SubscriptionInit,
    Topic,
};

#[allow(dead_code)]
pub struct GrpcClient {
    inner: SluiceClient<Channel>,
}

/// A handle for controlling an active subscription.
#[allow(dead_code)]
pub struct Subscription {
    /// Sender to send upstream messages (credits, acks).
    tx: mpsc::Sender<SubscribeUpstream>,
    /// Receiver for downstream message deliveries.
    pub rx: Streaming<SubscribeDownstream>,
    /// Configured credits window for refill policy.
    credits_window: u32,
    /// Remaining credits before refill.
    remaining_credits: u32,
}

impl Subscription {
    /// Check if credits should be refilled and send grant if needed.
    /// Returns true if a grant was sent.
    #[allow(dead_code)]
    pub async fn maybe_refill_credits(&mut self) -> Result<bool> {
        // Refill when remaining < credits_window / 2
        let threshold = self.credits_window / 2;
        if self.remaining_credits < threshold {
            let grant = self.credits_window;
            self.send_credit(grant).await?;
            self.remaining_credits += grant;
            return Ok(true);
        }
        Ok(false)
    }

    /// Decrement remaining credits (call when a message is received).
    #[allow(dead_code)]
    pub fn consume_credit(&mut self) {
        self.remaining_credits = self.remaining_credits.saturating_sub(1);
    }

    /// Send a CreditGrant message.
    #[allow(dead_code)]
    pub async fn send_credit(&self, credits: u32) -> Result<()> {
        self.tx
            .send(SubscribeUpstream {
                request: Some(subscribe_upstream::Request::Credit(CreditGrant { credits })),
            })
            .await
            .map_err(|_| anyhow!("subscription channel closed"))
    }

    /// Send an Ack message.
    #[allow(dead_code)]
    pub async fn send_ack(&self, message_id: String) -> Result<()> {
        self.tx
            .send(SubscribeUpstream {
                request: Some(subscribe_upstream::Request::Ack(Ack { message_id })),
            })
            .await
            .map_err(|_| anyhow!("subscription channel closed"))
    }

    /// Get the next message delivery, returning None if stream ends.
    #[allow(dead_code)]
    pub async fn next_message(&mut self) -> Result<Option<MessageDelivery>> {
        use futures::StreamExt;
        match self.rx.next().await {
            Some(Ok(downstream)) => {
                if let Some(crate::proto::sluice::v1::subscribe_downstream::Response::Delivery(
                    msg,
                )) = downstream.response
                {
                    self.consume_credit();
                    Ok(Some(msg))
                } else {
                    Ok(None)
                }
            }
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }
}

impl GrpcClient {
    #[allow(dead_code)]
    pub async fn connect(
        endpoint: &str,
        tls_ca: Option<&Path>,
        tls_domain: Option<&str>,
    ) -> Result<Self> {
        let is_https = endpoint.starts_with("https://");
        let is_http = endpoint.starts_with("http://");
        if !is_https && !is_http {
            return Err(anyhow!("endpoint must start with http:// or https://"));
        }

        if is_http {
            if tls_ca.is_some() || tls_domain.is_some() {
                return Err(anyhow!(
                    "TLS flags are only valid with an https:// endpoint"
                ));
            }
            let channel = Endpoint::from_shared(endpoint.to_string())?
                .connect()
                .await
                .context("failed to connect")?;
            return Ok(Self {
                inner: SluiceClient::new(channel),
            });
        }

        // https://
        let tls_ca =
            tls_ca.ok_or_else(|| anyhow!("--tls-ca is required for https:// endpoints"))?;
        let ca_pem = std::fs::read(tls_ca)
            .with_context(|| format!("failed to read tls ca file: {}", tls_ca.display()))?;
        let ca_cert = Certificate::from_pem(ca_pem);

        let mut tls = ClientTlsConfig::new().ca_certificate(ca_cert);
        if let Some(domain) = tls_domain {
            tls = tls.domain_name(domain);
        }

        let channel = Endpoint::from_shared(endpoint.to_string())?
            .tls_config(tls)?
            .connect()
            .await
            .context("failed to connect")?;

        Ok(Self {
            inner: SluiceClient::new(channel),
        })
    }

    #[allow(dead_code)]
    pub async fn list_topics(&mut self) -> Result<Vec<Topic>> {
        let resp = self
            .inner
            .list_topics(ListTopicsRequest {})
            .await
            .context("list_topics failed")?
            .into_inner();
        Ok(resp.topics)
    }

    /// Start a subscription to a topic.
    /// Sends the Init message and initial CreditGrant, returns a Subscription handle.
    #[allow(dead_code)]
    pub async fn subscribe(
        &mut self,
        topic: String,
        consumer_group: Option<String>,
        consumer_id: Option<String>,
        initial_position: InitialPosition,
        credits_window: u32,
    ) -> Result<Subscription> {
        let (tx, rx) = mpsc::channel::<SubscribeUpstream>(32);

        // Send init message
        let init = SubscribeUpstream {
            request: Some(subscribe_upstream::Request::Init(SubscriptionInit {
                topic,
                consumer_group: consumer_group.unwrap_or_else(|| "default".to_string()),
                consumer_id: consumer_id.unwrap_or_default(),
                initial_position: initial_position.into(),
            })),
        };
        tx.send(init)
            .await
            .map_err(|_| anyhow!("failed to send init"))?;

        // Send initial credit grant
        let credit = SubscribeUpstream {
            request: Some(subscribe_upstream::Request::Credit(CreditGrant {
                credits: credits_window,
            })),
        };
        tx.send(credit)
            .await
            .map_err(|_| anyhow!("failed to send initial credits"))?;

        // Start the bidirectional stream
        let stream = ReceiverStream::new(rx);
        let response = self
            .inner
            .subscribe(stream)
            .await
            .context("subscribe RPC failed")?;

        Ok(Subscription {
            tx,
            rx: response.into_inner(),
            credits_window,
            remaining_credits: credits_window,
        })
    }

    /// Publish a message to a topic.
    #[allow(dead_code)]
    pub async fn publish(&mut self, topic: String, payload: Vec<u8>) -> Result<PublishResponse> {
        let resp = self
            .inner
            .publish(PublishRequest {
                topic,
                payload,
                attributes: Default::default(),
            })
            .await
            .context("publish failed")?
            .into_inner();
        Ok(resp)
    }
}
