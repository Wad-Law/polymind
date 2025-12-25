use anyhow::{Context, Result};
use config::{Config, File};
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Deserialize, Clone, Default)]
pub struct AppCfg {
    pub http: HttpCfg,
    pub polymarket: PolyCfg,
    pub rss: RssCfg,
    #[serde(rename = "financialJuice")]
    pub financial_juice: FinJuiceCfg,
    #[serde(default)]
    pub strategy: StrategyCfg,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HttpCfg {
    #[serde(rename = "userAgent", default = "default_ua")]
    pub user_agent: String,
    #[serde(with = "humantime_serde", default = "default_timeout")]
    pub timeout: Duration,
    #[serde(rename = "poolIdleTimeout", with = "humantime_serde")]
    pub pool_idle_timeout: Duration,
    #[serde(rename = "tcpKeepAlive", with = "humantime_serde")]
    pub tcp_keep_alive: Duration,
    #[serde(rename = "poolMaxIdlePerHost", default = "default_pool")]
    pub pool_max_idle_per_host: usize,
}

impl Default for HttpCfg {
    fn default() -> Self {
        Self {
            user_agent: default_ua(),
            timeout: default_timeout(),
            pool_idle_timeout: Duration::from_secs(90),
            tcp_keep_alive: Duration::from_secs(60),
            pool_max_idle_per_host: default_pool(),
        }
    }
}
fn default_ua() -> String {
    "polymind/0.1".into()
}
fn default_timeout() -> Duration {
    Duration::from_secs(10)
}
fn default_pool() -> usize {
    16
}

#[derive(Debug, Deserialize, Clone)]
pub struct PolyCfg {
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    #[serde(rename = "gammaEventsUrl")]
    pub gamma_events_url: String,
    #[serde(rename = "gammaMarketsUrl")]
    pub gamma_markets_url: String,
    #[serde(rename = "marketListRefresh", with = "humantime_serde")]
    pub market_list_refresh: Duration,
    #[serde(rename = "pageLimit", default = "default_page_limit")]
    pub page_limit: u32,
    #[serde(default)]
    pub ascending: bool,
    #[serde(rename = "includeClosed", default)]
    pub include_closed: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_secret: String,
    #[serde(default)]
    pub passphrase: String,
    #[serde(rename = "tokenDecimals", default = "default_token_decimals")]
    pub token_decimals: u32,
}

impl Default for PolyCfg {
    fn default() -> Self {
        Self {
            base_url: "https://clob.polymarket.com".to_string(),
            gamma_events_url: "https://gamma-api.polymarket.com/events".to_string(),
            gamma_markets_url: "https://gamma-api.polymarket.com/markets".to_string(),
            market_list_refresh: Duration::from_secs(300),
            page_limit: default_page_limit(),
            ascending: false,
            include_closed: false,
            api_key: "".to_string(),
            api_secret: "".to_string(),
            passphrase: "".to_string(),
            token_decimals: default_token_decimals(),
        }
    }
}
fn default_page_limit() -> u32 {
    100
}
fn default_token_decimals() -> u32 {
    6
}

#[derive(Debug, Deserialize, Clone)]
pub struct RssFeedCfg {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub lang: String,
}

impl Default for RssFeedCfg {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            url: "http://localhost".to_string(),
            lang: "en".to_string(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct RssCfg {
    #[serde(with = "humantime_serde")]
    pub refresh: Duration,
    pub concurrency: usize,
    pub feeds: Vec<RssFeedCfg>,
}

impl Default for RssCfg {
    fn default() -> Self {
        Self {
            refresh: Duration::from_secs(60),
            concurrency: 1,
            feeds: vec![RssFeedCfg::default()],
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct FinJuiceCfg {
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    #[serde(with = "humantime_serde")]
    pub refresh: Duration,
    #[serde(rename = "altUrl")]
    pub alt_url: String,
    pub info: String,
    pub cookie: String,
}

impl Default for FinJuiceCfg {
    fn default() -> Self {
        Self {
            base_url: "http://localhost".to_string(),
            refresh: Duration::from_secs(60),
            alt_url: "http://localhost".to_string(),
            info: "".to_string(),
            cookie: "".to_string(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct StrategyCfg {
    #[serde(default)]
    pub calibration: CalibrationCfg,
    #[serde(default = "default_bankroll")]
    pub bankroll: f64,
}

fn default_bankroll() -> f64 {
    1000.0
}

#[derive(Debug, Deserialize, Clone)]
pub struct CalibrationCfg {
    pub a: f64,
    pub b: f64,
    pub lambda: f64,
}

impl Default for CalibrationCfg {
    fn default() -> Self {
        Self {
            a: -3.0,
            b: 6.0,
            lambda: 0.5,
        }
    }
}

impl AppCfg {
    pub fn load(path: &str) -> Result<Self> {
        let cfg = Config::builder()
            .add_source(File::with_name(path))
            .add_source(config::Environment::default().separator("__"))
            .build()
            .context("building config")?;

        let app: AppCfg = cfg.try_deserialize().context("deserializing config")?;
        app.validate()?;
        Ok(app)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.polymarket.base_url.is_empty(),
            "polymarket.baseUrl missing"
        );
        anyhow::ensure!(
            !self.polymarket.gamma_events_url.is_empty(),
            "polymarket.gammaEventsUrl missing"
        );
        anyhow::ensure!(
            !self.polymarket.gamma_markets_url.is_empty(),
            "polymarket.gammaMarketsUrl missing"
        );
        anyhow::ensure!(self.rss.concurrency > 0, "rss.concurrency must be > 0");
        anyhow::ensure!(!self.rss.feeds.is_empty(), "rss.feeds must not be empty");
        anyhow::ensure!(
            !self.financial_juice.base_url.is_empty(),
            "financialJuice.baseUrl required in non-dev env"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_env_var_override() {
        // Set environment variable
        unsafe {
            env::set_var("POLYMARKET__API_KEY", "env-key-123");
        }

        // Test that config::Environment picks it up
        let cfg = Config::builder()
            .add_source(config::Environment::default().separator("__"))
            .build()
            .unwrap();

        let val = cfg.get_string("polymarket.api_key").unwrap();
        assert_eq!(val, "env-key-123");

        unsafe {
            env::remove_var("POLYMARKET__API_KEY");
        }
    }
}
