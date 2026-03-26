//! Integration tests for the rara gRPC service.
//!
//! Each test spins up a real tonic server on a random port, connects a real
//! gRPC client, and exercises the actual RPC behaviour.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::net::TcpListener;
use tokio_stream::StreamExt;
use tonic::transport::Server;

use rara_domain::event::{Event, EventType};
use rara_event_bus::bus::EventBus;
use rara_server::rara_proto::rara_service_client::RaraServiceClient;
use rara_server::rara_proto::rara_service_server::RaraServiceServer;
use rara_server::rara_proto::{Empty, EventFilter};
use rara_server::service::RaraServiceImpl;

/// Start a real tonic gRPC server on a random port and return the endpoint URL.
async fn start_test_server(service: RaraServiceImpl) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        Server::builder()
            .add_service(RaraServiceServer::new(service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    format!("http://{addr}")
}

/// Helper to build a test event with a given type and source.
fn make_event(event_type: EventType, source: &str) -> Event {
    Event::builder()
        .event_type(event_type)
        .source(source)
        .correlation_id("test-corr")
        .payload(json!({"test": true}))
        .build()
}

#[tokio::test]
async fn get_system_status_returns_uptime() {
    let url = start_test_server(RaraServiceImpl::new()).await;
    let mut client = RaraServiceClient::connect(url).await.unwrap();

    let response = client.get_system_status(Empty {}).await.unwrap();
    let status = response.into_inner();

    // Uptime should be formatted as HH:MM:SS
    assert!(
        !status.uptime.is_empty(),
        "uptime must be non-empty"
    );
    assert!(
        status.uptime.contains(':'),
        "uptime should be HH:MM:SS format, got: {}",
        status.uptime
    );

    // The server just started so uptime should start with "00:"
    assert!(
        status.uptime.starts_with("00:"),
        "freshly started server should have 0 hours, got: {}",
        status.uptime
    );
}

#[tokio::test]
async fn stream_events_delivers_published_events() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(EventBus::open(dir.path()).unwrap());
    let service = RaraServiceImpl::with_event_bus(Arc::clone(&bus));

    let url = start_test_server(service).await;
    let mut client = RaraServiceClient::connect(url).await.unwrap();

    // Start streaming (no topic filter)
    let mut stream = client
        .stream_events(EventFilter {
            topic: String::new(),
        })
        .await
        .unwrap()
        .into_inner();

    // Small delay to let the subscriber register before publishing
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Publish two events
    let seq1 = bus.publish(&make_event(EventType::TradingOrderSubmitted, "test-1")).unwrap();
    let seq2 = bus.publish(&make_event(EventType::ResearchHypothesisCreated, "test-2")).unwrap();

    // Receive them in order
    let msg1 = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timed out waiting for first event")
        .expect("stream ended unexpectedly")
        .expect("gRPC error on first event");

    let msg2 = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timed out waiting for second event")
        .expect("stream ended unexpectedly")
        .expect("gRPC error on second event");

    assert_eq!(msg1.sequence, seq1);
    assert_eq!(msg1.source, "test-1");
    assert_eq!(msg1.event_type, "trading_order_submitted");

    assert_eq!(msg2.sequence, seq2);
    assert_eq!(msg2.source, "test-2");
    assert_eq!(msg2.event_type, "research_hypothesis_created");
}

#[tokio::test]
async fn stream_events_filters_by_topic() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(EventBus::open(dir.path()).unwrap());
    let service = RaraServiceImpl::with_event_bus(Arc::clone(&bus));

    let url = start_test_server(service).await;
    let mut client = RaraServiceClient::connect(url).await.unwrap();

    // Stream with topic filter = "research" (matches "research_*" event types)
    let mut stream = client
        .stream_events(EventFilter {
            topic: "research".to_owned(),
        })
        .await
        .unwrap()
        .into_inner();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Publish a trading event (should be filtered out) and a research event (should pass)
    bus.publish(&make_event(EventType::TradingOrderFilled, "trading-src"))
        .unwrap();
    let research_seq = bus
        .publish(&make_event(EventType::ResearchExperimentCompleted, "research-src"))
        .unwrap();

    // We should only receive the research event
    let msg = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timed out waiting for filtered event")
        .expect("stream ended unexpectedly")
        .expect("gRPC error");

    assert_eq!(msg.sequence, research_seq);
    assert_eq!(msg.event_type, "research_experiment_completed");
    assert_eq!(msg.source, "research-src");

    // Verify the trading event was filtered: publish another research event and
    // confirm it arrives next (proving trading event was skipped, not delayed).
    let seq2 = bus
        .publish(&make_event(EventType::ResearchStrategyCandidate, "research-src-2"))
        .unwrap();

    let msg2 = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timed out waiting for second filtered event")
        .expect("stream ended unexpectedly")
        .expect("gRPC error");

    assert_eq!(msg2.sequence, seq2);
    assert_eq!(msg2.event_type, "research_strategy_candidate");
}
