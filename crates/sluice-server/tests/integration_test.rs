//! Integration tests for durability and crash recovery.
//!
//! Tests:
//! - T018: Message survives process termination
//! - T042: Cursor persistence across restart

mod common;

use futures::StreamExt;
use sluice_server::proto::sluice::v1::{
    subscribe_downstream::Response as DownstreamResponse,
    subscribe_upstream::Request as UpstreamRequest, Ack, CreditGrant, DeleteConsumerGroupRequest,
    DeleteTopicRequest, InitialPosition, PublishRequest, ResetCursorRequest, SubscribeUpstream,
    SubscriptionInit,
};
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::timeout;

fn make_publish(topic: &str, payload: &[u8]) -> PublishRequest {
    PublishRequest {
        topic: topic.to_string(),
        payload: payload.to_vec(),
        attributes: HashMap::new(),
    }
}

fn make_init(topic: &str, consumer_group: &str, position: InitialPosition) -> SubscribeUpstream {
    SubscribeUpstream {
        request: Some(UpstreamRequest::Init(SubscriptionInit {
            topic: topic.to_string(),
            consumer_group: consumer_group.to_string(),
            consumer_id: "test".to_string(),
            initial_position: position as i32,
            offset: 0,
            mode: 0, // CONSUMER_GROUP
        })),
    }
}

fn make_credit(credits: u32) -> SubscribeUpstream {
    SubscribeUpstream {
        request: Some(UpstreamRequest::Credit(CreditGrant { credits })),
    }
}

fn make_ack(message_id: &str) -> SubscribeUpstream {
    SubscribeUpstream {
        request: Some(UpstreamRequest::Ack(Ack {
            message_id: message_id.to_string(),
        })),
    }
}

/// T018: Message survives server restart (simulated by starting new server on same DB).
///
/// This tests persistence - after publishing, stopping, and restarting,
/// the message should still be available.
#[tokio::test]
async fn test_message_survives_restart() {
    // Use a fixed temp dir that persists across "restarts"
    let temp_dir = tempfile::TempDir::new().expect("failed to create temp dir");
    let data_dir = temp_dir.path().to_path_buf();

    let message_id: String;
    let topic = "durability-topic";

    // Phase 1: Start server, publish message, shutdown
    {
        let config = sluice_server::config::Config {
            data_dir: data_dir.clone(),
            ..Default::default()
        };

        let server = common::TestServer::start_with_config(config).await;
        let mut client = server.client().await;

        // Publish a message
        let resp = client
            .publish(make_publish(topic, b"durable message"))
            .await
            .expect("publish failed")
            .into_inner();
        message_id = resp.message_id;

        // Graceful shutdown
        server.shutdown().await;
    }

    // Phase 2: Start new server on same database, verify message exists
    {
        let config = sluice_server::config::Config {
            data_dir,
            ..Default::default()
        };

        let server = common::TestServer::start_with_config(config).await;
        let mut client = server.client().await;

        // Subscribe and verify message is present
        let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

        tx.send(make_init(
            topic,
            "durability-group",
            InitialPosition::Earliest,
        ))
        .await
        .unwrap();
        tx.send(make_credit(10)).await.unwrap();

        let response = client.subscribe(stream).await.expect("subscribe failed");
        let mut stream = response.into_inner();

        // Should receive the message that was published before restart
        let delivery = timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("stream error");

        if let Some(DownstreamResponse::Delivery(d)) = delivery.response {
            assert_eq!(
                d.message_id, message_id,
                "message should persist across restart"
            );
            assert_eq!(d.payload, b"durable message");
        } else {
            panic!("expected delivery");
        }

        drop(tx);
        server.shutdown().await;
    }
}

