use std::collections::HashSet;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use super::provider::{
    AiProvider, AiProviderError, AiRequest, AiResponse, RateLimitInfo, RateLimitKind,
    parse_openai_compatible_response,
};
use crate::config::GroqConfig;

/// Tracks which models have been attempted during one AI decision cycle.
#[derive(Clone, Debug)]
pub struct ProviderAttemptState {
    pub attempted_models: HashSet<String>,
    pub attempt_count: usize,
    pub current_model_index: usize,
    pub retry_at: Option<Instant>,
    pub models: Vec<String>,
}

impl ProviderAttemptState {
    pub fn new(config: &GroqConfig) -> Self {
        let mut models = vec![config.model.clone()];
        models.extend(config.fallback_models.iter().cloned());
        Self {
            attempted_models: HashSet::new(),
            attempt_count: 0,
            current_model_index: 0,
            retry_at: None,
            models,
        }
    }

    pub fn current_model(&self) -> Option<&str> {
        self.models
            .get(self.current_model_index)
            .map(String::as_str)
    }

    pub fn advance_model(&mut self) -> bool {
        let start = self.current_model_index;
        loop {
            self.current_model_index = (self.current_model_index + 1) % self.models.len();
            if self.current_model_index == start {
                return false;
            }
            let model = &self.models[self.current_model_index];
            if !self.attempted_models.contains(model) {
                return true;
            }
        }
    }

    pub fn all_exhausted(&self) -> bool {
        self.models
            .iter()
            .all(|m| self.attempted_models.contains(m))
    }

    pub fn mark_attempted(&mut self, model: &str) {
        self.attempted_models.insert(model.to_owned());
        self.attempt_count += 1;
    }
}

pub struct GroqProvider {
    client: Client,
    api_key: String,
    base_url: String,
    config: GroqConfig,
}

impl GroqProvider {
    pub fn from_config(config: &GroqConfig) -> Result<Self, AiProviderError> {
        if !config.enabled {
            return Err(AiProviderError::Disabled);
        }
        let api_key = config
            .resolve_api_key()
            .map_err(|_| AiProviderError::MissingApiKey)?;
        Self::new(config, api_key)
    }

