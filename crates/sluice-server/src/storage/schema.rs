//! SQLite schema initialization and entity operations.
//!
//! Defines the database schema and provides CRUD operations for:
//! - Topics (auto-created on first publish)
//! - Messages (immutable after creation)
//! - Subscriptions (cursor tracking for consumer groups)

use rusqlite::{params, Connection, OptionalExtension, Result};

/// SQL schema for Sluice database.
///
/// Includes:
/// - Topics table (auto-created streams)
/// - Messages table (durable message storage)
/// - Subscriptions table (cursor tracking)
const SCHEMA: &str = r#"
-- Pragma configuration (applied separately on connection open)

-- Topics table
CREATE TABLE IF NOT EXISTS topics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

-- Messages table
CREATE TABLE IF NOT EXISTS messages (
    global_seq INTEGER PRIMARY KEY AUTOINCREMENT,
    topic_id INTEGER NOT NULL REFERENCES topics(id),
    message_id TEXT NOT NULL,
    payload BLOB,
    attributes TEXT,
    created_at INTEGER NOT NULL
);

-- Index for subscription seeking: fetch messages for topic after cursor
CREATE INDEX IF NOT EXISTS idx_messages_topic_seq
ON messages(topic_id, global_seq);

-- Subscriptions table
CREATE TABLE IF NOT EXISTS subscriptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    topic_id INTEGER NOT NULL REFERENCES topics(id),
    consumer_group TEXT NOT NULL,
    cursor_seq INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER,
    UNIQUE(topic_id, consumer_group)
);

-- Index for subscription lookups by topic and consumer group
CREATE INDEX IF NOT EXISTS idx_subscriptions_topic_group
ON subscriptions(topic_id, consumer_group);
"#;

/// Apply SQLite pragmas for optimal performance and durability.
///
/// Per research.md decision 1:
/// - WAL mode for concurrent readers
/// - synchronous=FULL for crash durability
pub fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = FULL;
        PRAGMA busy_timeout = 5000;
        PRAGMA cache_size = -2000;
        PRAGMA temp_store = MEMORY;
        "#,
    )
}

/// Apply read-only pragmas for reader connections.
pub fn apply_reader_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA busy_timeout = 5000;
        PRAGMA cache_size = -2000;
        "#,
    )
}

/// Initialize the database schema.
///
/// Creates tables and indexes if they don't exist.
pub fn initialize_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)
}

/// Topic entity for database operations.
#[derive(Debug, Clone)]
pub struct Topic {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
}

/// Message entity for database operations.
#[derive(Debug, Clone)]
pub struct Message {
    pub global_seq: i64,
    pub topic_id: i64,
    pub message_id: String,
    pub payload: Option<Vec<u8>>,
    pub attributes: Option<String>,
    pub created_at: i64,
}

/// Subscription entity for database operations.
#[derive(Debug, Clone)]
pub struct Subscription {
    pub id: i64,
    pub topic_id: i64,
    pub consumer_group: String,
    pub cursor_seq: i64,
    pub updated_at: Option<i64>,
}

/// Get or create a topic by name.
///
/// Uses INSERT OR IGNORE + SELECT pattern for atomic upsert.
/// Returns the topic ID.
pub fn insert_or_get_topic(conn: &Connection, name: &str, created_at: i64) -> Result<i64> {
    // Try to insert (will be ignored if exists due to UNIQUE constraint)
    conn.execute(
        "INSERT OR IGNORE INTO topics (name, created_at) VALUES (?1, ?2)",
        params![name, created_at],
    )?;

    // Fetch the ID (either newly created or existing)
    conn.query_row(
        "SELECT id FROM topics WHERE name = ?1",
        params![name],
        |row| row.get(0),
    )
}

/// Get a topic by name.
pub fn get_topic_by_name(conn: &Connection, name: &str) -> Result<Option<Topic>> {
    conn.query_row(
        "SELECT id, name, created_at FROM topics WHERE name = ?1",
        params![name],
        |row| {
            Ok(Topic {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
            })
        },
    )
    .optional()
}

