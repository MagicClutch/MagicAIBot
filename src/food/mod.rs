//! Conservative food gathering from observable, server-owned state.
//!
//! This deliberately supports only dropped edible item entities and mature
//! carrots, potatoes and beetroots. It does not infer growth, loot tables, or
//! unloaded state, and it never replants.

use std::time::{Duration, Instant, SystemTime};

use azalea::registry::builtin::ItemKind;
use azalea_inventory::{ItemStack, components::Food};
use tokio_util::sync::CancellationToken;

use crate::{
    blocks::{block_query::BlockSearchQuery, block_search::BlockSearchService},
    interaction::{InteractionController, interaction_controller::InteractionState},
    look::LookController,
    minecraft::{
        client::{ConnectionState, MinecraftClient},
        world_state::{BlockPosition, EntitySnapshot, InventorySnapshot, PositionSnapshot},
    },
    movement::{MovementService, NavigationMode},
};

#[derive(Clone, Debug, PartialEq)]
pub struct FoodInfo {
    pub item_id: String,
    pub nutrition: u32,
    pub saturation: f32,
    pub always_edible: bool,
}

/// Classifies food through Azalea's generated default data-component registry.
pub fn classify_food(item_id: &str) -> Option<FoodInfo> {
    let kind: ItemKind = item_id.parse().ok()?;
    let stack = ItemStack::new(kind, 1);
    let food = stack.get_component::<Food>()?;
    (food.nutrition > 0).then(|| FoodInfo {
        item_id: kind.to_string(),
        nutrition: food.nutrition as u32,
        saturation: food.saturation,
        always_edible: food.can_always_eat,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FoodGoal {
    Count { item: Option<String>, count: u32 },
    FoodValue(u32),
}

#[derive(Clone, Debug)]
pub struct CollectFoodRequest {
    pub goal: FoodGoal,
    pub radius: u32,
    pub timeout: Duration,
    pub allow_crops: bool,
}
impl Default for CollectFoodRequest {
    fn default() -> Self {
        Self {
            goal: FoodGoal::Count {
                item: None,
                count: 1,
            },
            radius: 32,
            timeout: Duration::from_secs(90),
            allow_crops: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CollectFoodOutcome {
    Completed,
    Unsupported,
    NoSource,
    NoSpace,
    PathFailure,
    OutOfRange,
    ChangedWorld,
    Timeout,
    Cancelled,
    Died,
    Disconnected,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CollectFoodResult {
    pub outcome: CollectFoodOutcome,
    pub requested: FoodGoal,
    pub collected_count: u32,
    pub collected_food_value: u32,
    pub sources_attempted: u32,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FoodSource {
    Dropped {
        entity_id: u32,
        position: PositionSnapshot,
        food: FoodInfo,
        count: u32,
    },
    MatureCrop {
        position: BlockPosition,
        block_id: String,
        food: FoodInfo,
    },
}
impl FoodSource {
    fn position(&self) -> PositionSnapshot {
        match self {
            Self::Dropped { position, .. } => *position,
            Self::MatureCrop { position, .. } => PositionSnapshot {
                x: f64::from(position.x) + 0.5,
                y: f64::from(position.y),
                z: f64::from(position.z) + 0.5,
            },
        }
    }
    fn food(&self) -> &FoodInfo {
        match self {
            Self::Dropped { food, .. } | Self::MatureCrop { food, .. } => food,
        }
    }
    fn stable_key(&self) -> (u8, i64, i64, i64) {
        match self {
            Self::Dropped { entity_id, .. } => (0, i64::from(*entity_id), 0, 0),
            Self::MatureCrop { position, .. } => (
                1,
                i64::from(position.x),
                i64::from(position.y),
                i64::from(position.z),
            ),
        }
    }
}

pub fn crop_maturity(
    block_id: &str,
    properties: &std::collections::HashMap<String, String>,
) -> Option<bool> {
    let maximum = match block_id {
        "minecraft:carrots" | "minecraft:potatoes" => 7,
        "minecraft:beetroots" => 3,
        _ => return None,
    };
    properties
        .get("age")?
        .parse::<u8>()
        .ok()
        .map(|age| age == maximum)
}

fn crop_food(block_id: &str) -> Option<FoodInfo> {
    classify_food(match block_id {
        "minecraft:carrots" => "minecraft:carrot",
        "minecraft:potatoes" => "minecraft:potato",
        "minecraft:beetroots" => "minecraft:beetroot",
        _ => return None,
    })
}
fn matches_goal(food: &FoodInfo, goal: &FoodGoal) -> bool {
    match goal {
        FoodGoal::Count {
            item: Some(item), ..
        } => food.item_id == *item,
        _ => true,
    }
}
fn distance2(a: PositionSnapshot, b: PositionSnapshot) -> f64 {
    (a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)
}

pub fn rank_sources(sources: &mut [FoodSource], origin: PositionSnapshot, goal: &FoodGoal) {
    sources.sort_by(|a, b| {
        // Drops are non-destructive and time-sensitive, then distance, then
        // exact relevance/value and a stable identity.
        let type_a = matches!(a, FoodSource::MatureCrop { .. });
        let type_b = matches!(b, FoodSource::MatureCrop { .. });
        type_a
            .cmp(&type_b)
            .then_with(|| {
                distance2(origin, a.position()).total_cmp(&distance2(origin, b.position()))
            })
            .then_with(|| matches_goal(b.food(), goal).cmp(&matches_goal(a.food(), goal)))
            .then_with(|| b.food().nutrition.cmp(&a.food().nutrition))
            .then_with(|| a.stable_key().cmp(&b.stable_key()))
    });
}

pub fn has_inventory_space(inventory: &InventorySnapshot, item_id: &str) -> bool {
    inventory.available
        && (inventory.slots.iter().any(|s| s.item_id.is_none())
            || inventory
                .slots
                .iter()
                .any(|s| s.item_id.as_deref() == Some(item_id) && s.count < 64))
}

#[derive(Clone, Debug)]
enum Phase {
    Navigating(FoodSource),
    Breaking(FoodSource),
    PickingUp(FoodSource),
}
#[derive(Clone, Debug)]
pub struct CollectFoodStatus {
    pub active: bool,
    pub phase: String,
    pub result: Option<CollectFoodResult>,
}

pub struct FoodCollector {
    request: Option<CollectFoodRequest>,
    phase: Option<Phase>,
    started: Option<Instant>,
    baseline: u32,
    baseline_food_count: u32,
    baseline_value: u32,
    attempted: u32,
    cancellation: CancellationToken,
    result: Option<CollectFoodResult>,
}
impl Default for FoodCollector {
    fn default() -> Self {
        Self {
            request: None,
            phase: None,
            started: None,
            baseline: 0,
            baseline_food_count: 0,
            baseline_value: 0,
            attempted: 0,
            cancellation: CancellationToken::new(),
            result: None,
        }
    }
}

impl FoodCollector {
    pub fn status(&self) -> CollectFoodStatus {
        CollectFoodStatus {
            active: self.request.is_some(),
            phase: match &self.phase {
                Some(Phase::Navigating(_)) => "navigating",
                Some(Phase::Breaking(_)) => "breaking",
                Some(Phase::PickingUp(_)) => "pickup",
                None => "idle",
            }
            .into(),
            result: self.result.clone(),
        }
    }
    pub async fn stop(
        &mut self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        interaction: &InteractionController,
        look: &LookController,
    ) {
        self.cancellation.cancel();
        interaction.cancel(minecraft, movement, look).await;
        let _ = movement.stop(minecraft).await;
        self.finish(CollectFoodOutcome::Cancelled, None);
    }
    fn progress(&self, inventory: &InventorySnapshot, request: &CollectFoodRequest) -> (u32, u32) {
        match &request.goal {
            FoodGoal::Count {
                item: Some(item), ..
            } => {
                let n = inventory.count_item(item).saturating_sub(self.baseline);
                (
                    n,
                    n.saturating_mul(classify_food(item).map_or(0, |f| f.nutrition)),
                )
            }
            FoodGoal::Count { item: None, .. } | FoodGoal::FoodValue(_) => {
                let count = inventory
                    .total_counts
                    .iter()
                    .filter(|(id, _)| classify_food(id).is_some())
                    .map(|(_, n)| n)
                    .sum::<u32>()
                    .saturating_sub(self.baseline_food_count);
                let value = inventory
                    .total_counts
                    .iter()
                    .filter_map(|(id, n)| classify_food(id).map(|f| n * f.nutrition))
                    .sum::<u32>()
                    .saturating_sub(self.baseline_value);
                (count, value)
            }
        }
    }
    fn satisfied(&self, p: (u32, u32), goal: &FoodGoal) -> bool {
        match goal {
            FoodGoal::Count {
                item: Some(_),
                count,
            } => p.0 >= *count,
            FoodGoal::Count { item: None, count } => p.0 >= *count,
            FoodGoal::FoodValue(v) => p.1 >= *v,
        }
    }
    fn finish(&mut self, outcome: CollectFoodOutcome, detail: Option<String>) {
        if let Some(request) = self.request.take() {
            self.result = Some(CollectFoodResult {
                outcome,
                requested: request.goal,
                collected_count: 0,
                collected_food_value: 0,
                sources_attempted: self.attempted,
                detail,
            });
        }
        self.phase = None;
    }

    pub async fn start(
        &mut self,
        request: CollectFoodRequest,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        search: &BlockSearchService,
    ) -> Result<(), CollectFoodResult> {
        if let FoodGoal::Count {
            item: Some(item), ..
        } = &request.goal
            && classify_food(item).is_none()
        {
            let r = CollectFoodResult {
                outcome: CollectFoodOutcome::Unsupported,
                requested: request.goal,
                collected_count: 0,
                collected_food_value: 0,
                sources_attempted: 0,
                detail: Some("requested item has no Azalea food component".into()),
            };
            self.result = Some(r.clone());
            return Err(r);
        }
        let world = minecraft.world_state_snapshot().await;
        let Some(origin) = world.bot.position else {
            let r = CollectFoodResult {
                outcome: CollectFoodOutcome::ChangedWorld,
                requested: request.goal,
                collected_count: 0,
                collected_food_value: 0,
                sources_attempted: 0,
                detail: Some("position unavailable".into()),
            };
            return Err(r);
        };
        self.baseline = match &request.goal {
            FoodGoal::Count { item: Some(i), .. } => world.inventory.count_item(i),
            _ => 0,
        };
        self.baseline_value = world
            .inventory
            .total_counts
            .iter()
            .filter_map(|(i, n)| classify_food(i).map(|f| n * f.nutrition))
            .sum();
        self.baseline_food_count = world
            .inventory
            .total_counts
            .iter()
            .filter(|(id, _)| classify_food(id).is_some())
            .map(|(_, count)| count)
            .sum();
        let mut sources: Vec<_> = world
            .entities
            .iter()
            .filter_map(|e| dropped_source(e, &request, origin))
            .collect();
        if request.allow_crops {
            for block_id in [
                "minecraft:carrots",
                "minecraft:potatoes",
                "minecraft:beetroots",
            ] {
                if let Ok(found) = search
                    .search_raw(
                        minecraft,
                        BlockSearchQuery {
                            block_id: block_id.into(),
                            radius: request.radius,
                            maximum_results: 64,
                            vertical_range: request.radius,
                        },
                    )
                    .await
                {
                    for candidate in found {
                        if let Ok(Some((id, props))) =
                            minecraft.block_state_at(candidate.position).await
                            && crop_maturity(&id, &props) == Some(true)
                        {
                            if let Some(food) = crop_food(&id)
                                && matches_goal(&food, &request.goal)
                            {
                                sources.push(FoodSource::MatureCrop {
                                    position: candidate.position,
                                    block_id: id,
                                    food,
                                });
                            }
                        }
                    }
                }
            }
        }
        rank_sources(&mut sources, origin, &request.goal);
        let Some(source) = sources
            .into_iter()
            .find(|s| has_inventory_space(&world.inventory, &s.food().item_id))
        else {
            let outcome = if world.inventory.available
                && world.inventory.slots.iter().all(|s| s.item_id.is_some())
            {
                CollectFoodOutcome::NoSpace
            } else {
                CollectFoodOutcome::NoSource
            };
            let r = CollectFoodResult {
                outcome,
                requested: request.goal,
                collected_count: 0,
                collected_food_value: 0,
                sources_attempted: 0,
                detail: None,
            };
            self.result = Some(r.clone());
            return Err(r);
        };
        self.request = Some(request);
        self.started = Some(Instant::now());
        self.attempted = 1;
        self.cancellation = CancellationToken::new();
        self.result = None;
        movement
            .goto(minecraft, source.position(), NavigationMode::MovementOnly)
            .await
            .map_err(|e| {
                let r = CollectFoodResult {
                    outcome: CollectFoodOutcome::PathFailure,
                    requested: self.request.as_ref().unwrap().goal.clone(),
                    collected_count: 0,
                    collected_food_value: 0,
                    sources_attempted: 1,
                    detail: Some(e.to_string()),
                };
                self.request = None;
                r
            })?;
        self.phase = Some(Phase::Navigating(source));
        Ok(())
    }

    pub async fn tick(
        &mut self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        interaction: &InteractionController,
        look: &LookController,
    ) {
        let Some(request) = self.request.clone() else {
            return;
        };
        let world = minecraft.world_state_snapshot().await;
        let progress = self.progress(&world.inventory, &request);
        if self.satisfied(progress, &request.goal) {
            self.finish_with_progress(CollectFoodOutcome::Completed, None, progress);
            return;
        }
        if self.cancellation.is_cancelled() {
            self.finish_with_progress(CollectFoodOutcome::Cancelled, None, progress);
            return;
        }
        if minecraft.connection_state() != ConnectionState::Connected {
            self.finish_with_progress(CollectFoodOutcome::Disconnected, None, progress);
            return;
        }
        if world.bot.alive == Some(false) {
            self.finish_with_progress(CollectFoodOutcome::Died, None, progress);
            return;
        }
        if self.started.is_some_and(|s| s.elapsed() >= request.timeout) {
            self.finish_with_progress(CollectFoodOutcome::Timeout, None, progress);
            return;
        }
        match self.phase.clone() {
            Some(Phase::Navigating(source)) => {
                let nav = minecraft.navigation_status().await.ok();
                if nav.is_some_and(|n| n.reached) {
                    match source {
                        FoodSource::Dropped { .. } => self.phase = Some(Phase::PickingUp(source)),
                        FoodSource::MatureCrop {
                            position,
                            ref block_id,
                            ..
                        } => match minecraft.block_state_at(position).await {
                            Ok(Some((id, p)))
                                if id == *block_id && crop_maturity(&id, &p) == Some(true) =>
                            {
                                if interaction
                                    .break_at(minecraft, movement, look, position)
                                    .await
                                    .is_ok()
                                {
                                    self.phase = Some(Phase::Breaking(source))
                                } else {
                                    self.finish_with_progress(
                                        CollectFoodOutcome::OutOfRange,
                                        None,
                                        progress,
                                    )
                                }
                            }
                            _ => self.finish_with_progress(
                                CollectFoodOutcome::ChangedWorld,
                                None,
                                progress,
                            ),
                        },
                    }
                }
            }
            Some(Phase::Breaking(source)) => {
                let s = interaction.snapshot().await;
                if s.state == InteractionState::Completed {
                    let _ = movement
                        .goto(minecraft, source.position(), NavigationMode::MovementOnly)
                        .await;
                    self.phase = Some(Phase::PickingUp(source))
                } else if s.state == InteractionState::Failed {
                    self.finish_with_progress(
                        CollectFoodOutcome::OutOfRange,
                        s.failure_reason,
                        progress,
                    )
                }
            }
            Some(Phase::PickingUp(source)) => {
                let exists = match source {
                    FoodSource::Dropped { entity_id, .. } => {
                        world.entities.iter().any(|e| e.entity_id == entity_id)
                    }
                    FoodSource::MatureCrop { .. } => false,
                };
                if !exists
                    && progress == (0, 0)
                    && self
                        .started
                        .is_some_and(|s| s.elapsed() > Duration::from_secs(5))
                {
                    self.finish_with_progress(
                        CollectFoodOutcome::ChangedWorld,
                        Some("source vanished without confirmed inventory progress".into()),
                        progress,
                    )
                }
            }
            None => {}
        }
    }
    fn finish_with_progress(
        &mut self,
        outcome: CollectFoodOutcome,
        detail: Option<String>,
        progress: (u32, u32),
    ) {
        self.finish(outcome, detail);
        if let Some(r) = &mut self.result {
            r.collected_count = progress.0;
            r.collected_food_value = progress.1
        }
    }
}

fn dropped_source(
    e: &EntitySnapshot,
    request: &CollectFoodRequest,
    origin: PositionSnapshot,
) -> Option<FoodSource> {
    let item = e.item.as_ref()?;
    let food = classify_food(&item.item_id)?;
    (matches_goal(&food, &request.goal)
        && distance2(origin, e.position) <= f64::from(request.radius).powi(2)
        && SystemTime::now()
            .duration_since(e.last_seen)
            .unwrap_or_default()
            < Duration::from_secs(5))
    .then(|| FoodSource::Dropped {
        entity_id: e.entity_id,
        position: e.position,
        food,
        count: item.count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    #[test]
    fn registry_food_classification() {
        let carrot = classify_food("minecraft:carrot").unwrap();
        assert_eq!(carrot.nutrition, 3);
        assert!(classify_food("minecraft:diamond").is_none())
    }
    #[test]
    fn maturity_is_exact_and_unknown_is_ambiguous() {
        let mut p = HashMap::from([("age".into(), "6".into())]);
        assert_eq!(crop_maturity("minecraft:carrots", &p), Some(false));
        p.insert("age".into(), "7".into());
        assert_eq!(crop_maturity("minecraft:carrots", &p), Some(true));
        assert_eq!(crop_maturity("minecraft:wheat", &p), None);
        p.clear();
        assert_eq!(crop_maturity("minecraft:carrots", &p), None)
    }
    #[test]
    fn drops_rank_before_crops_then_nearest() {
        let food = classify_food("minecraft:carrot").unwrap();
        let mut s = vec![
            FoodSource::MatureCrop {
                position: BlockPosition { x: 1, y: 0, z: 0 },
                block_id: "minecraft:carrots".into(),
                food: food.clone(),
            },
            FoodSource::Dropped {
                entity_id: 9,
                position: PositionSnapshot {
                    x: 5.,
                    y: 0.,
                    z: 0.,
                },
                food: food.clone(),
                count: 1,
            },
            FoodSource::Dropped {
                entity_id: 2,
                position: PositionSnapshot {
                    x: 2.,
                    y: 0.,
                    z: 0.,
                },
                food,
                count: 1,
            },
        ];
        rank_sources(
            &mut s,
            PositionSnapshot::default(),
            &FoodGoal::Count {
                item: None,
                count: 1,
            },
        );
        assert!(matches!(s[0], FoodSource::Dropped { entity_id: 2, .. }));
        assert!(matches!(s[2], FoodSource::MatureCrop { .. }))
    }
    #[test]
    fn inventory_capacity_accepts_partial_or_empty() {
        let mut i = InventorySnapshot {
            available: true,
            slots: vec![crate::minecraft::world_state::InventorySlot {
                slot: 0,
                item_id: Some("minecraft:carrot".into()),
                display_name: None,
                count: 63,
            }],
            ..Default::default()
        };
        assert!(has_inventory_space(&i, "minecraft:carrot"));
        i.slots[0].count = 64;
        assert!(!has_inventory_space(&i, "minecraft:carrot"));
        i.slots.push(crate::minecraft::world_state::InventorySlot {
            slot: 1,
            item_id: None,
            display_name: None,
            count: 0,
        });
        assert!(has_inventory_space(&i, "minecraft:carrot"))
    }
}
