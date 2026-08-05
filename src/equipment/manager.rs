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
        model::OFFHAND_PROTOCOL_SLOT,
        offhand,
    },
    logging,
    minecraft::client::MinecraftClient,
};

/// Window id 0 is always the player's own inventory (see
/// `MinecraftClient::container_click`'s doc comment) -- armor and offhand
/// slots only exist in that menu, so equip actions never target anything
/// else.
const PLAYER_INVENTORY_WINDOW: i32 = 0;

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
            if self
                .equip(minecraft, source_slot, slot.protocol_slot())
                .await
            {
                logging::info(format!("Equipped {item_label}"));
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
        let Some(source) = snapshot.inventory.iter().find(|item| item.item_id == desired) else {
            return;
        };
        let source_slot = source.slot;
        if self.equip(minecraft, source_slot, OFFHAND_PROTOCOL_SLOT).await {
            logging::info(format!("Equipped {desired} in offhand"));
        }
    }

    /// Three-click swap: pick the item up from `source`, place it at
    /// `destination` (swapping out whatever was already worn there, if
    /// anything, onto the cursor), then place the swapped-out item back
    /// into `source`. If `destination` was empty the third click is a
    /// harmless no-op (nothing on the cursor, nothing in the now-empty
    /// `source`).
    ///
    /// Azalea applies each click's client-side prediction synchronously --
    /// `ContainerClickEvent` is handled by an observer
    /// (`handle_container_click_event`), not a system scheduled for a
    /// later tick -- so the third click always sees the correct post-swap
    /// state with no delay needed between clicks.
    async fn equip(&self, minecraft: &MinecraftClient, source: usize, destination: usize) -> bool {
        for slot in [source, destination, source] {
            let click = InventoryClick {
                slot,
                button: ClickButton::Left,
            };
            if minecraft
                .container_click(PLAYER_INVENTORY_WINDOW, click)
                .await
                .is_err()
            {
                return false;
            }
        }
        true
    }
}
