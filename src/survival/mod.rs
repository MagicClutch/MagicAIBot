//! Automatic water-bucket MLG (a "falling water clutch"): whenever the bot
//! is falling from a height that would cause fall damage, it equips a water
//! bucket, aims at the predicted landing spot, places water just before
//! impact, lands safely, scoops the water back up, and restores whatever it
//! was holding before -- all without interrupting whatever brought it into
//! the air in the first place (`/goto`, `/follow`, bridging, pillaring,
//! mining, an AI task, knockback, ...).
//!
//! This is deliberately not a new movement system: `SurvivalController`
//! never touches `MovementService` or Azalea's own pathfinder (which already
//! re-selects its own hotbar slot immediately before every scaffold
//! placement or mining action -- see `azalea::pathfinder::tool_policy` --
//! so a transient hotbar swap here is self-healing and safe to perform
//! mid-path). It only ever borrows two things for the brief duration of the
//! clutch: the hotbar selection (restored to whatever was selected before)
//! and the camera, via the same `LookController::look_at_with_precision`
//! precise-aim path `InteractionController` already uses for
//! breaking/placing -- so it plugs into the exact same "precise look always
//! ticks, even mid-navigation" bypass in `App::run`'s look-tick branch.
//!
//! There is no task-orchestration layer in this codebase (see
//! `crate::tasks`'s doc comment) to formally "pause" -- instead, arming the
//! clutch cancels `InteractionController`/`CombatController` if either is
//! mid-operation (both are no-ops when idle), since those are the only two
//! controllers that would otherwise fight this one for the hotbar/camera.
//! Movement/navigation is left running untouched, and both of those
//! controllers already retry their own work from scratch on cancellation
//! (`App::run_get_item`/`run_mine`'s consecutive-failure retry loop,
//! `CombatController::retry_next_candidate`), so "resume the interrupted
//! task" falls out of behavior that already exists rather than needing new
//! plumbing.
//!
//! Landing prediction reuses the same world/collision primitives every other
//! block-aware controller in this codebase uses
//! (`MinecraftClient::block_ids_at`, `interaction::placement_rules`,
//! `interaction::faces`) plus a small tick-by-tick gravity simulation
//! (`prediction::ticks_to_impact`) using the same gravity/drag constants
//! Azalea's own physics engine applies every tick -- not a second physics
//! engine, just enough forward simulation to know when impact will happen.
//!
//! Placement timing itself is driven by remaining *distance*, not a fixed
//! delay: `SurvivalConfig::placement_offset_blocks` ("2-3 blocks before
//! impact") is recomputed against the bot's live position every tick
//! (`prediction::remaining_drop_blocks`), widened for the current fall
//! speed to compensate for round-trip network/server latency
//! (`prediction::latency_compensation_blocks`) and for a still-unstable
//! prediction (`prediction::effective_placement_offset`). See
//! `SurvivalController::tick_tracking` for the full decision and its debug
//! logging.

pub mod prediction;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::sync::Mutex;
use tracing::debug;

use crate::{
    config::SurvivalConfig,
    equipment::{manager::swap_into_slot, model::HOTBAR_PROTOCOL_SLOTS},
    interaction::{
        InteractionController,
        faces::{BlockFace, BlockFacePurpose, face_hit_points},
        reach::within_reach,
    },
    logging,
    look::{LookController, LookTarget, aim_point::LookPrecision, look_controller::LookState},
    minecraft::{
        client::{MinecraftClient, RaycastFace},
        world_state::{BlockPosition, WorldStateSnapshot},
    },
    mobs::CombatController,
    movement::MovementService,
};

/// Vanilla melee/interaction reach; matches the default every other
/// interaction reach setting in this codebase uses
/// (`config::default_interaction_reach`). A hard gate on the actual
/// `interact_block_face` call -- the bot cannot click a block it can't
/// reach -- but not the trigger for *when* to place: see
/// `config::SurvivalConfig::placement_offset_blocks` and
/// `prediction::effective_placement_offset`.
const PLACEMENT_REACH: f64 = 4.5;
const FACE_INSET: f64 = 0.05;
const FACE_EDGE_MARGIN: f64 = 0.15;
/// How far down to scan for a landing surface. 320 covers the full
/// Overworld build-height range in one pass.
const SCAN_DEPTH: i32 = 320;
/// Safety bound in case landing is never detected (a missed prediction) --
/// falls back to a clean abort rather than holding the bucket forever.
const PLACEMENT_TIMEOUT: Duration = Duration::from_secs(3);
const RECOVER_RETRY_INTERVAL: Duration = Duration::from_millis(400);
const MAX_RECOVER_ATTEMPTS: u32 = 3;

