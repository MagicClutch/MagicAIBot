use std::{fs, path::Path};

use serde::Deserialize;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
    Layer,
    filter::{LevelFilter as TargetLevel, Targets},
    fmt,
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

use crate::error::AppError;
use crate::minecraft::world_state::{
    DEFAULT_ENTITY_RADIUS, DEFAULT_MAXIMUM_ENTITIES, DEFAULT_STALE_SECONDS, validate_limits,
};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub minecraft: MinecraftConfig,
    pub reconnect: ReconnectConfig,
    #[serde(default)]
    pub console: ConsoleConfig,
    pub logging: LoggingConfig,
    #[serde(default)]
    pub world_state: WorldStateConfig,
    #[serde(default)]
    pub movement: MovementConfig,
    #[serde(default)]
    pub vertical_navigation: VerticalNavigationConfig,
    #[serde(default)]
    pub block_search: BlockSearchConfig,
    #[serde(default)]
    pub block_navigation: BlockNavigationConfig,
    #[serde(default)]
    pub look: LookConfig,
    #[serde(default)]
    pub interaction: InteractionConfig,
    #[serde(default)]
    pub smelting: crate::smelting::SmeltingConfig,
    #[serde(default)]
    pub multitasking: MultitaskingConfig,
    #[serde(default)]
    pub inventory_cleanup: crate::inventory_cleanup::CleanupPolicy,
    #[serde(default)]
    pub tree_chopping: TreeChoppingConfig,
    #[serde(default)]
    pub ensure_tool: EnsureToolConfig,
    #[serde(default)]
    pub gemini: GeminiConfig,
    #[serde(default)]
    pub groq: GroqConfig,
    #[serde(default)]
    pub ollama: OllamaConfig,
    #[serde(default)]
    pub ai: AiConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AiConfig {
    /// Which backend answers AI requests. `[groq]`/`[ollama]` below are
    /// only consulted for whichever one is selected here.
    #[serde(default)]
    pub provider: AiProviderKind,
    #[serde(default)]
    pub chat: AiChatConfig,
    #[serde(default)]
    pub busy_behavior: AiBusyBehavior,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AiProviderKind {
    #[default]
    Groq,
    Ollama,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AiBusyBehavior {
    #[default]
    Reject,
}
#[derive(Clone, Debug, Deserialize)]
pub struct AiChatConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_ai_prefix")]
    pub prefix: String,
    /// When false, every player chat message is treated as an AI request
    /// instead of only ones starting with `prefix`. `access`/`rate_limit`
    /// still apply, but this makes ambient chat between other players
    /// trigger the bot too -- there is no way to tell "talking to the bot"
    /// from "talking near the bot" without a prefix.
    #[serde(default = "default_true")]
    pub require_prefix: bool,
    #[serde(default = "default_true")]
    pub respond_in_chat: bool,
    #[serde(default = "default_true")]
    pub acknowledge_requests: bool,
    #[serde(default = "default_true")]
    pub accept_console_ai_command: bool,
    #[serde(default = "default_true")]
    pub strip_prefix_whitespace: bool,
    #[serde(default = "default_ai_request_length")]
    pub max_request_length: usize,
    #[serde(default = "default_incoming_queue_capacity")]
    pub incoming_queue_capacity: usize,
    #[serde(default)]
    pub access: AiChatAccessConfig,
    #[serde(default)]
    pub rate_limit: AiChatRateLimitConfig,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AiChatAccessConfig {
    #[serde(default)]
    pub operators_only: bool,
    #[serde(default)]
    pub allowed_players: Vec<String>,
    #[serde(default)]
    pub blocked_players: Vec<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct AiChatRateLimitConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_rate_limit_requests")]
    pub requests: usize,
    #[serde(default = "default_rate_limit_window")]
    pub window_seconds: u64,
}
impl Default for AiChatRateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests: 3,
            window_seconds: 30,
        }
    }
}
fn default_ai_prefix() -> String {
    "!".into()
}
fn default_ai_request_length() -> usize {
    500
}
fn default_incoming_queue_capacity() -> usize {
    64
}
fn default_rate_limit_requests() -> usize {
    3
}
fn default_rate_limit_window() -> u64 {
    30
}
impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: AiProviderKind::default(),
            chat: AiChatConfig::default(),
            busy_behavior: AiBusyBehavior::Reject,
        }
    }
}
impl Default for AiChatConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            prefix: default_ai_prefix(),
            require_prefix: true,
            respond_in_chat: true,
            acknowledge_requests: true,
            accept_console_ai_command: true,
            strip_prefix_whitespace: true,
            max_request_length: default_ai_request_length(),
            incoming_queue_capacity: default_incoming_queue_capacity(),
            access: AiChatAccessConfig::default(),
            rate_limit: AiChatRateLimitConfig::default(),
        }
    }
}

/// Configuration for the remote planner. Secrets are never included in logs
/// or errors; a local configuration key takes priority over an environment key.
#[derive(Clone, Deserialize)]
pub struct GeminiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_gemini_model")]
    pub model: String,
    #[serde(default = "default_gemini_timeout")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_gemini_retries")]
    pub max_request_retries: u32,
    #[serde(default = "default_gemini_steps")]
    pub max_steps_per_session: usize,
    #[serde(default = "default_gemini_session_seconds")]
    pub max_session_seconds: u64,
    #[serde(default = "default_gemini_temperature")]
    pub temperature: f32,
    #[serde(default = "default_true")]
    pub include_nearby_blocks: bool,
    #[serde(default = "default_true")]
    pub include_nearby_entities: bool,
    #[serde(default = "default_true")]
    pub include_inventory: bool,
    #[serde(default)]
    pub limits: GeminiLimits,
}

impl std::fmt::Debug for GeminiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiConfig")
            .field("enabled", &self.enabled)
            .field("api_key", &self.api_key.as_ref().map(|_| "********"))
            .field("api_key_env", &self.api_key_env)
            .field("model", &self.model)
            .field("request_timeout_seconds", &self.request_timeout_seconds)
            .field("max_request_retries", &self.max_request_retries)
            .field("max_steps_per_session", &self.max_steps_per_session)
            .field("max_session_seconds", &self.max_session_seconds)
            .field("temperature", &self.temperature)
            .field("include_nearby_blocks", &self.include_nearby_blocks)
            .field("include_nearby_entities", &self.include_nearby_entities)
            .field("include_inventory", &self.include_inventory)
            .field("limits", &self.limits)
            .finish()
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct GeminiLimits {
    #[serde(default = "default_gemini_quantity")]
    pub max_gather_quantity: u32,
    #[serde(default = "default_gemini_quantity")]
    pub max_mine_quantity: u32,
    #[serde(default = "default_gemini_quantity")]
    pub max_craft_quantity: u32,
    #[serde(default = "default_gemini_distance")]
    pub max_navigation_distance: f64,
    #[serde(default = "default_gemini_steps")]
    pub max_actions_per_session: usize,
    #[serde(default = "default_gemini_replans")]
    pub max_replans_per_session: usize,
    #[serde(default = "default_gemini_session_seconds")]
    pub max_session_seconds: u64,
    #[serde(default = "default_true")]
    pub allow_mining: bool,
    #[serde(default = "default_true")]
    pub allow_crafting: bool,
    #[serde(default = "default_true")]
    pub allow_containers: bool,
    #[serde(default)]
    pub allow_block_placement: bool,
}
fn default_gemini_model() -> String {
    "gemini-2.5-flash".into()
}
fn default_gemini_timeout() -> u64 {
    30
}
fn default_gemini_retries() -> u32 {
    2
}
fn default_gemini_steps() -> usize {
    30
}
fn default_gemini_session_seconds() -> u64 {
    600
}
fn default_gemini_temperature() -> f32 {
    0.1
}
fn default_gemini_quantity() -> u32 {
    64
}
fn default_gemini_distance() -> f64 {
    256.0
}
fn default_gemini_replans() -> usize {
    8
}
impl Default for GeminiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: None,
            api_key_env: None,
            model: default_gemini_model(),
            request_timeout_seconds: default_gemini_timeout(),
            max_request_retries: default_gemini_retries(),
            max_steps_per_session: default_gemini_steps(),
            max_session_seconds: default_gemini_session_seconds(),
            temperature: default_gemini_temperature(),
            include_nearby_blocks: true,
            include_nearby_entities: true,
            include_inventory: true,
            limits: GeminiLimits::default(),
        }
    }
}

