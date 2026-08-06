//! Pure attack-timing and critical-hit decisions.
//!
//! Vanilla awards a critical hit (1.5x damage, the little star particles)
//! when the attacker hits while falling (not on the ground, moving
//! downward) and isn't otherwise disqualified (sprinting *into* the hit,
//! climbing, blind, etc. -- none of which this bot ever does mid-swing, so
//! those exclusions don't need modeling here). The only lever available is
//! *when* to attack relative to a jump: jump, let gravity take over for a
//! couple of ticks, then swing while still airborne and descending.

use std::time::{Duration, Instant};

/// Minimum spacing between two attacks. Vanilla's own attack-strength
/// cooldown (`AttackStrengthScale`) still governs actual damage
/// server-side regardless of how often a packet is sent, but resending
/// far more often than the weapon's real cooldown wastes packets and
/// doesn't help -- mirrors `mobs::combat::ATTACK_INTERVAL`, tuned for a
/// sword's ~0.6s vanilla cooldown.
pub const ATTACK_COOLDOWN: Duration = Duration::from_millis(600);

/// Minimum spacing between two crit-seeking jumps -- "avoid random
/// jumping"/"do not spam jump every attack" from the spec. Roughly every
/// other attack cycle at [`ATTACK_COOLDOWN`]'s pace, not every single one.
pub const JUMP_COOLDOWN: Duration = Duration::from_millis(1000);

/// How full the attack cooldown must be before swinging -- the spec's
/// "suggested threshold: 90-100%", not the full 100% `attack_ready` used
/// to require. Waiting for exactly 100% before ever sending the packet
/// means the very next tick that crosses the line is always at least one
/// tick late relative to the server's own cooldown clock; swinging once
/// already most of the way there trades a negligible amount of damage
/// scaling for landing hits right as the window opens instead of just
/// after.
pub const ATTACK_READY_THRESHOLD: f64 = 0.9;

/// How full the attack cooldown is right now, from 0.0 (just attacked) to
/// 1.0 (fully recovered) -- vanilla's own `AttackStrengthScale` concept,
/// approximated here from elapsed time against this bot's fixed
/// [`ATTACK_COOLDOWN`] baseline (a real sword's cooldown; this bot doesn't
/// currently read the wielded weapon's actual attack-speed attribute, so
/// switching to an axe mid-fight uses the same baseline rather than a
/// slower, axe-accurate one -- see this module's doc comment).
pub fn attack_cooldown_progress(last_attack: Option<Instant>, now: Instant) -> f64 {
    match last_attack {
        None => 1.0,
        Some(last) => (now.saturating_duration_since(last).as_secs_f64()
            / ATTACK_COOLDOWN.as_secs_f64())
        .min(1.0),
    }
}

/// Whether the cooldown is full enough to swing -- see
/// [`ATTACK_READY_THRESHOLD`]. Below it, the caller should keep tracking,
/// strafing, and sprinting instead of attacking early and wasting the
/// cooldown's damage scaling.
pub fn attack_ready(last_attack: Option<Instant>, now: Instant) -> bool {
    attack_cooldown_progress(last_attack, now) >= ATTACK_READY_THRESHOLD
}

/// Whether attacking *right now* would land as a critical hit: airborne
/// and already descending. `velocity_y` is Minecraft's raw vertical
/// velocity (negative while falling).
pub fn is_critical_window(on_ground: bool, velocity_y: f64) -> bool {
    !on_ground && velocity_y < 0.0
}

/// Whether the bot should queue a jump this tick to set up a future
/// critical hit -- only from solid ground, only within striking distance
/// (jumping while still closing distance just wastes the arc), and not
/// again too soon after the last one.
pub fn should_jump_for_crit(
    on_ground: bool,
    within_attack_range: bool,
    last_jump: Option<Instant>,
    now: Instant,
) -> bool {
    on_ground
        && within_attack_range
        && last_jump.is_none_or(|last| now.saturating_duration_since(last) >= JUMP_COOLDOWN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attack_is_ready_immediately_with_no_prior_attack() {
        assert!(attack_ready(None, Instant::now()));
    }

    #[test]
    fn attack_is_not_ready_well_before_the_threshold() {
        let now = Instant::now();
        // Half the cooldown elapsed -- 50% progress, well under the 90%
        // threshold.
        assert!(!attack_ready(Some(now), now + ATTACK_COOLDOWN / 2));
    }

    #[test]
    fn attack_is_ready_once_the_threshold_is_reached_even_before_full_cooldown() {
        let now = Instant::now();
        let ninety_percent =
            Duration::from_secs_f64(ATTACK_COOLDOWN.as_secs_f64() * ATTACK_READY_THRESHOLD);
        assert!(attack_ready(Some(now), now + ninety_percent));
        assert!(!attack_ready(
            Some(now),
            now + ninety_percent - Duration::from_millis(1)
        ));
    }

    #[test]
    fn cooldown_progress_is_clamped_to_one_even_long_after_the_cooldown() {
        let now = Instant::now();
        assert_eq!(
            attack_cooldown_progress(Some(now), now + ATTACK_COOLDOWN * 10),
            1.0
        );
    }

    #[test]
    fn attack_is_ready_once_the_cooldown_fully_elapses() {
        let now = Instant::now();
        assert!(attack_ready(Some(now), now + ATTACK_COOLDOWN));
        assert!(attack_ready(Some(now), now + ATTACK_COOLDOWN * 2));
    }

    #[test]
    fn critical_window_requires_airborne_and_descending() {
        assert!(is_critical_window(false, -0.1));
        assert!(!is_critical_window(true, -0.1), "on the ground");
        assert!(!is_critical_window(false, 0.5), "still rising");
        assert!(!is_critical_window(false, 0.0), "apex, not yet falling");
    }

    #[test]
    fn jump_for_crit_requires_ground_and_range_and_respects_cooldown() {
        let now = Instant::now();
        assert!(should_jump_for_crit(true, true, None, now));
        assert!(!should_jump_for_crit(false, true, None, now), "airborne");
        assert!(
            !should_jump_for_crit(true, false, None, now),
            "out of range"
        );
        assert!(!should_jump_for_crit(
            true,
            true,
            Some(now),
            now + JUMP_COOLDOWN - Duration::from_millis(1)
        ));
        assert!(should_jump_for_crit(
            true,
            true,
            Some(now),
            now + JUMP_COOLDOWN
        ));
    }
}
