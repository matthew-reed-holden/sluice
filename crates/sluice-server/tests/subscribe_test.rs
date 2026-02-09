//! Contract tests for the Subscribe RPC.
//!
//! Tests:
//! - T027: SubscriptionInit establishes stream
//! - T028: No delivery without credits (flow control)
//! - T029: EARLIEST/LATEST initial positions
//! - T041: Ack updates cursor
//! - T043: Duplicate ACK is idempotent

mod common;

use futures::StreamExt;
use sluice_server::proto::sluice::v1::{
    subscribe_downstream::Response as DownstreamResponse,
    subscribe_upstream::Request as UpstreamRequest, Ack, CreditGrant, InitialPosition,
    PublishRequest, SubscribeUpstream, SubscriptionInit,
};
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::timeout;

/// Helper to create a subscription init message.
fn make_init(topic: &str, consumer_group: &str, position: InitialPosition) -> SubscribeUpstream {
    SubscribeUpstream {
        request: Some(UpstreamRequest::Init(SubscriptionInit {
            topic: topic.to_string(),
            consumer_group: consumer_group.to_string(),
            consumer_id: "test-consumer".to_string(),
            initial_position: position as i32,
            offset: 0,
            mode: 0, // CONSUMER_GROUP
        })),
    }
}

/// Helper to create a credit grant message.
fn make_credit(credits: u32) -> SubscribeUpstream {
    SubscribeUpstream {
        request: Some(UpstreamRequest::Credit(CreditGrant { credits })),
    }
}

/// Helper to create an ACK message.
fn make_ack(message_id: &str) -> SubscribeUpstream {
    SubscribeUpstream {
        request: Some(UpstreamRequest::Ack(Ack {
            message_id: message_id.to_string(),
        })),
    }
}

/// Helper to create a publish request.
fn make_publish(topic: &str, payload: &[u8]) -> PublishRequest {
    PublishRequest {
        topic: topic.to_string(),
        payload: payload.to_vec(),
        attributes: HashMap::new(),
    }
}

/// T027: SubscriptionInit establishes stream.
#[tokio::test]
async fn test_subscribe_init_establishes_stream() {
    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    // Publish a message first
    let pub_resp = client
        .publish(make_publish("sub-topic-1", b"test message"))
        .await
        .expect("publish failed")
        .into_inner();

    // Create request stream
    let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    // Send init
    tx.send(make_init(
        "sub-topic-1",
        "test-group",
        InitialPosition::Earliest,
    ))
    .await
    .expect("send init failed");

    // Send credits
    tx.send(make_credit(10)).await.expect("send credits failed");

    // Start subscription
    let response = client.subscribe(stream).await.expect("subscribe failed");
    let mut stream = response.into_inner();

    // Should receive the message
    let msg = timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timeout waiting for message")
        .expect("stream ended")
        .expect("stream error");

    if let Some(DownstreamResponse::Delivery(delivery)) = msg.response {
        assert_eq!(delivery.message_id, pub_resp.message_id);
        assert_eq!(delivery.payload, b"test message");
    } else {
        panic!("expected MessageDelivery, got {:?}", msg.response);
    }

    drop(tx);
    server.shutdown().await;
}

/// T028: No delivery without credits (flow control).
#[tokio::test]
async fn test_subscribe_no_delivery_without_credits() {
    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    // Publish a message
    client
        .publish(make_publish("credit-topic", b"waiting message"))
        .await
        .expect("publish failed");

    // Create request stream
    let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    // Send init but NO credits
    tx.send(make_init(
        "credit-topic",
        "no-credit-group",
        InitialPosition::Earliest,
    ))
    .await
    .expect("send init failed");

    // Start subscription
    let response = client.subscribe(stream).await.expect("subscribe failed");
    let mut stream = response.into_inner();

    // Should NOT receive any message (no credits)
    let result = timeout(Duration::from_millis(200), stream.next()).await;
    assert!(
        result.is_err(),
        "should timeout without credits, got message"
    );

    // Now grant credits
    tx.send(make_credit(1)).await.expect("send credits failed");

    // Should now receive the message
    let msg = timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timeout after granting credits")
        .expect("stream ended")
        .expect("stream error");

    assert!(
        matches!(msg.response, Some(DownstreamResponse::Delivery(_))),
        "expected delivery after credits"
    );

    drop(tx);
    server.shutdown().await;
}

