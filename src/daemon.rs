//! Unified daemon orchestrator — runs research, paper trading, feedback, and
//! gRPC server as concurrent tokio tasks in a single process.

use std::sync::Arc;

use snafu::ResultExt;
use tokio::task::JoinSet;
use tracing::{error, info};

use crate::app_config;
use crate::error::{self, EventBusSnafu};
use crate::event_bus::bus::EventBus;
use crate::paths;

/// Run the unified daemon: spawn all trading-loop components as concurrent
/// tokio tasks and wait for shutdown (Ctrl+C) or a fatal task error.
pub async fn run(contracts: String, iterations: u32, grpc_addr: String) -> error::Result<()> {
    let _cfg = app_config::load();

    // Persistent event bus shared across all components
    let trace_path = paths::data_dir().join("trace");
    let event_bus = Arc::new(EventBus::open(&trace_path.join("events")).context(EventBusSnafu)?);

    let contract_list: Vec<String> = contracts
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();

    info!(
        contracts = ?contract_list,
        iterations,
        grpc_addr = %grpc_addr,
        "daemon starting"
    );

    let mut tasks = JoinSet::new();

    // --- gRPC server task ---
    // TODO: spawn rara-server gRPC service once the crate is available on main.
    // Expected usage:
    //   let addr = grpc_addr.parse().expect("valid socket addr");
    //   let svc = rara_server::build_service(Arc::clone(&event_bus));
    //   tasks.spawn(async move { tonic::transport::Server::builder()
    //       .add_service(svc).serve(addr).await });
    let grpc_addr_clone = grpc_addr.clone();
    tasks.spawn(async move {
        info!(addr = %grpc_addr_clone, "gRPC server placeholder — waiting for rara-server crate");
        // Block until cancelled so the task stays alive in the JoinSet
        std::future::pending::<()>().await;
        Ok::<(), error::AppError>(())
    });

    // --- Research loop task ---
    if iterations > 0 {
        let bus = Arc::clone(&event_bus);
        let contract = contract_list.first().cloned().unwrap_or_default();
        tasks.spawn(async move {
            info!(iterations, contract = %contract, "research loop placeholder — wire up ResearchLoop here");
            // TODO: build and run ResearchLoop using the same pattern as
            // `run_research_loop` in main.rs, passing `bus` for event publishing.
            let _ = bus;
            Ok::<(), error::AppError>(())
        });
    }

    // --- Paper trading tasks (one per contract) ---
    for contract in &contract_list {
        let contract = contract.clone();
        let bus = Arc::clone(&event_bus);
        tasks.spawn(async move {
            info!(contract = %contract, "paper trading placeholder — wire up WS + aggregator + signal loop");
            // TODO: connect to exchange WS, run candle aggregator, feed
            // promoted strategies, and publish fills to event bus.
            let _ = bus;
            std::future::pending::<()>().await;
            Ok::<(), error::AppError>(())
        });
    }

    // --- Feedback consumer task ---
    {
        let bus = Arc::clone(&event_bus);
        tasks.spawn(async move {
            info!("feedback consumer placeholder — wire up event-driven feedback loop");
            // TODO: subscribe to event bus, consume paper-trading fills,
            // generate feedback, and publish results.
            let _ = bus;
            std::future::pending::<()>().await;
            Ok::<(), error::AppError>(())
        });
    }

    info!("daemon running — press Ctrl+C to shut down");

    // Wait for either a task to complete (crash) or Ctrl+C
    tokio::select! {
        result = tasks.join_next() => {
            match result {
                Some(Ok(Ok(()))) => {
                    info!("a task completed normally — initiating shutdown");
                }
                Some(Ok(Err(e))) => {
                    error!(error = %e, "a task failed — initiating shutdown");
                }
                Some(Err(join_err)) => {
                    error!(error = %join_err, "a task panicked — initiating shutdown");
                }
                None => {
                    info!("all tasks completed — shutting down");
                }
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("received Ctrl+C — shutting down");
        }
    }

    // Cancel all remaining tasks
    tasks.abort_all();

    // Drain the JoinSet so all tasks have a chance to drop cleanly
    while tasks.join_next().await.is_some() {}

    info!("daemon stopped");
    Ok(())
}
