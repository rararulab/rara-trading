//! CCXT-based broker implementation for real exchange integration.
//!
//! Wraps the `ccxt-rust` library to provide a unified broker interface
//! across multiple cryptocurrency exchanges (Binance, OKX, Bybit).

use std::collections::HashMap;

use async_trait::async_trait;
use bon::Builder;
use rust_decimal::Decimal;
use tracing::{debug, instrument, warn};

use ccxt_rust::prelude::{
    Amount, Binance, BinanceBuilder, Bybit, BybitBuilder, Okx, OkxBuilder,
    OrderSide as CcxtOrderSide, OrderStatus as CcxtOrderStatus, OrderType as CcxtOrderType, Price,
};

use rara_domain::trading::{ActionType, OrderType, Side, StagedAction};
use crate::account_config::{BrokerConfig, CcxtBrokerConfig};
use crate::broker::{
    AccountInfo, Broker, BrokerError, ExecutionReport, OrderResult, OrderStatus, Position,
};
use crate::broker_registry::{
    BrokerRegistryEntry, BrokerRegistryError, ConfigField, ConfigFieldType, InvalidValueSnafu,
    MissingFieldSnafu, SelectOption,
};

/// Supported exchange identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExchangeId {
    /// Binance exchange.
    Binance,
    /// OKX exchange.
    Okx,
    /// Bybit exchange.
    Bybit,
}

impl ExchangeId {
    /// Parse an exchange identifier from a string.
    fn parse(s: &str) -> Result<Self, BrokerError> {
        match s.to_lowercase().as_str() {
            "binance" => Ok(Self::Binance),
            "okx" => Ok(Self::Okx),
            "bybit" => Ok(Self::Bybit),
            other => Err(BrokerError::Connection {
                message: format!("unsupported exchange: {other}"),
            }),
        }
    }
}

/// Parsed CCXT config fields shared between `create_broker` and `create_config`.
struct CcxtFields {
    exchange: String,
    sandbox: bool,
    api_key: String,
    secret: String,
    passphrase: Option<String>,
}

/// Extract and validate common CCXT fields from a raw config map.
fn parse_ccxt_fields(fields: &HashMap<String, String>) -> Result<CcxtFields, BrokerRegistryError> {
    let exchange = fields
        .get("exchange")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            MissingFieldSnafu {
                field: "exchange".to_string(),
            }
            .build()
        })?
        .clone();

    // Validate exchange value
    ExchangeId::parse(&exchange).map_err(|_| {
        InvalidValueSnafu {
            field: "exchange".to_string(),
            reason: format!("unsupported exchange: {exchange}"),
        }
        .build()
    })?;

    let sandbox = fields
        .get("sandbox")
        .is_none_or(|v| v == "true");
    let api_key = fields.get("api_key").cloned().unwrap_or_default();
    let secret = fields.get("secret").cloned().unwrap_or_default();
    let passphrase = fields
        .get("passphrase")
        .filter(|v| !v.is_empty())
        .cloned();

    Ok(CcxtFields {
        exchange,
        sandbox,
        api_key,
        secret,
        passphrase,
    })
}

/// Return the config field definitions for the CCXT broker.
fn ccxt_config_fields() -> Vec<ConfigField> {
    vec![
        ConfigField::builder()
            .name("exchange")
            .field_type(ConfigFieldType::Select)
            .label("Exchange")
            .required(true)
            .sensitive(false)
            .default("binance")
            .options(vec![
                SelectOption { value: "binance".into(), label: "Binance".into() },
                SelectOption { value: "bybit".into(), label: "Bybit".into() },
                SelectOption { value: "okx".into(), label: "OKX".into() },
            ])
            .build(),
        ConfigField::builder()
            .name("sandbox")
            .field_type(ConfigFieldType::Boolean)
            .label("Sandbox mode")
            .required(false)
            .sensitive(false)
            .default("true")
            .description("Use testnet/sandbox environment (recommended for initial setup).")
            .build(),
        ConfigField::builder()
            .name("api_key")
            .field_type(ConfigFieldType::Password)
            .label("API key")
            .required(false)
            .sensitive(true)
            .description("Exchange API key. Can also be set via RARA_BROKER_API_KEY env var.")
            .build(),
        ConfigField::builder()
            .name("secret")
            .field_type(ConfigFieldType::Password)
            .label("API secret")
            .required(false)
            .sensitive(true)
            .description("Exchange API secret. Can also be set via RARA_BROKER_SECRET env var.")
            .build(),
        ConfigField::builder()
            .name("passphrase")
            .field_type(ConfigFieldType::Password)
            .label("API passphrase (OKX only)")
            .required(false)
            .sensitive(true)
            .description("Exchange API passphrase. Can also be set via RARA_BROKER_PASSPHRASE env var.")
            .build(),
    ]
}