const WATER_BUCKET_ID: &str = prediction::WATER_BUCKET_ID;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SurvivalState {
    #[default]
    Idle,
    /// A dangerous fall was detected but the clutch cannot run (Nether, no
    /// bucket, no valid landing surface). Held until the bot is grounded
    /// again so the same episode doesn't re-log every tick.
    Suppressed,
    /// Bucket equipped, aiming at the predicted landing point, waiting for
    /// the placement timing window.
    Tracking,
    /// Water placed; waiting for the bot to touch down.
    Placed,
    /// Landed in the placed water; scooping it back up.
    Recovering,
}

#[derive(Default)]
struct Inner {
    state: SurvivalState,
    previous_hotbar_slot: Option<u8>,
    target_support: Option<BlockPosition>,
    target_water: Option<BlockPosition>,
    placed_at: Option<Instant>,
    recover_started_at: Option<Instant>,
    recover_attempts: u32,
}

#[derive(Clone)]
pub struct SurvivalController {
    config: SurvivalConfig,
    inner: Arc<Mutex<Inner>>,
}

impl SurvivalController {
    pub fn new(config: SurvivalConfig) -> Self {
        Self {
            config,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// Kept for parity with every other controller's snapshot accessor in
    /// this codebase (`CombatController::snapshot`, `LookController::snapshot`,
    /// ...) and exercised directly by this module's own tests; unlike those,
    /// nothing currently surfaces it through `/status` since this feature is
    /// fully automatic and has no command surface of its own.
    #[allow(dead_code)]
    pub async fn state(&self) -> SurvivalState {
        self.inner.lock().await.state
    }

    /// Whether the clutch is actively mid-flight (tracking a fall, just
    /// placed, or scooping the water back up). `App`'s blocking-wait loops
    /// poll much faster while this is true -- the placement/landing window
    /// is only a handful of ticks wide, and those loops otherwise sleep up
    /// to 200ms between checks, easily wide enough to step right over it.
    pub async fn is_active(&self) -> bool {
        matches!(
            self.inner.lock().await.state,
            SurvivalState::Tracking | SurvivalState::Placed | SurvivalState::Recovering
        )
    }

    /// Drops any in-progress clutch state without touching the network --
    /// used when the connection itself is gone (see `App::run`'s
    /// disconnect-handling branch), where there is nothing left to restore
    /// a hotbar slot or camera aim on. A later reconnect starts every
    /// controller fresh; without this, a clutch that was mid-flight when the
    /// connection dropped would keep referencing block positions from the
    /// old session.
    pub async fn reset(&self) {
        *self.inner.lock().await = Inner::default();
    }

    /// Force-resets the clutch for a global emergency stop
    /// (`crate::control::stop::execute`). Unlike the internal `abort` path
    /// (used for the controller's own failure handling, which deliberately
    /// holds `Suppressed` so a still-falling bot doesn't immediately re-arm
    /// and re-fail the same way), this always returns straight to a clean
    /// `Idle` so the very next tick evaluates completely fresh -- matching
    /// every other controller's emergency-stop contract. Best-effort:
    /// restores the hotbar slot and releases the camera if a clutch was in
    /// progress, but -- as with every step of an emergency stop -- never
    /// waits on anything beyond those two direct calls.
    pub async fn emergency_stop(&self, minecraft: &MinecraftClient, look: &LookController) {
        self.finish(minecraft, look, false).await;
    }

    /// Drives the fall-detection/clutch state machine one step. Cheap to
    /// call every tick: while grounded (the overwhelming majority of ticks)
    /// this returns after a handful of `Option` checks with no world
    /// queries at all.
    pub async fn tick(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
        combat: &CombatController,
    ) {
        if !self.config.water_mlg_enabled {
            return;
        }
        let world = minecraft.world_state_snapshot().await;
        if !world.joined_world() {
            return;
        }
        let state = self.inner.lock().await.state;
        match state {
            SurvivalState::Idle => {
                self.tick_idle(minecraft, &world, movement, look, interaction, combat)
                    .await;
            }
            SurvivalState::Suppressed => self.tick_suppressed(&world).await,
            SurvivalState::Tracking => self.tick_tracking(minecraft, &world, look).await,
            SurvivalState::Placed => self.tick_placed(minecraft, &world, look).await,
            SurvivalState::Recovering => self.tick_recovering(minecraft, &world, look).await,
        }
    }

    async fn tick_idle(
        &self,
        minecraft: &MinecraftClient,
        world: &WorldStateSnapshot,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
        combat: &CombatController,
    ) {
        if world.bot.alive == Some(false) {
            return;
        }
        let (Some(position), Some(block_position), Some(false), Some(velocity_y)) = (
            world.bot.position,
            world.bot.block_position,
            world.bot.on_ground,
            world.bot.velocity_y,
        ) else {
            return;
        };
        if velocity_y >= 0.0 {
            return;
        }
        let fall_distance_so_far = world.bot.fall_distance.unwrap_or(0.0);
        let prediction = match predict_landing(
            minecraft,
            block_position,
            position.y,
            velocity_y,
            fall_distance_so_far,
        )
        .await
        {
            LandingOutcome::Prediction(prediction) => prediction,
            // Already falling toward open water (no damage) or lava (a
            // different hazard entirely) -- nothing this feature can or
            // needs to do.
            LandingOutcome::SafeLiquidLanding => return,
            // Unloaded chunk ahead, or no surface found within the scan
            // depth yet -- try again next tick as the bot keeps falling.
            LandingOutcome::Unknown => return,
        };
        if !prediction::is_dangerous(
            prediction.predicted_total_fall,
            self.config.min_fall_distance,
        ) {
            return;
        }
        logging::info("Dangerous fall detected");
        if prediction::nether_disables_mlg(
            world.bot.dimension.as_deref(),
            self.config.disable_in_nether,
        ) {
            logging::info("Water MLG disabled in Nether");
            self.inner.lock().await.state = SurvivalState::Suppressed;
            return;
        }
        // A bucket anywhere in the inventory is enough to attempt this --
        // `equip_water_bucket_now` below moves it into the hotbar and
        // selects it right now. Under normal operation it's usually already
        // there, kept stocked by
        // `equipment::hotbar::HotbarEquipmentService` (see
        // `config::HotbarEquipmentConfig::slots`'s `water_bucket` slot);
        // this is the reactive fallback for whenever that hasn't happened
        // yet (disabled in config, or the hotbar was full until this exact
        // tick).
        if !world.inventory.available || !world.inventory.has_item(WATER_BUCKET_ID, 1) {
            logging::warning("Water MLG unavailable: no water bucket in inventory");
            self.inner.lock().await.state = SurvivalState::Suppressed;
            return;
        }

        // Arm: the only two controllers that would fight this one for the
        // hotbar/camera are interaction and combat -- both are no-ops when
        // idle, so this is safe to call unconditionally. Movement/navigation
        // is deliberately left untouched (see this module's doc comment).
        interaction.cancel(minecraft, movement, look).await;
        combat.cancel(minecraft, movement, look).await;

        let previous_slot = world.bot.selected_hotbar_slot;
        if !self.equip_water_bucket_now(minecraft).await {
            logging::warning("Water MLG unavailable: could not equip water bucket");
            self.inner.lock().await.state = SurvivalState::Suppressed;
            return;
        }
        let _ = look
            .look_at_with_precision(
                minecraft,
                aim_target(prediction.support, &prediction.support_id),
                LookPrecision::Precise,
            )
            .await;
        logging::info("Executing Water MLG");
        let mut inner = self.inner.lock().await;
        inner.state = SurvivalState::Tracking;
        inner.previous_hotbar_slot = previous_slot;
        inner.target_support = Some(prediction.support);
        inner.target_water = Some(prediction.water_target);
        inner.placed_at = None;
        inner.recover_started_at = None;
        inner.recover_attempts = 0;
    }

    /// Guarantees a water bucket is selected in the hotbar right now,
    /// called while arming the clutch -- a fallback for whenever
    /// `equipment::hotbar::HotbarEquipmentService` hasn't already gotten one
    /// there (disabled in config, or the hotbar was full right up until
    /// this exact tick). Unlike that proactive stocking, this is an active
    /// emergency: if no hotbar slot is empty, it evicts the first hotbar
    /// slot instead of giving up -- `swap_into_slot` never destroys the
    /// displaced item, only relocates it to wherever the bucket was.
    async fn equip_water_bucket_now(&self, minecraft: &MinecraftClient) -> bool {
        if matches!(
            minecraft.select_item_in_hotbar(WATER_BUCKET_ID).await,
            Ok(true)
        ) {
            return true;
        }
        let Ok(snapshot) = minecraft.equipment_snapshot().await else {
            return false;
        };
        let Some(source) = snapshot
            .inventory
            .iter()
            .find(|item| item.item_id == WATER_BUCKET_ID)
        else {
            return false;
        };
        let occupied: std::collections::HashSet<usize> = snapshot
            .inventory
            .iter()
            .filter(|item| HOTBAR_PROTOCOL_SLOTS.contains(&item.slot))
            .map(|item| item.slot)
            .collect();
        let mut hotbar_slots = HOTBAR_PROTOCOL_SLOTS;
        let destination = hotbar_slots
            .find(|slot| !occupied.contains(slot))
            .unwrap_or(*HOTBAR_PROTOCOL_SLOTS.start());
        if !swap_into_slot(minecraft, source.slot, destination).await {
            return false;
        }
        matches!(
            minecraft.select_item_in_hotbar(WATER_BUCKET_ID).await,
            Ok(true)
        )
    }

    /// Held for the rest of the current fall so a disabled/unavailable
    /// clutch doesn't re-log every tick; released once grounded (or no
    /// longer falling) so the next genuinely dangerous fall re-evaluates
    /// from scratch.
    async fn tick_suppressed(&self, world: &WorldStateSnapshot) {
        let grounded = world.bot.on_ground != Some(false);
        let rising = world
            .bot
            .velocity_y
            .is_none_or(|velocity_y| velocity_y >= 0.0);
        if grounded || rising {
            self.inner.lock().await.state = SurvivalState::Idle;
        }
    }

    async fn tick_tracking(
        &self,
        minecraft: &MinecraftClient,
        world: &WorldStateSnapshot,
        look: &LookController,
    ) {
        if world.bot.alive == Some(false) {
            self.abort(minecraft, look, "bot died during the clutch")
                .await;
            return;
        }
        if world.bot.on_ground == Some(true) {
            self.abort(minecraft, look, "landed before the placement window")
                .await;
            return;
        }
        if look.snapshot().await.state == LookState::Failed {
            self.abort(minecraft, look, "aim lost track of the landing surface")
                .await;
            return;
        }
        let (Some(position), Some(block_position), Some(velocity_y)) = (
            world.bot.position,
            world.bot.block_position,
            world.bot.velocity_y,
        ) else {
            return;
        };
        let fall_distance_so_far = world.bot.fall_distance.unwrap_or(0.0);
        let prediction = match predict_landing(
            minecraft,
            block_position,
            position.y,
            velocity_y,
            fall_distance_so_far,
        )
        .await
        {
            LandingOutcome::Prediction(prediction) => prediction,
            // Drifted over open water/lava after all -- no clutch needed;
            // wrap up cleanly rather than logging a scary abort.
            LandingOutcome::SafeLiquidLanding => {
                self.finish(minecraft, look, false).await;
                return;
            }
            LandingOutcome::Unknown => {
                self.abort(minecraft, look, "lost track of the landing surface")
                    .await;
                return;
            }
        };

        let stored_support = self.inner.lock().await.target_support;
        // A changed target also feeds `effective_placement_offset` below as
        // an "uncertain prediction" signal: still-resolving horizontal
        // drift (or a knockback still being absorbed) means less confidence
        // in exactly when/where impact happens, so placement fires slightly
        // earlier than usual rather than risking a total miss.
        let target_changed = stored_support != Some(prediction.support);
        if target_changed {
            // The landing point drifted (moving target, e.g. still sliding
            // sideways while falling) -- re-aim at the new block rather than
            // fighting the old, now-stale target. Always dispatched well
            // before the placement window below can fire (arming happens as
            // soon as the fall is first judged dangerous), so the aim has
            // had time to settle by the time timing actually allows placing.
            let _ = look
                .look_at_with_precision(
                    minecraft,
                    aim_target(prediction.support, &prediction.support_id),
                    LookPrecision::Precise,
                )
                .await;
            let mut inner = self.inner.lock().await;
            inner.target_support = Some(prediction.support);
            inner.target_water = Some(prediction.water_target);
        }

        let landing_feet_y = f64::from(prediction.support.y) + 1.0;
        let remaining_drop = prediction::remaining_drop_blocks(position.y, landing_feet_y);
        let effective_offset = prediction::effective_placement_offset(
            self.config.placement_offset_blocks,
            velocity_y,
            self.config.placement_latency_compensation_ms,
            target_changed,
        );
        // Must physically be able to click the block regardless of timing.
        let in_reach = within_reach(Some(position), prediction.support, PLACEMENT_REACH);
        // The actual "when": recalculated fresh every tick from the live
        // position/velocity (never a fixed sleep), targeting
        // `placement_offset_blocks` above the landing surface -- "shortly
        // before impact" like a human clutch, not the instant the landing
        // block is first detected, and not "as soon as the ground comes
        // into interaction range" either (for a fall barely past
        // `min_fall_distance`, the whole fall can be shorter than
        // `PLACEMENT_REACH`).
        let timing_ready = prediction::should_place_now(
            remaining_drop,
            effective_offset,
            prediction.ticks_to_impact,
        );
        let reason = if !in_reach {
            "landing surface not yet in interaction reach"
        } else if !timing_ready {
            "waiting for the placement offset window"
        } else {
            "within the placement offset window"
        };
        debug!(
            y = position.y,
            velocity_y,
            predicted_landing_y = landing_feet_y,
            remaining_drop,
            effective_offset,
            ticks_to_impact = prediction.ticks_to_impact,
            in_reach,
            timing_ready,
            target_changed,
            reason,
            "water mlg: tracking fall"
        );
        if !in_reach || !timing_ready {
            return;
        }

        // Last-moment guarantees, right at the placement window rather than
        // trusted from arming time: something else could in principle have
        // reselected the hotbar since then (this is idempotent and cheap --
        // a no-op if it's already selected).
        if !matches!(
            minecraft.select_item_in_hotbar(WATER_BUCKET_ID).await,
            Ok(true)
        ) {
            self.abort(minecraft, look, "water bucket no longer selectable")
                .await;
            return;
        }
        // Purely diagnostic: `interact_block_face`'s `force_direction: Up`
        // guarantees the water lands on top of `support` regardless of
        // whether the live raycast agrees (see its doc comment), so this is
        // never a hard gate on placing -- blocking on it would reintroduce
        // exactly the missed-window risk that forced face exists to avoid.
        // Still logged so a genuinely obstructed line of sight (something
        // now standing between the bot and the target) is visible in the
        // debug log even though placement proceeds anyway.
        let line_of_sight = matches!(
            minecraft.looked_block().await,
            Ok(hit) if hit.position == prediction.support
        );
        debug!(
            line_of_sight,
            chosen_placement_tick = prediction.ticks_to_impact,
            target = ?prediction.support,
            "water mlg: placing water"
        );
        match minecraft
            .interact_block_face(prediction.support, RaycastFace::Up)
            .await
        {
            Ok(()) => {
                let mut inner = self.inner.lock().await;
                inner.state = SurvivalState::Placed;
                inner.placed_at = Some(Instant::now());
            }
            Err(error) => {
                self.abort(minecraft, look, &format!("placement failed: {error}"))
                    .await;
            }
        }
    }

    async fn tick_placed(
        &self,
        minecraft: &MinecraftClient,
        world: &WorldStateSnapshot,
        look: &LookController,
    ) {
        let (target_water, placed_at) = {
            let inner = self.inner.lock().await;
            (inner.target_water, inner.placed_at)
        };
        let Some(target_water) = target_water else {
            self.abort(minecraft, look, "lost the placement target")
                .await;
            return;
        };
        if world.bot.alive == Some(false) {
            self.finish(minecraft, look, false).await;
            return;
        }
        let timed_out = placed_at.is_some_and(|at| at.elapsed() >= PLACEMENT_TIMEOUT);
        if world.bot.on_ground != Some(true) {
            if timed_out {
                self.abort(minecraft, look, "landing not detected in time")
                    .await;
            }
            return;
        }
        let placed_ok = minecraft
            .block_ids_at(&[target_water])
            .await
            .ok()
            .and_then(|ids| ids.get(&target_water).cloned().flatten())
            .as_deref()
            == Some(prediction::WATER_BLOCK_ID);
        if !placed_ok {
            logging::warning("Water MLG: placement did not take effect");
            self.finish(minecraft, look, false).await;
            return;
        }
        if !self.config.pickup_after_landing {
            logging::success("Water MLG successful");
            self.finish(minecraft, look, true).await;
            return;
        }
        let _ = minecraft.interact_block(target_water).await;
        let mut inner = self.inner.lock().await;
        inner.state = SurvivalState::Recovering;
        inner.recover_started_at = Some(Instant::now());
        inner.recover_attempts = 0;
    }

    async fn tick_recovering(
        &self,
        minecraft: &MinecraftClient,
        world: &WorldStateSnapshot,
        look: &LookController,
    ) {
        let has_bucket = prediction::water_bucket_available(
            world.inventory.available,
            world.inventory.has_item(WATER_BUCKET_ID, 1),
            world.inventory.item_is_in_hotbar(WATER_BUCKET_ID),
        );
        if has_bucket {
            logging::success("Water MLG successful");
            self.finish(minecraft, look, true).await;
            return;
        }
        let (target_water, started, attempts) = {
            let inner = self.inner.lock().await;
            (
                inner.target_water,
                inner.recover_started_at,
                inner.recover_attempts,
            )
        };
        if !started.is_some_and(|at| at.elapsed() >= RECOVER_RETRY_INTERVAL) {
            return;
        }
        if attempts >= MAX_RECOVER_ATTEMPTS {
            // Bot is already safely on the ground; prioritize wrapping up
            // over retrying forever (see this module's "Water pickup"
            // safety contract).
            logging::warning("Water MLG: bucket pickup unconfirmed");
            self.finish(minecraft, look, false).await;
            return;
        }
        if let Some(target_water) = target_water {
            let _ = minecraft.interact_block(target_water).await;
        }
        let mut inner = self.inner.lock().await;
        inner.recover_started_at = Some(Instant::now());
        inner.recover_attempts += 1;
    }

    /// Successful (or safely abandoned-but-landed) completion: restores the
    /// hotbar slot and camera ownership, then returns to `Idle` so a later
    /// fall is evaluated fresh.
    async fn finish(&self, minecraft: &MinecraftClient, look: &LookController, success: bool) {
        let previous_slot = {
            let mut inner = self.inner.lock().await;
            let slot = inner.previous_hotbar_slot.take();
            *inner = Inner::default();
            slot
        };
        if let Some(slot) = previous_slot {
            let _ = minecraft.select_hotbar_slot(slot).await;
        }
        let _ = look.release_precise(minecraft).await;
        if success {
            logging::info("Resuming previous task");
        }
    }

    /// Failure exit taken whenever the clutch cannot be completed (bad
    /// prediction, lost aim, placement error, fall ended early). Restores
    /// the hotbar/camera exactly like a success, then falls back to
    /// `Suppressed` rather than `Idle` -- if the bot is still airborne and
    /// still dangerous, immediately re-evaluating from `Idle` would just
    /// re-arm and hit the same failure again next tick, spamming a warning
    /// every tick for the rest of the fall.
    async fn abort(&self, minecraft: &MinecraftClient, look: &LookController, reason: &str) {
        logging::warning(format!("Water MLG aborted: {reason}"));
        let previous_slot = {
            let mut inner = self.inner.lock().await;
            let slot = inner.previous_hotbar_slot.take();
            inner.state = SurvivalState::Suppressed;
            slot
        };
        if let Some(slot) = previous_slot {
            let _ = minecraft.select_hotbar_slot(slot).await;
        }
        let _ = look.release_precise(minecraft).await;
    }
}

fn aim_target(support: BlockPosition, support_id: &str) -> LookTarget {
    let point = face_hit_points(
        support,
        BlockFace::Up,
        BlockFacePurpose::PlaceSupport,
        FACE_INSET,
        FACE_EDGE_MARGIN,
        1,
    )
    .into_iter()
    .next()
    .unwrap_or([
        f64::from(support.x) + 0.5,
        f64::from(support.y) + 1.0,
        f64::from(support.z) + 0.5,
    ]);
    LookTarget::BlockFacePoint {
        position: support,
        block_id: Some(support_id.to_owned()),
        point,
    }
}

/// Outcome of scanning for a landing surface, distinguishing "no clutch
/// needed" from "can't tell yet" -- collapsing both into `None` used to make
/// `tick_idle`/`tick_tracking` treat drifting over open water the same as a
/// genuinely lost prediction (aborting with a scary warning instead of
/// quietly doing nothing, which is all that case actually needs).
enum LandingOutcome {
    Prediction(prediction::LandingPrediction),
    /// The bot will land in existing water (no fall damage, nothing to do)
    /// or lava (a different hazard this feature doesn't address either
    /// way) without any help.
    SafeLiquidLanding,
    /// Unloaded chunk ahead, or no surface found within the scan depth --
    /// try again next tick as the bot keeps falling.
    Unknown,
}

/// Async glue between the pure prediction helpers in [`prediction`] and
/// `MinecraftClient::block_ids_at`. Scans straight down from `bot_feet`,
/// which naturally re-tracks horizontal drift (a "moving landing target")
/// since it's recomputed from the bot's live position every tick.
async fn predict_landing(
    minecraft: &MinecraftClient,
    bot_feet: BlockPosition,
    bot_y: f64,
    velocity_y: f64,
    fall_distance_so_far: f64,
) -> LandingOutcome {
    let positions = prediction::landing_column(bot_feet, SCAN_DEPTH);
    let Ok(ids) = minecraft.block_ids_at(&positions).await else {
        return LandingOutcome::Unknown;
    };
    let column: Vec<(BlockPosition, Option<String>)> = positions
        .iter()
        .map(|position| (*position, ids.get(position).cloned().flatten()))
        .collect();
    let Some((support, support_id)) = prediction::find_landing_support(&column) else {
        return LandingOutcome::Unknown;
    };
    if prediction::lands_in_liquid(&support_id) {
        return LandingOutcome::SafeLiquidLanding;
    }
    let water_target = BlockFace::Up.neighbor(support);
    let above_id = column
        .iter()
        .find(|(position, _)| *position == water_target)
        .and_then(|(_, id)| id.clone())
        .or_else(|| Some("minecraft:air".to_owned()));
    if !prediction::is_valid_landing_surface(&support_id, above_id.as_deref()) {
        return LandingOutcome::Unknown;
    }
    let landing_feet_y = f64::from(support.y) + 1.0;
    LandingOutcome::Prediction(prediction::LandingPrediction {
        support,
        water_target,
        ticks_to_impact: prediction::ticks_to_impact(bot_y, velocity_y, landing_feet_y),
        predicted_total_fall: prediction::predicted_total_fall(
            fall_distance_so_far,
            bot_y,
            landing_feet_y,
        ),
        support_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        blocks::BlockSearchService,
        config::{
            AccountMode, BridgingConfig, ConsoleConfig, InteractionConfig, LookConfig,
            MinecraftConfig, MovementConfig, MultitaskingConfig, ReconnectConfig,
            VerticalNavigationConfig, WorldStateConfig,
        },
        interaction::InteractionController,
        look::LookController,
        minecraft::client::MinecraftClient,
        movement::MovementService,
        navigation::BlockNavigationService,
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

    fn interaction() -> InteractionController {
        let search = BlockSearchService::new(32, 20, 32);
        let navigation = BlockNavigationService::new(Default::default(), search.clone());
        InteractionController::new(InteractionConfig::default(), search, navigation)
    }

    fn combat() -> CombatController {
        CombatController::new()
    }

    #[tokio::test]
    async fn starts_idle() {
        let survival = SurvivalController::new(SurvivalConfig::default());
        assert_eq!(survival.state().await, SurvivalState::Idle);
    }

    #[tokio::test]
    async fn ticking_without_a_connection_does_not_panic_and_stays_idle() {
        // No live world state (never connected) -- `joined_world()` is
        // false, so every branch below `tick_idle`'s entry guard must be
        // unreachable, and nothing here should touch the network or panic.
        let survival = SurvivalController::new(SurvivalConfig::default());
        survival
            .tick(
                &minecraft(),
                &movement(),
                &look(),
                &interaction(),
                &combat(),
            )
            .await;
        assert_eq!(survival.state().await, SurvivalState::Idle);
    }

    #[tokio::test]
    async fn disabled_config_short_circuits_before_touching_world_state() {
        let survival = SurvivalController::new(SurvivalConfig {
            water_mlg_enabled: false,
            ..SurvivalConfig::default()
        });
        survival
            .tick(
                &minecraft(),
                &movement(),
                &look(),
                &interaction(),
                &combat(),
            )
            .await;
        assert_eq!(survival.state().await, SurvivalState::Idle);
    }
}
