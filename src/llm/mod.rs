use crate::config::config::LlmCfg;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::info;

#[derive(Clone)]
pub struct LlmClient {
    client: Client,
    cfg: LlmCfg,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignalResponse {
    pub sentiment: String, // "Positive", "Negative", "Neutral"
    pub confidence: f64,   // 0.0 to 1.0
    pub reasoning: String,
}

impl LlmClient {
    pub fn new(cfg: LlmCfg) -> Self {
        Self {
            client: Client::new(),
            cfg,
        }
    }

    pub fn model(&self) -> &str {
        &self.cfg.model
    }

    pub async fn analyze(&self, news_title: &str, market_question: &str, outcomes: &[String]) -> Result<(SignalResponse, String)> {
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

        let req_body = json!({
            "model": self.cfg.model,
            "messages": [
                {"role": "system", "content": "You are a helpful assistant that outputs JSON."},
                {"role": "user", "content": prompt}
            ],
            "temperature": 0.0
        });

        let url = format!("{}/chat/completions", self.cfg.base_url);

        // Log for debugging (don't log full key in prod)
        info!("Calling LLM at {} with model {}", url, self.cfg.model);

        let res = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.cfg.api_key))
            .json(&req_body)
            .send()
            .await
            .context("LLM request failed")?;

        if !res.status().is_success() {
            let err_text = res.text().await?;
            anyhow::bail!("LLM API error: {}", err_text);
        }

        let resp_json: serde_json::Value = res.json().await?;

        // Extract content from OpenAI-like response
        let content_str = resp_json["choices"][0]["message"]["content"]
            .as_str()
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
