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
        consume::{ConsumeAction, ConsumeLimits, ConsumeMachine, ConsumeObservation, ConsumeState},
        crits, defense, heal,
        health::{self, CombatMode},
        mace,
        movement::{
            self, CombatMovementController, LocalHazards, MovementCommand, MovementSnapshot,
            MovementTuning, Vec2,
        },
        shield_break, state,
        targeting::{self, PREDICTION_LEAD_SECONDS},
        wind_launch,
    },
    config::{KillbotConfig, ToolRankingMode},
    equipment::{
        manager::{swap_into_slot, swap_into_slot_during_consume},
        model::HOTBAR_PROTOCOL_SLOTS,
        tools,
    },
    interaction::tool_selection::{ToolCategory, category},
    logging,
    look::{LookController, LookTarget, aim_point::LookPrecision},
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

/// How many ordinary (non-Mace) hits must land between deliberate Wind
/// Charge self-launch attempts -- see `wants_to_begin_wind_launch`.
/// Randomized per attempt (rather than a fixed count) so the cadence
/// doesn't read as a metronome the target can anticipate.
const MACE_ATTEMPT_MIN_HITS: u32 = 3;
const MACE_ATTEMPT_MAX_HITS: u32 = 6;

/// How long `apply_weapon_selection` keeps holding the Mace after
/// `WindLaunchAction::Launched` before giving up and reverting to the
/// normal weapon -- covers the Mace's own slow (1.667s) cooldown catching
/// up if a swing wasn't already ready the instant the launch fired, without
/// stranding the bot holding a Mace it will never swing again because the
/// target dodged out of range mid-fall.
const SMASH_WINDOW_TIMEOUT: Duration = Duration::from_secs(3);

/// A uniformly random hit count in `[MACE_ATTEMPT_MIN_HITS, MACE_ATTEMPT_MAX_HITS]`
/// -- how many ordinary hits the next deliberate Wind Charge self-launch
/// attempt waits for. See `MACE_ATTEMPT_MIN_HITS`.
fn random_mace_attempt_hits(rng: &mut crate::look::aim_point::SeededRng) -> u32 {
    let span = MACE_ATTEMPT_MAX_HITS - MACE_ATTEMPT_MIN_HITS;
    MACE_ATTEMPT_MIN_HITS
        + (rng.next_unit() * f64::from(span + 1))
            .floor()
            .min(f64::from(span)) as u32
}

/// Stand-in for a player's maximum health when the real value isn't
/// observable. Max health can be modified by attributes/effects, but this
/// codebase has no visibility into that for a *remote* player (only their
/// current health -- see `MinecraftClient::player_combat_status`), and 20 is
/// both vanilla's default and the scale every threshold here is written in.
const ASSUMED_MAX_HEALTH: f32 = 20.0;

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
    /// The consume state machine -- see `crate::combat::consume`. Owns
    /// every "am I eating" question the rest of this module asks.
    consume: ConsumeMachine,
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
    /// Whether the weapon `held_weapon_cooldown` was just computed for is a
    /// Mace -- refreshed alongside it by [`apply_weapon_selection`], and
    /// [`apply_attack`]'s branch between sword/axe crit timing and
    /// `combat::mace`'s smash-attack timing.
    held_weapon_is_mace: bool,
    /// The Wind Charge self-launch state machine -- see
    /// `combat::wind_launch`'s module doc comment.
    wind_launch: wind_launch::WindLaunchMachine,
    /// Ordinary hits landed since the last deliberate Wind Charge self-launch
    /// attempt (successful, failed, or opportunistic -- see `swing`'s
    /// bookkeeping). Compared against `next_mace_attempt_hits` by
    /// `wants_to_begin_wind_launch`.
    hits_since_mace_attempt: u32,
    /// How many of those hits must land before the next deliberate attempt
    /// begins -- re-rolled every time it's reached, via
    /// `random_mace_attempt_hits`.
    next_mace_attempt_hits: u32,
    /// When `WindLaunchAction::Launched` last fired, if the Mace smash it
    /// set up hasn't landed (or timed out) yet -- see
    /// `apply_weapon_selection`'s `hold_mace_for_smash`. `None` whenever
    /// nothing is pending: no launch has fired, the swing already landed, or
    /// `SMASH_WINDOW_TIMEOUT` gave up on it.
    awaiting_smash_since: Option<Instant>,
    /// Which weapon category is due on the next swing while the bot's own
    /// shield is unavailable -- see
    /// `shield_break::desired_weapon_category`'s `axe_turn` parameter.
    /// Flipped unconditionally by every landed [`swing`] (Mace or not) --
    /// it's only ever consulted while the shield is unavailable, so
    /// flipping it for free the rest of the time is harmless and keeps
    /// `swing` decoupled from shield-availability.
    axe_turn: bool,
}