/// T029: EARLIEST position starts from oldest message.
#[tokio::test]
async fn test_subscribe_earliest_position() {
    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    // Publish messages before subscribing
    let msg1 = client
        .publish(make_publish("earliest-topic", b"first"))
        .await
        .expect("publish 1 failed")
        .into_inner();
    let msg2 = client
        .publish(make_publish("earliest-topic", b"second"))
        .await
        .expect("publish 2 failed")
        .into_inner();

    // Subscribe with EARLIEST
    let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    tx.send(make_init(
        "earliest-topic",
        "earliest-group",
        InitialPosition::Earliest,
    ))
    .await
    .expect("send init failed");
    tx.send(make_credit(10)).await.expect("send credits failed");

    let response = client.subscribe(stream).await.expect("subscribe failed");
    let mut stream = response.into_inner();

    // Should receive messages in order, starting from first
    let delivery1 = timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("stream error");
    if let Some(DownstreamResponse::Delivery(d)) = delivery1.response {
        assert_eq!(d.message_id, msg1.message_id, "should get first message");
        assert_eq!(d.payload, b"first");
    } else {
        panic!("expected delivery");
    }

    let delivery2 = timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("stream error");
    if let Some(DownstreamResponse::Delivery(d)) = delivery2.response {
        assert_eq!(d.message_id, msg2.message_id, "should get second message");
        assert_eq!(d.payload, b"second");
    } else {
        panic!("expected delivery");
    }

    drop(tx);
    server.shutdown().await;
}

/// T029: LATEST position only receives new messages.
#[tokio::test]
async fn test_subscribe_latest_position() {
    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    // Publish a message BEFORE subscribing
    client
        .publish(make_publish("latest-topic", b"old message"))
        .await
        .expect("publish failed");

    // Subscribe with LATEST
    let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    tx.send(make_init(
        "latest-topic",
        "latest-group",
        InitialPosition::Latest,
    ))
    .await
    .expect("send init failed");
    tx.send(make_credit(10)).await.expect("send credits failed");

    let response = client.subscribe(stream).await.expect("subscribe failed");
    let mut stream = response.into_inner();

    // Should NOT receive the old message
    let result = timeout(Duration::from_millis(200), stream.next()).await;
    assert!(
        result.is_err(),
        "should not receive old message with LATEST"
    );

    // Publish a NEW message
    let new_msg = client
        .publish(make_publish("latest-topic", b"new message"))
        .await
        .expect("publish failed")
        .into_inner();

    // Should receive the new message
    let delivery = timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timeout for new message")
        .expect("stream ended")
        .expect("stream error");

    if let Some(DownstreamResponse::Delivery(d)) = delivery.response {
        assert_eq!(d.message_id, new_msg.message_id);
        assert_eq!(d.payload, b"new message");
    } else {
        panic!("expected delivery of new message");
    }

    drop(tx);
    server.shutdown().await;
}