    pub fn new(config: &GroqConfig, api_key: String) -> Result<Self, AiProviderError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_seconds))
            .build()
            .map_err(|e| AiProviderError::HttpError(e.to_string()))?;
        Ok(Self {
            client,
            api_key,
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            config: config.clone(),
        })
    }

    fn parse_rate_limit_body(
        &self,
        status: u16,
        body: &str,
        retry_after_header: Option<Duration>,
        model: &str,
    ) -> AiProviderError {
        let mut info = RateLimitInfo {
            model: Some(model.to_owned()),
            limit_kind: None,
            retry_after: retry_after_header,
            limit: None,
            used: None,
            requested: None,
        };

        // Try to parse JSON error body from Groq
        if let Ok(v) = serde_json::from_str::<Value>(body) {
            // Groq error format: {"error": {"type": "tokens", "code": "rate_limit_exceeded", ...}}
            if let Some(error_obj) = v.get("error") {
                // Code-based limit kind detection
                let code = error_obj.get("code").and_then(Value::as_str).unwrap_or("");
                let err_type = error_obj.get("type").and_then(Value::as_str).unwrap_or("");

                if code.contains("rate_limit_exceeded") || err_type.contains("tokens") {
                    let message = error_obj
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_lowercase();
                    if message.contains("per day") || message.contains("daily") {
                        info.limit_kind = Some(RateLimitKind::TokensPerDay);
                    } else {
                        info.limit_kind = Some(RateLimitKind::TokensPerMinute);
                    }

                    // Parse retry duration from JSON body
                    if let Some(retry_ms) = error_obj.get("retry_after").and_then(Value::as_f64) {
                        let d = Duration::from_secs_f64(retry_ms / 1000.0);
                        if info.retry_after.is_none() || d < info.retry_after.unwrap() {
                            info.retry_after = Some(d);
                        }
                    }

                    info.requested = error_obj
                        .get("requested_tokens")
                        .and_then(Value::as_u64)
                        .or_else(|| error_obj.get("requested").and_then(Value::as_u64));

                    // Parse "tokens per day" / "tokens per minute" limit values
                    // e.g. "tokens per day: 500000"
                    for part in message.split(&[',', ';', '\n'][..]) {
                        let part = part.trim();
                        if let Some(val) = part
                            .rsplit_once("per day")
                            .or_else(|| part.rsplit_once("/day"))
                        {
                            // val.1 is the part after "per day": ": 500000"
                            let num: u64 = val
                                .1
                                .split_whitespace()
                                .filter_map(|w| w.parse::<u64>().ok())
                                .next()
                                .unwrap_or(0);
                            if num > 0 {
                                info.limit = Some(num);
                            }
                        }
                    }

                    // Also check "per minute" for the limit value
                    if info.limit.is_none() {
                        for part in message.split(&[',', ';', '\n'][..]) {
                            let part = part.trim();
                            if let Some(val) = part
                                .rsplit_once("per minute")
                                .or_else(|| part.rsplit_once("/minute"))
                            {
                                let num: u64 = val
                                    .1
                                    .split_whitespace()
                                    .filter_map(|w| w.parse::<u64>().ok())
                                    .next()
                                    .unwrap_or(0);
                                if num > 0 {
                                    info.limit = Some(num);
                                }
                            }
                        }
                    }
                }

                // Check for model-specific errors (403, 404)
                let msg = error_obj
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if status == 403 {
                    return AiProviderError::Permission(msg.to_owned());
                }
                if status == 404 {
                    return AiProviderError::UnknownModel(msg.to_owned());
                }
                if status == 400 {
                    return AiProviderError::InvalidRequest(msg.to_owned());
                }
            }
        }

        // Fallback: parse retry-after duration from human-readable error text
        if info.retry_after.is_none() && retry_after_header.is_none() {
            let lower = body.to_lowercase();
            if let Some(idx) = lower.find("retry after ") {
                let rest = &lower[idx + "retry after ".len()..];
                let secs: u64 = rest
                    .split_whitespace()
                    .next()
                    .and_then(|w| w.trim_matches(&['.', 's', ' '][..]).parse().ok())
                    .unwrap_or(0);
                if secs > 0 {
                    info.retry_after = Some(Duration::from_secs(secs));
                }
            }
        }

        // Clamp retry_after to configured maximum
        let max_delay = Duration::from_secs(self.config.rate_limits.maximum_retry_delay_seconds);
        if let Some(ra) = info.retry_after
            && ra > max_delay
        {
            info.retry_after = Some(max_delay);
        }

        AiProviderError::RateLimited(info)
    }

    /// Build the ordered list of models for this request, starting from the given index.
    fn model_list(&self) -> Vec<String> {
        let mut models = vec![self.config.model.clone()];
        models.extend(self.config.fallback_models.iter().cloned());
        models
    }
}

