use super::AiAction;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiCommandAccess {
    Allowed,
    RequiresTrustedRequester,
    Disallowed,
}

#[derive(Clone, Debug, Serialize)]
pub struct AiArgumentDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub required: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiExecutionType {
    Immediate,
    LongRunning,
    Continuous,
}

#[derive(Clone, Debug, Serialize)]
pub struct AiCommandDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub arguments: Vec<AiArgumentDefinition>,
    pub execution_type: AiExecutionType,
    pub completion_condition: &'static str,
    pub enabled: bool,
    pub ai_access: AiCommandAccess,
}

fn arg(name: &'static str, description: &'static str) -> AiArgumentDefinition {
    AiArgumentDefinition {
        name,
        description,
        required: true,
    }
}

fn opt_arg(name: &'static str, description: &'static str) -> AiArgumentDefinition {
    AiArgumentDefinition {
        name,
        description,
        required: false,
    }
}

/// Registry of only primitive capabilities exposed to the AI.
/// No task-specific or high-level commands are registered.
pub fn command_registry() -> Vec<AiCommandDefinition> {
    vec![
        // Movement
        AiCommandDefinition {
            name: "walk_to",
            description: "Walk to a world position using the existing pathfinding system.",
            arguments: vec![
                arg("x", "X coordinate"),
                arg("y", "Y coordinate"),
                arg("z", "Z coordinate"),
            ],
            execution_type: AiExecutionType::LongRunning,
            completion_condition: "Bot position is inside the destination radius",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "follow_entity",
            description: "Continuously follow a visible player or entity by name.",
            arguments: vec![arg("player", "Entity name to follow")],
            execution_type: AiExecutionType::Continuous,
            completion_condition: "Movement state confirms the requested follow target",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "look_at",
            description: "Look at a world position.",
            arguments: vec![
                arg("x", "X coordinate"),
                arg("y", "Y coordinate"),
                arg("z", "Z coordinate"),
            ],
            execution_type: AiExecutionType::LongRunning,
            completion_condition: "Bot rotation is aimed at the target",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "stop_movement",
            description: "Stop current movement.",
            arguments: vec![],
            execution_type: AiExecutionType::Immediate,
            completion_condition: "Movement stopped",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        // Block interaction
        AiCommandDefinition {
            name: "break_block",
            description: "Break one specific block. Internally handles walking into range, looking at the block, and breaking.",
            arguments: vec![
                arg("x", "X coordinate"),
                arg("y", "Y coordinate"),
                arg("z", "Z coordinate"),
            ],
            execution_type: AiExecutionType::LongRunning,
            completion_condition: "Block broken and inventory confirmed",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "place_block",
            description: "Place a block at a world position. Requires the item in the hotbar.",
            arguments: vec![
                arg("x", "X coordinate"),
                arg("y", "Y coordinate"),
                arg("z", "Z coordinate"),
                arg("block_id", "Minecraft block identifier"),
            ],
            execution_type: AiExecutionType::LongRunning,
            completion_condition: "Block placed at target position",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "use_block",
            description: "Interact with (right-click) a block at a position.",
            arguments: vec![
                arg("x", "X coordinate"),
                arg("y", "Y coordinate"),
                arg("z", "Z coordinate"),
            ],
            execution_type: AiExecutionType::LongRunning,
            completion_condition: "Block interaction completed",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        // Inventory
        AiCommandDefinition {
            name: "equip_item",
            description: "Equip an item from inventory to the active hotbar slot.",
            arguments: vec![arg("item_id", "Minecraft item identifier")],
            execution_type: AiExecutionType::Immediate,
            completion_condition: "Item selected in hotbar",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "move_inventory_item",
            description: "Move an item from one inventory slot to another. Fails cleanly if the destination slot already holds a different item -- it will not swap them. Omit count or use 0 to move the whole stack.",
            arguments: vec![
                arg("from_slot", "Source slot number"),
                arg(
                    "to_slot",
                    "Destination slot number (must be empty or already hold the same item)",
                ),
                opt_arg(
                    "count",
                    "Number of items to move; omit or 0 for the whole stack",
                ),
            ],
            execution_type: AiExecutionType::Immediate,
            completion_condition: "Item moved",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "drop_item",
            description: "Drop an item from inventory.",
            arguments: vec![
                arg("item_id", "Minecraft item identifier"),
                opt_arg(
                    "count",
                    "Number of items to drop. Omit or use 0 to drop the entire stack (\"drop all\").",
                ),
            ],
            execution_type: AiExecutionType::Immediate,
            completion_condition: "Item dropped",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "use_item",
            description: "Use the currently selected item (right-click).",
            arguments: vec![],
            execution_type: AiExecutionType::Immediate,
            completion_condition: "Item used",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        // Crafting
        AiCommandDefinition {
            name: "craft_item",
            description: "Craft a specific item. Does not collect missing materials.",
            arguments: vec![
                arg("item", "Minecraft item identifier"),
                arg("quantity", "Quantity to craft"),
            ],
            execution_type: AiExecutionType::LongRunning,
            completion_condition: "Items crafted and inventory confirmed",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        // Containers
        AiCommandDefinition {
            name: "open_container",
            description: "Open a chest or trapped chest at a position (navigates there automatically). Furnaces and other container types are not supported. Wait for this to succeed before calling take_item/store_item.",
            arguments: vec![
                arg("x", "X coordinate"),
                arg("y", "Y coordinate"),
                arg("z", "Z coordinate"),
            ],
            execution_type: AiExecutionType::LongRunning,
            completion_condition: "Container window is open",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "take_item",
            description: "Take an item from an open container.",
            arguments: vec![
                arg("item", "Minecraft item identifier"),
                arg("quantity", "Quantity to take"),
            ],
            execution_type: AiExecutionType::LongRunning,
            completion_condition: "Items transferred to inventory",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "store_item",
            description: "Store an item from inventory into an open container.",
            arguments: vec![
                arg("item", "Minecraft item identifier"),
                arg("quantity", "Quantity to store"),
            ],
            execution_type: AiExecutionType::LongRunning,
            completion_condition: "Items transferred to container",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "close_container",
            description: "Close the currently open container.",
            arguments: vec![],
            execution_type: AiExecutionType::Immediate,
            completion_condition: "Container closed",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        // Entities / Combat
        AiCommandDefinition {
            name: "attack_entity",
            description: "Attack an entity by its ID.",
            arguments: vec![arg("entity_id", "Entity ID to attack")],
            execution_type: AiExecutionType::LongRunning,
            completion_condition: "Attack executed",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "interact_with_entity",
            description: "Interact with an entity by its ID.",
            arguments: vec![arg("entity_id", "Entity ID to interact with")],
            execution_type: AiExecutionType::Immediate,
            completion_condition: "Interaction completed",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        // State / Observation
        AiCommandDefinition {
            name: "get_status",
            description: "Print current bot status to console.",
            arguments: vec![],
            execution_type: AiExecutionType::Immediate,
            completion_condition: "Status printed",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "get_inventory",
            description: "Print the current inventory contents to console.",
            arguments: vec![],
            execution_type: AiExecutionType::Immediate,
            completion_condition: "Inventory printed",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        // Control
        AiCommandDefinition {
            name: "wait",
            description: "Wait for a specified duration in milliseconds.",
            arguments: vec![arg("milliseconds", "Duration to wait in milliseconds")],
            execution_type: AiExecutionType::Immediate,
            completion_condition: "Wait completed",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "cancel_action",
            description: "Cancel the currently active physical action.",
            arguments: vec![],
            execution_type: AiExecutionType::Immediate,
            completion_condition: "Action cancelled",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
        AiCommandDefinition {
            name: "finish",
            description: "Finish the current objective and complete the session.",
            arguments: vec![opt_arg("summary", "Player-facing completion summary")],
            execution_type: AiExecutionType::Immediate,
            completion_condition: "Session completed",
            enabled: true,
            ai_access: AiCommandAccess::Allowed,
        },
    ]
}

/// Read-only state/observation tool definitions.
pub fn generate_readonly_definitions() -> Vec<super::provider::ToolDefinition> {
    let mut defs = Vec::new();

    defs.push(super::provider::ToolDefinition {
        tool_type: "function".into(),
        function: super::provider::ToolFunctionDefinition {
            name: "query_inventory".into(),
            description: "Return the bot's current inventory contents with item IDs, display names, counts, and slots. Read-only.".into(),
            parameters: serde_json::json!({
                "type": "object", "properties": {}, "required": [], "additionalProperties": false
            }),
        },
    });

    defs.push(super::provider::ToolDefinition {
        tool_type: "function".into(),
        function: super::provider::ToolFunctionDefinition {
            name: "get_recipe".into(),
            description: "Return the crafting recipe for an item, including ingredients and required count. Does not craft anything.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "item": { "type": "string", "description": "Minecraft item identifier" } },
                "required": ["item"], "additionalProperties": false
            }),
        },
    });

    defs.push(super::provider::ToolDefinition {
        tool_type: "function".into(),
        function: super::provider::ToolFunctionDefinition {
            name: "get_item_information".into(),
            description: "Return information about a Minecraft item.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "item": { "type": "string", "description": "Minecraft item identifier" } },
                "required": ["item"], "additionalProperties": false
            }),
        },
    });

    defs.push(super::provider::ToolDefinition {
        tool_type: "function".into(),
        function: super::provider::ToolFunctionDefinition {
            name: "get_block_information".into(),
            description: "Return information about a Minecraft block, including hardness and required tool.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "block": { "type": "string", "description": "Minecraft block identifier" } },
                "required": ["block"], "additionalProperties": false
            }),
        },
    });

    defs.push(super::provider::ToolDefinition {
        tool_type: "function".into(),
        function: super::provider::ToolFunctionDefinition {
            name: "get_available_capabilities".into(),
            description: "List all available primitive capabilities.".into(),
            parameters: serde_json::json!({
                "type": "object", "properties": {}, "required": [], "additionalProperties": false
            }),
        },
    });

    defs.push(super::provider::ToolDefinition {
        tool_type: "function".into(),
        function: super::provider::ToolFunctionDefinition {
            name: "get_active_action".into(),
            description: "Return information about the currently executing action, if any.".into(),
            parameters: serde_json::json!({
                "type": "object", "properties": {}, "required": [], "additionalProperties": false
            }),
        },
    });

    defs.push(super::provider::ToolDefinition {
        tool_type: "function".into(),
        function: super::provider::ToolFunctionDefinition {
            name: "get_bot_position".into(),
            description: "Returns the bot's current world position.".into(),
            parameters: serde_json::json!({
                "type": "object", "properties": {}, "required": [], "additionalProperties": false
            }),
        },
    });

    defs.push(super::provider::ToolDefinition {
        tool_type: "function".into(),
        function: super::provider::ToolFunctionDefinition {
            name: "get_bot_health".into(),
            description: "Returns the bot's current health.".into(),
            parameters: serde_json::json!({
                "type": "object", "properties": {}, "required": [], "additionalProperties": false
            }),
        },
    });

    defs.push(super::provider::ToolDefinition {
        tool_type: "function".into(),
        function: super::provider::ToolFunctionDefinition {
            name: "get_food_level".into(),
            description: "Returns the bot's current food/hunger level.".into(),
            parameters: serde_json::json!({
                "type": "object", "properties": {}, "required": [], "additionalProperties": false
            }),
        },
    });

    defs.push(super::provider::ToolDefinition {
        tool_type: "function".into(),
        function: super::provider::ToolFunctionDefinition {
            name: "get_equipment".into(),
            description: "Returns the bot's currently equipped items (hand, armor, offhand)."
                .into(),
            parameters: serde_json::json!({
                "type": "object", "properties": {}, "required": [], "additionalProperties": false
            }),
        },
    });

    defs.push(super::provider::ToolDefinition {
        tool_type: "function".into(),
        function: super::provider::ToolFunctionDefinition {
            name: "get_nearby_entities".into(),
            description: "List nearby entities (players, mobs, items) within a radius.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "radius": { "type": "integer", "description": "Search radius in blocks" } },
                "required": [], "additionalProperties": false
            }),
        },
    });

    defs.push(super::provider::ToolDefinition {
        tool_type: "function".into(),
        function: super::provider::ToolFunctionDefinition {
            name: "get_block".into(),
            description: "Get information about a specific block at a world position.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "x": { "type": "integer", "description": "X coordinate" },
                    "y": { "type": "integer", "description": "Y coordinate" },
                    "z": { "type": "integer", "description": "Z coordinate" }
                },
                "required": ["x", "y", "z"], "additionalProperties": false
            }),
        },
    });

    defs.push(super::provider::ToolDefinition {
        tool_type: "function".into(),
        function: super::provider::ToolFunctionDefinition {
            name: "find_blocks".into(),
            description: "Find matching blocks within a bounded radius.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "block_ids": {
                        "type": "array", "items": { "type": "string" },
                        "description": "Minecraft block identifiers to search for"
                    },
                    "radius": { "type": "integer", "description": "Search radius in blocks" },
                    "maximum_results": { "type": "integer", "description": "Maximum results to return" },
                    "reachable_only": { "type": "boolean", "description": "Only return reachable blocks" }
                },
                "required": ["block_ids", "radius"], "additionalProperties": false
            }),
        },
    });

    defs
}

