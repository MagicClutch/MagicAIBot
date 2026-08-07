//! Pure target-tracking math: distance, short-term movement prediction, and
//! "has this target effectively disappeared" detection. No Azalea types, no
//! I/O -- `crate::combat::executor` is the only caller, and it supplies
//! plain position/velocity readings already pulled out of the world
//! snapshot.

use crate::minecraft::world_state::PositionSnapshot;

/// How far ahead (in seconds) movement prediction leads a moving target --
/// short enough that a sudden direction change (a strafe flick, a sharp
/// turn) doesn't send the bot lunging at empty space, long enough to
/// meaningfully anticipate a straight sprint or a consistent strafe.
pub const PREDICTION_LEAD_SECONDS: f64 = 0.25;

/// Caps how far the predicted point can be pushed from the target's actual
/// position, regardless of how fast they're moving (an elytra boost, a
/// knockback spike) -- a prediction this bot chases should never be more
/// than about a body-length past reality.
pub const PREDICTION_MAX_LEAD_BLOCKS: f64 = 1.5;

/// A target with no observed position update in this long is treated as
/// lost rather than chased indefinitely on stale data.
pub const STALE_OBSERVATION_SECONDS: f64 = 3.0;

pub fn distance(a: PositionSnapshot, b: PositionSnapshot) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

/// Predicts where a target moving at `velocity` (blocks/tick, Minecraft's
/// native unit) will be `lead_seconds` from now, for the bot's own
/// *movement* positioning (see `crate::combat::movement`) -- distinct from
/// the look controller's own, much shorter aim-lead (see
/// `look::look_target::LookTarget::PredictedPlayer`), since overcommitting
/// the bot's feet to a many-tick-old prediction is far more costly than a
/// slightly-early crosshair. Horizontal only: vertical velocity (falling,
/// jumping) says nothing useful about where to walk.
pub fn predicted_position(
    position: PositionSnapshot,
    velocity: [f64; 3],
    lead_seconds: f64,
) -> PositionSnapshot {
    // Minecraft velocity is blocks/tick; 20 ticks/second converts to
    // blocks/second before applying the lead.
    let lead_x = (velocity[0] * 20.0 * lead_seconds)
        .clamp(-PREDICTION_MAX_LEAD_BLOCKS, PREDICTION_MAX_LEAD_BLOCKS);
    let lead_z = (velocity[2] * 20.0 * lead_seconds)
        .clamp(-PREDICTION_MAX_LEAD_BLOCKS, PREDICTION_MAX_LEAD_BLOCKS);
    PositionSnapshot {
        x: position.x + lead_x,
        y: position.y,
        z: position.z + lead_z,
    }
}

/// A target is lost once nothing has been observed about it for
/// [`STALE_OBSERVATION_SECONDS`] -- covers both "genuinely disconnected/out
/// of loaded range" and "despawned from view distance", which look
/// identical from here (see `crate::combat::kill`'s doc comment).
pub fn is_stale(seconds_since_last_seen: f64) -> bool {
    seconds_since_last_seen >= STALE_OBSERVATION_SECONDS
}

/// Minimum horizontal speed (blocks/tick) for a target's velocity to count
/// as "moving" at all for [`is_approaching`] -- below this, floating-point
/// noise in an essentially-stationary target's velocity shouldn't flip the
/// result back and forth.
const APPROACH_MIN_SPEED: f64 = 0.02;