/// T042: Cursor persists across restart - resume from ACKed position.
#[tokio::test]
async fn test_cursor_persists_across_restart() {
    let temp_dir = tempfile::TempDir::new().expect("failed to create temp dir");
    let data_dir = temp_dir.path().to_path_buf();

    let topic = "cursor-topic";
    let consumer_group = "cursor-group";

    // Phase 1: Publish messages, consume and ACK first one
    {
        let config = sluice_server::config::Config {
            data_dir: data_dir.clone(),
            ..Default::default()
        };

        let server = common::TestServer::start_with_config(config).await;
        let mut client = server.client().await;

        // Publish two messages
        client
            .publish(make_publish(topic, b"message 1"))
            .await
            .expect("publish 1 failed");
        client
            .publish(make_publish(topic, b"message 2"))
            .await
            .expect("publish 2 failed");

        // Subscribe and consume first message
        let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

        tx.send(make_init(topic, consumer_group, InitialPosition::Earliest))
            .await
            .unwrap();
        tx.send(make_credit(1)).await.unwrap();

        let response = client.subscribe(stream).await.expect("subscribe failed");
        let mut stream = response.into_inner();

        // Receive and ACK first message
        let delivery = timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("stream error");

        if let Some(DownstreamResponse::Delivery(d)) = delivery.response {
            assert_eq!(d.payload, b"message 1");
            tx.send(make_ack(&d.message_id)).await.unwrap();
        }

        // Wait for ACK to be processed
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(tx);
        server.shutdown().await;
    }

    // Phase 2: Restart and verify we resume from message 2
    {
        let config = sluice_server::config::Config {
            data_dir,
            ..Default::default()
        };

        let server = common::TestServer::start_with_config(config).await;
        let mut client = server.client().await;

        // Subscribe with same consumer group - should resume from cursor
        let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

        tx.send(make_init(topic, consumer_group, InitialPosition::Earliest))
            .await
            .unwrap();
        tx.send(make_credit(10)).await.unwrap();

        let response = client.subscribe(stream).await.expect("subscribe failed");
        let mut stream = response.into_inner();

        // Should get message 2 (message 1 was ACKed)
        let delivery = timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("stream error");

        if let Some(DownstreamResponse::Delivery(d)) = delivery.response {
            assert_eq!(d.payload, b"message 2", "should resume from ACKed position");
        } else {
            panic!("expected delivery");
        }

        drop(tx);
        server.shutdown().await;
    }
}

/// T030: Slow consumer does not block producer (backpressure isolation).
#[tokio::test]
async fn test_slow_consumer_does_not_block_producer() {
    let server = common::TestServer::start().await;
    let mut client = server.client().await;
    let mut producer_client = server.client().await;

    let topic = "backpressure-topic";

    // Start a consumer but don't grant credits (simulating slow consumer)
    let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    // First publish a message so topic exists
    producer_client
        .publish(make_publish(topic, b"initial"))
        .await
        .expect("initial publish failed");

    tx.send(make_init(topic, "slow-group", InitialPosition::Earliest))
        .await
        .unwrap();
    // Note: NOT granting credits - consumer is blocked

    let _response = client.subscribe(stream).await.expect("subscribe failed");

    // Producer should still be able to publish (not blocked by slow consumer)
    for i in 0..10 {
        let result = timeout(
            Duration::from_millis(500),
            producer_client.publish(make_publish(topic, format!("msg-{i}").as_bytes())),
        )
        .await;

        assert!(
            result.is_ok(),
            "producer should not be blocked by slow consumer"
        );
        assert!(result.unwrap().is_ok(), "publish should succeed");
    }

    drop(tx);
    server.shutdown().await;
}

