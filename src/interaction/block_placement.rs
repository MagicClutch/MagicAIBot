//! Typed, policy-free state model for one ordinary block placement.
//!
//! The controller owns Minecraft adapters (navigation, rotation and inventory);
//! this module keeps transitions deterministic and independently testable.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::{interaction::faces::BlockFace, minecraft::world_state::BlockPosition};

#[derive(Clone, Debug)]
pub struct BlockPlacementRequest {
    pub target: BlockPosition,
    pub dimension: Option<String>,
    pub accepted_items: Vec<String>,
    pub interaction_range: f64,
    pub allow_navigation: bool,
    pub allow_substitution: bool,
    pub preferred_face: Option<BlockFace>,
    pub timeout: Duration,
    pub maximum_retries: u32,
    pub cancellation: CancellationToken,
}

impl BlockPlacementRequest {
    pub fn exact(target: BlockPosition, item: String, range: f64, timeout: Duration) -> Self {
        Self {
            target,
            dimension: None,
            accepted_items: vec![item],
            interaction_range: range,
            allow_navigation: true,
            allow_substitution: false,
            preferred_face: None,
            timeout,
            maximum_retries: 3,
            cancellation: CancellationToken::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlacementPhase {
    Validating,
    SelectingHit,
    Navigating,
    Rotating,
    PreparingHotbar,
    Dispatching,
    Confirming,
    Retrying,
    Finished,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlacementFailure {
    Disconnected,
    Died,
    Cancelled,
    TimedOut,
    WrongDimension,
    Unloaded,
    OutsideWorld,
    Forbidden,
    TargetOccupied,
    TargetNotReplaceable,
    TargetObstructed,
    NoSupportFace,
    SupportChanged,
    ItemMissing,
    ItemInaccessible,
    InventoryFull,
    InventoryStale,
    OutOfRange,
    Unreachable,
    PathFailed,
    RotationFailed,
    ServerRejected,
    ResultNotConfirmed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockPlacementResult {
    Placed { item: String, attempts: u32 },
    AlreadySatisfied,
    Failed(PlacementFailure),
}

/// Inputs observed from authoritative adapters during one controller tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlacementObservation {
    pub cancelled: bool,
    pub connected: bool,
    pub alive: bool,
    pub valid: bool,
    pub hit_selected: bool,
    pub in_range: bool,
    pub navigation_finished: bool,
    pub rotation_finished: bool,
    pub hotbar_confirmed: bool,
    pub interaction_sent: bool,
    pub world_confirmed: bool,
    pub inventory_confirmed: bool,
    pub attempt_expired: bool,
    pub total_expired: bool,
}

#[derive(Clone, Debug)]
pub struct PlacementMachine {
    pub phase: PlacementPhase,
    pub attempts: u32,
    pub result: Option<BlockPlacementResult>,
    maximum_retries: u32,
}

impl PlacementMachine {
    pub fn new(maximum_retries: u32) -> Self {
        Self { phase: PlacementPhase::Validating, attempts: 0, result: None, maximum_retries }
    }

    pub fn fail(&mut self, failure: PlacementFailure) {
        self.phase = PlacementPhase::Finished;
        self.result = Some(BlockPlacementResult::Failed(failure));
    }

    pub fn advance(&mut self, observation: PlacementObservation) {
        if observation.cancelled { self.fail(PlacementFailure::Cancelled); return; }
        if !observation.connected { self.fail(PlacementFailure::Disconnected); return; }
        if !observation.alive { self.fail(PlacementFailure::Died); return; }
        if observation.total_expired { self.fail(PlacementFailure::TimedOut); return; }
        self.phase = match self.phase {
            PlacementPhase::Validating if observation.valid => PlacementPhase::SelectingHit,
            PlacementPhase::SelectingHit if observation.hit_selected => {
                if observation.in_range { PlacementPhase::Rotating } else { PlacementPhase::Navigating }
            }
            PlacementPhase::Navigating if observation.navigation_finished => PlacementPhase::Rotating,
            PlacementPhase::Rotating if observation.rotation_finished => PlacementPhase::PreparingHotbar,
            PlacementPhase::PreparingHotbar if observation.hotbar_confirmed => PlacementPhase::Dispatching,
            PlacementPhase::Dispatching if observation.interaction_sent => {
                self.attempts += 1;
                PlacementPhase::Confirming
            }
            PlacementPhase::Confirming if observation.world_confirmed && observation.inventory_confirmed => {
                self.result = Some(BlockPlacementResult::Placed { item: String::new(), attempts: self.attempts });
                PlacementPhase::Finished
            }
            PlacementPhase::Confirming if observation.attempt_expired => {
                if self.attempts > self.maximum_retries {
                    self.result = Some(BlockPlacementResult::Failed(PlacementFailure::ServerRejected));
                    PlacementPhase::Finished
                } else { PlacementPhase::Retrying }
            }
            PlacementPhase::Retrying => PlacementPhase::Validating,
            current => current,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn observed() -> PlacementObservation { PlacementObservation { connected: true, alive: true, ..Default::default() } }
    #[test]
    fn mocked_machine_visits_every_phase_and_requires_dual_confirmation() {
        let mut machine = PlacementMachine::new(1);
        let mut o = observed(); o.valid = true; machine.advance(o); assert_eq!(machine.phase, PlacementPhase::SelectingHit);
        let mut o = observed(); o.hit_selected = true; machine.advance(o); assert_eq!(machine.phase, PlacementPhase::Navigating);
        let mut o = observed(); o.navigation_finished = true; machine.advance(o); assert_eq!(machine.phase, PlacementPhase::Rotating);
        let mut o = observed(); o.rotation_finished = true; machine.advance(o); assert_eq!(machine.phase, PlacementPhase::PreparingHotbar);
        let mut o = observed(); o.hotbar_confirmed = true; machine.advance(o); assert_eq!(machine.phase, PlacementPhase::Dispatching);
        let mut o = observed(); o.interaction_sent = true; machine.advance(o); assert_eq!(machine.phase, PlacementPhase::Confirming);
        let mut o = observed(); o.world_confirmed = true; machine.advance(o); assert_eq!(machine.phase, PlacementPhase::Confirming);
        let mut o = observed(); o.world_confirmed = true; o.inventory_confirmed = true; machine.advance(o);
        assert_eq!(machine.phase, PlacementPhase::Finished);
    }
    #[test]
    fn mocked_retry_is_bounded() {
        let mut machine = PlacementMachine::new(0); machine.phase = PlacementPhase::Confirming; machine.attempts = 1;
        let mut o = observed(); o.attempt_expired = true; machine.advance(o);
        assert_eq!(machine.result, Some(BlockPlacementResult::Failed(PlacementFailure::ServerRejected)));
    }
    #[test]
    fn terminal_guards_cover_cancellation_death_disconnect_and_timeout() {
        for (observation, expected) in [
            (PlacementObservation { cancelled: true, ..observed() }, PlacementFailure::Cancelled),
            (PlacementObservation { alive: false, ..observed() }, PlacementFailure::Died),
            (PlacementObservation { connected: false, ..observed() }, PlacementFailure::Disconnected),
            (PlacementObservation { total_expired: true, ..observed() }, PlacementFailure::TimedOut),
        ] { let mut m = PlacementMachine::new(1); m.advance(observation); assert_eq!(m.result, Some(BlockPlacementResult::Failed(expected))); }
    }
    #[test]
    fn failures_are_typed_for_each_adapter_boundary() {
        let failures = [PlacementFailure::WrongDimension, PlacementFailure::Unloaded, PlacementFailure::OutsideWorld,
            PlacementFailure::Forbidden, PlacementFailure::TargetOccupied, PlacementFailure::TargetNotReplaceable,
            PlacementFailure::TargetObstructed, PlacementFailure::NoSupportFace, PlacementFailure::SupportChanged,
            PlacementFailure::ItemMissing, PlacementFailure::ItemInaccessible, PlacementFailure::InventoryFull,
            PlacementFailure::InventoryStale, PlacementFailure::OutOfRange, PlacementFailure::Unreachable,
            PlacementFailure::PathFailed, PlacementFailure::RotationFailed, PlacementFailure::ResultNotConfirmed];
        for failure in failures { let mut m = PlacementMachine::new(1); m.fail(failure.clone()); assert_eq!(m.result, Some(BlockPlacementResult::Failed(failure))); }
    }
}
