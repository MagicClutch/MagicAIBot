//! Pure food-selection policy for `crate::combat`'s healing flow. No
//! Azalea types, no I/O -- `crate::minecraft::client::MinecraftClient::
//! food_snapshot` (which does touch Azalea, to read each held item's real
//! `Food` component data) supplies the candidates [`best_food`] ranks.

/// Exact-match priority order for the foods the spec calls out by name
/// (golden apple first for its regeneration effect, then roughly
/// highest-to-lowest nutrition among common meats) -- checked before
/// falling back to nutrition alone for anything else edible. Earlier
/// entries win.
const NAMED_PRIORITY: &[&str] = &[
    "minecraft:golden_apple",
    "minecraft:cooked_beef",
    "minecraft:cooked_porkchop",
    "minecraft:cooked_mutton",
    "minecraft:cooked_chicken",
    "minecraft:bread",
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoodOption<'a> {
    pub slot: usize,
    pub item_id: &'a str,
    pub nutrition: i32,
}

/// Picks the best food to eat right now: the highest-priority named item
/// from [`NAMED_PRIORITY`] if any is held, otherwise the highest-nutrition
/// food available at all -- the spec's "Other edible food" catch-all --
/// so the bot never goes hungry with food in reach just because it isn't
/// one of the six explicitly named items. Ties (equal nutrition) resolve
/// toward whichever candidate appears first in `candidates`.
pub fn best_food<'a>(candidates: &[FoodOption<'a>]) -> Option<FoodOption<'a>> {
    NAMED_PRIORITY
        .iter()
        .find_map(|&wanted| candidates.iter().find(|c| c.item_id == wanted).copied())
        .or_else(|| candidates.iter().max_by_key(|c| c.nutrition).copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(slot: usize, item_id: &str, nutrition: i32) -> FoodOption<'_> {
        FoodOption {
            slot,
            item_id,
            nutrition,
        }
    }

    #[test]
    fn no_food_is_none() {
        assert_eq!(best_food(&[]), None);
    }

    #[test]
    fn prefers_golden_apple_over_everything_else() {
        let candidates = [
            option(0, "minecraft:cooked_beef", 8),
            option(1, "minecraft:golden_apple", 4),
        ];
        assert_eq!(best_food(&candidates), Some(candidates[1]));
    }

    #[test]
    fn follows_the_named_priority_order_when_no_golden_apple_is_held() {
        let candidates = [
            option(0, "minecraft:cooked_chicken", 6),
            option(1, "minecraft:cooked_porkchop", 8),
        ];
        assert_eq!(best_food(&candidates), Some(candidates[1]));
    }

    #[test]
    fn falls_back_to_highest_nutrition_for_unnamed_food() {
        let candidates = [
            option(0, "minecraft:melon_slice", 2),
            option(1, "minecraft:pumpkin_pie", 8),
            option(2, "minecraft:sweet_berries", 2),
        ];
        assert_eq!(best_food(&candidates), Some(candidates[1]));
    }

    #[test]
    fn a_named_priority_item_still_wins_over_higher_nutrition_unnamed_food() {
        let candidates = [
            option(0, "minecraft:rabbit_stew", 10),
            option(1, "minecraft:bread", 5),
        ];
        assert_eq!(best_food(&candidates), Some(candidates[1]));
    }
}