/// Whether `target`, moving at `target_velocity`, is currently heading
/// roughly toward `bot` -- the closest signal available for "is the
/// opponent charging at me" (see `crate::combat::defense`) without any
/// direct "is sprinting" flag for a remote player. Horizontal only, via
/// the dot product of the (target -> bot) direction and the target's
/// horizontal velocity: positive and non-negligible means closing in.
pub fn is_approaching(
    bot: PositionSnapshot,
    target: PositionSnapshot,
    target_velocity: [f64; 3],
) -> bool {
    let to_bot_x = bot.x - target.x;
    let to_bot_z = bot.z - target.z;
    let to_bot_length = to_bot_x.hypot(to_bot_z);
    let speed = target_velocity[0].hypot(target_velocity[2]);
    if to_bot_length < 1e-6 || speed < APPROACH_MIN_SPEED {
        return false;
    }
    let dot = (to_bot_x * target_velocity[0] + to_bot_z * target_velocity[2]) / to_bot_length;
    // Closing speed is more than half the target's total horizontal
    // speed -- i.e. moving mostly toward the bot, not just clipping past.
    dot > speed * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(x: f64, y: f64, z: f64) -> PositionSnapshot {
        PositionSnapshot { x, y, z }
    }

    #[test]
    fn distance_is_symmetric_and_zero_at_the_same_point() {
        let a = position(0.0, 0.0, 0.0);
        let b = position(3.0, 0.0, 4.0);
        assert_eq!(distance(a, a), 0.0);
        assert!((distance(a, b) - 5.0).abs() < 1e-9);
        assert!((distance(a, b) - distance(b, a)).abs() < 1e-9);
    }

    #[test]
    fn a_stationary_target_predicts_to_its_own_position() {
        let position = position(10.0, 64.0, 10.0);
        let predicted = predicted_position(position, [0.0, 0.0, 0.0], PREDICTION_LEAD_SECONDS);
        assert_eq!(predicted.x, position.x);
        assert_eq!(predicted.z, position.z);
    }

    #[test]
    fn a_moving_target_predicts_ahead_in_its_direction_of_travel() {
        let position = position(0.0, 64.0, 0.0);
        // Sprinting is roughly 0.28 blocks/tick horizontally.
        let predicted = predicted_position(position, [0.28, 0.0, 0.0], PREDICTION_LEAD_SECONDS);
        assert!(predicted.x > position.x);
        assert_eq!(predicted.z, position.z);
    }

    #[test]
    fn vertical_velocity_never_affects_the_predicted_position() {
        let position = position(0.0, 64.0, 0.0);
        let predicted = predicted_position(position, [0.0, -5.0, 0.0], PREDICTION_LEAD_SECONDS);
        assert_eq!(predicted.y, position.y);
    }

    #[test]
    fn an_extreme_velocity_spike_is_clamped_rather_than_chased_forever() {
        let position = position(0.0, 64.0, 0.0);
        let predicted = predicted_position(position, [500.0, 0.0, 0.0], PREDICTION_LEAD_SECONDS);
        assert!((predicted.x - position.x - PREDICTION_MAX_LEAD_BLOCKS).abs() < 1e-9);
    }

    #[test]
    fn staleness_uses_the_configured_threshold() {
        assert!(!is_stale(0.0));
        assert!(!is_stale(STALE_OBSERVATION_SECONDS - 0.1));
        assert!(is_stale(STALE_OBSERVATION_SECONDS));
        assert!(is_stale(STALE_OBSERVATION_SECONDS + 10.0));
    }

    #[test]
    fn a_target_moving_straight_at_the_bot_is_approaching() {
        let bot = position(0.0, 64.0, 0.0);
        let target = position(5.0, 64.0, 0.0);
        // Moving in -x, i.e. toward the bot.
        assert!(is_approaching(bot, target, [-0.28, 0.0, 0.0]));
    }

    #[test]
    fn a_target_moving_straight_away_is_not_approaching() {
        let bot = position(0.0, 64.0, 0.0);
        let target = position(5.0, 64.0, 0.0);
        assert!(!is_approaching(bot, target, [0.28, 0.0, 0.0]));
    }

    #[test]
    fn a_target_strafing_perpendicular_is_not_approaching() {
        let bot = position(0.0, 64.0, 0.0);
        let target = position(5.0, 64.0, 0.0);
        assert!(!is_approaching(bot, target, [0.0, 0.0, 0.28]));
    }

    #[test]
    fn a_near_stationary_target_is_not_approaching() {
        let bot = position(0.0, 64.0, 0.0);
        let target = position(5.0, 64.0, 0.0);
        assert!(!is_approaching(bot, target, [-0.001, 0.0, 0.0]));
    }

    #[test]
    fn vertical_velocity_alone_never_counts_as_approaching() {
        let bot = position(0.0, 64.0, 0.0);
        let target = position(5.0, 64.0, 0.0);
        assert!(!is_approaching(bot, target, [0.0, -5.0, 0.0]));
    }
}