/// Insert a message and return the global sequence number.
pub fn insert_message(
    conn: &Connection,
    topic_id: i64,
    message_id: &str,
    payload: Option<&[u8]>,
    attributes: Option<&str>,
    created_at: i64,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO messages (topic_id, message_id, payload, attributes, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![topic_id, message_id, payload, attributes, created_at],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Get or create a subscription, returning the subscription info.
pub fn get_or_create_subscription(
    conn: &Connection,
    topic_id: i64,
    consumer_group: &str,
    now: i64,
) -> Result<Subscription> {
    // Try to insert (will be ignored if exists)
    conn.execute(
        "INSERT OR IGNORE INTO subscriptions (topic_id, consumer_group, cursor_seq, updated_at) VALUES (?1, ?2, 0, ?3)",
        params![topic_id, consumer_group, now],
    )?;

    // Fetch the subscription
    conn.query_row(
        "SELECT id, topic_id, consumer_group, cursor_seq, updated_at FROM subscriptions WHERE topic_id = ?1 AND consumer_group = ?2",
        params![topic_id, consumer_group],
        |row| {
            Ok(Subscription {
                id: row.get(0)?,
                topic_id: row.get(1)?,
                consumer_group: row.get(2)?,
                cursor_seq: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
}

/// Update the cursor for a subscription.
///
/// Only advances the cursor if the new position is greater than current.
/// This ensures idempotent ACKs.
pub fn update_cursor(
    conn: &Connection,
    topic_id: i64,
    consumer_group: &str,
    cursor_seq: i64,
    now: i64,
) -> Result<usize> {
    conn.execute(
        "UPDATE subscriptions SET cursor_seq = ?1, updated_at = ?2 WHERE topic_id = ?3 AND consumer_group = ?4 AND cursor_seq < ?1",
        params![cursor_seq, now, topic_id, consumer_group],
    )
}

/// Fetch messages for a subscription starting after a given sequence.
pub fn fetch_messages_from_seq(
    conn: &Connection,
    topic_id: i64,
    after_seq: i64,
    limit: i64,
) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(
        "SELECT global_seq, topic_id, message_id, payload, attributes, created_at FROM messages WHERE topic_id = ?1 AND global_seq > ?2 ORDER BY global_seq ASC LIMIT ?3",
    )?;

    let rows = stmt.query_map(params![topic_id, after_seq, limit], |row| {
        Ok(Message {
            global_seq: row.get(0)?,
            topic_id: row.get(1)?,
            message_id: row.get(2)?,
            payload: row.get(3)?,
            attributes: row.get(4)?,
            created_at: row.get(5)?,
        })
    })?;

    rows.collect()
}

/// Get the maximum sequence number for a topic.
///
/// Used for LATEST initial position in subscriptions.
pub fn get_topic_max_seq(conn: &Connection, topic_id: i64) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(global_seq), 0) FROM messages WHERE topic_id = ?1",
        params![topic_id],
        |row| row.get(0),
    )
}

/// Look up a message by its message_id to get its sequence number.
pub fn get_message_seq_by_id(conn: &Connection, message_id: &str) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT global_seq FROM messages WHERE message_id = ?1",
        params![message_id],
        |row| row.get(0),
    )
    .optional()
}

/// Statistics about messages in a topic.
#[derive(Debug, Clone, Default)]
pub struct TopicMessageStats {
    pub total_messages: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub first_timestamp: Option<i64>,
    pub last_timestamp: Option<i64>,
}

/// Get message statistics for a topic.
///
/// Returns count, min/max sequence, and min/max timestamp.
pub fn get_topic_message_stats(conn: &Connection, topic_id: i64) -> Result<TopicMessageStats> {
    conn.query_row(
        r#"
        SELECT
            COUNT(*) as total,
            COALESCE(MIN(global_seq), 0) as first_seq,
            COALESCE(MAX(global_seq), 0) as last_seq,
            MIN(created_at) as first_ts,
            MAX(created_at) as last_ts
        FROM messages
        WHERE topic_id = ?1
        "#,
        params![topic_id],
        |row| {
            Ok(TopicMessageStats {
                total_messages: row.get::<_, i64>(0)? as u64,
                first_sequence: row.get::<_, i64>(1)? as u64,
                last_sequence: row.get::<_, i64>(2)? as u64,
                first_timestamp: row.get(3)?,
                last_timestamp: row.get(4)?,
            })
        },
    )
}

/// Information about a consumer group's position.
#[derive(Debug, Clone)]
pub struct ConsumerGroupInfo {
    pub group_name: String,
    pub cursor_seq: u64,
    pub updated_at: Option<i64>,
}