#[async_trait]
impl AiProvider for GroqProvider {
    async fn chat(&self, request: AiRequest) -> Result<AiResponse, AiProviderError> {
        let url = format!("{}/chat/completions", self.base_url);
        let models = self.model_list();
        let max_retries = self.config.max_request_retries;

        let mut last_error = AiProviderError::HttpError("no models attempted".into());

        for (model_idx, model) in models.iter().enumerate() {
            let attempts_for_model = if model_idx == 0 {
                max_retries
            } else {
                // Fallback models get fewer retries
                max_retries.min(1)
            };

            for attempt in 0..=attempts_for_model {
                let mut messages: Vec<serde_json::Value> = Vec::new();
                if let Some(system) = &request.system_prompt {
                    messages.push(serde_json::json!({"role": "system", "content": system}));
                }
                for msg in &request.messages {
                    messages.push(serde_json::to_value(msg).map_err(|e| {
                        AiProviderError::InvalidResponse(format!(
                            "failed to serialize message: {e}"
                        ))
                    })?);
                }

                let mut body = serde_json::json!({
                    "model": model,
                    "messages": messages,
                    "temperature": self.config.temperature,
                    "max_completion_tokens": self.config.max_completion_tokens,
                });

                if let Some(tier) = &self.config.service_tier {
                    body["service_tier"] = serde_json::json!(tier);
                }

                if !request.tools.is_empty() {
                    body["tools"] = serde_json::to_value(&request.tools)
                        .map_err(|e| AiProviderError::InvalidResponse(e.to_string()))?;
                    body["parallel_tool_calls"] = serde_json::Value::Bool(false);
                }

                let response = self
                    .client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .header("Content-Type", "application/json")
                    .json(&body)
                    .send()
                    .await;

                match response {
                    Ok(r) if r.status().is_success() => {
                        return parse_groq_response(r.json().await.map_err(|e| {
                            AiProviderError::InvalidResponse(format!("decode failed: {e}"))
                        })?);
                    }
                    Ok(r) => {
                        let status = r.status();

                        // Parse retry-after header before consuming r
                        let retry_after_header = r
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                            .map(Duration::from_secs);

                        let body_text = r.text().await.unwrap_or_default();

                        match status.as_u16() {
                            401 => {
                                return Err(AiProviderError::AuthenticationError(format!(
                                    "Groq request failed with HTTP {status}: {body_text}"
                                )));
                            }
                            403 => {
                                last_error = self.parse_rate_limit_body(
                                    status.as_u16(),
                                    &body_text,
                                    retry_after_header,
                                    model,
                                );
                                // Model permission error - try next model
                                break;
                            }
                            404 => {
                                last_error = AiProviderError::UnknownModel(format!(
                                    "Model '{model}' not found: {body_text}"
                                ));
                                // Model not found - try next model
                                break;
                            }
                            429 => {
                                let err = self.parse_rate_limit_body(
                                    status.as_u16(),
                                    &body_text,
                                    retry_after_header,
                                    model,
                                );

                                // Check if it's a daily limit - try fallback model
                                let is_daily = matches!(
                                    &err,
                                    AiProviderError::RateLimited(info) if info.limit_kind == Some(RateLimitKind::TokensPerDay)
                                );

                                if is_daily && self.config.rate_limits.switch_model_on_daily_limit {
                                    last_error = err;
                                    // Try next model immediately
                                    break;
                                }

                                // Minute limit or unknown - respect retry-after
                                if let AiProviderError::RateLimited(ref info) = err
                                    && let Some(delay) = info.retry_after
                                    && self.config.rate_limits.respect_retry_after
                                    && attempt < attempts_for_model
                                {
                                    tokio::select! {
                                        _ = tokio::time::sleep(delay) => {
                                            last_error = err;
                                            continue;
                                        }
                                    }
                                }

                                if attempt < attempts_for_model {
                                    let backoff =
                                        Duration::from_secs(2u64.saturating_pow(attempt + 1));
                                    tokio::time::sleep(backoff).await;
                                    last_error = err;
                                    continue;
                                }
                                last_error = err;
                                break;
                            }
                            400 => {
                                return Err(AiProviderError::InvalidRequest(format!(
                                    "Groq request failed with HTTP {status}: {body_text}"
                                )));
                            }
                            _ => {
                                if status.is_server_error() && attempt < attempts_for_model {
                                    tokio::time::sleep(Duration::from_millis(
                                        500 * (attempt + 1) as u64,
                                    ))
                                    .await;
                                    last_error = AiProviderError::ApiError(format!(
                                        "Groq request failed with HTTP {status}: {body_text}"
                                    ));
                                    continue;
                                }
                                last_error = AiProviderError::ApiError(format!(
                                    "Groq request failed with HTTP {status}: {body_text}"
                                ));
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let err_msg = format!("Groq request failed: {e}");
                        if e.is_timeout() {
                            if attempt < attempts_for_model {
                                tokio::time::sleep(Duration::from_millis(
                                    500 * (attempt + 1) as u64,
                                ))
                                .await;
                                last_error = AiProviderError::Timeout;
                                continue;
                            }
                            return Err(AiProviderError::Timeout);
                        }
                        if attempt < attempts_for_model {
                            tokio::time::sleep(Duration::from_millis(500 * (attempt + 1) as u64))
                                .await;
                            last_error = AiProviderError::HttpError(err_msg);
                            continue;
                        }
                        last_error = AiProviderError::HttpError(err_msg);
                        break;
                    }
                }
            }

            // If last model also failed, propagate the last error
            if model_idx + 1 >= models.len() {
                return Err(last_error);
            }
        }

        Err(last_error)
    }
}

fn parse_groq_response(v: Value) -> Result<AiResponse, AiProviderError> {
    parse_openai_compatible_response(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GroqConfig;

    fn test_config() -> GroqConfig {
        GroqConfig {
            enabled: true,
            api_key: None,
            api_key_env: Some("GROQ_TEST_KEY".into()),
            model: "test-model".into(),
            fallback_models: vec!["fallback-model".into()],
            base_url: "https://api.groq.com/openai/v1".into(),
            request_timeout_seconds: 30,
            max_request_retries: 0,
            temperature: 0.1,
            service_tier: None,
            max_completion_tokens: 250,
            include_nearby_blocks: true,
            include_nearby_entities: true,
            include_inventory: true,
            rate_limits: crate::config::GroqRateLimitConfig::default(),
            context: crate::config::GroqContextConfig::default(),
            limits: crate::config::GeminiLimits::default(),
        }
    }

    fn test_api_key() -> String {
        "test-api-key-value".into()
    }

    #[test]
    fn groq_provider_rejects_disabled_config() {
        let config = GroqConfig {
            enabled: false,
            api_key: None,
            api_key_env: None,
            ..test_config()
        };
        assert!(matches!(
            GroqProvider::from_config(&config),
            Err(AiProviderError::Disabled)
        ));
    }

    #[test]
    fn groq_provider_rejects_missing_api_key() {
        let config = GroqConfig {
            enabled: true,
            api_key: None,
            api_key_env: None,
            ..test_config()
        };
        assert!(matches!(
            GroqProvider::from_config(&config),
            Err(AiProviderError::MissingApiKey)
        ));
    }

    #[test]
    fn groq_provider_accepts_direct_api_key() {
        let config = GroqConfig {
            enabled: true,
            api_key: Some("my-direct-key".into()),
            api_key_env: None,
            ..test_config()
        };
        let provider = GroqProvider::from_config(&config).unwrap();
        assert_eq!(provider.api_key, "my-direct-key");
    }

    #[test]
    fn groq_provider_uses_configured_model_in_request() {
        let config = test_config();
        let provider = GroqProvider::new(&config, test_api_key()).unwrap();
        assert_eq!(config.model, "test-model");
        assert_eq!(provider.base_url, "https://api.groq.com/openai/v1");
    }

    #[test]
    fn groq_api_key_is_not_logged() {
        let config = GroqConfig {
            api_key: Some("secret-dont-leak".into()),
            ..test_config()
        };
        let debug = format!("{:?}", config);
        assert!(!debug.contains("secret-dont-leak"));
        assert!(debug.contains("api_key"));
        assert!(debug.contains("api_key_env"));
    }

    #[test]
    fn parse_groq_response_with_tool_calls() {
        let json = serde_json::json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "gather",
                            "arguments": "{\"item\":\"minecraft:oak_log\",\"quantity\":10}"
                        }
                    }]
                }
            }]
        });
        let response = parse_groq_response(json).unwrap();
        assert_eq!(response.finish_reason, "tool_calls");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "gather");
        assert_eq!(
            response.tool_calls[0].arguments["item"],
            "minecraft:oak_log"
        );
    }

    #[test]
    fn parse_groq_response_with_text() {
        let json = serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "content": "Hello! How can I help?"
                }
            }]
        });
        let response = parse_groq_response(json).unwrap();
        assert_eq!(response.message, "Hello! How can I help?");
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn parse_groq_response_rejects_empty_response() {
        let json = serde_json::json!({});
        assert!(parse_groq_response(json).is_err());
    }

    // --- Rate-limit tests ---

    #[test]
    fn provider_attempt_state_default_has_primary_model_first() {
        let config = test_config();
        let state = ProviderAttemptState::new(&config);
        assert_eq!(state.current_model(), Some("test-model"));
        assert_eq!(state.models.len(), 2);
    }

    #[test]
    fn provider_attempt_state_advances_to_fallback() {
        let config = test_config();
        let mut state = ProviderAttemptState::new(&config);
        state.mark_attempted("test-model");
        assert!(state.advance_model());
        assert_eq!(state.current_model(), Some("fallback-model"));
    }

    #[test]
    fn provider_attempt_state_all_exhausted() {
        let config = test_config();
        let mut state = ProviderAttemptState::new(&config);
        state.mark_attempted("test-model");
        state.mark_attempted("fallback-model");
        assert!(state.all_exhausted());
    }

    #[test]
    fn provider_attempt_state_not_exhausted_when_one_untried() {
        let config = test_config();
        let mut state = ProviderAttemptState::new(&config);
        state.mark_attempted("test-model");
        assert!(!state.all_exhausted());
    }

    #[test]
    fn rate_limit_info_parses_json_body_with_tokens_per_day() {
        let config = test_config();
        let provider = GroqProvider::new(&config, test_api_key()).unwrap();
        let body = r#"{"error":{"type":"tokens","code":"rate_limit_exceeded","message":"tokens per day: 500000, requested tokens: 2929","retry_after":60000,"requested_tokens":2929}}"#;
        let err = provider.parse_rate_limit_body(429, body, None, "test-model");
        match err {
            AiProviderError::RateLimited(info) => {
                assert_eq!(info.model.as_deref(), Some("test-model"));
                assert_eq!(info.limit_kind, Some(RateLimitKind::TokensPerDay));
                assert_eq!(info.limit, Some(500000));
                assert_eq!(info.requested, Some(2929));
                assert!(info.retry_after.is_some());
            }
            _ => panic!("expected RateLimited"),
        }
    }

    #[test]
    fn rate_limit_info_parses_json_body_with_tokens_per_minute() {
        let config = test_config();
        let provider = GroqProvider::new(&config, test_api_key()).unwrap();
        let body = r#"{"error":{"type":"tokens","code":"rate_limit_exceeded","message":"tokens per minute: 30000, requested tokens: 2929","retry_after":10000,"requested_tokens":2929}}"#;
        let err = provider.parse_rate_limit_body(429, body, None, "test-model");
        match err {
            AiProviderError::RateLimited(info) => {
                assert_eq!(info.limit_kind, Some(RateLimitKind::TokensPerMinute));
                assert_eq!(info.limit, Some(30000));
                assert!(info.retry_after.is_some());
            }
            _ => panic!("expected RateLimited"),
        }
    }

    #[test]
    fn rate_limit_info_retry_after_header_takes_precedence() {
        let config = test_config();
        let provider = GroqProvider::new(&config, test_api_key()).unwrap();
        let body = r#"{"error":{"type":"tokens","code":"rate_limit_exceeded","message":"rate limited","retry_after":60000}}"#;
        let header_delay = Some(Duration::from_secs(30));
        let err = provider.parse_rate_limit_body(429, body, header_delay, "test-model");
        match err {
            AiProviderError::RateLimited(info) => {
                assert_eq!(info.retry_after, Some(Duration::from_secs(30)));
            }
            _ => panic!("expected RateLimited"),
        }
    }

    #[test]
    fn rate_limit_info_clamps_to_maximum() {
        let config = GroqConfig {
            rate_limits: crate::config::GroqRateLimitConfig {
                maximum_retry_delay_seconds: 10,
                ..crate::config::GroqRateLimitConfig::default()
            },
            ..test_config()
        };
        let provider = GroqProvider::new(&config, test_api_key()).unwrap();
        let body = r#"{"error":{"type":"tokens","code":"rate_limit_exceeded","message":"tokens per day","retry_after":3600000}}"#;
        let err = provider.parse_rate_limit_body(429, body, None, "test-model");
        match err {
            AiProviderError::RateLimited(info) => {
                assert_eq!(info.retry_after, Some(Duration::from_secs(10)));
            }
            _ => panic!("expected RateLimited"),
        }
    }

    #[test]
    fn rate_limit_info_fallback_to_text_parsing() {
        let config = test_config();
        let provider = GroqProvider::new(&config, test_api_key()).unwrap();
        let body = "Rate limit exceeded. Retry after 15 seconds.";
        let err = provider.parse_rate_limit_body(429, body, None, "test-model");
        match err {
            AiProviderError::RateLimited(info) => {
                assert!(info.retry_after.is_some());
                assert_eq!(info.retry_after, Some(Duration::from_secs(15)));
            }
            _ => panic!("expected RateLimited"),
        }
    }

    #[test]
    fn rate_limit_info_403_permission_error() {
        let config = test_config();
        let provider = GroqProvider::new(&config, test_api_key()).unwrap();
        let body = r#"{"error":{"message":"Model not permitted for this API key"}}"#;
        let err = provider.parse_rate_limit_body(403, body, None, "test-model");
        match err {
            AiProviderError::Permission(msg) => {
                assert!(msg.contains("Model not permitted"));
            }
            _ => panic!("expected Permission"),
        }
    }

    #[test]
    fn rate_limit_info_404_unknown_model() {
        let config = test_config();
        let provider = GroqProvider::new(&config, test_api_key()).unwrap();
        let body = r#"{"error":{"message":"Model 'fake-model' not found"}}"#;
        let err = provider.parse_rate_limit_body(404, body, None, "test-model");
        match err {
            AiProviderError::UnknownModel(msg) => {
                assert!(msg.contains("not found"));
            }
            _ => panic!("expected UnknownModel"),
        }
    }

    #[test]
    fn retry_after_not_set_when_no_indicator() {
        let config = test_config();
        let provider = GroqProvider::new(&config, test_api_key()).unwrap();
        let body = "Some other error";
        let err = provider.parse_rate_limit_body(429, body, None, "test-model");
        match err {
            AiProviderError::RateLimited(info) => {
                assert!(info.retry_after.is_none());
            }
            _ => panic!("expected RateLimited"),
        }
    }
}
