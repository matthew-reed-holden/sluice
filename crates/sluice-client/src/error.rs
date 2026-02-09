//! Typed error types for the Sluice client.

use thiserror::Error;

/// Errors that can occur when using the Sluice client.
#[derive(Debug, Error)]
pub enum SluiceError {
    /// Failed to establish a connection to the server.
    #[error("connection failed to {endpoint}: {source}")]
    ConnectionFailed {
        endpoint: String,
        #[source]
        source: tonic::transport::Error,
    },

    /// An RPC call returned an error status.
    #[error("{method} RPC failed: {source}")]
    RpcFailed {
        method: &'static str,
        #[source]
        source: tonic::Status,
    },

    /// The subscription stream was closed by the server.
    #[error("subscription stream closed")]
    SubscriptionClosed,

    /// The client configuration is invalid.
    #[error("invalid configuration: {message}")]
    InvalidConfig { message: String },

    /// An internal channel was closed unexpectedly.
    #[error("channel closed")]
    ChannelClosed,

    /// An I/O error occurred (e.g., reading TLS certificates).
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

/// Convenience type alias for results using [`SluiceError`].
pub type Result<T> = std::result::Result<T, SluiceError>;
