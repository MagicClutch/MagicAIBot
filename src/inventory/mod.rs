//! Confirmed, application-owned inventory mutation orchestration.

mod planner;
mod service;

#[allow(unused_imports)]
pub use service::{InventoryActionService, InventoryOutcome, InventoryRequest, Rejection};
pub(crate) use service::{
    InventoryClick, InventorySlotView, InventoryTransport, InventoryView, MenuKind,
};
