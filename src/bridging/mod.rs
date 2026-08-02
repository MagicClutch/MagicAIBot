//! Scaffold/building material selection for bridging, pillaring, and
//! staircasing: which of the bot's held blocks are safe to consume for
//! disposable construction, and in what order it should prefer them.
//!
//! Unlike an allow-list (only ever use these specific blocks), this is a
//! deny-list model: the bot may use *any* placeable block it currently
//! holds except the ones on [`SCAFFOLD_BLACKLIST`], preferring
//! [`PREFERRED_SCAFFOLD_BLOCKS`] when it has a choice.

use azalea::registry::builtin::BlockKind;

/// Blocks the bot must never use for bridging, pillaring, staircasing, or
/// any other disposable construction: containers, crafting/utility blocks,
/// storage/automation, redstone components, valuable blocks, ores, rare/
/// expensive blocks, spawn/progression blocks, decorative blocks, crops
/// and plants, leaves and saplings, light sources, and doors/gates/
/// trapdoors. Matched against the item's bare id (without the
/// `minecraft:` namespace) -- see [`is_blacklisted`].
pub const SCAFFOLD_BLACKLIST: &[&str] = &[
    // Containers
    "chest",
    "trapped_chest",
    "barrel",
    "shulker_box",
    "white_shulker_box",
    "orange_shulker_box",
    "magenta_shulker_box",
    "light_blue_shulker_box",
    "yellow_shulker_box",
    "lime_shulker_box",
    "pink_shulker_box",
    "gray_shulker_box",
    "light_gray_shulker_box",
    "cyan_shulker_box",
    "purple_shulker_box",
    "blue_shulker_box",
    "brown_shulker_box",
    "green_shulker_box",
    "red_shulker_box",
    "black_shulker_box",
    // Crafting / utility blocks
    "crafting_table",
    "furnace",
    "blast_furnace",
    "smoker",
    "cartography_table",
    "fletching_table",
    "smithing_table",
    "loom",
    "stonecutter",
    "grindstone",
    "anvil",
    "chipped_anvil",
    "damaged_anvil",
    // Storage / automation
    "hopper",
    "dropper",
    "dispenser",
    "observer",
    "piston",
    "sticky_piston",
    // Redstone components
    "redstone_block",
    "redstone_lamp",
    "redstone_torch",
    "repeater",
    "comparator",
    "target",
    "daylight_detector",
    "note_block",
    "lever",
    "stone_button",
    "oak_button",
    "spruce_button",
    "birch_button",
    "jungle_button",
    "acacia_button",
    "dark_oak_button",
    "mangrove_button",
    "cherry_button",
    "bamboo_button",
    "crimson_button",
    "warped_button",
    "stone_pressure_plate",
    "oak_pressure_plate",
    "spruce_pressure_plate",
    "birch_pressure_plate",
    "jungle_pressure_plate",
    "acacia_pressure_plate",
    "dark_oak_pressure_plate",
    "mangrove_pressure_plate",
    "cherry_pressure_plate",
    "bamboo_pressure_plate",
    "crimson_pressure_plate",
    "warped_pressure_plate",
    "tripwire_hook",
    // Valuable blocks
    "diamond_block",
    "emerald_block",
    "gold_block",
    "iron_block",
    "lapis_block",
    "netherite_block",
    "copper_block",
    "raw_iron_block",
    "raw_gold_block",
    "raw_copper_block",
    // Ore blocks
    "diamond_ore",
    "deepslate_diamond_ore",
    "emerald_ore",
    "deepslate_emerald_ore",
    "gold_ore",
    "deepslate_gold_ore",
    "iron_ore",
    "deepslate_iron_ore",
    "copper_ore",
    "deepslate_copper_ore",
    "lapis_ore",
    "deepslate_lapis_ore",
    "redstone_ore",
    "deepslate_redstone_ore",
    "coal_ore",
    "deepslate_coal_ore",
    "nether_gold_ore",
    "nether_quartz_ore",
    "ancient_debris",
    // Rare / expensive blocks
    "obsidian",
    "crying_obsidian",
    "respawn_anchor",
    "beacon",
    "enchanting_table",
    "ender_chest",
    "end_portal_frame",
    // Spawn / progression blocks
    "white_bed",
    "orange_bed",
    "magenta_bed",
    "light_blue_bed",
    "yellow_bed",
    "lime_bed",
    "pink_bed",
    "gray_bed",
    "light_gray_bed",
    "cyan_bed",
    "purple_bed",
    "blue_bed",
    "brown_bed",
    "green_bed",
    "red_bed",
    "black_bed",
    "lodestone",
    // Decorative blocks
    "glass",
    "glass_pane",
    "white_stained_glass",
    "orange_stained_glass",
    "magenta_stained_glass",
    "light_blue_stained_glass",
    "yellow_stained_glass",
    "lime_stained_glass",
    "pink_stained_glass",
    "gray_stained_glass",
    "light_gray_stained_glass",
    "cyan_stained_glass",
    "purple_stained_glass",
    "blue_stained_glass",
    "brown_stained_glass",
    "green_stained_glass",
    "red_stained_glass",
    "black_stained_glass",
    "white_stained_glass_pane",
    "orange_stained_glass_pane",
    "magenta_stained_glass_pane",
    "light_blue_stained_glass_pane",
    "yellow_stained_glass_pane",
    "lime_stained_glass_pane",
    "pink_stained_glass_pane",
    "gray_stained_glass_pane",
    "light_gray_stained_glass_pane",
    "cyan_stained_glass_pane",
    "purple_stained_glass_pane",
    "blue_stained_glass_pane",
    "brown_stained_glass_pane",
    "green_stained_glass_pane",
    "red_stained_glass_pane",
    "black_stained_glass_pane",
    // Crops and plants
    "wheat",
    "carrots",
    "potatoes",
    "beetroots",
    "melon",
    "pumpkin",
    "sugar_cane",
    "bamboo",
    "cactus",
    "kelp",
    "torchflower",
    "pitcher_crop",
    // Leaves and saplings
    "oak_leaves",
    "spruce_leaves",
    "birch_leaves",
    "jungle_leaves",
    "acacia_leaves",
    "dark_oak_leaves",
    "mangrove_leaves",
    "cherry_leaves",
    "azalea_leaves",
    "flowering_azalea_leaves",
    "oak_sapling",
    "spruce_sapling",
    "birch_sapling",
    "jungle_sapling",
    "acacia_sapling",
    "dark_oak_sapling",
    "mangrove_propagule",
    "cherry_sapling",
    "azalea",
    "flowering_azalea",
    // Light sources
    "torch",
    "soul_torch",
    "lantern",
    "soul_lantern",
    "sea_lantern",
    "glowstone",
    "shroomlight",
    "jack_o_lantern",
    // Doors / gates / trapdoors
    "oak_door",
    "spruce_door",
    "birch_door",
    "jungle_door",
    "acacia_door",
    "dark_oak_door",
    "mangrove_door",
    "cherry_door",
    "bamboo_door",
    "crimson_door",
    "warped_door",
    "iron_door",
    "oak_fence_gate",
    "spruce_fence_gate",
    "birch_fence_gate",
    "jungle_fence_gate",
    "acacia_fence_gate",
    "dark_oak_fence_gate",
    "mangrove_fence_gate",
    "cherry_fence_gate",
    "bamboo_fence_gate",
    "crimson_fence_gate",
    "warped_fence_gate",
    "oak_trapdoor",
    "spruce_trapdoor",
    "birch_trapdoor",
    "jungle_trapdoor",
    "acacia_trapdoor",
    "dark_oak_trapdoor",
    "mangrove_trapdoor",
    "cherry_trapdoor",
    "bamboo_trapdoor",
    "crimson_trapdoor",
    "warped_trapdoor",
    "iron_trapdoor",
];

