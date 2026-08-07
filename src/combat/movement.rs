//! Pure PvP movement decisions: given the current distance to the target
//! and a strafe side, decide which way to walk and whether to sprint. This
//! is a deliberately separate, much simpler movement model than
//! `crate::movement::MovementService` -- no pathfinding, no A*, no terrain
//! awareness -- because combat only ever needs to answer "closer, further,
//! or hold this distance while circling", not "how do I get there". See
//! `crate::combat::executor` for how a decision here becomes an actual
//! `MinecraftClient::set_combat_walk` call, and `crate::combat::kill`'s
//! module doc comment for why this stays entirely separate from navigation.

use std::time::Duration;

use crate::look::aim_point::SeededRng;

/// Below this the bot is effectively standing inside the target. It no
/// longer backs off (see [`decide_movement`] -- full aggression never
/// retreats), it just stops advancing and circles, which is also what
/// `crate::combat::executor::compute_active_phase` reports as
/// `CombatPhase::Reposition`.
pub const BACK_OFF_DISTANCE: f64 = 1.2;
/// The top of the preferred band -- the bot holds position and just circles
/// anywhere from here all the way down to zero. There is no lower edge to
/// the band any more: full aggression never corrects back outward, so
/// "closer than preferred" is simply not a case that needs a distance.
pub const PREFERRED_MAX_DISTANCE: f64 = 1.9;
/// Past this, the bot switches from a gentle walking correction to
/// aggressively closing distance at a sprint -- a target this far away is
/// disengaging or repositioning, not just drifting inside the preferred
/// band.
pub const AGGRESSIVE_CLOSE_DISTANCE: f64 = 2.5;

/// How often the strafe side flips, before jitter -- frequent enough that
/// the bot doesn't circle predictably in one direction for more than a
/// third of a second or so, infrequent enough that it doesn't look like
/// it's vibrating. Roughly the midpoint of the spec's own example cadence
/// (250/340/210/390ms).
pub const STRAFE_FLIP_INTERVAL: Duration = Duration::from_millis(280);
/// +/- randomization applied to `STRAFE_FLIP_INTERVAL` (see
/// `next_flip_interval`) -- the "randomize strafe duration slightly, avoid
/// perfect robotic timing" the spec calls for, applied to timing rather
/// than position so it never causes the bot to drift out of combat range.
const STRAFE_FLIP_JITTER: Duration = Duration::from_millis(150);
/// Strafe-hold duration used for the tick right after landing a hit --
/// shorter than the normal interval, so a combo follow-up circles
/// noticeably faster instead of settling back into the lazier baseline
/// cadence mid-combo. See `crate::combat::executor`'s combo handling.
pub const COMBO_STRAFE_FLIP_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StrafeSide {
    #[default]
    Left,
    Right,
}
impl StrafeSide {
    #[must_use]
    pub fn flipped(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// Chance, each time the strafe timer flips (see [`should_flip_strafe`]),
/// that the bot plants its feet (`StrafePattern::Still`) for that interval
/// instead of picking a side to circle toward -- "don't always strafe":
/// permanent circling reads as robotic, and a real PvP player plants their
/// feet to square up a swing far more often than never.
const STILL_CHANCE: f64 = 0.3;

/// What the bot's strafing is doing this interval -- an active side, or a
/// deliberate pause. Layered on top of [`StrafeSide`] (which only ever
/// means "which way", not "whether at all") rather than replacing it,
/// since most of this module only cares about direction once strafing is
/// already known to be active.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StrafePattern {
    #[default]
    Left,
    Right,
    Still,
}
impl StrafePattern {
    #[must_use]
    pub fn side(self) -> Option<StrafeSide> {
        match self {
            Self::Left => Some(StrafeSide::Left),
            Self::Right => Some(StrafeSide::Right),
            Self::Still => None,
        }
    }
}

/// Picks the next strafe pattern when the flip timer elapses: mostly
/// alternates sides as before, but rolls [`STILL_CHANCE`] to pause instead
/// -- never two `Still` intervals in a row (this only rolls for stillness
/// when `current` wasn't already still), so "never remain stationary for
/// long periods" still holds; a pause is one interval long, not
/// indefinite.
pub fn next_strafe_pattern(current: StrafePattern, rng: &mut SeededRng) -> StrafePattern {
    let next_side = match current {
        StrafePattern::Left => StrafeSide::Left.flipped(),
        StrafePattern::Right => StrafeSide::Right.flipped(),
        StrafePattern::Still => StrafeSide::Left,
    };
    if current != StrafePattern::Still && rng.chance(STILL_CHANCE) {
        return StrafePattern::Still;
    }
    match next_side {
        StrafeSide::Left => StrafePattern::Left,
        StrafeSide::Right => StrafePattern::Right,
    }
}

/// What the bot's feet should be doing this tick. Translated into an
/// actual walk/sprint call by `crate::combat::executor` -- kept as plain
/// booleans here (rather than Azalea's `WalkDirection`) so this stays
/// testable without a live client, matching every other pure module in
/// this crate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MovementDecision {
    pub forward: bool,
    pub backward: bool,
    pub strafe: Option<StrafeSide>,
    pub sprint: bool,
}

