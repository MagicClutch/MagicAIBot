//! Pure, deterministic tool knowledge and selection policy, plus a
//! synchronous candidate-extraction step from an inventory [`Menu`].
//!
//! This is the canonical implementation of the app's tool-selection
//! algorithm. It lives here (rather than in the downstream `magic_ai_bot`
//! crate) because [`MiningCache`](super::mining::MiningCache) needs it
//! synchronously, inside a Bevy system/background A* thread, and Rust's
//! crate-dependency direction only allows the downstream crate to depend on
//! this one, not the reverse. `magic_ai_bot::interaction::tool_selection` is
//! a thin re-export of the types below so there is exactly one algorithm.

use std::cmp::Ordering;

use azalea_block::BlockState;
use azalea_inventory::{
    ItemStack, Menu,
    components::{Damage, Enchantments, MaxDamage, Tool},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolCategory {
    Pickaxe,
    Axe,
    Shovel,
    Hoe,
    Shears,
    Sword,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolFallbackPolicy {
    /// A non-tool/hand may be used only when the block does not require a tool.
    AllowHand,
    /// Intentional breaking fails unless an acceptable tool is available.
    RequireSuitableTool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCandidate {
    pub hotbar_slot: u8,
    pub item_id: String,
    pub category: Option<ToolCategory>,
    pub tier: u8,
    pub correct_for_drops: bool,
    pub mining_speed: f32,
    pub remaining_durability: Option<u32>,
    pub efficiency_level: Option<u32>,
    pub protected: bool,
    pub reserved: bool,
    pub held: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockKnowledge {
    pub block_id: String,
    pub preferred_category: Option<ToolCategory>,
    pub requires_correct_tool: bool,
}

#[derive(Clone, Debug)]
pub struct ToolSelectionPolicy {
    pub minimum_remaining_durability: u32,
    pub fallback: ToolFallbackPolicy,
    /// Keep the held tool when its speed is within this fraction of the best.
    pub held_material_equivalence: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolSelection {
    pub hotbar_slot: u8,
    pub item_id: String,
    pub explanation: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoSuitableTool {
    pub block_id: String,
    pub explanation: String,
}

/// Select without side effects. Suitability/harvest ability dominates speed,
/// durability and enchantment tie-breaks. Protected and reserved tools are
/// never consumed by this policy.
pub fn select_tool(
    block: &BlockKnowledge,
    candidates: &[ToolCandidate],
    policy: &ToolSelectionPolicy,
) -> Result<ToolSelection, NoSuitableTool> {
    let mut usable: Vec<&ToolCandidate> = candidates
        .iter()
        .filter(|candidate| !candidate.protected && !candidate.reserved)
        .filter(|candidate| {
            candidate
                .remaining_durability
                .is_none_or(|remaining| remaining >= policy.minimum_remaining_durability)
        })
        .filter(|candidate| {
            candidate.category.is_some()
                && (!block.requires_correct_tool || candidate.correct_for_drops)
                && (candidate.correct_for_drops || candidate.category == block.preferred_category)
        })
        .collect();

    usable.sort_by(|left, right| compare(right, left, block));
    let Some(best) = usable.first().copied() else {
        if policy.fallback == ToolFallbackPolicy::AllowHand && !block.requires_correct_tool {
            if let Some(held) = candidates.iter().find(|candidate| candidate.held) {
                return Ok(ToolSelection {
                    hotbar_slot: held.hotbar_slot,
                    item_id: held.item_id.clone(),
                    explanation: format!(
                        "kept held item for {}; no acceptable specialized tool was required",
                        block.block_id
                    ),
                });
            }
        }
        let rejected_durability = candidates.iter().any(|candidate| {
            candidate
                .remaining_durability
                .is_some_and(|remaining| remaining < policy.minimum_remaining_durability)
        });
        return Err(NoSuitableTool {
            block_id: block.block_id.clone(),
            explanation: if rejected_durability {
                format!(
                    "no unprotected suitable tool has at least {} durability remaining",
                    policy.minimum_remaining_durability
                )
            } else {
                "no unprotected, unreserved tool can safely harvest this block".into()
            },
        });
    };

    let selected = usable
        .iter()
        .copied()
        .find(|candidate| candidate.held)
        .filter(|held| {
            held.correct_for_drops == best.correct_for_drops
                && held.mining_speed
                    >= best.mining_speed * (1.0 - policy.held_material_equivalence.clamp(0.0, 1.0))
        })
        .unwrap_or(best);
    Ok(ToolSelection {
        hotbar_slot: selected.hotbar_slot,
        item_id: selected.item_id.clone(),
        explanation: format!(
            "{} slot {} for {}: category {:?}, tier {}, speed {:.2}, durability {}, efficiency {}{}",
            if selected.held {
                "kept held"
            } else {
                "selected"
            },
            selected.hotbar_slot + 1,
            block.block_id,
            selected.category,
            selected.tier,
            selected.mining_speed,
            selected
                .remaining_durability
                .map_or_else(|| "unavailable".into(), |v| v.to_string()),
            selected
                .efficiency_level
                .map_or_else(|| "unavailable".into(), |v| v.to_string()),
            if selected.correct_for_drops {
                ", harvest-capable"
            } else {
                ""
            }
        ),
    })
}

fn compare(a: &ToolCandidate, b: &ToolCandidate, block: &BlockKnowledge) -> Ordering {
    a.correct_for_drops
        .cmp(&b.correct_for_drops)
        .then_with(|| {
            (a.category == block.preferred_category).cmp(&(b.category == block.preferred_category))
        })
        .then_with(|| a.mining_speed.total_cmp(&b.mining_speed))
        .then(a.efficiency_level.cmp(&b.efficiency_level))
        .then(a.remaining_durability.cmp(&b.remaining_durability))
        .then(a.tier.cmp(&b.tier))
        .then(a.held.cmp(&b.held))
        .then_with(|| b.hotbar_slot.cmp(&a.hotbar_slot))
}

pub fn category(item: &str) -> Option<ToolCategory> {
    [
        ("_pickaxe", ToolCategory::Pickaxe),
        ("_shovel", ToolCategory::Shovel),
        ("_axe", ToolCategory::Axe),
        ("_hoe", ToolCategory::Hoe),
        ("_sword", ToolCategory::Sword),
    ]
    .into_iter()
    .find_map(|(suffix, category)| item.ends_with(suffix).then_some(category))
    .or_else(|| item.ends_with("shears").then_some(ToolCategory::Shears))
}

pub fn tier(item: &str) -> u8 {
    [
        "wooden_",
        "golden_",
        "stone_",
        "copper_",
        "iron_",
        "diamond_",
        "netherite_",
    ]
    .iter()
    .position(|tier| item.contains(tier))
    .map_or(0, |index| index as u8 + 1)
}

pub fn preferred_category(id: &str) -> Option<ToolCategory> {
    if ["wool", "vine", "lichen", "tripwire"]
        .iter()
        .any(|v| id.contains(v))
    {
        Some(ToolCategory::Shears)
    } else if [
        "log", "wood", "planks", "stem", "hyphae", "chest", "barrel", "fence", "door", "sign",
        "bamboo",
    ]
    .iter()
    .any(|v| id.contains(v))
    {
        Some(ToolCategory::Axe)
    } else if [
        "dirt",
        "grass_block",
        "sand",
        "gravel",
        "clay",
        "snow",
        "soul_",
        "mud",
    ]
    .iter()
    .any(|v| id.contains(v))
    {
        Some(ToolCategory::Shovel)
    } else if [
        "leaves",
        "hay_block",
        "moss",
        "sculk",
        "wart_block",
        "sponge",
    ]
    .iter()
    .any(|v| id.contains(v))
    {
        Some(ToolCategory::Hoe)
    } else if id.ends_with("cobweb") {
        Some(ToolCategory::Sword)
    } else {
        Some(ToolCategory::Pickaxe)
    }
}

/// Synchronously builds the candidate list for `block` from every present
/// hotbar slot in `menu`, mirroring what the app's async
/// `MinecraftClient::select_tool_for_block` does over the network-backed
/// client, but reading straight from an already-fetched [`Menu`] snapshot so
/// it can run inside the pathfinder's background A* thread / Bevy systems.
/// `protected`/`reserved` item ids and `requires_correct_tool` come from the
/// caller since neither is derivable from `Menu` alone.
pub fn candidates_for_block(
    menu: &Menu,
    block: BlockState,
    requires_correct_tool: bool,
    current_hotbar_slot: u8,
    protected_tools: &[String],
    reserved_tools: &[String],
) -> Vec<ToolCandidate> {
    let menu_slots = menu.slots();
    menu.hotbar_slots_range()
        .enumerate()
        .filter_map(|(hotbar_slot, menu_slot)| {
            let item = menu_slots.get(menu_slot)?;
            if item.is_empty() {
                return None;
            }
            let id = item.kind().to_string();
            let tool = item.get_component::<Tool>();
            let (speed, correct) = tool.as_ref().map_or((1.0, !requires_correct_tool), |tool| {
                let rule = tool
                    .rules
                    .iter()
                    .find(|rule| rule.blocks.contains(block.to_trait().as_block_kind()));
                (
                    rule.and_then(|rule| rule.speed)
                        .unwrap_or(tool.default_mining_speed),
                    rule.and_then(|rule| rule.correct_for_drops)
                        .unwrap_or(!requires_correct_tool),
                )
            });
            let remaining = item.get_component::<MaxDamage>().map(|maximum| {
                let damage = item
                    .get_component::<Damage>()
                    .map_or(0, |damage| damage.amount);
                maximum.amount.saturating_sub(damage).max(0) as u32
            });
            let efficiency = item
                .get_component::<Enchantments>()
                .and_then(|enchantments| {
                    enchantments.levels.iter().find_map(|(kind, level)| {
                        format!("{kind:?}")
                            .to_ascii_lowercase()
                            .contains("efficiency")
                            .then_some((*level).max(0) as u32)
                    })
                });
            Some(ToolCandidate {
                hotbar_slot: hotbar_slot as u8,
                item_id: id.clone(),
                category: category(&id),
                tier: tier(&id),
                correct_for_drops: correct,
                mining_speed: speed,
                remaining_durability: remaining,
                efficiency_level: efficiency,
                protected: protected_tools.iter().any(|p| p == &id),
                reserved: reserved_tools.iter().any(|r| r == &id),
                held: hotbar_slot as u8 == current_hotbar_slot,
            })
        })
        .collect()
}

/// Convenience: find the first hotbar slot holding an item whose id matches
/// `predicate`, without any of the tool-ranking machinery above. Used by
/// placement moves selecting a scaffold block rather than a tool.
pub fn find_hotbar_item(menu: &Menu, predicate: impl Fn(&ItemStack) -> bool) -> Option<u8> {
    let menu_slots = menu.slots();
    menu.hotbar_slots_range()
        .enumerate()
        .find_map(|(hotbar_slot, menu_slot)| {
            let item = menu_slots.get(menu_slot)?;
            (!item.is_empty() && predicate(item)).then_some(hotbar_slot as u8)
        })
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
    fn exhaustive_table_driven_selection() {
        struct Case {
            name: &'static str,
            candidates: Vec<ToolCandidate>,
            expected: Result<u8, &'static str>,
        }
        let mut held_near = candidate(0, "minecraft:iron_pickaxe", 7.2);
        held_near.held = true;
        let mut held_slow = candidate(0, "minecraft:iron_pickaxe", 5.0);
        held_slow.held = true;
        let diamond = candidate(1, "minecraft:diamond_pickaxe", 8.0);
        let mut fragile = diamond.clone();
        fragile.remaining_durability = Some(1);
        let mut protected = diamond.clone();
        protected.protected = true;
        let mut reserved = diamond.clone();
        reserved.reserved = true;
        let mut wrong = candidate(2, "minecraft:diamond_axe", 20.0);
        wrong.correct_for_drops = false;
        for case in [
            Case {
                name: "fastest",
                candidates: vec![candidate(0, "minecraft:iron_pickaxe", 6.0), diamond.clone()],
                expected: Ok(1),
            },
            Case {
                name: "held materially equivalent",
                candidates: vec![held_near, diamond.clone()],
                expected: Ok(0),
            },
            Case {
                name: "material improvement switches",
                candidates: vec![held_slow, diamond.clone()],
                expected: Ok(1),
            },
            Case {
                name: "fragile rejected",
                candidates: vec![fragile],
                expected: Err("durability"),
            },
            Case {
                name: "protected rejected",
                candidates: vec![protected],
                expected: Err("unprotected"),
            },
            Case {
                name: "reserved rejected",
                candidates: vec![reserved],
                expected: Err("unprotected"),
            },
            Case {
                name: "wrong category and harvest",
                candidates: vec![wrong],
                expected: Err("harvest"),
            },
            Case {
                name: "empty",
                candidates: vec![],
                expected: Err("harvest"),
            },
        ] {
            let result = select_tool(&block(true), &case.candidates, &policy());
            match case.expected {
                Ok(slot) => assert_eq!(result.unwrap().hotbar_slot, slot, "{}", case.name),
                Err(fragment) => assert!(
                    result.unwrap_err().explanation.contains(fragment),
                    "{}",
                    case.name
                ),
            }
        }
    }

    #[test]
    fn fallback_is_explicit_and_keeps_hand() {
        let mut hand = candidate(4, "minecraft:torch", 1.0);
        hand.category = None;
        hand.correct_for_drops = false;
        hand.held = true;
        hand.remaining_durability = None;
        let mut p = policy();
        p.fallback = ToolFallbackPolicy::AllowHand;
        assert_eq!(
            select_tool(&block(false), &[hand], &p).unwrap().hotbar_slot,
            4
        );
    }
}
