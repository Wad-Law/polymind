mod bus;
mod marketdata;
mod strategy;
mod execution;
mod polymarket;
mod rss;
mod finjuice;
mod core;
mod config;

use std::time::Duration;
use anyhow::Result;
use reqwest::Client;
use tracing::{error, info, info_span, Instrument};
use tokio_util::sync::CancellationToken;
use bus::types::Bus;
use execution::actor::ExecutionActor;
use finjuice::actor::FinJuiceActor;
use marketdata::actor::MarketDataActor;
use strategy::actor::StrategyActor;
use config::config::AppCfg;
use core::types::Actor;
use polymarket::actor::PolyActor;
use rss::actor::RssActor;

#[tokio::main]
async fn main()  -> Result<()>   {
    tracing_subscriber::fmt::init();

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
        .user_agent(cfg.http.user_agent)
        .pool_idle_timeout(cfg.http.poolIdleTimeout)
        .pool_max_idle_per_host(cfg.http.poolMaxIdlePerHost)
        .tcp_keepalive(cfg.http.tcpKeepAlive)
        .timeout(cfg.http.timeout)
        .build()
        .expect("client");

    info!("Building actors");
    let poly = PolyActor::new(bus.clone(), client.clone(), cfg.polymarket.clone(), shutdown.clone());
    let rss  = RssActor::new(bus.clone(), client.clone(), cfg.rss.clone(), shutdown.clone());
    let fj   = FinJuiceActor::new(bus.clone(), client.clone(), cfg.financial_juice.clone(), shutdown.clone());
    let market_data = MarketDataActor::new(bus.clone(), shutdown.clone());
    let strat = StrategyActor::new(bus.clone(), shutdown.clone());
    let exec = ExecutionActor::new(bus.clone(), shutdown.clone());

    info!("Spawning actors");
    let mut actors = tokio::task::JoinSet::new();

    actors.spawn(poly.run().instrument(info_span!("PolyMarket")));
    actors.spawn(rss.run().instrument(info_span!("RSS")));
    actors.spawn(fj.run().instrument(info_span!("FinancialJuice")));
    actors.spawn(market_data.run().instrument(info_span!("MarketData")));
    actors.spawn(strat.run().instrument(info_span!("Strat")));
    actors.spawn(exec.run().instrument(info_span!("Exec")));

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
            Ok(Ok(()))  => info!("Actor exited cleanly"),
            Ok(Err(e))  => error!(?e, "Actor returned error"),
            Err(panic)  => error!(?panic, "Actor panicked/cancelled"),
        }
    }


    info!("Supervisor exit");
    Ok(())
}
