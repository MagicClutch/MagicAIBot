//! Extends `#get` to also support mob drops and ore/conversion sources,
//! without touching the block-mining primitives themselves (still
//! `BlockNavigationService`/`InteractionController`, driven from
//! `App::run_get_item` in `src/app.rs`). [`resolve_resource`] is the single
//! decision point for "how is this item obtained" -- used both to validate
//! `#get`'s argument at parse time (`console::commands::parse_get`) and to
//! choose which gathering path `App::run_get_resource` dispatches to.
//!
//! `#get` is deliberately item-first: the argument names what the caller
//! wants to *end up holding*, never a block to mine directly (that's
//! `/mine`'s job -- see `console::commands::parse_mine` and
//! `App::run_mine`). Every path here still resolves to a concrete list of
//! source blocks or a mob before anything happens; `#get` never mines "the
//! item" itself.

pub mod combat;
pub mod drops;

pub use combat::{CombatController, CombatState};
pub use drops::{mob_for_resource, mob_label};

use crate::{
    blocks::{block_query::normalize_block_id, drop_blocks_for_item},
    error::AppError,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceKind {
    /// `resource_id` (the item actually counted in inventory) is produced by
    /// mining any of `blocks` -- always at least one entry, sorted for
    /// deterministic comparisons. A self-dropping block (`oak_log`)
    /// resolves to `blocks == [resource_id]`; an ore/conversion item
    /// (`diamond`) resolves to every documented source
    /// (`diamond_ore`, `deepslate_diamond_ore`), *plus* its own block form
    /// too when that's independently valid (`cobblestone` searches existing
    /// `cobblestone` blocks as well as `stone`, since mining stone converts
    /// to cobblestone -- only searching the conversion source would miss
    /// already-placed cobblestone entirely).
    Ore {
        resource_id: String,
        blocks: Vec<String>,
    },
    /// `resource_id` (the item actually counted in inventory) is obtained by
    /// killing `mob_id`.
    Mob { resource_id: String, mob_id: String },
}

/// Normalizes `input` the same way block identifiers are (trim, lowercase,
/// default `minecraft:` namespace, single-colon vanilla-namespace syntax)
/// without requiring it to be a *block* -- `#get`'s argument may equally be
/// an item obtained only from ore (`raw_iron`) or only from a mob
/// (`leather`).
///
/// Resolution order is deliberate and matters where sources overlap:
/// 1. **Ore/conversion table** (`blocks::drop_blocks_for_item`) -- checked
///    first. If the item is *also* independently a valid block (self-drop),
///    that block is folded into the same candidate list rather than
///    competing with it (see `Ore`'s doc comment) -- e.g. `dirt` searches
///    plain `dirt` *and* `grass_block`/`farmland`/`podzol`/`mycelium`/
///    `dirt_path`, not just the conversions.
/// 2. **Mob-drop table** (`drops::mob_for_resource`) -- only reached if step
///    1 found nothing. A few items are both rare mob drops *and* common
///    ore/crop products (`redstone`, `carrot`, `potato`, `glowstone_dust`);
///    mining/farming the block is the practical, intended source, so it
///    wins whenever step 1 matches at all.
/// 3. **Plain block fallback** -- if step 1 found nothing at all (not even
///    the item's own block form) and step 2 found no mob either, the
///    identifier is unknown. Note this means a mob-drop item that also
///    happens to be an unrelated placeable block (the wool colors) still
///    resolves to farming the mob: step 1 is empty for wool (no block
///    *converts* into it), so step 2 is reached before any block-identity
///    fallback would apply.
pub fn resolve_resource(input: &str) -> Result<ResourceKind, AppError> {
    let normalized = normalize_item_syntax(input)?;
    let mut ore_blocks: Vec<String> = drop_blocks_for_item(&normalized)
        .into_iter()
        .map(str::to_owned)
        .collect();
    if !ore_blocks.is_empty() {
        if let Ok(block_id) = normalize_block_id(input)
            && !ore_blocks.iter().any(|existing| existing == &block_id)
        {
            ore_blocks.push(block_id);
        }
        return Ok(ResourceKind::Ore {
            resource_id: normalized,
            blocks: ore_blocks,
        });
    }
    if let Some(mob_id) = mob_for_resource(&normalized) {
        return Ok(ResourceKind::Mob {
            resource_id: normalized,
            mob_id: mob_id.to_owned(),
        });
    }
    match normalize_block_id(input) {
        Ok(block_id) => Ok(ResourceKind::Ore {
            resource_id: normalized,
            blocks: vec![block_id],
        }),
        Err(_) => Err(AppError::UnknownResourceIdentifier(normalized)),
    }
}

/// The same structural validation `normalize_block_id` applies (trim,
/// lowercase, single `minecraft:` namespace, allowed charset) without the
/// final block-registry check -- the caller already tried that path via
/// `normalize_block_id` itself.
fn normalize_item_syntax(input: &str) -> Result<String, AppError> {
    let input = input.trim().to_ascii_lowercase();
    if input.is_empty() || input.chars().any(char::is_whitespace) {
        return Err(AppError::InvalidBlockIdentifier(
            "resource identifier must not be empty or contain spaces".into(),
        ));
    }
    let normalized = if input.contains(':') {
        input
    } else {
        format!("minecraft:{input}")
    };
    let Some((namespace, path)) = normalized.split_once(':') else {
        return Err(AppError::InvalidBlockIdentifier(
            "resource identifier must contain a valid namespace and path".into(),
        ));
    };
    if namespace != "minecraft"
        || path.is_empty()
        || normalized.matches(':').count() != 1
        || !crate::blocks::block_query::valid_identifier_part(namespace, true)
        || !crate::blocks::block_query::valid_identifier_part(path, false)
    {
        return Err(AppError::InvalidBlockIdentifier(format!(
            "malformed resource identifier: {normalized}"
        )));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ore_blocks(input: &str) -> Vec<String> {
        let Ok(ResourceKind::Ore { mut blocks, .. }) = resolve_resource(input) else {
            panic!("expected an Ore resolution for {input}");
        };
        blocks.sort_unstable();
        blocks
    }

    #[test]
    fn resolves_self_dropping_blocks_to_a_single_source() {
        assert_eq!(
            resolve_resource("oak_log").unwrap(),
            ResourceKind::Ore {
                resource_id: "minecraft:oak_log".into(),
                blocks: vec!["minecraft:oak_log".into()],
            }
        );
        assert_eq!(
            resolve_resource("minecraft:sand").unwrap(),
            ResourceKind::Ore {
                resource_id: "minecraft:sand".into(),
                blocks: vec!["minecraft:sand".into()],
            }
        );
    }

    #[test]
    fn resolves_ores_to_every_source_block() {
        assert_eq!(
            ore_blocks("diamond"),
            vec!["minecraft:deepslate_diamond_ore", "minecraft:diamond_ore"]
        );
        assert_eq!(
            ore_blocks("raw_iron"),
            vec!["minecraft:deepslate_iron_ore", "minecraft:iron_ore"]
        );
        assert_eq!(
            ore_blocks("coal"),
            vec!["minecraft:coal_ore", "minecraft:deepslate_coal_ore"]
        );
    }

    #[test]
    fn conversion_items_also_search_their_own_direct_block_form() {
        // `cobblestone` is both a real block and `stone`'s converted drop;
        // both must be searched, or existing cobblestone lying around would
        // never be picked up.
        assert_eq!(
            ore_blocks("cobblestone"),
            vec!["minecraft:cobblestone", "minecraft:stone"]
        );
        // `dirt` similarly: plain dirt, plus every block that converts to it.
        assert_eq!(
            ore_blocks("dirt"),
            vec![
                "minecraft:dirt",
                "minecraft:dirt_path",
                "minecraft:farmland",
                "minecraft:grass_block",
                "minecraft:mycelium",
                "minecraft:podzol",
            ]
        );
    }

    #[test]
    fn resolves_mob_drops_when_not_ore_or_a_block() {
        assert_eq!(
            resolve_resource("leather").unwrap(),
            ResourceKind::Mob {
                resource_id: "minecraft:leather".into(),
                mob_id: "minecraft:cow".into(),
            }
        );
        assert_eq!(
            resolve_resource("PORKCHOP").unwrap(),
            ResourceKind::Mob {
                resource_id: "minecraft:porkchop".into(),
                mob_id: "minecraft:pig".into(),
            }
        );
    }

    #[test]
    fn ore_and_crop_sources_win_over_a_rare_mob_drop_of_the_same_item() {
        // `redstone`, `carrot`, `potato`, and `glowstone_dust` are all rare
        // mob drops (witch/zombie) *and* the item produced by their ore/crop
        // block -- mining/farming must win since it's the practical source.
        for item in ["redstone", "carrot", "potato", "glowstone_dust"] {
            assert!(
                matches!(resolve_resource(item).unwrap(), ResourceKind::Ore { .. }),
                "{item} should resolve as Ore, not a mob drop"
            );
        }
    }

    #[test]
    fn a_wool_color_block_still_resolves_to_sheep_farming() {
        // `white_wool` is a real, placeable block, but no block *converts*
        // into it (empty ore-table result), so mob resolution is reached --
        // and that's the intended source, since wool isn't naturally
        // generated.
        assert_eq!(
            resolve_resource("white_wool").unwrap(),
            ResourceKind::Mob {
                resource_id: "minecraft:white_wool".into(),
                mob_id: "minecraft:sheep".into(),
            }
        );
    }

    #[test]
    fn rejects_identifiers_that_are_neither_ore_a_mob_drop_nor_a_block() {
        assert!(matches!(
            resolve_resource("not_a_real_thing"),
            Err(AppError::UnknownResourceIdentifier(_))
        ));
        assert!(matches!(
            resolve_resource(""),
            Err(AppError::InvalidBlockIdentifier(_))
        ));
        assert!(matches!(
            resolve_resource("has space"),
            Err(AppError::InvalidBlockIdentifier(_))
        ));
    }
}
