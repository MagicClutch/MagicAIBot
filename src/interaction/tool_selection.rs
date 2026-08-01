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
    BlockKnowledge, NoSuitableTool, ToolCandidate, ToolCategory, ToolFallbackPolicy, ToolSelection,
    ToolSelectionPolicy, category, preferred_category, select_tool, tier,
};

/// Injectable boundary for the pure selection decision. Runtime code remains
/// responsible for observing inventory and applying the selected hotbar slot.
pub(crate) trait ToolSelectionBoundary: Send + Sync {
    fn select(
        &self,
        block: &BlockKnowledge,
        candidates: &[ToolCandidate],
        policy: &ToolSelectionPolicy,
    ) -> Result<ToolSelection, NoSuitableTool>;
}

#[derive(Debug, Default)]
pub(crate) struct DeterministicToolSelector;

impl ToolSelectionBoundary for DeterministicToolSelector {
    fn select(
        &self,
        block: &BlockKnowledge,
        candidates: &[ToolCandidate],
        policy: &ToolSelectionPolicy,
    ) -> Result<ToolSelection, NoSuitableTool> {
        select_tool(block, candidates, policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(slot: u8, id: &str, speed: f32) -> ToolCandidate {
        ToolCandidate {
            hotbar_slot: slot,
            item_id: id.into(),
            category: category(id),
            tier: tier(id),
            correct_for_drops: true,
            mining_speed: speed,
            remaining_durability: Some(100),
            efficiency_level: None,
            protected: false,
            reserved: false,
            held: false,
        }
    }
    fn block(required: bool) -> BlockKnowledge {
        BlockKnowledge {
            block_id: "minecraft:stone".into(),
            preferred_category: Some(ToolCategory::Pickaxe),
            requires_correct_tool: required,
        }
    }
    fn policy() -> ToolSelectionPolicy {
        ToolSelectionPolicy {
            minimum_remaining_durability: 2,
            fallback: ToolFallbackPolicy::RequireSuitableTool,
            held_material_equivalence: 0.1,
        }
    }

    #[test]
    fn boundary_delegates_to_select_tool() {
        let candidates = vec![candidate(0, "minecraft:iron_pickaxe", 6.0)];
        let via_boundary = DeterministicToolSelector
            .select(&block(true), &candidates, &policy())
            .unwrap();
        let direct = select_tool(&block(true), &candidates, &policy()).unwrap();
        assert_eq!(via_boundary, direct);
    }
}
