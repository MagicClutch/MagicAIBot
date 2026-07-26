//! Pure, read-only models and calculations for furnace-like stations.
//! Azalea menu/packet types remain behind this snapshot adapter boundary.
use crate::minecraft::world_state::InventorySnapshot;

pub const STANDARD_COOK_TICKS: u32 = 200;
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StationKind {
    Furnace,
    BlastFurnace,
    Smoker,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StationIdentity {
    pub kind: StationKind,
    pub container_id: i32,
    pub dimension: String,
    pub position: Option<(i32, i32, i32)>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StackSnapshot {
    pub item_id: Option<String>,
    pub count: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessingStationSnapshot {
    pub identity: StationIdentity,
    pub revision: u64,
    pub input: StackSnapshot,
    pub fuel: StackSnapshot,
    pub output: StackSnapshot,
    pub burn_remaining_ticks: u32,
    pub burn_total_ticks: u32,
    pub cook_progress_ticks: u32,
    pub cook_total_ticks: Option<u32>,
}
impl ProcessingStationSnapshot {
    /// Adapts Azalea's pinned furnace/blast-furnace/smoker three-slot layout and
    /// vanilla `ClientboundContainerSetData` property ordering.
    pub fn from_menu(
        identity: StationIdentity,
        revision: u64,
        slots: &[StackSnapshot],
        properties: &[u16],
    ) -> Result<Self, ProcessingError> {
        if slots.len() < 3 {
            return Err(ProcessingError::InvalidLayout { slots: slots.len() });
        }
        let p = |i| properties.get(i).copied().map(u32::from);
        Ok(Self {
            identity,
            revision,
            input: slots[0].clone(),
            fuel: slots[1].clone(),
            output: slots[2].clone(),
            burn_remaining_ticks: p(0).unwrap_or(0),
            burn_total_ticks: p(1).unwrap_or(0),
            cook_progress_ticks: p(2).unwrap_or(0),
            cook_total_ticks: p(3),
        })
    }
    pub fn active(&self) -> bool {
        self.burn_remaining_ticks > 0
    }
    pub fn burn_progress(&self) -> Option<f32> {
        ratio(self.burn_remaining_ticks, self.burn_total_ticks)
    }
    pub fn cook_progress(&self) -> Option<f32> {
        ratio(self.cook_progress_ticks, self.cook_total_ticks?)
    }
}
fn ratio(value: u32, total: u32) -> Option<f32> {
    (total != 0).then(|| value.min(total) as f32 / total as f32)
}

#[derive(Clone, Debug, PartialEq)]
pub struct CookingRecipe {
    pub id: String,
    pub station: StationKind,
    pub compatible_inputs: Vec<String>,
    pub output_item: String,
    pub output_count: u32,
    pub cooking_ticks: u32,
    pub experience: f32,
    pub data_revision: u64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fuel {
    pub item_id: String,
    pub burn_ticks: u32,
    pub preference: u16,
    pub protected: bool,
    pub emergency_only: bool,
    pub remainder: Option<String>,
}
impl Fuel {
    pub fn standard_operations(&self) -> f32 {
        self.burn_ticks as f32 / STANDARD_COOK_TICKS as f32
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FuelChoice {
    pub item_id: String,
    pub items: u32,
    pub supplied_ticks: u64,
    pub waste_ticks: u64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessingRequirements {
    pub operations: u32,
    pub required_input: u32,
    pub available_input: u32,
    pub missing_input: u32,
    pub expected_output: u32,
    pub required_burn_ticks: u64,
    pub fuel: Option<FuelChoice>,
    pub missing_burn_ticks: u64,
    pub cooking_ticks: u64,
    pub experience: f32,
}
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ProcessingError {
    #[error("processing-station menu has {slots} slots; at least 3 are required")]
    InvalidLayout { slots: usize },
    #[error("recipe {recipe} is incompatible with {station:?}")]
    IncompatibleStation {
        recipe: String,
        station: StationKind,
    },
    #[error("recipe data is missing or invalid")]
    MissingRecipeData,
}
#[derive(Clone, Debug, Default)]
pub struct ProcessingKnowledge {
    pub revision: u64,
    recipes: Vec<CookingRecipe>,
    fuels: Vec<Fuel>,
}
impl ProcessingKnowledge {
    pub fn new(revision: u64, recipes: Vec<CookingRecipe>, fuels: Vec<Fuel>) -> Self {
        Self {
            revision,
            recipes,
            fuels,
        }
    }
    /// Versioned fallback data. Server/datapack ingestion can replace this catalog.
    pub fn vanilla_furnace() -> Self {
        let recipe = |id: &str, input: &str, output: &str, xp| CookingRecipe {
            id: id.into(),
            station: StationKind::Furnace,
            compatible_inputs: vec![input.into()],
            output_item: output.into(),
            output_count: 1,
            cooking_ticks: STANDARD_COOK_TICKS,
            experience: xp,
            data_revision: 1,
        };
        Self::new(
            1,
            vec![
                recipe(
                    "minecraft:iron_ingot_from_smelting_raw_iron",
                    "minecraft:raw_iron",
                    "minecraft:iron_ingot",
                    0.7,
                ),
                recipe(
                    "minecraft:gold_ingot_from_smelting_raw_gold",
                    "minecraft:raw_gold",
                    "minecraft:gold_ingot",
                    1.0,
                ),
                recipe("minecraft:glass", "minecraft:sand", "minecraft:glass", 0.1),
                recipe(
                    "minecraft:cooked_beef",
                    "minecraft:beef",
                    "minecraft:cooked_beef",
                    0.35,
                ),
            ],
            vanilla_fuels(),
        )
    }
    pub fn recipe(&self, id: &str) -> Option<&CookingRecipe> {
        self.recipes.iter().find(|r| r.id == id)
    }
    pub fn recipes_for_output(&self, item: &str) -> Vec<&CookingRecipe> {
        self.recipes
            .iter()
            .filter(|r| r.output_item == item)
            .collect()
    }
    pub fn recipes_for_input(&self, item: &str) -> Vec<&CookingRecipe> {
        self.recipes
            .iter()
            .filter(|r| r.compatible_inputs.iter().any(|i| i == item))
            .collect()
    }
    pub fn fuel(&self, item: &str) -> Option<&Fuel> {
        self.fuels.iter().find(|f| f.item_id == item)
    }
    pub fn fuels(&self) -> &[Fuel] {
        &self.fuels
    }
    pub fn requirements(
        &self,
        recipe: &CookingRecipe,
        station: StationKind,
        requested_output: u32,
        inventory: &InventorySnapshot,
    ) -> Result<ProcessingRequirements, ProcessingError> {
        if recipe.station != station {
            return Err(ProcessingError::IncompatibleStation {
                recipe: recipe.id.clone(),
                station,
            });
        }
        if recipe.output_count == 0
            || recipe.cooking_ticks == 0
            || recipe.compatible_inputs.is_empty()
        {
            return Err(ProcessingError::MissingRecipeData);
        }
        let operations = ceil32(requested_output, recipe.output_count);
        let available_input = recipe
            .compatible_inputs
            .iter()
            .map(|i| inventory.count_item(i))
            .sum();
        let required_burn_ticks = u64::from(operations) * u64::from(recipe.cooking_ticks);
        Ok(ProcessingRequirements {
            operations,
            required_input: operations,
            available_input,
            missing_input: operations.saturating_sub(available_input),
            expected_output: operations.saturating_mul(recipe.output_count),
            required_burn_ticks,
            fuel: self.choose_fuel(required_burn_ticks, inventory),
            missing_burn_ticks: required_burn_ticks
                .saturating_sub(self.available_fuel_ticks(inventory)),
            cooking_ticks: required_burn_ticks,
            experience: operations as f32 * recipe.experience,
        })
    }
    /// Ranks sufficient, unprotected fuel by waste, preference, item count, id.
    pub fn choose_fuel(&self, ticks: u64, inv: &InventorySnapshot) -> Option<FuelChoice> {
        self.fuels
            .iter()
            .filter(|f| !f.protected && !f.emergency_only && f.burn_ticks > 0)
            .filter_map(|f| {
                let n = ceil64(ticks, u64::from(f.burn_ticks));
                (u64::from(inv.count_item(&f.item_id)) >= n).then(|| {
                    (
                        FuelChoice {
                            item_id: f.item_id.clone(),
                            items: n as u32,
                            supplied_ticks: n * u64::from(f.burn_ticks),
                            waste_ticks: n * u64::from(f.burn_ticks) - ticks,
                        },
                        f.preference,
                    )
                })
            })
            .min_by_key(|(c, p)| (c.waste_ticks, *p, c.items, c.item_id.clone()))
            .map(|x| x.0)
    }
    pub fn available_fuel_ticks(&self, inv: &InventorySnapshot) -> u64 {
        self.fuels
            .iter()
            .filter(|f| !f.protected && !f.emergency_only)
            .map(|f| u64::from(inv.count_item(&f.item_id)) * u64::from(f.burn_ticks))
            .sum()
    }
}
fn vanilla_fuels() -> Vec<Fuel> {
    [
        ("minecraft:coal", 1600, 10),
        ("minecraft:charcoal", 1600, 20),
        ("minecraft:coal_block", 16000, 30),
        ("minecraft:blaze_rod", 2400, 40),
        ("minecraft:dried_kelp_block", 4000, 50),
        ("minecraft:stick", 100, 100),
    ]
    .into_iter()
    .map(|(id, burn, preference)| Fuel {
        item_id: id.into(),
        burn_ticks: burn,
        preference,
        protected: false,
        emergency_only: false,
        remainder: None,
    })
    .collect()
}
fn ceil32(v: u32, d: u32) -> u32 {
    v / d + u32::from(v % d != 0)
}
fn ceil64(v: u64, d: u64) -> u64 {
    v / d + u64::from(v % d != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    fn inv(items: &[(&str, u32)]) -> InventorySnapshot {
        InventorySnapshot {
            available: true,
            total_counts: items
                .iter()
                .map(|(i, n)| (i.to_string(), *n))
                .collect::<HashMap<_, _>>(),
            ..Default::default()
        }
    }
    fn identity(k: StationKind) -> StationIdentity {
        StationIdentity {
            kind: k,
            container_id: 2,
            dimension: "minecraft:overworld".into(),
            position: Some((1, 64, 2)),
        }
    }
    #[test]
    fn layout_and_progress() {
        let s = ProcessingStationSnapshot::from_menu(
            identity(StationKind::Furnace),
            7,
            &vec![StackSnapshot::default(); 3],
            &[800, 1600, 50, 200],
        )
        .unwrap();
        assert!(s.active());
        assert_eq!(s.revision, 7);
        assert_eq!(s.burn_progress(), Some(0.5));
        assert_eq!(s.cook_progress(), Some(0.25));
    }
    #[test]
    fn bad_layout_and_missing_data() {
        assert!(matches!(
            ProcessingStationSnapshot::from_menu(identity(StationKind::Furnace), 0, &[], &[]),
            Err(ProcessingError::InvalidLayout { .. })
        ));
        let s = ProcessingStationSnapshot::from_menu(
            identity(StationKind::Furnace),
            0,
            &vec![StackSnapshot::default(); 3],
            &[],
        )
        .unwrap();
        assert_eq!(s.cook_progress(), None);
    }
    #[test]
    fn recipe_rounding_and_missing_resources() {
        let r = CookingRecipe {
            id: "test:double".into(),
            station: StationKind::Furnace,
            compatible_inputs: vec!["test:ore".into()],
            output_item: "test:ingot".into(),
            output_count: 2,
            cooking_ticks: 200,
            experience: 0.5,
            data_revision: 9,
        };
        let k = ProcessingKnowledge::new(9, vec![r.clone()], vanilla_fuels());
        let q = k
            .requirements(
                &r,
                StationKind::Furnace,
                5,
                &inv(&[("test:ore", 2), ("minecraft:coal", 1)]),
            )
            .unwrap();
        assert_eq!(
            (
                q.operations,
                q.expected_output,
                q.missing_input,
                q.missing_burn_ticks
            ),
            (3, 6, 1, 0)
        );
    }
    #[test]
    fn fuel_choice_minimizes_waste() {
        let c = ProcessingKnowledge::vanilla_furnace()
            .choose_fuel(600, &inv(&[("minecraft:coal", 1), ("minecraft:stick", 6)]))
            .unwrap();
        assert_eq!(
            (c.item_id.as_str(), c.items, c.waste_ticks),
            ("minecraft:stick", 6, 0)
        );
    }
    #[test]
    fn protected_and_incompatible() {
        let r = ProcessingKnowledge::vanilla_furnace().recipes[0].clone();
        let k = ProcessingKnowledge::new(
            1,
            vec![r.clone()],
            vec![Fuel {
                item_id: "test:heirloom".into(),
                burn_ticks: 2000,
                preference: 0,
                protected: true,
                emergency_only: false,
                remainder: None,
            }],
        );
        assert!(k.choose_fuel(200, &inv(&[("test:heirloom", 1)])).is_none());
        assert!(matches!(
            k.requirements(&r, StationKind::Smoker, 1, &InventorySnapshot::default()),
            Err(ProcessingError::IncompatibleStation { .. })
        ));
    }
}
