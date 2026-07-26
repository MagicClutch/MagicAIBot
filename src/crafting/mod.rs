//! Read-only, deterministic crafting knowledge and dependency planning.
//!
//! Azalea 6249c29 (protocol 776 / Minecraft 26.2) exposes recipe displays in
//! recipe-book packets, but does not retain them in client state and those
//! displays do not carry resource-location recipe IDs. Until that runtime feed
//! is available here, `vanilla_26_2.json` is an intentionally small diagnostic
//! fallback. It is never presented as the complete vanilla recipe set.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::minecraft::world_state::InventorySnapshot;

const FALLBACK: &str = include_str!("vanilla_26_2.json");

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Station {
    Player,
    CraftingTable,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum Ingredient {
    Empty,
    Item(String),
    Alternatives(Vec<String>),
    Tag(String),
    #[serde(rename = "special")]
    Special(String),
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct IngredientSlot {
    pub ingredient: Ingredient,
    #[serde(default = "one")]
    pub count: u32,
}
const fn one() -> u32 {
    1
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case", tag = "layout")]
pub enum RecipeLayout {
    Shaped {
        width: u8,
        height: u8,
        ingredients: Vec<IngredientSlot>,
    },
    Shapeless {
        ingredients: Vec<IngredientSlot>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub output: String,
    pub output_count: u32,
    pub station: Station,
    #[serde(flatten)]
    pub layout: RecipeLayout,
    #[serde(default = "yes")]
    pub known: bool,
    #[serde(default)]
    pub special: bool,
}
const fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize)]
struct Dataset {
    version: String,
    protocol: i32,
    revision: String,
    tags: BTreeMap<String, Vec<String>>,
    recipes: Vec<Recipe>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceInfo {
    pub version: String,
    pub protocol: i32,
    pub revision: String,
    pub complete: bool,
}

#[derive(Clone, Debug)]
pub struct RecipeBook {
    source: SourceInfo,
    recipes: BTreeMap<String, Recipe>,
    by_output: BTreeMap<String, Vec<String>>,
    tags: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Failure {
    UnknownRecipe(String),
    UnknownItem(String),
    /// Reserved for a runtime source that has no dataset for its protocol.
    #[allow(dead_code)]
    DataUnavailable { version: String },
    UnsupportedIngredient(String),
    /// Returned by future runtime adapters for furnace/smithing displays.
    #[allow(dead_code)]
    UnsupportedRecipeType(String),
    StationUnavailable(Station),
    Cycle { path: Vec<String> },
    DepthLimit { item: String, limit: usize },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Missing {
    pub ingredient: String,
    pub count: u32,
    pub alternatives: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Availability {
    pub crafts: u32,
    pub maximum_crafts: u32,
    pub missing: Vec<Missing>,
    pub station_required: Station,
    pub failure: Option<Failure>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanStep {
    pub recipe_id: String,
    pub operations: u32,
    pub output: String,
    pub produced: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CraftPlan {
    pub requested: u32,
    pub steps: Vec<PlanStep>,
    pub remaining: BTreeMap<String, u32>,
    pub failure: Option<Failure>,
}

impl RecipeBook {
    pub fn fallback() -> Result<Self, String> {
        let data: Dataset = serde_json::from_str(FALLBACK)
            .map_err(|e| format!("malformed fallback recipe data: {e}"))?;
        let mut recipes = BTreeMap::new();
        let mut by_output: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for recipe in data.recipes {
            validate_recipe(&recipe)?;
            by_output
                .entry(recipe.output.clone())
                .or_default()
                .push(recipe.id.clone());
            if recipes.insert(recipe.id.clone(), recipe).is_some() {
                return Err("duplicate recipe id".into());
            }
        }
        Ok(Self {
            source: SourceInfo {
                version: data.version,
                protocol: data.protocol,
                revision: data.revision,
                complete: false,
            },
            recipes,
            by_output,
            tags: data.tags,
        })
    }

    pub fn source(&self) -> &SourceInfo {
        &self.source
    }
    pub fn recipe(&self, id: &str) -> Result<&Recipe, Failure> {
        self.recipes
            .get(&normalize(id))
            .ok_or_else(|| Failure::UnknownRecipe(normalize(id)))
    }
    pub fn recipes_for(&self, output: &str) -> Vec<&Recipe> {
        self.by_output
            .get(&normalize(output))
            .into_iter()
            .flatten()
            .filter_map(|id| self.recipes.get(id))
            .collect()
    }
    /// Stable preference: known, non-special, player-grid capable, then ID.
    pub fn preferred(&self, output: &str) -> Result<&Recipe, Failure> {
        self.recipes_for(output)
            .into_iter()
            .min_by_key(|r| {
                (
                    !r.known,
                    r.special,
                    r.station == Station::CraftingTable,
                    &r.id,
                )
            })
            .ok_or_else(|| Failure::UnknownItem(normalize(output)))
    }

    pub fn availability(
        &self,
        recipe: &Recipe,
        inventory: &InventorySnapshot,
        table_available: bool,
        operations: u32,
    ) -> Availability {
        if recipe.station == Station::CraftingTable && !table_available {
            return Availability {
                crafts: 0,
                maximum_crafts: 0,
                missing: vec![],
                station_required: recipe.station.clone(),
                failure: Some(Failure::StationUnavailable(Station::CraftingTable)),
            };
        }
        let mut virtual_inventory: BTreeMap<String, u32> = inventory
            .total_counts
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let maximum_crafts = self.maximum_operations(recipe, &virtual_inventory);
        let mut missing = Vec::new();
        for _ in 0..operations {
            self.consume_operation(recipe, &mut virtual_inventory, &mut missing);
        }
        consolidate_missing(&mut missing);
        Availability {
            crafts: operations.min(maximum_crafts),
            maximum_crafts,
            missing,
            station_required: recipe.station.clone(),
            failure: None,
        }
    }

    pub fn plan(
        &self,
        output: &str,
        count: u32,
        inventory: &InventorySnapshot,
        table_available: bool,
        max_depth: usize,
    ) -> CraftPlan {
        let mut stock: BTreeMap<String, u32> = inventory
            .total_counts
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let mut steps = Vec::new();
        let mut path = Vec::new();
        let failure = self
            .ensure(
                &normalize(output),
                count,
                table_available,
                max_depth,
                0,
                &mut path,
                &mut stock,
                &mut steps,
            )
            .err();
        CraftPlan {
            requested: count,
            steps,
            remaining: stock,
            failure,
        }
    }

    fn ensure(
        &self,
        item: &str,
        count: u32,
        table: bool,
        limit: usize,
        depth: usize,
        path: &mut Vec<String>,
        stock: &mut BTreeMap<String, u32>,
        steps: &mut Vec<PlanStep>,
    ) -> Result<(), Failure> {
        let held = stock.get(item).copied().unwrap_or(0);
        if held >= count {
            stock.insert(item.into(), held - count);
            return Ok(());
        }
        let need = count - held;
        stock.remove(item);
        if depth >= limit {
            return Err(Failure::DepthLimit {
                item: item.into(),
                limit,
            });
        }
        if path.iter().any(|p| p == item) {
            let mut p = path.clone();
            p.push(item.into());
            return Err(Failure::Cycle { path: p });
        }
        let recipe = self.preferred(item)?;
        if recipe.station == Station::CraftingTable && !table {
            return Err(Failure::StationUnavailable(Station::CraftingTable));
        }
        let operations = need.div_ceil(recipe.output_count);
        path.push(item.into());
        for slot in ingredients(&recipe.layout) {
            if matches!(slot.ingredient, Ingredient::Empty) {
                continue;
            }
            let amount = slot.count.saturating_mul(operations);
            let choices = self.choices(&slot.ingredient)?;
            let choice = choices
                .iter()
                .min_by_key(|id| {
                    (
                        std::cmp::Reverse(stock.get(*id).copied().unwrap_or(0)),
                        !self.by_output.contains_key(*id),
                        (*id).clone(),
                    )
                })
                .cloned()
                .ok_or_else(|| Failure::UnsupportedIngredient("empty alternatives".into()))?;
            self.ensure(&choice, amount, table, limit, depth + 1, path, stock, steps)?;
        }
        path.pop();
        let produced = operations * recipe.output_count;
        *stock.entry(item.into()).or_default() += produced - need;
        steps.push(PlanStep {
            recipe_id: recipe.id.clone(),
            operations,
            output: item.into(),
            produced,
        });
        Ok(())
    }

    fn choices(&self, ingredient: &Ingredient) -> Result<Vec<String>, Failure> {
        match ingredient {
            Ingredient::Item(i) => Ok(vec![i.clone()]),
            Ingredient::Alternatives(v) => Ok(v.clone()),
            Ingredient::Tag(t) => self
                .tags
                .get(t)
                .cloned()
                .ok_or_else(|| Failure::UnsupportedIngredient(format!("unknown tag {t}"))),
            Ingredient::Special(p) => Err(Failure::UnsupportedIngredient(p.clone())),
            Ingredient::Empty => Ok(vec![]),
        }
    }
    fn maximum_operations(&self, recipe: &Recipe, inventory: &BTreeMap<String, u32>) -> u32 {
        let mut copy = inventory.clone();
        let mut n = 0;
        loop {
            let mut missing = Vec::new();
            self.consume_operation(recipe, &mut copy, &mut missing);
            if !missing.is_empty() {
                break;
            }
            n += 1;
            if n == u32::MAX {
                break;
            }
        }
        n
    }
    fn consume_operation(
        &self,
        recipe: &Recipe,
        inventory: &mut BTreeMap<String, u32>,
        missing: &mut Vec<Missing>,
    ) {
        for slot in ingredients(&recipe.layout) {
            if matches!(slot.ingredient, Ingredient::Empty) {
                continue;
            }
            let Ok(mut choices) = self.choices(&slot.ingredient) else {
                missing.push(Missing {
                    ingredient: format!("{:?}", slot.ingredient),
                    count: slot.count,
                    alternatives: vec![],
                });
                continue;
            };
            choices.sort_by_key(|i| {
                (
                    std::cmp::Reverse(inventory.get(i).copied().unwrap_or(0)),
                    i.clone(),
                )
            });
            let mut needed = slot.count;
            for choice in &choices {
                let held = inventory.get(choice).copied().unwrap_or(0);
                let take = held.min(needed);
                if take > 0 {
                    inventory.insert(choice.clone(), held - take);
                    needed -= take;
                }
                if needed == 0 {
                    break;
                }
            }
            if needed > 0 {
                missing.push(Missing {
                    ingredient: choices.first().cloned().unwrap_or_default(),
                    count: needed,
                    alternatives: choices,
                });
            }
        }
    }
}

fn ingredients(layout: &RecipeLayout) -> &[IngredientSlot] {
    match layout {
        RecipeLayout::Shaped { ingredients, .. } | RecipeLayout::Shapeless { ingredients } => {
            ingredients
        }
    }
}
fn normalize(id: &str) -> String {
    if id.contains(':') {
        id.to_ascii_lowercase()
    } else {
        format!("minecraft:{}", id.to_ascii_lowercase())
    }
}
fn consolidate_missing(m: &mut Vec<Missing>) {
    let mut out: BTreeMap<(String, Vec<String>), u32> = BTreeMap::new();
    for x in m.drain(..) {
        *out.entry((x.ingredient, x.alternatives)).or_default() += x.count
    }
    *m = out
        .into_iter()
        .map(|((ingredient, alternatives), count)| Missing {
            ingredient,
            count,
            alternatives,
        })
        .collect();
}
fn validate_recipe(r: &Recipe) -> Result<(), String> {
    if r.output_count == 0 {
        return Err(format!("{} has zero output", r.id));
    }
    if let RecipeLayout::Shaped {
        width,
        height,
        ingredients,
    } = &r.layout
    {
        if *width == 0
            || *height == 0
            || *width > 3
            || *height > 3
            || ingredients.len() != usize::from(*width) * usize::from(*height)
        {
            return Err(format!("{} has invalid shaped grid", r.id));
        }
        if r.station == Station::Player && (*width > 2 || *height > 2) {
            return Err(format!("{} does not fit player grid", r.id));
        }
    }
    Ok(())
//! Transactional execution of an already-resolved crafting plan.
//!
//! Recipe discovery deliberately lives outside this module.  The executor only
//! consumes a concrete grid layout and delegates menu clicks/navigation to a
//! driver, making the confirmation rules usable with both Azalea and tests.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CraftingMenu {
    Player,
    Table,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IngredientPlacement {
    pub item: String,
    pub row: u8,
    pub column: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedCraftPlan {
    pub recipe_id: String,
    pub output_item: String,
    pub output_per_craft: u32,
    pub requested_output: u32,
    /// One entry per item consumed by one operation. Shapeless plans are put in
    /// deterministic row-major order by the resolver.
    pub placements: Vec<IngredientPlacement>,
    pub shaped: bool,
}

impl ResolvedCraftPlan {
    pub fn grid_size(&self) -> u8 {
        self.placements
            .iter()
            .map(|p| p.row.max(p.column) + 1)
            .max()
            .unwrap_or(0)
    }
    pub fn operations(&self) -> u32 {
        self.requested_output.div_ceil(self.output_per_craft.max(1))
    }
    pub fn requirements(&self, operations: u32) -> BTreeMap<String, u32> {
        let mut result = BTreeMap::new();
        for ingredient in &self.placements {
            *result.entry(ingredient.item.clone()).or_default() += operations;
        }
        result
    }
}

#[derive(Clone, Debug)]
pub struct CraftingOptions {
    pub allow_player_grid: bool,
    pub allow_table: bool,
    pub allow_table_navigation: bool,
    pub table_search_radius: u32,
    pub maximum_operations: u32,
    pub timeout: Duration,
    pub restore_grid: bool,
}
impl Default for CraftingOptions {
    fn default() -> Self {
        Self {
            allow_player_grid: true,
            allow_table: true,
            allow_table_navigation: true,
            table_search_radius: 32,
            maximum_operations: 64,
            timeout: Duration::from_secs(30),
            restore_grid: true,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MenuSnapshot {
    pub revision: u64,
    pub menu: Option<CraftingMenu>,
    pub inventory: BTreeMap<String, u32>,
    pub grid: Vec<Option<String>>,
    pub cursor: Option<(String, u32)>,
    pub output: Option<(String, u32)>,
    pub free_inventory_slots: u32,
    pub alive: bool,
    pub connected: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CraftStatus {
    Completed,
    Partial,
    MissingMaterials,
    NoSpace,
    WrongMenu,
    Rejected,
    TimedOut,
    Cancelled,
    Died,
    Disconnected,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CraftResult {
    pub status: CraftStatus,
    pub recipe_id: String,
    pub requested: u32,
    pub crafted: u32,
    pub completed_operations: u32,
    pub ingredients_consumed: BTreeMap<String, u32>,
    pub missing: BTreeMap<String, u32>,
    pub menu: Option<CraftingMenu>,
    pub initial_revision: u64,
    pub final_revision: u64,
    pub cursor: Option<(String, u32)>,
    pub grid: Vec<Option<String>>,
    pub recovered: bool,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverError {
    Rejected,
    TimedOut,
    Cancelled,
    Died,
    Disconnected,
    WrongMenu,
    NoSpace,
}

/// The sole inventory-operation owner while `execute` runs. Implementations
/// must return only after the server has acknowledged a newer menu revision.
pub trait CraftingDriver {
    fn snapshot(&mut self) -> MenuSnapshot;
    fn open_player_menu(&mut self) -> Result<MenuSnapshot, DriverError>;
    fn open_known_table(
        &mut self,
        radius: u32,
        navigate: bool,
    ) -> Result<MenuSnapshot, DriverError>;
    fn place_one(
        &mut self,
        grid_slot: usize,
        item: &str,
        after_revision: u64,
    ) -> Result<MenuSnapshot, DriverError>;
    fn take_output(&mut self, item: &str, after_revision: u64)
    -> Result<MenuSnapshot, DriverError>;
    fn recover(&mut self, after_revision: u64) -> Result<MenuSnapshot, DriverError>;
}

#[derive(Clone, Default)]
pub struct CraftCancellation(Arc<std::sync::atomic::AtomicBool>);
impl CraftCancellation {
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

pub fn execute<D: CraftingDriver>(
    driver: &mut D,
    plan: &ResolvedCraftPlan,
    options: &CraftingOptions,
    cancellation: &CraftCancellation,
) -> CraftResult {
    let deadline = Instant::now() + options.timeout;
    let start = driver.snapshot();
    let mut result = CraftResult {
        status: CraftStatus::Rejected,
        recipe_id: plan.recipe_id.clone(),
        requested: plan.requested_output,
        crafted: 0,
        completed_operations: 0,
        ingredients_consumed: BTreeMap::new(),
        missing: BTreeMap::new(),
        menu: None,
        initial_revision: start.revision,
        final_revision: start.revision,
        cursor: start.cursor.clone(),
        grid: start.grid.clone(),
        recovered: false,
        detail: None,
    };
    if plan.output_per_craft == 0
        || plan.requested_output == 0
        || plan.placements.is_empty()
        || plan.grid_size() > 3
    {
        result.detail = Some("invalid resolved plan".into());
        return result;
    }
    let operations = plan.operations();
    if operations > options.maximum_operations {
        result.detail = Some("operation bound exceeded".into());
        return result;
    }
    let required = plan.requirements(operations);
    for (item, count) in required {
        let available = start.inventory.get(&item).copied().unwrap_or(0);
        if available < count {
            result.missing.insert(item, count - available);
        }
    }
    if !result.missing.is_empty() {
        result.status = CraftStatus::MissingMaterials;
        return result;
    }
    if start.free_inventory_slots == 0 && !start.inventory.contains_key(&plan.output_item) {
        result.status = CraftStatus::NoSpace;
        return result;
    }
    let wanted_menu = if plan.grid_size() <= 2 && options.allow_player_grid {
        CraftingMenu::Player
    } else {
        CraftingMenu::Table
    };
    let opened = match wanted_menu {
        CraftingMenu::Player => driver.open_player_menu(),
        CraftingMenu::Table if options.allow_table => {
            driver.open_known_table(options.table_search_radius, options.allow_table_navigation)
        }
        CraftingMenu::Table => Err(DriverError::WrongMenu),
    };
    let mut state = match opened {
        Ok(s) if s.menu == Some(wanted_menu) && s.revision > start.revision => s,
        Ok(s) => {
            result.final_revision = s.revision;
            result.status = CraftStatus::WrongMenu;
            result.detail = Some("crafting menu was not authoritatively opened".into());
            return result;
        }
        Err(e) => {
            result.status = map_error(e, false);
            result.detail = Some("could not open crafting menu".into());
            return result;
        }
    };
    result.menu = state.menu;
    for _ in 0..operations {
        if Instant::now() >= deadline {
            result.status = finish_status(CraftStatus::TimedOut, result.completed_operations);
            break;
        }
        if cancellation.is_cancelled() {
            result.status = finish_status(CraftStatus::Cancelled, result.completed_operations);
            break;
        }
        for placement in &plan.placements {
            let width = if wanted_menu == CraftingMenu::Player {
                2
            } else {
                3
            };
            let slot = usize::from(placement.row * width + placement.column);
            match driver.place_one(slot, &placement.item, state.revision) {
                Ok(next) if next.revision > state.revision => {
                    state = next;
                    if cancellation.is_cancelled() {
                        result.status =
                            finish_status(CraftStatus::Cancelled, result.completed_operations);
                        return recover(driver, result, &state, options);
                    }
                }
                Ok(_) => {
                    result.status =
                        finish_status(CraftStatus::Rejected, result.completed_operations);
                    result.detail = Some("ingredient mutation had no revision".into());
                    return recover(driver, result, &state, options);
                }
                Err(e) => {
                    result.status = finish_status(map_error(e, false), result.completed_operations);
                    return recover(driver, result, &state, options);
                }
            }
        }
        let before_count = state.inventory.get(&plan.output_item).copied().unwrap_or(0);
        match driver.take_output(&plan.output_item, state.revision) {
            Ok(next)
                if next.revision > state.revision
                    && next.inventory.get(&plan.output_item).copied().unwrap_or(0)
                        >= before_count + plan.output_per_craft =>
            {
                state = next;
                result.completed_operations += 1;
                result.crafted += plan.output_per_craft;
                for p in &plan.placements {
                    *result
                        .ingredients_consumed
                        .entry(p.item.clone())
                        .or_default() += 1;
                }
            }
            Ok(next) => {
                let no_space = next.free_inventory_slots == 0;
                state = next;
                result.status = finish_status(
                    if no_space {
                        CraftStatus::NoSpace
                    } else {
                        CraftStatus::Rejected
                    },
                    result.completed_operations,
                );
                result.detail = Some("output inventory delta was not confirmed".into());
                return recover(driver, result, &state, options);
            }
            Err(e) => {
                result.status = finish_status(map_error(e, false), result.completed_operations);
                return recover(driver, result, &state, options);
            }
        }
    }
    if result.completed_operations == operations {
        result.status = CraftStatus::Completed;
    }
    recover(driver, result, &state, options)
}

fn map_error(error: DriverError, partial: bool) -> CraftStatus {
    if partial {
        return CraftStatus::Partial;
    }
    match error {
        DriverError::Rejected => CraftStatus::Rejected,
        DriverError::TimedOut => CraftStatus::TimedOut,
        DriverError::Cancelled => CraftStatus::Cancelled,
        DriverError::Died => CraftStatus::Died,
        DriverError::Disconnected => CraftStatus::Disconnected,
        DriverError::WrongMenu => CraftStatus::WrongMenu,
        DriverError::NoSpace => CraftStatus::NoSpace,
    }
}
fn finish_status(status: CraftStatus, done: u32) -> CraftStatus {
    if done > 0 {
        CraftStatus::Partial
    } else {
        status
    }
}
fn recover<D: CraftingDriver>(
    driver: &mut D,
    mut result: CraftResult,
    state: &MenuSnapshot,
    options: &CraftingOptions,
) -> CraftResult {
    let final_state = if options.restore_grid
        && (state.cursor.is_some() || state.grid.iter().any(Option::is_some))
    {
        match driver.recover(state.revision) {
            Ok(s) if s.revision > state.revision => {
                result.recovered = true;
                s
            }
            _ => state.clone(),
        }
    } else {
        state.clone()
    };
    result.final_revision = final_state.revision;
    result.cursor = final_state.cursor;
    result.grid = final_state.grid;
    result
}

#[derive(Clone, Debug, Default)]
pub struct CraftServiceStatus {
    pub active: bool,
    pub recipe_id: Option<String>,
    pub completed_operations: u32,
    pub crafted: u32,
    pub last_status: Option<CraftStatus>,
}
#[derive(Clone, Default)]
pub struct CraftService {
    status: Arc<Mutex<CraftServiceStatus>>,
    cancellation: CraftCancellation,
}
impl CraftService {
    pub fn status(&self) -> CraftServiceStatus {
        self.status.lock().expect("craft status poisoned").clone()
    }
    pub fn stop(&self) -> bool {
        let active = self.status().active;
        if active {
            self.cancellation.cancel();
        }
        active
    }
}

#[cfg(test)]
mod tests;
