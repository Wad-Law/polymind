mod bus;
mod config;
mod core;
mod discovery;
mod execution;
mod finjuice;
mod llm;
mod marketdata;
mod persistence;
mod risk;
mod rss;
mod strategy;

use anyhow::Result;
use bus::types::Bus;
use config::config::AppCfg;
use core::types::Actor;
use discovery::actor::MarketDiscoveryActor;
use finjuice::actor::FinJuiceActor;
use reqwest::Client;
use rss::actor::RssActor;

use risk::actor::RiskActor;
use strategy::actor::StrategyActor;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, error, info, info_span};

use marketdata::actor::MarketPricingActor;
use marketdata::client::MarketDataClient;
use marketdata::polymarket::PolyMarketDataClient;
use marketdata::simulator::SimMarketDataClient;

use execution::actor::ExecutionActor;
use execution::client::ExecutionClient;
use execution::polymarket::PolyExecutionClient;
use execution::simulator::SimExecutionClient;
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;

use persistence::database::Database;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(false)
        .init();
    dotenv::dotenv().ok();

    // Initialize Prometheus Exporter
    let builder = metrics_exporter_prometheus::PrometheusBuilder::new();
    let handle = builder
        .install_recorder()
        .expect("failed to install Prometheus recorder");

    // Spawn Metrics Server
    tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/metrics",
            axum::routing::get(move || {
                let handle = handle.clone();
                async move { handle.render() }
            }),
        );
        let listener = tokio::net::TcpListener::bind("0.0.0.0:9000").await.unwrap();
        info!("Metrics endpoint listening on 0.0.0.0:9000/metrics");
        axum::serve(listener, app).await.unwrap();
    });

    metrics::counter!("polymind_startups_total").increment(1);

    // Spawn System Metrics Collector
    tokio::spawn(async move {
        use sysinfo::{Pid, System};
        let mut sys = System::new_all();
        let pid = Pid::from_u32(std::process::id());

        // Initial refresh
        sys.refresh_process(pid);

        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            sys.refresh_process(pid);

            if let Some(process) = sys.process(pid) {
                let cpu = process.cpu_usage();
                let mem = process.memory();
                let virt = process.virtual_memory();

                metrics::gauge!("process_cpu_usage_ratio").set(cpu as f64);
                metrics::gauge!("process_resident_memory_bytes").set(mem as f64);
                metrics::gauge!("process_virtual_memory_bytes").set(virt as f64);
                // Thread count (if available on OS)
                // metrics::gauge!("process_threads_total").set(...) - Not reliable in cross-platform sysinfo 0.30 without specialized handling.
                // Keeping it simple as requested.

                // Add heartbeat
                metrics::counter!("process_collector_heartbeat").increment(1);
                info!(
                    "System metrics collected: CPU: {:.2}%, Mem: {} bytes",
                    cpu, mem
                );
            } else {
                error!("Failed to find own process ID for metrics collection");
            }
        }
    });

    let cfg = AppCfg::load("config.yml")?;

    // Root span for the supervisor/main thread
    let span = info_span!(
        "Supervisor",
        pid = %std::process::id(),
        version = env!("CARGO_PKG_VERSION"),
    );

    // logs below are inside "Supervisor"
    let _enter = span.enter();

    info!("Starting up");

    info!("Initializing shared pub/sub Bus");
    let bus = Bus::new();
    let shutdown = CancellationToken::new();

    info!("Initializing Client");
    let client = Client::builder()
        .use_rustls_tls()
        .user_agent(cfg.http.user_agent.clone())
        .pool_idle_timeout(cfg.http.pool_idle_timeout)
        .pool_max_idle_per_host(cfg.http.pool_max_idle_per_host)
        .tcp_keepalive(cfg.http.tcp_keep_alive)
        .timeout(cfg.http.timeout)
        .build()
        .expect("client");

    info!("Building actors");
    let poly = MarketDiscoveryActor::new(
        bus.clone(),
        client.clone(),
        cfg.polymarket.clone(),
        shutdown.clone(),
    );
    let rss = RssActor::new(
        bus.clone(),
        client.clone(),
        cfg.rss.clone(),
        shutdown.clone(),
    );
    let fj = FinJuiceActor::new(
        bus.clone(),
        client.clone(),
        cfg.financial_juice.clone(),
        shutdown.clone(),
    );

    let md_client: std::sync::Arc<dyn MarketDataClient> = if cfg.strategy.sim_market_data {
        info!("Running in SIMULATED MARKET DATA mode");
        std::sync::Arc::new(SimMarketDataClient::new())
    } else {
        info!("Running in LIVE MARKET DATA mode");
        std::sync::Arc::new(PolyMarketDataClient::new(
            cfg.polymarket.clone(),
            client.clone(),
        ))
    };

    let market_data = MarketPricingActor::new(bus.clone(), md_client, shutdown.clone());
    let risk = RiskActor::new(bus.clone(), shutdown.clone());

    info!("Initializing Database using Postgres URL");
    // Default to a common postgres local url if not set
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://user:password@localhost:5432/polymind".to_string());

    let db = Database::new(&db_url)
        .await
        .expect("Failed to init database");

    let strat = StrategyActor::new(bus.clone(), shutdown.clone(), &cfg, db.clone());

    let exec_client: std::sync::Arc<dyn ExecutionClient> = if cfg.strategy.sim_execution {
        info!("Running in SIMULATED EXECUTION mode");
        let initial_cash = Decimal::from_f64(cfg.strategy.sim_bankroll).unwrap_or(Decimal::ZERO);
        std::sync::Arc::new(SimExecutionClient::new(initial_cash))
    } else {
        info!("Running in LIVE EXECUTION mode");
        std::sync::Arc::new(PolyExecutionClient::new(
            cfg.polymarket.clone(),
            client.clone(),
        ))
    };

    let exec = ExecutionActor::new(bus.clone(), shutdown.clone(), exec_client);

    info!("Spawning actors");
    let mut actors = tokio::task::JoinSet::new();

    actors.spawn(poly.run().instrument(info_span!("PolyMarket")));
    actors.spawn(rss.run().instrument(info_span!("RSS")));
    actors.spawn(fj.run().instrument(info_span!("FinancialJuice")));
    actors.spawn(market_data.run().instrument(info_span!("MarketData")));
    actors.spawn(strat.run().instrument(info_span!("Strat")));
    actors.spawn(exec.run().instrument(info_span!("Exec")));
    actors.spawn(risk.run().instrument(info_span!("Risk")));

    info!("Waiting for actors");

    tokio::select! {
        _ = async {
             while let Some(res) = actors.join_next().await {
                 match res {
                    Ok(Ok(()))  => info!("Actor exited cleanly"),
                    Ok(Err(e))  => error!(?e, "Actor returned error"),
                    Err(panic)  => error!(?panic, "Actor panicked/cancelled"),
                }
            }
        } => {  }
        _ = tokio::signal::ctrl_c() => {
            info!("Ctrl-C received, shutting down supervisor loop");
            shutdown.cancel();
        }
    }

    info!("Waiting for graceful shutdown of actors");
    while let Some(res) = actors.join_next().await {
        match res {
            Ok(Ok(())) => info!("Actor exited cleanly"),
            Ok(Err(e)) => error!(?e, "Actor returned error"),
            Err(panic) => error!(?panic, "Actor panicked/cancelled"),
        }
    }

    info!("Supervisor exit");
    Ok(())
}
