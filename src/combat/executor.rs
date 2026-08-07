//! Per-tick async orchestration for `#kill`: reads live world/target state,
//! runs it through the pure decisions in `crate::combat::targeting`/
//! `movement`/`crits`/`shield_break`/`defense`/`health`/`heal`, and issues
//! the resulting client calls (raw walk/sprint/jump input, attacks, weapon
//! switches, aim, eating, blocking). `crate::combat::kill::KillController`
//! is the only caller -- this module holds the mutable per-fight state
//! ([`Inner`]) and the logic that mutates it, kept separate from `kill.rs`'s
//! public start/cancel/snapshot API purely to keep either half individually
//! smaller.
//!
//! `#kill` fights in **full aggression**: there is no disengage branch
//! anywhere in [`tick`]. The bot never retreats, never flees to heal, and
//! never ends a fight because it is hurt -- it closes the gap, stays in the
//! target's face, and keeps swinging until the target is dead (or leaves).
//! Low health changes exactly two things: it eats *while still chasing and
//! hitting* (see [`apply_eating`]), and it blocks more readily (see
//! `crate::combat::defense`). The one thing that does interrupt attacking is
//! a bite in progress, and only because vanilla cancels item use the moment
//! you swing or sprint -- so an eat that is never allowed to finish would
//! heal nothing at all.
//!
//! # Who is driving the bot
//!
//! Two systems, never both at once, split by distance:
//!
//! - **Beyond `engage_range`** the project's normal pathfinder has it
//!   ([`approach_target`]). Getting to a fight fifty blocks away is a
//!   navigation problem -- terrain, water, doors, cliffs -- and that is
//!   exactly what `crate::movement`/`crate::pathfinding` are for.
//! - **Inside it** `crate::combat::movement`'s continuous steering
//!   controller has it. There is nothing to path around at two blocks, and
//!   a route recomputed to block centres is the opposite of what a fight
//!   needs.
//!
//! The handover stops whichever system is losing control before the other
//! starts, so they can never write conflicting movement input in the same
//! tick, and it is sticky in both directions so a target loitering on the
//! boundary cannot flip the bot back and forth.
//!
//! Terrain awareness in the close-range half is deliberately *local* --
//! [`probe_hazards`] samples a few blocks around the bot each tick and hands
//! the combat controller a push-away vector, nothing more. It never invokes
//! a search for a combat adjustment.

use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::{
    combat::{
        crits, defense, heal,
        health::{self, CombatMode},
        movement::{
            self, CombatMovementController, LocalHazards, MovementCommand, MovementSnapshot,
            MovementTuning, Vec2,
        },
        shield_break, state,
        targeting::{self, PREDICTION_LEAD_SECONDS},
    },
    config::{KillbotConfig, ToolRankingMode},
    equipment::{manager::swap_into_slot, model::HOTBAR_PROTOCOL_SLOTS, tools},
    interaction::tool_selection::{ToolCategory, category},
    logging,
    look::{LookController, LookTarget},
    minecraft::{client::MinecraftClient, world_state::MovementStatus},
    movement::{MovementService, NavigationMode},
};

/// How far the target may drift before the approach re-issues its
/// pathfinding goal. Re-submitting every tick would restart Azalea's path
/// computation before it ever finishes -- the same tolerance
/// `mobs::combat` uses when chasing a mob, for the same reason.
const APPROACH_GOAL_DRIFT: f64 = 2.0;

/// Reserved hotbar slot (0-indexed, the 9th/last slot) `#kill` swaps its
/// weapon into when the best sword/axe isn't already somewhere in the
/// hotbar. Deliberately not one of `HotbarSlotsConfig`'s default slots
/// (1-6, the first six 1-indexed slots) so this never fights the
/// always-running automatic hotbar equipment system over the same slot.
const COMBAT_WEAPON_HOTBAR_INDEX: u8 = 8;
const COMBAT_WEAPON_PROTOCOL_SLOT: usize = 44;
/// Reserved hotbar slot for food -- distinct from
/// `COMBAT_WEAPON_HOTBAR_INDEX` so eating never displaces the weapon slot
/// (or vice versa) mid-fight.
const COMBAT_FOOD_HOTBAR_INDEX: u8 = 7;
const COMBAT_FOOD_PROTOCOL_SLOT: usize = 43;

/// Stand-in for a player's maximum health when the real value isn't
/// observable. Max health can be modified by attributes/effects, but this
/// codebase has no visibility into that for a *remote* player (only their
/// current health -- see `MinecraftClient::player_combat_status`), and 20 is
/// both vanilla's default and the scale every threshold here is written in.
const ASSUMED_MAX_HEALTH: f32 = 20.0;

/// How a finished bite is actually detected: the eaten stack shrinks by
/// one. Vanilla food takes 32 ticks (1.6s) to consume, but waiting a fixed
/// 1.8s per bite -- what this used to do -- threw away ~0.2s every single
/// time, which is most of a second across the three or four golden apples a
/// real fight takes. The inventory already reports the count on the same
/// world snapshot this module reads every tick, so the bite is released the
/// moment it genuinely completes and the next one starts immediately.
///
/// [`EAT_FALLBACK_WINDOW`] is the backstop for when that signal never
/// arrives: an interrupted bite, a dropped packet, or a server that doesn't
/// update the slot. Deliberately the same 1.8s this module used to wait
/// unconditionally, so the worst case is exactly the old behavior and the
/// normal case is ~0.2s per bite faster -- a backstop that made a broken
/// signal *slower* than before would be a poor trade for the gain.
const EAT_FALLBACK_WINDOW: Duration = Duration::from_millis(1800);

/// How long to wait for the server to be told about the food swap before
/// giving up on that attempt and starting over.
///
/// Generous next to the one game tick it normally takes: the carried-item
/// packet can be delayed by a contended inventory lock or a slow tick, and
/// abandoning a swap that was about to land would only restart the same
/// race. See [`apply_eating`] for what this is guarding against.
const EQUIP_CONFIRM_TIMEOUT: Duration = Duration::from_millis(500);

