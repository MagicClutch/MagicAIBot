//! Where the target is *going*, from where it has been. Pure -- fed
//! positions and timestamps, returns a predicted point.
//!
//! Steering toward where an opponent currently stands is how a bot ends up
//! permanently one step behind a sprinting player: by the time the input
//! takes effect they have already moved. Steering toward where they will be
//! is what closing distance on a moving target actually requires.
//!
//! # Why this doesn't just use the velocity Azalea reports
//!
//! It partly does -- but a single velocity sample says nothing about whether
//! the target is holding a straight line or reversing, and those want very
//! different leads. A player sprinting in one direction is worth
//! extrapolating a long way; a player strafing back and forth is worth
//! barely extrapolating at all, because the lead will be pointing the wrong
//! way half the time.
//!
//! So [`TargetTracker`] keeps a short history and measures how *consistent*
//! recent motion has been ([`MotionSample::consistency`]). Steady motion
//! earns the full lead; erratic motion has its lead cut. That single term is
//! the difference between anticipating a runner and being juked by a
//! strafer.

use std::time::{Duration, Instant};

use crate::combat::movement::steering::Vec2;

/// How many position samples to keep. Roughly a third of a second at the
/// combat tick rate -- long enough to see a direction change, short enough
/// that it reacts within a couple of ticks.
const HISTORY: usize = 6;

/// Samples older than this are thrown away rather than blended: after a
/// teleport or a reacquisition, stale history is worse than none.
const MAX_SAMPLE_AGE: Duration = Duration::from_millis(600);

/// Hard cap on how far ahead of the target the predicted point may sit,
/// however fast the target is moving. A prediction further out than about a
/// body length is one the bot ends up chasing instead of the player.
pub const MAX_LEAD_BLOCKS: f64 = 1.75;

/// A measurement of how the target is moving right now.
#[derive(Clone, Copy, Debug, Default)]
pub struct MotionSample {
    /// Horizontal velocity in blocks per second.
    pub velocity: Vec2,
    /// 0.0-1.0: how consistently the target has been holding this
    /// direction. 1.0 is a dead-straight sprint, 0.0 is direction changes
    /// every sample.
    pub consistency: f64,
}

#[derive(Clone, Copy, Debug)]
struct Observation {
    position: Vec2,
    at: Instant,
}

/// Short history of a target's positions.
#[derive(Clone, Debug, Default)]
pub struct TargetTracker {
    samples: Vec<Observation>,
}

impl TargetTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            samples: Vec::with_capacity(HISTORY),
        }
    }

    pub fn reset(&mut self) {
        self.samples.clear();
    }

    /// Records where the target is now, dropping anything too old to trust.
    pub fn observe(&mut self, position: Vec2, at: Instant) {
        self.samples
            .retain(|sample| at.saturating_duration_since(sample.at) <= MAX_SAMPLE_AGE);
        // A sample that arrives at the same instant as the previous one
        // carries no motion information and would divide by zero below.
        if self
            .samples
            .last()
            .is_some_and(|last| at.saturating_duration_since(last.at) < Duration::from_millis(1))
        {
            return;
        }
        self.samples.push(Observation { position, at });
        if self.samples.len() > HISTORY {
            self.samples.remove(0);
        }
    }

    /// Velocity and consistency over the retained history. Zero velocity
    /// until there are at least two usable samples.
    #[must_use]
    pub fn motion(&self) -> MotionSample {
        if self.samples.len() < 2 {
            return MotionSample::default();
        }
        let first = self.samples[0];
        let last = self.samples[self.samples.len() - 1];
        let span = last.at.saturating_duration_since(first.at).as_secs_f64();
        if span < 1e-3 {
            return MotionSample::default();
        }
        let velocity = last.position.minus(first.position).scaled(1.0 / span);

        // Consistency: the net displacement over the window divided by the
        // total distance actually travelled. A straight line scores 1; a
        // player who strafes out and back covers ground without getting
        // anywhere and scores near 0.
        let travelled: f64 = self
            .samples
            .windows(2)
            .map(|pair| pair[1].position.minus(pair[0].position).length())
            .sum();
        let net = last.position.minus(first.position).length();
        let consistency = if travelled < 1e-6 {
            0.0
        } else {
            (net / travelled).clamp(0.0, 1.0)
        };
        MotionSample {
            velocity,
            consistency,
        }
    }

    /// Where to actually aim the movement: the target's position led by its
    /// motion, scaled by how much that motion can be trusted.
    ///
    /// `lead_seconds` is the full lead applied to perfectly steady motion;
    /// erratic motion gets proportionally less.
    #[must_use]
    pub fn predicted_position(&self, current: Vec2, lead_seconds: f64) -> Vec2 {
        let motion = self.motion();
        current.plus(lead_offset(motion, lead_seconds))
    }
}