/// Direction to walk this tick, mirroring Azalea's own `WalkDirection`
/// one-to-one (see `MinecraftClient::set_combat_walk`) but defined here,
/// not in `crate::minecraft`, so this module -- and every pure test in it
/// -- never needs to depend on Azalea types, matching
/// `crate::minecraft::client`'s own module doc comment ("Azalea types are
/// deliberately kept in this module. Callers observe only our connection
/// state and application errors.").
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CombatWalk {
    #[default]
    None,
    Forward,
    Backward,
    Left,
    Right,
    ForwardLeft,
    ForwardRight,
    BackwardLeft,
    BackwardRight,
}

impl MovementDecision {
    /// Collapses the independent forward/backward/strafe fields into the
    /// single combined direction `MinecraftClient::set_combat_walk` needs.
    /// `decide_movement` never sets both `forward` and `backward` at once,
    /// but this resolves forward-wins rather than panicking if a future
    /// caller ever does.
    #[must_use]
    pub fn walk_direction(self) -> CombatWalk {
        match (self.forward, self.backward, self.strafe) {
            (true, _, Some(StrafeSide::Left)) => CombatWalk::ForwardLeft,
            (true, _, Some(StrafeSide::Right)) => CombatWalk::ForwardRight,
            (true, _, None) => CombatWalk::Forward,
            (false, true, Some(StrafeSide::Left)) => CombatWalk::BackwardLeft,
            (false, true, Some(StrafeSide::Right)) => CombatWalk::BackwardRight,
            (false, true, None) => CombatWalk::Backward,
            (false, false, Some(StrafeSide::Left)) => CombatWalk::Left,
            (false, false, Some(StrafeSide::Right)) => CombatWalk::Right,
            (false, false, None) => CombatWalk::None,
        }
    }
}

/// Core combat positioning policy -- full aggression, so only three bands,
/// and `backward` is never set at all:
///
/// - `> AGGRESSIVE_CLOSE_DISTANCE`: close the gap at a sprint.
/// - `PREFERRED_MAX_DISTANCE ..= AGGRESSIVE_CLOSE_DISTANCE`: close the gap
///   at a walk.
/// - `<= PREFERRED_MAX_DISTANCE`: hold and circle. Including *far* inside
///   the preferred band: the bot stays in the target's face rather than
///   stepping back out to a comfortable 1.7-1.9, since anything that walks
///   away from a target is an opening for them to eat, block, or run.
///
/// `pattern` supplies the strafe component in every band -- but per
/// [`StrafePattern::Still`], that component isn't always actually
/// strafing; "never remain stationary for *long* periods" (a `Still`
/// interval is always followed by an active one, see
/// [`next_strafe_pattern`]) rather than "never remain stationary at all".
pub fn decide_movement(distance_to_target: f64, pattern: StrafePattern) -> MovementDecision {
    let above_preferred = distance_to_target > PREFERRED_MAX_DISTANCE;
    let aggressive = distance_to_target > AGGRESSIVE_CLOSE_DISTANCE;
    MovementDecision {
        forward: above_preferred,
        backward: false,
        strafe: pattern.side(),
        sprint: aggressive,
    }
}