/// A food item selected into the hotbar whose slot change hasn't reached
/// the server yet -- see [`apply_eating`].
#[derive(Clone, Debug)]
struct PendingFood {
    hotbar_index: u8,
    item_id: String,
    since: Instant,
}

pub(crate) struct Inner {
    pub(crate) snapshot: state::KillSnapshot,
    config: KillbotConfig,
    /// Whether the project's normal pathfinder is currently driving the
    /// approach, rather than the combat movement controller. See
    /// `movement::pathfinder_should_drive`.
    approaching: bool,
    /// Where the approach's last pathfinding goal was aimed, so it is only
    /// re-issued once the target has actually moved.
    approach_goal: Option<crate::minecraft::world_state::PositionSnapshot>,
    /// The combat movement controller -- see `crate::combat::movement`.
    /// Owns every piece of state that used to live loose in here as strafe
    /// timers: orbit pattern, momentum, and target motion history.
    movement: CombatMovementController,
    rng: crate::look::aim_point::SeededRng,
    last_attack: Option<Instant>,
    last_jump: Option<Instant>,
    sprint_released_at: Option<Instant>,
    engaged_logged: bool,
    look_target_set_for: Option<String>,
    /// Last tick the target was actually found in world state -- see
    /// [`handle_unresolved_target`]. `Some` from the moment a fight starts
    /// (resolving the player in `kill::KillController::start` counts as a
    /// sighting), never `None` again until the next fight.
    last_seen_at: Option<Instant>,
    /// Set the instant `#kill` last started an eat action, and left set
    /// until the bite is observed to have completed (or [`EAT_FALLBACK_WINDOW`]
    /// expires). See [`apply_eating`].
    eating_since: Option<Instant>,
    /// The item id of the bite in flight, and how many of it the bot held
    /// when the bite started -- together, the "this bite is done" signal:
    /// the count dropping by one is the consume actually landing.
    eating_item: Option<String>,
    eating_count: u32,
    /// Food selected into the hotbar, waiting for the server to be told
    /// about it before the bite is started. See [`apply_eating`].
    pending_food: Option<PendingFood>,
    /// When the last bite finished. Enforces `eat_cooldown_ms` -- one bite,
    /// then back to fighting while it takes effect. See [`apply_eating`].
    last_eat_finished: Option<Instant>,
    /// Whether the bot decided to eat this trip below the health threshold
    /// -- only so the "eating mid-fight" line logs once rather than every
    /// tick.
    topping_up: bool,
    /// Same, for the finisher line: the target has dropped to
    /// `finisher_health` and the bot is ignoring its own.
    finishing_logged: bool,
    /// Whether the bot's own shield was raised as of the previous tick --
    /// same one-shot-transition purpose as `was_healing`, for
    /// `crate::combat::defense`.
    shield_raised: bool,
    /// Recharge time of the weapon the bot is actually holding, refreshed
    /// every tick by [`apply_weapon_selection`]. `None` until the first
    /// successful equipment read. Only consulted when the user hasn't set an
    /// explicit `attack_cooldown_ms` -- see [`attack_cooldown`].
    held_weapon_cooldown: Option<Duration>,
    /// When the bot first held back a ready swing waiting to become a crit.
    /// Cleared the moment it swings. Bounds the hold so a bot that cannot
    /// leave the ground still attacks -- see
    /// `crits::CRIT_HOLD_TIMEOUT`.
    crit_hold_since: Option<Instant>,
    /// Whether the target appeared to be blocking as of the previous tick
    /// -- so "Shield broken" logs exactly once, on the tick blocking is
    /// first observed to have stopped, not every tick afterward.
    target_was_blocking: bool,
}

impl Inner {
    pub(crate) fn new(seed: u64, config: KillbotConfig) -> Self {
        Self {
            snapshot: state::KillSnapshot::default(),
            config,
            approaching: false,
            approach_goal: None,
            movement: CombatMovementController::new(),
            rng: crate::look::aim_point::SeededRng::new(seed),
            last_attack: None,
            last_jump: None,
            sprint_released_at: None,
            engaged_logged: false,
            look_target_set_for: None,
            last_seen_at: None,
            eating_since: None,
            eating_item: None,
            eating_count: 0,
            pending_food: None,
            last_eat_finished: None,
            topping_up: false,
            finishing_logged: false,
            shield_raised: false,
            held_weapon_cooldown: None,
            crit_hold_since: None,
            target_was_blocking: false,
        }
    }

    /// Resets every per-fight timer/flag, but not the snapshot itself --
    /// called by `kill::KillController::start` right before overwriting
    /// the snapshot, so a second `#kill` never inherits stale strafe
    /// timers, attack cooldowns, or "already logged" flags from a
    /// previous fight.
    pub(crate) fn reset_for_new_fight(&mut self) {
        self.movement.reset(&mut self.rng);
        self.approaching = false;
        self.approach_goal = None;
        self.last_attack = None;
        self.last_jump = None;
        self.sprint_released_at = None;
        self.engaged_logged = false;
        self.look_target_set_for = None;
        self.eating_since = None;
        self.eating_item = None;
        self.eating_count = 0;
        self.pending_food = None;
        self.last_eat_finished = None;
        self.topping_up = false;
        self.finishing_logged = false;
        self.shield_raised = false;
        self.held_weapon_cooldown = None;
        self.crit_hold_since = None;
        self.target_was_blocking = false;
        // `start()` only reaches this point after already confirming the
        // player is resolvable right now -- that lookup itself counts as
        // the first sighting.
        self.last_seen_at = Some(Instant::now());
    }
}

