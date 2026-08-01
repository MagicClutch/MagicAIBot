use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct AiRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDefinition>,
    pub system_prompt: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ChatMessage {
    pub role: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", rename = "tool_calls")]
    pub tool_calls: Option<Vec<ToolCall>>,

    #[serde(skip_serializing_if = "Option::is_none", rename = "tool_call_id")]
    pub tool_call_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_text(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_tool_calls(tool_calls: Vec<ToolCallInfo>) -> Self {
        let payload: Vec<ToolCall> = tool_calls.into_iter().map(ToolCall::from_info).collect();
        Self {
            role: "assistant".into(),
            content: None,
            tool_calls: Some(payload),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ToolFunctionDefinition,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ToolFunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolCallFunction,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

impl ToolCall {
    pub fn from_info(info: ToolCallInfo) -> Self {
        let arguments_str = serde_json::to_string(&info.arguments).unwrap_or_else(|_| "{}".into());
        Self {
            id: info.id,
            call_type: "function".into(),
            function: ToolCallFunction {
                name: info.name,
                arguments: arguments_str,
            },
        }
    }
}

/// Internal representation of a tool call before it's serialized for the API.
/// This is used to pass tool call data around inside the application.
#[derive(Clone, Debug)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug)]
pub struct AiResponse {
    pub message: String,
    pub tool_calls: Vec<ToolCallInfo>,
    pub finish_reason: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RateLimitKind {
    RequestsPerMinute,
    RequestsPerDay,
    TokensPerMinute,
    TokensPerDay,
    InputTokensPerMinute,
    OutputTokensPerMinute,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct RateLimitInfo {
    pub model: Option<String>,
    pub limit_kind: Option<RateLimitKind>,
    pub retry_after: Option<Duration>,
    pub limit: Option<u64>,
    pub used: Option<u64>,
    pub requested: Option<u64>,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum AiProviderError {
    #[error("API key is missing or unavailable")]
    MissingApiKey,
    #[error("HTTP request failed: {0}")]
    HttpError(String),
    #[error("API returned error: {0}")]
    ApiError(String),
    #[error("Rate limited")]
    RateLimited(RateLimitInfo),
    #[error("Authentication failed: {0}")]
    AuthenticationError(String),
    #[error("Model not found or unavailable: {0}")]
    UnknownModel(String),
    #[error("Permission denied: {0}")]
    Permission(String),
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Request timed out")]
    Timeout,
    #[error("Invalid response format: {0}")]
    InvalidResponse(String),
    #[error("Provider is disabled")]
    Disabled,
    #[error("Empty response from provider")]
    EmptyResponse,
    #[error("Unknown tool call: {0}")]
    UnknownTool(String),
    #[error("Invalid tool message sequence at index {index}: {reason}")]
    InvalidToolMessageSequence { index: usize, reason: String },
}

#[async_trait]
pub trait AiProvider: Send + Sync {
    async fn chat(&self, request: AiRequest) -> Result<AiResponse, AiProviderError>;
}

/// Parses a `/chat/completions` response body shared by every
/// OpenAI-compatible backend (Groq, Ollama, ...): `choices[0]` holds
/// `finish_reason`, `message.content`, and an optional `message.tool_calls`
/// array where each entry's `function.arguments` is a JSON-encoded string.
pub(crate) fn parse_openai_compatible_response(v: Value) -> Result<AiResponse, AiProviderError> {
    let choice = v
        .pointer("/choices/0")
        .ok_or_else(|| AiProviderError::InvalidResponse("no choices in response".into()))?;

    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("stop")
        .to_owned();

    let message = choice
        .pointer("/message/content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();

    let tool_calls = if let Some(tcs) = choice
        .pointer("/message/tool_calls")
        .and_then(Value::as_array)
    {
        tcs.iter()
            .filter_map(|tc| {
                let id = tc.get("id")?.as_str()?.to_owned();
                let name = tc.pointer("/function/name")?.as_str()?.to_owned();
                let args_str = tc.pointer("/function/arguments")?.as_str()?;
                let arguments: Value = serde_json::from_str(args_str).ok()?;
                Some(ToolCallInfo {
                    id,
                    name,
                    arguments,
                })
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(AiResponse {
        message,
        tool_calls,
        finish_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_message_contains_tool_call_id() {
        let msg = ChatMessage::tool_result("call_123", r#"{"success":true}"#);
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "tool");
        assert_eq!(json["tool_call_id"], "call_123");
        assert_eq!(json["content"], r#"{"success":true}"#);
        assert!(json.get("tool_calls").is_none());
    }

    #[test]
    fn tool_call_id_matches_assistant_call() {
        let tool_call_id = "call_inventory_42";
        let info = ToolCallInfo {
            id: tool_call_id.into(),
            name: "get_inventory".into(),
            arguments: serde_json::json!({}),
        };
        let assistant_msg = ChatMessage::assistant_tool_calls(vec![info]);

        let assistant_json = serde_json::to_value(&assistant_msg).unwrap();
        assert_eq!(assistant_json["role"], "assistant");
        assert!(assistant_json.get("content").is_none() || assistant_json["content"].is_null());

        let call_id = assistant_json["tool_calls"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let result_msg = ChatMessage::tool_result(call_id, r#"{"items":[]}"#);
        let result_json = serde_json::to_value(&result_msg).unwrap();
        assert_eq!(result_json["tool_call_id"], "call_inventory_42");
    }

    #[test]
    fn assistant_tool_call_is_saved_before_tool_result() {
        let info = ToolCallInfo {
            id: "call_gather_1".into(),
            name: "gather".into(),
            arguments: serde_json::json!({"item": "minecraft:oak_log", "quantity": 10}),
        };
        let assistant = ChatMessage::assistant_tool_calls(vec![info]);

        let json = serde_json::to_value(&assistant).unwrap();
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["tool_calls"][0]["id"], "call_gather_1");
        assert_eq!(json["tool_calls"][0]["type"], "function");
        assert_eq!(json["tool_calls"][0]["function"]["name"], "gather");
        assert!(json["tool_calls"][0]["function"]["arguments"].is_string());
    }

    #[test]
    fn single_tool_call_roundtrip_is_valid() {
        let info = ToolCallInfo {
            id: "call_status_99".into(),
            name: "get_status".into(),
            arguments: serde_json::json!({}),
        };
        let assistant = ChatMessage::assistant_tool_calls(vec![info]);
        let result = ChatMessage::tool_result("call_status_99", r#"{"health":20}"#);

        let assistant_json = serde_json::to_value(&assistant).unwrap();
        let result_json = serde_json::to_value(&result).unwrap();

        assert_eq!(assistant_json["role"], "assistant");
        assert_eq!(assistant_json["tool_calls"][0]["id"], "call_status_99");
        assert_eq!(result_json["role"], "tool");
        assert_eq!(result_json["tool_call_id"], "call_status_99");
        assert_eq!(result_json["content"], r#"{"health":20}"#);
    }

    #[test]
    fn multiple_tool_calls_receive_separate_results() {
        let calls = vec![
            ToolCallInfo {
                id: "call_inv_1".into(),
                name: "get_inventory".into(),
                arguments: serde_json::json!({}),
            },
            ToolCallInfo {
                id: "call_pos_2".into(),
                name: "get_bot_position".into(),
                arguments: serde_json::json!({}),
            },
        ];

        let assistant = ChatMessage::assistant_tool_calls(calls);
        let assistant_json = serde_json::to_value(&assistant).unwrap();
        assert_eq!(assistant_json["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(assistant_json["tool_calls"][0]["id"], "call_inv_1");
        assert_eq!(assistant_json["tool_calls"][1]["id"], "call_pos_2");

        let result1 = ChatMessage::tool_result("call_inv_1", r#"{"items":[]}"#);
        let result2 = ChatMessage::tool_result("call_pos_2", r#"{"x":0.0}"#);

        let r1_json = serde_json::to_value(&result1).unwrap();
        let r2_json = serde_json::to_value(&result2).unwrap();

        assert_eq!(r1_json["tool_call_id"], "call_inv_1");
        assert_eq!(r2_json["tool_call_id"], "call_pos_2");
    }

    #[test]
    fn multiple_tool_results_preserve_order() {
        let calls = vec![
            ToolCallInfo {
                id: "call_a".into(),
                name: "query_inventory".into(),
                arguments: serde_json::json!({}),
            },
            ToolCallInfo {
                id: "call_b".into(),
                name: "get_recipe".into(),
                arguments: serde_json::json!({"item": "minecraft:stone"}),
            },
        ];

        let assistant = ChatMessage::assistant_tool_calls(calls);
        let json = serde_json::to_value(&assistant).unwrap();
        let tool_calls = json["tool_calls"].as_array().unwrap();
        assert_eq!(tool_calls[0]["function"]["name"], "query_inventory");
        assert_eq!(tool_calls[1]["function"]["name"], "get_recipe");
    }

    #[test]
    fn tool_failure_still_returns_tool_message() {
        let result = ChatMessage::tool_result(
            "call_fail_1",
            r#"{"success":false,"error":"Inventory unavailable"}"#,
        );
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["role"], "tool");
        assert_eq!(json["tool_call_id"], "call_fail_1");
        assert!(json["content"].as_str().unwrap().contains("success"));
    }

    #[test]
    fn tool_message_serializes_snake_case_id() {
        let result = ChatMessage::tool_result("call_xyz", "done");
        let json = serde_json::to_value(&result).unwrap();
        assert!(json.get("tool_call_id").is_some(), "must have tool_call_id");
        assert!(json.get("toolCallId").is_none(), "must not use camelCase");
    }

    #[test]
    fn ordinary_chat_message_has_no_tool_fields() {
        let user_msg = ChatMessage::user("hello");
        let json = serde_json::to_value(&user_msg).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "hello");
        assert!(json.get("tool_calls").is_none());
        assert!(json.get("tool_call_id").is_none());

        let assistant_msg = ChatMessage::assistant_text("hi there");
        let json = serde_json::to_value(&assistant_msg).unwrap();
        assert_eq!(json["role"], "assistant");
        assert!(json.get("tool_calls").is_none());
        assert!(json.get("tool_call_id").is_none());
    }

    #[test]
    fn tool_call_serializes_with_correct_fields() {
        let info = ToolCallInfo {
            id: "call_test".into(),
            name: "my_tool".into(),
            arguments: serde_json::json!({"key": "value"}),
        };
        let tc = ToolCall::from_info(info);

        let json = serde_json::to_value(&tc).unwrap();
        assert_eq!(json["id"], "call_test");
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"]["name"], "my_tool");
        assert_eq!(json["function"]["arguments"], r#"{"key":"value"}"#);
    }

    #[test]
    fn assistant_tool_calls_has_null_content() {
        let info = ToolCallInfo {
            id: "call_1".into(),
            name: "get_status".into(),
            arguments: serde_json::json!({}),
        };
        let msg = ChatMessage::assistant_tool_calls(vec![info]);
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "assistant");
        // content should be omitted (None) when tool_calls are present
        assert!(json.get("content").is_none() || json["content"].is_null());
        assert!(json.get("tool_calls").is_some());
    }

    #[test]
    fn full_tool_sequence_matches_groq_api_format() {
        // Simulate a full conversation roundtrip
        let user = ChatMessage::user("What do you have?");
        let assistant = ChatMessage::assistant_tool_calls(vec![ToolCallInfo {
            id: "call_001".into(),
            name: "query_inventory".into(),
            arguments: serde_json::json!({}),
        }]);
        let tool_result = ChatMessage::tool_result("call_001", r#"{"items":[]}"#);

        let messages = vec![user, assistant, tool_result];
        let all_json: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::to_value(m).unwrap())
            .collect();

        assert_eq!(all_json.len(), 3);
        assert_eq!(all_json[0]["role"], "user");
        assert_eq!(all_json[1]["role"], "assistant");
        assert_eq!(all_json[1]["tool_calls"][0]["id"], "call_001");
        assert_eq!(all_json[2]["role"], "tool");
        assert_eq!(all_json[2]["tool_call_id"], "call_001");

        // Verify the sequence is valid per Groq API requirements
        let tool_ids: Vec<&str> = all_json[1]["tool_calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tc| tc["id"].as_str().unwrap())
            .collect();
        assert_eq!(tool_ids, vec!["call_001"]);
        assert_eq!(all_json[2]["tool_call_id"], "call_001");
    }

    #[test]
    fn tool_sequence_with_multiple_assistant_calls() {
        // Groq returned two tool calls in one assistant message
        let assistant = ChatMessage::assistant_tool_calls(vec![
            ToolCallInfo {
                id: "call_inv".into(),
                name: "query_inventory".into(),
                arguments: serde_json::json!({}),
            },
            ToolCallInfo {
                id: "call_pos".into(),
                name: "get_bot_position".into(),
                arguments: serde_json::json!({}),
            },
        ]);

        let r1 = ChatMessage::tool_result("call_inv", r#"{"items":[]}"#);
        let r2 = ChatMessage::tool_result("call_pos", r#"{"x":1.0}"#);

        let seq = vec![
            serde_json::to_value(&assistant).unwrap(),
            serde_json::to_value(&r1).unwrap(),
            serde_json::to_value(&r2).unwrap(),
        ];

        assert_eq!(seq[0]["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(seq[1]["tool_call_id"], "call_inv");
        assert_eq!(seq[2]["tool_call_id"], "call_pos");
    }

    #[test]
    fn tool_call_info_has_expected_fields() {
        let info = ToolCallInfo {
            id: "call_xyz".into(),
            name: "gather".into(),
            arguments: serde_json::json!({"item": "minecraft:stone", "quantity": 5}),
        };
        assert_eq!(info.id, "call_xyz");
        assert_eq!(info.name, "gather");
        assert_eq!(info.arguments["item"], "minecraft:stone");
    }
}