/// Generate tool definitions including both read-only and executable tools.
pub fn generate_all_tool_definitions() -> Vec<super::provider::ToolDefinition> {
    let mut defs = generate_readonly_definitions();
    defs.extend(generate_tool_definitions());
    defs
}

/// Generate tool definitions from the command registry.
pub fn generate_tool_definitions() -> Vec<super::provider::ToolDefinition> {
    command_registry()
        .into_iter()
        .filter(|cmd| cmd.enabled && cmd.ai_access != AiCommandAccess::Disallowed)
        .map(|cmd| {
            let mut properties = serde_json::Map::new();
            let mut required = Vec::new();
            for arg in &cmd.arguments {
                let param_type = if arg.name == "block_ids" {
                    serde_json::json!({"type": "array", "items": {"type": "string"}, "description": arg.description})
                } else if arg.name == "count" || arg.name == "from_slot" || arg.name == "to_slot"
                    || arg.name == "milliseconds" || arg.name == "quantity"
                    || arg.name == "radius" || arg.name == "maximum_results"
                {
                    serde_json::json!({"type": "integer", "description": arg.description})
                } else if arg.name == "entity_id" {
                    serde_json::json!({"type": "integer", "description": arg.description})
                } else if arg.name == "x" || arg.name == "y" || arg.name == "z" {
                    if cmd.name == "walk_to" || cmd.name == "look_at" {
                        serde_json::json!({"type": "number", "description": arg.description})
                    } else {
                        serde_json::json!({"type": "integer", "description": arg.description})
                    }
                } else {
                    serde_json::json!({"type": "string", "description": arg.description})
                };
                properties.insert(arg.name.to_owned(), param_type);
                if arg.required {
                    required.push(arg.name.to_owned());
                }
            }
            let mut parameters = serde_json::Map::new();
            parameters.insert("type".into(), serde_json::Value::String("object".into()));
            parameters.insert("properties".into(), serde_json::Value::Object(properties));
            parameters.insert(
                "required".into(),
                serde_json::Value::Array(
                    required.into_iter().map(serde_json::Value::String).collect(),
                ),
            );
            parameters.insert("additionalProperties".into(), serde_json::Value::Bool(false));

            super::provider::ToolDefinition {
                tool_type: "function".into(),
                function: super::provider::ToolFunctionDefinition {
                    name: cmd.name.to_owned(),
                    description: cmd.description.to_owned(),
                    parameters: serde_json::Value::Object(parameters),
                },
            }
        })
        .collect()
}

