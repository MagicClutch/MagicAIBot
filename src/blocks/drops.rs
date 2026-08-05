//! Static block -> primary-drop-item registry for `#get`'s block-gathering
//! path (see `App::run_get_block` in `src/app.rs`). Mining a block does not
//! always yield an item with the same registry id as the block itself
//! (`minecraft:iron_ore` drops `minecraft:raw_iron`, `minecraft:stone` drops
//! `minecraft:cobblestone`, `minecraft:deepslate_diamond_ore` drops
//! `minecraft:diamond`, ...) -- inventory must always be counted against the
//! drop, never the block, or a `#get` run loops forever waiting for a count
//! that can never increase.
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

/// `(block id, drop item id)`, both fully namespaced. Every block a mined
/// block converts to *itself* (dirt, oak_log, cobblestone once already
/// cobblestone, ...) is intentionally absent -- see `drop_item_for_block`.
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

/// The item actually obtained from mining `block_id` (already normalized,
/// e.g. `minecraft:iron_ore`). Blocks that drop themselves -- the
/// overwhelming majority -- are not listed in `BLOCK_DROPS` and fall
/// through to `block_id` unchanged, so this is always safe to call
/// unconditionally rather than only for a known ore whitelist.
pub fn drop_item_for_block(block_id: &str) -> &str {
    BLOCK_DROPS
        .iter()
        .find(|(block, _)| *block == block_id)
        .map_or(block_id, |(_, item)| *item)
}

/// Bare (non-namespaced) id for concise console output, e.g. `iron_ore` from
/// `minecraft:iron_ore`.
pub fn bare_id(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_ores_to_their_documented_drops() {
        assert_eq!(
            drop_item_for_block("minecraft:iron_ore"),
            "minecraft:raw_iron"
        );
        assert_eq!(
            drop_item_for_block("minecraft:deepslate_iron_ore"),
            "minecraft:raw_iron"
        );
        assert_eq!(drop_item_for_block("minecraft:coal_ore"), "minecraft:coal");
        assert_eq!(
            drop_item_for_block("minecraft:deepslate_coal_ore"),
            "minecraft:coal"
        );
        assert_eq!(
            drop_item_for_block("minecraft:diamond_ore"),
            "minecraft:diamond"
        );
        assert_eq!(
            drop_item_for_block("minecraft:deepslate_diamond_ore"),
            "minecraft:diamond"
        );
        assert_eq!(
            drop_item_for_block("minecraft:copper_ore"),
            "minecraft:raw_copper"
        );
        assert_eq!(
            drop_item_for_block("minecraft:gold_ore"),
            "minecraft:raw_gold"
        );
        assert_eq!(
            drop_item_for_block("minecraft:nether_gold_ore"),
            "minecraft:gold_nugget"
        );
        assert_eq!(
            drop_item_for_block("minecraft:emerald_ore"),
            "minecraft:emerald"
        );
        assert_eq!(
            drop_item_for_block("minecraft:lapis_ore"),
            "minecraft:lapis_lazuli"
        );
        assert_eq!(
            drop_item_for_block("minecraft:redstone_ore"),
            "minecraft:redstone"
        );
        assert_eq!(
            drop_item_for_block("minecraft:nether_quartz_ore"),
            "minecraft:quartz"
        );
    }

    #[test]
    fn resolves_non_ore_conversions() {
        assert_eq!(
            drop_item_for_block("minecraft:stone"),
            "minecraft:cobblestone"
        );
        assert_eq!(
            drop_item_for_block("minecraft:deepslate"),
            "minecraft:cobbled_deepslate"
        );
        assert_eq!(
            drop_item_for_block("minecraft:grass_block"),
            "minecraft:dirt"
        );
        assert_eq!(drop_item_for_block("minecraft:farmland"), "minecraft:dirt");
        assert_eq!(drop_item_for_block("minecraft:clay"), "minecraft:clay_ball");
        assert_eq!(
            drop_item_for_block("minecraft:glowstone"),
            "minecraft:glowstone_dust"
        );
        assert_eq!(drop_item_for_block("minecraft:carrots"), "minecraft:carrot");
        assert_eq!(
            drop_item_for_block("minecraft:potatoes"),
            "minecraft:potato"
        );
        assert_eq!(
            drop_item_for_block("minecraft:beetroots"),
            "minecraft:beetroot"
        );
        assert_eq!(
            drop_item_for_block("minecraft:cocoa"),
            "minecraft:cocoa_beans"
        );
    }

    #[test]
    fn blocks_that_drop_themselves_are_unaffected() {
        for block in [
            "minecraft:oak_log",
            "minecraft:cobblestone",
            "minecraft:dirt",
            "minecraft:sand",
            "minecraft:gravel",
            "minecraft:obsidian",
            "minecraft:diamond_block",
            "minecraft:andesite",
            "minecraft:ancient_debris",
        ] {
            assert_eq!(drop_item_for_block(block), block);
        }
    }

    #[test]
    fn bare_id_strips_the_namespace() {
        assert_eq!(bare_id("minecraft:iron_ore"), "iron_ore");
        assert_eq!(bare_id("iron_ore"), "iron_ore");
    }
}
