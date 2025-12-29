use crate::bus::types::Bus;
use crate::config::config::FinJuiceCfg;
use crate::core::types::{Actor, RawNews};
use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

pub struct FinJuiceActor {
    pub bus: Bus,
    pub client: Client,
    pub cfg: FinJuiceCfg,
    pub shutdown: CancellationToken,
}

fn parse_date_published_to_utc(s: &str) -> Option<DateTime<Utc>> {
    // Example: "2025-11-14T16:51:20.647"
    // Treat as UTC (good enough for MVP; if it’s server-local you’re off by at most a few hours)
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .map(|naive| Utc.from_utc_datetime(&naive))
}

fn extract_inner_json(xml: &str) -> Result<String> {
    // 1) Find the <string ...> tag
    let string_tag_start = xml
        .find("<string")
        .ok_or_else(|| anyhow::anyhow!("no <string> tag found in XML"))?;

    // 2) Find the end of that tag ('>') starting from there
    let after_open_gt = xml[string_tag_start..]
        .find('>')
        .map(|i| string_tag_start + i + 1)
        .ok_or_else(|| anyhow::anyhow!("no '>' after <string> tag"))?;

    // 3) Find the closing </string>
    let close_tag_start = xml[after_open_gt..]
        .find("</string>")
        .map(|i| after_open_gt + i)
        .ok_or_else(|| anyhow::anyhow!("no </string> closing tag found"))?;

    Ok(xml[after_open_gt..close_tag_start].trim().to_string())
}

pub fn parse_fj_response_to_raw(xml: &str) -> Result<Vec<RawNews>> {
    let json_str = extract_inner_json(xml)?;
    let v: Value = serde_json::from_str(&json_str).context("parsing FJ JSON")?;

    let news_items = v["News"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("FJ JSON missing 'News' array"))?;

    let mut out = Vec::with_capacity(news_items.len());

    for item in news_items {
        // Safely pull fields with sane defaults
        let title = item
            .get("Title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if title.is_empty() {
            continue; // skip broken entries
        }

        let description = item
            .get("Description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();

        let url = item
            .get("EURL")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();

        let labels = item
            .get("Labels")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let date_published_str = item
            .get("DatePublished")
            .and_then(Value::as_str)
            .unwrap_or("");

        let published = parse_date_published_to_utc(date_published_str);

        let rn = RawNews {
            feed: "FinancialJuice".to_string(),
            title: title.to_string(),
            url: url.to_string(),
            labels: labels,
            published: published,
            description: description.to_string(),
        };

        out.push(rn);
    }

    Ok(out)
}

impl FinJuiceActor {
    pub fn new(
        bus: Bus,
        client: Client,
        cfg: FinJuiceCfg,
        shutdown: CancellationToken,
    ) -> FinJuiceActor {
        Self {
            bus,
            client,
            cfg,
            shutdown,
        }
    }

    fn get_api_url(&self) -> String {
        if !self.cfg.alt_url.is_empty() {
            return format!(
                "{}?info={}&TimeOffSet=1&tabID=0&oldID=0&TickerID=0&FeedCompanyID=0&strSearch=&extraNID=0",
                self.cfg.alt_url, self.cfg.info,
            );
        }
        format!("{}/news", self.cfg.base_url)
    }

    async fn fetch_data_from_api(&self) -> Result<Vec<RawNews>> {
        let start = std::time::Instant::now();
        let url = self.get_api_url();

        let xml = self
            .client
            .get(url)
            .header("Origin", self.cfg.base_url.to_string())
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        parse_fj_response_to_raw(&xml).map(|events| {
            metrics::histogram!("finjuice_fetch_duration_seconds")
                .record(start.elapsed().as_secs_f64());
            events
        })
    }
}
#[async_trait::async_trait]
impl Actor for FinJuiceActor {
    async fn run(mut self) -> Result<()> {
        info!("FinJuiceActor started");

        let mut tick = interval(Duration::from_secs(self.cfg.refresh.as_secs()));

        loop {
            tokio::select! {
                // Graceful shutdown signal
                _ = self.shutdown.cancelled() => {
                    info!("FinJuiceActor: shutdown requested");
                    break;
                }

                _ = tick.tick() => {
                    match self.fetch_data_from_api().await {
                        Ok(events) => {
                            metrics::counter!("finjuice_fetches_total", "status" => "success").increment(1);
                            metrics::counter!("finjuice_items_fetched_total").increment(events.len() as u64);
                            for n in events {
                                if let Err(e) = self.bus.raw_news.publish(n).await {
                                    tracing::warn!(?e, "publish raw news failed");
                                }
                            }
                        }
                         Err(e) => {
                            metrics::counter!("finjuice_fetches_total", "status" => "error").increment(1);
                            error!("FinJuiceActor: failed to fetch news from FinJuice: {:#}", e);
                            // backoff to avoid hot loop on repeated failures
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            }
        }

        info!("FinJuiceActor stopped cleanly");
        Ok(())
    }
}
