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
    #[serde(default)]
    pub llm: LlmCfg,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LlmCfg {
    #[serde(default = "default_llm_api_key")]
    pub api_key: String,
    #[serde(default = "default_llm_model")]
    pub model: String,
    #[serde(default = "default_llm_base_url", rename = "baseUrl")]
    pub base_url: String,
    #[serde(default = "default_llm_rate_limit", rename = "rateLimitRpm")]
    pub rate_limit_rpm: u32,
}

impl Default for LlmCfg {
    fn default() -> Self {
        Self {
            api_key: default_llm_api_key(),
            model: default_llm_model(),
            base_url: default_llm_base_url(),
            rate_limit_rpm: default_llm_rate_limit(),
        }
    }
}

fn default_llm_api_key() -> String {
    "".to_string()
}

fn default_llm_model() -> String {
    "gpt-5-nano".to_string()
}

fn default_llm_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}

fn default_llm_rate_limit() -> u32 {
    500
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

    #[serde(rename = "tokenDecimals", default = "default_token_decimals")]
    pub token_decimals: u32,
    #[serde(rename = "rpcUrl")]
    pub rpc_url: String,
    #[serde(rename = "dataApiUrl")]
    pub data_api_url: String,
    #[serde(default, rename = "privateKey")]
    pub private_key: String,
    #[serde(default, rename = "proxyAddress")]
    pub proxy_address: Option<String>,
    #[serde(default, rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(default, rename = "apiSecret")]
    pub api_secret: Option<String>,
    #[serde(default, rename = "apiPassphrase")]
    pub api_passphrase: Option<String>,
}

impl Default for PolyCfg {
    fn default() -> Self {
        Self {
            base_url: "https://clob.polymarket.com".to_string(),
            gamma_events_url: "https://gamma-api.polymarket.com/events".to_string(),
            gamma_markets_url: "https://gamma-api.polymarket.com/markets".to_string(),
            market_list_refresh: Duration::from_secs(300),
            page_limit: default_page_limit(),

            token_decimals: default_token_decimals(),
            rpc_url: "https://polygon-rpc.com".to_string(),
            data_api_url: "https://data-api.polymarket.com".to_string(),
            private_key: "".to_string(),
            proxy_address: None,
            api_key: None,
            api_secret: None,
            api_passphrase: None,
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
}

impl Default for RssFeedCfg {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            url: "http://localhost".to_string(),
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
}

impl Default for FinJuiceCfg {
    fn default() -> Self {
        Self {
            base_url: "http://localhost".to_string(),
            refresh: Duration::from_secs(60),
            alt_url: "http://localhost".to_string(),
            info: "".to_string(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct StrategyCfg {
    #[serde(default, rename = "simExecution")]
    pub sim_execution: bool,
    #[serde(default, rename = "simMarketData")]
    pub sim_market_data: bool,
    #[serde(default = "default_top_candidates", rename = "topCandidates")]
    pub top_candidates: usize,
    #[serde(
        default = "default_max_pos_drawdown",
        rename = "maxPositionDrawdownPct"
    )]
    pub max_position_drawdown_pct: f64,
}

fn default_top_candidates() -> usize {
    5
}

fn default_max_pos_drawdown() -> f64 {
    0.20 // 20%
}

impl AppCfg {
    pub fn load(path: &str) -> Result<Self> {
        let mut builder = Config::builder()
            .add_source(File::with_name(path))
            .add_source(config::Environment::default().separator("__"));

        // Manual overrides for standard environment variables (README compliant)
        if let Ok(key) = std::env::var("LLM_API_KEY") {
            builder = builder
                .set_override("llm.api_key", key)
                .context("setting LLM_API_KEY")?;
        }
        if let Ok(key) = std::env::var("POLY_PRIVATE_KEY") {
            builder = builder
                .set_override("polymarket.privateKey", key)
                .context("setting POLY_PRIVATE_KEY")?;
        }
        if let Ok(addr) = std::env::var("POLY_PROXY_ADDRESS") {
            builder = builder
                .set_override("polymarket.proxyAddress", addr)
                .context("setting POLY_PROXY_ADDRESS")?;
        }
        if let Ok(k) = std::env::var("POLY_API_KEY") {
            builder = builder
                .set_override("polymarket.apiKey", k)
                .context("setting POLY_API_KEY")?;
        }
        if let Ok(s) = std::env::var("POLY_API_SECRET") {
            builder = builder
                .set_override("polymarket.apiSecret", s)
                .context("setting POLY_API_SECRET")?;
        }
        if let Ok(p) = std::env::var("POLY_API_PASSPHRASE") {
            builder = builder
                .set_override("polymarket.apiPassphrase", p)
                .context("setting POLY_API_PASSPHRASE")?;
        }

        let cfg = builder.build().context("building config")?;

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
