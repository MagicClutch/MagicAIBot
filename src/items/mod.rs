//! Central item-identifier registry backing `#get`, the ore resolver, and
//! inventory counting.
//!
//! The bug class this exists to close: nothing in Minecraft guarantees a
//! block's or a display name's obvious "lowercase, spaces to underscores"
//! transform is the item id the server actually reports. Raw Beef is
//! `minecraft:beef`, not `minecraft:raw_beef`; Raw Cod is `minecraft:cod`,
//! not `minecraft:raw_cod` or `minecraft:fish`; mining `iron_ore` without
//! Silk Touch yields `minecraft:raw_iron`, not an item called
//! `minecraft:iron_ore`. A resolver that blindly stringifies the user's
//! input (or a block's own name) into the "item id" ends up asking
//! `InventorySnapshot::count_item` for something that can never appear,
//! which reads as "the bot mines forever and inventory never updates."
//!
//! This module is the single place that turns free-form input into a
//! canonical, registry-checked item id:
//! - [`normalize_item_id`] handles syntax (trim/lowercase/whitespace
//!   collapsing so multi-word input like `"raw beef"` works) and corrects
//!   known wrong guesses via [`ITEM_ALIASES`].
//! - [`validate_item_id`] confirms the resulting id actually exists in
//!   Azalea's item registry (`azalea::registry::builtin::ItemKind`, the same
//!   client-side registry `blocks::block_query::normalize_block_id` already
//!   trusts for blocks) before anything is allowed to search inventory for
//!   it.
//! - [`audit_registered_items`] walks every item id this bot's static
//!   tables reference (`blocks::drops::BLOCK_DROPS`, `mobs::drops::MOB_DROPS`,
//!   the alias targets here) and reports any that fail that same check, so a
//!   bad hand-maintained table entry is caught at startup instead of
//!   surfacing later as "why did #get never finish."

use crate::error::AppError;

/// `(alias, canonical bare item id)`. Both sides are bare -- no `minecraft:`
/// namespace, matching the path Azalea's registry macro parses. An alias
/// fires only when it is *itself* not a valid item id (checked by
/// `resolve_alias`'s caller ordering: canonical ids never need to appear on
/// the left), so this table only ever "fixes" a guess, never shadows a real
/// item.
///
/// Covers three failure modes seen in this codebase's own history:
/// 1. Guessing an item id from a mob's raw-drop *display name* ("Raw Beef"
///    -> `raw_beef`) when vanilla's actual id drops the "raw_" prefix
///    entirely -- every raw meat item (`beef`, `porkchop`, `mutton`,
///    `chicken`, `rabbit`) is named without it.
/// 2. An id that isn't simply the item's bare name -- `fish` -> `cod`/
///    `salmon` (the pre-1.13-flattening name players still use out of
///    habit), `scute` -> `turtle_scute` (the actual id is prefixed to
///    disambiguate from Armadillo Scute).
/// 3. A common colloquial shorthand a player might type (`lapis` for
///    `lapis_lazuli`).
const ITEM_ALIASES: &[(&str, &str)] = &[
    ("raw_beef", "beef"),
    ("raw_porkchop", "porkchop"),
    ("raw_mutton", "mutton"),
    ("raw_chicken", "chicken"),
    ("raw_rabbit", "rabbit"),
    ("raw_cod", "cod"),
    ("raw_salmon", "salmon"),
    ("fish", "cod"),
    ("raw_fish", "cod"),
    ("lapis", "lapis_lazuli"),
    ("lapis_lazuli_ore", "lapis_lazuli"),
    ("scute", "turtle_scute"),
];

/// Resolves a bare path (no namespace) through [`ITEM_ALIASES`] if it is a
/// known wrong guess; otherwise returns it unchanged.
fn resolve_alias(bare_id: &str) -> &str {
    ITEM_ALIASES
        .iter()
        .find(|(alias, _)| *alias == bare_id)
        .map_or(bare_id, |(_, canonical)| canonical)
}

/// Normalizes free-form item input -- including multi-word phrases such as
/// `"raw beef"` -- into a fully namespaced, alias-corrected identifier.
/// Whitespace of any kind (spaces, tabs, repeated runs) collapses to a
/// single `_` between words rather than being rejected, since `#get`'s
/// parser now hands this a joined multi-word item name rather than a single
/// token. This only checks syntax and known-alias correction; it does not
/// confirm the id exists in the registry -- see [`validate_item_id`] for
/// that.
pub fn normalize_item_id(input: &str) -> Result<String, AppError> {
    let collapsed = input.split_whitespace().collect::<Vec<_>>().join("_");
    let collapsed = collapsed.to_ascii_lowercase();
    if collapsed.is_empty() {
        return Err(AppError::InvalidBlockIdentifier(
            "item identifier must not be empty".into(),
        ));
    }
    let normalized = if collapsed.contains(':') {
        collapsed
    } else {
        format!("minecraft:{collapsed}")
    };
    let Some((namespace, path)) = normalized.split_once(':') else {
        return Err(AppError::InvalidBlockIdentifier(
            "item identifier must contain a valid namespace and path".into(),
        ));
    };
    if namespace != "minecraft"
        || path.is_empty()
        || normalized.matches(':').count() != 1
        || !crate::blocks::block_query::valid_identifier_part(namespace, true)
        || !crate::blocks::block_query::valid_identifier_part(path, false)
    {
        return Err(AppError::InvalidBlockIdentifier(format!(
            "malformed item identifier: {normalized}"
        )));
    }
    Ok(format!("{namespace}:{}", resolve_alias(path)))
}

