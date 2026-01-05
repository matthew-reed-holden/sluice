//! Operation result types for Sluice client.

/// Result of a publish operation.
#[derive(Debug, Clone)]
pub struct PublishResult {
    /// The server-assigned message ID.
    pub message_id: String,
    /// The local monotonic sequence number for this topic.
    pub sequence: u64,
}