pub(crate) async fn tick(
    minecraft: &MinecraftClient,
    movement: &MovementService,
    look: &LookController,
    inner: &mut Inner,
) {
    if inner.snapshot.state != state::KillState::Running {
        return;
    }
    let Some(name) = inner.snapshot.target_name.clone() else {
        return;
    };
    let world = minecraft.world_state_snapshot().await;
    if world.bot.alive == Some(false) || !world.joined_world() {
        return;
    }
    let Some(bot_position) = world.bot.position else {
        return;
    };

    // A player entirely absent from `world.players` has left the tab
    // list -- genuinely disconnected, not just out of render distance
    // (see `WorldState::remove_player`, driven by Azalea's own
    // `Event::RemovePlayer`) -- so this ends the fight immediately, no
    // staleness grace period. A player still present but without a live
    // `position` (in the tab list, not currently loaded nearby) instead
    // goes through the same "temporarily missing" reacquisition path as a
    // failed live-state query below.
    let Some(player) = world.find_player_by_name(&name) else {
        disconnect(minecraft, movement, look, inner, &name).await;
        return;
    };
    let Some(target_position) = player.position else {
        handle_unresolved_target(minecraft, movement, look, inner, &name).await;
        return;
    };
    let target_uuid = player.uuid;

    let Ok(status) = minecraft.player_combat_status(target_uuid).await else {
        handle_unresolved_target(minecraft, movement, look, inner, &name).await;
        return;
    };
    if !status.alive {
        eliminate(minecraft, movement, look, inner, &name).await;
        return;
    }

    inner.last_seen_at = Some(Instant::now());
    inner.snapshot.target_position = Some(target_position);
    inner.snapshot.shield_detected = status.using_item;
    if !inner.engaged_logged {
        inner.engaged_logged = true;
        inner.snapshot.phase = state::CombatPhase::Engage;
        logging::milestone("Engaging target");
    }

    let velocity = minecraft
        .player_velocity(target_uuid)
        .await
        .unwrap_or([0.0, 0.0, 0.0]);
    let lead_seconds = if inner.config.prediction_enabled {
        PREDICTION_LEAD_SECONDS
    } else {
        0.0
    };
    let predicted = targeting::predicted_position(target_position, velocity, lead_seconds);
    let distance = targeting::distance(bot_position, predicted);

    if distance > inner.config.max_chase_distance {
        abort(
            minecraft,
            movement,
            look,
            inner,
            &name,
            "target out of chase range",
        )
        .await;
        return;
    }

    let bot_health = f64::from(world.bot.health.unwrap_or(ASSUMED_MAX_HEALTH));
    let mode = health::mode_for_health(bot_health, inner.config.heal_threshold);
    inner.snapshot.mode = mode;

    // Full aggression: the aim never leaves the target and the feet never
    // walk away from it, whatever the health situation is.
    track_aim(minecraft, look, inner, &name).await;

    // Finisher: a target this close to death dies to the next couple of
    // hits, so the bot stops caring about its own health entirely. Stopping
    // to eat here is how a won fight turns into a lost one -- it hands the
    // target 1.6 seconds of free hits and a chance to heal themselves.
    let finishing = inner.config.finisher_health > 0.0
        && status
            .health
            .is_some_and(|health| f64::from(health) <= inner.config.finisher_health);
    if finishing && !inner.finishing_logged {
        inner.finishing_logged = true;
        logging::progress("Target low: going for the kill");
    } else if !finishing {
        inner.finishing_logged = false;
    }

    let wants_food = !finishing && health::wants_food(bot_health, inner.config.heal_threshold);
    if wants_food && !inner.topping_up {
        inner.topping_up = true;
        logging::milestone("Health low: eating mid-fight");
    } else if !wants_food && inner.topping_up {
        inner.topping_up = false;
    }
    // The one thing that suspends attacking, sprinting, and weapon swaps --
    // each of those cancels a vanilla bite, so allowing them here would mean
    // never actually healing. Everything else (chasing, strafing, jumping
    // obstacles, tracking the target) keeps running underneath it.
    let eating = apply_eating(minecraft, inner, &world.inventory, wants_food, finishing).await;

    // Long distance is the pathfinder's problem, close range is the combat
    // controller's -- see `movement::pathfinder_should_drive`. Only one of
    // them ever holds the controls, and handing over stops the other, so
    // they can never fight for the same input.
    let approaching = crate::combat::movement::pathfinder_should_drive(
        distance,
        inner.config.engage_range,
        inner.approaching,
    );
    if approaching {
        if !inner.approaching {
            inner.approaching = true;
            inner.approach_goal = None;
            // Release the raw combat input before the pathfinder takes over.
            let _ = minecraft
                .set_combat_walk(crate::combat::movement::CombatWalk::None, false)
                .await;
            debug_transition(inner, "Closing distance");
        }
        approach_target(minecraft, movement, inner, target_position).await;
    } else {
        if inner.approaching {
            inner.approaching = false;
            inner.approach_goal = None;
            // Give the pathfinder back before driving raw input, or the two
            // will overwrite each other's movement every tick.
            let _ = movement.stop(minecraft).await;
            inner.movement.reset(&mut inner.rng);
            debug_transition(inner, "Engaging in melee");
        }
        apply_movement(
            minecraft,
            inner,
            bot_position,
            target_position,
            world.bot.yaw.unwrap_or_default(),
            world.bot.on_ground.unwrap_or(true),
            world.bot.horizontal_collision.unwrap_or(false),
            !eating,
        )
        .await;
    }
    if !eating {
        apply_weapon_selection(
            minecraft,
            inner,
            world.bot.selected_hotbar_slot,
            inner.config.shield_break_enabled && status.using_item,
        )
        .await;
        let within_attack_range = distance <= inner.config.attack_range;
        apply_attack(
            minecraft,
            inner,
            target_uuid,
            world.bot.on_ground.unwrap_or(true),
            world.bot.velocity_y.unwrap_or(0.0),
            within_attack_range,
        )
        .await;
    }
    if eating {
        inner.snapshot.phase = state::CombatPhase::Heal;
    } else if inner.topping_up {
        // Hurt and after a bite, but not mid-bite right now (nothing edible
        // in the inventory, or between two bites). Purely a report of the
        // posture -- the bot is still chasing and swinging this tick, it
        // just also blocks whenever `defense` lets it.
        inner.snapshot.phase = state::CombatPhase::Defensive;
    } else if !matches!(inner.snapshot.phase, state::CombatPhase::Engage) {
        inner.snapshot.phase = compute_active_phase(distance);
    }

    // Skipped entirely mid-bite: raising the shield is another item use, and
    // starting or releasing one cancels the eat.
    if inner.config.shield_use_enabled && !eating {
        let approaching = targeting::is_approaching(bot_position, target_position, velocity);
        apply_defense(minecraft, inner, mode, distance, approaching).await;
    }
}