/// Build the broker registry entry for the CCXT broker.
pub fn registry_entry() -> BrokerRegistryEntry {
    BrokerRegistryEntry {
        type_key: "ccxt",
        name: "CCXT (Crypto Exchanges)",
        description: "Trade on Binance, Bybit, OKX, and other crypto exchanges via CCXT.",
        config_fields: ccxt_config_fields,
        create_broker: |fields: &HashMap<String, String>| {
            let f = parse_ccxt_fields(fields)?;
            let broker = CcxtBroker::builder()
                .exchange_id(f.exchange)
                .api_key(f.api_key)
                .secret(f.secret)
                .sandbox(f.sandbox)
                .maybe_passphrase(f.passphrase)
                .build();
            Ok(Box::new(broker) as Box<dyn crate::broker::Broker>)
        },
        create_config: |fields: &HashMap<String, String>| {
            let f = parse_ccxt_fields(fields)?;
            Ok(BrokerConfig::Ccxt(CcxtBrokerConfig {
                exchange: f.exchange,
                sandbox: f.sandbox,
                api_key: f.api_key,
                secret: f.secret,
                passphrase: f.passphrase,
            }))
        },
    }
}

/// Wrapper around different ccxt exchange implementations.
///
/// Each exchange in ccxt-rust is a concrete type with slightly different method
/// signatures. We wrap them in an enum and provide unified async helpers that
/// smooth over the per-exchange API differences.
enum ExchangeClient {
    /// Binance exchange client.
    Binance(Binance),
    /// OKX exchange client.
    Okx(Okx),
    /// Bybit exchange client.
    Bybit(Bybit),
}

impl std::fmt::Debug for ExchangeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Binance(_) => f.debug_tuple("Binance").finish(),
            Self::Okx(_) => f.debug_tuple("Okx").finish(),
            Self::Bybit(_) => f.debug_tuple("Bybit").finish(),
        }
    }
}

impl ExchangeClient {
    /// Place an order on the exchange.
    #[allow(deprecated)] // create_order is deprecated in favour of create_order_v2
    async fn create_order(
        &self,
        symbol: &str,
        order_type: CcxtOrderType,
        side: CcxtOrderSide,
        amount: Amount,
        price: Option<Price>,
    ) -> Result<ccxt_rust::prelude::Order, ccxt_rust::prelude::Error> {
        match self {
            Self::Binance(ex) => {
                ex.create_order(symbol, order_type, side, amount, price, None)
                    .await
            }
            Self::Okx(ex) => {
                ex.create_order(symbol, order_type, side, amount, price)
                    .await
            }
            Self::Bybit(ex) => {
                ex.create_order(symbol, order_type, side, amount, price)
                    .await
            }
        }
    }

    /// Cancel an order by ID.
    async fn cancel_order(
        &self,
        id: &str,
        symbol: &str,
    ) -> Result<ccxt_rust::prelude::Order, ccxt_rust::prelude::Error> {
        match self {
            Self::Binance(ex) => ex.cancel_order(id, symbol).await,
            Self::Okx(ex) => ex.cancel_order(id, symbol).await,
            Self::Bybit(ex) => ex.cancel_order(id, symbol).await,
        }
    }

    /// Fetch recently closed orders.
    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> Result<Vec<ccxt_rust::prelude::Order>, ccxt_rust::prelude::Error> {
        match self {
            Self::Binance(ex) => ex.fetch_closed_orders(symbol, since, limit).await,
            Self::Okx(ex) => ex.fetch_closed_orders(symbol, since, limit).await,
            Self::Bybit(ex) => ex.fetch_closed_orders(symbol, since, limit).await,
        }
    }

