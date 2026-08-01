//! Pure decision policy for vertical construction (pillaring, digging down)
//! plus scaffold-block selection. No Azalea or client dependency: this module
//! only decides *what* should happen given a described situation, so it can
//! be exercised by tests without a live connection. Live execution against
//! the real client lives in [`crate::navigation::vertical_construction`].

use crate::{
    config::VerticalNavigationConfig,
    minecraft::world_state::{BlockPosition, InventorySnapshot},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerticalAction {
    Walk,
    Jump,
    MineDown,
    DigStaircaseDown,
    PillarUp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerticalFailure {
    Disabled,
    NoBlocks,
    HeightLimit,
    DigLimit,
    Lava,
    UnsafeFall,
    NoHeadroom,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerticalLimits {
    pub max_pillar_height: u32,
    pub max_dig_depth: u32,
    pub max_fall: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerticalSafety {
    pub enabled: bool,
    pub pillar: bool,
    pub dig_down: bool,
    pub staircase: bool,
    /// Count of scaffold blocks currently available for placement.
    pub blocks: u32,
    /// Whether there is enough vertical clearance above to pillar safely.
    pub headroom: bool,
    /// Whether lava was detected beneath the block about to be broken.
    pub lava_below: bool,
    /// How far the bot would fall if the next block down were removed.
    pub safe_fall: u32,
}

#[allow(dead_code)] // exposed for a future cost-aware route comparison; unused by the current single-path executor.
pub const WALK_COST: u32 = 1;
#[allow(dead_code)]
pub const JUMP_COST: u32 = 2;
#[allow(dead_code)]
pub const MINE_DOWN_COST: u32 = 8;
#[allow(dead_code)]
pub const PILLAR_COST: u32 = 20;

/// Pick only a bounded, safe vertical primitive for one unit of remaining
/// vertical distance. The executor still must select an allowed block/tool
/// and confirm the resulting world update; this function never touches the
/// world itself.
pub fn choose_vertical_action(
    delta_y: i32,
    safety: VerticalSafety,
    limits: VerticalLimits,
) -> Result<VerticalAction, VerticalFailure> {
    if !safety.enabled {
        return Err(VerticalFailure::Disabled);
    }
    if delta_y > 1 {
        let height = delta_y as u32;
        if !safety.pillar {
            return Err(VerticalFailure::Disabled);
        }
        if height > limits.max_pillar_height {
            return Err(VerticalFailure::HeightLimit);
        }
        if safety.blocks < height || safety.blocks == 0 {
            return Err(VerticalFailure::NoBlocks);
        }
        if !safety.headroom {
            return Err(VerticalFailure::NoHeadroom);
        }
        return Ok(VerticalAction::PillarUp);
    }
    if delta_y < -1 {
        let depth = delta_y.unsigned_abs();
        if !safety.dig_down {
            return Err(VerticalFailure::Disabled);
        }
        if depth > limits.max_dig_depth {
            return Err(VerticalFailure::DigLimit);
        }
        if safety.lava_below {
            return Err(VerticalFailure::Lava);
        }
        if safety.safe_fall > limits.max_fall {
            return Err(VerticalFailure::UnsafeFall);
        }
        return Ok(if safety.staircase {
            VerticalAction::DigStaircaseDown
        } else {
            VerticalAction::MineDown
        });
    }
    Ok(if delta_y == 1 {
        VerticalAction::Jump
    } else {
        VerticalAction::Walk
    })
}

#[allow(dead_code)] // exposed for a future cost-aware route comparison; unused by the current single-path executor.
pub fn action_cost(action: VerticalAction) -> u32 {
    match action {
        VerticalAction::Walk => WALK_COST,
        VerticalAction::Jump => JUMP_COST,
        VerticalAction::MineDown | VerticalAction::DigStaircaseDown => MINE_DOWN_COST,
        VerticalAction::PillarUp => PILLAR_COST,
    }
}

/// Substrings that disqualify an item from ever being used as scaffolding,
/// regardless of configuration. This is a safety net on top of
/// `denied_building_blocks`, not a replacement for it.
const HARD_DENYLIST_SUBSTRINGS: &[&str] = &[
    "pickaxe",
    "axe",
    "shovel",
    "hoe",
    "sword",
    "shears",
    "chest",
    "barrel",
    "furnace",
    "smoker",
    "crafting_table",
    "shulker_box",
    "_ore",
    "ancient_debris",
    "diamond",
    "emerald",
    "netherite",
    "totem",
    "bucket",
    "spawner",
    "beacon",
    "enchant",
    "potion",
];

fn is_hard_denied(item_id: &str) -> bool {
    HARD_DENYLIST_SUBSTRINGS
        .iter()
        .any(|bad| item_id.contains(bad))
}

/// Chooses the first configured scaffold block (in preference order) that
/// the bot holds at least `minimum_building_blocks` of, skipping anything on
/// the deny list or the built-in hard denylist. Returns `None` rather than
/// guessing with an arbitrary held item when nothing suitable is available.
pub fn select_scaffold_block(
    config: &VerticalNavigationConfig,
    inventory: &InventorySnapshot,
) -> Option<String> {
    config
        .allowed_building_blocks
        .iter()
        .filter(|id| !config.denied_building_blocks.contains(id))
        .filter(|id| !is_hard_denied(id))
        .find(|id| inventory.count_item(id) >= config.minimum_building_blocks)
        .cloned()
}

/// Picks a single cardinal step (`(dx, dz)`, each -1/0/1) that advances along
/// whichever horizontal axis has the larger remaining distance to `to`. Used
/// by staircasing and bridging to decide which direction to build in.
pub fn dominant_step_toward(from: BlockPosition, to: BlockPosition) -> (i32, i32) {
    if (to.x - from.x).abs() >= (to.z - from.z).abs() {
        ((to.x - from.x).signum(), 0)
    } else {
        (0, (to.z - from.z).signum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safety() -> VerticalSafety {
        VerticalSafety {
            enabled: true,
            pillar: true,
            dig_down: true,
            staircase: true,
            blocks: 8,
            headroom: true,
            lava_below: false,
            safe_fall: 1,
        }
    }
    fn limits() -> VerticalLimits {
        VerticalLimits {
            max_pillar_height: 8,
            max_dig_depth: 16,
            max_fall: 3,
        }
    }

    #[test]
    fn selects_pillar_for_elevated_route() {
        assert_eq!(
            choose_vertical_action(4, safety(), limits()),
            Ok(VerticalAction::PillarUp)
        );
    }
    #[test]
    fn rejects_pillar_without_blocks_or_over_limit() {
        let mut s = safety();
        s.blocks = 2;
        assert_eq!(
            choose_vertical_action(4, s, limits()),
            Err(VerticalFailure::NoBlocks)
        );
        assert_eq!(
            choose_vertical_action(9, safety(), limits()),
            Err(VerticalFailure::HeightLimit)
        );
    }
    #[test]
    fn rejects_pillar_without_headroom() {
        let mut s = safety();
        s.headroom = false;
        assert_eq!(
            choose_vertical_action(3, s, limits()),
            Err(VerticalFailure::NoHeadroom)
        );
    }
    #[test]
    fn descent_refuses_lava_and_unsafe_falls() {
        let mut s = safety();
        s.lava_below = true;
        assert_eq!(
            choose_vertical_action(-2, s, limits()),
            Err(VerticalFailure::Lava)
        );
        let mut s = safety();
        s.safe_fall = 4;
        assert_eq!(
            choose_vertical_action(-2, s, limits()),
            Err(VerticalFailure::UnsafeFall)
        );
    }
    #[test]
    fn descent_prefers_staircase_label_when_configured() {
        let s = safety();
        assert_eq!(
            choose_vertical_action(-3, s, limits()),
            Ok(VerticalAction::DigStaircaseDown)
        );
        let mut s2 = safety();
        s2.staircase = false;
        assert_eq!(
            choose_vertical_action(-3, s2, limits()),
            Ok(VerticalAction::MineDown)
        );
    }
    #[test]
    fn small_deltas_stay_ordinary_movement() {
        assert_eq!(
            choose_vertical_action(1, safety(), limits()),
            Ok(VerticalAction::Jump)
        );
        assert_eq!(
            choose_vertical_action(0, safety(), limits()),
            Ok(VerticalAction::Walk)
        );
        assert_eq!(
            choose_vertical_action(-1, safety(), limits()),
            Ok(VerticalAction::Walk)
        );
    }
    #[test]
    fn disabled_policy_rejects_everything_nontrivial() {
        let mut s = safety();
        s.enabled = false;
        assert_eq!(
            choose_vertical_action(4, s, limits()),
            Err(VerticalFailure::Disabled)
        );
        assert_eq!(
            choose_vertical_action(-4, s, limits()),
            Err(VerticalFailure::Disabled)
        );
    }
    #[test]
    fn action_costs_rank_pillaring_and_digging_above_walking() {
        assert!(action_cost(VerticalAction::PillarUp) > action_cost(VerticalAction::MineDown));
        assert!(action_cost(VerticalAction::MineDown) > action_cost(VerticalAction::Jump));
        assert!(action_cost(VerticalAction::Jump) > action_cost(VerticalAction::Walk));
    }

    fn config() -> VerticalNavigationConfig {
        VerticalNavigationConfig {
            enabled: true,
            allow_pillaring: true,
            allow_digging_down: true,
            prefer_staircase_descent: true,
            max_pillar_height: 8,
            max_dig_depth: 16,
            minimum_building_blocks: 4,
            allowed_building_blocks: [
                "minecraft:dirt",
                "minecraft:cobblestone",
                "minecraft:stone",
                "minecraft:deepslate",
                "minecraft:netherrack",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            denied_building_blocks: [
                "minecraft:diamond_block",
                "minecraft:chest",
                "minecraft:crafting_table",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }
    fn inventory(items: &[(&str, u32)]) -> InventorySnapshot {
        InventorySnapshot {
            available: true,
            total_counts: items.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn scaffold_selection_follows_preference_order() {
        let inv = inventory(&[("minecraft:cobblestone", 10), ("minecraft:dirt", 20)]);
        assert_eq!(
            select_scaffold_block(&config(), &inv).as_deref(),
            Some("minecraft:dirt")
        );
    }
    #[test]
    fn scaffold_selection_skips_below_minimum_count() {
        let inv = inventory(&[("minecraft:dirt", 1), ("minecraft:stone", 10)]);
        assert_eq!(
            select_scaffold_block(&config(), &inv).as_deref(),
            Some("minecraft:stone")
        );
    }
    #[test]
    fn scaffold_selection_never_uses_denied_or_valuable_blocks() {
        let mut cfg = config();
        cfg.allowed_building_blocks
            .push("minecraft:diamond_block".into());
        cfg.allowed_building_blocks
            .push("minecraft:iron_pickaxe".into());
        let inv = inventory(&[
            ("minecraft:diamond_block", 64),
            ("minecraft:iron_pickaxe", 1),
        ]);
        assert_eq!(select_scaffold_block(&cfg, &inv), None);
    }
    #[test]
    fn scaffold_selection_returns_none_when_nothing_suitable_is_held() {
        assert_eq!(select_scaffold_block(&config(), &inventory(&[])), None);
    }

    fn pos(x: i32, y: i32, z: i32) -> BlockPosition {
        BlockPosition { x, y, z }
    }

    #[test]
    fn dominant_step_prefers_the_larger_remaining_axis() {
        assert_eq!(dominant_step_toward(pos(0, 64, 0), pos(5, 64, 1)), (1, 0));
        assert_eq!(dominant_step_toward(pos(0, 64, 0), pos(1, 64, 5)), (0, 1));
    }
    #[test]
    fn dominant_step_handles_negative_directions_and_ties() {
        assert_eq!(dominant_step_toward(pos(5, 64, 0), pos(0, 64, 0)), (-1, 0));
        // Tied axes: x wins (checked with >=).
        assert_eq!(dominant_step_toward(pos(0, 64, 0), pos(3, 64, 3)), (1, 0));
    }
    #[test]
    fn dominant_step_is_zero_when_already_aligned() {
        assert_eq!(dominant_step_toward(pos(4, 64, 4), pos(4, 70, 4)), (0, 0));
    }
}