/// Non-attack phase, computed after movement/attack for a tick that isn't
/// the very first "Engage" tick or currently healing -- see
/// [`state::CombatPhase`]'s doc comment for why this is derived rather
/// than enforced through strict transitions.
fn compute_active_phase(distance: f64) -> state::CombatPhase {
    if distance > movement::AGGRESSIVE_CLOSE_DISTANCE {
        state::CombatPhase::Chase
    } else if distance < movement::BACK_OFF_DISTANCE {
        state::CombatPhase::Reposition
    } else {
        state::CombatPhase::Strafe
    }
}

/// The target has left the server entirely -- ends the fight immediately,
/// per the spec's "if the target disconnects: cancel combat immediately"
/// (distinct from [`handle_unresolved_target`]'s grace period for a merely
/// temporarily-unloaded target).
async fn disconnect(
    minecraft: &MinecraftClient,
    movement: &MovementService,
    look: &LookController,
    inner: &mut Inner,
    name: &str,
) {
    stop_all(minecraft, movement, look).await;
    inner.snapshot.state = state::KillState::Failed;
    inner.snapshot.phase = state::CombatPhase::Abort;
    inner.snapshot.failure_reason = Some(format!("Target lost: {name} (disconnected)"));
}

/// A tick that found the target in the tab list but couldn't resolve a
/// live position or entity state for them right now (out of render
/// distance, chunk not loaded from this side, a momentary desync): stops
/// raw movement input (chasing a stale predicted position is worse than
/// standing still) and drops the look controller's own target so it gets
/// resubmitted fresh -- see `track_aim`'s doc comment -- the moment the
/// target reappears, "attempting reacquisition" without the caller needing
/// to do anything. Only actually gives up (`lose_target`) once nothing has
/// been seen for `targeting::STALE_OBSERVATION_SECONDS`, so a momentary
/// tracking gap doesn't end the fight.
async fn handle_unresolved_target(
    minecraft: &MinecraftClient,
    movement: &MovementService,
    look: &LookController,
    inner: &mut Inner,
    name: &str,
) {
    let stale = inner
        .last_seen_at
        .is_none_or(|last_seen| targeting::is_stale(last_seen.elapsed().as_secs_f64()));
    if stale {
        lose_target(minecraft, movement, look, inner, name).await;
        return;
    }
    let _ = minecraft
        .set_combat_walk(movement::CombatWalk::None, false)
        .await;
    inner.look_target_set_for = None;
}

/// Points the look controller at the target exactly once per fight --
/// `LookTarget::PredictedPlayer` tracks the live position (and leads it)
/// continuously on its own afterward (see that variant's doc comment), so
/// resubmitting it every tick would only reset the look controller's own
/// smoothing and cause visible jitter instead of a smooth track.
async fn track_aim(
    minecraft: &MinecraftClient,
    look: &LookController,
    inner: &mut Inner,
    name: &str,
) {
    if inner.look_target_set_for.as_deref() == Some(name) {
        return;
    }
    if look
        .look_at(minecraft, LookTarget::PredictedPlayer(name.to_owned()))
        .await
        .is_ok()
    {
        inner.look_target_set_for = Some(name.to_owned());
    }
}

/// `allow_sprint` is false only while a bite is in progress -- vanilla
/// cancels item use the instant you start sprinting, so the bot closes the
/// gap at a walk for those ~1.8 seconds rather than losing the food.
/// One tick of combat movement: builds the controller's view of the fight,
/// runs it, and dispatches the resulting key presses.
///
/// `allow_sprint` is false while eating -- sprinting cancels a vanilla bite
/// -- and is the one input the controller doesn't decide for itself.
#[expect(
    clippy::too_many_arguments,
    reason = "one call site, and each argument is an independent reading of               the same tick; bundling them would only rebuild the               `MovementSnapshot` this already assembles"
)]
async fn apply_movement(
    minecraft: &MinecraftClient,
    inner: &mut Inner,
    bot_position: crate::minecraft::world_state::PositionSnapshot,
    target_position: crate::minecraft::world_state::PositionSnapshot,
    camera_yaw: f32,
    on_ground: bool,
    horizontal_collision: bool,
    allow_sprint: bool,
) {
    let hazards = probe_hazards(minecraft, inner, bot_position, target_position).await;
    let tuning = movement_tuning(inner);
    let snapshot = MovementSnapshot {
        bot: Vec2::of(bot_position),
        target: Vec2::of(target_position),
        on_ground,
        horizontal_collision,
        camera_yaw,
        hazards,
        now: Instant::now(),
    };
    let Inner {
        movement: controller,
        rng,
        ..
    } = inner;
    let MovementCommand {
        walk,
        sprint,
        jump,
        sneak,
    } = controller.update(snapshot, &tuning, rng);

    let _ = minecraft
        .set_combat_walk(walk, sprint && allow_sprint)
        .await;
    if jump {
        let _ = minecraft.combat_jump_once().await;
    }
    let _ = minecraft.set_sneaking(sneak).await;
}