    /// Fetch open orders.
    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
    ) -> Result<Vec<ccxt_rust::prelude::Order>, ccxt_rust::prelude::Error> {
        match self {
            Self::Binance(ex) => ex.fetch_open_orders(symbol).await,
            Self::Okx(ex) => ex.fetch_open_orders(symbol, None, None).await,
            Self::Bybit(ex) => ex.fetch_open_orders(symbol, None, None).await,
        }
    }

    /// Fetch account balances.
    async fn fetch_balance(
        &self,
    ) -> Result<ccxt_rust::prelude::Balance, ccxt_rust::prelude::Error> {
        match self {
            Self::Binance(ex) => ex.fetch_balance(None).await,
            Self::Okx(ex) => ex.fetch_balance().await,
            Self::Bybit(ex) => ex.fetch_balance().await,
        }
    }
}

/// Real broker implementation backed by ccxt-rust exchange clients.
///
/// Supports Binance, OKX, and Bybit exchanges through the ccxt-rust library.
/// Use the builder to construct an instance with exchange credentials.
#[derive(Builder)]
pub struct CcxtBroker {
    /// Exchange identifier (e.g., "binance", "okx", "bybit").
    #[builder(into)]
    exchange_id: String,
    /// API key for authentication.
    #[builder(into)]
    api_key: String,
    /// API secret for authentication.
    #[builder(into)]
    secret: String,
    /// Passphrase for exchanges that require it (e.g., OKX).
    #[builder(into)]
    passphrase: Option<String>,
    /// Whether to use the exchange's sandbox/testnet environment.
    #[builder(default = false)]
    sandbox: bool,
}

impl std::fmt::Debug for CcxtBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CcxtBroker")
            .field("exchange_id", &self.exchange_id)
            .field("sandbox", &self.sandbox)
            .finish_non_exhaustive()
    }
}

impl CcxtBroker {
    /// Create the underlying exchange client based on the configured exchange ID.
    fn build_client(&self) -> Result<ExchangeClient, BrokerError> {
        let exchange_id = ExchangeId::parse(&self.exchange_id)?;

        match exchange_id {
            ExchangeId::Binance => {
                let exchange = BinanceBuilder::new()
                    .api_key(&self.api_key)
                    .secret(&self.secret)
                    .sandbox(self.sandbox)
                    .build()
                    .map_err(|e| BrokerError::Connection {
                        message: format!("failed to build Binance client: {e}"),
                    })?;
                Ok(ExchangeClient::Binance(exchange))
            }
            ExchangeId::Okx => {
                let mut builder = OkxBuilder::new()
                    .api_key(&self.api_key)
                    .secret(&self.secret)
                    .sandbox(self.sandbox);
                if let Some(ref pass) = self.passphrase {
                    builder = builder.passphrase(pass);
                }
                let exchange = builder.build().map_err(|e| BrokerError::Connection {
                    message: format!("failed to build OKX client: {e}"),
                })?;
                Ok(ExchangeClient::Okx(exchange))
            }
            ExchangeId::Bybit => {
                let exchange = BybitBuilder::new()
                    .api_key(&self.api_key)
                    .secret(&self.secret)
                    .sandbox(self.sandbox)
                    .build()
                    .map_err(|e| BrokerError::Connection {
                        message: format!("failed to build Bybit client: {e}"),
                    })?;
                Ok(ExchangeClient::Bybit(exchange))
            }
        }
    }
}

/// Map our `Side` to ccxt `OrderSide`.
const fn to_ccxt_side(side: Side) -> CcxtOrderSide {
    match side {
        Side::Buy => CcxtOrderSide::Buy,
        Side::Sell => CcxtOrderSide::Sell,
    }
}

/// Map our `OrderType` to ccxt `OrderType`.
const fn to_ccxt_order_type(order_type: OrderType) -> CcxtOrderType {
    match order_type {
        OrderType::Market => CcxtOrderType::Market,
        OrderType::Limit => CcxtOrderType::Limit,
        OrderType::StopLoss => CcxtOrderType::StopLoss,
        OrderType::StopLimit => CcxtOrderType::StopLimit,
    }
}