/// T041: Ack updates cursor.
#[tokio::test]
async fn test_subscribe_ack_updates_cursor() {
    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    // Publish two messages
    let msg1 = client
        .publish(make_publish("ack-topic", b"message 1"))
        .await
        .expect("publish 1 failed")
        .into_inner();
    client
        .publish(make_publish("ack-topic", b"message 2"))
        .await
        .expect("publish 2 failed");

    // First subscription - consume and ACK first message
    {
        let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

        tx.send(make_init(
            "ack-topic",
            "ack-group",
            InitialPosition::Earliest,
        ))
        .await
        .unwrap();
        tx.send(make_credit(1)).await.unwrap();

        let response = client.subscribe(stream).await.expect("subscribe failed");
        let mut stream = response.into_inner();

        // Receive first message
        let delivery = timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("stream error");

        if let Some(DownstreamResponse::Delivery(d)) = delivery.response {
            assert_eq!(d.message_id, msg1.message_id);
            // ACK the message
            tx.send(make_ack(&d.message_id)).await.unwrap();
        }

        // Small delay to ensure ACK is processed
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(tx);
    }

    // Second subscription - should resume after ACKed message
    {
        let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

        // Same consumer group - should resume from cursor
        tx.send(make_init(
            "ack-topic",
            "ack-group",
            InitialPosition::Earliest,
        ))
        .await
        .unwrap();
        tx.send(make_credit(10)).await.unwrap();

        let response = client.subscribe(stream).await.expect("subscribe failed");
        let mut stream = response.into_inner();

        // Should get message 2 (skipping message 1 which was ACKed)
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
    }

    server.shutdown().await;
}

/// T043: Duplicate ACK is idempotent (no error).
#[tokio::test]
async fn test_subscribe_duplicate_ack_is_idempotent() {
    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    // Publish a message
    let msg = client
        .publish(make_publish("dup-ack-topic", b"test message"))
        .await
        .expect("publish failed")
        .into_inner();

    let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    tx.send(make_init(
        "dup-ack-topic",
        "dup-ack-group",
        InitialPosition::Earliest,
    ))
    .await
    .unwrap();
    tx.send(make_credit(10)).await.unwrap();

    let response = client.subscribe(stream).await.expect("subscribe failed");
    let mut stream = response.into_inner();

    // Receive the message
    let _ = timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("stream error");

    // ACK the same message multiple times
    tx.send(make_ack(&msg.message_id)).await.unwrap();
    tx.send(make_ack(&msg.message_id)).await.unwrap();
    tx.send(make_ack(&msg.message_id)).await.unwrap();

    // Small delay to process ACKs
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stream should still be healthy (no error from duplicate ACKs)
    // Send more credits and verify stream is alive
    tx.send(make_credit(1)).await.unwrap();

    // Should just timeout (no more messages, no error)
    let result = timeout(Duration::from_millis(100), stream.next()).await;
    assert!(result.is_err(), "should timeout, stream should be healthy");

    drop(tx);
    server.shutdown().await;
}

/// Test subscribe validation - empty topic should fail.
#[tokio::test]
async fn test_subscribe_empty_topic_fails() {
    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    // Send init with empty topic
    tx.send(make_init("", "test-group", InitialPosition::Latest))
        .await
        .unwrap();

    let result = client.subscribe(stream).await;

    // Should fail with invalid argument
    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);

    server.shutdown().await;
}

/// Test that consumer group defaults to "default".
#[tokio::test]
async fn test_subscribe_empty_consumer_group_uses_default() {
    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    // Publish a message
    client
        .publish(make_publish("default-group-topic", b"test"))
        .await
        .expect("publish failed");

    let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    // Empty consumer_group should work (defaults to "default")
    tx.send(SubscribeUpstream {
        request: Some(UpstreamRequest::Init(SubscriptionInit {
            topic: "default-group-topic".to_string(),
            consumer_group: String::new(), // Empty
            consumer_id: "test".to_string(),
            initial_position: InitialPosition::Earliest as i32,
            offset: 0,
            mode: 0, // CONSUMER_GROUP
        })),
    })
    .await
    .unwrap();
    tx.send(make_credit(10)).await.unwrap();

    let response = client
        .subscribe(stream)
        .await
        .expect("subscribe should succeed");
    let mut stream = response.into_inner();

    // Should receive the message
    let delivery = timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("stream error");

    assert!(matches!(
        delivery.response,
        Some(DownstreamResponse::Delivery(_))
    ));

    drop(tx);
    server.shutdown().await;
}

