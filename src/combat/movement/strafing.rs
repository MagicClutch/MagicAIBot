//! Which way to circle, and for how long. Pure -- takes elapsed time and a
//! seeded RNG, returns a decision.
//!
//! Orbit *direction* is what the steering needs (see
//! `crate::combat::movement::steering`); this module owns the human part of
//! it: that a real player doesn't circle one way forever, doesn't reverse on
//! a metronome either, and occasionally plants their feet to line up a hit.
//!
//! The three things that keep it from reading as a bot:
//!
//! - **Jittered intervals.** Every hold is a different length, drawn around
//!   a base interval, so there is no period to lock onto.
//! - **Occasional stillness.** Sometimes the orbit stops for one interval
//!   instead of switching sides -- never twice running, so the bot is never
//!   a stationary target for long.
//! - **Combo pressure.** Right after landing a hit the intervals shorten,
//!   which is what a player does when they smell a kill.

use std::time::Duration;

use crate::look::aim_point::SeededRng;

/// Baseline hold time for one orbit direction, before jitter. Frequent
/// enough that the bot doesn't circle predictably for more than a third of a
/// second, infrequent enough that it doesn't read as vibration.
pub const BASE_INTERVAL: Duration = Duration::from_millis(280);
/// Extra time added on top of [`BASE_INTERVAL`], drawn per switch.
const INTERVAL_JITTER: Duration = Duration::from_millis(150);
/// Hold time used for the interval right after landing a hit -- shorter, so
/// a follow-up circles harder instead of settling back into the lazy
/// baseline mid-combo.
pub const COMBO_INTERVAL: Duration = Duration::from_millis(150);
/// Chance that a switch plants the feet for one interval instead of picking
/// the other side.
const STILL_CHANCE: f64 = 0.3;

/// Which way the bot is circling this interval.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OrbitDirection {
    #[default]
    Left,
    Right,
    /// A deliberate pause -- feet planted, no tangential component.
    Still,
}

impl OrbitDirection {
    /// Sign for the tangential steering component: +1, -1, or 0 while
    /// planted. See `steering::SteeringInput::orbit_sign`.
    #[must_use]
    pub fn sign(self) -> f64 {
        match self {
            Self::Left => 1.0,
            Self::Right => -1.0,
            Self::Still => 0.0,
        }
    }

    #[must_use]
    pub fn is_circling(self) -> bool {
        !matches!(self, Self::Still)
    }

    #[must_use]
    pub fn reversed(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Still => Self::Left,
        }
    }
}

/// Orbit direction plus the timer that decides when it changes.
#[derive(Clone, Debug)]
pub struct StrafePlanner {
    direction: OrbitDirection,
    /// Time accumulated on the current direction.
    held: Duration,
    /// How long the current direction is meant to last.
    interval: Duration,
}

impl Default for StrafePlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl StrafePlanner {
    #[must_use]
    pub fn new() -> Self {
        Self {
            direction: OrbitDirection::Left,
            held: Duration::ZERO,
            interval: BASE_INTERVAL,
        }
    }

    /// Restarts the pattern for a new fight, so the first interval isn't a
    /// leftover from the last one.
    pub fn reset(&mut self, rng: &mut SeededRng) {
        self.direction = if rng.chance(0.5) {
            OrbitDirection::Left
        } else {
            OrbitDirection::Right
        };
        self.held = Duration::ZERO;
        self.interval = next_interval(rng);
    }

    /// Shortens the current interval because a hit just landed -- the bot
    /// circles harder while the combo is live.
    pub fn note_attack(&mut self) {
        self.held = Duration::ZERO;
        self.interval = COMBO_INTERVAL;
    }

    /// Forces a switch on the next update regardless of the timer. Used when
    /// something external makes the current direction a bad idea -- circling
    /// into a wall or a ledge.
    pub fn force_switch(&mut self) {
        self.held = self.interval;
    }

    /// Advances the timer and returns the direction to orbit this tick.
    pub fn advance(&mut self, elapsed: Duration, rng: &mut SeededRng) -> OrbitDirection {
        self.held = self.held.saturating_add(elapsed);
        if self.held < self.interval {
            return self.direction;
        }
        self.held = Duration::ZERO;
        self.interval = next_interval(rng);
        // Never two planted intervals in a row: "don't always strafe" is
        // about being unpredictable, not about standing still.
        self.direction = if self.direction.is_circling() && rng.chance(STILL_CHANCE) {
            OrbitDirection::Still
        } else {
            self.direction.reversed()
        };
        self.direction
    }
}

