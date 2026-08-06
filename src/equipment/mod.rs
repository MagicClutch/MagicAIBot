//! Fully automatic equipment: always wear the best available armor and keep
//! the offhand stocked with whichever of a Totem of Undying or a Shield the
//! config prefers. See `crate::config::EquipmentConfig` for the user-facing
//! settings (`equipment.armor.mode`, `equipment.offhand.priority`).
//!
//! `armor` and `offhand` hold the pure decision logic (scoring,
//! classification, selection -- no Azalea types, directly unit-tested).
//! `manager::EquipmentService` is the thin async layer that reads the live
//! inventory through `MinecraftClient::equipment_snapshot` and issues the
//! clicks; `model` is the plain data passed between the two.

pub mod armor;

pub mod autodrop;
pub mod hotbar;
pub mod manager;
pub mod model;
pub mod offhand;
pub mod scoring;
pub mod tools;

pub use hotbar::HotbarEquipmentService;
pub use manager::EquipmentService;