/// Preference order for scaffold material: the first of these the bot
/// holds enough of wins over anything else it's carrying.
pub const PREFERRED_SCAFFOLD_BLOCKS: &[&str] = &[
    "dirt",
    "cobblestone",
    "stone",
    "cobbled_deepslate",
    "deepslate",
    "netherrack",
    "gravel",
    "sand",
];

/// Message logged when the bot has no eligible scaffold block at all (empty
/// hotbar, or everything held is blacklisted/not a block).
pub const NO_SAFE_SCAFFOLD_MESSAGE: &str = "No safe scaffold blocks available";

/// Strips a `minecraft:` namespace prefix, if present, so blacklist/
/// preferred-list lookups work regardless of which format the caller used.
fn bare_id(item_id: &str) -> &str {
    item_id.rsplit(':').next().unwrap_or(item_id)
}

/// Whether `item_id` (either bare, e.g. `"chest"`, or namespaced, e.g.
/// `"minecraft:chest"`) is on [`SCAFFOLD_BLACKLIST`].
#[must_use]
pub fn is_blacklisted(item_id: &str) -> bool {
    SCAFFOLD_BLACKLIST.contains(&bare_id(item_id))
}

/// Whether `item_id` actually names a placeable block (as opposed to a
/// tool, food, or other item with no block form). Delegates to the block
/// registry rather than hand-maintaining a second list, so it stays
/// correct for every item without needing to keep up with new tools/food/
/// misc items the bot might end up holding.
#[must_use]
pub fn is_placeable_block(item_id: &str) -> bool {
    item_id.parse::<BlockKind>().is_ok()
}

