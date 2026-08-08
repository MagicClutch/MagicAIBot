//! Pure Mace smash-attack detection. No Azalea types, no I/O.
//!
//! A smash attack triggers when the Mace lands a hit while the wielder is
//! falling from at least [`SMASH_ATTACK_MIN_FALL`] -- the server computes
//! the bonus damage automatically the instant such a hit lands (tiered by
//! how far the fall has gone: 4 damage/block for the first 3 blocks,
//! 2 damage/block for the next 5 (up to 8 total), 1 damage/block beyond
//! that, uncapped -- see [`smash_bonus_damage`]). The bot never computes
//! this itself; what this module decides is purely *when* a swing would
//! actually trigger it, so `combat::executor` knows to take the hit rather
//! than waiting on a fall that will never come.
//!
//! # Agnostic to how the fall happened
//!
//! This module only ever asks "is the bot falling far enough right now" --
//! it doesn't care whether that fall is incidental (knockback, terrain,
//! chasing a target off a ledge) or deliberately engineered by
//! `combat::wind_launch` throwing a Wind Charge straight down at the bot's
//! own feet. Keeping the two separate means `apply_attack`'s swing-timing
//! decision (this module) stays a small, easily-tested function regardless
//! of which of `wind_launch`'s many more moving parts (aim, hotbar swaps,
//! a use-item packet, a knockback event) produced the fall it's reacting to.

use std::time::Duration;

pub const MACE_ITEM_ID: &str = "minecraft:mace";

/// Vanilla's own attack-speed attribute for the Mace: 0.6/s, the slowest of
/// any melee weapon. `combat::crits::weapon_cooldown` didn't recognize the
/// item id at all before this constant existed, silently falling back to
/// the default 4.0/s (250ms) and swinging a real mace at roughly a seventh
/// of its actual recharge -- landing nearly every hit at a sliver of its
/// damage-cooldown charge (see `crits`'s module doc comment for why that
/// curve makes an unrecognized weapon's cooldown a real damage bug, not
/// just a cosmetic one).
pub const MACE_COOLDOWN: Duration = Duration::from_millis(1667);

/// Minimum fall distance (blocks) a hit needs to trigger the smash bonus at
/// all -- below this a Mace hit is just its 5 base damage, same as swinging
/// flat-footed.
pub const SMASH_ATTACK_MIN_FALL: f64 = 1.5;

/// Whether attacking *right now* would land as a smash attack: falling, and
/// far enough into the fall to clear [`SMASH_ATTACK_MIN_FALL`]. Mirrors
/// `crits::is_critical_window`'s shape, but keyed to the Mace's much larger
/// fall requirement rather than "any downward velocity at all".
#[must_use]
pub fn is_smash_attack_window(on_ground: bool, velocity_y: f64, fall_distance: f64) -> bool {
    !on_ground && velocity_y < 0.0 && fall_distance >= SMASH_ATTACK_MIN_FALL
}

/// The server-computed smash-attack bonus damage for a given fall distance,
/// uncapped: 4 damage/block for the first 3 blocks, 2 damage/block for the
/// next 5 (blocks 4-8), 1 damage/block beyond that. Never used to *decide*
/// anything (the server applies the real bonus automatically the instant a
/// smash lands, regardless of what this function says) -- `combat::executor`
/// reports it in the "Smash attack landed" log line so the number in chat
/// matches what the target actually took.
#[must_use]
pub fn smash_bonus_damage(fall_distance: f64) -> f64 {
    if fall_distance < SMASH_ATTACK_MIN_FALL {
        return 0.0;
    }
    let first_three_blocks = fall_distance.min(3.0) * 4.0;
    let next_five_blocks = (fall_distance - 3.0).clamp(0.0, 5.0) * 2.0;
    let remaining_blocks = (fall_distance - 8.0).max(0.0);
    first_three_blocks + next_five_blocks + remaining_blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_the_threshold_never_triggers() {
        assert!(!is_smash_attack_window(false, -1.0, 1.4));
        assert!(!is_smash_attack_window(false, -1.0, 0.0));
    }

    #[test]
    fn on_ground_or_rising_never_triggers_even_with_enough_recorded_fall() {
        assert!(!is_smash_attack_window(true, -1.0, 5.0), "on the ground");
        assert!(!is_smash_attack_window(false, 0.5, 5.0), "still rising");
    }

    #[test]
    fn falling_past_the_threshold_triggers() {
        assert!(is_smash_attack_window(false, -0.5, 1.5));
        assert!(is_smash_attack_window(false, -0.5, 10.0));
    }

    #[test]
    fn the_cooldown_matches_the_vanilla_attack_speed_attribute() {
        // 1 / 0.6 attacks-per-second = 1.6666...s.
        assert!((MACE_COOLDOWN.as_secs_f64() - 1.0 / 0.6).abs() < 0.001);
    }

    #[test]
    fn smash_bonus_is_zero_below_the_threshold() {
        assert_eq!(smash_bonus_damage(0.0), 0.0);
        assert_eq!(smash_bonus_damage(1.4), 0.0);
    }

    #[test]
    fn first_three_blocks_are_four_damage_each() {
        assert_eq!(smash_bonus_damage(3.0), 12.0);
    }

    #[test]
    fn blocks_four_through_eight_are_two_damage_each() {
        assert_eq!(smash_bonus_damage(8.0), 12.0 + 10.0);
    }

    #[test]
    fn blocks_past_eight_are_one_damage_each_uncapped() {
        assert_eq!(smash_bonus_damage(10.0), 12.0 + 10.0 + 2.0);
        assert_eq!(smash_bonus_damage(20.0), 12.0 + 10.0 + 12.0);
    }

    #[test]
    fn bonus_damage_increases_monotonically_with_fall_distance() {
        let samples = [0.0, 1.0, 1.5, 2.0, 3.0, 5.0, 8.0, 15.0, 50.0];
        for pair in samples.windows(2) {
            assert!(
                smash_bonus_damage(pair[0]) <= smash_bonus_damage(pair[1]),
                "{} -> {}",
                pair[0],
                pair[1]
            );
        }
    }
}
