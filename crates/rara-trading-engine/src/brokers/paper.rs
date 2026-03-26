//! Paper trading broker — immediately fills all orders at a configurable
//! price for use in paper trading mode.

use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::Mutex;
use uuid::Uuid;

use rara_domain::trading::{Side, StagedAction};
use crate::broker::{
    AccountInfo, Broker, BrokerError, ExecutionReport, OrderResult, OrderStatus, Position,
};

/// A paper trading broker that fills every order immediately at a fixed price.
pub struct PaperBroker {
    /// Price at which all orders are filled.
    fill_price: Decimal,
    /// Internal position state.
    positions: Mutex<Vec<Position>>,
    /// Record of all executions.
    executions: Mutex<Vec<ExecutionReport>>,
}

impl PaperBroker {
    /// Create a new paper trading broker that fills at the given price.
    pub fn new(fill_price: Decimal) -> Self {
        Self {
            fill_price,
            positions: Mutex::new(Vec::new()),
            executions: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Broker for PaperBroker {
    async fn push(&self, actions: &[StagedAction]) -> Result<Vec<OrderResult>, BrokerError> {
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
                    .price(self.fill_price)
                    .status(OrderStatus::Filled)
                    .filled_at(jiff::Timestamp::now())
                    .build();
                executions.push(report);

                // Update or create position
                let existing = positions
                    .iter_mut()
                    .find(|p| p.contract_id == action.contract_id.as_str());

                if let Some(pos) = existing {
                    match (pos.side, action.side) {
                        // Same side: increase quantity
                        (Side::Buy, Side::Buy) | (Side::Sell, Side::Sell) => {
                            pos.quantity += action.quantity;
                        }
                        // Opposite side: reduce or flip
                        _ => {
                            if action.quantity >= pos.quantity {
                                pos.quantity = action.quantity - pos.quantity;
                                pos.side = action.side;
                            } else {
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
                            .avg_entry_price(self.fill_price)
                            .unrealized_pnl(Decimal::ZERO)
                            .build(),
                    );
                }

                OrderResult {
                    order_id,
                    contract_id: action.contract_id.clone(),
                    status: OrderStatus::Filled,
                }
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
