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
//! Known limitation, stated plainly rather than left implicit: this does
//! not do any terrain/hazard awareness (avoiding lava, cliffs, getting
//! trapped) -- combat movement is a straight line toward/away from the
//! target with no block queries in the loop at all. A pathfinding-aware
//! version of that would need to query terrain every tick the way
//! `crate::navigation` already does for normal movement, which risks
//! re-coupling combat back to the pathfinder this module deliberately
//! stays independent of (see `crate::combat`'s module doc comment). Out of
//! scope here; noted for anyone picking this up next.

use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::{
    combat::{
        crits, defense, heal,
        health::{self, CombatMode},
        movement,
        movement::StrafePattern,
        shield_break, state,
        targeting::{self, PREDICTION_LEAD_SECONDS},
    },
    config::{KillbotConfig, ToolRankingMode},
    equipment::{manager::swap_into_slot, model::HOTBAR_PROTOCOL_SLOTS, tools},
    interaction::tool_selection::{ToolCategory, category},
    logging,
    look::{LookController, LookTarget},
    minecraft::client::MinecraftClient,
};

/// How briefly sprint is released right after an attack -- the same
/// "sprint reset" (a w-tap) real PvP players use to regain the small
/// positioning/knockback edge of a freshly-started sprint rather than one
/// held continuously through the hit.
const SPRINT_RESET_DURATION: Duration = Duration::from_millis(120);

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

/// Health, as a fraction of an assumed vanilla 20-point maximum, below
/// which "Target health low" is reported once. Player max health can
/// technically be modified by attributes/effects, but this codebase has no
/// visibility into a *remote* player's actual maximum (only their current
/// health -- see `MinecraftClient::player_combat_status`), so 20 is the
/// closest available approximation, and also vanilla's own default.
const LOW_HEALTH_FRACTION: f32 = 0.3;
const ASSUMED_MAX_HEALTH: f32 = 20.0;

/// How far past the preferred band counts as "disengaging" while healing --
/// wider than `movement::AGGRESSIVE_CLOSE_DISTANCE`, matching the spec's
/// "create at least 4-6 blocks of distance" before eating.
const HEAL_DISENGAGE_DISTANCE: f64 = 5.0;

/// How long one `start_use_main_hand` eat action is assumed to occupy the
/// hand for, before it's safe to re-trigger (re-selecting the food slot or
/// re-sending use-item mid-bite both cancel the vanilla eat animation) --
/// close to a plain food item's real ~1.6s consume time, with a little
/// margin so this doesn't re-trigger a fraction of a second early and
/// cancel a still-finishing bite.
const EAT_RETRIGGER_INTERVAL: Duration = Duration::from_millis(1800);

pub(crate) struct Inner {
    pub(crate) snapshot: state::KillSnapshot,
    config: KillbotConfig,
    strafe_pattern: StrafePattern,
    strafe_since: Instant,
    strafe_interval: Duration,
    rng: crate::look::aim_point::SeededRng,
    last_attack: Option<Instant>,
    last_jump: Option<Instant>,
    sprint_released_at: Option<Instant>,
    engaged_logged: bool,
    health_low_logged: bool,
    look_target_set_for: Option<String>,
    /// Last tick the target was actually found in world state -- see
    /// [`handle_unresolved_target`]. `Some` from the moment a fight starts
    /// (resolving the player in `kill::KillController::start` counts as a
    /// sighting), never `None` again until the next fight.
    last_seen_at: Option<Instant>,
    /// Set the instant `#kill` last started an eat action; cleared once
    /// healing mode ends. See [`apply_healing`].
    eating_since: Option<Instant>,
    /// Whether the *previous* tick was in a disengage/heal state -- purely
    /// so the "Re-engaging target" transition logs exactly once, on the
    /// tick healing actually ends, rather than every tick afterward.
    was_healing: bool,
    /// Whether the bot's own shield was raised as of the previous tick --
    /// same one-shot-transition purpose as `was_healing`, for
    /// `crate::combat::defense`.
    shield_raised: bool,
    /// Whether the target appeared to be blocking as of the previous tick
    /// -- so "Shield broken" logs exactly once, on the tick blocking is
    /// first observed to have stopped, not every tick afterward.
    target_was_blocking: bool,
    /// Whether the bot is currently facing away from the target to flee at
    /// a sprint (see `targeting::flee_point`), rather than tracking them
    /// with `look::LookTarget::PredictedPlayer`. Reset the moment the
    /// fight reaches a safe distance again -- see [`apply_healing`].
    fleeing_look_set: bool,
}