impl GeminiConfig {
    /// Resolve without ever returning a diagnostic that contains a secret.
    /// The callback keeps tests independent from process-wide environment state.
    pub fn resolve_api_key_with(
        &self,
        env_lookup: impl FnOnce(&str) -> Option<String>,
    ) -> Result<String, AppError> {
        if let Some(key) = self.api_key.as_deref().filter(|key| !key.trim().is_empty()) {
            return Ok(key.to_owned());
        }
        if let Some(name) = self
            .api_key_env
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            && let Some(key) = env_lookup(name).filter(|key| !key.trim().is_empty())
        {
            return Ok(key);
        }
        Err(AppError::InvalidConfiguration(
            "Gemini is enabled but no API key is configured or available".into(),
        ))
    }

    pub fn resolve_api_key(&self) -> Result<String, AppError> {
        self.resolve_api_key_with(|name| std::env::var(name).ok())
    }

    fn has_key_source(&self) -> bool {
        self.api_key
            .as_deref()
            .is_some_and(|key| !key.trim().is_empty())
            || self
                .api_key_env
                .as_deref()
                .is_some_and(|name| !name.trim().is_empty())
    }
}
impl Default for GeminiLimits {
    fn default() -> Self {
        Self {
            max_gather_quantity: 64,
            max_mine_quantity: 64,
            max_craft_quantity: 64,
            max_navigation_distance: 256.0,
            max_actions_per_session: 30,
            max_replans_per_session: 8,
            max_session_seconds: 600,
            allow_mining: true,
            allow_crafting: true,
            allow_containers: true,
            allow_block_placement: true,
        }
    }
}

/// Configuration for the Groq provider (OpenAI-compatible).
#[derive(Clone, Deserialize)]
pub struct GroqConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_groq_model")]
    pub model: String,
    #[serde(default)]
    pub fallback_models: Vec<String>,
    #[serde(default = "default_groq_base_url")]
    pub base_url: String,
    #[serde(default = "default_gemini_timeout")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_gemini_retries")]
    pub max_request_retries: u32,
    #[serde(default = "default_gemini_temperature")]
    pub temperature: f32,
    #[serde(default)]
    pub service_tier: Option<String>,
    /// Caps how many tokens a single completion may generate. Groq's decode
    /// speed is already very fast (hundreds of tokens/sec on
    /// `llama-3.1-8b-instant`), so the dominant lever for perceived latency
    /// per turn is *how much* it generates, not how fast each token is.
    #[serde(default = "default_groq_max_completion_tokens")]
    pub max_completion_tokens: u32,
    #[serde(default = "default_true")]
    pub include_nearby_blocks: bool,
    #[serde(default = "default_true")]
    pub include_nearby_entities: bool,
    #[serde(default = "default_true")]
    pub include_inventory: bool,
    #[serde(default)]
    pub rate_limits: GroqRateLimitConfig,
    #[serde(default)]
    pub context: GroqContextConfig,
    #[serde(default)]
    pub limits: GeminiLimits,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GroqRateLimitConfig {
    #[serde(default = "default_true")]
    pub respect_retry_after: bool,
    #[serde(default = "default_max_retry_delay")]
    pub maximum_retry_delay_seconds: u64,
    #[serde(default = "default_true")]
    pub switch_model_on_daily_limit: bool,
    #[serde(default = "default_true")]
    pub switch_model_on_rate_limit: bool,
}

fn default_max_retry_delay() -> u64 {
    3600
}

impl Default for GroqRateLimitConfig {
    fn default() -> Self {
        Self {
            respect_retry_after: true,
            maximum_retry_delay_seconds: 3600,
            switch_model_on_daily_limit: true,
            switch_model_on_rate_limit: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct GroqContextConfig {
    #[serde(default = "default_context_chat_messages")]
    pub max_chat_messages: usize,
    #[serde(default = "default_context_inventory_entries")]
    pub max_inventory_entries: usize,
    #[serde(default = "default_context_nearby_entities")]
    pub max_nearby_entities: usize,
    #[serde(default = "default_context_nearby_blocks")]
    pub max_nearby_blocks: usize,
    #[serde(default = "default_context_action_results")]
    pub max_previous_action_results: usize,
    #[serde(default = "default_true")]
    pub include_full_command_descriptions: bool,
}

fn default_context_chat_messages() -> usize {
    8
}
fn default_context_inventory_entries() -> usize {
    36
}
fn default_context_nearby_entities() -> usize {
    12
}
fn default_context_nearby_blocks() -> usize {
    32
}
fn default_context_action_results() -> usize {
    4
}

impl Default for GroqContextConfig {
    fn default() -> Self {
        Self {
            max_chat_messages: 8,
            max_inventory_entries: 36,
            max_nearby_entities: 12,
            max_nearby_blocks: 32,
            max_previous_action_results: 4,
            include_full_command_descriptions: true,
        }
    }
}

impl std::fmt::Debug for GroqConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroqConfig")
            .field("enabled", &self.enabled)
            .field("api_key", &self.api_key.as_deref().map(|_| "***"))
            .field("api_key_env", &self.api_key_env)
            .field("model", &self.model)
            .field("fallback_models", &self.fallback_models)
            .field("base_url", &self.base_url)
            .field("request_timeout_seconds", &self.request_timeout_seconds)
            .field("max_request_retries", &self.max_request_retries)
            .field("temperature", &self.temperature)
            .field("service_tier", &self.service_tier)
            .field("max_completion_tokens", &self.max_completion_tokens)
            .field("include_nearby_blocks", &self.include_nearby_blocks)
            .field("include_nearby_entities", &self.include_nearby_entities)
            .field("include_inventory", &self.include_inventory)
            .field("rate_limits", &self.rate_limits)
            .field("context", &self.context)
            .field("limits", &self.limits)
            .finish()
    }
}

fn default_groq_model() -> String {
    "llama-3.1-8b-instant".into()
}

/// A tool-call turn is a function name plus a small JSON args object; a
/// final chat reply is meant to be one short sentence (see
/// `send_ai_chat_response`). 400 tokens comfortably covers both while
/// preventing the model from rambling and adding latency for no benefit.
fn default_groq_max_completion_tokens() -> u32 {
    250
}

fn default_groq_base_url() -> String {
    "https://api.groq.com/openai/v1".into()
}

impl Default for GroqConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key: None,
            api_key_env: Some("GROQ_API_KEY".into()),
            model: default_groq_model(),
            fallback_models: Vec::new(),
            base_url: default_groq_base_url(),
            request_timeout_seconds: 60,
            max_request_retries: 3,
            temperature: 0.1,
            service_tier: None,
            max_completion_tokens: default_groq_max_completion_tokens(),
            include_nearby_blocks: true,
            include_nearby_entities: true,
            include_inventory: true,
            rate_limits: GroqRateLimitConfig::default(),
            context: GroqContextConfig::default(),
            limits: GeminiLimits::default(),
        }
    }
}

