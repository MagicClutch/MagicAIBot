//! Pure armor scoring and classification. No Azalea types, no I/O -- reused
//! by `crate::equipment::manager::EquipmentService` and directly
//! unit-tested here.

use crate::{
    config::ArmorMode,
    equipment::{model::EquipmentItem, scoring::penalize_for_durability},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArmorSlot {
    Head,
    Chest,
    Legs,
    Feet,
}

impl ArmorSlot {
    pub const ALL: [ArmorSlot; 4] = [
        ArmorSlot::Head,
        ArmorSlot::Chest,
        ArmorSlot::Legs,
        ArmorSlot::Feet,
    ];

    /// The protocol slot index of this piece inside the player's own
    /// inventory menu (`azalea_inventory::Player::ARMOR_SLOTS` is `5..=8`,
    /// in exactly this head/chest/legs/feet order -- see
    /// `azalea_entity::inventory::Inventory::get_equipment`, which maps
    /// `armor[0..=3]` the same way).
    pub const fn protocol_slot(self) -> usize {
        match self {
            ArmorSlot::Head => 5,
            ArmorSlot::Chest => 6,
            ArmorSlot::Legs => 7,
            ArmorSlot::Feet => 8,
        }
    }

    /// Index into `crate::equipment::model::EquipmentSnapshot::armor_worn`.
    pub const fn index(self) -> usize {
        match self {
            ArmorSlot::Head => 0,
            ArmorSlot::Chest => 1,
            ArmorSlot::Legs => 2,
            ArmorSlot::Feet => 3,
        }
    }
}

/// Ordered worst to best -- also the `rarity` mode ranking
/// (Netherite > Diamond > Iron > Chainmail > Copper > Gold > Leather), so
/// `rarity` mode can just compare `ArmorMaterial` directly instead of
/// maintaining a second ranking table.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum ArmorMaterial {
    Leather,
    Gold,
    Copper,
    Chainmail,
    Iron,
    Diamond,
    Netherite,
}

impl ArmorMaterial {
    pub const fn base_score(self) -> f32 {
        match self {
            ArmorMaterial::Leather => 1.0,
            ArmorMaterial::Gold => 2.0,
            ArmorMaterial::Copper => 2.5,
            ArmorMaterial::Chainmail => 3.0,
            ArmorMaterial::Iron => 5.0,
            ArmorMaterial::Diamond => 8.0,
            ArmorMaterial::Netherite => 10.0,
        }
    }
}

/// Classifies a plain armor item id (namespaced or not, e.g.
/// `minecraft:diamond_chestplate` or `diamond_chestplate`) into the slot it
/// occupies and the material it's made of. Returns `None` for anything that
/// isn't one of the seven vanilla armor materials -- turtle helmets, wolf
/// armor, elytra, carved pumpkins, and the like are simply never picked as
/// a candidate, and if one is already worn it's treated the same as an
/// empty slot (see `should_replace`).
pub fn classify(item_id: &str) -> Option<(ArmorSlot, ArmorMaterial)> {
    let bare = item_id.rsplit(':').next().unwrap_or(item_id);
    let (material, slot) = if let Some(material) = bare.strip_suffix("_helmet") {
        (material, ArmorSlot::Head)
    } else if let Some(material) = bare.strip_suffix("_chestplate") {
        (material, ArmorSlot::Chest)
    } else if let Some(material) = bare.strip_suffix("_leggings") {
        (material, ArmorSlot::Legs)
    } else if let Some(material) = bare.strip_suffix("_boots") {
        (material, ArmorSlot::Feet)
    } else {
        return None;
    };
    let material = match material {
        "leather" => ArmorMaterial::Leather,
        "golden" => ArmorMaterial::Gold,
        "copper" => ArmorMaterial::Copper,
        "chainmail" => ArmorMaterial::Chainmail,
        "iron" => ArmorMaterial::Iron,
        "diamond" => ArmorMaterial::Diamond,
        "netherite" => ArmorMaterial::Netherite,
        _ => return None,
    };
    Some((slot, material))
}

