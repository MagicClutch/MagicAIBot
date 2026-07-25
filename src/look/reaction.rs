use std::time::Duration;

use crate::{config::LookConfig, look::aim_point::SeededRng};

#[derive(Clone, Copy, Debug)]
struct Sample {
    position: [f64; 3],
    at: Duration,
}
#[derive(Clone, Copy, Debug)]
struct PendingUpdate {
    target: [f64; 3],
    due_at: Duration,
}

#[derive(Clone, Debug, Default)]
pub struct ReactionTracker {
    applied: Option<[f64; 3]>,
    observed: Option<Sample>,
    velocity: Option<[f64; 3]>,
    pending: Option<PendingUpdate>,
}

impl ReactionTracker {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn update(
        &mut self,
        now: Duration,
        target: [f64; 3],
        natural_moving_target: bool,
        config: &LookConfig,
        rng: &mut SeededRng,
    ) -> [f64; 3] {
        if !natural_moving_target {
            self.applied = Some(target);
            self.observed = Some(Sample {
                position: target,
                at: now,
            });
            self.velocity = None;
            self.pending = None;
            return target;
        }
        let Some(previous) = self.observed else {
            self.applied = Some(target);
            self.observed = Some(Sample {
                position: target,
                at: now,
            });
            return target;
        };
        let movement = subtract(target, previous.position);
        if length(movement) >= config.minimum_target_movement {
            let seconds = now
                .checked_sub(previous.at)
                .unwrap_or_default()
                .as_secs_f64()
                .max(0.001);
            let velocity = scale(movement, 1.0 / seconds);
            let sharp_turn = self.velocity.is_some_and(|old| dot(old, velocity) < 0.0);
            let predicted =
                if config.moving_target_prediction && !sharp_turn && length(velocity) > 0.05 {
                    let horizon = config.reaction_delay_max_ms as f64 / 1000.0;
                    blend_prediction(target, velocity, horizon, config.prediction_strength)
                } else {
                    target
                };
            self.velocity = (!sharp_turn).then_some(velocity);
            self.observed = Some(Sample {
                position: target,
                at: now,
            });
            if let Some(pending) = &mut self.pending {
                pending.target = predicted;
            } else {
                let delay = random_delay(config, rng);
                self.pending = Some(PendingUpdate {
                    target: predicted,
                    due_at: now + delay,
                });
            }
        }
        if self.pending.is_some_and(|pending| now >= pending.due_at) {
            self.applied = self.pending.take().map(|pending| pending.target);
        }
        self.applied.unwrap_or(target)
    }
    #[cfg(test)]
    pub fn pending_due_at(&self) -> Option<Duration> {
        self.pending.map(|pending| pending.due_at)
    }
}

pub fn random_delay(config: &LookConfig, rng: &mut SeededRng) -> Duration {
    let range = config.reaction_delay_max_ms - config.reaction_delay_min_ms;
    Duration::from_millis(
        config.reaction_delay_min_ms + (rng.next_unit() * range as f64).floor() as u64,
    )
}
pub fn blend_prediction(
    position: [f64; 3],
    velocity: [f64; 3],
    seconds: f64,
    strength: f64,
) -> [f64; 3] {
    add(
        position,
        scale(velocity, seconds * strength.clamp(0.0, 1.0)),
    )
}
fn subtract(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(value: [f64; 3], scalar: f64) -> [f64; 3] {
    [value[0] * scalar, value[1] * scalar, value[2] * scalar]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn length(value: [f64; 3]) -> f64 {
    dot(value, value).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> LookConfig {
        LookConfig::default()
    }
    #[test]
    fn seeded_delays_are_bounded_and_repeatable() {
        let config = config();
        let mut a = SeededRng::new(22);
        let mut b = SeededRng::new(22);
        let first = random_delay(&config, &mut a);
        let second = random_delay(&config, &mut b);
        assert_eq!(first, second);
        assert!((35..=90).contains(&first.as_millis()));
    }
    #[test]
    fn movement_waits_then_coalesces_to_newest_point() {
        let mut config = config();
        config.reaction_delay_min_ms = 50;
        config.reaction_delay_max_ms = 50;
        config.moving_target_prediction = false;
        let mut tracker = ReactionTracker::default();
        let mut rng = SeededRng::new(1);
        let start = Duration::ZERO;
        assert_eq!(
            tracker.update(start, [0.0; 3], true, &config, &mut rng),
            [0.0; 3]
        );
        assert_eq!(
            tracker.update(
                Duration::from_millis(10),
                [1.0, 0.0, 0.0],
                true,
                &config,
                &mut rng
            ),
            [0.0; 3]
        );
        let due = tracker.pending_due_at().unwrap();
        assert_eq!(
            tracker.update(
                Duration::from_millis(20),
                [2.0, 0.0, 0.0],
                true,
                &config,
                &mut rng
            ),
            [0.0; 3]
        );
        assert_eq!(tracker.pending_due_at(), Some(due));
        assert_eq!(
            tracker.update(
                Duration::from_millis(60),
                [2.0, 0.0, 0.0],
                true,
                &config,
                &mut rng
            ),
            [2.0, 0.0, 0.0]
        );
    }
    #[test]
    fn tiny_movement_and_precise_mode_do_not_delay() {
        let config = config();
        let mut tracker = ReactionTracker::default();
        let mut rng = SeededRng::new(3);
        tracker.update(Duration::ZERO, [0.0; 3], true, &config, &mut rng);
        assert_eq!(
            tracker.update(
                Duration::from_millis(1),
                [0.01, 0.0, 0.0],
                true,
                &config,
                &mut rng
            ),
            [0.0; 3]
        );
        assert_eq!(
            tracker.update(
                Duration::from_millis(2),
                [3.0, 0.0, 0.0],
                false,
                &config,
                &mut rng
            ),
            [3.0, 0.0, 0.0]
        );
    }
    #[test]
    fn prediction_is_a_subtle_blend() {
        assert_eq!(
            blend_prediction([1.0, 2.0, 3.0], [10.0, 0.0, -10.0], 0.1, 0.35),
            [1.35, 2.0, 2.65]
        );
    }
}