/// Confirms `item_id` (already namespaced, e.g. `minecraft:beef`) exists in
/// Azalea's item registry. This is what actually catches a hand-maintained
/// table entry that names an item Minecraft doesn't have -- syntax alone
/// (`normalize_item_id`) cannot tell `minecraft:raw_beef` apart from a real
/// id, since both are equally well-formed strings.
pub fn validate_item_id(item_id: &str) -> Result<(), AppError> {
    item_id
        .parse::<azalea::registry::builtin::ItemKind>()
        .map_err(|_| AppError::UnknownItemIdentifier(item_id.to_owned()))?;
    Ok(())
}

/// Every item id this bot's static tables reference, checked against the
/// item registry at startup. Returns one human-readable line per invalid
/// entry (empty when everything checks out) -- callers log these as
/// warnings rather than failing startup, since a bad table entry should not
/// take down a bot that doesn't even need that entry this session.
pub fn audit_registered_items() -> Vec<String> {
    let mut invalid = Vec::new();
    for item_id in crate::blocks::drops::registered_drop_items() {
        if let Err(error) = validate_item_id(item_id) {
            invalid.push(format!("block drop table: {error}"));
        }
    }
    for item_id in crate::mobs::drops::registered_drop_items() {
        if let Err(error) = validate_item_id(item_id) {
            invalid.push(format!("mob drop table: {error}"));
        }
    }
    for (alias, canonical) in ITEM_ALIASES {
        let namespaced = format!("minecraft:{canonical}");
        if let Err(error) = validate_item_id(&namespaced) {
            invalid.push(format!("item alias {alias} -> {error}"));
        }
    }
    invalid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_multi_word_input_by_collapsing_whitespace() {
        assert_eq!(normalize_item_id("raw beef").unwrap(), "minecraft:beef");
        assert_eq!(
            normalize_item_id("  Raw   Beef  ").unwrap(),
            "minecraft:beef"
        );
    }

    #[test]
    fn corrects_known_wrong_guesses() {
        assert_eq!(normalize_item_id("raw_beef").unwrap(), "minecraft:beef");
        assert_eq!(
            normalize_item_id("raw_porkchop").unwrap(),
            "minecraft:porkchop"
        );
        assert_eq!(normalize_item_id("raw_mutton").unwrap(), "minecraft:mutton");
        assert_eq!(
            normalize_item_id("raw_chicken").unwrap(),
            "minecraft:chicken"
        );
        assert_eq!(normalize_item_id("raw_rabbit").unwrap(), "minecraft:rabbit");
        assert_eq!(normalize_item_id("fish").unwrap(), "minecraft:cod");
        assert_eq!(normalize_item_id("raw_fish").unwrap(), "minecraft:cod");
        assert_eq!(normalize_item_id("raw_cod").unwrap(), "minecraft:cod");
        assert_eq!(normalize_item_id("raw_salmon").unwrap(), "minecraft:salmon");
        assert_eq!(
            normalize_item_id("lapis").unwrap(),
            "minecraft:lapis_lazuli"
        );
        assert_eq!(
            normalize_item_id("scute").unwrap(),
            "minecraft:turtle_scute"
        );
    }

    #[test]
    fn leaves_already_correct_ids_untouched() {
        for id in [
            "diamond",
            "coal",
            "raw_iron",
            "raw_copper",
            "raw_gold",
            "cooked_beef",
            "beef",
        ] {
            assert_eq!(normalize_item_id(id).unwrap(), format!("minecraft:{id}"));
        }
    }

    #[test]
    fn rejects_empty_input() {
        assert!(normalize_item_id("").is_err());
        assert!(normalize_item_id("   ").is_err());
    }

    #[test]
    fn validates_real_and_fake_items() {
        assert!(validate_item_id("minecraft:beef").is_ok());
        assert!(validate_item_id("minecraft:diamond").is_ok());
        assert!(validate_item_id("minecraft:raw_beef").is_err());
        assert!(validate_item_id("minecraft:not_a_real_item").is_err());
    }

    #[test]
    fn audit_finds_no_invalid_entries_in_the_current_tables() {
        assert!(
            audit_registered_items().is_empty(),
            "found invalid item ids: {:?}",
            audit_registered_items()
        );
    }
}