/// Admin: DeleteTopic removes topic, messages, and subscriptions.
#[tokio::test]
async fn test_delete_topic() {
    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    let topic = "delete-me";

    // Publish some messages
    client
        .publish(make_publish(topic, b"msg1"))
        .await
        .expect("publish failed");
    client
        .publish(make_publish(topic, b"msg2"))
        .await
        .expect("publish failed");

    // Create a subscription by subscribing
    let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    tx.send(make_init(topic, "my-group", InitialPosition::Earliest))
        .await
        .unwrap();
    tx.send(make_credit(1)).await.unwrap();

    let response = client.subscribe(stream).await.expect("subscribe failed");
    let mut sub_stream = response.into_inner();

    // Receive one message to confirm subscription works
    let delivery = timeout(Duration::from_secs(2), sub_stream.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("stream error");
    assert!(matches!(
        delivery.response,
        Some(DownstreamResponse::Delivery(_))
    ));
    drop(tx);

    // Delete the topic
    let resp = client
        .delete_topic(DeleteTopicRequest {
            topic: topic.to_string(),
        })
        .await
        .expect("delete_topic failed")
        .into_inner();
    assert!(resp.deleted);

    // Deleting again should return false
    let resp = client
        .delete_topic(DeleteTopicRequest {
            topic: topic.to_string(),
        })
        .await
        .expect("delete_topic failed")
        .into_inner();
    assert!(!resp.deleted);

    // Publishing to the deleted topic should auto-create it again
    let resp = client
        .publish(make_publish(topic, b"new msg"))
        .await
        .expect("publish after delete failed")
        .into_inner();
    // Note: global_seq is AUTOINCREMENT and doesn't reset on topic deletion,
    // but the publish should succeed and return a valid sequence.
    assert!(resp.sequence > 0);

    server.shutdown().await;
}

/// Admin: DeleteConsumerGroup removes subscription but preserves topic.
#[tokio::test]
async fn test_delete_consumer_group() {
    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    let topic = "cg-topic";

    // Publish messages
    client
        .publish(make_publish(topic, b"msg1"))
        .await
        .expect("publish failed");
    client
        .publish(make_publish(topic, b"msg2"))
        .await
        .expect("publish failed");

    // Subscribe with group-a and group-b, consume and ack msg1
    for group in &["group-a", "group-b"] {
        let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        tx.send(make_init(topic, group, InitialPosition::Earliest))
            .await
            .unwrap();
        tx.send(make_credit(1)).await.unwrap();

        let response = client.subscribe(stream).await.expect("subscribe failed");
        let mut sub_stream = response.into_inner();

        // Consume and ack one message
        let delivery = timeout(Duration::from_secs(2), sub_stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("stream error");

        if let Some(DownstreamResponse::Delivery(d)) = delivery.response {
            tx.send(make_ack(&d.message_id)).await.unwrap();
        }

        // Wait for ack to process
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(tx);
    }

    // Delete group-a
    let resp = client
        .delete_consumer_group(DeleteConsumerGroupRequest {
            topic: topic.to_string(),
            consumer_group: "group-a".to_string(),
        })
        .await
        .expect("delete_consumer_group failed")
        .into_inner();
    assert!(resp.deleted);

    // Deleting again should return false
    let resp = client
        .delete_consumer_group(DeleteConsumerGroupRequest {
            topic: topic.to_string(),
            consumer_group: "group-a".to_string(),
        })
        .await
        .expect("delete_consumer_group failed")
        .into_inner();
    assert!(!resp.deleted);

    // group-b should still work — cursor at msg1, should get msg2
    let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    tx.send(make_init(topic, "group-b", InitialPosition::Earliest))
        .await
        .unwrap();
    tx.send(make_credit(10)).await.unwrap();

    let response = client.subscribe(stream).await.expect("subscribe failed");
    let mut sub_stream = response.into_inner();

    let delivery = timeout(Duration::from_secs(2), sub_stream.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("stream error");

    // group-b acked msg1, so should get msg2 next
    if let Some(DownstreamResponse::Delivery(d)) = delivery.response {
        assert_eq!(d.payload, b"msg2");
    } else {
        panic!("expected delivery for group-b");
    }

    drop(tx);
    server.shutdown().await;
}

/// Admin: ResetCursor allows reprocessing from a specific sequence.
#[tokio::test]
async fn test_reset_cursor() {
    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    let topic = "reset-topic";
    let consumer_group = "reset-group";

    // Publish 3 messages
    for i in 1..=3 {
        client
            .publish(make_publish(topic, format!("msg{i}").as_bytes()))
            .await
            .expect("publish failed");
    }

    // Subscribe, consume and ack all 3
    {
        let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        tx.send(make_init(topic, consumer_group, InitialPosition::Earliest))
            .await
            .unwrap();
        tx.send(make_credit(10)).await.unwrap();

        let response = client.subscribe(stream).await.expect("subscribe failed");
        let mut sub_stream = response.into_inner();

        for _ in 0..3 {
            let delivery = timeout(Duration::from_secs(2), sub_stream.next())
                .await
                .expect("timeout")
                .expect("stream ended")
                .expect("stream error");

            if let Some(DownstreamResponse::Delivery(d)) = delivery.response {
                tx.send(make_ack(&d.message_id)).await.unwrap();
            }
        }

        // Wait for acks to process
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(tx);
    }

    // Reset cursor to sequence 1 (reprocess from message 2)
    let resp = client
        .reset_cursor(ResetCursorRequest {
            topic: topic.to_string(),
            consumer_group: consumer_group.to_string(),
            sequence: 1,
        })
        .await
        .expect("reset_cursor failed")
        .into_inner();
    assert!(resp.updated);

    // Subscribe again — should get msg2 and msg3
    {
        let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        tx.send(make_init(topic, consumer_group, InitialPosition::Earliest))
            .await
            .unwrap();
        tx.send(make_credit(10)).await.unwrap();

        let response = client.subscribe(stream).await.expect("subscribe failed");
        let mut sub_stream = response.into_inner();

        // Should get msg2 (sequence after cursor=1)
        let delivery = timeout(Duration::from_secs(2), sub_stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("stream error");

        if let Some(DownstreamResponse::Delivery(d)) = delivery.response {
            assert_eq!(d.payload, b"msg2", "should resume from reset position");
        } else {
            panic!("expected delivery after cursor reset");
        }

        drop(tx);
    }

    // Reset non-existent group should return false
    let resp = client
        .reset_cursor(ResetCursorRequest {
            topic: topic.to_string(),
            consumer_group: "nonexistent".to_string(),
            sequence: 0,
        })
        .await
        .expect("reset_cursor failed")
        .into_inner();
    assert!(!resp.updated);

    server.shutdown().await;
}