/// A hold time drawn around [`BASE_INTERVAL`]. Never shorter than the base,
/// so the jitter only ever makes the bot less regular, not twitchier.
fn next_interval(rng: &mut SeededRng) -> Duration {
    let jitter = (rng.signed(1.0).abs() * INTERVAL_JITTER.as_millis() as f64) as u64;
    BASE_INTERVAL + Duration::from_millis(jitter)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_millis(50);

    fn rng() -> SeededRng {
        SeededRng::new(7)
    }

    #[test]
    fn the_signs_drive_the_steering_in_opposite_directions() {
        assert_eq!(OrbitDirection::Left.sign(), 1.0);
        assert_eq!(OrbitDirection::Right.sign(), -1.0);
        assert_eq!(OrbitDirection::Still.sign(), 0.0);
    }

    #[test]
    fn a_direction_is_held_for_its_whole_interval() {
        let mut planner = StrafePlanner::new();
        let mut rng = rng();
        let start = planner.advance(Duration::ZERO, &mut rng);
        // The base interval is 280ms: five 50ms ticks stay inside it.
        for _ in 0..5 {
            assert_eq!(planner.advance(TICK, &mut rng), start);
        }
    }

    #[test]
    fn the_direction_eventually_changes() {
        let mut planner = StrafePlanner::new();
        let mut rng = rng();
        let start = planner.advance(Duration::ZERO, &mut rng);
        let mut changed = false;
        for _ in 0..20 {
            if planner.advance(TICK, &mut rng) != start {
                changed = true;
                break;
            }
        }
        assert!(changed, "the orbit must not be one-directional");
    }

    #[test]
    fn intervals_vary_so_there_is_no_period_to_read() {
        let mut planner = StrafePlanner::new();
        let mut rng = rng();
        let mut lengths = Vec::new();
        let mut ticks_on_current = 0;
        let mut previous = planner.advance(Duration::ZERO, &mut rng);
        for _ in 0..400 {
            let direction = planner.advance(TICK, &mut rng);
            ticks_on_current += 1;
            if direction != previous {
                lengths.push(ticks_on_current);
                ticks_on_current = 0;
                previous = direction;
            }
        }
        assert!(lengths.len() > 5, "should have switched repeatedly");
        let first = lengths[0];
        assert!(
            lengths.iter().any(|length| *length != first),
            "every interval was the same length: {lengths:?}"
        );
    }

    #[test]
    fn the_bot_never_plants_its_feet_twice_in_a_row() {
        let mut planner = StrafePlanner::new();
        let mut rng = SeededRng::new(11);
        let mut previous = planner.advance(Duration::ZERO, &mut rng);
        let mut consecutive_still = 0;
        for _ in 0..600 {
            let direction = planner.advance(TICK, &mut rng);
            if direction != previous {
                if direction == OrbitDirection::Still {
                    consecutive_still += 1;
                    assert!(consecutive_still < 2, "stood still two intervals running");
                } else {
                    consecutive_still = 0;
                }
                previous = direction;
            }
        }
    }

    #[test]
    fn both_circling_and_planted_intervals_actually_occur() {
        let mut planner = StrafePlanner::new();
        let mut rng = SeededRng::new(3);
        let mut saw_still = false;
        let mut saw_circling = false;
        for _ in 0..600 {
            match planner.advance(TICK, &mut rng) {
                OrbitDirection::Still => saw_still = true,
                _ => saw_circling = true,
            }
        }
        assert!(saw_still && saw_circling);
    }

    #[test]
    fn landing_a_hit_shortens_the_current_interval() {
        let mut planner = StrafePlanner::new();
        let mut rng = rng();
        let before = planner.advance(Duration::ZERO, &mut rng);
        planner.note_attack();
        // The combo interval is 150ms: three ticks is past it, where the
        // base interval would still have four to go.
        planner.advance(TICK, &mut rng);
        planner.advance(TICK, &mut rng);
        let after = planner.advance(TICK, &mut rng);
        assert_ne!(after, before, "a combo should switch sides sooner");
    }

    #[test]
    fn a_forced_switch_takes_effect_on_the_next_update() {
        let mut planner = StrafePlanner::new();
        let mut rng = rng();
        let before = planner.advance(Duration::ZERO, &mut rng);
        planner.force_switch();
        assert_ne!(planner.advance(TICK, &mut rng), before);
    }

    #[test]
    fn a_reset_starts_a_fresh_pattern() {
        let mut planner = StrafePlanner::new();
        let mut rng = rng();
        planner.advance(TICK, &mut rng);
        planner.reset(&mut rng);
        assert!(
            planner.advance(Duration::ZERO, &mut rng).is_circling(),
            "never resets to planted"
        );
    }

    #[test]
    fn the_same_seed_replays_the_same_pattern() {
        let replay = |seed: u64| {
            let mut planner = StrafePlanner::new();
            let mut rng = SeededRng::new(seed);
            (0..50)
                .map(|_| planner.advance(TICK, &mut rng))
                .collect::<Vec<_>>()
        };
        assert_eq!(replay(42), replay(42));
        assert_ne!(replay(42), replay(43));
    }
}
