//! Ticks the automatic equipment system: reads the live inventory, decides
//! whether any armor slot or the offhand should change per
//! `crate::equipment::armor`/`crate::equipment::offhand`, and issues the
//! clicks to do it.
//!
//! Deliberately stateless between ticks: every decision is recomputed from
//! a fresh `EquipmentSnapshot`, so pickups, drops, durability loss,
//! crafting, and looting are all picked up automatically on the next tick
//! without needing a separate event hook for each one. `should_replace`'s
//! thresholds (a 0.2-point score improvement, a strictly higher rarity
//! tier) are what keep this from flickering between near-identical pieces.

use crate::{
    config::EquipmentConfig,
    container::model::{ClickButton, InventoryClick},
    equipment::{
        armor::{self, ArmorSlot},
        autodrop::{drop_displaced_item, should_drop},
        model::OFFHAND_PROTOCOL_SLOT,
        offhand,
    },
    logging,
    minecraft::client::MinecraftClient,
};

#[derive(Clone)]
pub struct EquipmentService {
    config: EquipmentConfig,
}

impl EquipmentService {
    pub fn new(config: EquipmentConfig) -> Self {
        Self { config }
    }

    pub async fn tick(&self, minecraft: &MinecraftClient) {
        let Ok(snapshot) = minecraft.equipment_snapshot().await else {
            // No live connection, or (rarely) a container is open right as
            // this tick runs -- either way, nothing to do until the next
            // tick sees a normal snapshot again.
            return;
        };

        for slot in ArmorSlot::ALL {
            let worn = snapshot.armor_worn[slot.index()].as_ref();
            let Some(candidate) =
                armor::best_candidate(self.config.armor.mode, slot, &snapshot.inventory)
            else {
                continue;
            };
            if !armor::should_replace(self.config.armor.mode, worn, candidate) {
                continue;
            }
            let source_slot = candidate.item.slot;
            let item_label = candidate.item.item_id.clone();
            let displaced = worn.map(|item| item.item_id.clone());
            // Same reasoning as the offhand swap below: an armor slot is
            // never the hand, so upgrading it can't disrupt a bite or a
            // Wind Charge throw in progress -- and `#kill`'s more frequent
            // Wind Charge self-launches (see `combat::wind_launch`) mean the
            // consume guard is now up often enough that waiting for it to
            // clear would leave picked-up armor sitting unequipped through
            // most of a fight.
            if swap_into_slot_during_consume(minecraft, source_slot, slot.protocol_slot()).await {
                logging::info(format!("Equipped {item_label}"));
                if let Some(displaced_id) = displaced {
                    self.maybe_drop_armor(minecraft, &displaced_id, source_slot)
                        .await;
                }
            }
        }

        // An item already worn in the offhand no longer shows up in
        // `snapshot.inventory` -- it left the main inventory when it was
        // equipped -- so it must be counted here too. Otherwise the
        // moment either item gets equipped it looks "unavailable", the
        // priority falls through to the other item, that swap pushes the
        // first item back into the main inventory, and the next tick
        // swaps back: an infinite totem/shield flip-flop.
        let has_totem = snapshot
            .inventory
            .iter()
            .any(|item| item.item_id == offhand::TOTEM_OF_UNDYING)
            || snapshot
                .offhand_worn
                .as_ref()
                .is_some_and(|item| item.item_id == offhand::TOTEM_OF_UNDYING);
        let has_shield = snapshot
            .inventory
            .iter()
            .any(|item| item.item_id == offhand::SHIELD)
            || snapshot
                .offhand_worn
                .as_ref()
                .is_some_and(|item| item.item_id == offhand::SHIELD);
        let Some(desired) =
            offhand::desired_item(self.config.offhand.priority, has_totem, has_shield)
        else {
            return;
        };
        let already_worn = snapshot
            .offhand_worn
            .as_ref()
            .is_some_and(|item| item.item_id == desired);
        if already_worn {
            return;
        }
        let Some(source) = snapshot
            .inventory
            .iter()
            .find(|item| item.item_id == desired)
        else {
            return;
        };
        let source_slot = source.slot;
        // Emergency, like `crate::survival`'s water-bucket restock: allowed
        // to move the hand even mid-bite. A Totem of Undying can pop from a
        // hit landed while the bot is also mid-consume (already eating for
        // an unrelated `heal_threshold` dip when the killing blow arrives),
        // which empties the offhand at the worst possible moment -- 1 HP,
        // no totem, and up to ~2.7s (`consume::SLOT_ACK_TIMEOUT` +
        // `consume::USE_TIMEOUT`) before the guarded path would even be
        // allowed to restock it. A dead totem slot beats a dead bot.
        if swap_into_slot_during_consume(minecraft, source_slot, OFFHAND_PROTOCOL_SLOT).await {
            logging::info(format!("Equipped {desired} in offhand"));
        }
    }