/// Walks toward a distant target with the project's normal pathfinding,
/// exactly as `/goto` would -- terrain, water and cliffs included.
///
/// Uses `goto_for_block_navigation` rather than `goto` so the movement layer
/// stays quiet: this is one leg of a fight, not a user-issued trip, and it
/// would otherwise print a "Going to" line every time the target moved two
/// blocks.
async fn approach_target(
    minecraft: &MinecraftClient,
    movement: &MovementService,
    inner: &mut Inner,
    target_position: crate::minecraft::world_state::PositionSnapshot,
) {
    let snapshot = movement.snapshot().await;
    let drifted = inner.approach_goal.is_none_or(|previous| {
        targeting::distance(previous, target_position) > APPROACH_GOAL_DRIFT
    });
    // Also re-issue if the movement layer has finished or given up on the
    // last goal: a chase is not over just because one leg of it completed.
    let idle = snapshot.status != MovementStatus::MovingToPosition;
    if !drifted && !idle {
        return;
    }
    if movement
        .goto_for_block_navigation(minecraft, target_position, NavigationMode::AllowMining)
        .await
        .is_ok()
    {
        inner.approach_goal = Some(target_position);
    }
}

fn debug_transition(inner: &Inner, message: &str) {
    if inner.snapshot.state == state::KillState::Running {
        logging::info(message);
    }
}

/// Half-extent of the terrain sample taken around the bot each tick, in
/// blocks. Small on purpose: this is local obstacle avoidance for a fight,
/// not pathfinding, and the whole point is that it never invokes the
/// pathfinder for a combat adjustment.
const HAZARD_PROBE_RADIUS: i32 = 3;
/// Vertical extent of that sample: enough to see a step up, a head-height
/// obstruction, and a drop worth not walking off.
const HAZARD_PROBE_HEIGHT: i32 = 3;

/// Looks at the blocks immediately around the bot and turns them into the
/// push-away vector and stop flags the movement controller consumes.
///
/// Reuses `crate::pathfinding::terrain`'s classification -- solid, lava,
/// hazard, climbable -- so combat and navigation agree on what a dangerous
/// block is, but shares none of its search machinery: this is one small
/// sample and a handful of comparisons per tick, with no A*, no cache, and
/// no allocation beyond the sample itself.
///
/// Returns "clear" whenever the terrain can't be read. Combat movement has
/// always been terrain-blind, so failing open leaves the bot exactly as it
/// was rather than freezing it.
async fn probe_hazards(
    minecraft: &MinecraftClient,
    inner: &Inner,
    bot_position: crate::minecraft::world_state::PositionSnapshot,
    target_position: crate::minecraft::world_state::PositionSnapshot,
) -> LocalHazards {
    use crate::pathfinding::grid::GridBounds;

    let feet = bot_position.block();
    let bounds = GridBounds {
        min: crate::minecraft::world_state::BlockPosition {
            x: feet.x - HAZARD_PROBE_RADIUS,
            y: feet.y - HAZARD_PROBE_HEIGHT,
            z: feet.z - HAZARD_PROBE_RADIUS,
        },
        max: crate::minecraft::world_state::BlockPosition {
            x: feet.x + HAZARD_PROBE_RADIUS + 1,
            y: feet.y + HAZARD_PROBE_HEIGHT + 1,
            z: feet.z + HAZARD_PROBE_RADIUS + 1,
        },
    };
    let Ok(grid) = minecraft.sample_terrain(bounds).await else {
        return LocalHazards::default();
    };
    if grid.known_cells() == 0 {
        return LocalHazards::default();
    }
    crate::combat::terrain_probe::evaluate(
        &grid,
        feet,
        Vec2::of(bot_position),
        Vec2::of(target_position),
        inner.movement.heading(),
    )
}

/// Translates the user's `[killbot]` settings into the movement
/// controller's own tuning.
fn movement_tuning(inner: &Inner) -> MovementTuning {
    let defaults = MovementTuning::default();
    MovementTuning {
        band: movement::DistanceBand {
            preferred_min: inner.config.preferred_range.min(inner.config.attack_range),
            preferred_max: inner
                .config
                .preferred_range
                .max(defaults.band.preferred_max)
                .min(inner.config.attack_range),
            ..defaults.band
        },
        lead_seconds: if inner.config.prediction_enabled {
            defaults.lead_seconds
        } else {
            0.0
        },
        strafe_enabled: inner.config.strafe_enabled,
        sprint_reset_enabled: inner.config.sprint_reset_enabled,
        ..defaults
    }
}

