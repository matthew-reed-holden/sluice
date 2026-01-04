//! Dedicated writer thread for SQLite operations.
//!
//! Implements the dedicated writer pattern per research.md decision 2:
//! - Single std::thread owns the write connection
//! - Communication via tokio::sync::mpsc channel
//! - Enables group commit for high throughput

use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;
use std::thread::{self, JoinHandle};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use super::batch::{BatchAccumulator, BatchConfig};
use super::schema::{
    apply_pragmas, get_or_create_subscription, initialize_schema, insert_message,
    insert_or_get_topic, update_cursor, Subscription,
};
use crate::flow::notify::NotificationBus;
use crate::now_millis;

/// Error type for writer operations.
#[derive(Debug, Error)]
pub enum WriterError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("Writer channel closed")]
    ChannelClosed,

    #[error("Writer thread panicked")]
    ThreadPanic,
}

/// Result of a publish operation.
#[derive(Debug, Clone)]
pub struct PublishResult {
    pub message_id: String,
    pub sequence: i64,
    pub timestamp: i64,
}

/// Command sent to the writer thread.
pub struct PublishCommand {
    pub topic: String,
    pub message_id: String,
    pub payload: Option<Vec<u8>>,
    pub attributes: Option<String>,
    pub reply: oneshot::Sender<Result<PublishResult, WriterError>>,
}

/// Command to get or create a subscription.
pub struct SubscriptionCommand {
    pub topic_id: i64,
    pub consumer_group: String,
    pub reply: oneshot::Sender<Result<Subscription, WriterError>>,
}

/// Command to update a cursor position.
pub struct CursorUpdateCommand {
    pub topic_id: i64,
    pub consumer_group: String,
    pub cursor_seq: i64,
    pub reply: oneshot::Sender<Result<(), WriterError>>,
}

/// Handle to the writer thread.
///
/// Provides async interface to submit write operations.
#[derive(Clone)]
pub struct WriterHandle {
    sender: mpsc::Sender<WriterMessage>,
}

enum WriterMessage {
    Publish(PublishCommand),
    GetOrCreateSubscription(SubscriptionCommand),
    UpdateCursor(CursorUpdateCommand),
    Shutdown,
}

impl WriterHandle {
    /// Submit a publish command and wait for the result.
    pub async fn publish(
        &self,
        topic: String,
        message_id: String,
        payload: Option<Vec<u8>>,
        attributes: Option<String>,
    ) -> Result<PublishResult, WriterError> {
        let (reply_tx, reply_rx) = oneshot::channel();

        let cmd = PublishCommand {
            topic,
            message_id,
            payload,
            attributes,
            reply: reply_tx,
        };

        self.sender
            .send(WriterMessage::Publish(cmd))
            .await
            .map_err(|_| WriterError::ChannelClosed)?;

        reply_rx.await.map_err(|_| WriterError::ChannelClosed)?
    }

    /// Get or create a subscription.
    pub async fn get_or_create_subscription(
        &self,
        topic_id: i64,
        consumer_group: String,
    ) -> Result<Subscription, WriterError> {
        let (reply_tx, reply_rx) = oneshot::channel();

        let cmd = SubscriptionCommand {
            topic_id,
            consumer_group,
            reply: reply_tx,
        };

        self.sender
            .send(WriterMessage::GetOrCreateSubscription(cmd))
            .await
            .map_err(|_| WriterError::ChannelClosed)?;

        reply_rx.await.map_err(|_| WriterError::ChannelClosed)?
    }

    /// Update the cursor for a subscription.
    pub async fn update_cursor(
        &self,
        topic_id: i64,
        consumer_group: String,
        cursor_seq: i64,
    ) -> Result<(), WriterError> {
        let (reply_tx, reply_rx) = oneshot::channel();

        let cmd = CursorUpdateCommand {
            topic_id,
            consumer_group,
            cursor_seq,
            reply: reply_tx,
        };

        self.sender
            .send(WriterMessage::UpdateCursor(cmd))
            .await
            .map_err(|_| WriterError::ChannelClosed)?;

        reply_rx.await.map_err(|_| WriterError::ChannelClosed)?
    }

