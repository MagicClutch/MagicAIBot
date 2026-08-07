//! Combat-only movement: a continuous steering controller used by `#kill`
//! and by nothing else.
//!
//! # Scope, stated up front
//!
//! This module is deliberately walled off from
//! `crate::movement::MovementService` and `crate::pathfinding`. Everything
//! that is not a PvP fight -- `/goto`, `/follow`, `#get`, mining, gathering,
//! bridging, exploration -- keeps using those untouched. The only caller
//! here is `crate::combat::executor`, and only while a fight is actually in
//! range; the moment combat ends, control returns to the normal movement
//! system with nothing to undo.
//!
//! # What it replaces
//!
//! `#kill` never used the pathfinder -- it has always driven raw input --
//! but it used to do so by recomputing an eight-way decision from scratch
//! every tick from the distance alone. That has no memory, so it produced
//! instant direction reversals, no momentum, no anticipation of where the
//! opponent was going, and a strafe that was really just "hold left for a
//! while". This module replaces that decision with a proper steering
//! controller.
//!
//! # The constraint everything here is shaped by
//!
//! **Minecraft has no analogue movement input.** A player has WASD -- eight
//! directions, relative to the camera -- and that is all the protocol
//! carries. So a "continuous" controller cannot send a velocity vector; what
//! it can do is compute one in floating-point world space and project it
//! onto those eight keys every tick, with the camera yaw as the continuous
//! degree of freedom.
//!
//! That is not a compromise, it is how humans move: the smooth curve a real
//! player traces is a rotating camera plus key combinations that change
//! several times a second. Reproducing the curve is a matter of making the
//! *desired vector* rotate smoothly ([`smoothing`]) rather than jumping, and
//! then projecting honestly ([`steering::project_to_walk`]).
//!
//! # Layout
//!
//! - [`steering`] -- the desired-velocity vector, and the projection onto
//!   keys. Pure.
//! - [`smoothing`] -- turn-rate and throttle limits: momentum, curves,
//!   corner cutting. Pure.
//! - [`strafing`] -- which way to orbit and for how long, with human timing.
//!   Pure.
//! - [`prediction`] -- where the target is going, weighted by how
//!   predictably it has been moving. Pure.
//! - [`controller`] -- one tick: read the fight, produce key presses. Pure.
//!
//! Every module here is pure. `crate::combat::executor` owns the client
//! calls, the terrain probe, and the decision of when this controller is in
//! charge at all.

pub mod controller;
pub mod prediction;
pub mod smoothing;
pub mod steering;
pub mod strafing;

pub use controller::{
    CombatMovementController, LocalHazards, MovementCommand, MovementSnapshot, MovementTuning,
};
pub use steering::{CombatWalk, DistanceBand, Vec2};

/// Distance at which the bot is standing inside the opponent and needs to
/// circle out. Also what `crate::combat::executor` reports as the
/// `Reposition` phase.
pub const BACK_OFF_DISTANCE: f64 = 1.2;
/// Past this the bot is closing the gap rather than fighting, and the
/// approach dominates the orbit.
pub const AGGRESSIVE_CLOSE_DISTANCE: f64 = 2.5;

/// Extra reach, as a multiple of the engage range, before the pathfinder is
/// handed control back. Without a gap, a target hovering exactly at the
/// boundary would flip the bot between two movement systems every tick --
/// each handover stops the other, so the bot would stand still doing
/// nothing else.
const HANDOFF_HYSTERESIS: f64 = 1.35;

/// Which movement system should be driving right now.
///
/// Long distance is the existing pathfinder's job: it knows about terrain,
/// doors, water and cliffs, and getting to a fight fifty blocks away is
/// exactly the problem it was built for. Close range is this module's job:
/// there is nothing to path around at two blocks, and a route recomputed to
/// a block centre is the opposite of what a fight needs.
///
/// `currently_driving` makes the switch sticky -- see
/// [`HANDOFF_HYSTERESIS`].
#[must_use]
pub fn pathfinder_should_drive(distance: f64, engage_range: f64, currently_driving: bool) -> bool {
    if currently_driving {
        // Keep pathfinding until genuinely in the pocket.
        distance > engage_range
    } else {
        // Only hand back once the target is clearly out of combat range.
        distance > engage_range * HANDOFF_HYSTERESIS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_distant_target_is_approached_with_the_pathfinder() {
        assert!(pathfinder_should_drive(50.0, 6.0, false));
        assert!(pathfinder_should_drive(50.0, 6.0, true));
    }

    #[test]
    fn combat_movement_takes_over_inside_the_engage_range() {
        assert!(!pathfinder_should_drive(2.0, 6.0, true));
        assert!(!pathfinder_should_drive(2.0, 6.0, false));
    }

    #[test]
    fn the_handover_does_not_flap_at_the_boundary() {
        let engage = 6.0;
        // Sitting exactly on the line: whichever system holds control keeps
        // it, because handing over costs a stop on the other one.
        assert!(!pathfinder_should_drive(6.5, engage, false));
        assert!(pathfinder_should_drive(6.5, engage, true));
    }

    #[test]
    fn a_target_that_runs_away_is_handed_back_to_the_pathfinder() {
        let engage = 6.0;
        let mut driving = false;
        for distance in [2.0, 4.0, 7.0, 9.0] {
            driving = pathfinder_should_drive(distance, engage, driving);
        }
        assert!(driving, "should be chasing with the pathfinder again");
    }

    #[test]
    fn closing_from_range_switches_exactly_once() {
        let engage = 6.0;
        let mut driving = true;
        let mut switches = 0;
        for step in 0..40 {
            let distance = 20.0 - 0.5 * f64::from(step);
            let next = pathfinder_should_drive(distance, engage, driving);
            if next != driving {
                switches += 1;
            }
            driving = next;
        }
        assert_eq!(switches, 1, "one clean handover on the way in");
        assert!(!driving);
    }

    #[test]
    fn the_distance_bands_are_ordered_and_match_the_steering_defaults() {
        let band = DistanceBand::default();
        assert_eq!(band.too_close, BACK_OFF_DISTANCE);
        assert_eq!(band.chase, AGGRESSIVE_CLOSE_DISTANCE);
        assert!(band.preferred_min < band.preferred_max);
        assert!(
            band.preferred_min >= 1.5 && band.preferred_max <= 2.0,
            "the preferred band should sit in the 1.5-2.0 pocket"
        );
    }
}