/// Eating, without ever leaving the fight: no distance requirement, no
/// retreat, no waiting for an opening -- if health is below the threshold
/// and there is food anywhere in the inventory, the bot puts it in its hand
/// and bites while it is still walking into and circling the target.
///
/// Returns whether the main hand is busy with food right now -- either
/// swapping to it or mid-bite -- which is what makes the caller hold off on
/// swinging, sprinting, and weapon swaps for the duration. Each of those
/// cancels vanilla item use, and a weapon swap in particular would take the
/// food straight back out of the bot's hand.
///
/// # Why eating takes two ticks
///
/// Selecting the food and starting to use it in the same tick does not work,
/// and fails in a way that looks exactly like the bot standing there holding
/// an apple: Azalea sends the use-item packet *before* the carried-item
/// packet within a tick (see
/// `MinecraftClient::acknowledged_hotbar_slot`), so the server applies the
/// use to whatever was held before -- the sword -- and only then switches to
/// the apple. Nothing is eaten, and the bot waits out
/// [`EAT_FALLBACK_WINDOW`] holding food before trying again.
///
/// So the swap is confirmed first: select the slot, wait until the server
/// has actually been told about it, and only then send the use. It costs one
/// tick and makes every bite land.
///
/// A bite ends the moment the eaten stack shrinks, so consecutive apples
/// would chain with no dead time -- `KillbotConfig::eat_cooldown_ms` is what
/// deliberately spaces them out instead.
///
/// Returns false (and so gives up nothing) whenever there is no food to eat,
/// or whenever the target is close enough to death that `finishing` is set --
/// the fight simply carries on at whatever health the bot has.
async fn apply_eating(
    minecraft: &MinecraftClient,
    inner: &mut Inner,
    inventory: &crate::minecraft::world_state::InventorySnapshot,
    wants_food: bool,
    finishing: bool,
) -> bool {
    // The target dropped into finisher range mid-bite: abandon it rather
    // than chew for another second while they die or get away. Nothing is
    // lost -- an interrupted vanilla consume doesn't eat the item -- and the
    // weapon swap on the very next tick is what cancels it server-side.
    if finishing {
        inner.eating_since = None;
        inner.eating_item = None;
        inner.pending_food = None;
        return false;
    }
    if let Some(since) = inner.eating_since {
        let consumed = inner
            .eating_item
            .as_deref()
            .is_some_and(|item_id| inventory.count_item(item_id) < inner.eating_count);
        if !consumed && since.elapsed() < EAT_FALLBACK_WINDOW {
            return true;
        }
        inner.eating_since = None;
        inner.eating_item = None;
        inner.last_eat_finished = Some(Instant::now());
    }

    // A swap is in flight: start the bite as soon as the server knows the
    // food is in hand, and keep reporting "busy" until then so nothing
    // swaps the weapon back in underneath it.
    if let Some(pending) = inner.pending_food.clone() {
        let acknowledged = minecraft.acknowledged_hotbar_slot().await.ok().flatten();
        if acknowledged == Some(pending.hotbar_index) {
            inner.pending_food = None;
            if minecraft.start_use_main_hand().await.is_ok() {
                inner.eating_since = Some(Instant::now());
                inner.eating_count = inventory.count_item(&pending.item_id);
                logging::progress(format!(
                    "Eating {}",
                    crate::blocks::bare_id(&pending.item_id)
                ));
                inner.eating_item = Some(pending.item_id);
                return true;
            }
            // The use failed to dispatch; fall through and let the next tick
            // start over rather than pretending a bite is in flight.
            return false;
        }
        if pending.since.elapsed() >= EQUIP_CONFIRM_TIMEOUT {
            inner.pending_food = None;
            return false;
        }
        return true;
    }
    // One bite, then straight back to the fight. A golden apple heals over
    // five seconds, so the tick right after swallowing still reports the old
    // health -- without this the bot would immediately decide it is still
    // hurt and start another, standing there chewing through its whole stack
    // instead of swinging. See `KillbotConfig::eat_cooldown_ms`.
    if inner
        .last_eat_finished
        .is_some_and(|finished| finished.elapsed() < inner.config.eat_cooldown())
    {
        return false;
    }
    if !wants_food {
        return false;
    }
    let Ok(food) = minecraft.food_snapshot().await else {
        return false;
    };
    let candidates: Vec<heal::FoodOption<'_>> = food
        .iter()
        .map(|item| heal::FoodOption {
            slot: item.slot,
            item_id: &item.item_id,
            nutrition: item.nutrition,
        })
        .collect();
    let Some(best) = heal::best_food(&candidates) else {
        // Nothing edible -- keep fighting exactly as before and let natural
        // regeneration do whatever it does.
        return false;
    };
    // A raised shield is an off-hand item use; starting a main-hand one on
    // top of it is what cancels the bite, so lower it first.
    if inner.shield_raised {
        let _ = minecraft.release_use_item().await;
        inner.shield_raised = false;
    }
    let label = best.item_id.to_owned();
    let hotbar_index = if HOTBAR_PROTOCOL_SLOTS.contains(&best.slot) {
        Some((best.slot - HOTBAR_PROTOCOL_SLOTS.start()) as u8)
    } else if swap_into_slot(minecraft, best.slot, COMBAT_FOOD_PROTOCOL_SLOT).await {
        Some(COMBAT_FOOD_HOTBAR_INDEX)
    } else {
        None
    };
    let Some(hotbar_index) = hotbar_index else {
        return false;
    };
    // Always sent, even when this slot already looks selected: what matters
    // is what the *server* has been told, and the local view of that runs
    // ahead of the packet. Re-sending is harmless here -- nothing is being
    // used at this point, so there is no item use for it to cancel.
    if minecraft.select_hotbar_slot(hotbar_index).await.is_err() {
        return false;
    }
    inner.pending_food = Some(PendingFood {
        hotbar_index,
        item_id: label,
        since: Instant::now(),
    });
    // Busy from this tick on: the food is on its way into the hand, and a
    // weapon swap now would undo it.
    true
}

/// Keeps a weapon -- and the right one -- in the bot's hand every tick it
/// isn't eating.
///
/// Switches to the best available axe while the target appears to be
/// blocking, and back to the best sword the instant they stop (see
/// `crate::combat::shield_break`'s doc comment for the "appears to be"
/// caveat) -- and independently, regardless of blocking, forces a
/// re-evaluation whenever the currently-held weapon's durability is
/// critical, per the spec's "if durability becomes critical, switch
/// weapons automatically". Falls back to an axe as the general-purpose
/// weapon when no sword is held at all (`shield_break::desired_weapon_category`).
/// Re-evaluated fresh every tick from the live inventory/selected slot
/// rather than trusting a locally-cached "currently wielding an axe" flag,
/// so a manual inventory change or a failed equip attempt self-corrects on
/// the very next tick instead of leaving `#kill` stuck believing it holds
/// something it doesn't.
///
/// This runs on every non-eating tick, not only when `shield_break_enabled`
/// (the caller passes `target_blocking: false` when that is off, reducing
/// this to plain "hold the best sword"): eating mid-fight leaves *food* in
/// the main hand, and that same fresh-from-inventory re-evaluation is what
/// puts the weapon back the tick the bite finishes. Gating the whole
/// function off would leave the bot punching with a pork chop.
async fn apply_weapon_selection(
    minecraft: &MinecraftClient,
    inner: &mut Inner,
    selected_hotbar_slot: Option<u8>,
    target_blocking: bool,
) {
    // Best-effort "the target's shield is no longer up" transition -- see
    // `crate::combat::shield_break`'s doc comment for the same "appears to
    // be blocking" caveat; this can't distinguish an axe hit actually
    // disabling the shield from the target simply lowering it voluntarily,
    // but either way the threat this bot switched weapons for is gone.
    if inner.target_was_blocking && !target_blocking {
        logging::progress("Shield broken");
    }
    inner.target_was_blocking = target_blocking;

    let Ok(equipment) = minecraft.equipment_snapshot().await else {
        return;
    };
    let current =
        selected_hotbar_slot.and_then(|slot| currently_wielded(&equipment.inventory, slot));
    // Refreshed from the live selection rather than assumed from whatever
    // this function is about to equip: the swap can fail, and swinging on
    // the clock of a weapon the bot isn't holding is exactly the
    // partial-charge damage loss the automatic cadence exists to avoid.
    inner.held_weapon_cooldown = Some(crits::weapon_cooldown(
        current.map(|item| item.item_id.as_str()),
    ));
    let sword_available = tools::best_candidate(
        ToolRankingMode::Score,
        ToolCategory::Sword,
        &equipment.inventory,
    )
    .is_some();
    let wanted = shield_break::desired_weapon_category(target_blocking, sword_available);
    let durability_critical = current.is_some_and(|item| {
        shield_break::is_durability_critical(item.current_durability, item.max_durability)
    });
    let category_matches = current.is_some_and(|item| category(&item.item_id) == Some(wanted));
    if category_matches && !durability_critical {
        return;
    }
    if equip_weapon_category(minecraft, &equipment.inventory, wanted).await
        && wanted == ToolCategory::Axe
        && target_blocking
    {
        inner.snapshot.phase = state::CombatPhase::ShieldBreak;
        logging::milestone("Shield detected");
        logging::progress("Switching to axe");
    }
}