    /// Request graceful shutdown of the writer thread.
    pub async fn shutdown(&self) -> Result<(), WriterError> {
        self.sender
            .send(WriterMessage::Shutdown)
            .await
            .map_err(|_| WriterError::ChannelClosed)
    }
}

/// Writer thread that owns the SQLite write connection.
pub struct Writer {
    handle: Option<JoinHandle<()>>,
    sender: mpsc::Sender<WriterMessage>,
}

impl Writer {
    /// Spawn a new writer thread.
    ///
    /// # Arguments
    ///
    /// * `db_path` - Path to the SQLite database
    /// * `notify_bus` - Notification bus for waking subscriptions
    /// * `channel_size` - Size of the command channel (backpressure control)
    pub fn spawn<P: AsRef<Path>>(
        db_path: P,
        notify_bus: NotificationBus,
        channel_size: usize,
    ) -> Result<Self, WriterError> {
        let db_path = db_path.as_ref().to_path_buf();
        let (sender, receiver) = mpsc::channel(channel_size);

        let handle = thread::Builder::new()
            .name("sluice-writer".into())
            .spawn(move || {
                if let Err(e) = writer_thread_main(db_path, receiver, notify_bus) {
                    tracing::error!(error = %e, "Writer thread error");
                }
            })
            .map_err(|e| WriterError::Database(e.to_string()))?;

        Ok(Self {
            handle: Some(handle),
            sender,
        })
    }

    /// Get a handle for submitting commands.
    pub fn handle(&self) -> WriterHandle {
        WriterHandle {
            sender: self.sender.clone(),
        }
    }

    /// Wait for the writer thread to complete.
    pub fn join(mut self) -> Result<(), WriterError> {
        if let Some(handle) = self.handle.take() {
            handle.join().map_err(|_| WriterError::ThreadPanic)?;
        }
        Ok(())
    }
}

/// Main function for the writer thread.
fn writer_thread_main(
    db_path: std::path::PathBuf,
    mut receiver: mpsc::Receiver<WriterMessage>,
    notify_bus: NotificationBus,
) -> Result<(), WriterError> {
    // Open database connection
    let conn = Connection::open(&db_path).map_err(|e| WriterError::Database(e.to_string()))?;

    // Apply pragmas and initialize schema
    apply_pragmas(&conn).map_err(|e| WriterError::Database(e.to_string()))?;
    initialize_schema(&conn).map_err(|e| WriterError::Database(e.to_string()))?;

    tracing::info!(path = ?db_path, "Writer thread started");

    // Topic ID cache
    let mut topic_cache: HashMap<String, i64> = HashMap::new();

    // Batch accumulator
    let mut batch: BatchAccumulator<PublishCommand> = BatchAccumulator::new(BatchConfig::default());

    loop {
        // Determine how to wait for the next message
        let msg = if batch.is_empty() {
            // No pending batch, block indefinitely for first message
            receiver.blocking_recv()
        } else if batch.is_ready() {
            // Batch ready, try to receive without blocking
            receiver.try_recv().ok()
        } else {
            // Have pending items but batch not ready yet
            // Poll with short sleeps until ready or new message arrives
            let poll_interval = Duration::from_millis(1);
            loop {
                match receiver.try_recv() {
                    Ok(msg) => break Some(msg),
                    Err(mpsc::error::TryRecvError::Empty) => {
                        if batch.is_ready() {
                            break None; // Time to flush
                        }
                        std::thread::sleep(poll_interval);
                    }
                    Err(mpsc::error::TryRecvError::Disconnected) => break None,
                }
            }
        };

        match msg {
            Some(WriterMessage::Publish(cmd)) => {
                let ready = batch.push(cmd);
                if ready {
                    flush_batch(&conn, &mut batch, &mut topic_cache, &notify_bus)?;
                }
            }
            Some(WriterMessage::GetOrCreateSubscription(cmd)) => {
                // Flush pending batch first to ensure consistency
                if !batch.is_empty() {
                    flush_batch(&conn, &mut batch, &mut topic_cache, &notify_bus)?;
                }
                let result =
                    get_or_create_subscription(&conn, cmd.topic_id, &cmd.consumer_group, now_millis())
                        .map_err(|e| WriterError::Database(e.to_string()));
                let _ = cmd.reply.send(result);
            }
            Some(WriterMessage::UpdateCursor(cmd)) => {
                // Flush pending batch first to ensure consistency
                if !batch.is_empty() {
                    flush_batch(&conn, &mut batch, &mut topic_cache, &notify_bus)?;
                }
                let result =
                    update_cursor(&conn, cmd.topic_id, &cmd.consumer_group, cmd.cursor_seq, now_millis())
                        .map(|_| ())
                        .map_err(|e| WriterError::Database(e.to_string()));
                let _ = cmd.reply.send(result);
            }
            Some(WriterMessage::Shutdown) => {
                tracing::info!("Writer thread shutting down");
                // Flush remaining batch
                if !batch.is_empty() {
                    flush_batch(&conn, &mut batch, &mut topic_cache, &notify_bus)?;
                }
                break;
            }
            None => {
                // Timeout or channel closed - check if batch needs flushing
                if !batch.is_empty() {
                    flush_batch(&conn, &mut batch, &mut topic_cache, &notify_bus)?;
                }
                if receiver.is_closed() {
                    tracing::info!("Writer channel closed, exiting");
                    break;
                }
            }
        }
    }

    tracing::info!("Writer thread exited");
    Ok(())
}