/// Whether the bot should jump right now to clear an obstacle in front of
/// it: walking forward, on solid ground, and physics reports a horizontal
/// collision (see `crate::minecraft::world_state::BotSnapshot::
/// horizontal_collision`'s doc comment). This bot's own raw combat
/// movement has no terrain awareness otherwise (see
/// `crate::combat::executor`'s module doc comment) -- without this it
/// would just push uselessly against a single-block-high obstacle forever
/// while chasing, never actually reaching a target on anything but
/// perfectly flat ground.
pub fn should_jump_over_obstacle(
    moving_forward: bool,
    on_ground: bool,
    horizontal_collision: bool,
) -> bool {
    moving_forward && on_ground && horizontal_collision
}

/// Whether the strafe side is due to flip, given how long it's held the
/// current side and a (possibly jittered) interval from
/// [`next_flip_interval`].
pub fn should_flip_strafe(held_for: Duration, flip_interval: Duration) -> bool {
    held_for >= flip_interval
}

/// Picks the next strafe-hold duration with a bit of randomness layered on
/// top of [`STRAFE_FLIP_INTERVAL`] -- see that constant's doc comment.
/// Never returns a negative/zero duration even at the extreme end of the
/// jitter range.
pub fn next_flip_interval(rng: &mut SeededRng) -> Duration {
    STRAFE_FLIP_INTERVAL + Duration::from_millis((rng.signed(1.0) * jitter_millis()).abs() as u64)
}
fn jitter_millis() -> f64 {
    STRAFE_FLIP_JITTER.as_millis() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggressively_closes_distance_and_sprints_past_the_aggressive_threshold() {
        let decision = decide_movement(AGGRESSIVE_CLOSE_DISTANCE + 0.1, StrafePattern::Left);
        assert!(decision.forward);
        assert!(!decision.backward);
        assert!(decision.sprint);
    }

    #[test]
    fn gently_closes_distance_without_sprinting_just_past_the_preferred_band() {
        let decision = decide_movement(PREFERRED_MAX_DISTANCE + 0.1, StrafePattern::Left);
        assert!(decision.forward);
        assert!(!decision.backward);
        assert!(!decision.sprint);
    }

    #[test]
    fn never_backs_off_however_close_the_target_is() {
        for distance in [0.0, 0.5, BACK_OFF_DISTANCE - 0.2, BACK_OFF_DISTANCE, 1.6] {
            let decision = decide_movement(distance, StrafePattern::Right);
            assert!(
                !decision.backward,
                "full aggression must never retreat (distance {distance})"
            );
            assert!(!decision.forward);
            assert!(!decision.sprint);
            assert_eq!(decision.strafe, Some(StrafeSide::Right));
        }
    }

    #[test]
    fn holds_position_and_strafes_up_to_the_preferred_maximum() {
        let decision = decide_movement(PREFERRED_MAX_DISTANCE, StrafePattern::Left);
        assert!(!decision.forward);
        assert!(!decision.backward);
        assert!(!decision.sprint);
    }

    #[test]
    fn an_active_strafe_pattern_is_never_overridden_by_distance() {
        for distance in [0.2, 1.0, 1.75, 2.5, 10.0] {
            let decision = decide_movement(distance, StrafePattern::Right);
            assert_eq!(decision.strafe, Some(StrafeSide::Right));
        }
    }

    #[test]
    fn a_still_pattern_never_strafes_regardless_of_distance() {
        for distance in [0.2, 1.0, 1.75, 2.5, 10.0] {
            let decision = decide_movement(distance, StrafePattern::Still);
            assert_eq!(decision.strafe, None);
        }
    }

    #[test]
    fn strafe_side_flips_to_its_opposite_and_back() {
        assert_eq!(StrafeSide::Left.flipped(), StrafeSide::Right);
        assert_eq!(StrafeSide::Right.flipped(), StrafeSide::Left);
        assert_eq!(StrafeSide::Left.flipped().flipped(), StrafeSide::Left);
    }

    #[test]
    fn strafe_pattern_side_matches_active_sides_and_is_none_when_still() {
        assert_eq!(StrafePattern::Left.side(), Some(StrafeSide::Left));
        assert_eq!(StrafePattern::Right.side(), Some(StrafeSide::Right));
        assert_eq!(StrafePattern::Still.side(), None);
    }

    #[test]
    fn next_strafe_pattern_never_stays_still_two_intervals_in_a_row() {
        let mut rng = SeededRng::new(11);
        let mut current = StrafePattern::Still;
        for _ in 0..200 {
            let next = next_strafe_pattern(current, &mut rng);
            if current == StrafePattern::Still {
                assert_ne!(
                    next,
                    StrafePattern::Still,
                    "must not chain two Still intervals"
                );
            }
            current = next;
        }
    }

    #[test]
    fn next_strafe_pattern_sometimes_pauses_and_sometimes_stays_active() {
        // Over enough rolls, both outcomes should show up -- this is what
        // actually distinguishes "don't always strafe" from the old
        // unconditional alternation.
        let mut rng = SeededRng::new(42);
        let mut current = StrafePattern::Left;
        let mut saw_still = false;
        let mut saw_active = false;
        for _ in 0..200 {
            current = next_strafe_pattern(current, &mut rng);
            if current == StrafePattern::Still {
                saw_still = true;
            } else {
                saw_active = true;
            }
        }
        assert!(saw_still, "should pause sometimes");
        assert!(saw_active, "should still actively strafe sometimes");
    }

    #[test]
    fn next_strafe_pattern_alternates_sides_when_continuing_to_strafe() {
        // With `STILL_CHANCE` never triggering (deterministically skipped by
        // never calling this from `Still`, and checking only the
        // non-`Still` outputs), consecutive active patterns should differ
        // in side whenever both are active.
        let mut rng = SeededRng::new(3);
        let mut current = StrafePattern::Left;
        for _ in 0..200 {
            let next = next_strafe_pattern(current, &mut rng);
            if current != StrafePattern::Still && next != StrafePattern::Still {
                assert_ne!(
                    current, next,
                    "an active-to-active transition must flip sides"
                );
            }
            current = next;
        }
    }

    #[test]
    fn jumps_over_an_obstacle_only_while_moving_forward_grounded_and_blocked() {
        assert!(should_jump_over_obstacle(true, true, true));
    }

    #[test]
    fn does_not_jump_when_not_actually_moving_forward() {
        assert!(!should_jump_over_obstacle(false, true, true));
    }

    #[test]
    fn does_not_jump_again_while_already_airborne() {
        assert!(!should_jump_over_obstacle(true, false, true));
    }

    #[test]
    fn does_not_jump_when_nothing_is_blocking_the_way() {
        assert!(!should_jump_over_obstacle(true, true, false));
    }

    #[test]
    fn flip_is_due_once_the_hold_time_reaches_the_interval() {
        let interval = Duration::from_millis(700);
        assert!(!should_flip_strafe(Duration::from_millis(699), interval));
        assert!(should_flip_strafe(Duration::from_millis(700), interval));
        assert!(should_flip_strafe(Duration::from_secs(5), interval));
    }

    #[test]
    fn walk_direction_combines_approach_and_strafe() {
        assert_eq!(
            MovementDecision {
                forward: true,
                backward: false,
                strafe: Some(StrafeSide::Left),
                sprint: true,
            }
            .walk_direction(),
            CombatWalk::ForwardLeft
        );
        assert_eq!(
            MovementDecision {
                forward: false,
                backward: true,
                strafe: Some(StrafeSide::Right),
                sprint: false,
            }
            .walk_direction(),
            CombatWalk::BackwardRight
        );
        assert_eq!(
            MovementDecision {
                forward: false,
                backward: false,
                strafe: Some(StrafeSide::Left),
                sprint: false,
            }
            .walk_direction(),
            CombatWalk::Left
        );
        assert_eq!(
            MovementDecision {
                forward: false,
                backward: false,
                strafe: None,
                sprint: false,
            }
            .walk_direction(),
            CombatWalk::None
        );
    }

    #[test]
    fn next_flip_interval_stays_within_the_jittered_bounds() {
        let mut rng = SeededRng::new(7);
        for _ in 0..50 {
            let interval = next_flip_interval(&mut rng);
            assert!(interval >= STRAFE_FLIP_INTERVAL);
            assert!(interval <= STRAFE_FLIP_INTERVAL + STRAFE_FLIP_JITTER);
        }
    }
}
