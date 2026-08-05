//! Static block -> primary-drop-item registry backing `#get`'s item
//! resolution (see `mobs::resolve_resource` and `App::run_get_item` in
//! `src/app.rs`). Mining a block does not always yield an item with the
//! same registry id as the block itself (`minecraft:iron_ore` drops
//! `minecraft:raw_iron`, `minecraft:stone` drops `minecraft:cobblestone`,
//! `minecraft:deepslate_diamond_ore` drops `minecraft:diamond`, ...) --
//! `#get` needs the reverse direction, item -> every block that produces
//! it, so it can mine whichever candidate is nearer instead of hardcoding a
//! single source per item.
//!
//! There is no loot-table data bundled in Azalea to resolve this from:
//! vanilla loot tables are server-side data-pack JSON, not part of the
//! client protocol or any registry this bot has access to (confirmed by
//! searching the vendored crate -- `azalea_registry`'s `LootTable`-adjacent
//! entries are just resource-location *name* enums, not the actual
//! block-to-drop tables). This is therefore a hand-maintained fallback,
//! covering every vanilla block whose single, deterministic primary drop
//! differs from the block itself. Blocks whose drop is only *sometimes*
//! different (gravel's flint chance, gilded_blackstone's gold_nugget
//! chance, grass/leaves dropping seeds/sticks) are deliberately left out --
//! their most likely/primary drop still matches the block, which is exactly
//! what "not listed here" already defaults to. Extending coverage means
//! adding a row to `BLOCK_DROPS`; nothing else needs to change.

/// `(block id, drop item id)`, both fully namespaced. Every block that
/// simply drops itself (dirt, oak_log, cobblestone, ...) is intentionally
/// absent -- see `drop_blocks_for_item`.
const BLOCK_DROPS: &[(&str, &str)] = &[
    // Overworld ores -> raw materials or processed items.
    ("minecraft:iron_ore", "minecraft:raw_iron"),
    ("minecraft:deepslate_iron_ore", "minecraft:raw_iron"),
    ("minecraft:copper_ore", "minecraft:raw_copper"),
    ("minecraft:deepslate_copper_ore", "minecraft:raw_copper"),
    ("minecraft:gold_ore", "minecraft:raw_gold"),
    ("minecraft:deepslate_gold_ore", "minecraft:raw_gold"),
    ("minecraft:nether_gold_ore", "minecraft:gold_nugget"),
    ("minecraft:coal_ore", "minecraft:coal"),
    ("minecraft:deepslate_coal_ore", "minecraft:coal"),
    ("minecraft:diamond_ore", "minecraft:diamond"),
    ("minecraft:deepslate_diamond_ore", "minecraft:diamond"),
    ("minecraft:emerald_ore", "minecraft:emerald"),
    ("minecraft:deepslate_emerald_ore", "minecraft:emerald"),
    ("minecraft:lapis_ore", "minecraft:lapis_lazuli"),
    ("minecraft:deepslate_lapis_ore", "minecraft:lapis_lazuli"),
    ("minecraft:redstone_ore", "minecraft:redstone"),
    ("minecraft:deepslate_redstone_ore", "minecraft:redstone"),
    ("minecraft:nether_quartz_ore", "minecraft:quartz"),
    // Stone family: only the plain forms convert on break without silk
    // touch; polished/other variants (andesite, diorite, granite, ...) drop
    // themselves and are intentionally not listed.
    ("minecraft:stone", "minecraft:cobblestone"),
    ("minecraft:deepslate", "minecraft:cobbled_deepslate"),
    // Dirt-like blocks that drop plain dirt.
    ("minecraft:grass_block", "minecraft:dirt"),
    ("minecraft:podzol", "minecraft:dirt"),
    ("minecraft:mycelium", "minecraft:dirt"),
    ("minecraft:farmland", "minecraft:dirt"),
    ("minecraft:dirt_path", "minecraft:dirt"),
    // Misc single-item conversions.
    ("minecraft:clay", "minecraft:clay_ball"),
    ("minecraft:glowstone", "minecraft:glowstone_dust"),
    ("minecraft:melon", "minecraft:melon_slice"),
    ("minecraft:sea_lantern", "minecraft:prismarine_crystals"),
    ("minecraft:amethyst_cluster", "minecraft:amethyst_shard"),
    // Crops/plants whose block id doesn't match the harvested item id.
    ("minecraft:carrots", "minecraft:carrot"),
    ("minecraft:potatoes", "minecraft:potato"),
    ("minecraft:beetroots", "minecraft:beetroot"),
    ("minecraft:cocoa", "minecraft:cocoa_beans"),
    ("minecraft:sweet_berry_bush", "minecraft:sweet_berries"),
    ("minecraft:torchflower_crop", "minecraft:torchflower"),
    ("minecraft:pitcher_crop", "minecraft:pitcher_pod"),
    ("minecraft:kelp_plant", "minecraft:kelp"),
    ("minecraft:twisting_vines_plant", "minecraft:twisting_vines"),
    ("minecraft:weeping_vines_plant", "minecraft:weeping_vines"),
];