// ============================================================================
// GetTopicStats Tests
// ============================================================================

/// Test: GetTopicStats returns correct counts and sequences.
#[tokio::test]
async fn test_get_topic_stats_returns_correct_data() {
    use sluice_server::proto::sluice::v1::GetTopicStatsRequest;

    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    // Publish some messages to a topic
    for i in 0..5 {
        client
            .publish(make_publish("stats-test-topic", format!("msg-{}", i).as_bytes()))
            .await
            .expect("publish failed");
    }

    // Get stats for the topic
    let response = client
        .get_topic_stats(GetTopicStatsRequest {
            topics: vec!["stats-test-topic".to_string()],
        })
        .await
        .expect("get_topic_stats failed")
        .into_inner();

    assert_eq!(response.stats.len(), 1);
    let stats = &response.stats[0];
    assert_eq!(stats.topic, "stats-test-topic");
    assert_eq!(stats.total_messages, 5);
    assert_eq!(stats.first_sequence, 1);
    assert_eq!(stats.last_sequence, 5);

    server.shutdown().await;
}

/// Test: GetTopicStats returns all topics when no topics specified.
#[tokio::test]
async fn test_get_topic_stats_all_topics() {
    use sluice_server::proto::sluice::v1::GetTopicStatsRequest;

    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    // Publish to multiple topics
    client
        .publish(make_publish("topic-a", b"msg"))
        .await
        .expect("publish failed");
    client
        .publish(make_publish("topic-b", b"msg"))
        .await
        .expect("publish failed");

    // Get stats for all topics (empty request)
    let response = client
        .get_topic_stats(GetTopicStatsRequest { topics: vec![] })
        .await
        .expect("get_topic_stats failed")
        .into_inner();

    assert_eq!(response.stats.len(), 2);
    let topic_names: Vec<&str> = response.stats.iter().map(|s| s.topic.as_str()).collect();
    assert!(topic_names.contains(&"topic-a"));
    assert!(topic_names.contains(&"topic-b"));

    server.shutdown().await;
}

