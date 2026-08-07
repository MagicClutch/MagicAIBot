//! Continuous steering: where the bot *wants* to be moving this instant, as
//! a world-space vector, and how that vector becomes the eight-way input
//! Minecraft actually accepts. Pure -- no Azalea types, no I/O, no clocks.
//!
//! # The one hard constraint
//!
//! Minecraft has no continuous movement input. A player has WASD, which is
//! eight directions relative to wherever the camera is pointing, and nothing
//! finer. So "continuous steering" cannot mean sending a velocity vector to
//! the server; it means computing one *here*, in floating-point world
//! coordinates, and projecting it onto those eight keys every tick
//! ([`project_to_walk`]).
//!
//! That is not a workaround, it is what a human does: the smooth arc a real
//! player traces comes from a continuously-rotating camera plus key
//! combinations that change several times a second, not from analogue input.
//! Feeding the projection a desired vector that turns smoothly (see
//! `crate::combat::movement::smoothing`) reproduces the arc; feeding it a
//! vector recomputed from scratch each tick reproduces the robot.
//!
//! # What the vector is made of
//!
//! Four contributions, summed and clamped:
//!
//! - **radial** -- close the gap, or ease off inside the preferred band.
//! - **tangential** -- orbit the target, the component that makes the bot
//!   circle rather than bulldoze straight in.
//! - **separation** -- a short-range shove outward so the bot never ends up
//!   standing inside the opponent's hitbox.
//! - **avoidance** -- whatever the local obstacle probe says to stay away
//!   from (see `crate::combat::movement::controller`).

use crate::minecraft::world_state::PositionSnapshot;

/// A horizontal vector. Combat movement is a 2D problem -- vertical motion
/// is jumping, which is a separate output -- so nothing here carries a Y.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub z: f64,
}

impl Vec2 {
    #[must_use]
    pub fn new(x: f64, z: f64) -> Self {
        Self { x, z }
    }

    #[must_use]
    pub fn zero() -> Self {
        Self { x: 0.0, z: 0.0 }
    }

    /// The horizontal part of a world position.
    #[must_use]
    pub fn of(position: PositionSnapshot) -> Self {
        Self {
            x: position.x,
            z: position.z,
        }
    }

    #[must_use]
    pub fn length(self) -> f64 {
        self.x.hypot(self.z)
    }

    #[must_use]
    pub fn is_negligible(self) -> bool {
        self.length() < 1e-6
    }

    /// Unit vector in the same direction, or zero if there is no direction
    /// to speak of.
    #[must_use]
    pub fn normalized(self) -> Self {
        let length = self.length();
        if length < 1e-9 {
            return Self::zero();
        }
        Self {
            x: self.x / length,
            z: self.z / length,
        }
    }

    #[must_use]
    pub fn scaled(self, factor: f64) -> Self {
        Self {
            x: self.x * factor,
            z: self.z * factor,
        }
    }

    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            z: self.z + other.z,
        }
    }

    #[must_use]
    pub fn minus(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            z: self.z - other.z,
        }
    }

    #[must_use]
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.z * other.z
    }

    /// Rotated a quarter turn. Which way in world terms doesn't matter --
    /// callers pick an orbit direction by sign -- only that it is
    /// consistently perpendicular.
    #[must_use]
    pub fn perpendicular(self) -> Self {
        Self {
            x: -self.z,
            z: self.x,
        }
    }

    /// Shortens the vector to `limit` if it is longer, leaving direction
    /// alone.
    #[must_use]
    pub fn clamped(self, limit: f64) -> Self {
        let length = self.length();
        if length <= limit || length < 1e-9 {
            return self;
        }
        self.scaled(limit / length)
    }

    /// Minecraft yaw (degrees) of this direction, matching the convention
    /// used everywhere else in this codebase (see
    /// `movement::movement_service`'s own travel-yaw computation):
    /// `atan2(-x, z)`, so yaw 0 faces +Z.
    #[must_use]
    pub fn yaw_degrees(self) -> f32 {
        (-self.x).atan2(self.z).to_degrees() as f32
    }
}

/// The distance policy the steering works to, in blocks.
#[derive(Clone, Copy, Debug)]
pub struct DistanceBand {
    /// Below this the bot is crowding the target and gets pushed out.
    pub too_close: f64,
    /// Bottom of the band the bot tries to hold.
    pub preferred_min: f64,
    /// Top of the band the bot tries to hold.
    pub preferred_max: f64,
    /// Past this, closing the gap dominates everything else.
    pub chase: f64,
}

