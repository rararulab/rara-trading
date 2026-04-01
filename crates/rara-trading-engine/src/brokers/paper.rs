//! Paper trading broker — immediately fills all orders at a configurable
//! price for use in paper trading mode.

use std::collections::HashMap;

use async_trait::async_trait;
use rara_domain::trading::{Side, StagedAction};
use rust_decimal::Decimal;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    account_config::{BrokerConfig, PaperBrokerConfig},
    broker::{
        AccountInfo, Broker, BrokerError, ExecutionReport, OrderResult, OrderStatus, Position,
    },
    broker_registry::{
        BrokerRegistryEntry, BrokerRegistryError, ConfigField, ConfigFieldType, InvalidValueSnafu,
    },
};

/// A paper trading broker that fills every order immediately at a fixed price.
pub struct PaperBroker {
    /// Price at which all orders are filled; wrapped in a `Mutex` so it can
    /// be updated between trades (e.g. to simulate changing market prices).
    fill_price: Mutex<Decimal>,
    /// Internal position state.
    positions:  Mutex<Vec<Position>>,
    /// Record of all executions.
    executions: Mutex<Vec<ExecutionReport>>,
}

/// Build the broker registry entry for the paper trading broker.
pub fn registry_entry() -> BrokerRegistryEntry {
    BrokerRegistryEntry {
        type_key:      "paper",
        name:          "Paper Trading",
        description:   "Simulated fills with no real money — great for testing strategies.",
        config_fields: || {
            vec![
                ConfigField::builder()
                    .name("fill_price")
                    .field_type(ConfigFieldType::Number)
                    .label("Fill price")
                    .required(false)
                    .sensitive(false)
                    .description("Fixed price for all simulated fills (0 = use market price).")
                    .build(),
            ]
        },
        create_broker: |fields: &HashMap<String, String>| {
            let fill_price = parse_fill_price(fields)?;
            Ok(Box::new(PaperBroker::new(fill_price)))
        },
        create_config: |fields: &HashMap<String, String>| {
            let fill_price = parse_fill_price(fields)?;
            let fp_f64 = fill_price.try_into().ok();
            Ok(BrokerConfig::Paper(PaperBrokerConfig {
                fill_price: fp_f64,
            }))
        },
    }
}

/// Parse the `fill_price` field from a config map, defaulting to zero (market
/// price).
fn parse_fill_price(fields: &HashMap<String, String>) -> Result<Decimal, BrokerRegistryError> {
    fields
        .get("fill_price")
        .filter(|v| !v.is_empty())
        .map_or_else(
            || Ok(Decimal::ZERO),
            |v| {
                v.parse::<Decimal>().map_err(|e| {
                    InvalidValueSnafu {
                        field:  "fill_price".to_string(),
                        reason: e.to_string(),
                    }
                    .build()
                })
            },
        )
}

impl PaperBroker {
    /// Create a new paper trading broker that fills at the given price.
    pub fn new(fill_price: Decimal) -> Self {
        Self {
            fill_price: Mutex::new(fill_price),
            positions:  Mutex::new(Vec::new()),
            executions: Mutex::new(Vec::new()),
        }
    }

    /// Update the fill price for subsequent orders.
    ///
    /// Useful for simulating price movement between trades in tests.
    pub async fn set_fill_price(&self, price: Decimal) { *self.fill_price.lock().await = price; }
}

#[async_trait]
impl Broker for PaperBroker {
    async fn push(&self, actions: &[StagedAction]) -> Result<Vec<OrderResult>, BrokerError> {
        let fill_price = *self.fill_price.lock().await;
        let mut positions = self.positions.lock().await;
        let mut executions = self.executions.lock().await;

        let results = actions
            .iter()
            .map(|action| {
                let order_id = Uuid::new_v4().to_string()[..8].to_string();

                let report = ExecutionReport::builder()
                    .order_id(&order_id)
                    .contract_id(&action.contract_id)
                    .side(action.side)
                    .quantity(action.quantity)
                    .price(fill_price)
                    .status(OrderStatus::Filled)
                    .filled_at(jiff::Timestamp::now())
                    .build();
                executions.push(report);

                // Compute realized PnL when reducing or closing a position
                let mut realized_pnl = Decimal::ZERO;

                let existing = positions
                    .iter_mut()
                    .find(|p| p.contract_id == action.contract_id.as_str());

                if let Some(pos) = existing {
                    match (pos.side, action.side) {
                        // Same side: increase position with weighted average entry price
                        (Side::Buy, Side::Buy) | (Side::Sell, Side::Sell) => {
                            let new_total = pos.quantity + action.quantity;
                            // Weighted average: (old_qty * old_price + new_qty * new_price) / total
                            let old_cost = pos.avg_entry_price * pos.quantity;
                            let new_cost = fill_price * action.quantity;
                            pos.avg_entry_price = (old_cost + new_cost) / new_total;
                            pos.quantity = new_total;
                        }
                        // Opposite side: reduce or flip — realize PnL on the closed portion
                        _ => {
                            let close_qty = action.quantity.min(pos.quantity);
                            // Long closing: profit when exit > entry
                            // Short closing: profit when entry > exit (flip sign)
                            let side_multiplier = match pos.side {
                                Side::Buy => Decimal::ONE,
                                Side::Sell => -Decimal::ONE,
                            };
                            realized_pnl =
                                (fill_price - pos.avg_entry_price) * close_qty * side_multiplier;

                            if action.quantity >= pos.quantity {
                                // Full close or flip: remainder opens a new position
                                let remainder = action.quantity - pos.quantity;
                                pos.side = action.side;
                                pos.quantity = remainder;
                                if remainder > Decimal::ZERO {
                                    pos.avg_entry_price = fill_price;
                                }
                            } else {
                                // Partial close — entry price stays the same
                                pos.quantity -= action.quantity;
                            }
                        }
                    }
                } else {
                    positions.push(
                        Position::builder()
                            .contract_id(&action.contract_id)
                            .side(action.side)
                            .quantity(action.quantity)
                            .avg_entry_price(fill_price)
                            .unrealized_pnl(Decimal::ZERO)
                            .build(),
                    );
                }

                OrderResult::builder()
                    .order_id(&order_id)
                    .contract_id(&action.contract_id)
                    .status(OrderStatus::Filled)
                    .side(action.side)
                    .quantity(action.quantity)
                    .price(fill_price)
                    .realized_pnl(realized_pnl)
                    .build()
            })
            .collect();

        Ok(results)
    }

