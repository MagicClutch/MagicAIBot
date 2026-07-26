//! Deterministic, hotbar-only tool selection for block breaking.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tool {
    Pickaxe,
    Axe,
    Shovel,
    Hoe,
    Shears,
    Sword,
}

/// Return a score for a tool and block. Zero means that switching to the item
/// would not be useful. The tier is deliberately secondary to suitability.
pub(crate) fn score(item: &str, block: &str) -> u16 {
    let Some(tool) = tool(item) else { return 0 };
    let suitable = match tool {
        Tool::Pickaxe => is_pickaxe_block(block),
        Tool::Axe => is_axe_block(block),
        Tool::Shovel => is_shovel_block(block),
        Tool::Hoe => is_hoe_block(block),
        Tool::Shears => is_shears_block(block),
        Tool::Sword => block.ends_with("cobweb"),
    };
    if !suitable {
        return 0;
    }
    let special = matches!(tool, Tool::Shears | Tool::Sword) as u16;
    100 + special * 20 + tier(item)
}

fn tool(item: &str) -> Option<Tool> {
    [
        ("_pickaxe", Tool::Pickaxe),
        ("_shovel", Tool::Shovel),
        ("_axe", Tool::Axe),
        ("_hoe", Tool::Hoe),
        ("_sword", Tool::Sword),
    ]
    .into_iter()
    .find_map(|(suffix, tool)| item.ends_with(suffix).then_some(tool))
    .or_else(|| item.ends_with("shears").then_some(Tool::Shears))
}

fn tier(item: &str) -> u16 {
    if item.contains("netherite_") {
        6
    } else if item.contains("diamond_") {
        5
    } else if item.contains("golden_") {
        4
    } else if item.contains("iron_") {
        3
    } else if item.contains("stone_") {
        2
    } else if item.contains("wooden_") {
        1
    } else {
        0
    }
}

fn is_pickaxe_block(id: &str) -> bool {
    [
        "stone",
        "ore",
        "deepslate",
        "cobblestone",
        "brick",
        "terracotta",
        "concrete",
        "obsidian",
        "anvil",
        "rail",
        "lantern",
        "cauldron",
        "hopper",
        "furnace",
    ]
    .iter()
    .any(|part| id.contains(part))
}
fn is_axe_block(id: &str) -> bool {
    [
        "log",
        "wood",
        "planks",
        "stem",
        "hyphae",
        "bookshelf",
        "chest",
        "barrel",
        "crafting_table",
        "fence",
        "door",
        "trapdoor",
        "sign",
        "bamboo",
    ]
    .iter()
    .any(|part| id.contains(part))
}
fn is_shovel_block(id: &str) -> bool {
    [
        "dirt",
        "grass_block",
        "sand",
        "gravel",
        "clay",
        "snow",
        "soul_sand",
        "soul_soil",
        "mud",
    ]
    .iter()
    .any(|part| id.contains(part))
}
fn is_hoe_block(id: &str) -> bool {
    [
        "leaves",
        "hay_block",
        "moss",
        "sculk",
        "wart_block",
        "sponge",
    ]
    .iter()
    .any(|part| id.contains(part))
}
fn is_shears_block(id: &str) -> bool {
    [
        "leaves",
        "wool",
        "vine",
        "glow_lichen",
        "tripwire",
        "cobweb",
    ]
    .iter()
    .any(|part| id.contains(part))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_suitable_tools_by_tier() {
        assert!(
            score("minecraft:diamond_pickaxe", "minecraft:stone")
                > score("minecraft:iron_pickaxe", "minecraft:stone")
        );
        assert_eq!(score("minecraft:diamond_axe", "minecraft:stone"), 0);
    }

    #[test]
    fn recognizes_each_tool_family() {
        assert!(score("minecraft:iron_axe", "minecraft:oak_log") > 0);
        assert!(score("minecraft:iron_shovel", "minecraft:dirt") > 0);
        assert!(score("minecraft:iron_hoe", "minecraft:sculk") > 0);
        assert!(score("minecraft:shears", "minecraft:white_wool") > 0);
        assert!(score("minecraft:iron_sword", "minecraft:cobweb") > 0);
    }
}