    /// Off by default (`equipment.autodrop.armor.enabled = false`) --
    /// players often want to keep backup armor -- but when enabled, drops
    /// the piece this replacement just displaced (now sitting wherever the
    /// new piece used to be, per `swap_into_slot`'s doc comment) the same
    /// way `equipment::hotbar::HotbarEquipmentService` does for
    /// tools/weapons, via the same shared policy.
    async fn maybe_drop_armor(&self, minecraft: &MinecraftClient, item_id: &str, slot: usize) {
        let autodrop = &self.config.autodrop;
        if !should_drop(
            autodrop.enabled,
            &autodrop.armor,
            item_id,
            &autodrop.protected_items,
        ) {
            return;
        }
        if drop_displaced_item(minecraft, slot).await {
            logging::info(format!("Dropped {item_id}"));
        }
    }
}

/// Three-click swap: pick the item up from `source`, place it at
/// `destination` (swapping out whatever was already there, if anything,
/// onto the cursor), then place the swapped-out item back into `source`. If
/// `destination` was empty the third click is a harmless no-op (nothing on
/// the cursor, nothing in the now-empty `source`).
///
/// Azalea applies each click's client-side prediction synchronously --
/// `ContainerClickEvent` is handled by an observer
/// (`handle_container_click_event`), not a system scheduled for a later tick
/// -- so the third click always sees the correct post-swap state with no
/// delay needed between clicks.
///
/// Shared by [`EquipmentService`] (armor/offhand) and
/// `crate::survival::SurvivalController` (stocking a water bucket into the
/// hotbar) -- the only inventory-mutation primitive either needs, so neither
/// duplicates it.
pub(crate) async fn swap_into_slot(
    minecraft: &MinecraftClient,
    source: usize,
    destination: usize,
) -> bool {
    swap_into_slot_inner(minecraft, source, destination, false).await
}

/// [`swap_into_slot`] for the consume path, which is allowed to move items
/// while the consume guard is held -- see
/// `MinecraftClient::consume_guard`. Every other caller must use
/// [`swap_into_slot`] and be refused mid-bite.
pub(crate) async fn swap_into_slot_during_consume(
    minecraft: &MinecraftClient,
    source: usize,
    destination: usize,
) -> bool {
    swap_into_slot_inner(minecraft, source, destination, true).await
}

async fn swap_into_slot_inner(
    minecraft: &MinecraftClient,
    source: usize,
    destination: usize,
    during_consume: bool,
) -> bool {
    // Resolved once, up front, against one consistent snapshot of whichever
    // menu is currently open -- so all three clicks below land in the same
    // window/slot mapping even if a container opens or closes right as this
    // swap starts. Without this, every equip/hotbar swap hardcoded window 0
    // and the player-menu's own slot numbers, which silently failed
    // (`container_click`'s window-id check) for the entire time any other
    // task -- `/getitem`, a chest transfer, ... -- had a container open,
    // stalling auto-equip until it closed. See
    // `MinecraftClient::active_menu_window`'s doc comment.
    let Ok(active) = minecraft.active_menu_window().await else {
        return false;
    };
    let (Some(source), Some(destination)) =
        (active.translate(source), active.translate(destination))
    else {
        // Armor/offhand while a non-player menu is open -- no equivalent
        // slot exists until it closes; retried fresh next tick like every
        // other equip decision in this module.
        return false;
    };
    for slot in [source, destination, source] {
        let click = InventoryClick {
            slot,
            button: ClickButton::Left,
        };
        let outcome = if during_consume {
            minecraft
                .container_click_during_consume(active.window_id, click)
                .await
        } else {
            minecraft.container_click(active.window_id, click).await
        };
        if outcome.is_err() {
            return false;
        }
    }
    true
}
