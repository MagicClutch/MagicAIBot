//! Pure health-based behavior mode. No Azalea types, no I/O --
//! `crate::combat::executor` reads the bot's own live health (already
//! exposed via `BotSnapshot::health`, no new plumbing needed) and calls
//! [`mode_for_health`] once per tick.

/// How aggressively the bot should be fighting right now, purely as a
/// function of its own current health (HP, 0-20 scale -- the same scale
/// `KillbotConfig::heal_threshold`/`reengage_threshold` use, and the same
/// scale `BotSnapshot::health` already reports). `crate::combat::executor`
/// uses this to decide shield behavior and to report the current tier; it
/// does not by itself change anything.
///
/// Note that since `#kill` fights in full-aggression mode (see
/// `crate::combat::executor`'s module doc comment) none of these tiers ever
/// means "stop fighting" -- the low ones only mean "block more, and eat
/// while continuing to press the target".
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CombatMode {
    /// Full pressure: chase, crit, combo.
    #[default]
    Aggressive,
    /// Moderate pressure: fewer unnecessary jumps, more willing to block.
    Balanced,
    /// Still pressing, but eating mid-fight and blocking whenever the
    /// shield is free.
    Defensive,
    /// Same as `Defensive`, one tier more urgent -- eat at the first
    /// opportunity and block whenever in range.
    Critical,
}

/// Below this, `Critical` regardless of `heal_threshold`.
const CRITICAL_HEALTH: f64 = 4.0;
/// At or below this (and above `heal_threshold`), `Balanced`; above it,
/// `Aggressive`.
const BALANCED_UPPER_HEALTH: f64 = 12.0;

/// `health` and `heal_threshold` are both HP on the vanilla 0-20 scale
/// (`heal_threshold` is typically `KillbotConfig::heal_threshold`, passed
/// through rather than hardcoded so a user who configures a higher or
/// lower threshold actually changes when the bot disengages, not just when
/// it starts eating).
pub fn mode_for_health(health: f64, heal_threshold: f64) -> CombatMode {
    if health < CRITICAL_HEALTH {
        CombatMode::Critical
    } else if health < heal_threshold {
        CombatMode::Defensive
    } else if health <= BALANCED_UPPER_HEALTH {
        CombatMode::Balanced
    } else {
        CombatMode::Aggressive
    }
}

/// Whether the bot is hurt enough to eat: a single threshold, checked
/// against the bot's own health.
///
/// There is deliberately no hysteresis or "top up to full" band. Spacing
/// between bites is `KillbotConfig::eat_cooldown_ms`'s job -- one bite, then
/// commit to the fight for several seconds -- and stacking a second
/// mechanism on top of it is what produced a bot that ate its way through a
/// whole stack of golden apples without swinging. Below the threshold it
/// eats one; above it, it fights.
///
/// Nothing here disengages -- see `crate::combat::executor`; the bot eats
/// while still chasing and hitting the target.
pub fn wants_food(health: f64, heal_threshold: f64) -> bool {
    health < heal_threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eats_below_the_threshold_and_not_at_or_above_it() {
        // The default threshold is four hearts.
        assert!(wants_food(7.9, 8.0));
        assert!(!wants_food(8.0, 8.0));
        assert!(!wants_food(20.0, 8.0));
        assert!(wants_food(0.5, 8.0));
    }

    #[test]
    fn full_health_is_aggressive() {
        assert_eq!(mode_for_health(20.0, 8.0), CombatMode::Aggressive);
        assert_eq!(mode_for_health(12.01, 8.0), CombatMode::Aggressive);
    }

    #[test]
    fn moderate_health_is_balanced() {
        assert_eq!(mode_for_health(12.0, 8.0), CombatMode::Balanced);
        assert_eq!(mode_for_health(8.0, 8.0), CombatMode::Balanced);
    }

    #[test]
    fn low_health_is_defensive() {
        assert_eq!(mode_for_health(7.99, 8.0), CombatMode::Defensive);
        assert_eq!(mode_for_health(4.0, 8.0), CombatMode::Defensive);
    }

    #[test]
    fn very_low_health_is_critical() {
        assert_eq!(mode_for_health(3.99, 8.0), CombatMode::Critical);
        assert_eq!(mode_for_health(0.0, 8.0), CombatMode::Critical);
    }

    #[test]
    fn a_configured_heal_threshold_moves_the_defensive_boundary() {
        assert_eq!(
            mode_for_health(10.0, 12.0),
            CombatMode::Defensive,
            "with a higher heal_threshold, 10 HP should already be defensive"
        );
        assert_eq!(
            mode_for_health(10.0, 5.0),
            CombatMode::Balanced,
            "with a lower heal_threshold, 10 HP should not yet be defensive"
        );
    }
}