impl Inner {
    pub(crate) fn new(seed: u64, config: KillbotConfig) -> Self {
        Self {
            snapshot: state::KillSnapshot::default(),
            config,
            strafe_pattern: StrafePattern::default(),
            strafe_since: Instant::now(),
            strafe_interval: movement::STRAFE_FLIP_INTERVAL,
            rng: crate::look::aim_point::SeededRng::new(seed),
            last_attack: None,
            last_jump: None,
            sprint_released_at: None,
            engaged_logged: false,
            health_low_logged: false,
            look_target_set_for: None,
            last_seen_at: None,
            eating_since: None,
            was_healing: false,
            shield_raised: false,
            target_was_blocking: false,
            fleeing_look_set: false,
        }
    }

    /// Resets every per-fight timer/flag, but not the snapshot itself --
    /// called by `kill::KillController::start` right before overwriting
    /// the snapshot, so a second `#kill` never inherits stale strafe
    /// timers, attack cooldowns, or "already logged" flags from a
    /// previous fight.
    pub(crate) fn reset_for_new_fight(&mut self) {
        self.strafe_pattern = StrafePattern::default();
        self.strafe_since = Instant::now();
        self.strafe_interval = movement::STRAFE_FLIP_INTERVAL;
        self.last_attack = None;
        self.last_jump = None;
        self.sprint_released_at = None;
        self.engaged_logged = false;
        self.health_low_logged = false;
        self.look_target_set_for = None;
        self.eating_since = None;
        self.was_healing = false;
        self.shield_raised = false;
        self.target_was_blocking = false;
        self.fleeing_look_set = false;
        // `start()` only reaches this point after already confirming the
        // player is resolvable right now -- that lookup itself counts as
        // the first sighting.
        self.last_seen_at = Some(Instant::now());
    }
}

