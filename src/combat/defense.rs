//! Pure decision for the bot's *own* shield use (distinct from
//! `crate::combat::shield_break`, which is about breaking the *target's*
//! shield with an axe). No Azalea types, no I/O.
//!
//! Real PvP shield timing ("raise 150-300ms before an expected hit") relies
//! on reading the opponent's own attack windup, which isn't something this
//! bot can observe -- Azalea doesn't expose another player's swing
//! animation state. Two of [`should_raise_shield`]'s three triggers are
//! therefore a proactive heuristic rather than a reactive one: they stay
//! raised for the whole window a threat condition holds (low health, or the
//! target visibly closing distance fast -- see
//! `crate::combat::targeting::is_approaching`) and drop the instant that
//! condition clears.
//!
//! The third trigger (`attack_ready`) is genuinely reactive, just keyed to a
//! signal the bot *can* observe: its own attack cooldown. Vanilla melee has
//! no windup to read even for a human opponent -- a swing lands the instant
//! it's clicked -- so "block right before their hit" isn't a real technique
//! to begin with. What a good player actually does between their own swings
//! is hold guard while they can't yet answer back, then drop it the instant
//! they can and counter. That's exactly what tying the raise to the bot's
//! own `attack_ready` produces: in melee range this keeps the shield up
//! almost continuously, dropping only for the tick a swing actually lands --
//! "always block, counter the moment ready" is the intended behavior here,
//! not a bug.

use crate::combat::health::CombatMode;

/// Blocking is pointless outside melee range -- nothing can land a hit
/// worth blocking from further than this.
pub const SHIELD_RANGE: f64 = 3.0;

/// Whether the bot should be holding its shield up right now.
///
/// Raises for any of three independent conditions, all gated by melee
/// range: `Defensive`/`Critical` health mode (also covers "healing" and
/// "repositioning", since both only ever happen in those modes -- see
/// `crate::combat::executor`), the target closing distance fast regardless
/// of mode, or -- see this module's doc comment -- the bot simply not being
/// able to land a counter-swing right now (`!attack_ready`).
pub fn should_raise_shield(
    mode: CombatMode,
    distance_to_target: f64,
    target_approaching: bool,
    attack_ready: bool,
) -> bool {
    if distance_to_target > SHIELD_RANGE {
        return false;
    }
    matches!(mode, CombatMode::Defensive | CombatMode::Critical) || target_approaching || !attack_ready
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raises_when_defensive_or_critical_and_in_range() {
        assert!(should_raise_shield(CombatMode::Defensive, 2.0, false, true));
        assert!(should_raise_shield(CombatMode::Critical, 2.0, false, true));
    }

    #[test]
    fn stays_down_when_aggressive_or_balanced_and_nothing_is_approaching_and_attack_is_ready() {
        assert!(!should_raise_shield(CombatMode::Aggressive, 2.0, false, true));
        assert!(!should_raise_shield(CombatMode::Balanced, 2.0, false, true));
    }

    #[test]
    fn raises_for_an_approaching_target_even_at_full_health() {
        assert!(should_raise_shield(CombatMode::Aggressive, 2.0, true, true));
    }

    #[test]
    fn never_raises_outside_shield_range_regardless_of_mode_or_approach() {
        assert!(!should_raise_shield(
            CombatMode::Critical,
            SHIELD_RANGE + 0.1,
            true,
            true
        ));
    }

    #[test]
    fn raises_when_attack_is_not_ready_even_at_full_health_and_not_approaching() {
        assert!(should_raise_shield(CombatMode::Aggressive, 2.0, false, false));
    }

    #[test]
    fn never_raises_outside_shield_range_even_when_attack_is_not_ready() {
        assert!(!should_raise_shield(
            CombatMode::Critical,
            SHIELD_RANGE + 0.1,
            true,
            false
        ));
    }
}