impl Default for DistanceBand {
    fn default() -> Self {
        Self {
            too_close: 1.2,
            preferred_min: 1.7,
            preferred_max: 1.9,
            chase: 2.5,
        }
    }
}

/// Relative pull of each steering contribution. Tunable so the bot can be
/// made to orbit harder or commit straighter without touching the geometry.
#[derive(Clone, Copy, Debug)]
pub struct SteeringWeights {
    pub radial: f64,
    pub tangential: f64,
    pub separation: f64,
    pub avoidance: f64,
}

impl Default for SteeringWeights {
    fn default() -> Self {
        Self {
            radial: 1.0,
            tangential: 0.85,
            separation: 1.4,
            avoidance: 2.0,
        }
    }
}

/// Everything the steering needs to know about this instant.
#[derive(Clone, Copy, Debug)]
pub struct SteeringInput {
    pub bot: Vec2,
    /// Where the target is expected to be shortly, not where it is now --
    /// see `crate::combat::movement::prediction`.
    pub target: Vec2,
    /// +1.0 or -1.0: which way around the target to orbit. Zero holds a
    /// straight line in (a deliberate pause, see
    /// `crate::combat::movement::strafing`).
    pub orbit_sign: f64,
    /// Unit-ish push away from nearby obstacles and hazards, already
    /// weighted by urgency by the caller.
    pub avoidance: Vec2,
}

/// The steering vector for this tick: direction to move, with a magnitude
/// between 0 and 1 standing in for "how much of the available speed to
/// spend".
///
/// Magnitude matters because Minecraft has no throttle: the controller
/// spends it on *choices* -- whether to sprint, whether to hold the key at
/// all -- rather than on analogue speed.
#[must_use]
pub fn desired_velocity(
    input: SteeringInput,
    band: &DistanceBand,
    weights: &SteeringWeights,
) -> Vec2 {
    let to_target = input.target.minus(input.bot);
    let distance = to_target.length();
    if distance < 1e-6 {
        // Standing exactly inside the target: any direction is an
        // improvement, and the avoidance push is the only meaningful signal.
        return input.avoidance.scaled(weights.avoidance).clamped(1.0);
    }
    let inward = to_target.normalized();

    // Radial: full commitment past the chase distance, easing to zero across
    // the preferred band, and mildly negative when crowding. The negative
    // part is deliberately weak -- the bot is not meant to back away, it is
    // meant to let the orbit component carry it around (see this module's
    // doc comment and `crate::combat`'s full-aggression policy).
    let radial_scale = if distance > band.chase {
        1.0
    } else if distance > band.preferred_max {
        let span = (band.chase - band.preferred_max).max(1e-6);
        0.35 + 0.65 * ((distance - band.preferred_max) / span)
    } else if distance >= band.preferred_min {
        0.0
    } else if distance >= band.too_close {
        let span = (band.preferred_min - band.too_close).max(1e-6);
        -0.15 * ((band.preferred_min - distance) / span)
    } else {
        -0.35
    };

    // Tangential: strongest in and around the band, tapering off while
    // sprinting in from range, where circling only widens the gap.
    let tangential_scale = if distance > band.chase {
        0.15
    } else if distance > band.preferred_max {
        0.55
    } else {
        1.0
    };
    let tangential = inward
        .perpendicular()
        .scaled(input.orbit_sign.clamp(-1.0, 1.0) * tangential_scale * weights.tangential);

    // Separation: a hard shove that only exists at knife range, so the bot
    // never settles inside the opponent's hitbox.
    let separation = if distance < band.too_close {
        let urgency = ((band.too_close - distance) / band.too_close.max(1e-6)).clamp(0.0, 1.0);
        inward.scaled(-urgency * weights.separation)
    } else {
        Vec2::zero()
    };

    inward
        .scaled(radial_scale * weights.radial)
        .plus(tangential)
        .plus(separation)
        .plus(input.avoidance.scaled(weights.avoidance))
        .clamped(1.0)
}

/// The eight-way key combination this codebase speaks, mirroring Azalea's
/// own `WalkDirection` (see `crate::minecraft::client`).
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

impl CombatWalk {
    /// Whether this direction has any forward component -- vanilla only
    /// sprints forward or forward-diagonally, so this is exactly the set of
    /// directions a sprint request can survive.
    #[must_use]
    pub fn allows_sprint(self) -> bool {
        matches!(self, Self::Forward | Self::ForwardLeft | Self::ForwardRight)
    }

