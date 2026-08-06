//! Pure sword/pickaxe/axe/shovel ranking for the automatic hotbar
//! equipment system. No Azalea types, no I/O -- reused by
//! `crate::equipment::hotbar::HotbarEquipmentService` and directly
//! unit-tested here.
//!
//! Deliberately does not invent a second tool-knowledge system: material
//! classification and tier both come straight from
//! `crate::interaction::tool_selection` (`category`, `tier`) -- the same
//! tables the pathfinder's mining-cost evaluation and `/mine`'s tool
//! selection already use. Only the *ranking policy* (rarity vs. a
//! durability-penalized score, and the replace-threshold) is new here, and
//! it mirrors `equipment::armor` exactly, down to reusing the same
//! `equipment::scoring::penalize_for_durability` formula.

use crate::{
    config::ToolRankingMode,
    equipment::{model::EquipmentItem, scoring::penalize_for_durability},
    interaction::tool_selection::{ToolCategory, category, tier},
};

/// One inventory candidate for a given `ToolCategory`, paired with its
/// parsed material tier (see `interaction::tool_selection::tier`: 1 = wood
/// .. 7 = netherite).
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub item: &'a EquipmentItem,
    pub tier: u8,
}

/// Maps a tool's material tier onto the same 1.0-10.0 scale
/// `armor::ArmorMaterial::base_score` uses, so `ToolRankingMode::Score`
/// reads on the same scale as armor's `score` mode. Copper (tier 4) has no
/// vanilla tool form; kept for completeness rather than special-cased.
fn base_score(tier: u8) -> f32 {
    match tier {
        1 => 1.0,  // wood
        2 => 2.0,  // gold
        3 => 3.0,  // stone
        4 => 4.0,  // copper
        5 => 6.0,  // iron
        6 => 8.5,  // diamond
        7 => 10.0, // netherite
        _ => 1.0,
    }
}

/// `Score` mode's rating for one tool: material tier sets the ceiling, a
/// durability penalty pulls it down -- a badly damaged Netherite pickaxe
/// can score below a pristine Diamond one.
pub fn rank_score(item: &EquipmentItem) -> f32 {
    penalize_for_durability(
        base_score(tier(&item.item_id)),
        item.current_durability,
        item.max_durability,
    )
}

/// Finds the best-ranked candidate of `category` among `inventory`, per
/// `mode`. Ties resolve toward whichever candidate is later in `inventory`
/// -- see `armor::best_candidate`'s doc comment for why that's harmless.
pub fn best_candidate<'a>(
    mode: ToolRankingMode,
    wanted: ToolCategory,
    inventory: &'a [EquipmentItem],
) -> Option<Candidate<'a>> {
    let mut best: Option<Candidate<'a>> = None;
    for item in inventory {
        if category(&item.item_id) != Some(wanted) {
            continue;
        }
        let candidate = Candidate {
            item,
            tier: tier(&item.item_id),
        };
        let better = match &best {
            None => true,
            Some(current) => match mode {
                ToolRankingMode::Rarity => candidate.tier >= current.tier,
                ToolRankingMode::Score => rank_score(candidate.item) >= rank_score(current.item),
            },
        };
        if better {
            best = Some(candidate);
        }
    }
    best
}

/// Whether `candidate` should replace whatever (if anything) is currently
/// held in that hotbar slot. `Rarity` requires a strictly higher material
/// tier; `Score` requires at least a 0.2-point improvement -- both exist
/// purely to stop the bot from swapping between two near-identical tools on
/// every re-evaluation. Mirrors `armor::should_replace` exactly.
pub fn should_replace(
    mode: ToolRankingMode,
    held: Option<&EquipmentItem>,
    candidate: Candidate,
) -> bool {
    match mode {
        ToolRankingMode::Rarity => {
            let held_tier = held.map_or(0, |item| tier(&item.item_id));
            candidate.tier > held_tier
        }
        ToolRankingMode::Score => {
            let held_score = held.map_or(0.0, rank_score);
            rank_score(candidate.item) >= held_score + 0.2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(slot: usize, id: &str, current: u32, max: u32) -> EquipmentItem {
        EquipmentItem {
            slot,
            item_id: id.into(),
            current_durability: current,
            max_durability: max,
        }
    }

    #[test]
    fn best_candidate_in_rarity_mode_prefers_higher_tier_regardless_of_durability() {
        let inventory = vec![
            item(10, "minecraft:netherite_pickaxe", 1, 100),
            item(11, "minecraft:diamond_pickaxe", 100, 100),
        ];
        let best =
            best_candidate(ToolRankingMode::Rarity, ToolCategory::Pickaxe, &inventory).unwrap();
        assert_eq!(best.item.slot, 10);
    }

    #[test]
    fn best_candidate_in_score_mode_can_prefer_a_pristine_lower_tier() {
        let inventory = vec![
            item(10, "minecraft:netherite_pickaxe", 1, 100), // ruined -> low score
            item(11, "minecraft:diamond_pickaxe", 100, 100), // full -> 8.5
        ];
        let best =
            best_candidate(ToolRankingMode::Score, ToolCategory::Pickaxe, &inventory).unwrap();
        assert_eq!(best.item.slot, 11);
    }

    #[test]
    fn best_candidate_ignores_the_wrong_category() {
        let inventory = vec![item(10, "minecraft:diamond_axe", 100, 100)];
        assert!(
            best_candidate(ToolRankingMode::Rarity, ToolCategory::Pickaxe, &inventory).is_none()
        );
    }

    #[test]
    fn should_replace_in_rarity_mode_requires_a_strictly_higher_tier() {
        let candidate_item = item(1, "minecraft:diamond_sword", 100, 100);
        let candidate = Candidate {
            item: &candidate_item,
            tier: tier("minecraft:diamond_sword"),
        };
        let held_same_tier = item(5, "minecraft:diamond_sword", 10, 100);
        assert!(!should_replace(
            ToolRankingMode::Rarity,
            Some(&held_same_tier),
            candidate
        ));
        let held_lower_tier = item(5, "minecraft:iron_sword", 100, 100);
        assert!(should_replace(
            ToolRankingMode::Rarity,
            Some(&held_lower_tier),
            candidate
        ));
        assert!(should_replace(ToolRankingMode::Rarity, None, candidate));
    }

    #[test]
    fn should_replace_in_score_mode_requires_a_meaningful_improvement() {
        let candidate_item = item(1, "minecraft:diamond_pickaxe", 100, 100); // 8.5
        let candidate = Candidate {
            item: &candidate_item,
            tier: tier("minecraft:diamond_pickaxe"),
        };
        let held_barely_lower = item(5, "minecraft:diamond_pickaxe", 99, 100); // ~8.44 -> 8.4
        assert!(!should_replace(
            ToolRankingMode::Score,
            Some(&held_barely_lower),
            candidate
        ));
        let held_much_lower = item(5, "minecraft:stone_pickaxe", 100, 100); // 3.0
        assert!(should_replace(
            ToolRankingMode::Score,
            Some(&held_much_lower),
            candidate
        ));
    }

    #[test]
    fn rank_score_matches_the_armor_scale_at_full_durability() {
        assert_eq!(
            rank_score(&item(0, "minecraft:netherite_axe", 100, 100)),
            10.0
        );
        assert_eq!(rank_score(&item(0, "minecraft:wooden_hoe", 100, 100)), 1.0);
    }
}