impl Inner {
    pub(crate) fn new(seed: u64, config: KillbotConfig) -> Self {
        let mut rng = crate::look::aim_point::SeededRng::new(seed);
        let next_mace_attempt_hits = random_mace_attempt_hits(&mut rng);
        Self {
            snapshot: state::KillSnapshot::default(),
            config,
            approaching: false,
            approach_goal: None,
            movement: CombatMovementController::new(),
            rng,
            last_attack: None,
            last_jump: None,
            sprint_released_at: None,
            engaged_logged: false,
            look_target_set_for: None,
            last_seen_at: None,
            consume: ConsumeMachine::new(Instant::now()),
            last_eat_finished: None,
            topping_up: false,
            finishing_logged: false,
            shield_raised: false,
            held_weapon_cooldown: None,
            crit_hold_since: None,
            target_was_blocking: false,
            held_weapon_is_mace: false,
            wind_launch: wind_launch::WindLaunchMachine::new(Instant::now()),
            hits_since_mace_attempt: 0,
            next_mace_attempt_hits,
            awaiting_smash_since: None,
            axe_turn: false,
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
        self.consume.reset(Instant::now());
        self.last_eat_finished = None;
        self.topping_up = false;
        self.finishing_logged = false;
        self.shield_raised = false;
        self.held_weapon_cooldown = None;
        self.held_weapon_is_mace = false;
        self.crit_hold_since = None;
        self.target_was_blocking = false;
        self.wind_launch.reset(Instant::now());
        self.hits_since_mace_attempt = 0;
        self.next_mace_attempt_hits = random_mace_attempt_hits(&mut self.rng);
        self.awaiting_smash_since = None;
        self.axe_turn = false;
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
    let within_attack_range = distance <= inner.config.attack_range;

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
    // Two different questions, and they part company during the weapon
    // restore: the hand is still the consume's (no attacking, no weapon
    // swap), but the bite has landed, so sprint and shield are free again.
    let eating_blocked = inner.consume.state().blocks_combat();
    // A self-launch attempt in flight claims the hand and the camera the
    // same way a bite does (see `apply_wind_launch`'s doc comment), so it
    // gets the same treatment here: no shield, no sprint. It deliberately
    // does *not* factor into `retreat` below -- backing off would defeat
    // the whole point of self-launching onto the target.
    let wind_launch_active = inner.wind_launch.state().is_active();
    let combat_blocked = eating_blocked || wind_launch_active;
    let retreat = inner.config.allow_retreat_while_eating && eating_blocked;

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
        if wind_launch_active {
            // Hold position for the aim-down/throw sequence -- walking
            // (and `camera_yaw`, which the steering projection needs) both
            // stop meaning anything sensible while the camera is pointed
            // straight down. See `apply_wind_launch`'s doc comment.
            let _ = minecraft
                .set_combat_walk(crate::combat::movement::CombatWalk::None, false)
                .await;
        } else {
            apply_movement(
                minecraft,
                inner,
                bot_position,
                target_position,
                world.bot.yaw.unwrap_or_default(),
                world.bot.on_ground.unwrap_or(true),
                world.bot.horizontal_collision.unwrap_or(false),
                !combat_blocked,
                retreat,
            )
            .await;
        }
    }
    if !eating {
        let on_ground = world.bot.on_ground.unwrap_or(true);
        // `apply_wind_launch` makes its own "should I begin" decision when
        // idle and reports back whether it actually claimed the tick --
        // relying on that return value (rather than re-checking the same
        // trigger condition out here) is what keeps a tick from going to
        // waste on the rare miss (e.g. no Wind Charge left after all):
        // this falls straight through to an ordinary attack instead of
        // both branches silently doing nothing.
        let wind_launching = apply_wind_launch(
            minecraft,
            look,
            inner,
            bot_position,
            on_ground,
            world.bot.velocity_y.unwrap_or(0.0),
            within_attack_range,
        )
        .await;
        if !wind_launching {
            apply_weapon_selection(
                minecraft,
                inner,
                world.bot.selected_hotbar_slot,
                inner.config.shield_break_enabled && status.using_item,
                on_ground,
                world.bot.velocity_y.unwrap_or(0.0),
                world.bot.fall_distance.unwrap_or(0.0),
            )
            .await;
            apply_attack(
                minecraft,
                inner,
                target_uuid,
                on_ground,
                world.bot.velocity_y.unwrap_or(0.0),
                within_attack_range,
                finishing,
                world.bot.fall_distance.unwrap_or(0.0),
            )
            .await;
        }
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
    if inner.config.shield_use_enabled && !combat_blocked {
        let approaching = targeting::is_approaching(bot_position, target_position, velocity);
        // Same cooldown check `apply_attack` already makes before swinging --
        // see `defense`'s module doc comment for why "can I hit back right
        // now" is the reactive half of the raise decision.
        let attack_ready =
            crits::attack_ready(inner.last_attack, Instant::now(), attack_cooldown(inner));
        // Fetched independently rather than reusing `apply_weapon_selection`'s
        // snapshot: this branch also runs during `ConsumeState::RestoringWeapon`
        // (the bite just landed, shield/sprint are free again), a state where
        // `apply_weapon_selection` does not run at all -- see `tick`'s `eating`
        // gate above. `equipment_snapshot` is a local ECS read, not a network
        // round trip, so fetching it again here costs nothing real.
        let own_shield_equipped = minecraft
            .equipment_snapshot()
            .await
            .is_ok_and(|equipment| shield_break::own_shield_serviceable(equipment.offhand_worn.as_ref()));
        apply_defense(
            minecraft,
            inner,
            mode,
            distance,
            approaching,
            attack_ready,
            own_shield_equipped,
        )
        .await;
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
    retreat: bool,
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
        retreat,
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

/// How short of `attack_range` the sprint-in band stops, in blocks.
///
/// `CombatMovementController::update` only sprints past `band.preferred_max`
/// (`distance > band.preferred_max` -- see that function's `sprint` line),
/// so this is the difference between the bot landing most hits from a
/// freshly-reset sprint (real knockback spacing, the actual core loop of
/// PvP -- see [`apply_attack`]'s doc comment) and landing them from a
/// standstill because it downshifted to a walk a full block or more before
/// it was even close enough to swing. Not zero: sprinting the very last
/// sliver into the target's hitbox is what real players avoid too (the
/// W-tap release), and it keeps `preferred_max` a hair under `attack_range`
/// so the clamp below never collapses the band to a single point.
const SPRINT_IN_MARGIN: f64 = 0.3;

/// Translates the user's `[killbot]` settings into the movement
/// controller's own tuning.
fn movement_tuning(inner: &Inner) -> MovementTuning {
    let defaults = MovementTuning::default();
    // Sprint stops (and the bot settles into orbiting) at `preferred_max` --
    // deriving it from `attack_range` rather than steering's compiled-in
    // 1.9-block default means it scales with however far the bot can
    // actually reach, instead of leaving a dead walk-only zone between
    // wherever sprinting stops and wherever attacks start.
    let preferred_max = inner
        .config
        .preferred_range
        .max(inner.config.attack_range - SPRINT_IN_MARGIN)
        .min(inner.config.attack_range);
    MovementTuning {
        band: movement::DistanceBand {
            preferred_min: inner.config.preferred_range.min(preferred_max),
            preferred_max,
            // `chase` (full-commitment closing distance) must stay past
            // `preferred_max` or `steering::desired_velocity`'s eased
            // approach zone between them collapses to nothing -- see that
            // function's `radial_scale` computation. The compiled default
            // (2.5) already clears a `preferred_max` derived from the
            // default 2.0-block `attack_range`, so this only ever raises it
            // for a larger configured `attack_range`.
            chase: defaults.band.chase.max(preferred_max + 0.5),
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
/// waiting for an opening -- if health is below the threshold and there is
/// food anywhere in the inventory, the bot puts it in its hand and bites
/// while it is still moving. With `allow_retreat_while_eating` it backs off
/// and circles while chewing rather than standing there.
///
/// Returns whether the hand is committed to food right now, which is what
/// makes the caller suppress attacking, sprinting, weapon swaps and the
/// shield for the duration -- every one of those cancels vanilla item use.
///
/// # Eating is an atomic action
///
/// The bite is driven by [`ConsumeMachine`], and for its whole duration the
/// bot's hand is claimed by `MinecraftClient::begin_consume_guard`. That
/// guard is the fix for a bug the `if !eating` checks in [`tick`] could
/// never catch: this module is not the only thing that touches the
/// inventory. `HotbarEquipmentService` and `EquipmentService` both run every
/// tick and re-evaluate whenever the inventory *revision* changes -- and
/// eating changes the revision. A bite reliably woke them, they clicked or
/// selected a slot, and the server cancelled the use. The bot stood there
/// holding an apple it never ate.
///
/// With the guard raised, every one of those mutations is refused with
/// `InventoryBusy` (which they already treat as "later"), and the consume
/// path uses the `*_during_consume` variants that bypass it. Only
/// `crate::survival`'s fall clutch is allowed to break in, because drowning
/// in lava beats finishing a snack.
///
/// # Why it takes two ticks
///
/// Selecting the food and using it in the same tick does not work: Azalea
/// sends the use-item packet *before* the carried-item packet within a tick,
/// so the server applies the use to whatever was held before -- the sword.
/// The machine therefore waits for `acknowledged_hotbar_slot()` to confirm
/// the swap before sending use.
async fn apply_eating(
    minecraft: &MinecraftClient,
    inner: &mut Inner,
    inventory: &crate::minecraft::world_state::InventorySnapshot,
    wants_food: bool,
    finishing: bool,
) -> bool {
    let now = Instant::now();

    // The target dropped into finisher range: abandon whatever is in flight
    // rather than chew while they die or get away. An interrupted vanilla
    // consume doesn't lose the item, so this costs nothing.
    if finishing {
        if inner.consume.state().is_active() {
            inner.consume.reset(now);
            minecraft.end_consume_guard();
        }
        return false;
    }

    // Drive whatever attempt is already running.
    if inner.consume.state().is_active() {
        return drive_consume(minecraft, inner, inventory, now).await;
    }

    // One bite, then straight back to the fight: a golden apple heals over
    // five seconds, so the tick after swallowing still reports the old
    // health. Without this the bot would immediately decide it is still hurt
    // and start another, chewing through the stack instead of swinging.
    if inner
        .last_eat_finished
        .is_some_and(|finished| finished.elapsed() < inner.config.eat_cooldown())
    {
        return false;
    }
    // A Wind Charge self-launch is mid-flight, holding the same consume
    // guard this is about to claim -- see `apply_wind_launch`'s doc
    // comment. Its every state is bounded by a short timeout, so this is a
    // wait of at most about two seconds, not a stall: the tick after it
    // finishes (launched or not), this sees `is_active() == false` again.
    if !wants_food || !inner.consume.ready_to_start(now) || inner.wind_launch.state().is_active() {
        return false;
    }
    begin_consume(minecraft, inner, inventory, now).await
}

/// Picks food, clears the hand, and starts an attempt. Returns whether the
/// hand is now committed.
async fn begin_consume(
    minecraft: &MinecraftClient,
    inner: &mut Inner,
    inventory: &crate::minecraft::world_state::InventorySnapshot,
    now: Instant,
) -> bool {
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
        // Nothing edible -- keep fighting and let natural regeneration do
        // whatever it does.
        return false;
    };
    let label = best.item_id.to_owned();

    // Claim the hand before touching anything: from here until the attempt
    // ends, no other subsystem may move an item.
    minecraft.begin_consume_guard();

    // A raised shield is an off-hand item use, and starting a main-hand one
    // on top of it cancels the bite. Down it goes, and `tick` will not raise
    // it again while the consume is active.
    if inner.shield_raised {
        let _ = minecraft.release_use_item().await;
        inner.shield_raised = false;
    }

    let hotbar_index = if HOTBAR_PROTOCOL_SLOTS.contains(&best.slot) {
        Some((best.slot - HOTBAR_PROTOCOL_SLOTS.start()) as u8)
    } else if swap_into_slot_during_consume(minecraft, best.slot, COMBAT_FOOD_PROTOCOL_SLOT).await {
        Some(COMBAT_FOOD_HOTBAR_INDEX)
    } else {
        None
    };
    let Some(hotbar_index) = hotbar_index else {
        minecraft.end_consume_guard();
        return false;
    };
    logging::info(format!("Preparing {}", crate::blocks::bare_id(&label)));
    inner
        .consume
        .begin(label, hotbar_index, inventory.count_item(best.item_id), now);
    true
}

/// One tick of an in-flight attempt: reads the world, advances the machine,
/// and performs whatever it asks for.
async fn drive_consume(
    minecraft: &MinecraftClient,
    inner: &mut Inner,
    inventory: &crate::minecraft::world_state::InventorySnapshot,
    now: Instant,
) -> bool {
    let held_count = inner
        .consume
        .item_id()
        .map_or(0, |item_id| inventory.count_item(item_id));
    let observation = ConsumeObservation {
        acknowledged_slot: minecraft.acknowledged_hotbar_slot().await.ok().flatten(),
        held_count,
        // The restore is confirmed by the weapon actually being back in
        // hand, not by the swap call returning.
        weapon_restored: inner.held_weapon_cooldown.is_some() && weapon_in_hand(inventory),
        now,
    };
    let limits = ConsumeLimits {
        retry_limit: inner.config.eat_retry_limit,
        retry_delay: inner.config.eat_retry_delay(),
    };

    match inner.consume.advance(observation, &limits) {
        ConsumeAction::Wait => {}
        ConsumeAction::SelectSlot { hotbar_index } => {
            // Always sent, even when the slot already looks selected: what
            // matters is what the *server* has been told, and the local view
            // runs ahead of the packet.
            if minecraft
                .select_hotbar_slot_during_consume(hotbar_index)
                .await
                .is_ok()
            {
                inner.consume.slot_selected(now);
                logging::info("Waiting for hotbar acknowledgement");
            }
        }
        ConsumeAction::StartUse => {
            if minecraft.start_use_main_hand().await.is_ok() {
                inner.consume.use_started(held_count, now);
                logging::info(format!(
                    "Started eating {}",
                    inner.consume.item_id().map_or_else(
                        || "food".to_owned(),
                        |id| crate::blocks::bare_id(id).to_owned()
                    )
                ));
            }
        }
        ConsumeAction::RestoreWeapon => {
            // The bite has landed, so there is no longer any item use to
            // protect -- and the restore itself goes through the ordinary
            // guarded inventory path, which would otherwise refuse itself
            // and stall until the restore timeout.
            minecraft.end_consume_guard();
            if inner.consume.state() == ConsumeState::Consumed {
                logging::success(format!(
                    "{} consumed",
                    inner.consume.item_id().map_or_else(
                        || "Food".to_owned(),
                        |id| crate::blocks::bare_id(id).to_owned()
                    )
                ));
                logging::info("Restoring weapon");
                inner.consume.restore_started(now);
                inner.last_eat_finished = Some(now);
            }
            restore_weapon(minecraft).await;
        }
        ConsumeAction::Failed { reason, retry_in } => {
            minecraft.end_consume_guard();
            match retry_in {
                Some(_) => {
                    logging::warning(format!("Golden apple use interrupted: {}", reason.label()));
                    logging::info(format!(
                        "Retrying consume ({}/{})",
                        inner.consume.attempts(),
                        inner.config.eat_retry_limit
                    ));
                }
                None => {
                    logging::warning(format!(
                        "Giving up on eating: {} after {} attempts",
                        reason.label(),
                        inner.consume.attempts()
                    ));
                    // Treated as a completed bite for pacing purposes so a
                    // hopeless food situation doesn't retry in a tight loop
                    // for the rest of the fight.
                    inner.last_eat_finished = Some(now);
                }
            }
            restore_weapon(minecraft).await;
            return false;
        }
        ConsumeAction::Finished => {
            minecraft.end_consume_guard();
            logging::info("Re-engaging target");
            return false;
        }
    }
    inner.consume.state().holds_hand()
}

/// Reserved hotbar slot for the Wind Charge used to self-launch into a Mace
/// smash attack -- distinct from `COMBAT_FOOD_HOTBAR_INDEX`/
/// `COMBAT_WEAPON_HOTBAR_INDEX` so none of the three ever displaces another
/// mid-sequence. `HotbarSlotsConfig`'s user-managed slots stop at index 5
/// (slots 1-6, 1-indexed -- see `COMBAT_WEAPON_HOTBAR_INDEX`'s doc comment),
/// leaving 6/7/8 free for combat's own reserved use; 7 and 8 are already
/// food and weapon, so this is 6.
const COMBAT_WIND_CHARGE_HOTBAR_INDEX: u8 = 6;
const COMBAT_WIND_CHARGE_PROTOCOL_SLOT: usize = 42;

/// A point directly below `bot_position`, for aiming the Wind Charge throw
/// at the bot's own feet (see `wind_launch`'s module doc comment for why
/// straight down). The exact distance below doesn't matter -- any point on
/// the same vertical line produces the same pitch, since the horizontal
/// component of the look vector is zero either way (see
/// `look::rotation::rotation_towards`) -- it just needs to be unambiguously
/// "down" rather than level.
fn feet_below(
    bot_position: crate::minecraft::world_state::PositionSnapshot,
) -> crate::minecraft::world_state::PositionSnapshot {
    crate::minecraft::world_state::PositionSnapshot {
        x: bot_position.x,
        y: bot_position.y - 10.0,
        z: bot_position.z,
    }
}

/// Whether to begin a new Wind Charge self-launch attempt this tick. Never
/// while eating or mid an existing attempt -- both claim the hand via the
/// same consume guard, and only one may hold it at a time -- and not until
/// `next_mace_attempt_hits` ordinary hits have actually landed since the
/// last one (see `MACE_ATTEMPT_MIN_HITS`), so the deliberate combo is spaced
/// out by real fight progress rather than attempted every time the cooldown
/// alone allows it.
fn wants_to_begin_wind_launch(
    inner: &Inner,
    on_ground: bool,
    within_attack_range: bool,
    now: Instant,
) -> bool {
    inner.config.prefer_mace
        && on_ground
        && within_attack_range
        && !inner.consume.state().is_active()
        && inner.wind_launch.ready_to_start(now)
        && inner.hits_since_mace_attempt >= inner.next_mace_attempt_hits
}

/// Drives (or begins) a Wind Charge self-launch combo for a Mace smash
/// attack -- see `wind_launch`'s module doc comment for the full state
/// machine. Returns whether it claimed this tick's turn: when `true`, the
/// caller must skip `apply_weapon_selection`/`apply_attack`/normal movement
/// entirely for this tick, the same way `eating` does (and for the same
/// reason -- the hand and the camera are both committed); when `false`,
/// nothing is happening and normal combat proceeds untouched.
///
/// # Why this needs the consume guard too
///
/// Equipping the Wind Charge, throwing it, and (on success) equipping the
/// Mace afterward are all inventory/hotbar mutations spread across several
/// ticks -- exactly the shape `HotbarEquipmentService`/`EquipmentService`
/// would otherwise race the moment the inventory revision changes (see
/// `apply_eating`'s doc comment for the bug this guard originally fixed).
/// Reusing `MinecraftClient`'s single consume guard rather than inventing a
/// second one keeps there being exactly one "the hand is claimed" flag in
/// the whole bot, and exactly one bypass (`_during_consume`) every
/// legitimate claimant already knows how to use.
async fn apply_wind_launch(
    minecraft: &MinecraftClient,
    look: &LookController,
    inner: &mut Inner,
    bot_position: crate::minecraft::world_state::PositionSnapshot,
    on_ground: bool,
    velocity_y: f64,
    within_attack_range: bool,
) -> bool {
    let now = Instant::now();
    if inner.wind_launch.state() == wind_launch::WindLaunchState::Idle {
        if !wants_to_begin_wind_launch(inner, on_ground, within_attack_range, now) {
            return false;
        }
        return begin_wind_launch(minecraft, inner, now).await;
    }
    drive_wind_launch(
        minecraft,
        look,
        inner,
        bot_position,
        on_ground,
        velocity_y,
        now,
    )
    .await;
    true
}

/// Finds a Wind Charge, claims the hand, and swaps it into its reserved
/// slot. Returns whether an attempt is now in flight -- `false` (with the
/// guard never raised) just means try again another tick, exactly like
/// `begin_consume` finding nothing edible.
async fn begin_wind_launch(minecraft: &MinecraftClient, inner: &mut Inner, now: Instant) -> bool {
    let Ok(equipment) = minecraft.equipment_snapshot().await else {
        return false;
    };
    let Some(charge) = equipment
        .inventory
        .iter()
        .find(|item| item.item_id == wind_launch::WIND_CHARGE_ITEM_ID)
    else {
        return false;
    };
    minecraft.begin_consume_guard();
    if !swap_into_slot_during_consume(minecraft, charge.slot, COMBAT_WIND_CHARGE_PROTOCOL_SLOT)
        .await
    {
        minecraft.end_consume_guard();
        return false;
    }
    inner.wind_launch.begin(now);
    // The attempt is genuinely underway now -- rearm the counter for the
    // *next* one rather than leaving it sitting at (or past) the threshold,
    // which would otherwise fire again the instant this attempt's cooldown
    // clears regardless of how many more hits actually landed in between.
    inner.hits_since_mace_attempt = 0;
    inner.next_mace_attempt_hits = random_mace_attempt_hits(&mut inner.rng);
    true
}

/// One tick of an in-flight self-launch attempt: reads the world and the
/// look controller, advances the machine, and performs whatever it asks
/// for.
async fn drive_wind_launch(
    minecraft: &MinecraftClient,
    look: &LookController,
    inner: &mut Inner,
    bot_position: crate::minecraft::world_state::PositionSnapshot,
    on_ground: bool,
    velocity_y: f64,
    now: Instant,
) {
    let observation = wind_launch::WindLaunchObservation {
        acknowledged_slot: minecraft.acknowledged_hotbar_slot().await.ok().flatten(),
        aim_settled: look
            .snapshot()
            .await
            .pitch
            .is_some_and(|pitch| pitch >= wind_launch::AIM_DOWN_PITCH_THRESHOLD),
        on_ground,
        velocity_y,
        now,
    };
    match inner.wind_launch.advance(observation) {
        wind_launch::WindLaunchAction::Wait => {}
        wind_launch::WindLaunchAction::SelectSlot => {
            if minecraft
                .select_hotbar_slot_during_consume(COMBAT_WIND_CHARGE_HOTBAR_INDEX)
                .await
                .is_ok()
            {
                inner.wind_launch.slot_selected(now);
                logging::info("Preparing wind charge launch");
            }
        }
        wind_launch::WindLaunchAction::AimDown => {
            if look
                .look_at_with_precision(
                    minecraft,
                    LookTarget::World(feet_below(bot_position)),
                    LookPrecision::Precise,
                )
                .await
                .is_ok()
            {
                inner.wind_launch.aim_started(now);
            }
        }
        wind_launch::WindLaunchAction::Throw => {
            if minecraft.throw_main_hand_item().await.is_ok() {
                inner.wind_launch.thrown(now);
                logging::info("Throwing wind charge");
            }
        }
        wind_launch::WindLaunchAction::Launched => {
            minecraft.end_consume_guard();
            logging::progress("Launched -- going for a smash attack");
            // Keeps `apply_weapon_selection` holding the Mace through the
            // rest of the fall -- see `hold_mace_for_smash`'s doc comment
            // for why the opportunistic fall check alone isn't enough right
            // at this instant (the bot is still rising, not yet falling).
            inner.awaiting_smash_since = Some(now);
            let _ = look.release_precise(minecraft).await;
            // Best-effort: if this fails, `apply_weapon_selection`'s
            // ordinary prefer_mace branch picks it up the very next tick
            // now that the guard is down, same as any other equip retry
            // in this codebase.
            if let Ok(equipment) = minecraft.equipment_snapshot().await
                && let Some(mace_item) = equipment
                    .inventory
                    .iter()
                    .find(|item| item.item_id == mace::MACE_ITEM_ID)
            {
                equip_item_at_slot_during_consume(minecraft, mace_item.slot).await;
            }
        }
        wind_launch::WindLaunchAction::Failed { reason, .. } => {
            minecraft.end_consume_guard();
            let _ = look.release_precise(minecraft).await;
            logging::warning(format!("Wind charge launch aborted: {}", reason.label()));
        }
    }
}

/// Whether the selected hotbar slot holds something the bot fights with --
/// how the restore step knows it is finished.
fn weapon_in_hand(inventory: &crate::minecraft::world_state::InventorySnapshot) -> bool {
    inventory
        .selected_item()
        .and_then(|item| item.item_id.as_deref())
        .and_then(category)
        .is_some_and(|kind| matches!(kind, ToolCategory::Sword | ToolCategory::Axe))
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
    on_ground: bool,
    velocity_y: f64,
    fall_distance: f64,
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
    inner.held_weapon_is_mace = current.is_some_and(|item| item.item_id == mace::MACE_ITEM_ID);
    let durability_critical = current.is_some_and(|item| {
        shield_break::is_durability_critical(item.current_durability, item.max_durability)
    });
    let own_shield_available = shield_break::own_shield_serviceable(equipment.offhand_worn.as_ref());

    // `awaiting_smash_since` covers the deliberate combo (set the instant
    // `WindLaunchAction::Launched` fires, while the bot is still rising --
    // too early for the fall-based check below to see anything yet) but
    // expires on its own timeout so a target that dodges out of range
    // mid-fall doesn't strand the bot holding a Mace forever.
    if let Some(since) = inner.awaiting_smash_since
        && Instant::now().saturating_duration_since(since) >= SMASH_WINDOW_TIMEOUT
    {
        inner.awaiting_smash_since = None;
    }
    // The opportunistic half: any fall the fight produces on its own --
    // jumping down onto the target, a Breeze's wind charge, a Wind Burst
    // Mace re-launching itself off a smash that already landed -- not just
    // this bot's own deliberate combo. `is_smash_attack_window` alone would
    // only pick this up once the fall has *already* crossed
    // `SMASH_ATTACK_MIN_FALL`; the `velocity_y` half of this catches a hard
    // launch a tick or two earlier, while still rising, the same threshold
    // `wind_launch::LAUNCH_VELOCITY_THRESHOLD` uses to tell a real launch
    // apart from a normal jump's ~0.42 peak.
    let opportunistic_fall = mace::is_smash_attack_window(on_ground, velocity_y, fall_distance)
        || (!on_ground && velocity_y >= wind_launch::LAUNCH_VELOCITY_THRESHOLD);
    let hold_mace_for_smash = inner.awaiting_smash_since.is_some() || opportunistic_fall;

    // A Mace can't disable a shield the way an axe does, so shield-breaking
    // still gets first claim on the hand -- this only ever takes over while
    // there's nothing to break through. Bypasses the `ToolCategory`-keyed
    // logic below entirely: a Mace structurally isn't one of Azalea's
    // mining-tool categories (it doesn't mine), so it's matched by item id
    // directly, the same way `equipment::offhand` matches a totem/shield.
    //
    // Deliberately *not* "whenever `prefer_mace` is on": that would hold
    // the Mace for every ordinary hit, eating its much slower recharge for
    // no smash bonus most of the time. It's only worth the trade for the
    // swing that might actually land as a smash.
    if inner.config.prefer_mace
        && hold_mace_for_smash
        && !target_blocking
        && let Some(mace_item) = equipment
            .inventory
            .iter()
            .find(|item| item.item_id == mace::MACE_ITEM_ID)
    {
        inner.held_weapon_is_mace = true;
        inner.held_weapon_cooldown = Some(mace::MACE_COOLDOWN);
        if !current.is_some_and(|item| item.item_id == mace::MACE_ITEM_ID) || durability_critical {
            equip_item_at_slot(minecraft, mace_item.slot).await;
        }
        return;
    }

    let sword_available = tools::best_candidate(
        ToolRankingMode::Score,
        ToolCategory::Sword,
        &equipment.inventory,
    )
    .is_some();
    let wanted = shield_break::desired_weapon_category(
        target_blocking,
        sword_available,
        own_shield_available,
        inner.axe_turn,
    );
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

/// Selects `source_slot` directly if it's already in the hotbar, otherwise
/// swaps it into the reserved combat-weapon slot first -- the one place
/// both [`equip_weapon_category`] (`ToolCategory`-keyed) and
/// [`apply_weapon_selection`]'s Mace branch (item-id-keyed) actually touch
/// the client, so there is exactly one implementation of "put this
/// inventory item in the bot's hand" to get right.
async fn equip_item_at_slot(minecraft: &MinecraftClient, source_slot: usize) -> bool {
    equip_item_at_slot_inner(minecraft, source_slot, false).await
}

/// [`equip_item_at_slot`] for the consume path -- for the wind-launch
/// driver's own mace-equip step once launched, which runs while it still
/// holds the consume guard (see `apply_wind_launch`'s doc comment) and would
/// otherwise refuse itself.
async fn equip_item_at_slot_during_consume(
    minecraft: &MinecraftClient,
    source_slot: usize,
) -> bool {
    equip_item_at_slot_inner(minecraft, source_slot, true).await
}

async fn equip_item_at_slot_inner(
    minecraft: &MinecraftClient,
    source_slot: usize,
    during_consume: bool,
) -> bool {
    if HOTBAR_PROTOCOL_SLOTS.contains(&source_slot) {
        let hotbar_index = (source_slot - HOTBAR_PROTOCOL_SLOTS.start()) as u8;
        return if during_consume {
            minecraft
                .select_hotbar_slot_during_consume(hotbar_index)
                .await
                .is_ok()
        } else {
            minecraft.select_hotbar_slot(hotbar_index).await.is_ok()
        };
    }
    let swapped = if during_consume {
        swap_into_slot_during_consume(minecraft, source_slot, COMBAT_WEAPON_PROTOCOL_SLOT).await
    } else {
        swap_into_slot(minecraft, source_slot, COMBAT_WEAPON_PROTOCOL_SLOT).await
    };
    swapped
        && if during_consume {
            minecraft
                .select_hotbar_slot_during_consume(COMBAT_WEAPON_HOTBAR_INDEX)
                .await
                .is_ok()
        } else {
            minecraft
                .select_hotbar_slot(COMBAT_WEAPON_HOTBAR_INDEX)
                .await
                .is_ok()
        }
}

async fn equip_weapon_category(
    minecraft: &MinecraftClient,
    inventory: &[crate::equipment::model::EquipmentItem],
    wanted: ToolCategory,
) -> bool {
    let Some(candidate) = tools::best_candidate(ToolRankingMode::Score, wanted, inventory) else {
        return false;
    };
    equip_item_at_slot(minecraft, candidate.item.slot).await
}

/// Decides and sends this tick's attack, if any.
///
/// The default neutral-game hit is a **sprint-reset knockback hit**, not a
/// jump-held crit: vanilla only awards sprint knockback on a swing thrown
/// while sprinting, and only awards a critical hit on a swing thrown while
/// *not* sprinting -- the two are mutually exclusive per hit (see `crits`'s
/// module doc comment for the damage-cooldown side of this and
/// `movement_tuning`'s [`SPRINT_IN_MARGIN`] for why the bot is actually
/// sprinting when it arrives here). Landing the knockback is what keeps a
/// fight one-sided: it pushes the target out of their own attack range
/// every hit, so trading away that knockback for +50% damage on a hit that
/// leaves them standing right next to the bot is a bad trade in a live
/// exchange, even though it looks like more damage. See the `finishing`
/// branch below for the one case where that trade is unambiguously good.
#[expect(
    clippy::too_many_arguments,
    reason = "one call site, and each argument is an independent reading of               the same tick, exactly like apply_movement's"
)]
async fn apply_attack(
    minecraft: &MinecraftClient,
    inner: &mut Inner,
    target_uuid: Uuid,
    on_ground: bool,
    velocity_y: f64,
    within_attack_range: bool,
    finishing: bool,
    fall_distance: f64,
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
        // A Mace's smash attack needs a real fall (`mace::SMASH_ATTACK_MIN_FALL`
        // is 1.5 blocks) -- the tiny hop this pre-jump produces (timed to
        // clear only `crits::is_critical_window`'s "any downward velocity
        // at all") can never reach it, so there is nothing useful to jump
        // for here. See `apply_attack`'s Mace branch below for what a Mace
        // actually does with a fall the fight happens to produce.
        if !inner.held_weapon_is_mace
            && inner.config.crit_enabled
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

    if inner.held_weapon_is_mace {
        // No hold-and-wait here, unlike the sword/axe crit logic below: a
        // smash-worthy fall isn't something this bot engineers (see
        // `combat::mace`'s module doc comment), so there's nothing to wait
        // for that isn't already reflected in `fall_distance` this tick --
        // take the hit now, smash bonus or not, rather than stalling the
        // weapon's already-slow cadence any further.
        let is_smash = mace::is_smash_attack_window(on_ground, velocity_y, fall_distance);
        swing(minecraft, inner, target_uuid, now, false).await;
        if is_smash {
            logging::progress(format!(
                "Smash attack landed (+{:.0} bonus damage)",
                mace::smash_bonus_damage(fall_distance)
            ));
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
    // own retry spacing, or the bot only just came into range.
    //
    // Vanilla makes sprint-knockback and critical hits mutually exclusive
    // per swing (a sprinting attacker never crits, full stop -- see
    // `crits`'s module doc comment), so forcing a jump-hold here is not a
    // free +50% damage: it is trading away this swing's sprint-reset
    // knockback, the thing that keeps the target from just standing there
    // landing a return hit. That trade is only clearly worth it on a
    // finishing blow (`finishing` -- the target dies to the next couple of
    // hits regardless of spacing) or when the user has explicitly asked for
    // maximum aggression every swing via `always_crit`. Otherwise the plain
    // `swing` below, launched from whatever sprint the movement controller
    // has going (see `movement_tuning`'s `SPRINT_IN_MARGIN`), *is* the
    // sprint-reset hit.
    if inner.config.crit_enabled
        && (finishing || inner.config.always_crit)
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
    inner.axe_turn = !inner.axe_turn;
    inner.snapshot.phase = state::CombatPhase::Attack;
    // Sprint reset and combo pressure both live in the movement controller:
    // it drops sprint for a moment without releasing the keys, and tightens
    // the orbit while the combo is live.
    inner.movement.note_attack(now);
    if is_crit {
        inner.snapshot.crits_landed += 1;
        logging::progress("Critical hit landed");
    }
    // A Mace swing landing -- deliberate combo or an opportunistic fall --
    // is what `hits_since_mace_attempt` and `awaiting_smash_since` are both
    // waiting for: rearm the hit counter for the next deliberate attempt and
    // let `apply_weapon_selection` revert to the normal weapon starting next
    // tick. An ordinary swing instead counts toward that next attempt.
    if inner.held_weapon_is_mace {
        inner.hits_since_mace_attempt = 0;
        inner.next_mace_attempt_hits = random_mace_attempt_hits(&mut inner.rng);
        inner.awaiting_smash_since = None;
    } else {
        inner.hits_since_mace_attempt += 1;
    }
}

/// Raises/lowers the bot's own shield -- see `crate::combat::defense`'s
/// module doc comment for the timing this approximates and its real
/// limitations. Only actually calls into the client on a state *change*
/// (raising or lowering), not every tick, since re-sending "start use
/// item" every tick while already blocking is unnecessary and re-sending
/// "release" while not blocking is a harmless no-op but still pointless
/// network chatter.
///
/// `own_shield_equipped` gates the raise on there actually being a shield
/// in the offhand right now (see `shield_break::own_shield_serviceable`) --
/// without it, `should_raise` alone used to send "start use offhand" even
/// with nothing there, a silent no-op that happened to make a broken shield
/// invisible to the rest of the bot's behavior. It also has to gate the
/// *release*, not just the raise: if the shield breaks (or is swapped out)
/// while `inner.shield_raised` is already `true`, `should_raise` can still
/// be `true` next tick (nothing about the threat that raised it changed),
/// so checking only `!should_raise` would leave `inner.shield_raised` stuck
/// `true` forever -- never re-raised (that needs a shield), never released
/// (that needed `should_raise` to flip). Checking `!own_shield_equipped` in
/// the release branch too closes that gap.
async fn apply_defense(
    minecraft: &MinecraftClient,
    inner: &mut Inner,
    mode: CombatMode,
    distance: f64,
    target_approaching: bool,
    attack_ready: bool,
    own_shield_equipped: bool,
) {
    let should_raise = defense::should_raise_shield(mode, distance, target_approaching, attack_ready);
    if should_raise && own_shield_equipped && !inner.shield_raised {
        if minecraft.start_use_off_hand().await.is_ok() {
            inner.shield_raised = true;
        }
    } else if (!should_raise || !own_shield_equipped) && inner.shield_raised {
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
    // The fight is ending, so there is no longer an in-flight bite worth
    // protecting -- and `KillController::cancel` reaches this *without*
    // ever driving `drive_consume` again (`tick` bails the instant the
    // snapshot leaves `Running`), so the guard raised by `begin_consume`
    // would otherwise stay stuck forever, refusing every hotbar/container
    // mutation in the bot -- including `restore_weapon` below -- until a
    // brand new `#kill` happens to start. Safe to call unconditionally: a
    // no-op when nothing was mid-bite.
    minecraft.end_consume_guard();
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
    // Post-fight cleanup, not a live combat decision -- always wants the
    // sword back (never the target-blocking or shield-loss alternation this
    // policy also handles mid-fight), so `own_shield_available: true` keeps
    // alternation off regardless of the bot's actual shield state.
    let wanted = shield_break::desired_weapon_category(false, sword_available, true, false);
    let _ = equip_weapon_category(minecraft, &equipment.inventory, wanted).await;
}

#[cfg(test)]
mod movement_tuning_tests {
    use super::*;

    fn tuning_for(config: KillbotConfig) -> MovementTuning {
        movement_tuning(&Inner::new(1, config))
    }

    #[test]
    fn preferred_max_tracks_a_larger_attack_range_instead_of_the_steering_default() {
        // Default attack_range (3.0) with the default preferred_range (1.8,
        // deliberately below the derived floor): preferred_max should land
        // at attack_range - SPRINT_IN_MARGIN, not steering's compiled-in
        // 1.9-block default -- the whole point of this fix.
        let tuning = tuning_for(KillbotConfig::default());
        assert_eq!(tuning.band.preferred_max, 3.0 - SPRINT_IN_MARGIN);
    }

    #[test]
    fn preferred_max_never_exceeds_attack_range() {
        let config = KillbotConfig {
            attack_range: 2.0,
            preferred_range: 10.0, // absurdly high on purpose
            ..KillbotConfig::default()
        };
        let tuning = tuning_for(config);
        assert!(tuning.band.preferred_max <= 2.0);
    }

    #[test]
    fn an_explicit_preferred_range_above_the_derived_floor_still_wins() {
        let config = KillbotConfig {
            attack_range: 3.0,
            preferred_range: 2.9, // above 3.0 - SPRINT_IN_MARGIN
            ..KillbotConfig::default()
        };
        let tuning = tuning_for(config);
        assert_eq!(tuning.band.preferred_max, 2.9);
    }

    #[test]
    fn chase_always_stays_past_preferred_max_so_the_eased_approach_zone_survives() {
        // A large configured attack_range used to be able to push
        // preferred_max past the compiled-in `chase` default (2.5),
        // collapsing `steering::desired_velocity`'s eased zone between them.
        let config = KillbotConfig {
            attack_range: 6.0,
            ..KillbotConfig::default()
        };
        let tuning = tuning_for(config);
        assert!(tuning.band.chase > tuning.band.preferred_max);
    }

    #[test]
    fn a_small_attack_range_does_not_shrink_chase_below_its_compiled_default() {
        let config = KillbotConfig {
            attack_range: 1.0,
            preferred_range: 0.5,
            ..KillbotConfig::default()
        };
        let tuning = tuning_for(config);
        assert_eq!(tuning.band.chase, MovementTuning::default().band.chase);
    }
}

#[cfg(test)]
mod mace_cadence_tests {
    use super::*;

    #[test]
    fn random_mace_attempt_hits_stays_within_the_configured_range() {
        let mut rng = crate::look::aim_point::SeededRng::new(1);
        for _ in 0..200 {
            let hits = random_mace_attempt_hits(&mut rng);
            assert!(
                (MACE_ATTEMPT_MIN_HITS..=MACE_ATTEMPT_MAX_HITS).contains(&hits),
                "{hits} outside [{MACE_ATTEMPT_MIN_HITS}, {MACE_ATTEMPT_MAX_HITS}]"
            );
        }
    }

    #[test]
    fn a_new_fight_starts_with_zero_hits_and_a_threshold_in_range() {
        let inner = Inner::new(1, KillbotConfig::default());
        assert_eq!(inner.hits_since_mace_attempt, 0);
        assert!(
            (MACE_ATTEMPT_MIN_HITS..=MACE_ATTEMPT_MAX_HITS).contains(&inner.next_mace_attempt_hits)
        );
    }

    #[test]
    fn a_deliberate_attempt_is_not_wanted_until_the_hit_threshold_is_reached() {
        let mut inner = Inner::new(1, KillbotConfig::default());
        inner.next_mace_attempt_hits = 4;
        inner.hits_since_mace_attempt = 3;
        let now = Instant::now();
        assert!(!wants_to_begin_wind_launch(&inner, true, true, now));
        inner.hits_since_mace_attempt = 4;
        assert!(wants_to_begin_wind_launch(&inner, true, true, now));
    }

    #[test]
    fn a_deliberate_attempt_is_never_wanted_with_prefer_mace_off() {
        let mut inner = Inner::new(
            1,
            KillbotConfig {
                prefer_mace: false,
                ..KillbotConfig::default()
            },
        );
        inner.hits_since_mace_attempt = 999;
        assert!(!wants_to_begin_wind_launch(&inner, true, true, Instant::now()));
    }

    #[tokio::test]
    async fn landing_a_mace_swing_rearms_the_counter_and_clears_the_smash_wait() {
        let mut inner = Inner::new(1, KillbotConfig::default());
        inner.hits_since_mace_attempt = 5;
        inner.held_weapon_is_mace = true;
        inner.awaiting_smash_since = Some(Instant::now());
        let now = Instant::now();
        swing(&minecraft_for_tests(), &mut inner, Uuid::nil(), now, false).await;
        assert_eq!(inner.hits_since_mace_attempt, 0);
        assert!(inner.awaiting_smash_since.is_none());
        assert!(
            (MACE_ATTEMPT_MIN_HITS..=MACE_ATTEMPT_MAX_HITS).contains(&inner.next_mace_attempt_hits)
        );
    }

    #[tokio::test]
    async fn an_ordinary_swing_counts_toward_the_next_attempt() {
        let mut inner = Inner::new(1, KillbotConfig::default());
        inner.hits_since_mace_attempt = 0;
        inner.held_weapon_is_mace = false;
        let now = Instant::now();
        swing(&minecraft_for_tests(), &mut inner, Uuid::nil(), now, false).await;
        assert_eq!(inner.hits_since_mace_attempt, 1);
    }

    fn minecraft_for_tests() -> MinecraftClient {
        MinecraftClient::new(
            crate::config::MinecraftConfig {
                server: "localhost:25565".to_owned(),
                username: "MagicBot".to_owned(),
                account_mode: crate::config::AccountMode::Offline,
            },
            crate::config::ReconnectConfig {
                enabled: false,
                delay_seconds: 10,
                maximum_attempts: 5,
            },
            crate::config::ConsoleConfig::default(),
            crate::config::WorldStateConfig::default(),
            crate::config::VerticalNavigationConfig::default(),
            crate::config::BridgingConfig::default(),
        )
    }
}
