//! Pure decision for the bot's *own* shield use (distinct from
//! `crate::combat::shield_break`, which is about breaking the *target's*
//! shield with an axe). No Azalea types, no I/O.
//!
//! Real PvP shield timing ("raise 150-300ms before an expected hit") relies
//! on reading the opponent's own attack windup, which isn't something this
//! bot can observe -- Azalea doesn't expose another player's swing
//! animation state. [`should_raise_shield`] is therefore a proactive
//! heuristic rather than a reactive one: it stays raised for the whole
//! window a threat condition holds (low health, or the target visibly
//! closing distance fast -- see `crate::combat::targeting::is_approaching`)
//! and drops the instant that condition clears, which is the closest
//! honest approximation to "block right before impact, release right
//! after" available without that missing signal.

use crate::combat::health::CombatMode;

/// Blocking is pointless outside melee range -- nothing can land a hit
/// worth blocking from further than this.
pub const SHIELD_RANGE: f64 = 3.0;

/// Whether the bot should be holding its shield up right now.
///
/// Raises for the two conditions the spec ties directly to survival --
/// `Defensive`/`Critical` health mode (also covers "healing" and
/// "repositioning", since both only ever happen in those modes -- see
/// `crate::combat::executor`) -- and independently whenever the target is
/// closing distance fast within range, regardless of mode, since a
/// charging opponent is a real incoming-hit signal even at full health.
pub fn should_raise_shield(
    mode: CombatMode,
    distance_to_target: f64,
    target_approaching: bool,
) -> bool {
    if distance_to_target > SHIELD_RANGE {
        return false;
    }
    matches!(mode, CombatMode::Defensive | CombatMode::Critical) || target_approaching
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raises_when_defensive_or_critical_and_in_range() {
        assert!(should_raise_shield(CombatMode::Defensive, 2.0, false));
        assert!(should_raise_shield(CombatMode::Critical, 2.0, false));
    }

    #[test]
    fn stays_down_when_aggressive_or_balanced_and_nothing_is_approaching() {
        assert!(!should_raise_shield(CombatMode::Aggressive, 2.0, false));
        assert!(!should_raise_shield(CombatMode::Balanced, 2.0, false));
    }

    #[test]
    fn raises_for_an_approaching_target_even_at_full_health() {
        assert!(should_raise_shield(CombatMode::Aggressive, 2.0, true));
    }

    #[test]
    fn never_raises_outside_shield_range_regardless_of_mode_or_approach() {
        assert!(!should_raise_shield(
            CombatMode::Critical,
            SHIELD_RANGE + 0.1,
            true
        ));
    }
}
