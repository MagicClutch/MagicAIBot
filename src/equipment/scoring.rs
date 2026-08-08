//! Shared "material ceiling minus durability penalty" scoring formula.
//!
//! `equipment::armor::score` and `equipment::tools::rank_score` both rank a
//! piece of equipment on the same 1.0-10.0 scale, from the same shape of
//! input (a material-derived base score, plus current/max durability) --
//! this is the one implementation both call, rather than each maintaining
//! its own copy of the same formula. Everything category-specific (which
//! materials exist, what an item's base score is) stays in the two callers.

/// ```text
/// durabilityPercent = currentDurability / maxDurability
/// durabilityPenalty = (1 - durabilityPercent) * 6
/// score = clamp(baseScore - durabilityPenalty, 1, 10)
/// ```
///
/// An item with no durability data (`max_durability == 0`) is scored as if
/// fully repaired rather than excluded, matching how the rest of this
/// codebase treats unknown durability (see `interaction::tool_selection`).
pub fn penalize_for_durability(
    base_score: f32,
    current_durability: u32,
    max_durability: u32,
) -> f32 {
    if max_durability == 0 {
        return round_to_tenth(base_score.clamp(1.0, 10.0));
    }
    let durability_percent = (current_durability as f32 / max_durability as f32).min(1.0);
    let durability_penalty = (1.0 - durability_percent) * 6.0;
    round_to_tenth((base_score - durability_penalty).clamp(1.0, 10.0))
}

pub fn round_to_tenth(value: f32) -> f32 {
    (value * 10.0).round() / 10.0
}

/// Per-level score bonus for weapon enchantments, used by
/// `equipment::tools::rank_score`. Sharpness is weighted heaviest since it
/// helps in every fight; Smite/Bane of Arthropods are mob-specific (dead
/// weight in `#kill` PvP) but this ranking is shared with `#get <mob>`
/// combat too (see `equipment::tools`'s module doc comment), so they still
/// count, just less than a universally useful enchantment. Knockback/Fire
/// Aspect/Sweeping Edge are minor tactical bonuses, not damage-equivalent to
/// Sharpness.
///
/// Deliberately additive on top of [`penalize_for_durability`]'s 1.0-10.0
/// scale rather than folded into it before the clamp -- otherwise two
/// full-durability Netherite weapons (already clamped to the ceiling) could
/// never be told apart by enchantment even though one is strictly better.
/// Not a vanilla damage-formula simulation, just enough of a tiebreak that a
/// genuinely better-enchanted weapon or armor piece outranks a bare one of
/// the same or lower material tier.
pub const WEAPON_ENCHANTMENT_WEIGHTS: &[(&str, f32)] = &[
    ("sharpness", 0.5),
    ("smite", 0.25),
    ("bane_of_arthropods", 0.25),
    ("knockback", 0.15),
    ("fire_aspect", 0.1),
    ("sweeping_edge", 0.1),
];

/// Per-level score bonus for armor enchantments, used by
/// `equipment::armor::score`. Protection is weighted heaviest since it
/// reduces damage from every source; the specialized Protections and Thorns
/// are situational; Feather Falling only ever appears on boots but is cheap
/// insurance against the bot's own combat-driven jumping (see
/// `combat::crits`).
pub const ARMOR_ENCHANTMENT_WEIGHTS: &[(&str, f32)] = &[
    ("protection", 0.4),
    ("blast_protection", 0.2),
    ("fire_protection", 0.2),
    ("projectile_protection", 0.15),
    ("thorns", 0.15),
    ("feather_falling", 0.1),
];

/// Sums the weighted per-level bonus for every enchantment in `enchantments`
/// that appears in `weights` ([`WEAPON_ENCHANTMENT_WEIGHTS`] or
/// [`ARMOR_ENCHANTMENT_WEIGHTS`]); an item with none, or only enchantments
/// this scoring has no opinion on (Unbreaking, Mending, ...), contributes 0.
pub fn enchantment_bonus(enchantments: &[(String, u32)], weights: &[(&str, f32)]) -> f32 {
    enchantments
        .iter()
        .map(|(name, level)| {
            weights
                .iter()
                .find(|(candidate, _)| candidate == name)
                .map_or(0.0, |(_, weight)| weight * (*level as f32))
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_durability_matches_the_base_score() {
        assert_eq!(penalize_for_durability(10.0, 100, 100), 10.0);
        assert_eq!(penalize_for_durability(5.0, 100, 100), 5.0);
    }

    #[test]
    fn low_durability_can_drag_a_high_base_score_below_a_lower_one() {
        // 10% remaining: 10 - (1 - 0.1) * 6 = 4.6.
        assert!(penalize_for_durability(10.0, 10, 100) < penalize_for_durability(8.0, 100, 100));
    }

    #[test]
    fn score_never_leaves_the_one_to_ten_range() {
        assert_eq!(penalize_for_durability(1.0, 0, 100), 1.0);
        assert_eq!(penalize_for_durability(10.0, 100, 100), 10.0);
    }

    #[test]
    fn missing_durability_data_is_treated_as_full_durability() {
        assert_eq!(penalize_for_durability(8.0, 0, 0), 8.0);
    }

    #[test]
    fn unenchanted_item_gets_no_bonus() {
        assert_eq!(enchantment_bonus(&[], WEAPON_ENCHANTMENT_WEIGHTS), 0.0);
    }

    #[test]
    fn known_weapon_enchantments_scale_with_level() {
        assert_eq!(
            enchantment_bonus(&[("sharpness".to_owned(), 5)], WEAPON_ENCHANTMENT_WEIGHTS),
            2.5
        );
    }

    #[test]
    fn multiple_enchantments_sum() {
        let enchantments = [("sharpness".to_owned(), 5), ("knockback".to_owned(), 2)];
        assert_eq!(
            enchantment_bonus(&enchantments, WEAPON_ENCHANTMENT_WEIGHTS),
            2.5 + 0.3
        );
    }

    #[test]
    fn an_enchantment_this_scoring_has_no_opinion_on_contributes_nothing() {
        // Unbreaking helps durability, not raw combat power, and isn't in
        // either weight table.
        assert_eq!(
            enchantment_bonus(&[("unbreaking".to_owned(), 3)], WEAPON_ENCHANTMENT_WEIGHTS),
            0.0
        );
    }

    #[test]
    fn a_mob_specific_weapon_enchantment_is_weighted_lower_than_sharpness() {
        let sharpness =
            enchantment_bonus(&[("sharpness".to_owned(), 1)], WEAPON_ENCHANTMENT_WEIGHTS);
        let smite = enchantment_bonus(&[("smite".to_owned(), 1)], WEAPON_ENCHANTMENT_WEIGHTS);
        assert!(sharpness > smite);
    }

    #[test]
    fn armor_weights_are_independent_of_weapon_weights() {
        // "sharpness" means nothing on armor.
        assert_eq!(
            enchantment_bonus(&[("sharpness".to_owned(), 5)], ARMOR_ENCHANTMENT_WEIGHTS),
            0.0
        );
        assert_eq!(
            enchantment_bonus(&[("protection".to_owned(), 4)], ARMOR_ENCHANTMENT_WEIGHTS),
            1.6
        );
    }
}
