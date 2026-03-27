//! Application-level error types.

use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum AppError {
    #[snafu(display("IO error: {source}"))]
    Io { source: std::io::Error },

    #[snafu(display("HTTP error: {source}"))]
    Http { source: reqwest::Error },

    #[snafu(display("JSON error: {source}"))]
    Json { source: serde_json::Error },

    #[snafu(display("config error: {message}"))]
    Config { message: String },

    #[snafu(display("agent execution failed: {source}"))]
    AgentExecution { source: std::io::Error },

    #[snafu(display("agent backend error: {source}"))]
    AgentBackend {
        source: crate::agent::backend::BackendError,
    },

    #[snafu(display("research loop error: {source}"))]
    Research {
        source: crate::research::research_loop::ResearchLoopError,
    },

    #[snafu(display("trace storage error: {source}"))]
    Trace {
        source: crate::research::trace::TraceError,
    },

    #[snafu(display("event bus error: {source}"))]
    EventBus {
        source: crate::event_bus::store::StoreError,
    },

    #[snafu(display("prompt renderer error: {source}"))]
    PromptRenderer {
        source: crate::research::prompt_renderer::PromptError,
    },

    #[snafu(display("strategy promoter error: {source}"))]
    Promoter {
        source: crate::research::strategy_promoter::PromoterError,
    },

    #[snafu(display("market data store error: {source}"))]
    MarketStore {
        source: rara_market_data::store::StoreError,
    },

    #[snafu(display("data fetch error: {source}"))]
    DataFetch {
        source: rara_market_data::fetcher::FetchError,
    },

    #[snafu(display("gRPC server error: {source}"))]
    GrpcServe { source: tonic::transport::Error },

    #[snafu(display("TUI error: {source}"))]
    Tui {
        source: Box<rara_tui::error::TuiError>,
    },

    #[snafu(display("strategy registry error: {source}"))]
    Registry {
        source: crate::research::strategy_registry::RegistryError,
    },
}

pub type Result<T> = std::result::Result<T, AppError>;