/// Test: GetTopicStats includes consumer group info.
#[tokio::test]
async fn test_get_topic_stats_includes_consumer_groups() {
    use sluice_server::proto::sluice::v1::GetTopicStatsRequest;

    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    // Publish messages
    for i in 0..3 {
        client
            .publish(make_publish("cg-stats-topic", format!("msg-{}", i).as_bytes()))
            .await
            .expect("publish failed");
    }

    // Subscribe and ack one message to create a consumer group with cursor
    let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    tx.send(make_init("cg-stats-topic", "test-group", InitialPosition::Earliest))
        .await
        .unwrap();
    tx.send(make_credit(10)).await.unwrap();

    let response = client.subscribe(stream).await.expect("subscribe failed");
    let mut stream = response.into_inner();

    // Receive and ack first message
    let delivery = timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("stream error");

    if let Some(DownstreamResponse::Delivery(msg)) = delivery.response {
        tx.send(make_ack(&msg.message_id)).await.unwrap();
        // Small delay for ack to be processed
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    drop(tx);

    // Now get stats
    let stats_response = client
        .get_topic_stats(GetTopicStatsRequest {
            topics: vec!["cg-stats-topic".to_string()],
        })
        .await
        .expect("get_topic_stats failed")
        .into_inner();

    assert_eq!(stats_response.stats.len(), 1);
    let stats = &stats_response.stats[0];

    // Should have consumer group info
    assert!(!stats.consumer_groups.is_empty());
    let cg = &stats.consumer_groups[0];
    assert_eq!(cg.group_name, "test-group");
    assert!(cg.cursor_sequence > 0, "cursor should have advanced");

    server.shutdown().await;
}

// ============================================================================
// Browse Mode Tests
// ============================================================================

/// Helper to create a browse mode subscription init message.
fn make_browse_init(topic: &str) -> SubscribeUpstream {
    SubscribeUpstream {
        request: Some(UpstreamRequest::Init(SubscriptionInit {
            topic: topic.to_string(),
            consumer_group: "browse-group".to_string(),
            consumer_id: "browse-consumer".to_string(),
            initial_position: InitialPosition::Earliest as i32,
            offset: 0,
            mode: 1, // BROWSE
        })),
    }
}

/// Test: Browse mode starts from beginning regardless of cursor.
#[tokio::test]
async fn test_browse_mode_starts_from_beginning() {
    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    let topic = "browse-test-topic";

    // Publish messages
    for i in 0..3 {
        client
            .publish(make_publish(topic, format!("msg-{}", i).as_bytes()))
            .await
            .expect("publish failed");
    }

    // First, subscribe in consumer group mode and ack all messages
    let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    tx.send(make_init(topic, "browse-group", InitialPosition::Earliest))
        .await
        .unwrap();
    tx.send(make_credit(10)).await.unwrap();

    let response = client.subscribe(stream).await.expect("subscribe failed");
    let mut stream = response.into_inner();

    // Ack all 3 messages to advance cursor
    for _ in 0..3 {
        let delivery = timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("stream error");

        if let Some(DownstreamResponse::Delivery(msg)) = delivery.response {
            tx.send(make_ack(&msg.message_id)).await.unwrap();
        }
    }
    drop(tx);

    // Small delay for acks to be processed
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Now subscribe in browse mode - should see all messages again
    let (tx2, rx2) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream2 = tokio_stream::wrappers::ReceiverStream::new(rx2);

    tx2.send(make_browse_init(topic)).await.unwrap();
    tx2.send(make_credit(10)).await.unwrap();

    let response2 = client.subscribe(stream2).await.expect("subscribe failed");
    let mut stream2 = response2.into_inner();

    // Should receive all 3 messages even though cursor was advanced
    let mut received = 0;
    while let Ok(Some(Ok(downstream))) =
        timeout(Duration::from_secs(1), stream2.next()).await
    {
        if let Some(DownstreamResponse::Delivery(_)) = downstream.response {
            received += 1;
            if received == 3 {
                break;
            }
        }
    }

    assert_eq!(received, 3, "Browse mode should see all messages");

    drop(tx2);
    server.shutdown().await;
}

/// Test: Browse mode acks don't update cursor.
#[tokio::test]
async fn test_browse_mode_acks_ignored() {
    use sluice_server::proto::sluice::v1::GetTopicStatsRequest;

    let server = common::TestServer::start().await;
    let mut client = server.client().await;

    let topic = "browse-ack-test";

    // Publish messages
    client
        .publish(make_publish(topic, b"msg-1"))
        .await
        .expect("publish failed");

    // Subscribe in browse mode
    let (tx, rx) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    tx.send(make_browse_init(topic)).await.unwrap();
    tx.send(make_credit(10)).await.unwrap();

    let response = client.subscribe(stream).await.expect("subscribe failed");
    let mut stream = response.into_inner();

    // Receive and ack in browse mode
    let delivery = timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("stream error");

    if let Some(DownstreamResponse::Delivery(msg)) = delivery.response {
        tx.send(make_ack(&msg.message_id)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    drop(tx);

    // Check that cursor was NOT updated (consumer group should have cursor 0 or not exist)
    let stats_response = client
        .get_topic_stats(GetTopicStatsRequest {
            topics: vec![topic.to_string()],
        })
        .await
        .expect("get_topic_stats failed")
        .into_inner();

    let stats = &stats_response.stats[0];

    // In browse mode, acks shouldn't create/update cursor
    // The consumer group cursor should be 0 or not exist
    if !stats.consumer_groups.is_empty() {
        let cg = stats
            .consumer_groups
            .iter()
            .find(|cg| cg.group_name == "browse-group");
        if let Some(cg) = cg {
            assert_eq!(
                cg.cursor_sequence, 0,
                "Browse mode should not update cursor"
            );
        }
    }

    server.shutdown().await;
}
