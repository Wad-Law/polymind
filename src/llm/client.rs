use crate::config::config::LlmCfg;
use anyhow::{Context, Result};
use async_openai::{
    Client,
    config::OpenAIConfig,
    types::chat::{
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
        CreateChatCompletionRequestArgs,
    },
};
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::sync::Arc;
use tracing::info;

#[derive(Clone)]
pub struct LlmClient {
    client: Client<OpenAIConfig>,
    cfg: LlmCfg,
    // RateLimiter is internal state, wrap in Arc if LlmClient is cloned (LlmClient derives Clone)
    // governor::RateLimiter is thread-safe and shareable if wrapped or if its state handles it.
    // RateLimiter<NotKeyed, InMemoryState, DefaultClock> is shareable?
    // Actually, RateLimiter usually needs to be shared.
    // LlmClient is cloned in StrategyActor? Yes.
    // RateLimiter inside Arc? Correct.
    limiter: Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignalResponse {
    pub sentiment: String, // "Positive", "Negative", "Neutral"
    pub confidence: f64,   // 0.0 to 1.0
    pub reasoning: String,
}

impl LlmClient {
    pub fn new(cfg: LlmCfg) -> Self {
        // Governor initialization
        let rpm = NonZeroU32::new(cfg.rate_limit_rpm).unwrap_or(NonZeroU32::new(1).unwrap());
        let quota = Quota::per_minute(rpm);
        let limiter = Arc::new(RateLimiter::direct(quota));

        let openai_config = OpenAIConfig::new()
            .with_api_key(&cfg.api_key)
            .with_api_base(&cfg.base_url);

        // async-openai uses reqwest internally, but properly typed
        let client = Client::with_config(openai_config);

        Self {
            client,
            cfg,
            limiter,
        }
    }

    pub fn model(&self) -> &str {
        &self.cfg.model
    }

    pub async fn analyze(
        &self,
        news_title: &str,
        market_question: &str,
        outcomes: &[String],
    ) -> Result<(SignalResponse, String)> {
        // Enforce Rate Limit
        self.limiter.until_ready().await;

        let outcomes_list = outcomes.join(", ");
        let prompt = format!(
            "You are a financial analyst specializing in event-driven market prediction. Analyze the following news to determine if it predicts a specific outcome for the market.

            News: \"{}\"
            Market Question: \"{}\"
            Possible Outcomes: [{}]

            Perform the following analysis step-by-step:
            1. Identify key entities/events in the news.
            2. Determine if this news explicitly supports one of the Possible Outcomes.
            3. If the news creates a high conviction that a specific outcome will occur (or win), select it.
            4. If the news is irrelevant or ambiguous, select 'None'.

            Output strictly valid JSON with fields: 
            - 'sentiment' (The exact string of the selected outcome, or 'None'), 
            - 'confidence' (0.0 to 1.0, representing the strength of the prediction), 
            - 'reasoning' (A concise summary of your analysis).

            Example: If outcomes are [\"Yes\", \"No\"] and news strongly supports Yes, sentiment should be \"Yes\".",
            news_title, market_question, outcomes_list
        );

        // Models like o1-preview/o1-mini (reasoning models) do NOT support temperature.
        // gpt-4o, gpt-3.5 do.
        // We'll optionally set temperature only if not a reasoning model or let the lib handle default.
        // For strict JSON output, temperature 0 is good for standard GPT models.

        let request = CreateChatCompletionRequestArgs::default()
            .model(&self.cfg.model)
            .messages([
                ChatCompletionRequestSystemMessageArgs::default()
                    .content("You are a helpful assistant that outputs JSON.")
                    .build()?
                    .into(),
                ChatCompletionRequestUserMessageArgs::default()
                    .content(prompt.clone())
                    .build()?
                    .into(),
            ])
            // Do NOT set temperature for strict reasoning models if using o1 series.
            // If user uses gpt-4o-mini (default), temperature 0.0 is fine.
            // Safe bet: set it to 0.0 only if we know it's supported, or just leave it default (1.0).
            // However, 0.0 is better for deterministic code output.
            // Let's assume standard models for now as per config default.
            .build()?;

        info!(
            "Calling LLM at {} with model {}",
            self.cfg.base_url, self.cfg.model
        );

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .context("LLM request failed")?;

        let choice = response
            .choices
            .first()
            .context("No choices in LLM response")?;
        let content_str = choice
            .message
            .content
            .as_ref()
            .context("No content in LLM response")?;

        // Parse JSON from content (handle potential markdown code blocks)
        let clean_content = content_str
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```");

        let signal: SignalResponse = serde_json::from_str(clean_content)
            .context(format!("Failed to parse LLM JSON: {}", clean_content))?;

        Ok((signal, prompt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config::AppCfg;

    #[tokio::test]
    #[ignore] // Run with: cargo test -- --ignored
    async fn test_real_llm_call() -> Result<()> {
        // Load real config from file (ensure config.yml exists or env vars are set)
        let mut cfg = AppCfg::load("config.yml").expect("Failed to load config");
        cfg.llm.api_key = "api-key here".to_string();
        let client = LlmClient::new(cfg.llm.clone());
        println!("Testing with model: {}", client.model());

        // Test Case: Clear Positive
        let news = "Bitcoin officially approved as legal tender in the US today.";
        let question = "Will Bitcoin be legal tender in the US in 2025?";
        let outcomes = vec!["Yes".to_string(), "No".to_string()];

        let (signal, _prompt) = client.analyze(news, question, &outcomes).await?;

        println!("Response: {:?}", signal);

        assert_eq!(signal.sentiment, "Yes");
        assert!(signal.confidence > 0.8);

        Ok(())
    }
}