use std::time::Duration;

/// Flush the accumulated batch in a single transaction.
fn flush_batch(
    conn: &Connection,
    batch: &mut BatchAccumulator<PublishCommand>,
    topic_cache: &mut HashMap<String, i64>,
    notify_bus: &NotificationBus,
) -> Result<(), WriterError> {
    let commands = batch.drain();
    if commands.is_empty() {
        return Ok(());
    }

    let now = now_millis();
    let batch_size = commands.len();

    tracing::debug!(batch_size, "Flushing batch");

    // Track max sequence per topic for notifications
    let mut topic_max_seq: HashMap<i64, i64> = HashMap::new();

    // Execute batch in a transaction
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| WriterError::Database(e.to_string()))?;

    for cmd in commands {
        // Get or create topic
        let topic_id = match topic_cache.get(&cmd.topic) {
            Some(&id) => id,
            None => {
                let id = insert_or_get_topic(&tx, &cmd.topic, now)
                    .map_err(|e| WriterError::Database(e.to_string()))?;
                topic_cache.insert(cmd.topic.clone(), id);
                id
            }
        };

        // Insert message
        let seq = insert_message(
            &tx,
            topic_id,
            &cmd.message_id,
            cmd.payload.as_deref(),
            cmd.attributes.as_deref(),
            now,
        )
        .map_err(|e| WriterError::Database(e.to_string()))?;

        // Track max sequence for topic
        topic_max_seq
            .entry(topic_id)
            .and_modify(|max| *max = (*max).max(seq))
            .or_insert(seq);

        // Send reply
        let result = PublishResult {
            message_id: cmd.message_id,
            sequence: seq,
            timestamp: now,
        };
        let _ = cmd.reply.send(Ok(result));
    }

    // Commit transaction (single fsync for entire batch)
    tx.commit()
        .map_err(|e| WriterError::Database(e.to_string()))?;

    tracing::debug!(batch_size, "Batch committed");

    // Notify subscribers
    for (topic_id, max_seq) in topic_max_seq {
        notify_bus.notify(topic_id, max_seq);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_writer_publish() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let notify_bus = NotificationBus::new(16);

        let writer = Writer::spawn(&db_path, notify_bus.clone(), 100).unwrap();
        let handle = writer.handle();

        // Publish a message
        let result = handle
            .publish(
                "orders".into(),
                "msg-001".into(),
                Some(b"hello".to_vec()),
                None,
            )
            .await
            .unwrap();

        assert_eq!(result.message_id, "msg-001");
        assert_eq!(result.sequence, 1);
        assert!(result.timestamp > 0);

        // Publish another message
        let result2 = handle
            .publish(
                "orders".into(),
                "msg-002".into(),
                Some(b"world".to_vec()),
                None,
            )
            .await
            .unwrap();

        assert_eq!(result2.sequence, 2);

        // Shutdown
        handle.shutdown().await.unwrap();
        writer.join().unwrap();
    }
}
