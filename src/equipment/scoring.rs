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
}
