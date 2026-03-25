//! Trading engine orchestrating guard checks, broker execution, and event
//! publishing.

use std::sync::Arc;

use serde::Serialize;
use snafu::Snafu;

use rara_domain::event::{Event, EventType};
use rara_domain::trading::{Side, TradingCommit};

/// Event payload for an order submission.
#[derive(Debug, Serialize)]
struct OrderSubmittedPayload<'a> {
    /// Contract identifier.
    contract_id: &'a str,
    /// Order side (buy/sell).
    side: &'a Side,
    /// Quantity as decimal string.
    quantity: String,
}

/// Event payload for an order outcome.
#[derive(Debug, Serialize)]
struct OrderOutcomePayload<'a> {
    /// Broker-assigned order identifier.
    order_id: &'a str,
    /// Contract identifier.
    contract_id: &'a str,
    /// Order status.
    status: &'a OrderStatus,
}
use rara_event_bus::bus::EventBus;
use crate::binding::StrategyBinding;
use crate::broker::{Broker, OrderResult, OrderStatus};
use crate::guard_pipeline::GuardPipeline;
use crate::guards::GuardResult;

/// Errors that can occur during trading engine operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum EngineError {
    /// A guard rejected the trading commit.
    #[snafu(display("guard rejected: {reason}"))]
    GuardRejected {
        /// Reason the guard rejected the commit.
        reason: String,
    },
    /// The broker returned an error.
    #[snafu(display("broker error: {source}"))]
    Broker {
        /// Underlying broker error.
        source: crate::broker::BrokerError,
    },
    /// Failed to publish an event.
    #[snafu(display("event publish error: {message}"))]
    EventPublish {
        /// Description of the failure.
        message: String,
    },
}

/// Result type for trading engine operations.
pub type Result<T> = std::result::Result<T, EngineError>;

/// Orchestrates guard checks, broker execution, and event publishing for
/// trading commits.
pub struct TradingEngine {
    /// Broker for order execution.
    broker: Box<dyn Broker>,
    /// Pre-trade risk guard pipeline.
    guard_pipeline: GuardPipeline,
    /// Active strategy bindings.
    bindings: Vec<StrategyBinding>,
    /// Event bus for publishing trading events.
    event_bus: Arc<EventBus>,
}

impl TradingEngine {
    /// Create a new trading engine.
    pub fn new(
        broker: Box<dyn Broker>,
        guard_pipeline: GuardPipeline,
        event_bus: Arc<EventBus>,
    ) -> Self {
        Self {
            broker,
            guard_pipeline,
            bindings: Vec::new(),
            event_bus,
        }
    }

    /// Register a strategy binding.
    pub fn add_binding(&mut self, binding: StrategyBinding) {
        self.bindings.push(binding);
    }

    /// Execute a trading commit: run guards, push to broker, publish events.
    pub async fn execute_commit(&self, commit: TradingCommit) -> Result<Vec<OrderResult>> {
        // Run guard pipeline
        let account = self
            .broker
            .account_info()
            .await
            .map_err(|e| EngineError::Broker { source: e })?;

        if let GuardResult::Reject { reason } = self.guard_pipeline.run(&commit, &account).await {
            return Err(EngineError::GuardRejected { reason });
        }

        // Publish submitted events
        for action in &commit.actions {
            let event = Event::builder()
                .event_type(EventType::TradingOrderSubmitted)
                .source("trading-engine")
                .correlation_id(&commit.hash)
                .strategy_id(commit.strategy_id.clone())
                .payload(
                    serde_json::to_value(OrderSubmittedPayload {
                        contract_id: &action.contract_id,
                        side: &action.side,
                        quantity: action.quantity.to_string(),
                    })
                    .expect("OrderSubmittedPayload must serialize"),
                )
                .build();

            self.event_bus
                .publish(&event)
                .map_err(|e| EngineError::EventPublish {
                    message: e.to_string(),
                })?;
        }

        // Execute via broker
        let results = self
            .broker
            .push(&commit.actions)
            .await
            .map_err(|e| EngineError::Broker { source: e })?;

        // Publish outcome events
        for result in &results {
            let event_type = match result.status {
                OrderStatus::Filled => EventType::TradingOrderFilled,
                OrderStatus::Rejected => EventType::TradingOrderRejected,
                _ => EventType::TradingOrderUpdated,
            };

            let event = Event::builder()
                .event_type(event_type)
                .source("trading-engine")
                .correlation_id(&commit.hash)
                .strategy_id(commit.strategy_id.clone())
                .payload(
                    serde_json::to_value(OrderOutcomePayload {
                        order_id: &result.order_id,
                        contract_id: &result.contract_id,
                        status: &result.status,
                    })
                    .expect("OrderOutcomePayload must serialize"),
                )
                .build();

            self.event_bus
                .publish(&event)
                .map_err(|e| EngineError::EventPublish {
                    message: e.to_string(),
                })?;
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use rara_domain::trading::{ActionType, OrderType, Side, StagedAction, TradingCommit};
    use crate::broker::OrderStatus;
    use crate::brokers::paper::PaperBroker;
    use crate::guards::symbol_whitelist::SymbolWhitelist;

    use super::*;

    fn test_commit(contract_id: &str) -> TradingCommit {
        TradingCommit::builder()
            .message("test")
            .strategy_id("strat-1")
            .strategy_version(1)
            .actions(vec![StagedAction::builder()
                .action_type(ActionType::PlaceOrder)
                .contract_id(contract_id)
                .side(Side::Buy)
                .quantity(Decimal::ONE)
                .order_type(OrderType::Market)
                .build()])
            .build()
    }

    #[tokio::test]
    async fn execute_commit_fills_and_publishes_events() {
        let dir = tempfile::tempdir().unwrap();
        let event_bus = Arc::new(EventBus::open(dir.path()).unwrap());
        let mut rx = event_bus.subscribe();

        let broker = PaperBroker::new(Decimal::new(50_000, 0));
        let pipeline = GuardPipeline::new(vec![]);

        let engine = TradingEngine::new(Box::new(broker), pipeline, event_bus);

        let results = engine
            .execute_commit(test_commit("BTC-USD"))
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, OrderStatus::Filled);

        // Should have received submitted + filled events
        let seq1 = rx.recv().await.unwrap();
        let seq2 = rx.recv().await.unwrap();
        assert!(seq2 > seq1);
    }

    #[tokio::test]
    async fn execute_commit_rejects_on_guard_failure() {
        let dir = tempfile::tempdir().unwrap();
        let event_bus = Arc::new(EventBus::open(dir.path()).unwrap());

        let broker = PaperBroker::new(Decimal::new(50_000, 0));
        // Only allow ETH-USD, so BTC-USD will be rejected
        let pipeline = GuardPipeline::new(vec![Box::new(SymbolWhitelist::new(vec![
            "ETH-USD".to_string(),
        ]))]);

        let engine = TradingEngine::new(Box::new(broker), pipeline, event_bus);

        let err = engine
            .execute_commit(test_commit("BTC-USD"))
            .await
            .unwrap_err();

        assert!(err.to_string().contains("guard rejected"));
    }
}