/// Build the system prompt for the primitive capability architecture.
pub fn build_system_prompt() -> String {
    let mut prompt = String::from(
        "You control a Minecraft bot through a small set of reliable primitive capabilities.\n\n",
    );

    prompt.push_str("## Intent\n");
    prompt.push_str("First determine what the player wants. You may use read-only capabilities to look up information before deciding.\n");
    prompt.push_str("Categories:\n");
    prompt.push_str("- Conversation: casual chat, greetings, jokes, general questions\n");
    prompt.push_str(
        "- Knowledge question: Minecraft recipes, item info, block info, game mechanics\n",
    );
    prompt.push_str("- State query: what the bot is doing, its inventory, position, health\n");
    prompt.push_str("- Task execution: compose primitive capabilities to complete a request\n");
    prompt.push_str("- Cancel: stop current action\n\n");

    prompt.push_str("## Rules\n");
    prompt.push_str(
        "- For conversation or knowledge questions, respond with text only (no tool calls).\n",
    );
    prompt.push_str("- For state queries, use the read-only capabilities (query_inventory, get_bot_position, etc.).\n");
    prompt.push_str(
        "- For task execution, you MUST compose only the primitive capabilities listed below.\n",
    );
    prompt.push_str("- You do NOT have task-specific commands such as collect_stone, build_house, or craft_furnace.\n");
    prompt.push_str(
        "- You must solve every request by planning a sequence of primitive capabilities.\n",
    );
    prompt.push_str("- Never invent capability names or argument fields.\n");
    prompt.push_str("- Execute no more than one physical capability at a time.\n");
    prompt.push_str("- Wait for the result before selecting the next capability.\n");
    prompt.push_str("- Never claim success without checking the resulting state.\n");
    prompt.push_str("- Do not repeatedly call a capability whose result has not changed.\n");
    prompt
        .push_str("- When a capability fails, inspect the failure and choose a recovery action.\n");
    prompt.push_str("- When the objective is complete, call the finish capability.\n");
    prompt.push_str("- Keep chat replies concise and suitable for Minecraft chat.\n\n");

    prompt.push_str("## Read-only capabilities (safe to call anytime):\n");
    prompt.push_str("- query_inventory: Read current bot inventory contents\n");
    prompt.push_str("- get_bot_position: Get current position\n");
    prompt.push_str("- get_bot_health: Get current health\n");
    prompt.push_str("- get_food_level: Get current hunger level\n");
    prompt.push_str("- get_equipment: Get currently equipped items\n");
    prompt.push_str("- get_recipe: Look up a crafting recipe\n");
    prompt.push_str("- get_item_information: Look up item info\n");
    prompt.push_str("- get_block_information: Look up block info\n");
    prompt.push_str("- get_block: Get block at a specific coordinate\n");
    prompt.push_str("- find_blocks: Search for blocks within a radius\n");
    prompt.push_str("- get_available_capabilities: List available primitive capabilities\n");
    prompt.push_str("- get_active_action: Check current active action\n");
    prompt.push_str("- get_nearby_entities: List nearby entities\n\n");

    prompt.push_str("## Primitive capabilities:\n");

    for cmd in command_registry() {
        if !cmd.enabled || cmd.ai_access == AiCommandAccess::Disallowed {
            continue;
        }
        prompt.push_str(&format!("- {}: {}", cmd.name, cmd.description));
        if !cmd.arguments.is_empty() {
            let args: Vec<String> = cmd
                .arguments
                .iter()
                .map(|a| {
                    if a.required {
                        format!("{} ({})", a.name, a.description)
                    } else {
                        format!("{}? ({})", a.name, a.description)
                    }
                })
                .collect();
            prompt.push_str(&format!(" Args: {}", args.join(", ")));
        }
        prompt.push('\n');
    }

    prompt.push_str("\n## How to compose tasks\n");
    prompt
        .push_str("You must solve player requests by composing multiple primitive capabilities.\n");
    prompt.push_str("\nExample: collect 16 cobblestone\n");
    prompt.push_str("1. query_inventory to check current cobblestone\n");
    prompt.push_str("2. find_blocks to locate minecraft:stone\n");
    prompt.push_str("3. walk_to a stone block\n");
    prompt.push_str("4. equip_item with a pickaxe\n");
    prompt.push_str("5. break_block\n");
    prompt.push_str("6. query_inventory to verify cobblestone increased\n");
    prompt.push_str("7. Repeat 2-6 until target 16 is reached\n");
    prompt.push_str("8. finish with summary\n");
    prompt.push_str("\nExample: craft a furnace\n");
    prompt.push_str("1. get_recipe for minecraft:furnace\n");
    prompt.push_str("2. query_inventory to check cobblestone\n");
    prompt.push_str("3. find_blocks / break_block to get missing cobblestone\n");
    prompt.push_str("4. craft_item minecraft:furnace 1\n");
    prompt.push_str("5. query_inventory to verify\n");
    prompt.push_str("6. finish\n");
    prompt.push_str("\nExample: put cobblestone in a chest\n");
    prompt.push_str("1. walk_to the chest\n");
    prompt.push_str("2. open_container\n");
    prompt.push_str("3. query_inventory\n");
    prompt.push_str("4. store_item minecraft:cobblestone <amount>\n");
    prompt.push_str("5. close_container\n");
    prompt.push_str("6. finish\n");

    prompt
}

