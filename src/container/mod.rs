//! Generic, server-confirmed container interaction primitives.
//! Only ordinary and trapped single/double chests are adapted here; storage
//! indexing, sorting, crafting, furnaces, and exotic menus deliberately live out of scope.
pub mod model;
pub mod planner;
pub mod service;
pub mod state_machine;
