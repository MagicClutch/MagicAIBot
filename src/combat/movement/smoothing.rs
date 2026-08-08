//! Momentum and curvature: the difference between a bot that snaps to a new
//! direction the instant the maths changes, and one that leans into a turn.
//! Pure -- takes the previous steering vector, the new one, and how much
//! time passed.
//!
//! # Why this is the module that makes it look human
//!
//! The steering vector can change direction arbitrarily fast -- the target
//! sidesteps, the orbit flips, an obstacle appears -- and if that raw vector
//! is projected straight onto the keys, the result is a bot that switches
//! from `ForwardLeft` to `BackwardRight` between two ticks. Real players
//! cannot do that: a hand takes time on the keys, and a body carries
//! momentum through the change.
//!
//! [`SteeringSmoother`] imposes both limits:
//!
//! - **Turn rate** caps how many degrees the desired direction may rotate
//!   per second, so a 180-degree reversal becomes a curve with a radius
//!   rather than a pivot. This is what produces corner-cutting: the bot
//!   starts turning before it reaches the point it was heading for, because
//!   the direction is already rotating toward the next one.
//! - **Throttle slew** caps how fast the magnitude may change, so
//!   acceleration and braking are gradual instead of on/off.

use std::time::Duration;

use crate::combat::movement::steering::Vec2;

/// How quickly the desired direction and throttle are allowed to change.
#[derive(Clone, Copy, Debug)]
pub struct SmoothingLimits {
    /// Maximum rotation of the steering direction, degrees per second.
    ///
    /// Tuned to be quick enough that the bot doesn't feel sluggish reacting
    /// to a sidestep, slow enough that a full reversal takes a noticeable
    /// arc. A real player's direction changes are limited by their hands and
    /// their momentum, not by physics alone.
    pub turn_rate_degrees: f64,
    /// Maximum change in throttle (0-1) per second.
    pub throttle_rate: f64,
    /// Below this angle the turn limit is skipped entirely -- fine
    /// corrections should land immediately rather than lag a frame behind.
    pub instant_below_degrees: f64,
}

impl Default for SmoothingLimits {
    fn default() -> Self {
        Self {
            turn_rate_degrees: 720.0,
            throttle_rate: 4.0,
            instant_below_degrees: 8.0,
        }
    }
}

/// Carries the smoothed steering vector between ticks.
#[derive(Clone, Copy, Debug, Default)]
pub struct SteeringSmoother {
    current: Vec2,
}

impl SteeringSmoother {
    #[must_use]
    pub fn new() -> Self {
        Self {
            current: Vec2::zero(),
        }
    }

    /// The vector as of the last [`Self::advance`].
    #[must_use]
    pub fn current(&self) -> Vec2 {
        self.current
    }

    /// Forgets all momentum -- used when a fight starts, so the first tick
    /// of a new fight doesn't inherit the last one's direction.
    pub fn reset(&mut self) {
        self.current = Vec2::zero();
    }

    /// Advances the smoothed vector toward `desired` by at most what
    /// `limits` allows in `elapsed`.
    pub fn advance(&mut self, desired: Vec2, limits: &SmoothingLimits, elapsed: Duration) -> Vec2 {
        let seconds = elapsed.as_secs_f64().clamp(0.0, 0.25);
        let desired_length = desired.length();

        // From a standstill there is no direction to rotate away from, so
        // the first vector is adopted outright.
        if self.current.is_negligible() {
            self.current = desired.clamped(throttle_step(0.0, desired_length, limits, seconds));
            return self.current;
        }
        if desired.is_negligible() {
            // Releasing the keys: bleed the throttle down rather than
            // dropping it, so stopping is a deceleration, not a wall.
            let length = throttle_step(self.current.length(), 0.0, limits, seconds);
            self.current = if length < 1e-6 {
                Vec2::zero()
            } else {
                self.current.normalized().scaled(length)
            };
            return self.current;
        }

        let current_direction = self.current.normalized();
        let desired_direction = desired.normalized();
        let angle = angle_between_degrees(current_direction, desired_direction);
        let allowance = limits.turn_rate_degrees * seconds;
        let direction = if angle <= limits.instant_below_degrees || angle <= allowance {
            desired_direction
        } else {
            rotate_towards(current_direction, desired_direction, allowance)
        };
        let length = throttle_step(self.current.length(), desired_length, limits, seconds);
        self.current = direction.scaled(length);
        self.current
    }
}

fn throttle_step(current: f64, desired: f64, limits: &SmoothingLimits, seconds: f64) -> f64 {
    let allowance = limits.throttle_rate * seconds;
    let delta = (desired - current).clamp(-allowance, allowance);
    (current + delta).clamp(0.0, 1.0)
}

/// Unsigned angle between two unit vectors, in degrees.
#[must_use]
pub fn angle_between_degrees(from: Vec2, to: Vec2) -> f64 {
    let dot = from.dot(to).clamp(-1.0, 1.0);
    dot.acos().to_degrees()
}