pub fn action_name(action: &AiAction) -> &'static str {
    match action {
        AiAction::GetStatus => "get_status",
        AiAction::GetInventory => "get_inventory",
        AiAction::WalkTo { .. } => "walk_to",
        AiAction::FollowEntity { .. } => "follow_entity",
        AiAction::LookAt { .. } => "look_at",
        AiAction::StopMovement => "stop_movement",
        AiAction::BreakBlock { .. } => "break_block",
        AiAction::PlaceBlock { .. } => "place_block",
        AiAction::UseBlock { .. } => "use_block",
        AiAction::EquipItem { .. } => "equip_item",
        AiAction::MoveInventoryItem { .. } => "move_inventory_item",
        AiAction::DropItem { .. } => "drop_item",
        AiAction::UseItem => "use_item",
        AiAction::CraftItem { .. } => "craft_item",
        AiAction::OpenContainer { .. } => "open_container",
        AiAction::TakeItem { .. } => "take_item",
        AiAction::StoreItem { .. } => "store_item",
        AiAction::CloseContainer => "close_container",
        AiAction::AttackEntity { .. } => "attack_entity",
        AiAction::InteractWithEntity { .. } => "interact_with_entity",
        AiAction::Wait { .. } => "wait",
        AiAction::CancelAction => "cancel_action",
        AiAction::Finish { .. } => "finish",
    }
}

pub fn action_is_registered(action: &AiAction) -> bool {
    command_registry().iter().any(|d| {
        d.enabled && d.ai_access != AiCommandAccess::Disallowed && d.name == action_name(action)
    })
}
