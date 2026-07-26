//! Bounded dropped-item collection policy.
//!
//! This module deliberately emits navigation directives instead of writing
//! controls. `App` remains the movement owner and applies them through the
//! existing `MovementService`.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant, SystemTime},
};

use crate::minecraft::world_state::{
    InventorySnapshot, PositionSnapshot, WorldConnectionState, WorldStateSnapshot,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemFilter {
    Exact(String),
    AnyOf(Vec<String>),
    Group(ItemGroup),
    Nearest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemGroup {
    Ores,
    Logs,
    Food,
}

impl ItemFilter {
    pub fn matches(&self, id: &str) -> bool {
        match self {
            Self::Exact(wanted) => wanted == id,
            Self::AnyOf(wanted) => wanted.iter().any(|item| item == id),
            Self::Nearest => true,
            Self::Group(group) => match group {
                ItemGroup::Ores => {
                    id.ends_with("_ore")
                        || matches!(
                            id,
                            "minecraft:coal"
                                | "minecraft:raw_iron"
                                | "minecraft:raw_gold"
                                | "minecraft:raw_copper"
                                | "minecraft:diamond"
                                | "minecraft:emerald"
                                | "minecraft:lapis_lazuli"
                                | "minecraft:redstone"
                                | "minecraft:quartz"
                        )
                }
                ItemGroup::Logs => id.ends_with("_log") || id.ends_with("_stem"),
                ItemGroup::Food => matches!(
                    id,
                    "minecraft:apple"
                        | "minecraft:bread"
                        | "minecraft:carrot"
                        | "minecraft:potato"
                        | "minecraft:cooked_beef"
                        | "minecraft:cooked_chicken"
                        | "minecraft:cooked_porkchop"
                        | "minecraft:golden_apple"
                ),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CollectRequest {
    pub filter: ItemFilter,
    pub entity_id: Option<u32>,
    pub minimum_stack: u32,
    pub quantity: u32,
    pub search_radius: f64,
    pub chase_distance: f64,
    pub maximum_entities: usize,
    pub timeout: Duration,
    pub allow_partial: bool,
}

impl CollectRequest {
    pub fn exact(item: String, quantity: u32) -> Self {
        Self {
            filter: ItemFilter::Exact(item),
            entity_id: None,
            minimum_stack: 1,
            quantity,
            search_radius: 32.0,
            chase_distance: 48.0,
            maximum_entities: 32,
            timeout: Duration::from_secs(90),
            allow_partial: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectOutcome {
    Completed,
    Partial,
    NoMatchingDrops,
    InventoryFull,
    AllUnsafe,
    AllUnreachable,
    ItemLost,
    PathFailed,
    TimedOut,
    Cancelled,
    Disconnected,
    Died,
    Exhausted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectResult {
    pub outcome: CollectOutcome,
    pub requested: u32,
    pub collected: u32,
    pub entities_targeted: u32,
    pub entities_collected: u32,
    pub entities_lost: u32,
    pub remaining: u32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[allow(dead_code)] // Safe/Unsafe are populated once terrain annotations are wired.
pub enum TargetSafety {
    Safe,
    Unknown,
    Unsafe,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CollectDirective {
    None,
    Navigate(PositionSnapshot),
    Stop,
    Finished(CollectResult),
}

#[derive(Clone, Debug)]
struct Target {
    id: u32,
    position: PositionSnapshot,
    item: String,
    inventory_before: u32,
    revision_before: u64,
    replans: u8,
    last_replan: Instant,
}

#[derive(Clone, Debug)]
pub struct CollectorConfig {
    pub pickup_range: f64,
    pub confirmation_timeout: Duration,
    pub destination_threshold: f64,
    pub minimum_replan_interval: Duration,
    pub maximum_replans: u8,
    pub unknown_terrain_allowed: bool,
}
impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            pickup_range: 1.75,
            confirmation_timeout: Duration::from_secs(3),
            destination_threshold: 0.75,
            minimum_replan_interval: Duration::from_millis(500),
            maximum_replans: 12,
            unknown_terrain_allowed: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DropCollector {
    config: CollectorConfig,
    request: Option<CollectRequest>,
    started: Option<Instant>,
    target: Option<Target>,
    failed: HashSet<u32>,
    result: CollectResult,
    cancelled: bool,
    waiting_since: Option<Instant>,
    last_outcome_hint: Option<CollectOutcome>,
}

impl DropCollector {
    pub fn new(config: CollectorConfig) -> Self {
        Self {
            config,
            request: None,
            started: None,
            target: None,
            failed: HashSet::new(),
            result: CollectResult {
                outcome: CollectOutcome::Exhausted,
                requested: 0,
                collected: 0,
                entities_targeted: 0,
                entities_collected: 0,
                entities_lost: 0,
                remaining: 0,
            },
            cancelled: false,
            waiting_since: None,
            last_outcome_hint: None,
        }
    }
    pub fn running(&self) -> bool {
        self.request.is_some()
    }
    pub fn start(&mut self, request: CollectRequest, now: Instant) {
        self.result = CollectResult {
            outcome: CollectOutcome::Exhausted,
            requested: request.quantity,
            collected: 0,
            entities_targeted: 0,
            entities_collected: 0,
            entities_lost: 0,
            remaining: request.quantity,
        };
        self.request = Some(request);
        self.started = Some(now);
        self.target = None;
        self.failed.clear();
        self.cancelled = false;
        self.waiting_since = None;
        self.last_outcome_hint = None;
    }
    pub fn cancel(&mut self) {
        if self.running() {
            self.cancelled = true;
        }
    }
    pub fn status(&self) -> (&CollectResult, Option<u32>) {
        (&self.result, self.target.as_ref().map(|t| t.id))
    }

    pub fn tick(
        &mut self,
        world: &WorldStateSnapshot,
        safety: &HashMap<u32, TargetSafety>,
        unreachable: &HashSet<u32>,
        path_failed: bool,
        now: Instant,
    ) -> CollectDirective {
        let Some(request) = self.request.clone() else {
            return CollectDirective::None;
        };
        if self.cancelled {
            return self.finish(CollectOutcome::Cancelled);
        }
        if world.connection.state != WorldConnectionState::Connected || !world.joined_world() {
            return self.finish(CollectOutcome::Disconnected);
        }
        if world.bot.alive == Some(false) {
            return self.finish(CollectOutcome::Died);
        }
        if now.duration_since(self.started.unwrap_or(now)) >= request.timeout {
            return self.finish(CollectOutcome::TimedOut);
        }
        if path_failed {
            if let Some(target) = self.target.take() {
                self.failed.insert(target.id);
                self.last_outcome_hint = Some(CollectOutcome::PathFailed);
            }
            self.waiting_since = None;
        }
        if let Some(mut target) = self.target.take() {
            let current_total = matching_count(&world.inventory, &request.filter);
            let revision_changed = world.inventory.revision != target.revision_before;
            let gained = current_total.saturating_sub(target.inventory_before);
            let entity = world
                .entities
                .iter()
                .find(|e| e.entity_id == target.id && e.item_id.as_deref() == Some(&target.item));
            if gained > 0 && revision_changed {
                let credited = gained.min(self.result.remaining);
                self.result.collected += credited;
                self.result.remaining -= credited;
                self.result.entities_collected += 1;
                self.waiting_since = None;
                if self.result.remaining == 0 {
                    return self.finish(CollectOutcome::Completed);
                }
                // If the entity survived (partial pickup), remember it as a fresh candidate.
                if entity.is_none() {
                    self.failed.insert(target.id);
                }
            } else if entity.is_none() {
                self.result.entities_lost += 1;
                self.failed.insert(target.id);
                self.last_outcome_hint = Some(CollectOutcome::ItemLost);
                self.waiting_since = None;
            } else {
                let entity = entity.unwrap();
                let target_distance =
                    distance(world.bot.position.unwrap_or_default(), entity.position);
                if target_distance <= self.config.pickup_range {
                    let since = *self.waiting_since.get_or_insert(now);
                    if now.duration_since(since) >= self.config.confirmation_timeout {
                        self.failed.insert(target.id);
                        self.last_outcome_hint = Some(CollectOutcome::ItemLost);
                        self.result.entities_lost += 1;
                        self.waiting_since = None;
                    } else {
                        self.target = Some(target);
                        return CollectDirective::Stop;
                    }
                } else {
                    self.waiting_since = None;
                    let moved = distance(target.position, entity.position)
                        >= self.config.destination_threshold;
                    if moved
                        && now.duration_since(target.last_replan)
                            >= self.config.minimum_replan_interval
                    {
                        if target.replans >= self.config.maximum_replans {
                            self.failed.insert(target.id);
                            self.last_outcome_hint = Some(CollectOutcome::AllUnreachable);
                        } else {
                            target.position = entity.position;
                            target.replans += 1;
                            target.last_replan = now;
                            let destination = target.position;
                            self.target = Some(target);
                            return CollectDirective::Navigate(destination);
                        }
                    } else {
                        self.target = Some(target);
                        return CollectDirective::None;
                    }
                }
            }
        }
        if inventory_capacity(&world.inventory, &request.filter) == 0 {
            return self.finish(CollectOutcome::InventoryFull);
        }
        if self.result.entities_targeted as usize >= request.maximum_entities {
            return self.finish(CollectOutcome::Exhausted);
        }
        let dimension = world.bot.dimension.as_deref();
        let mut saw_match = false;
        let mut saw_unsafe = false;
        let mut saw_unreachable = false;
        let mut candidates: Vec<_> = world
            .entities
            .iter()
            .filter_map(|entity| {
                let item = entity.item_id.as_ref()?;
                if entity.entity_type != "minecraft:item"
                    || !request.filter.matches(item)
                    || request.entity_id.is_some_and(|id| id != entity.entity_id)
                    || entity.item_count < request.minimum_stack
                    || entity.distance > request.search_radius
                    || entity.distance > request.chase_distance
                    || entity.dimension.as_deref() != dimension
                    || SystemTime::now()
                        .duration_since(entity.last_seen)
                        .unwrap_or_default()
                        > Duration::from_secs(5)
                    || self.failed.contains(&entity.entity_id)
                {
                    return None;
                }
                saw_match = true;
                if unreachable.contains(&entity.entity_id) {
                    saw_unreachable = true;
                    return None;
                }
                let level = safety
                    .get(&entity.entity_id)
                    .copied()
                    .unwrap_or(TargetSafety::Unknown);
                if level == TargetSafety::Unsafe
                    || (level == TargetSafety::Unknown && !self.config.unknown_terrain_allowed)
                {
                    saw_unsafe = true;
                    return None;
                }
                Some((
                    level,
                    entity.distance,
                    relevance(&request.filter, item),
                    std::cmp::Reverse(entity.item_count),
                    entity.last_seen,
                    entity.entity_id,
                    entity,
                ))
            })
            .take(request.maximum_entities)
            .collect();
        candidates.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.total_cmp(&b.1))
                .then(a.2.cmp(&b.2))
                .then(a.3.cmp(&b.3))
                .then(a.4.cmp(&b.4))
                .then(a.5.cmp(&b.5))
        });
        let Some((_, _, _, _, _, _, entity)) = candidates.first() else {
            let outcome = self
                .last_outcome_hint
                .or_else(|| saw_unsafe.then_some(CollectOutcome::AllUnsafe))
                .or_else(|| saw_unreachable.then_some(CollectOutcome::AllUnreachable))
                .unwrap_or(if saw_match {
                    CollectOutcome::Exhausted
                } else {
                    CollectOutcome::NoMatchingDrops
                });
            return self.finish(outcome);
        };
        let item = entity.item_id.clone().unwrap();
        self.result.entities_targeted += 1;
        self.target = Some(Target {
            id: entity.entity_id,
            position: entity.position,
            item,
            inventory_before: matching_count(&world.inventory, &request.filter),
            revision_before: world.inventory.revision,
            replans: 0,
            last_replan: now,
        });
        CollectDirective::Navigate(entity.position)
    }

    fn finish(&mut self, outcome: CollectOutcome) -> CollectDirective {
        let allow_partial = self.request.as_ref().is_some_and(|r| r.allow_partial);
        self.result.outcome =
            if self.result.collected > 0 && outcome != CollectOutcome::Completed && allow_partial {
                CollectOutcome::Partial
            } else {
                outcome
            };
        self.result.remaining = self.result.requested.saturating_sub(self.result.collected);
        self.request = None;
        self.target = None;
        self.waiting_since = None;
        CollectDirective::Finished(self.result.clone())
    }
}

fn distance(a: PositionSnapshot, b: PositionSnapshot) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt()
}
fn relevance(filter: &ItemFilter, id: &str) -> u8 {
    match filter {
        ItemFilter::Exact(x) if x == id => 0,
        ItemFilter::AnyOf(xs) => xs.iter().position(|x| x == id).unwrap_or(255) as u8,
        ItemFilter::Group(_) => 1,
        _ => 2,
    }
}
fn matching_count(inv: &InventorySnapshot, filter: &ItemFilter) -> u32 {
    inv.total_counts
        .iter()
        .filter(|(id, _)| filter.matches(id))
        .map(|(_, n)| *n)
        .sum()
}
pub fn inventory_capacity(inv: &InventorySnapshot, filter: &ItemFilter) -> u32 {
    if !inv.available {
        return 0;
    }
    inv.slots
        .iter()
        .map(|slot| match &slot.item_id {
            None => 64,
            Some(id) if filter.matches(id) => 64u32.saturating_sub(slot.count),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests;