/// `score` mode: a 1.0-10.0 rating for one armor piece, rounded to one
/// decimal place. Material sets the ceiling; `scoring::penalize_for_durability`
/// pulls it down for wear, so a badly damaged Netherite piece can score
/// below a fully-repaired Diamond one.
pub fn score(material: ArmorMaterial, current_durability: u32, max_durability: u32) -> f32 {
    penalize_for_durability(material.base_score(), current_durability, max_durability)
}

/// One inventory candidate for a given `ArmorSlot`, paired with its parsed
/// material.
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub item: &'a EquipmentItem,
    pub material: ArmorMaterial,
}

/// Finds the best-ranked candidate for `slot` among `inventory`, per `mode`.
/// Ties resolve toward whichever candidate is later in `inventory` --
/// harmless in practice since callers always build `inventory` by scanning
/// protocol slots in the same ascending order every time, so a tie between
/// two otherwise-identical pieces resolves the same way on every tick
/// instead of flickering.
pub fn best_candidate<'a>(
    mode: ArmorMode,
    slot: ArmorSlot,
    inventory: &'a [EquipmentItem],
) -> Option<Candidate<'a>> {
    let mut best: Option<Candidate<'a>> = None;
    for item in inventory {
        let Some((item_slot, material)) = classify(&item.item_id) else {
            continue;
        };
        if item_slot != slot {
            continue;
        }
        let candidate = Candidate { item, material };
        let better = match &best {
            None => true,
            Some(current) => match mode {
                ArmorMode::Score => rank_score(candidate) >= rank_score(*current),
                ArmorMode::Rarity => candidate.material >= current.material,
            },
        };
        if better {
            best = Some(candidate);
        }
    }
    best
}

fn rank_score(candidate: Candidate) -> f32 {
    score(
        candidate.material,
        candidate.item.current_durability,
        candidate.item.max_durability,
    )
}

