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
