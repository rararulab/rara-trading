//! gRPC service implementation for [`RaraService`].

use std::{pin::Pin, sync::Arc, time::Instant};

use rara_event_bus::bus::EventBus;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};
use tracing::info;

use crate::{
    health::{self, HealthConfig},
    rara_proto::{
        Empty, EventFilter, EventMessage, SystemStatus, rara_service_server::RaraService,
    },
};

/// Holds shared state needed by the gRPC handlers.
pub struct RaraServiceImpl {
    /// Application start time, used to compute uptime.
    start_time:    Instant,
    /// Event bus for subscribing to real-time events.
    event_bus:     Option<Arc<EventBus>>,
    /// Health probe configuration (None = legacy scaffold mode, all indicators
    /// false).
    health_config: Option<Arc<HealthConfig>>,
}

impl RaraServiceImpl {
    /// Create a new service instance with no event bus (scaffold mode).
    #[must_use]
    pub fn new() -> Self {
        Self {
            start_time:    Instant::now(),
            event_bus:     None,
            health_config: None,
        }
    }

    /// Create a new service instance with health probes enabled.
    #[must_use]
    pub fn with_health(health_config: HealthConfig) -> Self {
        Self {
            start_time:    Instant::now(),
            event_bus:     None,
            health_config: Some(Arc::new(health_config)),
        }
    }

    /// Create a new service instance connected to an event bus.
    #[must_use]
    pub fn with_event_bus(event_bus: Arc<EventBus>) -> Self {
        Self {
            start_time:    Instant::now(),
            event_bus:     Some(event_bus),
            health_config: None,
        }
    }

    /// Attach health probe configuration to an existing instance.
    #[must_use]
    pub fn health(mut self, config: HealthConfig) -> Self {
        self.health_config = Some(Arc::new(config));
        self
    }
}

impl Default for RaraServiceImpl {
    fn default() -> Self { Self::new() }
}

#[tonic::async_trait]
impl RaraService for RaraServiceImpl {
    type StreamEventsStream =
        Pin<Box<dyn Stream<Item = Result<EventMessage, Status>> + Send + 'static>>;

    async fn get_system_status(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<SystemStatus>, Status> {
        let uptime = self.start_time.elapsed();
        let hours = uptime.as_secs() / 3600;
        let minutes = (uptime.as_secs() % 3600) / 60;
        let seconds = uptime.as_secs() % 60;

        let (database_connected, websocket_connected, llm_available, contract_count) =
            if let Some(ref config) = self.health_config {
                let h = health::probe(config).await;
                (
                    h.database_connected,
                    h.websocket_connected,
                    h.llm_available,
                    config.contract_count,
                )
            } else {
                (false, false, false, 0)
            };

        let mut warnings = Vec::new();
        if contract_count == 0 {
            warnings.push(
                "No contracts configured. Run `rara setup -i` to add trading pairs.".to_string(),
            );
        }
        if !database_connected {
            warnings.push("Database unreachable. Check PostgreSQL is running.".to_string());
        }
        if !llm_available {
            warnings.push(
                "LLM backend not found in PATH. Run `rara setup -i` to configure.".to_string(),
            );
        }

        let status = SystemStatus {
            database_connected,
            websocket_connected,
            llm_available,
            event_count: 0,
            uptime: format!("{hours:02}:{minutes:02}:{seconds:02}"),
            strategy_count: 0,
            contract_count,
            warnings,
        };

        info!("GetSystemStatus called, uptime={}", status.uptime);
        Ok(Response::new(status))
    }

    async fn stream_events(
        &self,
        request: Request<EventFilter>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        let filter = request.into_inner();
        let topic_filter = if filter.topic.is_empty() {
            None
        } else {
            Some(filter.topic)
        };

        info!("StreamEvents called, topic_filter={topic_filter:?}");

        let event_bus = self.event_bus.clone();

        let stream = async_stream::try_stream! {
            if let Some(bus) = event_bus {
                let mut rx = bus.subscribe();
                loop {
                    match rx.recv().await {
                        Ok(seq) => {
                            if let Ok(Some(event)) = bus.store().get(seq) {
                                let event_type_str = event.event_type.to_string();
                                // Apply topic filter if provided
                                if let Some(ref topic) = topic_filter
                                    && !event_type_str.starts_with(topic.as_str())
                                {
                                    continue;
                                }

                                let payload_json = event.payload.to_string();

                                yield EventMessage {
                                    sequence: seq,
                                    event_type: event_type_str,
                                    timestamp: event.timestamp.to_string(),
                                    source: event.source,
                                    strategy_id: event
                                        .strategy_id
                                        .unwrap_or_default(),
                                    payload_json,
                                };
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("event stream lagged by {n} messages");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            break;
                        }
                    }
                }
            } else {
                // No event bus — yield nothing, just keep the stream open
                let () = std::future::pending().await;
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }
}