impl GroqConfig {
    pub fn resolve_api_key(&self) -> Result<String, AppError> {
        if let Some(key) = self.api_key.as_deref()
            && !key.trim().is_empty()
        {
            return Ok(key.trim().to_owned());
        }
        if let Some(name) = self.api_key_env.as_deref().filter(|n| !n.trim().is_empty())
            && let Ok(key) = std::env::var(name.trim())
            && !key.trim().is_empty()
        {
            return Ok(key.trim().to_owned());
        }
        Err(AppError::InvalidConfiguration(
            "No Groq API key configured. Set `groq.api_key` in config or `groq.api_key_env`."
                .into(),
        ))
    }
}

/// Configuration for a local OpenAI-compatible AI server. Despite the name
/// this works for Ollama *or* LM Studio *or* anything else exposing a
/// `/v1/chat/completions` endpoint in that shape -- only `base_url`/`model`
/// need to change (Ollama: `http://localhost:11434/v1`; LM Studio:
/// `http://localhost:1234/v1`). Selected instead of Groq by setting
/// `[ai] provider = "ollama"`. No real API key is needed -- these servers
/// accept any placeholder value. `limits` is intentionally not duplicated
/// here: bot behavior limits (`[groq.limits]`) apply regardless of which
/// provider is active.
#[derive(Clone, Debug, Deserialize)]
pub struct OllamaConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_ollama_base_url")]
    pub base_url: String,
    /// Must already be pulled locally (`ollama pull <model>`) and support
    /// tool calling -- not every model does. llama3.1/3.2, qwen2.5, and
    /// mistral-nemo are known-good choices as of writing.
    #[serde(default = "default_ollama_model")]
    pub model: String,
    /// Local inference on modest hardware can be far slower than a cloud
    /// API, so this defaults higher than Groq's timeout.
    #[serde(default = "default_ollama_timeout")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_gemini_retries")]
    pub max_request_retries: u32,
    #[serde(default = "default_gemini_temperature")]
    pub temperature: f32,
    #[serde(default = "default_ollama_max_tokens")]
    pub max_tokens: u32,
}

