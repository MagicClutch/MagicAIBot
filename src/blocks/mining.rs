//! Conservative, loaded-world-only ore planning primitives.
//!
//! This module deliberately does not discover terrain. `None` means unknown or
//! unloaded, never air. Callers must re-read a target immediately before an
//! intentional break; the pure functions here also make that contract easy to
//! exercise with mocked worlds.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::minecraft::world_state::BlockPosition;

pub const MAX_VEIN_BLOCKS: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OreSelector {
    Id(String),
    Group(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SafetyVerdict {
    Safe,
    AdjacentLava,
    FallingBlock,
    VoidExposure,
    StraightDown,
    Overhead,
    LostSupport,
    SafetyUnknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum PickaxeTier {
    Wooden,
    Stone,
    Iron,
    Diamond,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OreDefinition {
    pub blocks: Vec<&'static str>,
    pub drop: &'static str,
    pub minimum_tier: PickaxeTier,
}

/// Resolve the intentionally small, explicit ore vocabulary. Unknown names are
/// rejected rather than interpreted as a request to scan arbitrary blocks.
pub fn resolve(selector: &OreSelector) -> Option<OreDefinition> {
    let value = match selector {
        OreSelector::Id(v) | OreSelector::Group(v) => v.as_str(),
    };
    let value = value.strip_prefix("minecraft:").unwrap_or(value);
    let value = value.strip_prefix("deepslate_").unwrap_or(value);
    let (blocks, drop, minimum_tier): (Vec<&str>, &str, PickaxeTier) = match value {
        "coal" | "coal_ore" => (
            vec!["minecraft:coal_ore", "minecraft:deepslate_coal_ore"],
            "minecraft:coal",
            PickaxeTier::Wooden,
        ),
        "iron" | "iron_ore" => (
            vec!["minecraft:iron_ore", "minecraft:deepslate_iron_ore"],
            "minecraft:raw_iron",
            PickaxeTier::Stone,
        ),
        "copper" | "copper_ore" => (
            vec!["minecraft:copper_ore", "minecraft:deepslate_copper_ore"],
            "minecraft:raw_copper",
            PickaxeTier::Stone,
        ),
        "gold" | "gold_ore" => (
            vec!["minecraft:gold_ore", "minecraft:deepslate_gold_ore"],
            "minecraft:raw_gold",
            PickaxeTier::Iron,
        ),
        "nether_gold" | "nether_gold_ore" => (
            vec!["minecraft:nether_gold_ore"],
            "minecraft:gold_nugget",
            PickaxeTier::Wooden,
        ),
        "redstone" | "redstone_ore" => (
            vec!["minecraft:redstone_ore", "minecraft:deepslate_redstone_ore"],
            "minecraft:redstone",
            PickaxeTier::Iron,
        ),
        "lapis" | "lapis_ore" => (
            vec!["minecraft:lapis_ore", "minecraft:deepslate_lapis_ore"],
            "minecraft:lapis_lazuli",
            PickaxeTier::Stone,
        ),
        "diamond" | "diamond_ore" => (
            vec!["minecraft:diamond_ore", "minecraft:deepslate_diamond_ore"],
            "minecraft:diamond",
            PickaxeTier::Iron,
        ),
        "emerald" | "emerald_ore" => (
            vec!["minecraft:emerald_ore", "minecraft:deepslate_emerald_ore"],
            "minecraft:emerald",
            PickaxeTier::Iron,
        ),
        "quartz" | "nether_quartz_ore" => (
            vec!["minecraft:nether_quartz_ore"],
            "minecraft:quartz",
            PickaxeTier::Wooden,
        ),
        _ => return None,
    };
    Some(OreDefinition {
        blocks,
        drop,
        minimum_tier,
    })
}

pub fn pickaxe_tier(item: &str) -> Option<PickaxeTier> {
    if !item.ends_with("_pickaxe") {
        return None;
    }
    if item.contains("netherite_") || item.contains("diamond_") {
        Some(PickaxeTier::Diamond)
    } else if item.contains("iron_") {
        Some(PickaxeTier::Iron)
    } else if item.contains("stone_") {
        Some(PickaxeTier::Stone)
    } else if item.contains("wooden_") || item.contains("golden_") {
        Some(PickaxeTier::Wooden)
    } else {
        None
    }
}

pub fn has_harvest_tool<'a>(
    items: impl IntoIterator<Item = &'a str>,
    minimum: PickaxeTier,
) -> bool {
    items
        .into_iter()
        .filter_map(pickaxe_tier)
        .any(|tier| tier >= minimum)
}

pub fn neighbors(position: BlockPosition) -> [BlockPosition; 6] {
    let BlockPosition { x, y, z } = position;
    [
        BlockPosition { x: x + 1, y, z },
        BlockPosition { x: x - 1, y, z },
        BlockPosition { x, y: y + 1, z },
        BlockPosition { x, y: y - 1, z },
        BlockPosition { x, y, z: z + 1 },
        BlockPosition { x, y, z: z - 1 },
    ]
}

fn is_air(id: &str) -> bool {
    matches!(
        id,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}
fn is_lava(id: &str) -> bool {
    id == "minecraft:lava"
}
fn is_falling(id: &str) -> bool {
    matches!(
        id,
        "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel" | "minecraft:concrete_powder"
    ) || id.ends_with("_concrete_powder")
}

/// Evaluate only facts present in `known`. All six neighbors, two standing
/// cells, and floor support must be known; otherwise the correct answer is
/// `SafetyUnknown` (including a loaded-chunk boundary).
pub fn safety(
    known: &HashMap<BlockPosition, String>,
    target: BlockPosition,
    bot: BlockPosition,
) -> SafetyVerdict {
    if target.x == bot.x && target.z == bot.z && target.y < bot.y {
        return SafetyVerdict::StraightDown;
    }
    if target.x == bot.x && target.z == bot.z && target.y > bot.y {
        return SafetyVerdict::Overhead;
    }
    let adjacent = neighbors(target);
    if adjacent
        .iter()
        .any(|p| known.get(p).is_some_and(|id| is_lava(id)))
    {
        return SafetyVerdict::AdjacentLava;
    }
    if adjacent.iter().any(|p| !known.contains_key(p)) {
        return SafetyVerdict::SafetyUnknown;
    }
    let above = BlockPosition {
        y: target.y + 1,
        ..target
    };
    if known.get(&above).is_some_and(|id| is_falling(id)) {
        return SafetyVerdict::FallingBlock;
    }
    let feet = bot;
    let head = BlockPosition {
        y: bot.y + 1,
        ..bot
    };
    let floor = BlockPosition {
        y: bot.y - 1,
        ..bot
    };
    if [feet, head, floor].iter().any(|p| !known.contains_key(p)) {
        return SafetyVerdict::SafetyUnknown;
    }
    if target == floor {
        return SafetyVerdict::LostSupport;
    }
    if known.get(&floor).is_some_and(|id| is_air(id)) {
        return SafetyVerdict::VoidExposure;
    }
    SafetyVerdict::Safe
}

/// Six-connected, deterministically ordered and hard-bounded vein discovery.
/// Unknown neighbors are boundaries, never inferred blocks.
pub fn connected_vein(
    known: &HashMap<BlockPosition, String>,
    seed: BlockPosition,
    ore_ids: &HashSet<String>,
    limit: usize,
) -> Vec<BlockPosition> {
    let limit = limit.min(MAX_VEIN_BLOCKS);
    if limit == 0 || !known.get(&seed).is_some_and(|id| ore_ids.contains(id)) {
        return Vec::new();
    }
    let mut queue = VecDeque::from([seed]);
    let mut seen = HashSet::from([seed]);
    let mut result = Vec::new();
    while let Some(position) = queue.pop_front() {
        result.push(position);
        if result.len() == limit {
            break;
        }
        let mut next = neighbors(position);
        next.sort_by_key(|p| (p.y, p.x, p.z));
        for neighbor in next {
            if !seen.contains(&neighbor)
                && known.get(&neighbor).is_some_and(|id| ore_ids.contains(id))
            {
                seen.insert(neighbor);
                queue.push_back(neighbor);
            }
        }
    }
    result
}

/// Deterministic ranking: safe first, squared distance, then coordinates.
pub fn rank_targets(
    mut targets: Vec<(BlockPosition, SafetyVerdict)>,
    bot: BlockPosition,
) -> Vec<(BlockPosition, SafetyVerdict)> {
    targets.sort_by_key(|(p, verdict)| {
        let safety_rank = if *verdict == SafetyVerdict::Safe {
            0
        } else {
            1
        };
        let dx = i64::from(p.x - bot.x);
        let dy = i64::from(p.y - bot.y);
        let dz = i64::from(p.z - bot.z);
        (safety_rank, dx * dx + dy * dy + dz * dz, p.x, p.y, p.z)
    });
    targets
}

use crate::{
    blocks::{BlockSearchService, block_query::BlockSearchQuery},
    error::AppError,
    interaction::{InteractionController, interaction_controller::InteractionState},
    look::LookController,
    minecraft::client::{ConnectionState, MinecraftClient},
    movement::MovementService,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MiningState {
    #[default]
    Idle,
    Selecting,
    Breaking,
    AwaitingPickup,
    Completed,
    Partial,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct MiningSnapshot {
    pub state: MiningState,
    pub requested: u32,
    pub collected: u32,
    pub blocks_broken: u32,
    pub skipped: u32,
    pub failures: usize,
    pub target: Option<BlockPosition>,
    pub reason: Option<String>,
}
impl Default for MiningSnapshot {
    fn default() -> Self {
        Self {
            state: MiningState::Idle,
            requested: 0,
            collected: 0,
            blocks_broken: 0,
            skipped: 0,
            failures: 0,
            target: None,
            reason: None,
        }
    }
}

struct MiningInner {
    snapshot: MiningSnapshot,
    definition: Option<OreDefinition>,
    candidates: Vec<BlockPosition>,
    failed: HashSet<BlockPosition>,
    initial_count: u32,
    observed_count: u32,
    started: Option<Instant>,
    pickup_since: Option<Instant>,
    timeout: Duration,
}

/// A bounded coordinator which delegates world scanning, Azalea navigation and
/// intentional digging to the existing services. Progress is inventory delta,
/// never an assumed block drop.
#[derive(Clone)]
pub struct MiningService {
    search: BlockSearchService,
    inner: Arc<Mutex<MiningInner>>,
}
impl MiningService {
    pub fn new(search: BlockSearchService) -> Self {
        Self {
            search,
            inner: Arc::new(Mutex::new(MiningInner {
                snapshot: MiningSnapshot::default(),
                definition: None,
                candidates: Vec::new(),
                failed: HashSet::new(),
                initial_count: 0,
                observed_count: 0,
                started: None,
                pickup_since: None,
                timeout: Duration::from_secs(120),
            })),
        }
    }
    pub async fn snapshot(&self) -> MiningSnapshot {
        self.inner.lock().await.snapshot.clone()
    }
    pub async fn start(
        &self,
        minecraft: &MinecraftClient,
        selector: OreSelector,
        requested: u32,
        radius: u32,
    ) -> Result<(), AppError> {
        if requested == 0 {
            return Err(AppError::InvalidConsoleSyntax(
                "ore count must be positive".into(),
            ));
        }
        let definition = resolve(&selector)
            .ok_or_else(|| AppError::InvalidBlockIdentifier("unsupported ore id/group".into()))?;
        let world = minecraft.world_state_snapshot().await;
        let bot = world
            .bot
            .block_position
            .ok_or(AppError::BlockSearchPositionUnavailable)?;
        let tools = world
            .inventory
            .slots
            .iter()
            .filter_map(|s| s.item_id.as_deref());
        if !has_harvest_tool(tools, definition.minimum_tier) {
            return Err(AppError::NoSuitableHarvestTool);
        }
        let mut found = Vec::new();
        for id in &definition.blocks {
            found.extend(
                self.search
                    .search_raw(
                        minecraft,
                        BlockSearchQuery {
                            block_id: (*id).into(),
                            radius,
                            maximum_results: 64,
                            vertical_range: radius.min(64),
                        },
                    )
                    .await?,
            );
        }
        let ore_ids: HashSet<String> = definition
            .blocks
            .iter()
            .map(|id| (*id).to_string())
            .collect();
        let ore_world: HashMap<_, _> = found
            .iter()
            .map(|c| (c.position, c.block_id.clone()))
            .collect();
        let mut seeds = found.iter().map(|c| c.position).collect::<Vec<_>>();
        seeds.sort_by_key(|p| (p.x, p.y, p.z));
        let mut vein_order = Vec::new();
        let mut included = HashSet::new();
        for seed in seeds {
            for position in connected_vein(&ore_world, seed, &ore_ids, MAX_VEIN_BLOCKS) {
                if included.insert(position) {
                    vein_order.push(position);
                }
            }
        }
        let mut assessed = Vec::new();
        for position in vein_order {
            let mut cells = neighbors(position).to_vec();
            cells.extend([
                bot,
                BlockPosition {
                    y: bot.y + 1,
                    ..bot
                },
                BlockPosition {
                    y: bot.y - 1,
                    ..bot
                },
            ]);
            let known = minecraft
                .block_ids_at(&cells)
                .await?
                .into_iter()
                .filter_map(|(p, id)| id.map(|id| (p, id)))
                .collect();
            assessed.push((position, safety(&known, position, bot)));
        }
        let ranked = rank_targets(assessed, bot);
        let skipped = ranked
            .iter()
            .filter(|(_, v)| *v != SafetyVerdict::Safe)
            .count() as u32;
        let candidates = ranked
            .into_iter()
            .filter_map(|(p, v)| (v == SafetyVerdict::Safe).then_some(p))
            .collect::<Vec<_>>();
        let initial = if definition.drop.is_empty() {
            0
        } else {
            world.inventory.count_item(definition.drop)
        };
        let mut inner = self.inner.lock().await;
        inner.snapshot = MiningSnapshot {
            state: MiningState::Selecting,
            requested,
            collected: 0,
            blocks_broken: 0,
            skipped,
            failures: 0,
            target: None,
            reason: None,
        };
        inner.definition = Some(definition);
        inner.candidates = candidates;
        inner.failed.clear();
        inner.initial_count = initial;
        inner.observed_count = initial;
        inner.started = Some(Instant::now());
        inner.pickup_since = None;
        Ok(())
    }
    pub async fn tick(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
    ) {
        let snap = self.snapshot().await;
        if !matches!(
            snap.state,
            MiningState::Selecting | MiningState::Breaking | MiningState::AwaitingPickup
        ) {
            return;
        }
        let world = minecraft.world_state_snapshot().await;
        if minecraft.connection_state() != ConnectionState::Connected || !world.joined_world() {
            self.finish(MiningState::Failed, "disconnected").await;
            return;
        }
        if world.bot.alive == Some(false) {
            self.finish(MiningState::Failed, "died").await;
            return;
        }
        let timed_out = {
            let i = self.inner.lock().await;
            i.started.is_some_and(|at| at.elapsed() > i.timeout)
        };
        if timed_out {
            self.finish(MiningState::Partial, "timed out").await;
            return;
        }
        match snap.state {
            MiningState::Selecting => {
                let target = {
                    let i = self.inner.lock().await;
                    i.candidates.iter().copied().find(|p| !i.failed.contains(p))
                };
                let Some(target) = target else {
                    self.finish(
                        if snap.collected > 0 {
                            MiningState::Partial
                        } else {
                            MiningState::Failed
                        },
                        "no safe matching loaded ore remains",
                    )
                    .await;
                    return;
                };
                let current = minecraft
                    .block_ids_at(&[target])
                    .await
                    .ok()
                    .and_then(|mut m| m.remove(&target).flatten());
                let still_ore = {
                    let i = self.inner.lock().await;
                    current.as_deref().is_some_and(|id| {
                        i.definition
                            .as_ref()
                            .is_some_and(|d| d.blocks.contains(&id))
                    })
                };
                if !still_ore {
                    let mut i = self.inner.lock().await;
                    i.failed.insert(target);
                    i.snapshot.failures = i.failed.len();
                    return;
                }
                match interaction
                    .break_at(minecraft, movement, look, target)
                    .await
                {
                    Ok(()) => {
                        let mut i = self.inner.lock().await;
                        i.snapshot.state = MiningState::Breaking;
                        i.snapshot.target = Some(target);
                    }
                    Err(e) => {
                        let mut i = self.inner.lock().await;
                        i.failed.insert(target);
                        i.snapshot.failures = i.failed.len();
                        i.snapshot.reason = Some(e.to_string());
                    }
                }
            }
            MiningState::Breaking => match interaction.snapshot().await.state {
                InteractionState::Completed => {
                    let mut i = self.inner.lock().await;
                    i.snapshot.blocks_broken += 1;
                    i.snapshot.state = MiningState::AwaitingPickup;
                    i.pickup_since = Some(Instant::now());
                }
                InteractionState::Failed | InteractionState::Cancelled => {
                    let mut i = self.inner.lock().await;
                    if let Some(p) = i.snapshot.target {
                        i.failed.insert(p);
                    }
                    i.snapshot.failures = i.failed.len();
                    i.snapshot.state = MiningState::Selecting;
                }
                _ => {}
            },
            MiningState::AwaitingPickup => {
                let (drop, initial, observed, since) = {
                    let i = self.inner.lock().await;
                    (
                        i.definition.as_ref().map(|d| d.drop).unwrap_or(""),
                        i.initial_count,
                        i.observed_count,
                        i.pickup_since,
                    )
                };
                let now = world.inventory.count_item(drop);
                if now > observed {
                    let mut i = self.inner.lock().await;
                    i.observed_count = now;
                    i.snapshot.collected = now.saturating_sub(initial);
                    i.snapshot.target = None;
                    i.pickup_since = None;
                    if i.snapshot.collected >= i.snapshot.requested {
                        i.snapshot.state = MiningState::Completed;
                    } else {
                        i.snapshot.state = MiningState::Selecting;
                    }
                } else if since.is_some_and(|s| s.elapsed() > Duration::from_secs(5)) {
                    let mut i = self.inner.lock().await;
                    if let Some(p) = i.snapshot.target {
                        i.failed.insert(p);
                    }
                    i.snapshot.failures = i.failed.len();
                    i.snapshot.reason = Some("pickup not confirmed".into());
                    i.snapshot.target = None;
                    i.snapshot.state = MiningState::Selecting;
                    i.pickup_since = None;
                }
            }
            _ => {}
        }
    }
    async fn finish(&self, state: MiningState, reason: &str) {
        let mut i = self.inner.lock().await;
        i.snapshot.state = state;
        i.snapshot.reason = Some(reason.into());
        i.snapshot.target = None;
    }
    pub async fn cancel(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
    ) {
        interaction.cancel(minecraft, movement, look).await;
        self.finish(MiningState::Cancelled, "cancelled").await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(x: i32, y: i32, z: i32) -> BlockPosition {
        BlockPosition { x, y, z }
    }
    fn safe_world(target: BlockPosition, bot: BlockPosition) -> HashMap<BlockPosition, String> {
        let mut w = HashMap::new();
        for n in neighbors(target) {
            w.insert(n, "minecraft:stone".into());
        }
        w.insert(bot, "minecraft:air".into());
        w.insert(p(bot.x, bot.y + 1, bot.z), "minecraft:air".into());
        w.insert(p(bot.x, bot.y - 1, bot.z), "minecraft:stone".into());
        w
    }
    #[test]
    fn resolves_groups_and_enforces_tiers() {
        let d = resolve(&OreSelector::Group("diamond".into())).unwrap();
        assert!(d.blocks.len() == 2);
        assert!(!has_harvest_tool(
            ["minecraft:stone_pickaxe"],
            d.minimum_tier
        ));
        assert!(has_harvest_tool(["minecraft:iron_pickaxe"], d.minimum_tier));
    }
    #[test]
    fn vein_is_connected_deterministic_and_bounded() {
        let mut w = HashMap::new();
        for x in 0..100 {
            w.insert(p(x, 10, 0), "minecraft:iron_ore".into());
        }
        w.insert(p(1, 11, 1), "minecraft:iron_ore".into());
        let ids = HashSet::from(["minecraft:iron_ore".into()]);
        let v = connected_vein(&w, p(0, 10, 0), &ids, 8);
        assert_eq!(v.len(), 8);
        assert_eq!(v[0], p(0, 10, 0));
        assert!(!v.contains(&p(1, 11, 1)));
    }
    #[test]
    fn hazards_and_unknown_boundaries_are_conservative() {
        let t = p(2, 10, 0);
        let b = p(0, 10, 0);
        let mut w = safe_world(t, b);
        assert_eq!(safety(&w, t, b), SafetyVerdict::Safe);
        w.insert(p(2, 10, 1), "minecraft:lava".into());
        assert_eq!(safety(&w, t, b), SafetyVerdict::AdjacentLava);
        w.remove(&p(2, 10, 1));
        assert_eq!(safety(&w, t, b), SafetyVerdict::SafetyUnknown);
    }
    #[test]
    fn falling_void_forbidden_and_support_are_detected() {
        let t = p(2, 10, 0);
        let b = p(0, 10, 0);
        let mut w = safe_world(t, b);
        w.insert(p(2, 11, 0), "minecraft:gravel".into());
        assert_eq!(safety(&w, t, b), SafetyVerdict::FallingBlock);
        assert_eq!(
            safety(&safe_world(p(0, 9, 0), b), p(0, 9, 0), b),
            SafetyVerdict::StraightDown
        );
        assert_eq!(
            safety(&safe_world(p(0, 11, 0), b), p(0, 11, 0), b),
            SafetyVerdict::Overhead
        );
        let mut w = safe_world(t, b);
        w.insert(p(0, 9, 0), "minecraft:air".into());
        assert_eq!(safety(&w, t, b), SafetyVerdict::VoidExposure);
    }
    #[test]
    fn ranking_is_stable_and_safe_first() {
        let b = p(0, 0, 0);
        let got = rank_targets(
            vec![
                (p(1, 0, 0), SafetyVerdict::SafetyUnknown),
                (p(0, 0, 2), SafetyVerdict::Safe),
                (p(-2, 0, 0), SafetyVerdict::Safe),
            ],
            b,
        );
        assert_eq!(got[0].0, p(-2, 0, 0));
        assert_eq!(got[1].0, p(0, 0, 2));
    }
    #[test]
    fn changed_or_unloaded_target_is_not_a_known_ore() {
        let ids = HashSet::from(["minecraft:diamond_ore".into()]);
        let mut w = HashMap::new();
        assert!(connected_vein(&w, p(1, 2, 3), &ids, 8).is_empty());
        w.insert(p(1, 2, 3), "minecraft:stone".into());
        assert!(connected_vein(&w, p(1, 2, 3), &ids, 8).is_empty());
    }
}