/// All blocks whose primary drop is `item_id` (already normalized, e.g.
/// `minecraft:diamond`). A single item can map back to multiple source
/// blocks (`minecraft:diamond` comes from both `minecraft:diamond_ore` and
/// `minecraft:deepslate_diamond_ore`), which is exactly what `#get` needs:
/// mine whichever candidate is nearer. Empty means no block in
/// `BLOCK_DROPS` produces this item as its differing drop -- callers fall
/// back to treating `item_id` as a block id directly (see
/// `mobs::resolve_resource`), which is correct for any block that simply
/// drops itself (logs, dirt, ancient_debris, ...).
pub fn drop_blocks_for_item(item_id: &str) -> Vec<&'static str> {
    BLOCK_DROPS
        .iter()
        .filter(|(_, item)| *item == item_id)
        .map(|(block, _)| *block)
        .collect()
}

/// Bare (non-namespaced) id for concise console output, e.g. `iron_ore` from
/// `minecraft:iron_ore`.
pub fn bare_id(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut blocks: Vec<&'static str>) -> Vec<&'static str> {
        blocks.sort_unstable();
        blocks
    }

    #[test]
    fn resolves_item_to_every_ore_source_block() {
        assert_eq!(
            sorted(drop_blocks_for_item("minecraft:diamond")),
            vec!["minecraft:deepslate_diamond_ore", "minecraft:diamond_ore"]
        );
        assert_eq!(
            sorted(drop_blocks_for_item("minecraft:raw_iron")),
            vec!["minecraft:deepslate_iron_ore", "minecraft:iron_ore"]
        );
        assert_eq!(
            sorted(drop_blocks_for_item("minecraft:coal")),
            vec!["minecraft:coal_ore", "minecraft:deepslate_coal_ore"]
        );
        assert_eq!(
            sorted(drop_blocks_for_item("minecraft:raw_copper")),
            vec!["minecraft:copper_ore", "minecraft:deepslate_copper_ore"]
        );
        assert_eq!(
            sorted(drop_blocks_for_item("minecraft:raw_gold")),
            vec!["minecraft:deepslate_gold_ore", "minecraft:gold_ore"]
        );
        assert_eq!(
            sorted(drop_blocks_for_item("minecraft:emerald")),
            vec!["minecraft:deepslate_emerald_ore", "minecraft:emerald_ore"]
        );
        assert_eq!(
            sorted(drop_blocks_for_item("minecraft:lapis_lazuli")),
            vec!["minecraft:deepslate_lapis_ore", "minecraft:lapis_ore"]
        );
        assert_eq!(
            sorted(drop_blocks_for_item("minecraft:redstone")),
            vec!["minecraft:deepslate_redstone_ore", "minecraft:redstone_ore"]
        );
    }

    #[test]
    fn resolves_non_ore_conversions() {
        assert_eq!(
            drop_blocks_for_item("minecraft:cobblestone"),
            vec!["minecraft:stone"]
        );
        assert_eq!(
            drop_blocks_for_item("minecraft:cobbled_deepslate"),
            vec!["minecraft:deepslate"]
        );
        assert_eq!(
            sorted(drop_blocks_for_item("minecraft:dirt")),
            vec![
                "minecraft:dirt_path",
                "minecraft:farmland",
                "minecraft:grass_block",
                "minecraft:mycelium",
                "minecraft:podzol",
            ]
        );
        assert_eq!(
            drop_blocks_for_item("minecraft:clay_ball"),
            vec!["minecraft:clay"]
        );
        assert_eq!(
            drop_blocks_for_item("minecraft:glowstone_dust"),
            vec!["minecraft:glowstone"]
        );
        assert_eq!(
            drop_blocks_for_item("minecraft:carrot"),
            vec!["minecraft:carrots"]
        );
        assert_eq!(
            drop_blocks_for_item("minecraft:potato"),
            vec!["minecraft:potatoes"]
        );
        assert_eq!(
            drop_blocks_for_item("minecraft:beetroot"),
            vec!["minecraft:beetroots"]
        );
        assert_eq!(
            drop_blocks_for_item("minecraft:cocoa_beans"),
            vec!["minecraft:cocoa"]
        );
    }

    #[test]
    fn item_with_a_single_source_block_resolves_to_one() {
        assert_eq!(
            drop_blocks_for_item("minecraft:quartz"),
            vec!["minecraft:nether_quartz_ore"]
        );
        assert_eq!(
            drop_blocks_for_item("minecraft:gold_nugget"),
            vec!["minecraft:nether_gold_ore"]
        );
    }

    #[test]
    fn item_with_no_ore_source_resolves_to_nothing() {
        // Falls back to "treat the item id as a block id directly" one level
        // up in `mobs::resolve_resource` -- correct for self-dropping blocks
        // like logs, and for genuine non-ore items like `ancient_debris`
        // (the block drops the identically-named item; there is no *other*
        // block that produces it).
        assert!(drop_blocks_for_item("minecraft:oak_log").is_empty());
        assert!(drop_blocks_for_item("minecraft:ancient_debris").is_empty());
        assert!(drop_blocks_for_item("minecraft:stick").is_empty());
    }

    #[test]
    fn bare_id_strips_the_namespace() {
        assert_eq!(bare_id("minecraft:iron_ore"), "iron_ore");
        assert_eq!(bare_id("iron_ore"), "iron_ore");
    }
}
