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
//!
//! The bot's *own* shield being unavailable (broken, or never picked up)
//! is a second, independent input to the same policy: sword-in-main-hand
//! with a shield in the offhand is the default stance
//! (`crate::combat::defense` owns *when* to raise it), but with no shield
//! left to fall back on for defense, [`desired_weapon_category`] instead
//! alternates every swing between sword and axe -- trading the lost
//! defensive option for the extra damage a weapon swap provides, rather
//! than fighting the same way with strictly less safety.

use crate::equipment::model::EquipmentItem;
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
/// while the target is blocking ([`should_wield_axe`]) -- that still wins
/// outright -- or when no sword is held at all, in which case the axe --
/// the only other weapon category this bot equips -- is the fallback rather
/// than fighting bare-handed.
///
/// A third case sits between those two: `own_shield_available == false`
/// (the bot's own shield is gone or critically damaged, see
/// [`own_shield_serviceable`]) with the target *not* blocking. There is
/// nothing to break through and no shield left to protect the bot either,
/// so rather than defaulting back to sword-only, `axe_turn` (flipped once
/// per landed swing by the caller) alternates the category swing to swing.
pub fn desired_weapon_category(
    target_blocking: bool,
    sword_available: bool,
    own_shield_available: bool,
    axe_turn: bool,
) -> ToolCategory {
    if should_wield_axe(target_blocking) {
        return ToolCategory::Axe;
    }
    if !own_shield_available && axe_turn {
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

/// Whether the bot's own offhand item is a shield healthy enough to still
/// rely on for defense: actually present, actually
/// `crate::equipment::offhand::SHIELD` (not a totem or anything else that
/// might be sitting in the offhand), and not at critically low durability
/// (reuses [`is_durability_critical`], the same threshold already applied
/// to the held weapon). Feeds both `desired_weapon_category`'s
/// `own_shield_available` and `crate::combat::defense`'s "is there
/// anything to actually raise" gate -- the one place either question is
/// answered, so the two can never disagree about the shield's state.
pub fn own_shield_serviceable(offhand: Option<&EquipmentItem>) -> bool {
    offhand.is_some_and(|item| {
        item.item_id == crate::equipment::offhand::SHIELD
            && !is_durability_critical(item.current_durability, item.max_durability)
    })
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
        assert_eq!(
            desired_weapon_category(false, true, true, false),
            ToolCategory::Sword
        );
    }

    #[test]
    fn switches_to_the_axe_while_the_target_blocks_even_with_a_sword_available() {
        assert_eq!(
            desired_weapon_category(true, true, true, false),
            ToolCategory::Axe
        );
    }

    #[test]
    fn falls_back_to_the_axe_when_no_sword_is_held_at_all() {
        assert_eq!(
            desired_weapon_category(false, false, true, false),
            ToolCategory::Axe
        );
    }

    #[test]
    fn alternates_to_axe_on_its_turn_when_own_shield_is_unavailable() {
        assert_eq!(
            desired_weapon_category(false, true, false, true),
            ToolCategory::Axe
        );
    }

    #[test]
    fn alternates_to_sword_on_its_turn_when_own_shield_is_unavailable() {
        assert_eq!(
            desired_weapon_category(false, true, false, false),
            ToolCategory::Sword
        );
    }

    #[test]
    fn ignores_alternation_when_own_shield_is_available() {
        assert_eq!(
            desired_weapon_category(false, true, true, true),
            ToolCategory::Sword
        );
    }

    #[test]
    fn target_blocking_still_wins_over_alternation() {
        assert_eq!(
            desired_weapon_category(true, true, false, true),
            ToolCategory::Axe
        );
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

    fn equipment_item(item_id: &str, current_durability: u32, max_durability: u32) -> EquipmentItem {
        EquipmentItem {
            slot: 45,
            item_id: item_id.to_owned(),
            current_durability,
            max_durability,
            enchantments: Vec::new(),
        }
    }

    #[test]
    fn nothing_in_the_offhand_is_never_serviceable() {
        assert!(!own_shield_serviceable(None));
    }

    #[test]
    fn a_healthy_shield_is_serviceable() {
        let shield = equipment_item(crate::equipment::offhand::SHIELD, 300, 336);
        assert!(own_shield_serviceable(Some(&shield)));
    }

    #[test]
    fn a_critically_damaged_shield_is_not_serviceable() {
        let shield = equipment_item(crate::equipment::offhand::SHIELD, 10, 336);
        assert!(!own_shield_serviceable(Some(&shield)));
    }

    #[test]
    fn a_non_shield_offhand_item_is_never_serviceable() {
        let totem = equipment_item(crate::equipment::offhand::TOTEM_OF_UNDYING, 1, 1);
        assert!(!own_shield_serviceable(Some(&totem)));
    }
}
