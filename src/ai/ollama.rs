use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;

use super::provider::{
    AiProvider, AiProviderError, AiRequest, AiResponse, parse_openai_compatible_response,
};
use crate::config::OllamaConfig;

/// Talks to a local OpenAI-compatible `/v1/chat/completions` server --
/// Ollama, LM Studio, or anything else exposing that same API shape (only
/// `base_url`/`model` need to change; the wire format is identical). No
/// real API key is required; these servers accept (and ignore) any
/// placeholder `Authorization` value. Unlike Groq there is no
/// model-fallback list -- a local server only ever has the one model
/// loaded, so a failure just retries the same model.
pub struct OllamaProvider {
    client: Client,
    base_url: String,
    config: OllamaConfig,
}

impl OllamaProvider {
    pub fn from_config(config: &OllamaConfig) -> Result<Self, AiProviderError> {
        if !config.enabled {
            return Err(AiProviderError::Disabled);
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_seconds))
            .build()
            .map_err(|e| AiProviderError::HttpError(e.to_string()))?;
        Ok(Self {
            client,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            config: config.clone(),
        })
    }
}

#[async_trait]
impl AiProvider for OllamaProvider {
    async fn chat(&self, request: AiRequest) -> Result<AiResponse, AiProviderError> {
        let url = format!("{}/chat/completions", self.base_url);

        let mut messages: Vec<serde_json::Value> = Vec::new();
        if let Some(system) = &request.system_prompt {
            messages.push(serde_json::json!({"role": "system", "content": system}));
        }
        for msg in &request.messages {
            messages.push(serde_json::to_value(msg).map_err(|e| {
                AiProviderError::InvalidResponse(format!("failed to serialize message: {e}"))
            })?);
        }

        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": messages,
            "temperature": self.config.temperature,
            // The classic `max_tokens` field, not the newer
            // `max_completion_tokens` -- Ollama maps this to its native
            // `num_predict`, and it's the more universally supported name
            // across other OpenAI-compatible servers (LM Studio, etc.).
            "max_tokens": self.config.max_tokens,
        });

        if !request.tools.is_empty() {
            body["tools"] = serde_json::to_value(&request.tools)
                .map_err(|e| AiProviderError::InvalidResponse(e.to_string()))?;
        }

        let mut last_error = AiProviderError::HttpError("no attempts made".into());

        for attempt in 0..=self.config.max_request_retries {
            let response = self
                .client
                .post(&url)
                .header("Authorization", "Bearer local")
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await;

            match response {
                Ok(r) if r.status().is_success() => {
                    return parse_openai_compatible_response(r.json().await.map_err(|e| {
                        AiProviderError::InvalidResponse(format!("decode failed: {e}"))
                    })?);
                }
                Ok(r) => {
                    let status = r.status();
                    let body_text = r.text().await.unwrap_or_default();
                    match status.as_u16() {
                        404 => {
                            // A wrong path, or (Ollama specifically) "model
                            // not pulled" -- the body text is the only way
                            // to tell those apart.
                            return Err(AiProviderError::UnknownModel(format!(
                                "'{}' not found on the local server at {}: {body_text}. If this is Ollama, is it pulled (`ollama pull {}`)? If this is LM Studio, is it loaded? Is the server running?",
                                self.config.model, self.base_url, self.config.model
                            )));
                        }
                        400 => {
                            return Err(AiProviderError::InvalidRequest(format!(
                                "Local AI server request failed with HTTP {status}: {body_text}"
                            )));
                        }
                        _ => {
                            last_error = AiProviderError::ApiError(format!(
                                "Local AI server request failed with HTTP {status}: {body_text}"
                            ));
                            if attempt < self.config.max_request_retries {
                                tokio::time::sleep(Duration::from_millis(
                                    300 * (attempt + 1) as u64,
                                ))
                                .await;
                                continue;
                            }
                        }
                    }
                }
                Err(e) => {
                    let connect_hint = if e.is_connect() {
                        format!(
                            " Is a local AI server (Ollama: `ollama serve`, or LM Studio's local server) running at {}?",
                            self.base_url
                        )
                    } else {
                        String::new()
                    };
                    last_error = if e.is_timeout() {
                        AiProviderError::Timeout
                    } else {
                        AiProviderError::HttpError(format!(
                            "Local AI server request failed: {e}{connect_hint}"
                        ))
                    };
                    if attempt < self.config.max_request_retries {
                        tokio::time::sleep(Duration::from_millis(300 * (attempt + 1) as u64)).await;
                        continue;
                    }
                }
            }
        }

        Err(last_error)
    }
}
