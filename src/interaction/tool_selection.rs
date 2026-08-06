//! Pure, deterministic tool knowledge and selection policy.
//!
//! The actual algorithm lives in `azalea::pathfinder::tool_policy` because
//! the pathfinding engine needs it synchronously, inside a Bevy
//! system/background A* thread, and Rust's crate-dependency direction only
//! allows this crate to depend on azalea, not the reverse. This module is a
//! thin re-export so there is exactly one implementation, reused by both the
//! pathfinder's mining-cost evaluation and this app's explicit `/mine`
//! tool-selection flow (`MinecraftClient::select_tool_for_block`).

pub(crate) use azalea::pathfinder::tool_policy::{
    BlockKnowledge, ToolCandidate, ToolCategory, ToolFallbackPolicy, ToolSelection,
    ToolSelectionPolicy, category, preferred_category, select_tool, tier,
};
