//! Cancellation-aware, event-driven furnace execution.
//!
//! This module deliberately owns neither recipe/container mechanics nor Azalea
//! clicks. `SmeltingKnowledge` produces an immutable plan and `FurnacePort` is
//! the serialized container boundary. The executor only coordinates confirmed
//! observations and actions.

use crate::minecraft::world_state::BlockPosition;
use serde::Deserialize;
use std::{
    future::Future,
    pin::Pin,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Deserialize)]
pub struct SmeltingConfig {
    #[serde(default = "radius")]
    pub station_search_radius: u32,
    #[serde(default = "poll")]
    pub observation_interval_ms: u64,
    #[serde(default = "confirm")]
    pub confirmation_timeout_ms: u64,
    #[serde(default = "total")]
    pub total_timeout_seconds: u64,
    #[serde(default = "retries")]
    pub reopen_limit: u8,
}
fn radius() -> u32 {
    32
}
fn poll() -> u64 {
    250
}
fn confirm() -> u64 {
    3_000
}
fn total() -> u64 {
    300
}
fn retries() -> u8 {
    2
}
impl Default for SmeltingConfig {
    fn default() -> Self {
        Self {
            station_search_radius: radius(),
            observation_interval_ms: poll(),
            confirmation_timeout_ms: confirm(),
            total_timeout_seconds: total(),
            reopen_limit: retries(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationKind {
    Furnace,
    BlastFurnace,
    Smoker,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stack {
    pub item: String,
    pub count: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SmeltingPlan {
    pub recipe_id: String,
    pub input: String,
    pub output: String,
    pub operations: u32,
    pub output_per_operation: u32,
    pub cook_ticks: u32,
    pub compatible: Vec<StationKind>,
    pub fuel: Stack,
    pub fuel_burn_ticks: u32,
}
#[derive(Clone, Debug)]
pub struct PlanRequest {
    pub target: String,
    pub count: u32,
    pub recipe_id: Option<String>,
    pub preferred: Option<StationKind>,
    pub alternatives: bool,
    pub allow_partial: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanError {
    UnknownRecipe,
    MissingInput { needed: u32, available: u32 },
    MissingFuel { burn_ticks: u32 },
    ProtectedFuel,
}
pub trait SmeltingKnowledge: Send + Sync {
    fn resolve(
        &self,
        request: &PlanRequest,
        inventory: &InventoryView,
    ) -> Result<SmeltingPlan, PlanError>;
}

#[derive(Clone, Debug, Default)]
pub struct InventoryView {
    pub revision: u64,
    pub stacks: Vec<Stack>,
    pub free_capacity: u32,
    pub protected: Vec<String>,
}
impl InventoryView {
    pub fn count(&self, id: &str) -> u32 {
        self.stacks
            .iter()
            .filter(|s| s.item == id)
            .map(|s| s.count)
            .sum()
    }
}
#[derive(Clone, Debug)]
pub struct StationCandidate {
    pub position: BlockPosition,
    pub kind: StationKind,
    pub distance: f64,
    pub reachable: bool,
    pub known_compatible: bool,
}
pub fn rank_stations(
    mut stations: Vec<StationCandidate>,
    plan: &SmeltingPlan,
) -> Vec<StationCandidate> {
    stations.retain(|s| plan.compatible.contains(&s.kind));
    stations.sort_by(|a, b| {
        b.known_compatible
            .cmp(&a.known_compatible)
            .then_with(|| b.reachable.cmp(&a.reachable))
            .then_with(|| a.distance.total_cmp(&b.distance))
            .then_with(|| {
                (a.position.x, a.position.y, a.position.z).cmp(&(
                    b.position.x,
                    b.position.y,
                    b.position.z,
                ))
            })
    });
    stations
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FurnaceContents {
    pub input: Option<Stack>,
    pub fuel: Option<Stack>,
    pub output: Option<Stack>,
    pub progress_ticks: u32,
}
#[derive(Clone, Debug)]
pub struct Observation {
    pub connected: bool,
    pub alive: bool,
    pub station_loaded: bool,
    pub menu: Option<StationKind>,
    pub revision: u64,
    pub inventory: InventoryView,
    pub contents: FurnaceContents,
}
#[derive(Clone, Debug)]
pub enum Action {
    Navigate(BlockPosition),
    Open(BlockPosition),
    InsertInput(Stack, u64),
    InsertFuel(Stack, u64),
    CollectOutput(u64),
    Close,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Failure {
    MissingInput,
    MissingFuel,
    NoCompatibleStation,
    StationUnreachable,
    StationOccupied,
    IncompatibleContents,
    InsufficientInventorySpace,
    StationUnavailable,
    WrongMenu,
    ChangedContents,
    ProcessingInterrupted,
    Rejected,
    TimedOut,
    Cancelled,
    Disconnected,
    Died,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    Completed,
    Partial,
    Failed(Failure),
}
#[derive(Clone, Debug)]
pub struct SmeltResult {
    pub outcome: Outcome,
    pub output_collected: u32,
    pub input_consumed: u32,
    pub fuel_consumed: u32,
    pub station: Option<BlockPosition>,
    pub elapsed: Duration,
    pub last_phase: Phase,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Resolving,
    Selecting,
    Navigating,
    Opening,
    Inspecting,
    LoadingInput,
    LoadingFuel,
    Processing,
    Collecting,
    Confirming,
    Finished,
}
#[derive(Clone, Debug)]
pub struct ExecutionOptions {
    pub allow_existing_input: bool,
    pub use_existing_fuel: bool,
    pub collect_on_cancel: bool,
    pub close_on_cancel: bool,
    pub allow_partial: bool,
    pub wait_for_all: bool,
    pub timeout: Duration,
    pub observation_interval: Duration,
    pub confirmation_timeout: Duration,
    pub reopen_limit: u8,
}
impl From<&SmeltingConfig> for ExecutionOptions {
    fn from(c: &SmeltingConfig) -> Self {
        Self {
            allow_existing_input: true,
            use_existing_fuel: true,
            collect_on_cancel: true,
            close_on_cancel: true,
            allow_partial: true,
            wait_for_all: true,
            timeout: Duration::from_secs(c.total_timeout_seconds),
            observation_interval: Duration::from_millis(c.observation_interval_ms.max(50)),
            confirmation_timeout: Duration::from_millis(c.confirmation_timeout_ms),
            reopen_limit: c.reopen_limit,
        }
    }
}

type PortFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, Failure>> + Send + 'a>>;
pub trait FurnacePort: Send {
    fn candidates<'a>(
        &'a mut self,
        plan: &'a SmeltingPlan,
    ) -> PortFuture<'a, Vec<StationCandidate>>;
    fn observe<'a>(&'a mut self) -> PortFuture<'a, Observation>;
    fn act<'a>(&'a mut self, action: Action) -> PortFuture<'a, ()>;
}

pub struct FurnaceExecutor {
    pub phase: Phase,
}
impl Default for FurnaceExecutor {
    fn default() -> Self {
        Self {
            phase: Phase::Resolving,
        }
    }
}
impl FurnaceExecutor {
    pub async fn execute(
        &mut self,
        plan: SmeltingPlan,
        opts: ExecutionOptions,
        cancel: CancellationToken,
        port: &mut dyn FurnacePort,
    ) -> SmeltResult {
        let started = Instant::now();
        let mut station = None;
        let mut initial_input: u32 = 0;
        let mut initial_fuel: u32 = 0;
        let mut initial_player_output: u32 = 0;
        let mut initialized = false;
        let mut collected_from_slot = false;
        let mut last_output: u32 = 0;
        let mut reopen = 0;
        let mut last_input: u32 = 0;
        let mut last_fuel: u32 = 0;
        macro_rules! finish {
            ($o:expr) => {{
                self.phase = Phase::Finished;
                return SmeltResult {
                    outcome: $o,
                    output_collected: last_output,
                    input_consumed: initial_input.saturating_sub(last_input),
                    fuel_consumed: initial_fuel.saturating_sub(last_fuel),
                    station,
                    elapsed: started.elapsed(),
                    last_phase: Phase::Finished,
                };
            }};
        }
        self.phase = Phase::Selecting;
        let candidates = match port.candidates(&plan).await {
            Ok(v) => rank_stations(v, &plan),
            Err(e) => finish!(Outcome::Failed(e)),
        };
        let Some(chosen) = candidates.into_iter().next() else {
            finish!(Outcome::Failed(Failure::NoCompatibleStation))
        };
        if !chosen.reachable {
            finish!(Outcome::Failed(Failure::StationUnreachable))
        }
        station = Some(chosen.position);
        self.phase = Phase::Navigating;
        if let Err(e) = port.act(Action::Navigate(chosen.position)).await {
            finish!(Outcome::Failed(e))
        }
        self.phase = Phase::Opening;
        if let Err(e) = port.act(Action::Open(chosen.position)).await {
            finish!(Outcome::Failed(e))
        }
        let deadline = started + opts.timeout;
        loop {
            if Instant::now() >= deadline {
                finish!(if last_output > 0 {
                    Outcome::Partial
                } else {
                    Outcome::Failed(Failure::TimedOut)
                })
            }
            if cancel.is_cancelled() {
                if opts.collect_on_cancel {
                    let _ = port.act(Action::CollectOutput(0)).await;
                }
                if opts.close_on_cancel {
                    let _ = port.act(Action::Close).await;
                }
                finish!(if last_output > 0 {
                    Outcome::Partial
                } else {
                    Outcome::Failed(Failure::Cancelled)
                })
            }
            let obs = match port.observe().await {
                Ok(v) => v,
                Err(e) => finish!(if last_output > 0 {
                    Outcome::Partial
                } else {
                    Outcome::Failed(e)
                }),
            };
            if !obs.connected {
                finish!(Outcome::Failed(Failure::Disconnected))
            }
            if !obs.alive {
                finish!(Outcome::Failed(Failure::Died))
            }
            if !obs.station_loaded {
                finish!(Outcome::Failed(Failure::StationUnavailable))
            }
            if obs.menu != Some(chosen.kind) {
                if reopen >= opts.reopen_limit {
                    finish!(Outcome::Failed(Failure::WrongMenu))
                }
                reopen += 1;
                if port.act(Action::Open(chosen.position)).await.is_err() {
                    finish!(Outcome::Failed(Failure::WrongMenu))
                };
                sleep_or_cancel(opts.observation_interval, &cancel).await;
                continue;
            }
            let c = &obs.contents;
            last_input = c.input.as_ref().map_or(0, |s| s.count);
            last_fuel = c.fuel.as_ref().map_or(0, |s| s.count);
            if !initialized {
                initialized = true;
                initial_player_output = obs.inventory.count(&plan.output);
                initial_input = last_input;
                initial_fuel = last_fuel;
            }
            if c.input.as_ref().is_some_and(|s| s.item != plan.input)
                || c.output.as_ref().is_some_and(|s| s.item != plan.output)
            {
                finish!(Outcome::Failed(Failure::IncompatibleContents))
            }
            if c.fuel.as_ref().is_some_and(|s| s.item != plan.fuel.item) && !opts.use_existing_fuel
            {
                finish!(Outcome::Failed(Failure::IncompatibleContents))
            }
            if collected_from_slot {
                last_output = obs
                    .inventory
                    .count(&plan.output)
                    .saturating_sub(initial_player_output);
                if c.output.is_none() && last_output >= plan.operations * plan.output_per_operation
                {
                    let _ = port.act(Action::Close).await;
                    finish!(Outcome::Completed)
                }
            }
            if last_input < plan.operations {
                self.phase = Phase::LoadingInput;
                let n = plan.operations - last_input;
                if obs.inventory.count(&plan.input) < n {
                    finish!(Outcome::Failed(Failure::MissingInput))
                }
                if port
                    .act(Action::InsertInput(
                        Stack {
                            item: plan.input.clone(),
                            count: n,
                        },
                        obs.revision,
                    ))
                    .await
                    .is_err()
                {
                    finish!(Outcome::Failed(Failure::Rejected))
                };
                initial_input = plan.operations;
                sleep_or_cancel(opts.observation_interval, &cancel).await;
                continue;
            }
            if last_fuel < plan.fuel.count {
                self.phase = Phase::LoadingFuel;
                let n = plan.fuel.count - last_fuel;
                if obs.inventory.count(&plan.fuel.item) < n {
                    finish!(Outcome::Failed(Failure::MissingFuel))
                }
                if obs.inventory.protected.contains(&plan.fuel.item) {
                    finish!(Outcome::Failed(Failure::MissingFuel))
                }
                if port
                    .act(Action::InsertFuel(
                        Stack {
                            item: plan.fuel.item.clone(),
                            count: n,
                        },
                        obs.revision,
                    ))
                    .await
                    .is_err()
                {
                    finish!(Outcome::Failed(Failure::Rejected))
                };
                initial_fuel = plan.fuel.count;
                sleep_or_cancel(opts.observation_interval, &cancel).await;
                continue;
            }
            let available = c.output.as_ref().map_or(0, |s| s.count);
            let player_gain = obs
                .inventory
                .count(&plan.output)
                .saturating_sub(initial_player_output);
            last_output = player_gain;
            if available > 0 {
                if obs.inventory.free_capacity < available {
                    finish!(if last_output > 0 {
                        Outcome::Partial
                    } else {
                        Outcome::Failed(Failure::InsufficientInventorySpace)
                    })
                }
                self.phase = Phase::Collecting;
                if port.act(Action::CollectOutput(obs.revision)).await.is_err() {
                    finish!(Outcome::Failed(Failure::Rejected))
                };
                collected_from_slot = true;
                self.phase = Phase::Confirming;
                sleep_or_cancel(opts.observation_interval, &cancel).await;
                continue;
            }
            if collected_from_slot && player_gain >= plan.operations * plan.output_per_operation {
                let _ = port.act(Action::Close).await;
                finish!(Outcome::Completed)
            }
            self.phase = Phase::Processing;
            sleep_or_cancel(opts.observation_interval, &cancel).await;
        }
    }
}
async fn sleep_or_cancel(duration: Duration, cancel: &CancellationToken) {
    tokio::select! {_=tokio::time::sleep(duration)=>{},_=cancel.cancelled()=>{}}
}

#[cfg(test)]
mod tests;
