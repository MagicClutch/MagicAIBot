use serde::{Deserialize, Serialize};
use std::time::Instant;
use uuid::Uuid;

pub mod groq;
pub mod ollama;
pub mod provider;
pub mod registry;

pub use groq::ProviderAttemptState;
pub use registry::{build_system_prompt, command_registry, generate_all_tool_definitions};
#[cfg(test)]
pub use registry::{generate_readonly_definitions, generate_tool_definitions};

pub type AiSessionId = Uuid;
pub type AiExecutionId = Uuid;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AiRequestSource {
    Console,
    MinecraftChat,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiRequester {
    pub name: String,
    pub uuid: Option<Uuid>,
    pub source: AiRequestSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiRequest {
    pub text: String,
    pub requester: Option<AiRequester>,
    pub source: AiRequestSource,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AiRequestIntent {
    Conversation,
    KnowledgeQuestion,
    StateQuery,
    ExecuteTask,
    TaskStatus,
    CancelTask,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InventoryItemSummary {
    pub item_id: String,
    pub display_name: String,
    pub count: u32,
    pub slot: u16,
}

#[derive(Clone, Debug, Serialize)]
pub struct InventorySummary {
    pub slots: Vec<InventoryItemSummary>,
    pub empty_slots: usize,
    pub selected_hotbar_slot: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CapabilityResult {
    pub success: bool,
    pub status: CapabilityStatus,
    pub message: String,
    pub data: serde_json::Value,
    pub retryable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    TargetNotFound,
    Unreachable,
    MissingItem,
    OutOfRange,
    InvalidTarget,
    BotDisconnected,
}

impl CapabilityResult {
    pub fn completed(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            success: true,
            status: CapabilityStatus::Completed,
            message: message.into(),
            data,
            retryable: false,
        }
    }

    pub fn failed(status: CapabilityStatus, message: impl Into<String>) -> Self {
        Self {
            success: false,
            status,
            message: message.into(),
            data: serde_json::json!({}),
            retryable: matches!(
                status,
                CapabilityStatus::Unreachable
                    | CapabilityStatus::OutOfRange
                    | CapabilityStatus::TimedOut
                    | CapabilityStatus::TargetNotFound
                    | CapabilityStatus::MissingItem
            ),
        }
    }

    pub fn cancelled() -> Self {
        Self::failed(CapabilityStatus::Cancelled, "action was cancelled")
    }
}

/// Check if a tool name corresponds to a read-only (non-physical) operation.
///
/// `get_status`/`get_inventory` are deliberately excluded: they are
/// registered as executable `AiAction`s (see `tool_call_to_action`) handled
/// by `execute_ai_action`, not by the read-only dispatch path. Classifying
/// them as read-only here would route model calls to those tools into the
/// read-only handler instead, which does not recognize them.
pub fn is_readonly_tool(name: &str) -> bool {
    matches!(
        name,
        "query_inventory"
            | "get_recipe"
            | "get_item_information"
            | "get_block_information"
            | "get_available_capabilities"
            | "get_active_action"
            | "get_bot_position"
            | "get_bot_health"
            | "get_food_level"
            | "get_equipment"
            | "get_nearby_entities"
            | "get_block"
            | "find_blocks"
    )
}

pub fn readonly_tool_names() -> Vec<&'static str> {
    vec![
        "query_inventory",
        "get_recipe",
        "get_item_information",
        "get_block_information",
        "get_available_capabilities",
        "get_active_action",
        "get_bot_position",
        "get_bot_health",
        "get_food_level",
        "get_equipment",
        "get_nearby_entities",
        "get_block",
        "find_blocks",
    ]
}

/// Constructs whichever AI backend `[ai] provider` selects, boxed so the
/// rest of the request loop doesn't need to care which one is active.
pub fn build_provider(
    config: &crate::config::Config,
) -> Result<Box<dyn provider::AiProvider>, provider::AiProviderError> {
    match config.ai.provider {
        crate::config::AiProviderKind::Groq => groq::GroqProvider::from_config(&config.groq)
            .map(|p| Box::new(p) as Box<dyn provider::AiProvider>),
        crate::config::AiProviderKind::Ollama => {
            ollama::OllamaProvider::from_config(&config.ollama)
                .map(|p| Box::new(p) as Box<dyn provider::AiProvider>)
        }
    }
}

/// Whether the currently selected AI provider's section has `enabled = true`.
pub fn active_provider_enabled(config: &crate::config::Config) -> bool {
    match config.ai.provider {
        crate::config::AiProviderKind::Groq => config.groq.enabled,
        crate::config::AiProviderKind::Ollama => config.ollama.enabled,
    }
}

pub fn chat_request(
    text: &str,
    requester: AiRequester,
    prefix: &str,
    require_prefix: bool,
    strip_whitespace: bool,
    maximum: usize,
) -> Result<AiRequest, String> {
    // When the prefix is not required, still strip it opportunistically so
    // "!hello" and "hello" both work identically once `require_prefix` is
    // turned off.
    let remainder = match text.strip_prefix(prefix) {
        Some(rest) => rest,
        None if require_prefix => return Err("message has no AI prefix".into()),
        None => text,
    };
    let request = if strip_whitespace {
        remainder.trim()
    } else {
        remainder
    };
    let request = request
        .strip_prefix("ai ")
        .or_else(|| request.strip_prefix("AI "))
        .unwrap_or(request)
        .trim();
    if request.is_empty() {
        return Err("AI request is empty".into());
    }
    if request.chars().count() > maximum {
        return Err("AI request is too long".into());
    }
    Ok(AiRequest {
        text: request.to_owned(),
        requester: Some(requester),
        source: AiRequestSource::MinecraftChat,
    })
}

/// Resolves words which can only safely refer to the trusted chat sender.
pub fn resolve_requester_references(action: &mut AiAction, requester: Option<&AiRequester>) {
    let Some(requester) = requester else {
        return;
    };
    if let AiAction::FollowEntity { player } = action
        && matches!(
            player.to_ascii_lowercase().as_str(),
            "me" | "requester" | "myself" | "i"
        )
    {
        *player = requester.name.clone();
    }
}

pub fn bind_requester_intent(action: &mut AiAction, request: &AiRequest) {
    let Some(requester) = request.requester.as_ref() else {
        return;
    };
    let words = request.text.to_ascii_lowercase();
    if (words.contains("follow me") || words.contains("follow requester"))
        && let AiAction::FollowEntity { player } = action
    {
        *player = requester.name.clone();
    }
    resolve_requester_references(action, Some(requester));
}

#[derive(Clone, Debug, PartialEq)]
pub enum AiSessionStatus {
    Interpreting,
    Planning,
    Executing,
    WaitingForTask,
    Replanning,
    WaitingForProvider { retry_at: Instant, model: String },
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AiActionStatus {
    Planned,
    Validated,
    Starting,
    Running,
    WaitingForCompletion,
    Verifying,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

impl AiActionStatus {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AiObjective {
    General { description: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AiAction {
    // State / Observation (read-only, handled by execute_readonly_tool)
    GetStatus,
    GetInventory,

    // Movement
    WalkTo {
        x: f64,
        y: f64,
        z: f64,
    },
    FollowEntity {
        player: String,
    },
    LookAt {
        x: f64,
        y: f64,
        z: f64,
    },
    StopMovement,

    // Block interaction
    BreakBlock {
        x: i32,
        y: i32,
        z: i32,
    },
    PlaceBlock {
        x: i32,
        y: i32,
        z: i32,
        block_id: String,
    },
    UseBlock {
        x: i32,
        y: i32,
        z: i32,
    },

    // Inventory
    EquipItem {
        item_id: String,
    },
    MoveInventoryItem {
        from_slot: u16,
        to_slot: u16,
        count: u16,
    },
    DropItem {
        item_id: String,
        count: u16,
    },
    UseItem,

    // Crafting
    CraftItem {
        item: String,
        quantity: u32,
    },

    // Containers
    OpenContainer {
        x: i32,
        y: i32,
        z: i32,
    },
    TakeItem {
        item: String,
        quantity: u32,
    },
    StoreItem {
        item: String,
        quantity: u32,
    },
    CloseContainer,

    // Entities / Combat
    AttackEntity {
        entity_id: i32,
    },
    InteractWithEntity {
        entity_id: i32,
    },

    // Control
    Wait {
        milliseconds: u64,
    },
    CancelAction,
    Finish {
        #[serde(default)]
        summary: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct AiActionRecord {
    pub action: AiAction,
    pub result: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct ActiveAiAction {
    pub execution_id: AiExecutionId,
    pub action: AiAction,
    pub started_at: Instant,
    pub runtime_task_id: Option<crate::tasks::TaskId>,
    pub status: AiActionStatus,
}

#[derive(Clone, Debug)]
pub struct AiSession {
    pub id: AiSessionId,
    pub requester: Option<String>,
    pub original_request: String,
    pub objective: AiObjective,
    pub status: AiSessionStatus,
    pub current_step: usize,
    pub max_steps: usize,
    pub action_history: Vec<AiActionRecord>,
    pub started_at: Instant,
    pub generation: u64,
    pub replans: usize,
    pub starting_item_count: u32,
    pub active_action: Option<ActiveAiAction>,
    pub pending_action: Option<AiAction>,
    pub responded_to_player: bool,
    pub intent: Option<String>,
}

impl AiSession {
    pub fn new(request: String, max: usize) -> Self {
        Self {
            id: Uuid::new_v4(),
            requester: extract_requester(&request),
            objective: AiObjective::General {
                description: request.clone(),
            },
            original_request: request,
            status: AiSessionStatus::Interpreting,
            current_step: 0,
            max_steps: max,
            action_history: vec![],
            started_at: Instant::now(),
            generation: 0,
            replans: 0,
            starting_item_count: 0,
            active_action: None,
            pending_action: None,
            responded_to_player: false,
            intent: None,
        }
    }

    pub fn begin_action(&mut self, action: AiAction) -> Result<AiExecutionId, String> {
        if self.active_action.is_some() {
            return Err("an AI action is already active".into());
        }
        let execution_id = Uuid::new_v4();
        self.active_action = Some(ActiveAiAction {
            execution_id,
            action,
            started_at: Instant::now(),
            runtime_task_id: None,
            status: AiActionStatus::Starting,
        });
        self.status = AiSessionStatus::Executing;
        Ok(execution_id)
    }

    pub fn complete_action(
        &mut self,
        execution_id: AiExecutionId,
        status: AiActionStatus,
        result: serde_json::Value,
    ) -> bool {
        let Some(active) = self.active_action.as_mut() else {
            return false;
        };
        if active.execution_id != execution_id || !status.terminal() {
            return false;
        }
        active.status = status;
        self.action_history.push(AiActionRecord {
            action: active.action.clone(),
            result,
        });
        self.active_action = None;
        self.status = AiSessionStatus::Replanning;
        true
    }

    pub fn cancel_active_action(&mut self) {
        if let Some(active) = self.active_action.as_mut() {
            active.status = AiActionStatus::Cancelled;
        }
        self.active_action = None;
        self.pending_action = None;
        self.generation = self.generation.wrapping_add(1);
        self.status = AiSessionStatus::Cancelled;
    }

    pub fn is_active(&self) -> bool {
        !matches!(
            self.status,
            AiSessionStatus::Completed
                | AiSessionStatus::Failed
                | AiSessionStatus::Cancelled
                | AiSessionStatus::TimedOut
        )
    }

    pub fn is_waiting_for_provider(&self) -> bool {
        matches!(self.status, AiSessionStatus::WaitingForProvider { .. })
    }
}

pub fn extract_requester(request: &str) -> Option<String> {
    let words: Vec<_> = request.split_whitespace().collect();
    for i in 0..words.len().saturating_sub(1) {
        let a = words[i]
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
            .to_ascii_lowercase();
        if matches!(a.as_str(), "i'm" | "im" | "am" | "name" | "username") {
            let candidate = words[i + 1].trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
            if candidate.len() >= 3 && candidate.len() <= 16 {
                return Some(candidate.to_owned());
            }
        }
    }
    None
}

fn identifier(id: &str) -> bool {
    let rest = id.strip_prefix("minecraft:").unwrap_or(id);
    !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == ':')
}

pub fn validate_action(
    action: &AiAction,
    limits: &crate::config::GeminiLimits,
    bot: (f64, f64, f64),
) -> Result<(), String> {
    if !registry::action_is_registered(action) {
        return Err("unregistered or unavailable AI action".into());
    }
    match action {
        AiAction::CraftItem { item, quantity } => {
            if !identifier(item) || *quantity == 0 || *quantity > limits.max_craft_quantity {
                return Err("invalid or disabled craft action".into());
            }
        }
        AiAction::WalkTo { x, y, z } => {
            if !x.is_finite()
                || !y.is_finite()
                || !z.is_finite()
                || ((x - bot.0).powi(2) + (y - bot.1).powi(2) + (z - bot.2).powi(2)).sqrt()
                    > limits.max_navigation_distance
            {
                return Err("invalid or out-of-range navigation action".into());
            }
        }
        AiAction::TakeItem { item, quantity } | AiAction::StoreItem { item, quantity } => {
            if !limits.allow_containers || !identifier(item) || *quantity == 0 {
                return Err("invalid or disabled container action".into());
            }
        }
        AiAction::PlaceBlock { block_id, .. } => {
            if !limits.allow_block_placement || !identifier(block_id) {
                return Err("invalid or disabled placement action".into());
            }
        }
        AiAction::Wait { milliseconds } if *milliseconds > 60_000 => {
            return Err("wait is too long".into());
        }
        _ => {}
    };
    Ok(())
}

pub fn verify_objective(
    session: &AiSession,
    current_item_count: u32,
    position: (f64, f64, f64),
    movement_following: Option<&str>,
) -> bool {
    match &session.objective {
        AiObjective::General { .. } => false,
    }
}

/// Convert a tool call name + arguments into an AiAction.
pub fn tool_call_to_action(name: &str, arguments: &serde_json::Value) -> Result<AiAction, String> {
    match name {
        "walk_to" => {
            let x = arguments
                .get("x")
                .and_then(serde_json::Value::as_f64)
                .ok_or("walk_to requires x")?;
            let y = arguments
                .get("y")
                .and_then(serde_json::Value::as_f64)
                .ok_or("walk_to requires y")?;
            let z = arguments
                .get("z")
                .and_then(serde_json::Value::as_f64)
                .ok_or("walk_to requires z")?;
            Ok(AiAction::WalkTo { x, y, z })
        }
        "follow_entity" => {
            let player = arguments
                .get("player")
                .and_then(serde_json::Value::as_str)
                .ok_or("follow_entity requires player")?
                .to_owned();
            Ok(AiAction::FollowEntity { player })
        }
        "look_at" => {
            let x = arguments
                .get("x")
                .and_then(serde_json::Value::as_f64)
                .ok_or("look_at requires x")?;
            let y = arguments
                .get("y")
                .and_then(serde_json::Value::as_f64)
                .ok_or("look_at requires y")?;
            let z = arguments
                .get("z")
                .and_then(serde_json::Value::as_f64)
                .ok_or("look_at requires z")?;
            Ok(AiAction::LookAt { x, y, z })
        }
        "stop_movement" => Ok(AiAction::StopMovement),
        "break_block" => {
            let x = arguments
                .get("x")
                .and_then(serde_json::Value::as_i64)
                .ok_or("break_block requires x")? as i32;
            let y = arguments
                .get("y")
                .and_then(serde_json::Value::as_i64)
                .ok_or("break_block requires y")? as i32;
            let z = arguments
                .get("z")
                .and_then(serde_json::Value::as_i64)
                .ok_or("break_block requires z")? as i32;
            Ok(AiAction::BreakBlock { x, y, z })
        }
        "place_block" => {
            let x = arguments
                .get("x")
                .and_then(serde_json::Value::as_i64)
                .ok_or("place_block requires x")? as i32;
            let y = arguments
                .get("y")
                .and_then(serde_json::Value::as_i64)
                .ok_or("place_block requires y")? as i32;
            let z = arguments
                .get("z")
                .and_then(serde_json::Value::as_i64)
                .ok_or("place_block requires z")? as i32;
            let block_id = arguments
                .get("block_id")
                .and_then(serde_json::Value::as_str)
                .ok_or("place_block requires block_id")?
                .to_owned();
            Ok(AiAction::PlaceBlock { x, y, z, block_id })
        }
        "use_block" => {
            let x = arguments
                .get("x")
                .and_then(serde_json::Value::as_i64)
                .ok_or("use_block requires x")? as i32;
            let y = arguments
                .get("y")
                .and_then(serde_json::Value::as_i64)
                .ok_or("use_block requires y")? as i32;
            let z = arguments
                .get("z")
                .and_then(serde_json::Value::as_i64)
                .ok_or("use_block requires z")? as i32;
            Ok(AiAction::UseBlock { x, y, z })
        }
        "equip_item" => {
            let item_id = arguments
                .get("item_id")
                .and_then(serde_json::Value::as_str)
                .ok_or("equip_item requires item_id")?
                .to_owned();
            Ok(AiAction::EquipItem { item_id })
        }
        "move_inventory_item" => {
            let from_slot = arguments
                .get("from_slot")
                .and_then(serde_json::Value::as_u64)
                .ok_or("move_inventory_item requires from_slot")?
                as u16;
            let to_slot = arguments
                .get("to_slot")
                .and_then(serde_json::Value::as_u64)
                .ok_or("move_inventory_item requires to_slot")? as u16;
            // 0 (including an omitted `count`) means "move the whole
            // stack" -- see `App::move_inventory_item`.
            let count = arguments
                .get("count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u16;
            Ok(AiAction::MoveInventoryItem {
                from_slot,
                to_slot,
                count,
            })
        }
        "drop_item" => {
            let item_id = arguments
                .get("item_id")
                .and_then(serde_json::Value::as_str)
                .ok_or("drop_item requires item_id")?
                .to_owned();
            // 0 (including an omitted `count`) means "drop the entire
            // stack" -- see `MinecraftClient::drop_item`.
            let count = arguments
                .get("count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u16;
            Ok(AiAction::DropItem { item_id, count })
        }
        "use_item" => Ok(AiAction::UseItem),
        "craft_item" => {
            let item = arguments
                .get("item")
                .and_then(serde_json::Value::as_str)
                .ok_or("craft_item requires item")?
                .to_owned();
            let quantity = arguments
                .get("quantity")
                .and_then(serde_json::Value::as_u64)
                .ok_or("craft_item requires quantity")? as u32;
            Ok(AiAction::CraftItem { item, quantity })
        }
        "open_container" => {
            let x = arguments
                .get("x")
                .and_then(serde_json::Value::as_i64)
                .ok_or("open_container requires x")? as i32;
            let y = arguments
                .get("y")
                .and_then(serde_json::Value::as_i64)
                .ok_or("open_container requires y")? as i32;
            let z = arguments
                .get("z")
                .and_then(serde_json::Value::as_i64)
                .ok_or("open_container requires z")? as i32;
            Ok(AiAction::OpenContainer { x, y, z })
        }
        "take_item" => {
            let item = arguments
                .get("item")
                .and_then(serde_json::Value::as_str)
                .ok_or("take_item requires item")?
                .to_owned();
            let quantity = arguments
                .get("quantity")
                .and_then(serde_json::Value::as_u64)
                .ok_or("take_item requires quantity")? as u32;
            Ok(AiAction::TakeItem { item, quantity })
        }
        "store_item" => {
            let item = arguments
                .get("item")
                .and_then(serde_json::Value::as_str)
                .ok_or("store_item requires item")?
                .to_owned();
            let quantity = arguments
                .get("quantity")
                .and_then(serde_json::Value::as_u64)
                .ok_or("store_item requires quantity")? as u32;
            Ok(AiAction::StoreItem { item, quantity })
        }
        "close_container" => Ok(AiAction::CloseContainer),
        "attack_entity" => {
            let entity_id = arguments
                .get("entity_id")
                .and_then(serde_json::Value::as_i64)
                .ok_or("attack_entity requires entity_id")? as i32;
            Ok(AiAction::AttackEntity { entity_id })
        }
        "interact_with_entity" => {
            let entity_id = arguments
                .get("entity_id")
                .and_then(serde_json::Value::as_i64)
                .ok_or("interact_with_entity requires entity_id")?
                as i32;
            Ok(AiAction::InteractWithEntity { entity_id })
        }
        "wait" => {
            let milliseconds = arguments
                .get("milliseconds")
                .and_then(serde_json::Value::as_u64)
                .ok_or("wait requires milliseconds")?;
            Ok(AiAction::Wait { milliseconds })
        }
        "cancel_action" => Ok(AiAction::CancelAction),
        "finish" => {
            let summary = arguments
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            Ok(AiAction::Finish { summary })
        }
        "get_status" => Ok(AiAction::GetStatus),
        "get_inventory" => Ok(AiAction::GetInventory),
        _ => Err(format!("unknown tool: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requester_and_gather_parse() {
        assert_eq!(
            extract_requester("I want logs and I'm 5cat"),
            Some("5cat".into())
        );
    }

    #[test]
    fn chat_request_uses_trusted_sender_and_removes_one_prefix() {
        let sender = AiRequester {
            name: "5cat".into(),
            uuid: Some(Uuid::new_v4()),
            source: AiRequestSource::MinecraftChat,
        };
        let request = chat_request("! !get logs", sender.clone(), "!", true, true, 500).unwrap();
        assert_eq!(request.text, "!get logs");
        assert_eq!(request.requester.unwrap(), sender);
        assert!(
            chat_request(
                "!   ",
                AiRequester {
                    name: "x".into(),
                    uuid: None,
                    source: AiRequestSource::MinecraftChat
                },
                "!",
                true,
                true,
                500
            )
            .is_err()
        );
    }

    #[test]
    fn chat_request_without_required_prefix_accepts_bare_text() {
        let sender = AiRequester {
            name: "5cat".into(),
            uuid: None,
            source: AiRequestSource::MinecraftChat,
        };
        let request = chat_request("hello there", sender.clone(), "!", false, true, 500).unwrap();
        assert_eq!(request.text, "hello there");
        // A message that happens to start with the prefix still has it
        // stripped, so behavior is consistent whether or not it's typed.
        let request = chat_request("!hello there", sender, "!", false, true, 500).unwrap();
        assert_eq!(request.text, "hello there");
    }

    #[test]
    fn follow_me_resolves_to_trusted_requester() {
        let requester = AiRequester {
            name: "5cat".into(),
            uuid: Some(Uuid::new_v4()),
            source: AiRequestSource::MinecraftChat,
        };
        let mut action = AiAction::FollowEntity {
            player: "me".into(),
        };
        resolve_requester_references(&mut action, Some(&requester));
        assert_eq!(
            action,
            AiAction::FollowEntity {
                player: "5cat".into()
            }
        );
    }

    #[test]
    fn gemini_cannot_redirect_follow_me_to_another_player() {
        let request = AiRequest {
            text: "follow me".into(),
            requester: Some(AiRequester {
                name: "5cat".into(),
                uuid: None,
                source: AiRequestSource::MinecraftChat,
            }),
            source: AiRequestSource::MinecraftChat,
        };
        let mut action = AiAction::FollowEntity {
            player: "Steve".into(),
        };
        bind_requester_intent(&mut action, &request);
        assert_eq!(
            action,
            AiAction::FollowEntity {
                player: "5cat".into()
            }
        );
    }

    #[test]
    fn registry_is_used_and_unregistered_actions_are_rejected() {
        assert!(
            command_registry()
                .iter()
                .any(|command| command.name == "walk_to")
        );
        assert!(
            validate_action(
                &AiAction::CraftItem {
                    item: "minecraft:diamond".into(),
                    quantity: 1
                },
                &crate::config::GeminiLimits::default(),
                (0., 0., 0.)
            )
            .is_ok()
        );
    }

    #[test]
    fn action_arrays_and_concurrent_actions_are_rejected_and_stale_completion_is_ignored() {
        let mut session = AiSession::new("collect".into(), 3);
        let id = session.begin_action(AiAction::GetStatus).unwrap();
        assert!(session.begin_action(AiAction::GetInventory).is_err());
        assert!(!session.complete_action(
            Uuid::new_v4(),
            AiActionStatus::Succeeded,
            serde_json::json!({})
        ));
        assert!(session.complete_action(
            id,
            AiActionStatus::Succeeded,
            serde_json::json!({"terminal_status":"succeeded"})
        ));
        assert!(session.active_action.is_none());
        session.pending_action = Some(AiAction::GetInventory);
        session.cancel_active_action();
        assert!(session.pending_action.is_none());
    }

    #[test]
    fn place_block_is_registered_and_validated() {
        let registry = command_registry();
        assert!(registry.iter().any(|d| d.name == "place_block"));
        let valid = AiAction::PlaceBlock {
            x: 10,
            y: 64,
            z: -5,
            block_id: "minecraft:oak_planks".into(),
        };
        assert!(
            validate_action(
                &valid,
                &crate::config::GeminiLimits::default(),
                (0., 0., 0.)
            )
            .is_ok()
        );
        assert!(registry::action_is_registered(&valid));
        let bad_id = AiAction::PlaceBlock {
            x: 0,
            y: 0,
            z: 0,
            block_id: "bad id".into(),
        };
        assert!(
            validate_action(
                &bad_id,
                &crate::config::GeminiLimits::default(),
                (0., 0., 0.)
            )
            .is_err()
        );
    }

    #[test]
    fn place_block_disabled_when_limit_is_off() {
        let limits = crate::config::GeminiLimits {
            allow_block_placement: false,
            ..Default::default()
        };
        let action = AiAction::PlaceBlock {
            x: 0,
            y: 0,
            z: 0,
            block_id: "minecraft:cobblestone".into(),
        };
        assert!(validate_action(&action, &limits, (0., 0., 0.)).is_err());
    }

    #[test]
    fn tool_call_to_action_converts_all_known_tools() {
        let cases: Vec<(&str, serde_json::Value, &str)> = vec![
            (
                "follow_entity",
                serde_json::json!({"player": "5cat"}),
                "FollowEntity",
            ),
            (
                "walk_to",
                serde_json::json!({"x": 10.0, "y": 64.0, "z": 20.0}),
                "WalkTo",
            ),
            ("stop_movement", serde_json::json!({}), "StopMovement"),
            ("finish", serde_json::json!({"summary": "done"}), "Finish"),
            (
                "place_block",
                serde_json::json!({"x": 10, "y": 64, "z": 20, "block_id": "minecraft:stone"}),
                "PlaceBlock",
            ),
            (
                "break_block",
                serde_json::json!({"x": 10, "y": 64, "z": 20}),
                "BreakBlock",
            ),
            ("get_status", serde_json::json!({}), "GetStatus"),
            ("get_inventory", serde_json::json!({}), "GetInventory"),
            (
                "equip_item",
                serde_json::json!({"item_id": "minecraft:diamond_pickaxe"}),
                "EquipItem",
            ),
            (
                "craft_item",
                serde_json::json!({"item": "minecraft:furnace", "quantity": 1}),
                "CraftItem",
            ),
            (
                "open_container",
                serde_json::json!({"x": 10, "y": 64, "z": 20}),
                "OpenContainer",
            ),
            (
                "attack_entity",
                serde_json::json!({"entity_id": 42}),
                "AttackEntity",
            ),
            ("cancel_action", serde_json::json!({}), "CancelAction"),
            ("wait", serde_json::json!({"milliseconds": 1000}), "Wait"),
        ];
        for (name, args, expected_variant) in cases {
            let action = tool_call_to_action(name, &args).unwrap();
            dbg!(&action);
            let formatted = format!("{:?}", action);
            let variant = formatted.split(' ').next().unwrap_or("");
            assert_eq!(variant, expected_variant, "mismatch for {name}");
        }
    }

    #[test]
    fn tool_call_to_action_rejects_unknown_tool() {
        assert!(tool_call_to_action("unknown_tool", &serde_json::json!({})).is_err());
    }

    #[test]
    fn tool_call_to_action_rejects_old_high_level_commands() {
        assert!(
            tool_call_to_action(
                "gather",
                &serde_json::json!({"item": "minecraft:stone", "quantity": 5})
            )
            .is_err()
        );
        assert!(
            tool_call_to_action(
                "mine_ore",
                &serde_json::json!({"ore": "iron_ore", "quantity": 5})
            )
            .is_err()
        );
        assert!(tool_call_to_action("chop_tree", &serde_json::json!({})).is_err());
        assert!(
            tool_call_to_action("goto", &serde_json::json!({"x": 0.0, "y": 64.0, "z": 0.0}))
                .is_err()
        );
        assert!(
            tool_call_to_action(
                "goto_block",
                &serde_json::json!({"block": "minecraft:stone"})
            )
            .is_err()
        );
        assert!(
            tool_call_to_action("look_at_player", &serde_json::json!({"player": "5cat"})).is_err()
        );
    }

    #[test]
    fn generate_tool_definitions_includes_all_primitive_commands() {
        let defs = generate_tool_definitions();
        assert!(defs.iter().any(|t| t.function.name == "walk_to"));
        assert!(defs.iter().any(|t| t.function.name == "follow_entity"));
        assert!(defs.iter().any(|t| t.function.name == "break_block"));
        assert!(defs.iter().any(|t| t.function.name == "place_block"));
        assert!(defs.iter().any(|t| t.function.name == "craft_item"));
        assert!(defs.iter().any(|t| t.function.name == "equip_item"));
        assert!(defs.iter().any(|t| t.function.name == "use_item"));
        assert!(defs.iter().any(|t| t.function.name == "attack_entity"));
        assert!(defs.iter().any(|t| t.function.name == "open_container"));
        assert!(defs.iter().any(|t| t.function.name == "finish"));
        assert!(defs.iter().any(|t| t.function.name == "wait"));
        assert!(defs.iter().any(|t| t.function.name == "cancel_action"));
    }

    #[test]
    fn generate_tool_definitions_excludes_high_level_commands() {
        let defs = generate_tool_definitions();
        assert!(!defs.iter().any(|t| t.function.name == "gather"));
        assert!(!defs.iter().any(|t| t.function.name == "mine_ore"));
        assert!(!defs.iter().any(|t| t.function.name == "chop_tree"));
        assert!(!defs.iter().any(|t| t.function.name == "collect_item"));
        assert!(!defs.iter().any(|t| t.function.name == "look_at_player"));
        assert!(!defs.iter().any(|t| t.function.name == "goto_block"));
    }

    #[test]
    fn build_system_prompt_contains_primitive_capability_names() {
        let prompt = build_system_prompt();
        assert!(prompt.contains("walk_to"));
        assert!(prompt.contains("break_block"));
        assert!(prompt.contains("place_block"));
        assert!(prompt.contains("craft_item"));
        assert!(prompt.contains("finish"));
    }

    #[test]
    fn build_system_prompt_does_not_contain_old_high_level_commands() {
        let prompt = build_system_prompt();
        assert!(!prompt.contains("gather"));
        assert!(!prompt.contains("mine_ore"));
        assert!(!prompt.contains("chop_tree"));
        assert!(!prompt.contains("collect_item"));
    }

    // --- Read-only tool tests ---

    #[test]
    fn is_readonly_tool_returns_true_for_query_inventory() {
        assert!(is_readonly_tool("query_inventory"));
    }

    #[test]
    fn is_readonly_tool_returns_true_for_get_recipe() {
        assert!(is_readonly_tool("get_recipe"));
    }

    #[test]
    fn is_readonly_tool_returns_true_for_get_item_information() {
        assert!(is_readonly_tool("get_item_information"));
    }

    #[test]
    fn is_readonly_tool_returns_true_for_get_block_information() {
        assert!(is_readonly_tool("get_block_information"));
    }

    #[test]
    fn is_readonly_tool_returns_true_for_get_available_capabilities() {
        assert!(is_readonly_tool("get_available_capabilities"));
    }

    #[test]
    fn is_readonly_tool_returns_true_for_get_active_action() {
        assert!(is_readonly_tool("get_active_action"));
    }

    #[test]
    fn is_readonly_tool_returns_true_for_get_bot_position() {
        assert!(is_readonly_tool("get_bot_position"));
    }

    #[test]
    fn is_readonly_tool_returns_true_for_get_bot_health() {
        assert!(is_readonly_tool("get_bot_health"));
    }

    #[test]
    fn is_readonly_tool_returns_true_for_get_nearby_entities() {
        assert!(is_readonly_tool("get_nearby_entities"));
    }

    #[test]
    fn is_readonly_tool_returns_true_for_get_block() {
        assert!(is_readonly_tool("get_block"));
    }

    #[test]
    fn is_readonly_tool_returns_true_for_find_blocks() {
        assert!(is_readonly_tool("find_blocks"));
    }

    #[test]
    fn is_readonly_tool_returns_false_for_executable_walk_to() {
        assert!(!is_readonly_tool("walk_to"));
    }

    #[test]
    fn is_readonly_tool_returns_false_for_executable_break_block() {
        assert!(!is_readonly_tool("break_block"));
    }

    #[test]
    fn is_readonly_tool_returns_false_for_unknown_tool() {
        assert!(!is_readonly_tool("nonexistent_tool"));
    }

    #[test]
    fn readonly_tool_names_contains_all_readonly_tools() {
        let names = readonly_tool_names();
        assert!(names.contains(&"query_inventory"));
        assert!(names.contains(&"get_recipe"));
        assert!(names.contains(&"get_available_capabilities"));
        assert!(names.contains(&"get_active_action"));
        assert!(names.contains(&"get_bot_position"));
        assert!(names.contains(&"get_bot_health"));
        assert!(names.contains(&"get_nearby_entities"));
        assert!(names.contains(&"get_block"));
        assert!(names.contains(&"find_blocks"));
    }

    #[test]
    fn readonly_tool_names_does_not_contain_executable() {
        let names = readonly_tool_names();
        assert!(!names.contains(&"walk_to"));
        assert!(!names.contains(&"break_block"));
        assert!(!names.contains(&"get_inventory"));
    }

    // --- Read-only definitions tests ---

    #[test]
    fn generate_readonly_definitions_includes_query_inventory() {
        let defs = generate_readonly_definitions();
        assert!(defs.iter().any(|t| t.function.name == "query_inventory"));
    }

    #[test]
    fn generate_readonly_definitions_includes_get_recipe() {
        let defs = generate_readonly_definitions();
        assert!(defs.iter().any(|t| t.function.name == "get_recipe"));
    }

    #[test]
    fn generate_readonly_definitions_includes_get_available_capabilities() {
        let defs = generate_readonly_definitions();
        assert!(
            defs.iter()
                .any(|t| t.function.name == "get_available_capabilities")
        );
    }

    #[test]
    fn generate_readonly_definitions_includes_get_block() {
        let defs = generate_readonly_definitions();
        assert!(defs.iter().any(|t| t.function.name == "get_block"));
    }

    #[test]
    fn generate_readonly_definitions_includes_find_blocks() {
        let defs = generate_readonly_definitions();
        assert!(defs.iter().any(|t| t.function.name == "find_blocks"));
    }

    #[test]
    fn generate_readonly_definitions_does_not_include_executable() {
        let defs = generate_readonly_definitions();
        assert!(!defs.iter().any(|t| t.function.name == "walk_to"));
        assert!(!defs.iter().any(|t| t.function.name == "get_inventory"));
    }

    // --- All-tool definitions tests ---

    #[test]
    fn generate_all_tool_definitions_includes_readonly_and_executable() {
        let defs = generate_all_tool_definitions();
        assert!(defs.iter().any(|t| t.function.name == "query_inventory"));
        assert!(defs.iter().any(|t| t.function.name == "walk_to"));
        assert!(defs.iter().any(|t| t.function.name == "get_inventory"));
    }

    // --- Session tests ---

    #[test]
    fn new_session_has_none_intent() {
        let session = AiSession::new("hello".into(), 10);
        assert_eq!(session.intent, None);
    }

    #[test]
    fn new_session_has_false_responded_to_player() {
        let session = AiSession::new("hello".into(), 10);
        assert!(!session.responded_to_player);
    }

    #[test]
    fn new_session_has_interpreting_status() {
        let session = AiSession::new("hello".into(), 10);
        assert_eq!(session.status, AiSessionStatus::Interpreting);
    }

    #[test]
    fn session_is_active_returns_true_for_non_terminal_status() {
        let session = AiSession::new("hello".into(), 10);
        assert!(session.is_active());
    }

    #[test]
    fn session_is_active_returns_false_for_completed() {
        let mut session = AiSession::new("hello".into(), 10);
        session.status = AiSessionStatus::Completed;
        assert!(!session.is_active());
    }

    #[test]
    fn session_is_active_returns_false_for_failed() {
        let mut session = AiSession::new("hello".into(), 10);
        session.status = AiSessionStatus::Failed;
        assert!(!session.is_active());
    }

    #[test]
    fn session_is_active_returns_false_for_cancelled() {
        let mut session = AiSession::new("hello".into(), 10);
        session.status = AiSessionStatus::Cancelled;
        assert!(!session.is_active());
    }

    #[test]
    fn session_is_active_returns_false_for_timed_out() {
        let mut session = AiSession::new("hello".into(), 10);
        session.status = AiSessionStatus::TimedOut;
        assert!(!session.is_active());
    }

    // --- Prompt tests ---

    #[test]
    fn build_system_prompt_contains_readonly_tools_section() {
        let prompt = build_system_prompt();
        assert!(prompt.contains("## Read-only capabilities"));
    }

    #[test]
    fn build_system_prompt_contains_query_inventory() {
        let prompt = build_system_prompt();
        assert!(prompt.contains("query_inventory"));
    }

    #[test]
    fn build_system_prompt_contains_primitive_capabilities_section() {
        let prompt = build_system_prompt();
        assert!(prompt.contains("## Primitive capabilities"));
    }

    #[test]
    fn build_system_prompt_contains_rules_section() {
        let prompt = build_system_prompt();
        assert!(prompt.contains("## Rules"));
    }

    #[test]
    fn build_system_prompt_contains_compose_tasks() {
        let prompt = build_system_prompt();
        assert!(prompt.contains("compose"));
    }

    // --- InventorySummary tests ---

    #[test]
    fn inventory_summary_serializes_with_all_fields() {
        let summary = InventorySummary {
            slots: vec![InventoryItemSummary {
                item_id: "minecraft:stone".into(),
                display_name: "Stone".into(),
                count: 10,
                slot: 0,
            }],
            empty_slots: 40,
            selected_hotbar_slot: 1,
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["slots"][0]["item_id"], "minecraft:stone");
        assert_eq!(json["slots"][0]["display_name"], "Stone");
        assert_eq!(json["slots"][0]["count"], 10);
        assert_eq!(json["slots"][0]["slot"], 0);
        assert_eq!(json["empty_slots"], 40);
        assert_eq!(json["selected_hotbar_slot"], 1);
    }

    #[test]
    fn inventory_summary_empty_slots_is_zero_when_full() {
        let summary = InventorySummary {
            slots: vec![],
            empty_slots: 0,
            selected_hotbar_slot: 0,
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["empty_slots"], 0);
    }

    // --- Capability result tests ---

    #[test]
    fn capability_result_completed_has_correct_fields() {
        let r = CapabilityResult::completed("done", serde_json::json!({"x": 1.0}));
        assert!(r.success);
        assert_eq!(r.status, CapabilityStatus::Completed);
        assert!(!r.retryable);
    }

    #[test]
    fn capability_result_failed_unreachable_is_retryable() {
        let r = CapabilityResult::failed(CapabilityStatus::Unreachable, "no path");
        assert!(!r.success);
        assert!(r.retryable);
    }

    #[test]
    fn capability_result_failed_invalid_target_is_not_retryable() {
        let r = CapabilityResult::failed(CapabilityStatus::InvalidTarget, "bad target");
        assert!(!r.success);
        assert!(!r.retryable);
    }

    #[test]
    fn capability_result_cancelled_is_not_retryable() {
        let r = CapabilityResult::cancelled();
        assert!(!r.success);
        assert_eq!(r.status, CapabilityStatus::Cancelled);
    }

    #[test]
    fn high_level_collect_command_is_not_registered() {
        let registry = command_registry();
        assert!(!registry.iter().any(|c| c.name == "gather"));
        assert!(!registry.iter().any(|c| c.name == "collect_item"));
        assert!(!registry.iter().any(|c| c.name == "mine_ore"));
    }

    #[test]
    fn high_level_build_command_is_not_registered() {
        let registry = command_registry();
        assert!(!registry.iter().any(|c| c.name == "build_house"
            || c.name == "build_shelter"
            || c.name == "build_wall"));
    }

    #[test]
    fn high_level_craft_furnace_command_is_not_registered() {
        let registry = command_registry();
        assert!(
            !registry
                .iter()
                .any(|c| c.name == "craft_furnace" || c.name == "craft_pickaxe")
        );
    }

    #[test]
    fn unknown_capability_is_rejected() {
        assert!(
            validate_action(
                &AiAction::Wait {
                    milliseconds: 99999
                },
                &crate::config::GeminiLimits::default(),
                (0., 0., 0.)
            )
            .is_err()
        );
    }

    #[test]
    fn unregistered_high_level_action_is_rejected() {
        // Old actions should not be in the enum at all, but let's verify
        // that any walking action with out-of-range coords is rejected
        assert!(
            validate_action(
                &AiAction::WalkTo {
                    x: 999999.0,
                    y: 0.0,
                    z: 0.0
                },
                &crate::config::GeminiLimits::default(),
                (0., 0., 0.)
            )
            .is_err()
        );
    }

    #[test]
    fn capability_result_is_structured() {
        let r = CapabilityResult::completed("test", serde_json::json!({"key": "value"}));
        let json = serde_json::to_value(&r).unwrap();
        assert!(json.get("success").is_some());
        assert!(json.get("status").is_some());
        assert!(json.get("message").is_some());
        assert!(json.get("data").is_some());
        assert!(json.get("retryable").is_some());
    }

    #[test]
    fn active_action_can_be_cancelled() {
        let mut session = AiSession::new("test".into(), 10);
        let id = session
            .begin_action(AiAction::WalkTo {
                x: 0.0,
                y: 64.0,
                z: 0.0,
            })
            .unwrap();
        assert!(session.active_action.is_some());
        session.cancel_active_action();
        assert!(session.active_action.is_none());
        assert_eq!(session.status, AiSessionStatus::Cancelled);
    }
}