pub(crate) async fn tick(minecraft: &MinecraftClient, look: &LookController, inner: &mut Inner) {
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
        disconnect(minecraft, look, inner, &name).await;
        return;
    };
    let Some(target_position) = player.position else {
        handle_unresolved_target(minecraft, look, inner, &name).await;
        return;
    };
    let target_uuid = player.uuid;

    let Ok(status) = minecraft.player_combat_status(target_uuid).await else {
        handle_unresolved_target(minecraft, look, inner, &name).await;
        return;
    };
    if !status.alive {
        eliminate(minecraft, look, inner, &name).await;
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
    if let Some(health) = status.health
        && !inner.health_low_logged
        && health <= ASSUMED_MAX_HEALTH * LOW_HEALTH_FRACTION
    {
        inner.health_low_logged = true;
        logging::progress("Target health low");
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
        abort(minecraft, look, inner, &name, "target out of chase range").await;
        return;
    }

    let bot_health = f64::from(world.bot.health.unwrap_or(ASSUMED_MAX_HEALTH));
    let mode = health::mode_for_health(bot_health, inner.config.heal_threshold);
    inner.snapshot.mode = mode;
    let healing_active = matches!(mode, CombatMode::Defensive | CombatMode::Critical)
        && bot_health < inner.config.reengage_threshold;

    if healing_active {
        if !inner.was_healing {
            inner.was_healing = true;
            logging::milestone("Entering defensive mode");
        }
        apply_healing(
            minecraft,
            look,
            inner,
            &name,
            bot_position,
            target_position,
            distance,
        )
        .await;
    } else {
        if inner.was_healing {
            inner.was_healing = false;
            inner.eating_since = None;
            inner.fleeing_look_set = false;
            logging::milestone("Health recovered");
            logging::milestone("Re-engaging target");
        }
        track_aim(minecraft, look, inner, &name).await;
        apply_movement(
            minecraft,
            inner,
            distance,
            world.bot.on_ground.unwrap_or(true),
            world.bot.horizontal_collision.unwrap_or(false),
        )
        .await;
        if inner.config.shield_break_enabled {
            apply_shield_response(
                minecraft,
                inner,
                world.bot.selected_hotbar_slot,
                status.using_item,
            )
            .await;
        }
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
        if !matches!(inner.snapshot.phase, state::CombatPhase::Engage) {
            inner.snapshot.phase = compute_active_phase(distance);
        }
    }

    if inner.config.shield_use_enabled {
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
    look: &LookController,
    inner: &mut Inner,
    name: &str,
) {
    stop_all(minecraft, look).await;
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
    look: &LookController,
    inner: &mut Inner,
    name: &str,
) {
    let stale = inner
        .last_seen_at
        .is_none_or(|last_seen| targeting::is_stale(last_seen.elapsed().as_secs_f64()));
    if stale {
        lose_target(minecraft, look, inner, name).await;
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

async fn apply_movement(
    minecraft: &MinecraftClient,
    inner: &mut Inner,
    distance: f64,
    on_ground: bool,
    horizontal_collision: bool,
) {
    if movement::should_flip_strafe(inner.strafe_since.elapsed(), inner.strafe_interval) {
        inner.strafe_pattern = movement::next_strafe_pattern(inner.strafe_pattern, &mut inner.rng);
        inner.strafe_since = Instant::now();
        inner.strafe_interval = movement::next_flip_interval(&mut inner.rng);
    }
    let mut decision = movement::decide_movement(distance, inner.strafe_pattern);
    if !inner.config.strafe_enabled {
        decision.strafe = None;
    }
    let sprint_reset_active = inner.config.sprint_reset_enabled
        && inner
            .sprint_released_at
            .is_some_and(|released_at| released_at.elapsed() < SPRINT_RESET_DURATION);
    let sprint = decision.sprint && !sprint_reset_active;
    let _ = minecraft
        .set_combat_walk(decision.walk_direction(), sprint)
        .await;
    // This bot's own raw movement has no other terrain awareness (see this
    // module's doc comment) -- without an explicit jump here, a single
    // block-high step, stair, or fence in the way would leave it pushing
    // uselessly against it forever instead of chasing.
    if movement::should_jump_over_obstacle(decision.forward, on_ground, horizontal_collision) {
        let _ = minecraft.combat_jump_once().await;
    }
}

/// The whole "getting low" flow: while the target is closer than
/// [`HEAL_DISENGAGE_DISTANCE`], flee -- face directly away from them and
/// sprint (see `targeting::flee_point`'s doc comment for why that, not a
/// `Backward` walk, is what actually outruns a chasing target) -- and
/// don't eat yet, no matter how long it's been. Only once genuinely clear
/// (`distance >= HEAL_DISENGAGE_DISTANCE`, the spec's "only eat once
/// [safely] away") does this stop advancing, resume tracking the target
/// with the normal look controller (to notice them closing back in), and
/// actually start eating.
///
/// Deliberately does *not* touch weapon selection while eating (see
/// [`apply_shield_response`]'s call site being skipped entirely while
/// healing): re-selecting a hotbar slot mid-bite cancels the vanilla eat
/// animation, so the combat weapon just stays wherever it already was
/// until healing ends.
async fn apply_healing(
    minecraft: &MinecraftClient,
    look: &LookController,
    inner: &mut Inner,
    name: &str,
    bot_position: crate::minecraft::world_state::PositionSnapshot,
    target_position: crate::minecraft::world_state::PositionSnapshot,
    distance: f64,
) {
    if movement::should_flip_strafe(inner.strafe_since.elapsed(), inner.strafe_interval) {
        inner.strafe_pattern = movement::next_strafe_pattern(inner.strafe_pattern, &mut inner.rng);
        inner.strafe_since = Instant::now();
        inner.strafe_interval = movement::next_flip_interval(&mut inner.rng);
    }
    let strafe = if inner.config.strafe_enabled {
        inner.strafe_pattern.side()
    } else {
        None
    };
    let safe = distance >= HEAL_DISENGAGE_DISTANCE;

    if safe {
        inner.fleeing_look_set = false;
        track_aim(minecraft, look, inner, name).await;
        let decision = movement::MovementDecision {
            forward: false,
            backward: false,
            strafe,
            sprint: false,
        };
        let _ = minecraft
            .set_combat_walk(decision.walk_direction(), false)
            .await;
    } else {
        if !inner.fleeing_look_set {
            let flee = targeting::flee_point(bot_position, target_position);
            if look
                .look_at(minecraft, LookTarget::World(flee))
                .await
                .is_ok()
            {
                inner.fleeing_look_set = true;
                // Force `track_aim` to resubmit fresh once safe/re-engaging
                // rather than believing it's still tracking the target --
                // the look controller's actual target is `World` now, not
                // `PredictedPlayer`, regardless of what this cache says.
                inner.look_target_set_for = None;
            }
        }
        let decision = movement::MovementDecision {
            forward: true,
            backward: false,
            strafe,
            sprint: true,
        };
        let _ = minecraft
            .set_combat_walk(decision.walk_direction(), true)
            .await;
        inner.snapshot.phase = state::CombatPhase::Defensive;
        // "Only eat once [safely] away" -- don't even attempt it yet.
        return;
    }

    let ready_to_eat_again = inner
        .eating_since
        .is_none_or(|since| since.elapsed() >= EAT_RETRIGGER_INTERVAL);
    if !ready_to_eat_again {
        inner.snapshot.phase = state::CombatPhase::Heal;
        return;
    }
    let Ok(food) = minecraft.food_snapshot().await else {
        inner.snapshot.phase = state::CombatPhase::Defensive;
        return;
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
        // Nothing to eat -- stay in Defensive posture and just wait for
        // `reengage_threshold` via regeneration instead.
        inner.snapshot.phase = state::CombatPhase::Defensive;
        return;
    };
    let label = best.item_id.to_owned();
    let equipped = if HOTBAR_PROTOCOL_SLOTS.contains(&best.slot) {
        let hotbar_index = (best.slot - HOTBAR_PROTOCOL_SLOTS.start()) as u8;
        minecraft.select_hotbar_slot(hotbar_index).await.is_ok()
    } else {
        swap_into_slot(minecraft, best.slot, COMBAT_FOOD_PROTOCOL_SLOT).await
            && minecraft
                .select_hotbar_slot(COMBAT_FOOD_HOTBAR_INDEX)
                .await
                .is_ok()
    };
    if equipped && minecraft.start_use_main_hand().await.is_ok() {
        inner.eating_since = Some(Instant::now());
        inner.snapshot.phase = state::CombatPhase::Heal;
        logging::progress(format!("Eating {}", crate::blocks::bare_id(&label)));
    } else {
        inner.snapshot.phase = state::CombatPhase::Defensive;
    }
}

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
async fn apply_shield_response(
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
        return;
    }
    let now = Instant::now();
    if !crits::attack_ready(inner.last_attack, now) {
        return;
    }
    if crits::is_critical_window(on_ground, velocity_y) {
        swing(minecraft, inner, target_uuid, now, true).await;
        return;
    }
    if !on_ground {
        // Still rising (or at the apex) from a crit-seeking jump --
        // `is_critical_window` above didn't match, so attacking *right
        // now* would land as a plain, non-critical hit instead. Wait
        // rather than swinging early: `last_attack` is untouched, so the
        // cooldown stays exactly as ready as it is now, and the very next
        // tick that reports falling (`velocity_y < 0`) lands the crit this
        // jump was for. Previously this fell through to an immediate
        // non-crit swing the tick right after every jump, which is why
        // crits only ever landed by accident, on the way back down, never
        // "right after the jump" the way a real jump-crit combo does.
        return;
    }
    if inner.config.crit_enabled
        && crits::should_jump_for_crit(on_ground, within_attack_range, inner.last_jump, now)
    {
        inner.last_jump = Some(now);
        inner.snapshot.phase = state::CombatPhase::CritPrep;
        let _ = minecraft.combat_jump_once().await;
        return;
    }
    swing(minecraft, inner, target_uuid, now, false).await;
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
    inner.sprint_released_at = Some(now);
    inner.snapshot.hits_landed += 1;
    inner.snapshot.phase = state::CombatPhase::Attack;
    // Combo pressure: circle noticeably faster right after landing a hit
    // instead of settling back into the lazier baseline cadence -- see
    // `movement::COMBO_STRAFE_FLIP_INTERVAL`'s doc comment.
    inner.strafe_interval = movement::COMBO_STRAFE_FLIP_INTERVAL;
    inner.strafe_since = now;
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
    look: &LookController,
    inner: &mut Inner,
    name: &str,
) {
    stop_all(minecraft, look).await;
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
    look: &LookController,
    inner: &mut Inner,
    name: &str,
    reason: &str,
) {
    stop_all(minecraft, look).await;
    inner.snapshot.state = state::KillState::Failed;
    inner.snapshot.phase = state::CombatPhase::Abort;
    inner.snapshot.failure_reason = Some(format!("Target lost: {name} ({reason})"));
}

async fn eliminate(
    minecraft: &MinecraftClient,
    look: &LookController,
    inner: &mut Inner,
    name: &str,
) {
    stop_all(minecraft, look).await;
    inner.snapshot.state = state::KillState::Completed;
    inner.snapshot.phase = state::CombatPhase::Finish;
    logging::success(format!("Target eliminated: {name}"));
}

/// Releases raw movement input, any raised shield, and the camera --
/// shared by every terminal transition (`lose_target`, `abort`,
/// `eliminate`, `disconnect`) and by `kill::KillController::cancel`.
pub(crate) async fn stop_all(minecraft: &MinecraftClient, look: &LookController) {
    let _ = minecraft
        .set_combat_walk(movement::CombatWalk::None, false)
        .await;
    let _ = minecraft.release_use_item().await;
    look.cancel().await;
}