fn currently_wielded(
    inventory: &[crate::equipment::model::EquipmentItem],
    selected_hotbar_slot: u8,
) -> Option<&crate::equipment::model::EquipmentItem> {
    let slot = HOTBAR_PROTOCOL_SLOTS.start() + usize::from(selected_hotbar_slot);
    inventory.iter().find(|item| item.slot == slot)
}

async fn equip_weapon_category(
    minecraft: &MinecraftClient,
    inventory: &[crate::equipment::model::EquipmentItem],
    wanted: ToolCategory,
) -> bool {
    let Some(candidate) = tools::best_candidate(ToolRankingMode::Score, wanted, inventory) else {
        return false;
    };
    let source_slot = candidate.item.slot;
    if HOTBAR_PROTOCOL_SLOTS.contains(&source_slot) {
        let hotbar_index = (source_slot - HOTBAR_PROTOCOL_SLOTS.start()) as u8;
        return minecraft.select_hotbar_slot(hotbar_index).await.is_ok();
    }
    swap_into_slot(minecraft, source_slot, COMBAT_WEAPON_PROTOCOL_SLOT).await
        && minecraft
            .select_hotbar_slot(COMBAT_WEAPON_HOTBAR_INDEX)
            .await
            .is_ok()
}

async fn apply_attack(
    minecraft: &MinecraftClient,
    inner: &mut Inner,
    target_uuid: Uuid,
    on_ground: bool,
    velocity_y: f64,
    within_attack_range: bool,
) {
    if !within_attack_range {
        // Nothing is being held back while the target is out of reach, and a
        // hold left over from before would otherwise read as "already waited
        // too long" the moment they come back into range, costing that
        // hit's critical.
        inner.crit_hold_since = None;
        return;
    }
    let now = Instant::now();
    let cooldown = attack_cooldown(inner);

    // Not ready yet -- but this is exactly when the crit jump belongs, so
    // the bot is already falling the instant the cooldown opens. Jumping
    // *after* the cooldown (what this used to do) added the whole rise of
    // the jump on top of every single hit, so the real hit rate was always
    // slower than the configured cadence.
    if !crits::attack_ready(inner.last_attack, now, cooldown) {
        if inner.config.crit_enabled
            && crits::should_prejump_for_crit(
                on_ground,
                within_attack_range,
                inner.last_attack,
                inner.last_jump,
                now,
                cooldown,
            )
        {
            inner.last_jump = Some(now);
            inner.snapshot.phase = state::CombatPhase::CritPrep;
            let _ = minecraft.combat_jump_once().await;
        }
        return;
    }

    if crits::is_critical_window(on_ground, velocity_y) {
        swing(minecraft, inner, target_uuid, now, true).await;
        return;
    }
    if !on_ground {
        // Still rising (or at the apex) from the crit jump --
        // `is_critical_window` above didn't match, so attacking *right now*
        // would land as a plain, non-critical hit instead. Wait rather than
        // swinging early: `last_attack` is untouched, so the cooldown stays
        // exactly as ready as it is now, and the very next tick that reports
        // falling (`velocity_y < 0`) lands the crit this jump was for.
        //
        // Bounded by the same hold timeout as the grounded case: a jump's
        // rise is a handful of ticks, but knockback, an elytra, or a boat can
        // keep the bot climbing for far longer, and a ready swing must not
        // wait on a crit that isn't coming.
        let holding_since = *inner.crit_hold_since.get_or_insert(now);
        if now.saturating_duration_since(holding_since) < crits::CRIT_HOLD_TIMEOUT {
            return;
        }
    }
    // Ready, on the ground, and no crit set up -- the pre-jump was on its
    // own retry spacing, or the bot only just came into range. With
    // `always_crit`, hold the swing and jump for it rather than taking a
    // flat hit worth two thirds as much. Applies to whatever is in hand: an
    // axe's longer cooldown makes the hold cheaper, not less worthwhile.
    if inner.config.crit_enabled
        && inner.config.always_crit
        && crits::should_force_crit_jump(
            on_ground,
            within_attack_range,
            inner.last_jump,
            now,
            inner.crit_hold_since.map(|since| now - since),
        )
    {
        if inner.crit_hold_since.is_none() {
            inner.crit_hold_since = Some(now);
        }
        inner.last_jump = Some(now);
        inner.snapshot.phase = state::CombatPhase::CritPrep;
        let _ = minecraft.combat_jump_once().await;
        return;
    }
    swing(minecraft, inner, target_uuid, now, false).await;
}