/// Map ccxt `OrderStatus` to our `OrderStatus`.
const fn from_ccxt_order_status(status: CcxtOrderStatus) -> OrderStatus {
    match status {
        CcxtOrderStatus::Open | CcxtOrderStatus::Partial => OrderStatus::Submitted,
        CcxtOrderStatus::Closed => OrderStatus::Filled,
        CcxtOrderStatus::Cancelled => OrderStatus::Cancelled,
        CcxtOrderStatus::Expired | CcxtOrderStatus::Rejected => OrderStatus::Rejected,
    }
}

/// Convert a ccxt `OrderSide` to our `Side`.
const fn from_ccxt_side(side: CcxtOrderSide) -> Side {
    match side {
        CcxtOrderSide::Buy => Side::Buy,
        CcxtOrderSide::Sell => Side::Sell,
    }
}

/// Classify a ccxt error into the appropriate `BrokerError` variant.
fn map_ccxt_error(err: &ccxt_rust::prelude::Error) -> BrokerError {
    let msg = err.to_string();

    if msg.contains("authentication")
        || msg.contains("invalid key")
        || msg.contains("signature")
        || msg.contains("apiKey")
    {
        BrokerError::Authentication { message: msg }
    } else if msg.contains("rate limit") || msg.contains("too many") || msg.contains("429") {
        BrokerError::RateLimit { message: msg }
    } else if msg.contains("rejected") || msg.contains("insufficient") {
        BrokerError::OrderRejected { message: msg }
    } else {
        BrokerError::Exchange { message: msg }
    }
}

#[async_trait]
impl Broker for CcxtBroker {
    #[instrument(skip(self, actions), fields(exchange = %self.exchange_id, count = actions.len()))]
    async fn push(&self, actions: &[StagedAction]) -> Result<Vec<OrderResult>, BrokerError> {
        let client = self.build_client()?;
        let mut results = Vec::with_capacity(actions.len());

        for action in actions {
            let result = match action.action_type {
                ActionType::PlaceOrder => {
                    let ccxt_side = to_ccxt_side(action.side);
                    let ccxt_type = to_ccxt_order_type(action.order_type);
                    let amount = Amount::new(action.quantity);
                    let price = action.limit_price.map(Price::new);

                    debug!(
                        contract = action.contract_id,
                        side = ?action.side,
                        qty = %action.quantity,
                        "placing order"
                    );

                    let order = client
                        .create_order(&action.contract_id, ccxt_type, ccxt_side, amount, price)
                        .await
                        .map_err(|e| map_ccxt_error(&e))?;

                    OrderResult {
                        order_id: order.id,
                        contract_id: action.contract_id.clone(),
                        status: from_ccxt_order_status(order.status),
                    }
                }
                ActionType::CancelOrder => {
                    debug!(contract = action.contract_id, "cancelling order");

                    let order = client
                        .cancel_order(&action.contract_id, &action.contract_id)
                        .await
                        .map_err(|e| map_ccxt_error(&e))?;

                    OrderResult {
                        order_id: order.id,
                        contract_id: action.contract_id.clone(),
                        status: from_ccxt_order_status(order.status),
                    }
                }
                ActionType::ClosePosition => {
                    // Close position by placing an opposite-side market order
                    let close_side = match action.side {
                        Side::Buy => CcxtOrderSide::Sell,
                        Side::Sell => CcxtOrderSide::Buy,
                    };
                    let amount = Amount::new(action.quantity);

                    debug!(
                        contract = action.contract_id,
                        side = ?close_side,
                        qty = %action.quantity,
                        "closing position with opposite market order"
                    );

                    let order = client
                        .create_order(
                            &action.contract_id,
                            CcxtOrderType::Market,
                            close_side,
                            amount,
                            None,
                        )
                        .await
                        .map_err(|e| map_ccxt_error(&e))?;

                    OrderResult {
                        order_id: order.id,
                        contract_id: action.contract_id.clone(),
                        status: from_ccxt_order_status(order.status),
                    }
                }
                ActionType::ModifyOrder => {
                    // ccxt-rust Exchange trait has no edit_order, so cancel + re-place
                    warn!(
                        contract = action.contract_id,
                        "ModifyOrder: cancel + re-place (no native edit_order in ccxt-rust)"
                    );

                    let _ = client
                        .cancel_order(&action.contract_id, &action.contract_id)
                        .await
                        .map_err(|e| map_ccxt_error(&e))?;

                    let ccxt_side = to_ccxt_side(action.side);
                    let ccxt_type = to_ccxt_order_type(action.order_type);
                    let amount = Amount::new(action.quantity);
                    let price = action.limit_price.map(Price::new);

                    let order = client
                        .create_order(&action.contract_id, ccxt_type, ccxt_side, amount, price)
                        .await
                        .map_err(|e| map_ccxt_error(&e))?;

                    OrderResult {
                        order_id: order.id,
                        contract_id: action.contract_id.clone(),
                        status: from_ccxt_order_status(order.status),
                    }
                }
            };

            results.push(result);
        }

        Ok(results)
    }

