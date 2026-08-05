//! Mob-combat extension of `#get`'s existing block-gathering pipeline (see
//! `App::run_get_mob` in `src/app.rs`). This is deliberately not a new
//! pathfinding or aiming system: movement reuses `MovementService::goto`
//! exactly as `/goto` does (just re-aimed at the target's live position
//! while out of range, mirroring how `BlockNavigationService` falls back
//! across candidates when one proves unreachable), and aiming reuses
//! `LookController`'s existing `LookTarget::Entity` support (already used by
//! `/lookentity`). The only genuinely new primitive is dispatching the
//! attack packet itself (`MinecraftClient::attack_entity`), since nothing in
//! this codebase attacked an entity before.

use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use tokio::sync::Mutex;

use super::drops::mob_label;
use crate::{
    error::AppError,
    interaction::tool_selection::tier,
    logging,
    look::{LookController, LookTarget},
    minecraft::{
        client::MinecraftClient,
        world_state::{InventorySnapshot, MovementStatus, PositionSnapshot},
    },
    movement::{MovementService, NavigationMode},
};

/// Vanilla melee reach is roughly 3 blocks; this is the range at which the
/// bot stops closing distance and starts attacking.
const ATTACK_RANGE: f64 = 3.0;
/// Once attacking, don't immediately break off and re-path for a small,
/// probably-momentary overshoot -- only resume chasing once genuinely out of
/// range.
const REENGAGE_RANGE: f64 = ATTACK_RANGE * 1.6;
/// Paces how often the client bothers to send another attack packet;
/// Azalea's own `AttackStrengthScale`/cooldown machinery still governs
/// server-side damage regardless of how often this fires.
const ATTACK_INTERVAL: Duration = Duration::from_millis(600);
/// `LookTarget::Entity` tracks a moving target continuously and therefore
/// (see `look::look_controller::keeps_ticking`) never reports `Completed` --
/// waiting for that would hang forever. Give the rotation a short, bounded
/// head start instead: a miss here is fine, and never worth waiting forever
/// for a perfect aim.
const LOOK_GRACE: Duration = Duration::from_millis(400);
/// Re-issue movement toward the target's updated position only once it has
/// drifted more than this -- avoids fighting Azalea's in-flight path
/// computation by resubmitting a goto every single tick.
const ENTITY_GOAL_DRIFT_TOLERANCE: f64 = 2.0;
/// Safety ceiling on chasing/attacking one specific target so a mob that
/// keeps fleeing (or one that's genuinely unreachable, across terrain the
/// pathfinder can't cross) cannot stall `#get` forever. The caller treats
/// this exactly like an unreachable block candidate and tries the next
/// nearest mob -- see `retry_next_candidate`.
const TARGET_TIMEOUT: Duration = Duration::from_secs(45);
/// Bounded walk onto the drop location after a kill, to reliably trigger
/// vanilla's proximity item pickup even though melee reach is wider than the
/// pickup radius.
const COLLECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Vanilla's actual item pickup range is much tighter than
/// `MovementConfig::arrival_distance` (the general-purpose "close enough to
/// interact" threshold `MovementService` itself stops at) -- `tick_collecting`
/// polls the bot's live position against this instead of trusting
/// `MovementStatus::Completed`, re-issuing the walk if movement gives up
/// early while still outside it.
const COLLECT_ARRIVAL_DISTANCE: f64 = 0.6;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CombatState {
    #[default]
    Idle,
    Moving,
    Looking,
    Attacking,
    Collecting,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct CombatSnapshot {
    pub state: CombatState,
    pub mob_id: Option<String>,
    pub target_entity_id: Option<u32>,
    pub target_position: Option<PositionSnapshot>,
    /// Kept for parity with every other controller's snapshot in this
    /// codebase (`InteractionSnapshot`, `LookSnapshot`, ...); unlike those,
    /// nothing currently reads it back since combat has no dedicated status
    /// command (this task extends `#get`, not the command surface).
    #[allow(dead_code)]
    pub started_at: Option<SystemTime>,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct EntityCandidate {
    entity_id: u32,
    position: PositionSnapshot,
}

#[derive(Default)]
struct Inner {
    snapshot: CombatSnapshot,
    candidates: Vec<EntityCandidate>,
    failed_entities: HashSet<u32>,
    target_entity_id: Option<u32>,
    last_goto_position: Option<PositionSnapshot>,
    target_acquired_at: Option<Instant>,
    looking_since: Option<Instant>,
    next_attack_at: Option<SystemTime>,
    collecting_started_at: Option<Instant>,
}

#[derive(Clone)]
pub struct CombatController {
    inner: Arc<Mutex<Inner>>,
}

impl Default for CombatController {
    fn default() -> Self {
        Self::new()
    }
}

impl CombatController {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    pub async fn snapshot(&self) -> CombatSnapshot {
        self.inner.lock().await.snapshot.clone()
    }

    /// Scans currently loaded entities for the nearest live `mob_id` within
    /// `radius` blocks and starts approaching it. Mirrors
    /// `BlockNavigationService::start`'s candidate-list-with-fallback shape,
    /// just over entities instead of blocks.
    pub async fn kill_nearest(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        mob_id: String,
        radius: u32,
    ) -> Result<(), AppError> {
        let world = minecraft.world_state_snapshot().await;
        let bot_position = world
            .bot
            .position
            .ok_or(AppError::BlockSearchPositionUnavailable)?;
        let mut candidates: Vec<EntityCandidate> = world
            .entities
            .iter()
            .filter(|entity| {
                entity.entity_type == mob_id
                    && entity.alive != Some(false)
                    && entity.health.is_none_or(|health| health > 0.0)
                    && entity.distance <= f64::from(radius)
            })
            .map(|entity| EntityCandidate {
                entity_id: entity.entity_id,
                position: entity.position,
            })
            .collect();
        candidates.sort_by(|a, b| {
            distance(bot_position, a.position)
                .total_cmp(&distance(bot_position, b.position))
                .then_with(|| a.entity_id.cmp(&b.entity_id))
        });
        if candidates.is_empty() {
            return Err(AppError::NoMatchingMob);
        }
        {
            let mut inner = self.inner.lock().await;
            inner.candidates = candidates;
            inner.failed_entities.clear();
            inner.target_entity_id = None;
            inner.last_goto_position = None;
            inner.looking_since = None;
            inner.next_attack_at = None;
            inner.collecting_started_at = None;
            inner.snapshot = CombatSnapshot {
                state: CombatState::Idle,
                mob_id: Some(mob_id),
                started_at: Some(SystemTime::now()),
                ..CombatSnapshot::default()
            };
        }
        let _ = movement.stop(minecraft).await;
        if !self.try_next_candidate(minecraft, movement).await? {
            self.fail("no reachable mob").await;
            return Err(AppError::NoMatchingMob);
        }
        Ok(())
    }

    /// An idle controller is a no-op, the same contract every other
    /// controller in this codebase follows (see e.g.
    /// `InteractionController::cancel`) -- callers defensively cancel before
    /// starting something new.
    pub async fn cancel(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
    ) {
        let active = {
            let inner = self.inner.lock().await;
            matches!(
                inner.snapshot.state,
                CombatState::Moving
                    | CombatState::Looking
                    | CombatState::Attacking
                    | CombatState::Collecting
            )
        };
        if !active {
            return;
        }
        let _ = movement.stop(minecraft).await;
        look.cancel().await;
        logging::info("Combat cancelled");
        let mut inner = self.inner.lock().await;
        inner.snapshot.state = CombatState::Cancelled;
        inner.snapshot.failure_reason = None;
        inner.candidates.clear();
        inner.failed_entities.clear();
        inner.target_entity_id = None;
    }

    pub async fn tick(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
    ) {
        let state = self.inner.lock().await.snapshot.state;
        match state {
            CombatState::Moving => self.tick_moving(minecraft, movement, look).await,
            CombatState::Looking => self.tick_looking(minecraft, movement).await,
            CombatState::Attacking => self.tick_attacking(minecraft, movement).await,
            CombatState::Collecting => self.tick_collecting(minecraft, movement).await,
            _ => {}
        }
    }

    async fn tick_moving(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
    ) {
        let world = minecraft.world_state_snapshot().await;
        if world.bot.alive == Some(false) || !world.joined_world() {
            self.fail("bot is not available").await;
            return;
        }
        let (target_entity_id, timed_out) = {
            let inner = self.inner.lock().await;
            (
                inner.target_entity_id,
                inner
                    .target_acquired_at
                    .is_some_and(|at| at.elapsed() >= TARGET_TIMEOUT),
            )
        };
        let Some(target_entity_id) = target_entity_id else {
            self.fail("lost target").await;
            return;
        };
        if timed_out {
            self.retry_next_candidate(minecraft, movement, target_entity_id)
                .await;
            return;
        }
        let Some(entity) = world
            .entities
            .iter()
            .find(|entity| entity.entity_id == target_entity_id)
        else {
            // Despawned, or killed by something else, while approaching --
            // not a hard failure. Treated exactly like an unreachable block
            // candidate: drop it and try the next nearest mob.
            self.retry_next_candidate(minecraft, movement, target_entity_id)
                .await;
            return;
        };
        let Some(bot_position) = world.bot.position else {
            return;
        };
        if distance(bot_position, entity.position) <= ATTACK_RANGE {
            let _ = movement.stop(minecraft).await;
            let mob_id = self.mob_id().await;
            logging::info(format!("Looking at {}", mob_label(&mob_id)));
            let _ = look
                .look_at(minecraft, LookTarget::Entity(target_entity_id))
                .await;
            let mut inner = self.inner.lock().await;
            inner.looking_since = Some(Instant::now());
            inner.snapshot.state = CombatState::Looking;
            inner.snapshot.target_position = Some(entity.position);
            return;
        }
        let movement_snapshot = movement.snapshot().await;
        if movement_snapshot.status == MovementStatus::Failed {
            self.retry_next_candidate(minecraft, movement, target_entity_id)
                .await;
            return;
        }
        let last_goto_position = self.inner.lock().await.last_goto_position;
        let need_new_goto = last_goto_position.is_none_or(|previous| {
            distance(previous, entity.position) > ENTITY_GOAL_DRIFT_TOLERANCE
        }) || movement_snapshot.status != MovementStatus::MovingToPosition;
        if need_new_goto {
            if movement
                .goto(minecraft, entity.position, NavigationMode::AllowMining)
                .await
                .is_err()
            {
                self.retry_next_candidate(minecraft, movement, target_entity_id)
                    .await;
                return;
            }
            self.inner.lock().await.last_goto_position = Some(entity.position);
        }
        self.inner.lock().await.snapshot.target_position = Some(entity.position);
    }

    async fn tick_looking(&self, minecraft: &MinecraftClient, movement: &MovementService) {
        let (target_entity_id, looking_since) = {
            let inner = self.inner.lock().await;
            (inner.target_entity_id, inner.looking_since)
        };
        let Some(target_entity_id) = target_entity_id else {
            self.fail("lost target").await;
            return;
        };
        let world = minecraft.world_state_snapshot().await;
        let Some(entity) = world
            .entities
            .iter()
            .find(|entity| entity.entity_id == target_entity_id)
        else {
            self.retry_next_candidate(minecraft, movement, target_entity_id)
                .await;
            return;
        };
        if world
            .bot
            .position
            .is_some_and(|position| distance(position, entity.position) > REENGAGE_RANGE)
        {
            self.inner.lock().await.snapshot.state = CombatState::Moving;
            return;
        }
        // `LookTarget::Entity` continuously tracks the (possibly still
        // moving) target and therefore never reports `Completed` -- see this
        // module's doc comment. A bounded grace period stands in for that.
        if looking_since.is_some_and(|since| since.elapsed() < LOOK_GRACE) {
            return;
        }
        let mob_id = self.mob_id().await;
        logging::info(format!("Attacking {}", mob_label(&mob_id)));
        self.equip_best_sword(minecraft).await;
        let mut inner = self.inner.lock().await;
        inner.snapshot.state = CombatState::Attacking;
        inner.next_attack_at = None;
    }

    /// Switches to the best sword already sitting in the hotbar before the
    /// first attack, the same way `InteractionController::dispatch` selects
    /// a mining tool before breaking -- reusing the same material-tier
    /// knowledge (`crate::interaction::tool_selection::tier`) rather than a
    /// separate weapon-ranking scheme. If the hotbar holds no sword at all,
    /// this is a no-op: the bot attacks with whatever is already held rather
    /// than refusing to fight unarmed.
    async fn equip_best_sword(&self, minecraft: &MinecraftClient) {
        let inventory = minecraft.world_state_snapshot().await.inventory;
        let Some(sword) = best_sword_in_hotbar(&inventory) else {
            return;
        };
        if matches!(minecraft.select_item_in_hotbar(&sword).await, Ok(true)) {
            logging::info(format!("Equipped {sword}"));
        }
    }

    async fn tick_attacking(&self, minecraft: &MinecraftClient, movement: &MovementService) {
        let target_entity_id = self.inner.lock().await.target_entity_id;
        let Some(target_entity_id) = target_entity_id else {
            self.fail("lost target").await;
            return;
        };
        let world = minecraft.world_state_snapshot().await;
        let Some(entity) = world
            .entities
            .iter()
            .find(|entity| entity.entity_id == target_entity_id)
        else {
            // The target is gone: killed (this is the only removal signal
            // available -- `WorldState::upsert_entity`/`remove_entity` drop
            // an entity on death or despawn alike, see world_state.rs) or,
            // more rarely, it despawned/unloaded first. Either way nothing
            // is left to attack, so treat it as a completed kill and collect
            // whatever drop landed at its last known position.
            let death_position = self.inner.lock().await.snapshot.target_position;
            self.begin_collecting(minecraft, movement, death_position)
                .await;
            return;
        };
        let Some(bot_position) = world.bot.position else {
            return;
        };
        if distance(bot_position, entity.position) > REENGAGE_RANGE {
            self.inner.lock().await.snapshot.state = CombatState::Moving;
            return;
        }
        self.inner.lock().await.snapshot.target_position = Some(entity.position);
        let ready = {
            let inner = self.inner.lock().await;
            inner
                .next_attack_at
                .is_none_or(|at| SystemTime::now() >= at)
        };
        if ready {
            let _ = minecraft.attack_entity(target_entity_id).await;
            self.inner.lock().await.next_attack_at = Some(SystemTime::now() + ATTACK_INTERVAL);
        }
    }

    async fn begin_collecting(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        death_position: Option<PositionSnapshot>,
    ) {
        if let Some(position) = death_position {
            let _ = movement
                .goto(minecraft, position, NavigationMode::AllowMining)
                .await;
        }
        let mut inner = self.inner.lock().await;
        inner.snapshot.state = CombatState::Collecting;
        inner.collecting_started_at = Some(Instant::now());
    }

    async fn tick_collecting(&self, minecraft: &MinecraftClient, movement: &MovementService) {
        let (death_position, started_at) = {
            let inner = self.inner.lock().await;
            (inner.snapshot.target_position, inner.collecting_started_at)
        };
        let world = minecraft.world_state_snapshot().await;
        let arrived = world
            .bot
            .position
            .zip(death_position)
            .is_some_and(|(position, target)| {
                distance(position, target) <= COLLECT_ARRIVAL_DISTANCE
            });
        let timed_out = started_at.is_some_and(|at| at.elapsed() >= COLLECT_TIMEOUT);
        if arrived || timed_out {
            let _ = movement.stop(minecraft).await;
            self.inner.lock().await.snapshot.state = CombatState::Completed;
            return;
        }
        // `MovementService`'s own arrival threshold is looser than
        // `COLLECT_ARRIVAL_DISTANCE` -- if it already gave up (reached its
        // own threshold, or genuinely failed) while still outside the tight
        // distance actually needed for pickup, nudge it toward the drop
        // again rather than silently leaving the item uncollected.
        let movement_status = movement.snapshot().await.status;
        if matches!(
            movement_status,
            MovementStatus::Completed | MovementStatus::Idle | MovementStatus::Failed
        ) && let Some(target) = death_position
        {
            let _ = movement
                .goto(minecraft, target, NavigationMode::AllowMining)
                .await;
        }
    }

    async fn retry_next_candidate(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        failed_entity_id: u32,
    ) {
        self.mark_entity_failed(failed_entity_id).await;
        if !self
            .try_next_candidate(minecraft, movement)
            .await
            .unwrap_or(false)
        {
            self.fail("no reachable mob").await;
        }
    }

    async fn try_next_candidate(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
    ) -> Result<bool, AppError> {
        loop {
            let candidate = {
                let inner = self.inner.lock().await;
                inner
                    .candidates
                    .iter()
                    .find(|candidate| !inner.failed_entities.contains(&candidate.entity_id))
                    .copied()
            };
            let Some(candidate) = candidate else {
                return Ok(false);
            };
            if movement
                .goto(minecraft, candidate.position, NavigationMode::AllowMining)
                .await
                .is_err()
            {
                self.mark_entity_failed(candidate.entity_id).await;
                continue;
            }
            let mob_id = {
                let mut inner = self.inner.lock().await;
                inner.target_entity_id = Some(candidate.entity_id);
                inner.last_goto_position = Some(candidate.position);
                inner.target_acquired_at = Some(Instant::now());
                inner.snapshot.target_entity_id = Some(candidate.entity_id);
                inner.snapshot.target_position = Some(candidate.position);
                inner.snapshot.state = CombatState::Moving;
                inner.snapshot.mob_id.clone().unwrap_or_default()
            };
            logging::info(format!(
                "Going to nearest {} at ({}, {}, {})",
                mob_label(&mob_id),
                candidate.position.x.floor() as i32,
                candidate.position.y.floor() as i32,
                candidate.position.z.floor() as i32,
            ));
            return Ok(true);
        }
    }

    async fn mark_entity_failed(&self, entity_id: u32) {
        self.inner.lock().await.failed_entities.insert(entity_id);
    }

    async fn mob_id(&self) -> String {
        self.inner
            .lock()
            .await
            .snapshot
            .mob_id
            .clone()
            .unwrap_or_default()
    }

    async fn fail(&self, reason: &str) {
        let mut inner = self.inner.lock().await;
        inner.snapshot.state = CombatState::Failed;
        inner.snapshot.failure_reason = Some(reason.to_owned());
    }
}

fn distance(a: PositionSnapshot, b: PositionSnapshot) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

/// Best available sword already sitting in the hotbar (this bot has no way
/// to move an item there automatically -- the same hotbar-only constraint
/// `MinecraftClient::select_item_in_hotbar` already imposes), ranked by
/// material tier via the shared `tier` knowledge the mining tool-selection
/// path uses. `None` if the hotbar holds no sword at all.
fn best_sword_in_hotbar(inventory: &InventorySnapshot) -> Option<String> {
    let first_hotbar = inventory.slots.len().checked_sub(9)?;
    inventory.slots[first_hotbar..]
        .iter()
        .filter_map(|slot| slot.item_id.as_deref())
        .filter(|id| id.ends_with("_sword"))
        .max_by_key(|id| tier(id))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        blocks::BlockSearchService,
        config::{
            AccountMode, BridgingConfig, ConsoleConfig, LookConfig, MinecraftConfig,
            MovementConfig, MultitaskingConfig, ReconnectConfig, VerticalNavigationConfig,
            WorldStateConfig,
        },
        look::LookController,
        minecraft::client::MinecraftClient,
        movement::MovementService,
    };

    fn minecraft() -> MinecraftClient {
        MinecraftClient::new(
            MinecraftConfig {
                server: "localhost:25565".to_owned(),
                username: "MagicBot".to_owned(),
                account_mode: AccountMode::Offline,
            },
            ReconnectConfig {
                enabled: false,
                delay_seconds: 10,
                maximum_attempts: 5,
            },
            ConsoleConfig::default(),
            WorldStateConfig::default(),
            VerticalNavigationConfig::default(),
            BridgingConfig::default(),
        )
    }

    fn movement() -> MovementService {
        MovementService::new(MovementConfig::default(), MultitaskingConfig::default())
    }

    fn look() -> LookController {
        LookController::new(LookConfig::default(), BlockSearchService::new(32, 20, 32))
    }

    #[tokio::test]
    async fn starts_idle() {
        let combat = CombatController::new();
        assert_eq!(combat.snapshot().await.state, CombatState::Idle);
    }

    #[tokio::test]
    async fn cancel_on_an_idle_controller_is_a_no_op() {
        let combat = CombatController::new();
        combat.cancel(&minecraft(), &movement(), &look()).await;
        assert_eq!(combat.snapshot().await.state, CombatState::Idle);
    }

    #[tokio::test]
    async fn kill_nearest_without_a_known_position_fails_without_crashing() {
        // No live connection, so the bot has no known position -- must
        // report an error rather than panicking (never crash on missing
        // world state, matching the block-gathering path's contract).
        let combat = CombatController::new();
        let result = combat
            .kill_nearest(&minecraft(), &movement(), "minecraft:cow".into(), 32)
            .await;
        assert!(result.is_err());
        assert_eq!(combat.snapshot().await.state, CombatState::Idle);
    }

    #[test]
    fn distance_is_symmetric_and_zero_at_the_same_point() {
        let a = PositionSnapshot {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let b = PositionSnapshot {
            x: 3.0,
            y: 0.0,
            z: 4.0,
        };
        assert_eq!(distance(a, a), 0.0);
        assert!((distance(a, b) - 5.0).abs() < 1e-9);
        assert!((distance(a, b) - distance(b, a)).abs() < 1e-9);
    }

    fn inventory_with_hotbar(items: &[Option<&str>; 9]) -> InventorySnapshot {
        InventorySnapshot {
            available: true,
            slots: items
                .iter()
                .enumerate()
                .map(
                    |(slot, item_id)| crate::minecraft::world_state::InventorySlot {
                        slot,
                        item_id: item_id.map(str::to_owned),
                        display_name: None,
                        count: u32::from(item_id.is_some()),
                    },
                )
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn picks_the_highest_tier_sword_present() {
        let inventory = inventory_with_hotbar(&[
            Some("minecraft:wooden_sword"),
            Some("minecraft:diamond_sword"),
            Some("minecraft:stone_pickaxe"),
            None,
            Some("minecraft:iron_sword"),
            None,
            None,
            None,
            None,
        ]);
        assert_eq!(
            best_sword_in_hotbar(&inventory),
            Some("minecraft:diamond_sword".to_owned())
        );
    }

    #[test]
    fn no_sword_in_hotbar_is_not_an_error() {
        let inventory = inventory_with_hotbar(&[
            Some("minecraft:diamond_pickaxe"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ]);
        assert_eq!(best_sword_in_hotbar(&inventory), None);
        assert_eq!(best_sword_in_hotbar(&InventorySnapshot::default()), None);
    }

    #[test]
    fn a_sword_outside_the_hotbar_is_not_selected() {
        // Matches `select_item_in_hotbar`'s own hotbar-only constraint: this
        // bot has no way to move an item into the hotbar automatically.
        let mut inventory = inventory_with_hotbar(&[None; 9]);
        inventory.slots.insert(
            0,
            crate::minecraft::world_state::InventorySlot {
                slot: 0,
                item_id: Some("minecraft:netherite_sword".into()),
                display_name: None,
                count: 1,
            },
        );
        assert_eq!(best_sword_in_hotbar(&inventory), None);
    }
}
