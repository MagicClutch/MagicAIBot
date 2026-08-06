//! Pure weapon-choice policy: which category (sword vs. axe) the bot
//! should be wielding right now, both for breaking a blocking target's
//! shield and for the general "always fight with the best available
//! weapon, falling back sensibly" requirement.
//!
//! Detecting the target's block itself is a best-effort heuristic --
//! `crate::minecraft::client::MinecraftClient::player_combat_status`'s doc
//! comment explains exactly what signal is (and isn't) available for a
//! remote player -- but *given* a "target appears to be blocking" reading,
//! the switch policy itself is simple and exact: axes deal bonus damage to
//! a raised shield and can disable it outright, so switch to the best axe
//! held while the target is blocking, then switch back once they stop.

use crate::interaction::tool_selection::ToolCategory;

/// Whether the bot should be wielding an axe right now, given whether the
/// target currently appears to be blocking. A simple mirror of the input --
/// kept as its own function (rather than inlined at the call site) so the
/// policy is independently testable and the one place this rule could ever
/// need to grow a hysteresis/cooldown later.
pub fn should_wield_axe(target_blocking: bool) -> bool {
    target_blocking
}

/// Whether it's time to switch back to the preferred weapon (a sword) --
/// the exact complement of [`should_wield_axe`], split out as its own
/// function purely for readability at call sites that ask the question the
/// other way around.
pub fn should_switch_back_to_sword(target_blocking: bool) -> bool {
    !target_blocking
}

/// The full weapon-category policy: sword is always preferred *except*
/// while the target is blocking ([`should_wield_axe`]) or when no sword is
/// held at all, in which case the axe -- the only other weapon category
/// this bot equips -- is the fallback rather than fighting bare-handed.
pub fn desired_weapon_category(target_blocking: bool, sword_available: bool) -> ToolCategory {
    if should_wield_axe(target_blocking) {
        return ToolCategory::Axe;
    }
    if should_switch_back_to_sword(target_blocking) && sword_available {
        ToolCategory::Sword
    } else {
        ToolCategory::Axe
    }
}

/// Fraction of maximum durability at or below which a weapon counts as
/// critically low -- "durability becomes critical: switch weapons
/// automatically" from the spec. 10% mirrors the same "about to break"
/// intuition a human player uses when checking their held item's damage
/// bar.
pub const CRITICAL_DURABILITY_FRACTION: f32 = 0.1;

/// Whether `current`/`max` durability (both from `EquipmentItem`, whole
/// item-damage units) counts as critically low. `max == 0` (an item this
/// codebase doesn't track durability for, e.g. one with no `MaxDamage`
/// component) is never critical -- there's nothing to run out of.
pub fn is_durability_critical(current: u32, max: u32) -> bool {
    max > 0 && (current as f32 / max as f32) <= CRITICAL_DURABILITY_FRACTION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wields_axe_only_while_the_target_is_blocking() {
        assert!(should_wield_axe(true));
        assert!(!should_wield_axe(false));
    }

    #[test]
    fn switches_back_the_instant_blocking_stops() {
        assert!(should_switch_back_to_sword(false));
        assert!(!should_switch_back_to_sword(true));
    }

    #[test]
    fn the_two_policies_never_agree() {
        for blocking in [true, false] {
            assert_ne!(
                should_wield_axe(blocking),
                should_switch_back_to_sword(blocking)
            );
        }
    }

    #[test]
    fn prefers_the_sword_when_available_and_the_target_is_not_blocking() {
        assert_eq!(desired_weapon_category(false, true), ToolCategory::Sword);
    }

    #[test]
    fn switches_to_the_axe_while_the_target_blocks_even_with_a_sword_available() {
        assert_eq!(desired_weapon_category(true, true), ToolCategory::Axe);
    }

    #[test]
    fn falls_back_to_the_axe_when_no_sword_is_held_at_all() {
        assert_eq!(desired_weapon_category(false, false), ToolCategory::Axe);
    }

    #[test]
    fn a_zero_max_durability_is_never_critical() {
        assert!(!is_durability_critical(0, 0));
    }

    #[test]
    fn low_remaining_durability_is_critical() {
        assert!(is_durability_critical(10, 100));
        assert!(is_durability_critical(1, 100));
    }

    #[test]
    fn healthy_durability_is_not_critical() {
        assert!(!is_durability_critical(50, 100));
        assert!(!is_durability_critical(100, 100));
    }
}