    #[instrument(skip(self), fields(exchange = %self.exchange_id))]
    async fn sync_orders(&self) -> Result<Vec<ExecutionReport>, BrokerError> {
        let client = self.build_client()?;

        let orders = client
            .fetch_closed_orders(None, None, Some(50))
            .await
            .map_err(|e| map_ccxt_error(&e))?;

        let reports = orders
            .into_iter()
            .map(|order| {
                let side = from_ccxt_side(order.side);
                let filled_qty = order.filled.unwrap_or(Decimal::ZERO);
                let fill_price = order.average.or(order.price).unwrap_or(Decimal::ZERO);

                let filled_at = order
                    .timestamp
                    .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
                    .unwrap_or_else(jiff::Timestamp::now);

                ExecutionReport::builder()
                    .order_id(&order.id)
                    .contract_id(order.symbol.as_str())
                    .side(side)
                    .quantity(filled_qty)
                    .price(fill_price)
                    .status(from_ccxt_order_status(order.status))
                    .filled_at(filled_at)
                    .build()
            })
            .collect();

        Ok(reports)
    }

    #[instrument(skip(self), fields(exchange = %self.exchange_id))]
    async fn positions(&self) -> Result<Vec<Position>, BrokerError> {
        // The Exchange trait does not include fetch_positions (it lives on the
        // Margin sub-trait). We approximate positions from partially-filled
        // open orders. A production deployment would call the exchange-specific
        // positions endpoint directly.
        let client = self.build_client()?;

        let open_orders = client
            .fetch_open_orders(None)
            .await
            .map_err(|e| map_ccxt_error(&e))?;

        let positions = open_orders
            .into_iter()
            .filter_map(|order| {
                let filled = order.filled.unwrap_or(Decimal::ZERO);
                if filled.is_zero() {
                    return None;
                }
                let side = from_ccxt_side(order.side);
                let avg_price = order.average.or(order.price).unwrap_or(Decimal::ZERO);

                Some(
                    Position::builder()
                        .contract_id(order.symbol.as_str())
                        .side(side)
                        .quantity(filled)
                        .avg_entry_price(avg_price)
                        .unrealized_pnl(Decimal::ZERO)
                        .build(),
                )
            })
            .collect();

        Ok(positions)
    }