/// Whether `candidate` should replace whatever (if anything) is currently
/// worn. `score` mode requires at least a 0.2-point improvement; `rarity`
/// mode requires a strictly higher material tier and ignores durability
/// entirely. Both thresholds exist purely to stop the bot from swapping
/// between two near-identical pieces on every re-evaluation.
pub fn should_replace(mode: ArmorMode, worn: Option<&EquipmentItem>, candidate: Candidate) -> bool {
    let worn_material = worn.and_then(|item| classify(&item.item_id).map(|(_, material)| material));
    match mode {
        ArmorMode::Score => {
            let worn_score = worn.zip(worn_material).map_or(0.0, |(item, material)| {
                score(material, item.current_durability, item.max_durability)
            });
            rank_score(candidate) >= worn_score + 0.2
        }
        ArmorMode::Rarity => worn_material.is_none_or(|material| candidate.material > material),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(slot: usize, id: &str, current: u32, max: u32) -> EquipmentItem {
        EquipmentItem {
            slot,
            item_id: id.into(),
            current_durability: current,
            max_durability: max,
        }
    }

    #[test]
    fn classifies_every_material_and_slot() {
        assert_eq!(
            classify("minecraft:netherite_helmet"),
            Some((ArmorSlot::Head, ArmorMaterial::Netherite))
        );
        assert_eq!(
            classify("golden_chestplate"),
            Some((ArmorSlot::Chest, ArmorMaterial::Gold))
        );
        assert_eq!(
            classify("minecraft:iron_leggings"),
            Some((ArmorSlot::Legs, ArmorMaterial::Iron))
        );
        assert_eq!(
            classify("minecraft:diamond_boots"),
            Some((ArmorSlot::Feet, ArmorMaterial::Diamond))
        );
        assert_eq!(
            classify("minecraft:copper_chestplate"),
            Some((ArmorSlot::Chest, ArmorMaterial::Copper))
        );
        assert_eq!(classify("minecraft:carved_pumpkin"), None);
        assert_eq!(classify("minecraft:elytra"), None);
    }

    #[test]
    fn copper_ranks_between_gold_and_chainmail() {
        assert!(ArmorMaterial::Copper > ArmorMaterial::Gold);
        assert!(ArmorMaterial::Copper < ArmorMaterial::Chainmail);
    }

    #[test]
    fn full_durability_scores_match_base_material_score() {
        assert_eq!(score(ArmorMaterial::Netherite, 100, 100), 10.0);
        assert_eq!(score(ArmorMaterial::Diamond, 100, 100), 8.0);
        assert_eq!(score(ArmorMaterial::Iron, 100, 100), 5.0);
    }

    #[test]
    fn damaged_netherite_can_fall_below_undamaged_diamond() {
        // 10% remaining: 10 - (1 - 0.1) * 6 = 4.6.
        let damaged_netherite = score(ArmorMaterial::Netherite, 10, 100);
        let full_diamond = score(ArmorMaterial::Diamond, 100, 100);
        assert!(damaged_netherite < full_diamond);
    }

    #[test]
    fn score_never_leaves_the_one_to_ten_range() {
        assert_eq!(score(ArmorMaterial::Leather, 0, 100), 1.0);
        assert_eq!(score(ArmorMaterial::Netherite, 100, 100), 10.0);
    }

    #[test]
    fn missing_durability_data_is_treated_as_full_durability() {
        assert_eq!(score(ArmorMaterial::Diamond, 0, 0), 8.0);
    }

    #[test]
    fn best_candidate_in_score_mode_prefers_higher_score_over_higher_tier() {
        let inventory = vec![
            item(10, "minecraft:netherite_helmet", 1, 100), // ~1% durability -> score ~4.1
            item(11, "minecraft:diamond_helmet", 100, 100), // full durability -> score 8.0
        ];
        let best = best_candidate(ArmorMode::Score, ArmorSlot::Head, &inventory).unwrap();
        assert_eq!(best.item.slot, 11);
    }

    #[test]
    fn best_candidate_in_rarity_mode_ignores_durability() {
        let inventory = vec![
            item(10, "minecraft:netherite_helmet", 1, 100), // ruined, but still Netherite
            item(11, "minecraft:diamond_helmet", 100, 100),
        ];
        let best = best_candidate(ArmorMode::Rarity, ArmorSlot::Head, &inventory).unwrap();
        assert_eq!(best.item.slot, 10);
    }

    #[test]
    fn should_replace_requires_a_meaningful_score_improvement() {
        let candidate_item = item(1, "minecraft:diamond_helmet", 100, 100); // 8.0
        let candidate = Candidate {
            item: &candidate_item,
            material: ArmorMaterial::Diamond,
        };
        let worn_barely_lower = item(5, "minecraft:diamond_helmet", 98, 100); // 7.88 -> 7.9
        assert!(!should_replace(
            ArmorMode::Score,
            Some(&worn_barely_lower),
            candidate
        ));
        let worn_much_lower = item(5, "minecraft:diamond_helmet", 50, 100); // 5.0
        assert!(should_replace(
            ArmorMode::Score,
            Some(&worn_much_lower),
            candidate
        ));
        assert!(should_replace(ArmorMode::Score, None, candidate));
    }

    #[test]
    fn should_replace_in_rarity_mode_ignores_durability_and_requires_a_higher_tier() {
        let candidate_item = item(1, "minecraft:diamond_helmet", 1, 100);
        let candidate = Candidate {
            item: &candidate_item,
            material: ArmorMaterial::Diamond,
        };
        let worn_same_tier = item(5, "minecraft:diamond_helmet", 100, 100);
        assert!(!should_replace(
            ArmorMode::Rarity,
            Some(&worn_same_tier),
            candidate
        ));
        let worn_lower_tier = item(5, "minecraft:iron_helmet", 100, 100);
        assert!(should_replace(
            ArmorMode::Rarity,
            Some(&worn_lower_tier),
            candidate
        ));
    }

    #[test]
    fn should_replace_treats_an_unclassifiable_worn_item_as_empty() {
        let candidate_item = item(1, "minecraft:leather_helmet", 100, 100);
        let candidate = Candidate {
            item: &candidate_item,
            material: ArmorMaterial::Leather,
        };
        let worn_pumpkin = item(5, "minecraft:carved_pumpkin", 0, 0);
        assert!(should_replace(
            ArmorMode::Score,
            Some(&worn_pumpkin),
            candidate
        ));
    }
}