/// Builds a preference-ordered scaffold candidate list from everything the
/// bot currently holds: [`PREFERRED_SCAFFOLD_BLOCKS`] first (in their fixed
/// order, if held), then every other held item that's an actual placeable
/// block and isn't blacklisted, in the order given.
///
/// This is a deny-list model, not an allow-list: the bot can build with
/// whatever it has except the blacklisted blocks, rather than being
/// restricted to a small hardcoded set. Callers should treat an empty
/// result as "nothing safe to build with" (see [`NO_SAFE_SCAFFOLD_MESSAGE`])
/// -- never fall back to guessing with a blacklisted or non-block item.
#[must_use]
pub fn scaffold_allow_list<'a>(held_item_ids: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let held: Vec<&str> = held_item_ids.into_iter().collect();
    let mut ordered = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for preferred in PREFERRED_SCAFFOLD_BLOCKS {
        let id = format!("minecraft:{preferred}");
        if held.iter().any(|held_id| bare_id(held_id) == *preferred) && seen.insert(id.clone()) {
            ordered.push(id);
        }
    }
    for held_id in held {
        if is_blacklisted(held_id) || !is_placeable_block(held_id) {
            continue;
        }
        let id = if held_id.contains(':') {
            held_id.to_owned()
        } else {
            format!("minecraft:{held_id}")
        };
        if seen.insert(id.clone()) {
            ordered.push(id);
        }
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_blocks_come_first_in_fixed_order() {
        let held = ["minecraft:cobblestone", "minecraft:dirt", "minecraft:sand"];
        assert_eq!(
            scaffold_allow_list(held),
            vec![
                "minecraft:dirt".to_string(),
                "minecraft:cobblestone".to_string(),
                "minecraft:sand".to_string(),
            ]
        );
    }

    #[test]
    fn falls_back_to_any_other_held_non_blacklisted_block() {
        let held = ["minecraft:granite", "minecraft:andesite"];
        let result = scaffold_allow_list(held);
        assert_eq!(
            result,
            vec![
                "minecraft:granite".to_string(),
                "minecraft:andesite".to_string()
            ]
        );
    }

    #[test]
    fn blacklisted_blocks_are_never_included() {
        let held = [
            "minecraft:dirt",
            "minecraft:chest",
            "minecraft:diamond_block",
            "minecraft:oak_door",
        ];
        assert_eq!(
            scaffold_allow_list(held),
            vec!["minecraft:dirt".to_string()]
        );
    }

    #[test]
    fn non_block_items_are_excluded_without_needing_a_blacklist_entry() {
        let held = [
            "minecraft:dirt",
            "minecraft:diamond_pickaxe",
            "minecraft:bread",
            "minecraft:water_bucket",
        ];
        assert_eq!(
            scaffold_allow_list(held),
            vec!["minecraft:dirt".to_string()]
        );
    }

    #[test]
    fn empty_or_all_unusable_inventory_yields_empty_list() {
        assert!(scaffold_allow_list([]).is_empty());
        assert!(scaffold_allow_list(["minecraft:chest", "minecraft:iron_sword"]).is_empty());
    }

    #[test]
    fn is_blacklisted_matches_bare_and_namespaced_ids() {
        assert!(is_blacklisted("chest"));
        assert!(is_blacklisted("minecraft:chest"));
        assert!(!is_blacklisted("cobblestone"));
    }

    #[test]
    fn sample_of_every_blacklist_category_is_actually_blacklisted() {
        for id in [
            "shulker_box",
            "furnace",
            "hopper",
            "redstone_block",
            "diamond_block",
            "diamond_ore",
            "obsidian",
            "white_bed",
            "glass",
            "wheat",
            "oak_leaves",
            "torch",
            "oak_door",
        ] {
            assert!(is_blacklisted(id), "{id} should be blacklisted");
        }
    }
}
