//! Plain data shared between `MinecraftClient::equipment_snapshot` (the only
//! place that touches Azalea's inventory types) and the pure decision logic
//! in `crate::equipment::armor`/`crate::equipment::offhand`. Keeping this
//! Azalea-free is what makes that decision logic reusable and unit-testable
//! without a live connection.

use std::ops::RangeInclusive;

/// The protocol slot index of the offhand inside the player's own inventory
/// menu (`azalea_inventory::Player::OFFHAND_SLOT`).
pub const OFFHAND_PROTOCOL_SLOT: usize = 45;

/// The protocol slot range of the 36-slot main inventory (hotbar included),
/// excluding the 2x2 crafting grid, armor, and offhand
/// (`azalea_inventory::Player::INVENTORY_SLOTS`).
pub const INVENTORY_PROTOCOL_SLOTS: RangeInclusive<usize> = 9..=44;

/// The protocol slot range of just the hotbar, the last 9 of
/// [`INVENTORY_PROTOCOL_SLOTS`] (`azalea_inventory::Player::HOTBAR_SLOTS`).
pub const HOTBAR_PROTOCOL_SLOTS: RangeInclusive<usize> = 36..=44;

/// A single item stack the equipment system cares about: an armor slot, the
/// offhand slot, or a main-inventory slot. `slot` is the protocol slot index
/// inside the player's own inventory menu, used unchanged as the click
/// source/destination when equipping (see `equipment::manager`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EquipmentItem {
    pub slot: usize,
    pub item_id: String,
    pub current_durability: u32,
    pub max_durability: u32,
}

/// A snapshot of everything the equipment system needs to decide what
/// should change, all read in one pass so a decision is always made against
/// one consistent moment of inventory state.
#[derive(Clone, Debug, Default)]
pub struct EquipmentSnapshot {
    /// Indexed by `crate::equipment::armor::ArmorSlot::index` (head, chest,
    /// legs, feet).
    pub armor_worn: [Option<EquipmentItem>; 4],
    pub offhand_worn: Option<EquipmentItem>,
    pub inventory: Vec<EquipmentItem>,
}