    #[must_use]
    pub fn is_moving(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Signed yaw offset from the camera that this key combination actually
    /// travels along, in degrees. The inverse of [`project_to_walk`], and
    /// what lets the controller reason about how much direction error the
    /// projection introduced.
    #[must_use]
    pub fn offset_degrees(self) -> Option<f64> {
        Some(match self {
            Self::None => return None,
            Self::Forward => 0.0,
            Self::ForwardRight => 45.0,
            Self::Right => 90.0,
            Self::BackwardRight => 135.0,
            Self::Backward => 180.0,
            Self::BackwardLeft => -135.0,
            Self::Left => -90.0,
            Self::ForwardLeft => -45.0,
        })
    }
}

/// Shortest signed difference between two yaws, in degrees. Positive means
/// `to` is to the right of `from` -- the same convention
/// `movement::multitasking::shortest_yaw_delta` uses.
#[must_use]
pub fn shortest_yaw_delta(from: f32, to: f32) -> f64 {
    f64::from((to - from + 180.0).rem_euclid(360.0) - 180.0)
}

/// Half-width of each of the eight direction sectors.
const SECTOR_HALF_WIDTH: f64 = 22.5;

/// Projects a world-space steering vector onto the eight keys, given where
/// the camera is pointing.
///
/// `previous` and `hysteresis` together stop the bot from stuttering between
/// two adjacent key combinations when the desired direction sits exactly on
/// a sector boundary: the new direction has to win by `hysteresis` degrees
/// before the keys change. Without it a vector hovering at 22.5 degrees
/// flickers between forward and forward-right every tick, which both looks
/// wrong and actually slows the bot down.
///
/// `throttle_floor` is the magnitude below which the bot simply stops rather
/// than shuffling: a steering vector that has nearly cancelled itself out
/// means "hold this position", not "creep in a random direction".
#[must_use]
pub fn project_to_walk(
    desired: Vec2,
    camera_yaw: f32,
    previous: CombatWalk,
    hysteresis: f64,
    throttle_floor: f64,
) -> CombatWalk {
    if desired.length() < throttle_floor {
        return CombatWalk::None;
    }
    let error = shortest_yaw_delta(camera_yaw, desired.yaw_degrees());
    let candidate = sector_for(error);
    // Keep the previous keys unless the new sector is a clear win.
    if previous.is_moving()
        && previous != candidate
        && let Some(previous_offset) = previous.offset_degrees()
    {
        let previous_error = (error - previous_offset).abs().min(
            // Wrapping: -180 and 180 are the same direction.
            (error - previous_offset).abs().mul_add(-1.0, 360.0),
        );
        let candidate_error = (error - candidate.offset_degrees().unwrap_or(0.0)).abs();
        if previous_error - candidate_error < hysteresis {
            return previous;
        }
    }
    candidate
}

fn sector_for(error: f64) -> CombatWalk {
    let magnitude = error.abs();
    let right = error > 0.0;
    if magnitude <= SECTOR_HALF_WIDTH {
        CombatWalk::Forward
    } else if magnitude <= SECTOR_HALF_WIDTH * 3.0 {
        if right {
            CombatWalk::ForwardRight
        } else {
            CombatWalk::ForwardLeft
        }
    } else if magnitude <= SECTOR_HALF_WIDTH * 5.0 {
        if right {
            CombatWalk::Right
        } else {
            CombatWalk::Left
        }
    } else if magnitude <= SECTOR_HALF_WIDTH * 7.0 {
        if right {
            CombatWalk::BackwardRight
        } else {
            CombatWalk::BackwardLeft
        }
    } else {
        CombatWalk::Backward
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(bot: Vec2, target: Vec2, orbit_sign: f64) -> SteeringInput {
        SteeringInput {
            bot,
            target,
            orbit_sign,
            avoidance: Vec2::zero(),
        }
    }

    #[test]
    fn a_distant_target_is_approached_at_full_commitment() {
        let desired = desired_velocity(
            input(Vec2::new(0.0, 0.0), Vec2::new(0.0, 20.0), 0.0),
            &DistanceBand::default(),
            &SteeringWeights::default(),
        );
        assert!(desired.z > 0.9, "should drive straight in: {desired:?}");
        assert!(desired.x.abs() < 0.2);
    }

    #[test]
    fn a_distant_target_still_gets_a_little_orbit_so_the_approach_is_not_a_straight_line() {
        let straight = desired_velocity(
            input(Vec2::new(0.0, 0.0), Vec2::new(0.0, 20.0), 0.0),
            &DistanceBand::default(),
            &SteeringWeights::default(),
        );
        let curving = desired_velocity(
            input(Vec2::new(0.0, 0.0), Vec2::new(0.0, 20.0), 1.0),
            &DistanceBand::default(),
            &SteeringWeights::default(),
        );
        assert!(curving.x.abs() > straight.x.abs());
    }

    #[test]
    fn inside_the_preferred_band_the_movement_is_pure_orbit() {
        let band = DistanceBand::default();
        let distance = (band.preferred_min + band.preferred_max) / 2.0;
        let desired = desired_velocity(
            input(Vec2::new(0.0, 0.0), Vec2::new(0.0, distance), 1.0),
            &band,
            &SteeringWeights::default(),
        );
        // Target is straight ahead (+Z), so a pure orbit is entirely
        // sideways: no radial component left.
        assert!(desired.z.abs() < 1e-6, "no closing component: {desired:?}");
        assert!(desired.x.abs() > 0.5, "should be circling: {desired:?}");
    }

    #[test]
    fn reversing_the_orbit_sign_reverses_the_circle() {
        let band = DistanceBand::default();
        let distance = (band.preferred_min + band.preferred_max) / 2.0;
        let left = desired_velocity(
            input(Vec2::new(0.0, 0.0), Vec2::new(0.0, distance), 1.0),
            &band,
            &SteeringWeights::default(),
        );
        let right = desired_velocity(
            input(Vec2::new(0.0, 0.0), Vec2::new(0.0, distance), -1.0),
            &band,
            &SteeringWeights::default(),
        );
        assert!(
            (left.x + right.x).abs() < 1e-9,
            "mirrored: {left:?} {right:?}"
        );
        assert!(left.x.abs() > 0.1);
    }

    #[test]
    fn crowding_the_target_circles_out_rather_than_walking_backwards() {
        let band = DistanceBand::default();
        // Just inside the crowding threshold: the bot is too close, but not
        // stuck inside the hitbox.
        let desired = desired_velocity(
            input(Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), 1.0),
            &band,
            &SteeringWeights::default(),
        );
        assert!(desired.z < 0.0, "must not keep pressing in: {desired:?}");
        assert!(
            desired.x.abs() > desired.z.abs(),
            "orbit should dominate a mild crowd: {desired:?}"
        );
    }

    #[test]
    fn standing_in_the_hitbox_prioritises_getting_out_over_circling() {
        let band = DistanceBand::default();
        let desired = desired_velocity(
            input(Vec2::new(0.0, 0.0), Vec2::new(0.0, 0.4), 1.0),
            &band,
            &SteeringWeights::default(),
        );
        assert!(
            desired.z.abs() > desired.x.abs(),
            "at knife range the shove has to win: {desired:?}"
        );
    }

    #[test]
    fn standing_exactly_on_the_target_still_produces_a_direction() {
        let mut steering = input(Vec2::new(5.0, 5.0), Vec2::new(5.0, 5.0), 1.0);
        steering.avoidance = Vec2::new(1.0, 0.0);
        let desired = desired_velocity(
            steering,
            &DistanceBand::default(),
            &SteeringWeights::default(),
        );
        assert!(!desired.is_negligible());
    }

    #[test]
    fn avoidance_bends_the_approach_away_from_the_obstacle() {
        let band = DistanceBand::default();
        let mut steering = input(Vec2::new(0.0, 0.0), Vec2::new(0.0, 10.0), 0.0);
        let clean = desired_velocity(steering, &band, &SteeringWeights::default());
        steering.avoidance = Vec2::new(-1.0, 0.0);
        let avoiding = desired_velocity(steering, &band, &SteeringWeights::default());
        assert!(clean.x.abs() < 0.01);
        assert!(avoiding.x < -0.3, "should veer away: {avoiding:?}");
    }

    #[test]
    fn the_steering_vector_never_exceeds_full_throttle() {
        let band = DistanceBand::default();
        let mut steering = input(Vec2::new(0.0, 0.0), Vec2::new(0.0, 0.3), 1.0);
        steering.avoidance = Vec2::new(3.0, 3.0);
        let desired = desired_velocity(steering, &band, &SteeringWeights::default());
        assert!(desired.length() <= 1.0 + 1e-9);
    }

    #[test]
    fn yaw_matches_the_conventions_used_elsewhere_in_the_codebase() {
        // Yaw 0 faces +Z, and the codebase computes travel yaw as
        // atan2(-dx, dz) -- see `MovementService::update_local_input`.
        assert!((Vec2::new(0.0, 1.0).yaw_degrees() - 0.0).abs() < 1e-6);
        assert!((Vec2::new(-1.0, 0.0).yaw_degrees() - 90.0).abs() < 1e-6);
        assert!((Vec2::new(1.0, 0.0).yaw_degrees() + 90.0).abs() < 1e-6);
    }

    #[test]
    fn a_vector_straight_ahead_projects_to_forward() {
        let walk = project_to_walk(Vec2::new(0.0, 1.0), 0.0, CombatWalk::None, 0.0, 0.05);
        assert_eq!(walk, CombatWalk::Forward);
    }

    #[test]
    fn a_diagonal_vector_projects_to_a_diagonal_key_combination() {
        // 45 degrees to the camera's right.
        let walk = project_to_walk(Vec2::new(-1.0, 1.0), 0.0, CombatWalk::None, 0.0, 0.05);
        assert_eq!(walk, CombatWalk::ForwardRight);
        let walk = project_to_walk(Vec2::new(1.0, 1.0), 0.0, CombatWalk::None, 0.0, 0.05);
        assert_eq!(walk, CombatWalk::ForwardLeft);
    }

    #[test]
    fn a_sideways_vector_projects_to_a_pure_strafe() {
        let walk = project_to_walk(Vec2::new(-1.0, 0.0), 0.0, CombatWalk::None, 0.0, 0.05);
        assert_eq!(walk, CombatWalk::Right);
        let walk = project_to_walk(Vec2::new(1.0, 0.0), 0.0, CombatWalk::None, 0.0, 0.05);
        assert_eq!(walk, CombatWalk::Left);
    }

    #[test]
    fn the_camera_yaw_rotates_the_whole_projection() {
        // Facing +X (yaw -90): a world vector pointing at +X is now
        // "forward" rather than "left".
        let walk = project_to_walk(Vec2::new(1.0, 0.0), -90.0, CombatWalk::None, 0.0, 0.05);
        assert_eq!(walk, CombatWalk::Forward);
    }

    #[test]
    fn a_nearly_cancelled_vector_stops_instead_of_shuffling() {
        let walk = project_to_walk(Vec2::new(0.01, 0.01), 0.0, CombatWalk::Forward, 0.0, 0.05);
        assert_eq!(walk, CombatWalk::None);
    }

    #[test]
    fn hysteresis_holds_the_previous_keys_across_a_sector_boundary() {
        // Just past the forward/forward-right boundary (24 degrees):
        // without hysteresis this flips every tick as the angle jitters.
        let boundary = Vec2::new(-0.45, 1.0);
        let free = project_to_walk(boundary, 0.0, CombatWalk::Forward, 0.0, 0.05);
        let sticky = project_to_walk(boundary, 0.0, CombatWalk::Forward, 12.0, 0.05);
        assert_eq!(free, CombatWalk::ForwardRight);
        assert_eq!(
            sticky,
            CombatWalk::Forward,
            "should not flap at the boundary"
        );
    }

    #[test]
    fn hysteresis_still_yields_to_a_decisive_direction_change() {
        let walk = project_to_walk(Vec2::new(1.0, 0.0), 0.0, CombatWalk::Forward, 12.0, 0.05);
        assert_eq!(walk, CombatWalk::Left, "a 90 degree change must win");
    }

    #[test]
    fn only_forward_directions_admit_a_sprint() {
        assert!(CombatWalk::Forward.allows_sprint());
        assert!(CombatWalk::ForwardLeft.allows_sprint());
        assert!(CombatWalk::ForwardRight.allows_sprint());
        assert!(!CombatWalk::Left.allows_sprint());
        assert!(!CombatWalk::Backward.allows_sprint());
        assert!(!CombatWalk::None.allows_sprint());
    }

    #[test]
    fn every_direction_round_trips_through_its_own_offset() {
        for walk in [
            CombatWalk::Forward,
            CombatWalk::ForwardRight,
            CombatWalk::Right,
            CombatWalk::BackwardRight,
            CombatWalk::Backward,
            CombatWalk::BackwardLeft,
            CombatWalk::Left,
            CombatWalk::ForwardLeft,
        ] {
            let offset = walk.offset_degrees().unwrap();
            assert_eq!(sector_for(offset), walk, "{walk:?} at {offset}");
        }
    }
}
