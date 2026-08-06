//! Shared "should we drop the item a replacement just displaced" policy,
//! plus the single async primitive that actually drops it -- reused by both
//! `equipment::manager::EquipmentService` (armor) and
//! `equipment::hotbar::HotbarEquipmentService` (tools/weapons) so there is
//! exactly one auto-drop implementation.
//!
//! Only ever called on an item a *successful* swap already displaced (see
//! each caller's use of `equipment::manager::swap_into_slot`): the
//! currently-equipped item itself is never a candidate, and a replacement
//! is always confirmed equipped before this runs -- see this module's
//! functions' own doc comments for exactly where in that sequence each one
//! belongs.

use crate::{
    config::AutoDropCategoryConfig, container::model::DropClick, minecraft::client::MinecraftClient,
};

/// Whether a just-displaced item should be dropped, given its category's
/// config and the master `equipment.autodrop.enabled` switch.
/// `protected_items` (exact item ids the user wants kept regardless, e.g. a
/// deliberately held spare tool) always wins over every other setting --
/// this is the "avoid dropping rare/valuable items accidentally" guard.
pub fn should_drop(
    autodrop_enabled: bool,
    category: &AutoDropCategoryConfig,
    item_id: &str,
    protected_items: &[String],
) -> bool {
    autodrop_enabled
        && category.enabled
        && category.drop_when_better_available
        && !protected_items.iter().any(|protected| protected == item_id)
}

/// Drops the whole stack sitting in `slot` (a protocol slot index -- e.g.
/// wherever `equipment::manager::swap_into_slot` just left the displaced
/// item). Call only after the replacement is already confirmed equipped:
/// this never touches the slot the new item was placed into, only the one
/// the old item was swapped out to. `false` on any failure -- the caller
/// already has the replacement equipped either way, so a failed drop just
/// leaves the old item sitting in the inventory rather than requiring a
/// rollback.
pub async fn drop_displaced_item(minecraft: &MinecraftClient, slot: usize) -> bool {
    minecraft
        .drop_click(
            0,
            DropClick {
                slot,
                whole_stack: true,
            },
        )
        .await
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn category(enabled: bool) -> AutoDropCategoryConfig {
        AutoDropCategoryConfig {
            enabled,
            drop_when_better_available: true,
        }
    }

    #[test]
    fn drops_only_when_every_gate_is_open() {
        assert!(should_drop(
            true,
            &category(true),
            "minecraft:wooden_pickaxe",
            &[]
        ));
    }

    #[test]
    fn master_switch_overrides_category() {
        assert!(!should_drop(
            false,
            &category(true),
            "minecraft:wooden_pickaxe",
            &[]
        ));
    }

    #[test]
    fn category_disabled_blocks_the_drop() {
        assert!(!should_drop(
            true,
            &category(false),
            "minecraft:wooden_pickaxe",
            &[]
        ));
    }

    #[test]
    fn drop_when_better_available_false_blocks_the_only_current_trigger() {
        let category = AutoDropCategoryConfig {
            enabled: true,
            drop_when_better_available: false,
        };
        assert!(!should_drop(
            true,
            &category,
            "minecraft:wooden_pickaxe",
            &[]
        ));
    }

    #[test]
    fn protected_items_are_never_dropped() {
        assert!(!should_drop(
            true,
            &category(true),
            "minecraft:wooden_pickaxe",
            &["minecraft:wooden_pickaxe".to_owned()]
        ));
    }

    #[test]
    fn protection_is_by_exact_id_only() {
        assert!(should_drop(
            true,
            &category(true),
            "minecraft:stone_pickaxe",
            &["minecraft:wooden_pickaxe".to_owned()]
        ));
    }
}
