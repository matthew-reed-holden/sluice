//! End-to-end quickstart validation test.
//!
//! Tests:
//! - T065: Full quickstart.md validation - end-to-end smoke test
//!
//! This test simulates the complete quickstart workflow:
//! 1. Start server
//! 2. Publish messages
//! 3. Subscribe and consume messages
//! 4. Verify ACK advances cursor
//! 5. Verify message durability

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

fn make_publish(topic: &str, payload: &[u8]) -> PublishRequest {
    PublishRequest {
        topic: topic.to_string(),
        payload: payload.to_vec(),
        attributes: HashMap::new(),
    }
}

fn make_init(topic: &str, group: &str) -> SubscribeUpstream {
    SubscribeUpstream {
        request: Some(UpstreamRequest::Init(SubscriptionInit {
            topic: topic.to_string(),
            consumer_group: group.to_string(),
            consumer_id: "quickstart-test".to_string(),
            initial_position: InitialPosition::Earliest as i32,
            offset: 0,
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

/// T065: Full quickstart validation - end-to-end smoke test.
///
/// This test validates the complete workflow described in quickstart.md:
/// 1. Producer publishes messages to a topic
/// 2. Consumer subscribes with flow control (credits)
/// 3. Consumer receives messages and sends ACKs
/// 4. Cursor persists across reconnection
/// 5. Multiple consumer groups can consume independently
#[tokio::test]
async fn test_quickstart_full_workflow() {
    let server = common::TestServer::start().await;

    // === Step 1: Producer publishes messages ===
    let mut producer = server.client().await;
    let topic = "orders";

    // Publish 5 order events
    let mut message_ids = Vec::new();
    for i in 1..=5 {
        let payload = format!(r#"{{"order_id": {}, "status": "created"}}"#, i);
        let resp = producer
            .publish(make_publish(topic, payload.as_bytes()))
            .await
            .expect("publish failed")
            .into_inner();

        // Verify response has all required fields
        assert!(!resp.message_id.is_empty(), "message_id should be set");
        assert!(resp.sequence > 0, "sequence should be positive");
        assert!(resp.timestamp > 0, "timestamp should be set");

        message_ids.push(resp.message_id);
    }

    assert_eq!(message_ids.len(), 5, "should have published 5 messages");

    // === Step 2: Consumer A subscribes and consumes 3 messages ===
    let (tx_a, rx_a) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream_a = tokio_stream::wrappers::ReceiverStream::new(rx_a);

    tx_a.send(make_init(topic, "order-processor"))
        .await
        .unwrap();
    tx_a.send(make_credit(3)).await.unwrap(); // Request 3 messages

    let mut consumer_a = server.client().await;
    let response = consumer_a
        .subscribe(stream_a)
        .await
        .expect("subscribe failed");
    let mut stream_a = response.into_inner();

    // Receive and ACK 3 messages
    let mut received_ids = Vec::new();
    for _ in 0..3 {
        let delivery = timeout(Duration::from_secs(2), stream_a.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("stream error");

        if let Some(DownstreamResponse::Delivery(d)) = delivery.response {
            received_ids.push(d.message_id.clone());
            tx_a.send(make_ack(&d.message_id)).await.unwrap();
        } else {
            panic!("expected delivery");
        }
    }

    assert_eq!(received_ids.len(), 3, "should have received 3 messages");
    assert_eq!(
        received_ids,
        &message_ids[0..3],
        "should receive first 3 messages"
    );

    // Wait for ACKs to be processed
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(tx_a);

    // === Step 3: Consumer A reconnects and resumes from message 4 ===
    let (tx_a2, rx_a2) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream_a2 = tokio_stream::wrappers::ReceiverStream::new(rx_a2);

    tx_a2
        .send(make_init(topic, "order-processor"))
        .await
        .unwrap();
    tx_a2.send(make_credit(10)).await.unwrap();

    let mut consumer_a2 = server.client().await;
    let response = consumer_a2
        .subscribe(stream_a2)
        .await
        .expect("subscribe failed");
    let mut stream_a2 = response.into_inner();

    // Should receive messages 4 and 5 (resumed from cursor)
    let delivery4 = timeout(Duration::from_secs(2), stream_a2.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("stream error");

    if let Some(DownstreamResponse::Delivery(d)) = delivery4.response {
        assert_eq!(d.message_id, message_ids[3], "should resume from message 4");
    } else {
        panic!("expected delivery");
    }

    let delivery5 = timeout(Duration::from_secs(2), stream_a2.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("stream error");

    if let Some(DownstreamResponse::Delivery(d)) = delivery5.response {
        assert_eq!(d.message_id, message_ids[4], "should get message 5");
    } else {
        panic!("expected delivery");
    }

    drop(tx_a2);

    // === Step 4: Consumer B (different group) gets all messages ===
    let (tx_b, rx_b) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream_b = tokio_stream::wrappers::ReceiverStream::new(rx_b);

    tx_b.send(make_init(topic, "analytics")).await.unwrap(); // Different consumer group
    tx_b.send(make_credit(10)).await.unwrap();

    let mut consumer_b = server.client().await;
    let response = consumer_b
        .subscribe(stream_b)
        .await
        .expect("subscribe failed");
    let mut stream_b = response.into_inner();

    // Consumer B should get all 5 messages (independent cursor)
    for (i, expected_id) in message_ids.iter().enumerate() {
        let delivery = timeout(Duration::from_secs(2), stream_b.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("stream error");

        if let Some(DownstreamResponse::Delivery(d)) = delivery.response {
            assert_eq!(
                d.message_id,
                *expected_id,
                "consumer B should get message {}",
                i + 1
            );
        } else {
            panic!("expected delivery for message {}", i + 1);
        }
    }

    drop(tx_b);

    // === Step 5: Verify credit-based flow control ===
    let (tx_c, rx_c) = tokio::sync::mpsc::channel::<SubscribeUpstream>(10);
    let stream_c = tokio_stream::wrappers::ReceiverStream::new(rx_c);

    tx_c.send(make_init(topic, "slow-consumer")).await.unwrap();
    // Don't send credits yet

    let mut consumer_c = server.client().await;
    let response = consumer_c
        .subscribe(stream_c)
        .await
        .expect("subscribe failed");
    let mut stream_c = response.into_inner();

    // Should not receive any messages without credits
    let result = timeout(Duration::from_millis(200), stream_c.next()).await;
    assert!(
        result.is_err(),
        "should not receive messages without credits"
    );

    // Grant 1 credit
    tx_c.send(make_credit(1)).await.unwrap();

    // Now should receive exactly 1 message
    let delivery = timeout(Duration::from_secs(2), stream_c.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("stream error");

    assert!(
        matches!(delivery.response, Some(DownstreamResponse::Delivery(_))),
        "should receive 1 message after granting 1 credit"
    );

    // Should not receive more (only had 1 credit)
    let result = timeout(Duration::from_millis(200), stream_c.next()).await;
    assert!(
        result.is_err(),
        "should not receive more messages without more credits"
    );

    drop(tx_c);
    server.shutdown().await;

    println!("✅ Quickstart validation complete!");
    println!("   - Published 5 messages");
    println!("   - Consumer A consumed 3, reconnected and resumed from 4");
    println!("   - Consumer B (different group) consumed all 5");
    println!("   - Credit-based flow control verified");
}