/// The cadence this tick's attack is measured against: the user's explicit
/// `attack_cooldown_ms` when set, otherwise the recharge time of the weapon
/// actually in hand, otherwise a sword's (the fastest real melee weapon, and
/// the one `#kill` equips by default).
fn attack_cooldown(inner: &Inner) -> Duration {
    inner
        .config
        .attack_cooldown()
        .or(inner.held_weapon_cooldown)
        .unwrap_or(crits::SWORD_COOLDOWN)
}

async fn swing(
    minecraft: &MinecraftClient,
    inner: &mut Inner,
    target_uuid: Uuid,
    now: Instant,
    is_crit: bool,
) {
    // Best-effort: nothing productive to do differently if the attack
    // packet itself fails to send (a disconnect mid-swing, the target
    // despawning this exact tick) -- the next tick's liveness check on the
    // target handles either case.
    let _ = minecraft.attack_player(target_uuid).await;
    inner.last_attack = Some(now);
    inner.crit_hold_since = None;
    inner.sprint_released_at = Some(now);
    inner.snapshot.hits_landed += 1;
    inner.snapshot.phase = state::CombatPhase::Attack;
    // Sprint reset and combo pressure both live in the movement controller:
    // it drops sprint for a moment without releasing the keys, and tightens
    // the orbit while the combo is live.
    inner.movement.note_attack(now);
    if is_crit {
        inner.snapshot.crits_landed += 1;
        logging::progress("Critical hit landed");
    }
}

/// Raises/lowers the bot's own shield -- see `crate::combat::defense`'s
/// module doc comment for the timing this approximates and its real
/// limitations. Only actually calls into the client on a state *change*
/// (raising or lowering), not every tick, since re-sending "start use
/// item" every tick while already blocking is unnecessary and re-sending
/// "release" while not blocking is a harmless no-op but still pointless
/// network chatter.
async fn apply_defense(
    minecraft: &MinecraftClient,
    inner: &mut Inner,
    mode: CombatMode,
    distance: f64,
    target_approaching: bool,
) {
    let should_raise = defense::should_raise_shield(mode, distance, target_approaching);
    if should_raise && !inner.shield_raised {
        if minecraft.start_use_off_hand().await.is_ok() {
            inner.shield_raised = true;
        }
    } else if !should_raise && inner.shield_raised {
        let _ = minecraft.release_use_item().await;
        inner.shield_raised = false;
    }
}

/// Sets the terminal `Failed` state but deliberately does *not* log here --
/// mirrors every other controller in this codebase (e.g.
/// `mobs::combat::CombatController::fail`): the controller only records
/// *why*, and the caller awaiting it (`App::await_kill_terminal`) is what
/// prints the user-facing message, using the player name already in its
/// own scope rather than round-tripping it through this snapshot.
async fn lose_target(
    minecraft: &MinecraftClient,
    movement: &MovementService,
    look: &LookController,
    inner: &mut Inner,
    name: &str,
) {
    stop_all(minecraft, movement, look).await;
    inner.snapshot.state = state::KillState::Failed;
    inner.snapshot.phase = state::CombatPhase::Abort;
    inner.snapshot.failure_reason = Some(format!("Target lost: {name}"));
}

/// Like [`lose_target`], for a fight abandoned for a reason other than the
/// target simply disappearing (currently only `max_chase_distance`) --
/// kept distinct so the failure reason recorded (and, via
/// `App::await_kill_terminal`, ultimately logged) says why.
async fn abort(
    minecraft: &MinecraftClient,
    movement: &MovementService,
    look: &LookController,
    inner: &mut Inner,
    name: &str,
    reason: &str,
) {
    stop_all(minecraft, movement, look).await;
    inner.snapshot.state = state::KillState::Failed;
    inner.snapshot.phase = state::CombatPhase::Abort;
    inner.snapshot.failure_reason = Some(format!("Target lost: {name} ({reason})"));
}

async fn eliminate(
    minecraft: &MinecraftClient,
    movement: &MovementService,
    look: &LookController,
    inner: &mut Inner,
    name: &str,
) {
    stop_all(minecraft, movement, look).await;
    inner.snapshot.state = state::KillState::Completed;
    inner.snapshot.phase = state::CombatPhase::Finish;
    logging::success(format!("Target eliminated: {name}"));
}

/// Releases raw movement input, any raised shield, and the camera --
/// shared by every terminal transition (`lose_target`, `abort`,
/// `eliminate`, `disconnect`) and by `kill::KillController::cancel`.
pub(crate) async fn stop_all(
    minecraft: &MinecraftClient,
    movement_service: &MovementService,
    look: &LookController,
) {
    let _ = minecraft
        .set_combat_walk(movement::CombatWalk::None, false)
        .await;
    // Whichever system was driving, both are released: the raw input above,
    // and the pathfinder here if the fight ended during a long approach.
    let _ = movement_service.stop(minecraft).await;
    let _ = minecraft.release_use_item().await;
    look.cancel().await;
    restore_weapon(minecraft).await;
}

/// Puts a weapon back in the bot's hand when a fight ends.
///
/// Without this the bot can be left standing around holding a golden apple
/// indefinitely: food only ever gets swapped out by `apply_weapon_selection`,
/// which runs *during* a fight, so a fight that ends mid-bite -- the target
/// dies, or `#kill` is cancelled, right as the bot is eating -- leaves the
/// apple in hand until the next fight starts. Best-effort and fire-and-forget
/// like everything else on this path: if there is no weapon to hold, or the
/// inventory is momentarily busy, the bot simply keeps holding what it has.
async fn restore_weapon(minecraft: &MinecraftClient) {
    let Ok(equipment) = minecraft.equipment_snapshot().await else {
        return;
    };
    let sword_available = tools::best_candidate(
        ToolRankingMode::Score,
        ToolCategory::Sword,
        &equipment.inventory,
    )
    .is_some();
    let wanted = shield_break::desired_weapon_category(false, sword_available);
    let _ = equip_weapon_category(minecraft, &equipment.inventory, wanted).await;
}