fn default_ollama_base_url() -> String {
    "http://localhost:11434/v1".into()
}
fn default_ollama_model() -> String {
    "llama3.1".into()
}
fn default_ollama_timeout() -> u64 {
    120
}
fn default_ollama_max_tokens() -> u32 {
    250
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: default_ollama_base_url(),
            model: default_ollama_model(),
            request_timeout_seconds: default_ollama_timeout(),
            max_request_retries: 1,
            temperature: 0.1,
            max_tokens: default_ollama_max_tokens(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct TreeChoppingConfig {
    #[serde(default = "default_tree_types")]
    pub allowed_tree_types: Vec<String>,
    #[serde(default = "default_true")]
    pub require_nearby_leaves: bool,
    #[serde(default = "default_max_connected_logs")]
    pub maximum_connected_logs: usize,
    #[serde(default = "default_max_tree_height")]
    pub maximum_tree_height: u32,
    #[serde(default = "default_max_branch_distance")]
    pub maximum_branch_distance: u32,
    #[serde(default = "default_max_horizontal_logs")]
    pub maximum_horizontal_logs: usize,
    #[serde(default)]
    pub break_leaves: bool,
    #[serde(default = "default_true")]
    pub collect_saplings: bool,
    #[serde(default)]
    pub allow_hand_chopping: bool,
    #[serde(default = "default_tree_search_radius")]
    pub search_radius: u32,
    #[serde(default = "default_maximum_trees")]
    pub maximum_trees: u32,
    #[serde(default = "default_tree_timeout")]
    pub total_timeout_seconds: u64,
}
fn default_tree_types() -> Vec<String> {
    [
        "oak", "birch", "spruce", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "pale_oak",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
fn default_max_connected_logs() -> usize {
    192
}
fn default_max_tree_height() -> u32 {
    48
}
fn default_max_branch_distance() -> u32 {
    12
}
fn default_max_horizontal_logs() -> usize {
    48
}
fn default_tree_search_radius() -> u32 {
    32
}
fn default_maximum_trees() -> u32 {
    16
}
fn default_tree_timeout() -> u64 {
    300
}
impl Default for TreeChoppingConfig {
    fn default() -> Self {
        Self {
            allowed_tree_types: default_tree_types(),
            require_nearby_leaves: true,
            maximum_connected_logs: default_max_connected_logs(),
            maximum_tree_height: default_max_tree_height(),
            maximum_branch_distance: default_max_branch_distance(),
            maximum_horizontal_logs: default_max_horizontal_logs(),
            break_leaves: false,
            collect_saplings: true,
            allow_hand_chopping: false,
            search_radius: default_tree_search_radius(),
            maximum_trees: default_maximum_trees(),
            total_timeout_seconds: default_tree_timeout(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct EnsureToolConfig {
    #[serde(default = "default_tool_tiers")]
    pub material_tier_preference: Vec<String>,
    #[serde(default = "default_tool_reserve")]
    pub durability_reserve: u32,
    #[serde(default)]
    pub allow_smelting: bool,
}
fn default_tool_tiers() -> Vec<String> {
    ["iron", "stone", "wood"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}
fn default_tool_reserve() -> u32 {
    10
}
impl Default for EnsureToolConfig {
    fn default() -> Self {
        Self {
            material_tier_preference: default_tool_tiers(),
            durability_reserve: default_tool_reserve(),
            allow_smelting: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct MinecraftConfig {
    pub server: String,
    pub username: String,
    pub account_mode: AccountMode,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountMode {
    Offline,
    Microsoft,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReconnectConfig {
    pub enabled: bool,
    pub delay_seconds: u64,
    pub maximum_attempts: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ConsoleConfig {
    pub enabled: bool,
    pub send_plain_input_to_chat: bool,
    pub show_system_messages: bool,
    pub show_action_bar_messages: bool,
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            send_plain_input_to_chat: false,
            show_system_messages: true,
            show_action_bar_messages: false,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
    #[serde(default)]
    pub debug: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WorldStateConfig {
    #[serde(default = "default_entity_radius")]
    pub nearby_entity_radius: f64,
    #[serde(default = "default_maximum_entities")]
    pub maximum_tracked_entities: usize,
    #[serde(default = "default_stale_seconds")]
    pub stale_entity_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MovementConfig {
    #[serde(default = "default_follow_distance")]
    pub follow_distance: f64,
    #[serde(default = "default_repath_interval_ms")]
    pub repath_interval_ms: u64,
    #[serde(default = "default_arrival_distance")]
    pub arrival_distance: f64,
}

/// Policy for vertical construction routes (pillaring, safe digging-down)
/// that Azalea's own pathfinder cannot perform on its own. Enabled by
/// default; `denied_building_blocks` plus a built-in hard denylist (tools,
/// ores, containers, valuables) keep it from ever consuming or placing
/// anything unexpected.
#[derive(Clone, Debug, Deserialize)]
pub struct VerticalNavigationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub allow_pillaring: bool,
    #[serde(default = "default_true")]
    pub allow_digging_down: bool,
    #[serde(default = "default_true")]
    pub prefer_staircase_descent: bool,
    #[serde(default = "default_vertical_pillar_height")]
    pub max_pillar_height: u32,
    #[serde(default = "default_vertical_dig_depth")]
    pub max_dig_depth: u32,
    #[serde(default = "default_minimum_building_blocks")]
    pub minimum_building_blocks: u32,
    #[serde(default = "default_allowed_building_blocks")]
    pub allowed_building_blocks: Vec<String>,
    #[serde(default = "default_denied_building_blocks")]
    pub denied_building_blocks: Vec<String>,
}
fn default_vertical_pillar_height() -> u32 {
    32
}
fn default_vertical_dig_depth() -> u32 {
    64
}
fn default_minimum_building_blocks() -> u32 {
    4
}
fn default_allowed_building_blocks() -> Vec<String> {
    [
        "minecraft:dirt",
        "minecraft:cobblestone",
        "minecraft:stone",
        "minecraft:deepslate",
        "minecraft:netherrack",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
fn default_denied_building_blocks() -> Vec<String> {
    [
        "minecraft:diamond_block",
        "minecraft:chest",
        "minecraft:crafting_table",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
impl Default for VerticalNavigationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_pillaring: true,
            allow_digging_down: true,
            prefer_staircase_descent: true,
            max_pillar_height: default_vertical_pillar_height(),
            max_dig_depth: default_vertical_dig_depth(),
            minimum_building_blocks: default_minimum_building_blocks(),
            allowed_building_blocks: default_allowed_building_blocks(),
            denied_building_blocks: default_denied_building_blocks(),
        }
    }
}

/// Angles used by the movement adapter when pathfinding direction and camera
/// direction differ.  Keeping these in configuration prevents command code
/// from hardcoding a particular movement style.
#[derive(Clone, Debug, Deserialize)]
pub struct MultitaskingConfig {
    #[serde(default = "default_normal_forward_angle")]
    pub normal_forward_angle: f32,
    #[serde(default = "default_strafe_angle")]
    pub strafe_angle: f32,
    #[serde(default = "default_backward_angle")]
    pub backward_angle: f32,
    #[serde(default = "default_extreme_angle")]
    pub extreme_angle: f32,
}

fn default_normal_forward_angle() -> f32 {
    35.0
}
fn default_strafe_angle() -> f32 {
    75.0
}
fn default_backward_angle() -> f32 {
    130.0
}
fn default_extreme_angle() -> f32 {
    160.0
}

impl Default for MultitaskingConfig {
    fn default() -> Self {
        Self {
            normal_forward_angle: default_normal_forward_angle(),
            strafe_angle: default_strafe_angle(),
            backward_angle: default_backward_angle(),
            extreme_angle: default_extreme_angle(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct BlockSearchConfig {
    #[serde(default = "default_block_search_radius")]
    pub default_radius: u32,
    #[serde(default = "default_block_search_maximum_radius")]
    pub maximum_radius: u32,
    #[serde(default = "default_block_search_result_limit")]
    pub default_result_limit: usize,
    #[serde(default = "default_block_search_maximum_result_limit")]
    pub maximum_result_limit: usize,
    #[serde(default = "default_block_search_vertical_range")]
    pub default_vertical_range: u32,
}

fn default_block_search_radius() -> u32 {
    32
}
fn default_block_search_maximum_radius() -> u32 {
    128
}
fn default_block_search_result_limit() -> usize {
    20
}
fn default_block_search_maximum_result_limit() -> usize {
    256
}
fn default_block_search_vertical_range() -> u32 {
    32
}

impl Default for BlockSearchConfig {
    fn default() -> Self {
        Self {
            default_radius: default_block_search_radius(),
            maximum_radius: default_block_search_maximum_radius(),
            default_result_limit: default_block_search_result_limit(),
            maximum_result_limit: default_block_search_maximum_result_limit(),
            default_vertical_range: default_block_search_vertical_range(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct BlockNavigationConfig {
    #[serde(default = "default_block_search_radius")]
    pub default_search_radius: u32,
    #[serde(default = "default_block_search_maximum_radius")]
    pub maximum_search_radius: u32,
    #[serde(default = "default_block_search_result_limit")]
    pub candidate_limit: usize,
    #[serde(default = "default_interaction_distance")]
    pub interaction_distance: f64,
    #[serde(default = "default_arrival_distance")]
    pub arrival_distance: f64,
    #[serde(default = "default_maximum_target_attempts")]
    pub maximum_target_attempts: usize,
    #[serde(default = "default_stuck_timeout_seconds")]
    pub stuck_timeout_seconds: u64,
    #[serde(default = "default_maximum_navigation_seconds")]
    pub maximum_navigation_seconds: u64,
    #[serde(default = "default_block_repath_interval_ms")]
    pub repath_interval_ms: u64,
}

fn default_interaction_distance() -> f64 {
    4.5
}
fn default_maximum_target_attempts() -> usize {
    10
}
fn default_stuck_timeout_seconds() -> u64 {
    12
}
fn default_maximum_navigation_seconds() -> u64 {
    120
}
fn default_block_repath_interval_ms() -> u64 {
    1000
}

impl Default for BlockNavigationConfig {
    fn default() -> Self {
        Self {
            default_search_radius: 32,
            maximum_search_radius: 128,
            candidate_limit: 20,
            interaction_distance: 4.5,
            arrival_distance: 1.5,
            maximum_target_attempts: 10,
            stuck_timeout_seconds: 12,
            maximum_navigation_seconds: 120,
            repath_interval_ms: 1000,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct LookConfig {
    #[serde(default = "default_look_update_rate")]
    pub update_rate: u32,
    #[serde(default = "default_reaction_delay_min_ms")]
    pub reaction_delay_min_ms: u64,
    #[serde(default = "default_reaction_delay_max_ms")]
    pub reaction_delay_max_ms: u64,
    #[serde(default = "default_true")]
    pub moving_target_prediction: bool,
    #[serde(default = "default_prediction_strength")]
    pub prediction_strength: f64,
    #[serde(default = "default_minimum_target_movement")]
    pub minimum_target_movement: f64,
    #[serde(default)]
    pub randomization: LookRandomizationConfig,
    #[serde(default)]
    pub motion: LookMotionConfig,
}

fn default_look_update_rate() -> u32 {
    20
}
fn default_reaction_delay_min_ms() -> u64 {
    35
}
fn default_reaction_delay_max_ms() -> u64 {
    90
}
fn default_prediction_strength() -> f64 {
    0.35
}
fn default_minimum_target_movement() -> f64 {
    0.03
}

#[derive(Clone, Debug, Deserialize)]
pub struct LookRandomizationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub block_randomization: bool,
    #[serde(default = "default_true")]
    pub entity_randomization: bool,
    #[serde(default = "default_true")]
    pub player_randomization: bool,
    #[serde(default = "default_horizontal_strength")]
    pub horizontal_strength: f64,
    #[serde(default = "default_vertical_strength")]
    pub vertical_strength: f64,
    #[serde(default = "default_minimum_hold_time_ms")]
    pub minimum_hold_time_ms: u64,
    #[serde(default = "default_maximum_hold_time_ms")]
    pub maximum_hold_time_ms: u64,
    #[serde(default = "default_retarget_chance")]
    pub retarget_chance_per_second: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LookMotionConfig {
    #[serde(default = "default_minimum_yaw_speed")]
    pub minimum_yaw_speed: f64,
    #[serde(default = "default_maximum_yaw_speed")]
    pub maximum_yaw_speed: f64,
    #[serde(default = "default_minimum_pitch_speed")]
    pub minimum_pitch_speed: f64,
    #[serde(default = "default_maximum_pitch_speed")]
    pub maximum_pitch_speed: f64,
    #[serde(default = "default_yaw_acceleration")]
    pub yaw_acceleration: f64,
    #[serde(default = "default_pitch_acceleration")]
    pub pitch_acceleration: f64,
    #[serde(default = "default_yaw_deceleration")]
    pub yaw_deceleration: f64,
    #[serde(default = "default_pitch_deceleration")]
    pub pitch_deceleration: f64,
    #[serde(default = "default_slowdown_angle")]
    pub slowdown_angle: f64,
    #[serde(default = "default_look_arrival_tolerance")]
    pub arrival_tolerance: f64,
    #[serde(default = "default_true")]
    pub micro_correction_enabled: bool,
    #[serde(default = "default_micro_correction_strength")]
    pub micro_correction_strength: f64,
    #[serde(default = "default_speed_variation")]
    pub speed_variation: f64,
    #[serde(default = "default_overshoot_chance")]
    pub overshoot_chance: f64,
    #[serde(default = "default_maximum_overshoot_degrees")]
    pub maximum_overshoot_degrees: f64,
}

fn default_true() -> bool {
    true
}
fn default_horizontal_strength() -> f64 {
    0.35
}
fn default_vertical_strength() -> f64 {
    0.30
}
fn default_minimum_hold_time_ms() -> u64 {
    350
}
fn default_maximum_hold_time_ms() -> u64 {
    1200
}
fn default_retarget_chance() -> f64 {
    0.35
}
fn default_minimum_yaw_speed() -> f64 {
    25.0
}
fn default_maximum_yaw_speed() -> f64 {
    220.0
}
fn default_minimum_pitch_speed() -> f64 {
    20.0
}
fn default_maximum_pitch_speed() -> f64 {
    160.0
}
fn default_yaw_acceleration() -> f64 {
    600.0
}
fn default_pitch_acceleration() -> f64 {
    450.0
}
fn default_yaw_deceleration() -> f64 {
    700.0
}
fn default_pitch_deceleration() -> f64 {
    550.0
}
fn default_slowdown_angle() -> f64 {
    18.0
}
fn default_look_arrival_tolerance() -> f64 {
    1.2
}
fn default_micro_correction_strength() -> f64 {
    0.35
}
fn default_speed_variation() -> f64 {
    0.10
}
fn default_overshoot_chance() -> f64 {
    0.08
}
fn default_maximum_overshoot_degrees() -> f64 {
    1.8
}

impl Default for LookConfig {
    fn default() -> Self {
        Self {
            update_rate: 20,
            reaction_delay_min_ms: 35,
            reaction_delay_max_ms: 90,
            moving_target_prediction: true,
            prediction_strength: 0.35,
            minimum_target_movement: 0.03,
            randomization: LookRandomizationConfig::default(),
            motion: LookMotionConfig::default(),
        }
    }
}

impl Default for LookRandomizationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            block_randomization: true,
            entity_randomization: true,
            player_randomization: true,
            horizontal_strength: 0.35,
            vertical_strength: 0.30,
            minimum_hold_time_ms: 350,
            maximum_hold_time_ms: 1200,
            retarget_chance_per_second: 0.35,
        }
    }
}

impl Default for LookMotionConfig {
    fn default() -> Self {
        Self {
            minimum_yaw_speed: 25.0,
            maximum_yaw_speed: 220.0,
            minimum_pitch_speed: 20.0,
            maximum_pitch_speed: 160.0,
            yaw_acceleration: 600.0,
            pitch_acceleration: 450.0,
            yaw_deceleration: 700.0,
            pitch_deceleration: 550.0,
            slowdown_angle: 18.0,
            arrival_tolerance: 1.2,
            micro_correction_enabled: true,
            micro_correction_strength: 0.35,
            speed_variation: 0.10,
            overshoot_chance: 0.08,
            maximum_overshoot_degrees: 1.8,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct InteractionConfig {
    #[serde(default = "default_interaction_reach")]
    pub maximum_reach: f64,
    #[serde(default = "default_interaction_reach")]
    pub placement_reach: f64,
    #[serde(default = "default_interaction_reach")]
    pub breaking_reach: f64,
    #[serde(default = "default_interaction_retry_limit")]
    pub retry_limit: u32,
    #[serde(default = "default_interaction_retry_delay")]
    pub retry_delay_ms: u64,
    #[serde(default = "default_interaction_verification_timeout")]
    pub verification_timeout_ms: u64,
    #[serde(default = "default_true")]
    pub auto_navigate: bool,
    #[serde(default = "default_true")]
    pub auto_precise_look: bool,
    /// Select the fastest suitable tool already present in the hotbar before breaking.
    #[serde(default = "default_true")]
    pub auto_tool_switch: bool,
    /// Never intentionally spend the last uses of a damageable tool.
    #[serde(default = "default_minimum_tool_durability")]
    pub minimum_tool_durability: u32,
    /// Permit the held item/hand when the block does not require a tool.
    #[serde(default = "default_true")]
    pub allow_hand_fallback: bool,
    /// Fractional speed difference within which the held tool is preferred.
    #[serde(default = "default_held_tool_equivalence")]
    pub held_tool_equivalence: f32,
    /// Item identifiers excluded from automatic tool selection.
    #[serde(default)]
    pub protected_tools: Vec<String>,
    /// Item identifiers temporarily reserved for another purpose.
    #[serde(default)]
    pub reserved_tools: Vec<String>,
    #[serde(default)]
    pub face_targeting: FaceTargetingConfig,
}
#[derive(Clone, Debug, Deserialize)]
pub struct FaceTargetingConfig {
    #[serde(default = "default_face_inset")]
    pub face_inset: f64,
    #[serde(default = "default_edge_margin")]
    pub edge_margin: f64,
    #[serde(default = "default_face_attempts")]
    pub maximum_face_attempts: usize,
    #[serde(default = "default_hit_points_per_face")]
    pub maximum_hit_points_per_face: usize,
}
fn default_face_inset() -> f64 {
    0.001
}
fn default_edge_margin() -> f64 {
    0.12
}
fn default_face_attempts() -> usize {
    6
}
fn default_hit_points_per_face() -> usize {
    5
}
impl Default for FaceTargetingConfig {
    fn default() -> Self {
        Self {
            face_inset: default_face_inset(),
            edge_margin: default_edge_margin(),
            maximum_face_attempts: default_face_attempts(),
            maximum_hit_points_per_face: default_hit_points_per_face(),
        }
    }
}
fn default_interaction_reach() -> f64 {
    4.5
}
fn default_interaction_retry_limit() -> u32 {
    3
}
fn default_interaction_retry_delay() -> u64 {
    150
}
fn default_interaction_verification_timeout() -> u64 {
    1500
}
fn default_minimum_tool_durability() -> u32 {
    2
}
fn default_held_tool_equivalence() -> f32 {
    0.10
}
impl Default for InteractionConfig {
    fn default() -> Self {
        Self {
            maximum_reach: 4.5,
            placement_reach: 4.5,
            breaking_reach: 4.5,
            retry_limit: 3,
            retry_delay_ms: 150,
            verification_timeout_ms: 1500,
            auto_navigate: true,
            auto_precise_look: true,
            auto_tool_switch: true,
            minimum_tool_durability: default_minimum_tool_durability(),
            allow_hand_fallback: true,
            held_tool_equivalence: default_held_tool_equivalence(),
            protected_tools: Vec::new(),
            reserved_tools: Vec::new(),
            face_targeting: FaceTargetingConfig::default(),
        }
    }
}
fn default_follow_distance() -> f64 {
    3.0
}
fn default_repath_interval_ms() -> u64 {
    150
}
fn default_arrival_distance() -> f64 {
    1.5
}
impl Default for MovementConfig {
    fn default() -> Self {
        Self {
            follow_distance: 3.0,
            repath_interval_ms: 150,
            arrival_distance: 1.5,
        }
    }
}
fn default_entity_radius() -> f64 {
    DEFAULT_ENTITY_RADIUS
}
fn default_maximum_entities() -> usize {
    DEFAULT_MAXIMUM_ENTITIES
}
fn default_stale_seconds() -> u64 {
    DEFAULT_STALE_SECONDS
}
impl Default for WorldStateConfig {
    fn default() -> Self {
        Self {
            nearby_entity_radius: DEFAULT_ENTITY_RADIUS,
            maximum_tracked_entities: DEFAULT_MAXIMUM_ENTITIES,
            stale_entity_seconds: DEFAULT_STALE_SECONDS,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, AppError> {
        let contents = fs::read_to_string(path)?;
        let config: Self = toml::from_str(&contents)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), AppError> {
        if self.minecraft.server.trim().is_empty() {
            return Err(AppError::InvalidConfiguration(
                "minecraft.server must not be empty".to_owned(),
            ));
        }
        if self.minecraft.username.trim().is_empty() {
            return Err(AppError::InvalidConfiguration(
                "minecraft.username must not be empty".to_owned(),
            ));
        }
        if self.reconnect.enabled && self.reconnect.maximum_attempts == 0 {
            return Err(AppError::InvalidConfiguration(
                "reconnect.maximum_attempts must be greater than zero when reconnect is enabled"
                    .to_owned(),
            ));
        }
        validate_limits(
            self.world_state.nearby_entity_radius,
            self.world_state.maximum_tracked_entities,
            self.world_state.stale_entity_seconds,
        )?;
        if !(self.movement.follow_distance > 0.0 && self.movement.follow_distance <= 32.0) {
            return Err(AppError::InvalidMovementConfiguration(
                "follow_distance must be greater than zero and at most 32".into(),
            ));
        }
        if !(50..=10_000).contains(&self.movement.repath_interval_ms) {
            return Err(AppError::InvalidMovementConfiguration(
                "repath_interval_ms must be between 50 and 10000".into(),
            ));
        }
        if !(self.movement.arrival_distance > 0.0 && self.movement.arrival_distance <= 16.0) {
            return Err(AppError::InvalidMovementConfiguration(
                "arrival_distance must be greater than zero and at most 16".into(),
            ));
        }
        let multitasking = &self.multitasking;
        if !(0.0 < multitasking.normal_forward_angle
            && multitasking.normal_forward_angle <= multitasking.strafe_angle
            && multitasking.strafe_angle <= multitasking.backward_angle
            && multitasking.backward_angle <= multitasking.extreme_angle
            && multitasking.extreme_angle <= 180.0)
        {
            return Err(AppError::InvalidMovementConfiguration(
                "multitasking angles must be ordered between 0 and 180 degrees".into(),
            ));
        }
        if self.block_search.default_radius == 0
            || self.block_search.maximum_radius == 0
            || self.block_search.default_radius > self.block_search.maximum_radius
            || self.block_search.maximum_radius > 256
        {
            return Err(AppError::InvalidBlockSearchConfiguration(
                "radius must be positive, default_radius <= maximum_radius, and maximum_radius <= 256".into(),
            ));
        }
        if self.block_search.default_result_limit == 0
            || self.block_search.maximum_result_limit == 0
            || self.block_search.default_result_limit > self.block_search.maximum_result_limit
            || self.block_search.maximum_result_limit > 4096
        {
            return Err(AppError::InvalidBlockSearchConfiguration(
                "result limits must be positive, default_result_limit <= maximum_result_limit, and maximum_result_limit <= 4096".into(),
            ));
        }
        if self.block_search.default_vertical_range == 0
            || self.block_search.default_vertical_range > 384
        {
            return Err(AppError::InvalidBlockSearchConfiguration(
                "default_vertical_range must be between 1 and 384".into(),
            ));
        }
        let navigation = &self.block_navigation;
        if navigation.default_search_radius == 0
            || navigation.maximum_search_radius == 0
            || navigation.default_search_radius > navigation.maximum_search_radius
            || navigation.maximum_search_radius > 256
        {
            return Err(AppError::InvalidBlockNavigationConfiguration(
                "search radius values are invalid".into(),
            ));
        }
        if navigation.candidate_limit == 0 || navigation.candidate_limit > 4096 {
            return Err(AppError::InvalidBlockNavigationConfiguration(
                "candidate_limit must be between 1 and 4096".into(),
            ));
        }
        if !(navigation.interaction_distance > 0.0 && navigation.interaction_distance <= 16.0)
            || !(navigation.arrival_distance > 0.0 && navigation.arrival_distance <= 16.0)
            || navigation.maximum_target_attempts == 0
            || navigation.maximum_target_attempts > 64
            || navigation.stuck_timeout_seconds == 0
            || navigation.stuck_timeout_seconds > 3600
            || navigation.maximum_navigation_seconds == 0
            || navigation.maximum_navigation_seconds > 3600
            || !(100..=10_000).contains(&navigation.repath_interval_ms)
        {
            return Err(AppError::InvalidBlockNavigationConfiguration(
                "navigation distances, attempts, timeouts, or repath interval are invalid".into(),
            ));
        }
        let randomization = &self.look.randomization;
        let motion = &self.look.motion;
        if !(1..=120).contains(&self.look.update_rate)
            || self.look.reaction_delay_max_ms < self.look.reaction_delay_min_ms
            || !(0.0..=1.0).contains(&self.look.prediction_strength)
            || !(self.look.minimum_target_movement >= 0.0
                && self.look.minimum_target_movement <= 4.0)
            || !(0.0..=1.0).contains(&randomization.horizontal_strength)
            || !(0.0..=1.0).contains(&randomization.vertical_strength)
            || randomization.minimum_hold_time_ms == 0
            || randomization.maximum_hold_time_ms < randomization.minimum_hold_time_ms
            || !(0.0..=1.0).contains(&randomization.retarget_chance_per_second)
            || !(motion.minimum_yaw_speed > 0.0
                && motion.minimum_yaw_speed <= motion.maximum_yaw_speed
                && motion.maximum_yaw_speed <= 720.0)
            || !(motion.minimum_pitch_speed > 0.0
                && motion.minimum_pitch_speed <= motion.maximum_pitch_speed
                && motion.maximum_pitch_speed <= 720.0)
            || !(motion.yaw_acceleration > 0.0 && motion.yaw_acceleration <= 5000.0)
            || !(motion.pitch_acceleration > 0.0 && motion.pitch_acceleration <= 5000.0)
            || !(motion.yaw_deceleration > 0.0 && motion.yaw_deceleration <= 5000.0)
            || !(motion.pitch_deceleration > 0.0 && motion.pitch_deceleration <= 5000.0)
            || !(motion.slowdown_angle > 0.0 && motion.slowdown_angle <= 180.0)
            || !(motion.arrival_tolerance > 0.0 && motion.arrival_tolerance <= 45.0)
            || !(0.0..=1.0).contains(&motion.micro_correction_strength)
            || !(0.0..=0.5).contains(&motion.speed_variation)
            || !(0.0..=1.0).contains(&motion.overshoot_chance)
            || !(0.0..=10.0).contains(&motion.maximum_overshoot_degrees)
        {
            return Err(AppError::InvalidLookConfiguration(
                "look randomization or motion configuration is invalid".into(),
            ));
        }
        let interaction = &self.interaction;
        if !(interaction.maximum_reach > 0.0 && interaction.maximum_reach <= 10.0)
            || !(interaction.placement_reach > 0.0
                && interaction.placement_reach <= interaction.maximum_reach)
            || !(interaction.breaking_reach > 0.0
                && interaction.breaking_reach <= interaction.maximum_reach)
            || interaction.retry_limit > 10
            || interaction.retry_delay_ms == 0
            || interaction.retry_delay_ms > 10_000
            || interaction.verification_timeout_ms == 0
            || interaction.verification_timeout_ms > 30_000
            || !(interaction.face_targeting.face_inset > 0.0
                && interaction.face_targeting.face_inset < 0.5)
            || !(interaction.face_targeting.edge_margin > interaction.face_targeting.face_inset
                && interaction.face_targeting.edge_margin < 0.5)
            || interaction.face_targeting.maximum_face_attempts == 0
            || interaction.face_targeting.maximum_face_attempts > 36
            || interaction.face_targeting.maximum_hit_points_per_face == 0
            || interaction.face_targeting.maximum_hit_points_per_face > 5
            || !interaction.held_tool_equivalence.is_finite()
            || !(0.0..=1.0).contains(&interaction.held_tool_equivalence)
        {
            return Err(AppError::InvalidInteractionConfiguration(
                "reach, retry, or verification values are invalid".into(),
            ));
        }
        let tree = &self.tree_chopping;
        if tree.allowed_tree_types.is_empty()
            || tree.maximum_connected_logs == 0
            || tree.maximum_connected_logs > 4096
            || tree.maximum_tree_height == 0
            || tree.maximum_tree_height > 128
            || tree.maximum_branch_distance == 0
            || tree.maximum_branch_distance > 32
            || tree.maximum_horizontal_logs == 0
            || tree.maximum_horizontal_logs > tree.maximum_connected_logs
            || tree.search_radius == 0
            || tree.search_radius > self.block_search.maximum_radius
            || tree.maximum_trees == 0
            || tree.maximum_trees > 128
            || tree.total_timeout_seconds == 0
            || tree.total_timeout_seconds > 3600
        {
            return Err(AppError::InvalidConfiguration(
                "tree_chopping limits or allowed_tree_types are invalid".into(),
            ));
        }
        let gemini = &self.gemini;
        if gemini.model.trim().is_empty()
            || gemini.request_timeout_seconds == 0
            || gemini.max_steps_per_session == 0
            || gemini.max_session_seconds == 0
            || !(0.0..=2.0).contains(&gemini.temperature)
            || gemini.limits.max_gather_quantity == 0
            || gemini.limits.max_mine_quantity == 0
            || gemini.limits.max_craft_quantity == 0
            || gemini.limits.max_actions_per_session == 0
            || gemini.limits.max_replans_per_session == 0
            || !gemini.limits.max_navigation_distance.is_finite()
            || gemini.limits.max_navigation_distance <= 0.0
        {
            return Err(AppError::InvalidConfiguration(
                "gemini configuration or limits are invalid".into(),
            ));
        }
        if gemini.enabled && !gemini.has_key_source() {
            return Err(AppError::InvalidConfiguration(
                "Gemini is enabled but neither gemini.api_key nor gemini.api_key_env is configured"
                    .into(),
            ));
        }
        let groq = &self.groq;
        if groq.model.trim().is_empty()
            || groq.request_timeout_seconds == 0
            || groq.max_request_retries == 0
            || groq.base_url.trim().is_empty()
            || !(0.0..=2.0).contains(&groq.temperature)
            || groq.limits.max_gather_quantity == 0
            || groq.limits.max_mine_quantity == 0
            || groq.limits.max_craft_quantity == 0
            || groq.limits.max_actions_per_session == 0
            || groq.limits.max_replans_per_session == 0
            || !groq.limits.max_navigation_distance.is_finite()
            || groq.limits.max_navigation_distance <= 0.0
        {
            return Err(AppError::InvalidConfiguration(
                "groq configuration or limits are invalid".into(),
            ));
        }
        let chat = &self.ai.chat;
        if chat.enabled
            && (chat.prefix.is_empty()
                || chat.incoming_queue_capacity == 0
                || (chat.rate_limit.enabled
                    && (chat.rate_limit.requests == 0 || chat.rate_limit.window_seconds == 0))
                || chat.access.allowed_players.iter().any(|allowed| {
                    chat.access
                        .blocked_players
                        .iter()
                        .any(|blocked| allowed.eq_ignore_ascii_case(blocked))
                }))
        {
            return Err(AppError::InvalidConfiguration(
                "AI chat prefix, queue, rate limit, or access lists are invalid".into(),
            ));
        }
        Ok(())
    }
}

pub fn init_logging(config: &LoggingConfig) -> Result<(), AppError> {
    let level = config
        .level
        .parse::<LevelFilter>()
        .map_err(|_| AppError::InvalidLogLevel(config.level.clone()))?;

    let mut targets = Targets::new()
        .with_default(TargetLevel::OFF)
        .with_target(env!("CARGO_PKG_NAME"), level);
    if config.debug {
        targets = targets
            .with_target("azalea", TargetLevel::DEBUG)
            .with_target("azalea_client", TargetLevel::DEBUG)
            .with_target("azalea_entity", TargetLevel::DEBUG)
            .with_target("azalea_world", TargetLevel::DEBUG)
            .with_target("bevy", TargetLevel::DEBUG);
    }

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false).with_filter(targets))
        .try_init()
        .map_err(|error| AppError::Logging(Box::new(error)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_uses_configured_api_key() {
        let config = GeminiConfig {
            api_key: Some("file-key".into()),
            ..GeminiConfig::default()
        };
        assert_eq!(
            config
                .resolve_api_key_with(|_| Some("environment-key".into()))
                .unwrap(),
            "file-key"
        );
    }

    #[test]
    fn gemini_uses_environment_api_key_when_config_key_is_absent() {
        let config = GeminiConfig {
            api_key_env: Some("TEST_GEMINI_KEY".into()),
            ..GeminiConfig::default()
        };
        assert_eq!(
            config
                .resolve_api_key_with(
                    |name| (name == "TEST_GEMINI_KEY").then(|| "environment-key".into())
                )
                .unwrap(),
            "environment-key"
        );
    }

    #[test]
    fn gemini_prefers_configured_key_over_environment_key() {
        let config = GeminiConfig {
            api_key: Some("preferred".into()),
            api_key_env: Some("IGNORED".into()),
            ..GeminiConfig::default()
        };
        assert_eq!(
            config
                .resolve_api_key_with(|_| panic!("environment lookup must not happen"))
                .unwrap(),
            "preferred"
        );
    }

    #[test]
    fn gemini_missing_key_is_rejected_and_debug_redacts_secret() {
        let config = GeminiConfig {
            enabled: true,
            ..GeminiConfig::default()
        };
        assert!(config.resolve_api_key_with(|_| None).is_err());
        assert!(
            format!(
                "{:?}",
                GeminiConfig {
                    api_key: Some("private-key".into()),
                    ..GeminiConfig::default()
                }
            )
            .contains("********")
        );
        assert!(
            !format!(
                "{:?}",
                GeminiConfig {
                    api_key: Some("private-key".into()),
                    ..GeminiConfig::default()
                }
            )
            .contains("private-key")
        );
    }

    #[test]
    fn loads_localhost_without_requiring_a_port() {
        let config: Config = toml::from_str(
            r#"
                [minecraft]
                server = "localhost"
                username = "MagicBot"
                account_mode = "offline"

                [reconnect]
                enabled = false
                delay_seconds = 10
                maximum_attempts = 5

                [logging]
                level = "info"
            "#,
        )
        .expect("test configuration should parse");

        config.validate().expect("localhost should be valid");
        assert_eq!(config.minecraft.server, "localhost");
        assert_eq!(config.block_search.default_radius, 32);
        assert_eq!(config.block_search.default_result_limit, 20);
        assert_eq!(config.block_navigation.maximum_target_attempts, 10);
        assert_eq!(config.look.motion.maximum_yaw_speed, 220.0);
        assert_eq!(config.look.update_rate, 20);
        assert!(config.look.randomization.enabled);
        assert_eq!(config.look.reaction_delay_min_ms, 35);
        assert!(config.look.moving_target_prediction);
        assert_eq!(config.interaction.maximum_reach, 4.5);
        assert_eq!(config.multitasking.normal_forward_angle, 35.0);
        assert_eq!(config.multitasking.extreme_angle, 160.0);
    }

    #[test]
    fn rejects_enabled_reconnect_without_attempts() {
        let config: Config = toml::from_str(
            r#"
                [minecraft]
                server = "localhost:25565"
                username = "MagicBot"
                account_mode = "offline"

                [reconnect]
                enabled = true
                delay_seconds = 1
                maximum_attempts = 0

                [logging]
                level = "info"
            "#,
        )
        .expect("test configuration should parse");

        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidConfiguration(message))
                if message.contains("maximum_attempts")
        ));
    }

    #[test]
    fn rejects_invalid_movement_configuration() {
        let mut config: Config = toml::from_str(
            r#"
                [minecraft]
                server = "localhost"
                username = "MagicBot"
                account_mode = "offline"
                [reconnect]
                enabled = false
                delay_seconds = 1
                maximum_attempts = 1
                [logging]
                level = "info"
                [movement]
                follow_distance = 0
                repath_interval_ms = 500
                arrival_distance = 1.5
            "#,
        )
        .unwrap();
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidMovementConfiguration(_))
        ));
        config.movement.follow_distance = 3.0;
        config.multitasking.strafe_angle = 20.0;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidMovementConfiguration(_))
        ));
    }

    #[test]
    fn rejects_invalid_look_configuration() {
        let mut config: Config = toml::from_str(
            r#"
                [minecraft]
                server = "localhost"
                username = "MagicBot"
                account_mode = "offline"

                [reconnect]
                enabled = false
                delay_seconds = 1
                maximum_attempts = 1

                [logging]
                level = "info"

                [look]
                update_rate = 0
            "#,
        )
        .expect("test configuration should parse");

        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidLookConfiguration(_))
        ));
        config.look.update_rate = 20;
        config
            .validate()
            .expect("restored look configuration is valid");
        config.look.randomization.horizontal_strength = 1.1;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidLookConfiguration(_))
        ));
        config.look.randomization.horizontal_strength = 0.35;
        config.look.motion.minimum_yaw_speed = 221.0;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidLookConfiguration(_))
        ));
        config.look.motion.minimum_yaw_speed = 25.0;
        config.look.reaction_delay_max_ms = 20;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidLookConfiguration(_))
        ));
        config.look.reaction_delay_max_ms = 90;
        config.interaction.retry_limit = 11;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidInteractionConfiguration(_))
        ));
    }

    #[test]
    fn rejects_invalid_block_search_configuration() {
        let mut config: Config = toml::from_str(
            r#"
                [minecraft]
                server = "localhost"
                username = "MagicBot"
                account_mode = "offline"
                [reconnect]
                enabled = false
                delay_seconds = 1
                maximum_attempts = 1
                [logging]
                level = "info"
                [block_search]
                default_radius = 129
                maximum_radius = 128
                default_result_limit = 20
                maximum_result_limit = 256
                default_vertical_range = 32
            "#,
        )
        .unwrap();
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidBlockSearchConfiguration(_))
        ));
        config.block_search.default_radius = 32;
        config.block_search.maximum_radius = 128;
        config.block_search.default_result_limit = 20;
        config.block_search.maximum_result_limit = 4097;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidBlockSearchConfiguration(_))
        ));
    }

    #[test]
    fn rejects_invalid_block_navigation_configuration() {
        let mut config: Config = toml::from_str(
            r#"
                [minecraft]
                server = "localhost"
                username = "MagicBot"
                account_mode = "offline"
                [reconnect]
                enabled = false
                delay_seconds = 1
                maximum_attempts = 1
                [logging]
                level = "info"
                [block_navigation]
                default_search_radius = 32
                maximum_search_radius = 128
                candidate_limit = 20
                interaction_distance = 4.5
                arrival_distance = 1.5
                maximum_target_attempts = 0
                stuck_timeout_seconds = 12
                maximum_navigation_seconds = 120
                repath_interval_ms = 1000
            "#,
        )
        .unwrap();
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidBlockNavigationConfiguration(_))
        ));
        config.block_navigation.maximum_target_attempts = 8;
        config.block_navigation.maximum_search_radius = 0;
        assert!(matches!(
            config.validate(),
            Err(AppError::InvalidBlockNavigationConfiguration(_))
        ));
    }
}