/// Get all consumer groups for a topic with their cursor positions.
pub fn get_consumer_groups_for_topic(
    conn: &Connection,
    topic_id: i64,
) -> Result<Vec<ConsumerGroupInfo>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT consumer_group, cursor_seq, updated_at
        FROM subscriptions
        WHERE topic_id = ?1
        ORDER BY consumer_group ASC
        "#,
    )?;

    let rows = stmt.query_map(params![topic_id], |row| {
        Ok(ConsumerGroupInfo {
            group_name: row.get(0)?,
            cursor_seq: row.get::<_, i64>(1)? as u64,
            updated_at: row.get(2)?,
        })
    })?;

    rows.collect()
}

/// Get all topics with their IDs.
pub fn get_all_topics(conn: &Connection) -> Result<Vec<Topic>> {
    let mut stmt = conn.prepare("SELECT id, name, created_at FROM topics ORDER BY name ASC")?;
    let rows = stmt.query_map([], |row| {
        Ok(Topic {
            id: row.get(0)?,
            name: row.get(1)?,
            created_at: row.get(2)?,
        })
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        apply_pragmas(&conn).unwrap();
        initialize_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn test_schema_initialization() {
        let conn = setup_test_db();
        // Verify tables exist by querying them
        let topic_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM topics", [], |row| row.get(0))
            .unwrap();
        assert_eq!(topic_count, 0);
    }

    #[test]
    fn test_insert_or_get_topic() {
        let conn = setup_test_db();
        let now = 1234567890000i64;

        // First insert creates the topic
        let id1 = insert_or_get_topic(&conn, "orders", now).unwrap();
        assert!(id1 > 0);

        // Second insert returns same ID
        let id2 = insert_or_get_topic(&conn, "orders", now + 1000).unwrap();
        assert_eq!(id1, id2);

        // Different topic gets different ID
        let id3 = insert_or_get_topic(&conn, "events", now).unwrap();
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_insert_message() {
        let conn = setup_test_db();
        let now = 1234567890000i64;

        let topic_id = insert_or_get_topic(&conn, "orders", now).unwrap();
        let seq = insert_message(
            &conn,
            topic_id,
            "msg-001",
            Some(b"hello"),
            Some(r#"{"key":"value"}"#),
            now,
        )
        .unwrap();

        assert_eq!(seq, 1);

        // Second message gets next sequence
        let seq2 = insert_message(&conn, topic_id, "msg-002", Some(b"world"), None, now).unwrap();
        assert_eq!(seq2, 2);
    }

    #[test]
    fn test_subscription_cursor() {
        let conn = setup_test_db();
        let now = 1234567890000i64;

        let topic_id = insert_or_get_topic(&conn, "orders", now).unwrap();
        let sub = get_or_create_subscription(&conn, topic_id, "workers", now).unwrap();

        assert_eq!(sub.cursor_seq, 0);

        // Update cursor
        let updated = update_cursor(&conn, topic_id, "workers", 5, now + 1000).unwrap();
        assert_eq!(updated, 1);

        // Fetch updated subscription
        let sub2 = get_or_create_subscription(&conn, topic_id, "workers", now + 2000).unwrap();
        assert_eq!(sub2.cursor_seq, 5);

        // Cursor only advances (idempotent)
        let updated = update_cursor(&conn, topic_id, "workers", 3, now + 3000).unwrap();
        assert_eq!(updated, 0); // No rows updated
    }

    #[test]
    fn test_fetch_messages() {
        let conn = setup_test_db();
        let now = 1234567890000i64;

        let topic_id = insert_or_get_topic(&conn, "orders", now).unwrap();

        // Insert some messages
        for i in 1..=5 {
            insert_message(
                &conn,
                topic_id,
                &format!("msg-{i:03}"),
                Some(format!("payload-{i}").as_bytes()),
                None,
                now + i,
            )
            .unwrap();
        }

        // Fetch from beginning
        let messages = fetch_messages_from_seq(&conn, topic_id, 0, 10).unwrap();
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0].global_seq, 1);
        assert_eq!(messages[4].global_seq, 5);

        // Fetch with cursor
        let messages = fetch_messages_from_seq(&conn, topic_id, 2, 10).unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].global_seq, 3);
    }
}
