//! Extends `#get` to also support mob drops, without touching the existing
//! block-gathering implementation (`App::run_get_block` in `src/app.rs`).
//! [`resolve_resource`] is the single decision point for "is this a block or
//! a mob drop" -- used both to validate `/get`'s argument at parse time
//! (`console::commands::parse_get`) and to choose which gathering path
//! `App::run_get_resource` dispatches to.

pub mod combat;
pub mod drops;

pub use combat::{CombatController, CombatState};
pub use drops::{mob_for_resource, mob_label};

use crate::{blocks::block_query::normalize_block_id, error::AppError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceKind {
    Block(String),
    Mob { resource_id: String, mob_id: String },
}

/// Normalizes `input` the same way block identifiers are (trim, lowercase,
/// default `minecraft:` namespace, single-colon vanilla-namespace syntax)
/// without requiring it to be a *block* -- `#get`'s resource argument may
/// equally be an item obtained only from a mob (e.g. `leather`).
///
/// The mob-drop table is checked *before* falling back to block validation,
/// not after: a few documented drops (the wool colors) are also real,
/// placeable blocks, but in practice mining a loaded `white_wool` block is
/// essentially never possible (it isn't naturally generated) while shearing
/// or farming it from a sheep always is. Letting an incidental block-id
/// collision win would silently defeat the mob-drop mapping for exactly the
/// resources it exists to cover, so mob drops take priority.
pub fn resolve_resource(input: &str) -> Result<ResourceKind, AppError> {
    let normalized = normalize_item_syntax(input)?;
    if let Some(mob_id) = mob_for_resource(&normalized) {
        return Ok(ResourceKind::Mob {
            resource_id: normalized,
            mob_id: mob_id.to_owned(),
        });
    }
    normalize_block_id(input)
        .map(ResourceKind::Block)
        .map_err(|_| AppError::UnknownResourceIdentifier(normalized))
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

    #[test]
    fn resolves_blocks_directly() {
        assert_eq!(
            resolve_resource("oak_log").unwrap(),
            ResourceKind::Block("minecraft:oak_log".into())
        );
        assert_eq!(
            resolve_resource("minecraft:cobblestone").unwrap(),
            ResourceKind::Block("minecraft:cobblestone".into())
        );
    }

    #[test]
    fn resolves_mob_drops_when_not_a_block() {
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
    fn a_wool_color_block_still_resolves_to_sheep_farming() {
        // `white_wool` is a real, placeable block, but the task's intent
        // (and the mob-drop table) is for wool-colored resources to trigger
        // sheep farming, since that's how wool is actually obtained in a
        // fresh world -- mob drops win over an incidental block-id
        // collision.
        assert_eq!(
            resolve_resource("white_wool").unwrap(),
            ResourceKind::Mob {
                resource_id: "minecraft:white_wool".into(),
                mob_id: "minecraft:sheep".into(),
            }
        );
    }

    #[test]
    fn rejects_identifiers_that_are_neither_a_block_nor_a_mob_drop() {
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