/// Rotates `from` toward `to` by at most `limit` degrees.
#[must_use]
pub fn rotate_towards(from: Vec2, to: Vec2, limit: f64) -> Vec2 {
    let angle = angle_between_degrees(from, to);
    if angle <= limit || angle < 1e-9 {
        return to;
    }
    // Sign of the shortest rotation: the perpendicular of `from` points to
    // one side, and the sign of its dot with `to` says which.
    let sign = if from.perpendicular().dot(to) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    let radians = limit.to_radians() * sign;
    let (sin, cos) = radians.sin_cos();
    Vec2::new(from.x * cos - from.z * sin, from.x * sin + from.z * cos).normalized()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_millis(50);

    fn limits() -> SmoothingLimits {
        SmoothingLimits::default()
    }

    #[test]
    fn a_smoother_starts_still() {
        let smoother = SteeringSmoother::new();
        assert!(smoother.current().is_negligible());
    }

    #[test]
    fn the_first_vector_is_adopted_directionally_but_the_throttle_ramps() {
        let mut smoother = SteeringSmoother::new();
        let out = smoother.advance(Vec2::new(0.0, 1.0), &limits(), TICK);
        assert!(
            out.x.abs() < 1e-9 && out.z > 0.0,
            "direction taken: {out:?}"
        );
        assert!(
            out.length() < 1.0,
            "throttle should ramp, not snap: {out:?}"
        );
    }

    #[test]
    fn the_throttle_reaches_full_over_several_ticks() {
        let mut smoother = SteeringSmoother::new();
        let mut last = 0.0;
        for _ in 0..10 {
            last = smoother
                .advance(Vec2::new(0.0, 1.0), &limits(), TICK)
                .length();
        }
        assert!((last - 1.0).abs() < 1e-6, "should be at full speed: {last}");
    }

    #[test]
    fn a_reversal_sweeps_around_instead_of_snapping() {
        let mut smoother = SteeringSmoother::new();
        for _ in 0..10 {
            smoother.advance(Vec2::new(0.0, 1.0), &limits(), TICK);
        }
        // Ask for the exact opposite direction.
        let after_one_tick = smoother.advance(Vec2::new(0.0, -1.0), &limits(), TICK);
        let turned = angle_between_degrees(Vec2::new(0.0, 1.0), after_one_tick.normalized());
        assert!(
            turned <= limits().turn_rate_degrees * TICK.as_secs_f64() + 1e-6,
            "turned {turned} degrees in one tick"
        );
        assert!(turned > 0.0, "but it did start turning");
    }

    #[test]
    fn a_reversal_completes_within_a_realistic_number_of_ticks() {
        let mut smoother = SteeringSmoother::new();
        smoother.advance(Vec2::new(0.0, 1.0), &limits(), TICK);
        let mut ticks = 0;
        while angle_between_degrees(smoother.current().normalized(), Vec2::new(0.0, -1.0)) > 1.0 {
            smoother.advance(Vec2::new(0.0, -1.0), &limits(), TICK);
            ticks += 1;
            assert!(ticks < 40, "a reversal should not take forever");
        }
        assert!(ticks >= 2, "nor should it be instant: {ticks} ticks");
    }

    #[test]
    fn small_corrections_are_applied_immediately() {
        let mut smoother = SteeringSmoother::new();
        for _ in 0..10 {
            smoother.advance(Vec2::new(0.0, 1.0), &limits(), TICK);
        }
        // A five-degree nudge is inside `instant_below_degrees`.
        let nudge = Vec2::new(-0.087, 0.996);
        let out = smoother.advance(nudge, &limits(), TICK);
        let error = angle_between_degrees(out.normalized(), nudge.normalized());
        assert!(error < 1e-6, "fine aim corrections must not lag: {error}");
    }

    #[test]
    fn releasing_the_keys_decelerates_rather_than_stopping_dead() {
        let mut smoother = SteeringSmoother::new();
        for _ in 0..10 {
            smoother.advance(Vec2::new(0.0, 1.0), &limits(), TICK);
        }
        let out = smoother.advance(Vec2::zero(), &limits(), TICK);
        assert!(out.length() > 0.0, "momentum carries: {out:?}");
        assert!(out.length() < 1.0);
    }

    #[test]
    fn a_reset_drops_all_momentum() {
        let mut smoother = SteeringSmoother::new();
        smoother.advance(Vec2::new(0.0, 1.0), &limits(), TICK);
        smoother.reset();
        assert!(smoother.current().is_negligible());
    }

    #[test]
    fn a_long_stall_between_ticks_cannot_produce_an_enormous_jump() {
        let mut smoother = SteeringSmoother::new();
        smoother.advance(Vec2::new(0.0, 1.0), &limits(), TICK);
        // Ten seconds of lag: the clamp inside `advance` keeps this
        // equivalent to a quarter second of turning.
        let out = smoother.advance(Vec2::new(0.0, -1.0), &limits(), Duration::from_secs(10));
        assert!(out.length() <= 1.0);
    }

    #[test]
    fn rotate_towards_takes_the_short_way_around() {
        let from = Vec2::new(0.0, 1.0);
        let to = Vec2::new(1.0, 0.0);
        let stepped = rotate_towards(from, to, 30.0);
        assert!(
            angle_between_degrees(stepped, to) < angle_between_degrees(from, to),
            "must move toward the goal"
        );
        assert!(stepped.x > 0.0, "and on the correct side: {stepped:?}");

        let other = Vec2::new(-1.0, 0.0);
        let stepped = rotate_towards(from, other, 30.0);
        assert!(stepped.x < 0.0, "mirrored on the other side: {stepped:?}");
    }

    #[test]
    fn rotate_towards_arrives_exactly_when_within_the_limit() {
        let from = Vec2::new(0.0, 1.0);
        let to = Vec2::new(0.1, 1.0).normalized();
        assert_eq!(rotate_towards(from, to, 90.0), to);
    }

    #[test]
    fn the_angle_helper_agrees_with_known_angles() {
        assert!(angle_between_degrees(Vec2::new(0.0, 1.0), Vec2::new(0.0, 1.0)).abs() < 1e-9);
        assert!(
            (angle_between_degrees(Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0)) - 90.0).abs() < 1e-6
        );
        assert!(
            (angle_between_degrees(Vec2::new(0.0, 1.0), Vec2::new(0.0, -1.0)) - 180.0).abs() < 1e-6
        );
    }
}
