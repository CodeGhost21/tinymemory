//! An optional model that answers each probe from its pack (`--llm`).
//!
//! The scripted agent's extractive answer only finds lines that share words
//! with the question, so it cannot answer a paraphrase even when the pack
//! holds the fact. A model reading the same pack shows whether the pack is
//! usable, which is what a host cares about.
//!
//! Any OpenAI-compatible chat endpoint works:
//!
//! - `EVAL_LLM_URL`: default `https://openrouter.ai/api/v1`.
//! - `EVAL_LLM_KEY`: default `OPENROUTER_API_KEY`.
//! - `EVAL_LLM_MODEL`: default `openai/gpt-4.1-mini`.
//!
//! It answers at temperature 0 and is scored like the extractive answer.

use serde_json::{Value, json};

/// What the model is told.
const SYSTEM: &str = "You are an assistant with a long-term memory. The user's message \
     starts with what your memory recalled, then their question. Answer the question in one \
     short sentence, using only the memory. If the memory does not say, answer \"unknown\".";

/// A chat model.
pub(crate) struct Llm {
    client: reqwest::Client,
    url: String,
    key: String,
    /// The model's id.
    pub(crate) model: String,
}

impl Llm {
    /// The model the environment names.
    ///
    /// # Errors
    ///
    /// When neither `EVAL_LLM_KEY` nor `OPENROUTER_API_KEY` is set.
    pub(crate) fn from_env() -> Result<Self, String> {
        let key = std::env::var("EVAL_LLM_KEY")
            .or_else(|_| std::env::var("OPENROUTER_API_KEY"))
            .map_err(|_| "--llm needs EVAL_LLM_KEY or OPENROUTER_API_KEY".to_string())?;
        Ok(Self {
            client: reqwest::Client::new(),
            url: std::env::var("EVAL_LLM_URL")
                .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_string())
                .trim_end_matches('/')
                .to_string(),
            key,
            model: std::env::var("EVAL_LLM_MODEL")
                .unwrap_or_else(|_| "openai/gpt-4.1-mini".to_string()),
        })
    }

    /// The model's answer to `question` given `pack`.
    ///
    /// # Errors
    ///
    /// A transport failure, or an answer without text.
    pub(crate) async fn answer(&self, pack: &str, question: &str) -> Result<String, String> {
        let body = json!({
            "model": self.model,
            "temperature": 0,
            "max_tokens": 120,
            "messages": [
                { "role": "system", "content": SYSTEM },
                { "role": "user", "content": format!("{pack}\n\nQuestion: {question}") },
            ],
        });
        let answer: Value = self
            .client
            .post(format!("{}/chat/completions", self.url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| error.to_string())?
            .json()
            .await
            .map_err(|error| error.to_string())?;
        answer
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(|text| text.trim().to_string())
            .ok_or_else(|| format!("no answer text in {answer}"))
    }
}