/// How far ahead of a target moving like `motion` to aim, given a full lead
/// of `lead_seconds`.
#[must_use]
pub fn lead_offset(motion: MotionSample, lead_seconds: f64) -> Vec2 {
    if lead_seconds <= 0.0 {
        return Vec2::zero();
    }
    motion
        .velocity
        .scaled(lead_seconds * motion.consistency.clamp(0.0, 1.0))
        .clamped(MAX_LEAD_BLOCKS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: u64) -> Instant {
        // A fixed origin so the tests are deterministic relative to each
        // other; `Instant` has no public constructor, so this walks forward
        // from one taken at the start.
        origin() + Duration::from_millis(millis)
    }

    fn origin() -> Instant {
        // A single `Instant` shared by every call in a test run. Taking it
        // once keeps the arithmetic in `at` consistent.
        use std::sync::OnceLock;
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        *ORIGIN.get_or_init(Instant::now)
    }

    #[test]
    fn an_empty_tracker_reports_no_motion() {
        let tracker = TargetTracker::new();
        assert!(tracker.samples.is_empty());
        assert!(tracker.motion().velocity.is_negligible());
    }

    #[test]
    fn a_single_observation_is_not_enough_to_infer_motion() {
        let mut tracker = TargetTracker::new();
        tracker.observe(Vec2::new(0.0, 0.0), at(0));
        assert!(tracker.motion().velocity.is_negligible());
    }

    #[test]
    fn a_straight_sprint_reads_as_fast_and_perfectly_consistent() {
        let mut tracker = TargetTracker::new();
        // 5.6 blocks/s is roughly a sprinting player.
        for step in 0..5 {
            let millis = step * 50;
            tracker.observe(Vec2::new(0.0, 0.28 * step as f64), at(millis));
        }
        let motion = tracker.motion();
        assert!(
            (motion.velocity.length() - 5.6).abs() < 0.2,
            "speed {}",
            motion.velocity.length()
        );
        assert!(
            motion.consistency > 0.99,
            "consistency {}",
            motion.consistency
        );
        assert!(motion.velocity.z > 0.0);
    }

    #[test]
    fn a_player_strafing_back_and_forth_reads_as_inconsistent() {
        let mut tracker = TargetTracker::new();
        let path = [0.0, 0.3, 0.0, 0.3, 0.0, 0.3];
        for (step, x) in path.iter().enumerate() {
            tracker.observe(Vec2::new(*x, 0.0), at(step as u64 * 50));
        }
        let motion = tracker.motion();
        assert!(
            motion.consistency < 0.4,
            "juking should not read as steady: {}",
            motion.consistency
        );
    }

    #[test]
    fn the_lead_is_cut_for_erratic_motion_and_full_for_steady_motion() {
        let steady = MotionSample {
            velocity: Vec2::new(0.0, 5.6),
            consistency: 1.0,
        };
        let erratic = MotionSample {
            velocity: Vec2::new(0.0, 5.6),
            consistency: 0.1,
        };
        let steady_lead = lead_offset(steady, 0.25).length();
        let erratic_lead = lead_offset(erratic, 0.25).length();
        assert!(steady_lead > erratic_lead * 5.0);
        assert!(erratic_lead < 0.2, "barely lead a juking player");
    }

    #[test]
    fn the_lead_is_capped_however_fast_the_target_is_moving() {
        let rocket = MotionSample {
            velocity: Vec2::new(0.0, 400.0),
            consistency: 1.0,
        };
        assert!((lead_offset(rocket, 1.0).length() - MAX_LEAD_BLOCKS).abs() < 1e-9);
    }

    #[test]
    fn no_lead_is_applied_when_prediction_is_switched_off() {
        let motion = MotionSample {
            velocity: Vec2::new(0.0, 5.6),
            consistency: 1.0,
        };
        assert!(lead_offset(motion, 0.0).is_negligible());
    }

    #[test]
    fn the_predicted_point_sits_ahead_of_a_runner() {
        let mut tracker = TargetTracker::new();
        for step in 0..5 {
            tracker.observe(Vec2::new(0.0, 0.28 * step as f64), at(step * 50));
        }
        let current = Vec2::new(0.0, 1.12);
        let predicted = tracker.predicted_position(current, 0.25);
        assert!(predicted.z > current.z, "should lead the runner");
        assert!(predicted.z - current.z <= MAX_LEAD_BLOCKS + 1e-9);
    }

    #[test]
    fn a_stationary_target_is_predicted_exactly_where_it_stands() {
        let mut tracker = TargetTracker::new();
        for step in 0..5 {
            tracker.observe(Vec2::new(3.0, -2.0), at(step * 50));
        }
        let predicted = tracker.predicted_position(Vec2::new(3.0, -2.0), 0.25);
        assert!(predicted.minus(Vec2::new(3.0, -2.0)).is_negligible());
    }

    #[test]
    fn history_is_bounded_and_stale_samples_are_dropped() {
        let mut tracker = TargetTracker::new();
        for step in 0..50 {
            tracker.observe(Vec2::new(0.0, step as f64), at(step * 50));
        }
        assert!(tracker.samples.len() <= HISTORY);

        // A long gap (a reacquired target) leaves only the fresh sample.
        tracker.observe(Vec2::new(99.0, 99.0), at(50_000));
        assert_eq!(tracker.samples.len(), 1);
        assert!(tracker.motion().velocity.is_negligible());
    }

    #[test]
    fn duplicate_timestamps_are_ignored_rather_than_dividing_by_zero() {
        let mut tracker = TargetTracker::new();
        tracker.observe(Vec2::new(0.0, 0.0), at(0));
        tracker.observe(Vec2::new(5.0, 5.0), at(0));
        assert_eq!(tracker.samples.len(), 1);
        assert!(tracker.motion().velocity.length().is_finite());
    }

    #[test]
    fn a_reset_forgets_the_target_entirely() {
        let mut tracker = TargetTracker::new();
        tracker.observe(Vec2::new(0.0, 0.0), at(0));
        tracker.observe(Vec2::new(0.0, 1.0), at(50));
        tracker.reset();
        assert!(tracker.samples.is_empty());
        assert!(tracker.motion().velocity.is_negligible());
    }
}