    async fn sync_orders(&self) -> Result<Vec<ExecutionReport>, BrokerError> {
        Ok(self.executions.lock().await.clone())
    }

    async fn positions(&self) -> Result<Vec<Position>, BrokerError> {
        Ok(self.positions.lock().await.clone())
    }

    async fn account_info(&self) -> Result<AccountInfo, BrokerError> {
        let positions = self.positions.lock().await.clone();
        Ok(AccountInfo::builder()
            .total_equity(Decimal::new(100_000, 0))
            .available_cash(Decimal::new(100_000, 0))
            .positions(positions)
            .realized_pnl(Decimal::ZERO)
            .build())
    }
}

#[cfg(test)]
mod tests {
    use rara_domain::trading::{ActionType, OrderType, Side, StagedAction};
    use rust_decimal_macros::dec;

    use super::*;

    fn buy(contract: &str, qty: Decimal) -> StagedAction {
        StagedAction::builder()
            .action_type(ActionType::PlaceOrder)
            .contract_id(contract)
            .side(Side::Buy)
            .quantity(qty)
            .order_type(OrderType::Market)
            .build()
    }

    fn sell(contract: &str, qty: Decimal) -> StagedAction {
        StagedAction::builder()
            .action_type(ActionType::PlaceOrder)
            .contract_id(contract)
            .side(Side::Sell)
            .quantity(qty)
            .order_type(OrderType::Market)
            .build()
    }

    #[tokio::test]
    async fn realized_pnl_zero_for_new_position() {
        let broker = PaperBroker::new(dec!(100));
        let results = broker.push(&[buy("BTC", dec!(1))]).await.unwrap();
        assert_eq!(results[0].realized_pnl, dec!(0));
    }

    #[tokio::test]
    async fn realized_pnl_long_profit() {
        // Buy at 100, sell at 150 => PnL = (150 - 100) * 2 = 100
        let broker = PaperBroker::new(dec!(100));
        broker.push(&[buy("BTC", dec!(2))]).await.unwrap();

        broker.set_fill_price(dec!(150)).await;
        let results = broker.push(&[sell("BTC", dec!(2))]).await.unwrap();
        assert_eq!(results[0].realized_pnl, dec!(100));
    }

    #[tokio::test]
    async fn realized_pnl_long_loss() {
        // Buy at 100, sell at 80 => PnL = (80 - 100) * 3 = -60
        let broker = PaperBroker::new(dec!(100));
        broker.push(&[buy("BTC", dec!(3))]).await.unwrap();

        broker.set_fill_price(dec!(80)).await;
        let results = broker.push(&[sell("BTC", dec!(3))]).await.unwrap();
        assert_eq!(results[0].realized_pnl, dec!(-60));
    }

    #[tokio::test]
    async fn realized_pnl_short_profit() {
        // Sell (open short) at 200, buy (close short) at 150
        // PnL = (150 - 200) * 1 * (-1) = 50
        let broker = PaperBroker::new(dec!(200));
        broker.push(&[sell("BTC", dec!(1))]).await.unwrap();

        broker.set_fill_price(dec!(150)).await;
        let results = broker.push(&[buy("BTC", dec!(1))]).await.unwrap();
        assert_eq!(results[0].realized_pnl, dec!(50));
    }

    #[tokio::test]
    async fn realized_pnl_partial_close() {
        // Buy 4 at 100, sell 1 at 120 => PnL = (120 - 100) * 1 = 20
        // Remaining position: 3 at avg_entry 100
        let broker = PaperBroker::new(dec!(100));
        broker.push(&[buy("BTC", dec!(4))]).await.unwrap();

        broker.set_fill_price(dec!(120)).await;
        let results = broker.push(&[sell("BTC", dec!(1))]).await.unwrap();
        assert_eq!(results[0].realized_pnl, dec!(20));

        let pos = broker.positions().await.unwrap();
        assert_eq!(pos[0].quantity, dec!(3));
        assert_eq!(pos[0].avg_entry_price, dec!(100));
    }

    #[tokio::test]
    async fn weighted_avg_entry_on_same_side_add() {
        // Buy 2 at 100, buy 1 at 130 => avg = (200 + 130) / 3 = 110
        let broker = PaperBroker::new(dec!(100));
        broker.push(&[buy("BTC", dec!(2))]).await.unwrap();

        broker.set_fill_price(dec!(130)).await;
        broker.push(&[buy("BTC", dec!(1))]).await.unwrap();

        let pos = broker.positions().await.unwrap();
        assert_eq!(pos[0].avg_entry_price, dec!(110));
        assert_eq!(pos[0].quantity, dec!(3));

        // Now sell all at 120 => PnL = (120 - 110) * 3 = 30
        broker.set_fill_price(dec!(120)).await;
        let results = broker.push(&[sell("BTC", dec!(3))]).await.unwrap();
        assert_eq!(results[0].realized_pnl, dec!(30));
    }
}
