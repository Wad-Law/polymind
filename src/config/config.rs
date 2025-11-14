use serde::Deserialize;
use anyhow::{Result, Context};
use std::time::Duration;
use config::{Config, File};

#[derive(Debug, Deserialize, Clone)]
pub struct AppCfg {
    pub http: HttpCfg,
    pub polymarket: PolyCfg,
    pub rss: RssCfg,
    #[serde(rename = "financialJuice")]
    pub financial_juice: FinJuiceCfg,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HttpCfg {
    #[serde(rename = "userAgent", default = "default_ua")]
    pub user_agent: String,
    #[serde(with = "humantime_serde", default = "default_timeout")]
    pub timeout: Duration,
    #[serde(with = "humantime_serde")]
    pub poolIdleTimeout: Duration,
    #[serde(with = "humantime_serde")]
    pub tcpKeepAlive: Duration,
    #[serde(default = "default_pool")]
    pub poolMaxIdlePerHost: usize,
}
fn default_ua() -> String { "polymind/0.1".into() }
fn default_timeout() -> Duration { Duration::from_secs(10) }
fn default_pool() -> usize { 16 }

#[derive(Debug, Deserialize, Clone)]
pub struct PolyCfg {
    pub baseUrl: String,
    pub gammaUrl: String,
    #[serde(with = "humantime_serde")]
    pub marketListRefresh: Duration,
    #[serde(default = "default_page_limit")]
    pub pageLimit: u32,
    #[serde(default)]
    pub ascending: bool,
    #[serde(default)]
    pub includeClosed: bool,
}
fn default_page_limit() -> u32 { 100 }

#[derive(Debug, Deserialize, Clone)]
pub struct RssFeedCfg {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub lang: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RssCfg {
    #[serde(with = "humantime_serde")]
    pub refresh: Duration,
    pub concurrency: usize,
    pub feeds: Vec<RssFeedCfg>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FinJuiceCfg {
    pub baseUrl: String,
    #[serde(with = "humantime_serde")]
    pub refresh: Duration
}


impl AppCfg {
    pub fn load(path: &str) -> Result<Self> {
        let cfg = Config::builder()
            .add_source(File::with_name(path))
            .build()
            .context("building config")?;

        let app: AppCfg = cfg.try_deserialize().context("deserializing config")?;
        app.validate()?;
        Ok(app)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(!self.polymarket.baseUrl.is_empty(), "polymarket.baseUrl missing");
        anyhow::ensure!(!self.polymarket.gammaUrl.is_empty(), "polymarket.gammaUrl missing");
        anyhow::ensure!(self.rss.concurrency > 0, "rss.concurrency must be > 0");
        anyhow::ensure!(!self.rss.feeds.is_empty(), "rss.feeds must not be empty");
        anyhow::ensure!(!self.financial_juice.baseUrl.is_empty(), "financialJuice.baseUrl required in non-dev env");
        Ok(())
    }
}