    #[instrument(skip(self), fields(exchange = %self.exchange_id))]
    async fn account_info(&self) -> Result<AccountInfo, BrokerError> {
        let client = self.build_client()?;

        let balance = client.fetch_balance().await.map_err(|e| map_ccxt_error(&e))?;

        // Sum all currency balances. A production system would convert to a
        // single quote currency for a meaningful equity figure.
        let (total_equity, available_cash) =
            balance
                .balances
                .values()
                .fold((Decimal::ZERO, Decimal::ZERO), |(eq, cash), entry| {
                    (eq + entry.total, cash + entry.free)
                });

        let positions = self.positions().await?;

        Ok(AccountInfo::builder()
            .total_equity(total_equity)
            .available_cash(available_cash)
            .positions(positions)
            .realized_pnl(Decimal::ZERO)
            .build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ccxt_broker_builder() {
        let broker = CcxtBroker::builder()
            .exchange_id("binance")
            .api_key("test-key")
            .secret("test-secret")
            .sandbox(true)
            .build();

        assert_eq!(broker.exchange_id, "binance");
        assert!(broker.sandbox);
        assert!(broker.passphrase.is_none());
    }

    #[test]
    fn test_ccxt_broker_builder_with_passphrase() {
        let broker = CcxtBroker::builder()
            .exchange_id("okx")
            .api_key("test-key")
            .secret("test-secret")
            .passphrase("test-pass")
            .sandbox(true)
            .build();

        assert_eq!(broker.exchange_id, "okx");
        assert_eq!(broker.passphrase.as_deref(), Some("test-pass"));
    }

    #[test]
    fn test_exchange_id_parsing() {
        assert_eq!(ExchangeId::parse("binance").unwrap(), ExchangeId::Binance);
        assert_eq!(ExchangeId::parse("okx").unwrap(), ExchangeId::Okx);
        assert_eq!(ExchangeId::parse("bybit").unwrap(), ExchangeId::Bybit);
        assert_eq!(ExchangeId::parse("BINANCE").unwrap(), ExchangeId::Binance);
        assert!(ExchangeId::parse("unsupported").is_err());
    }

    #[test]
    fn test_side_mapping() {
        assert_eq!(to_ccxt_side(Side::Buy), CcxtOrderSide::Buy);
        assert_eq!(to_ccxt_side(Side::Sell), CcxtOrderSide::Sell);
    }

    #[test]
    fn test_order_type_mapping() {
        assert!(matches!(
            to_ccxt_order_type(OrderType::Market),
            CcxtOrderType::Market
        ));
        assert!(matches!(
            to_ccxt_order_type(OrderType::Limit),
            CcxtOrderType::Limit
        ));
        assert!(matches!(
            to_ccxt_order_type(OrderType::StopLoss),
            CcxtOrderType::StopLoss
        ));
        assert!(matches!(
            to_ccxt_order_type(OrderType::StopLimit),
            CcxtOrderType::StopLimit
        ));
    }

    #[test]
    fn test_order_status_mapping() {
        assert_eq!(
            from_ccxt_order_status(CcxtOrderStatus::Open),
            OrderStatus::Submitted
        );
        assert_eq!(
            from_ccxt_order_status(CcxtOrderStatus::Closed),
            OrderStatus::Filled
        );
        assert_eq!(
            from_ccxt_order_status(CcxtOrderStatus::Cancelled),
            OrderStatus::Cancelled
        );
        assert_eq!(
            from_ccxt_order_status(CcxtOrderStatus::Expired),
            OrderStatus::Rejected
        );
    }

    #[test]
    fn test_side_roundtrip() {
        assert_eq!(from_ccxt_side(to_ccxt_side(Side::Buy)), Side::Buy);
        assert_eq!(from_ccxt_side(to_ccxt_side(Side::Sell)), Side::Sell);
    }

    #[test]
    fn test_build_client_binance() {
        let broker = CcxtBroker::builder()
            .exchange_id("binance")
            .api_key("test")
            .secret("test")
            .sandbox(true)
            .build();

        assert!(broker.build_client().is_ok());
    }

    #[test]
    fn test_build_client_okx() {
        let broker = CcxtBroker::builder()
            .exchange_id("okx")
            .api_key("test")
            .secret("test")
            .passphrase("test-pass")
            .sandbox(true)
            .build();

        assert!(broker.build_client().is_ok());
    }

    #[test]
    fn test_build_client_bybit() {
        let broker = CcxtBroker::builder()
            .exchange_id("bybit")
            .api_key("test")
            .secret("test")
            .sandbox(true)
            .build();

        assert!(broker.build_client().is_ok());
    }

    #[test]
    fn test_build_client_unsupported() {
        let broker = CcxtBroker::builder()
            .exchange_id("kraken")
            .api_key("test")
            .secret("test")
            .build();

        assert!(broker.build_client().is_err());
    }

    #[test]
    fn test_ccxt_broker_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CcxtBroker>();
    }

    #[test]
    fn test_ccxt_error_classification() {
        let auth_err =
            map_ccxt_error(&ccxt_rust::prelude::Error::authentication("invalid apiKey"));
        assert!(matches!(auth_err, BrokerError::Authentication { .. }));

        let exchange_err =
            map_ccxt_error(&ccxt_rust::prelude::Error::exchange("exchange", "server error"));
        assert!(matches!(exchange_err, BrokerError::Exchange { .. }));
    }
}
