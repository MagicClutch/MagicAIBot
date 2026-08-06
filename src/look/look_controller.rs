use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use tokio::sync::Mutex;

use crate::{
    blocks::{block_query::BlockSearchQuery, block_search::BlockSearchService},
    config::LookConfig,
    error::AppError,
    logging,
    look::{
        aim_point::{
            AimSelection, BlockAimMode, Hitbox, LookPrecision, RelativeAimPoint, SeededRng,
            aim_position, block_relative_point, hitbox_relative_point, new_selection,
        },
        interpolation::{
            RotationMotion, advance_motion, overshoot_rotation, should_overshoot, tolerance,
            within_tolerance,
        },
        look_target::LookTarget,
        reaction::ReactionTracker,
        rotation::{Rotation, rotation_towards},
    },
    minecraft::{
        client::{HitboxSnapshot, MinecraftClient},
        world_state::{BlockPosition, WorldStateSnapshot},
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LookState {
    #[default]
    Idle,
    Looking,
    Completed,
    #[allow(dead_code)]
    Cancelled,
    Failed,
}

/// Ownership of the camera. Higher-priority requests may temporarily replace
/// a lower-priority request without affecting movement ownership.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub enum LookPriority {
    #[default]
    NaturalIdle,
    #[allow(dead_code)]
    NavigationAssistance,
    #[allow(dead_code)]
    FollowTargetTracking,
    ExplicitCommand,
    PreciseInteraction,
}

#[derive(Clone, Debug, Default)]
pub struct LookSnapshot {
    pub state: LookState,
    pub target: Option<String>,
    pub precision: Option<LookPrecision>,
    pub priority: LookPriority,
    pub target_point: Option<String>,
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    pub yaw_speed: Option<f32>,
    pub pitch_speed: Option<f32>,
    pub started_at: Option<SystemTime>,
    pub failure_reason: Option<String>,
    pub generation: u64,
}

struct LookInner {
    snapshot: LookSnapshot,
    target: Option<LookTarget>,
    selection: Option<AimSelection>,
    motion: RotationMotion,
    overshoot: Option<Rotation>,
    reaction: ReactionTracker,
    operation_started_at: Instant,
    rng: SeededRng,
    suspended: Option<(LookTarget, LookPrecision, LookPriority)>,
}

#[derive(Clone)]
pub struct LookController {
    config: LookConfig,
    block_search: BlockSearchService,
    inner: Arc<Mutex<LookInner>>,
}

impl LookController {
    pub fn new(config: LookConfig, block_search: BlockSearchService) -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self::new_with_seed(config, block_search, seed)
    }
    pub fn new_with_seed(config: LookConfig, block_search: BlockSearchService, seed: u64) -> Self {
        Self {
            config,
            block_search,
            inner: Arc::new(Mutex::new(LookInner {
                snapshot: LookSnapshot::default(),
                target: None,
                selection: None,
                motion: RotationMotion::default(),
                overshoot: None,
                reaction: ReactionTracker::default(),
                operation_started_at: Instant::now(),
                rng: SeededRng::new(seed),
                suspended: None,
            })),
        }
    }
    pub async fn snapshot(&self) -> LookSnapshot {
        self.inner.lock().await.snapshot.clone()
    }
    /// Whether this controller is currently driving (or still correcting) a
    /// precise interaction aim and must therefore keep being ticked
    /// regardless of what the pathfinder is doing -- see [`keeps_ticking`].
    /// `LookSnapshot::precision` alone is not enough to answer this: it is
    /// only ever replaced by the next `look_at_with_precision` call, never
    /// cleared by `cancel()`/`release_precise()`, so it keeps reporting
    /// whatever precision the *last* look used even long after that look
    /// finished -- checking it without also checking `state` would treat
    /// every tick after any past precise interaction as still precise.
    pub async fn is_precise_active(&self) -> bool {
        let inner = self.inner.lock().await;
        keeps_ticking(inner.snapshot.state, inner.snapshot.precision)
    }
    pub async fn look_at(
        &self,
        minecraft: &MinecraftClient,
        target: LookTarget,
    ) -> Result<(), AppError> {
        self.look_at_with_precision(minecraft, target, LookPrecision::Natural)
            .await
    }
    pub async fn look_at_with_precision(
        &self,
        minecraft: &MinecraftClient,
        target: LookTarget,
        precision: LookPrecision,
    ) -> Result<(), AppError> {
        let label = target.label();
        let priority = if precision == LookPrecision::Precise {
            LookPriority::PreciseInteraction
        } else {
            LookPriority::ExplicitCommand
        };
        let generation = {
            let mut inner = self.inner.lock().await;
            if priority == LookPriority::PreciseInteraction
                && inner.snapshot.priority < LookPriority::PreciseInteraction
                && let (Some(previous_target), Some(previous_precision)) =
                    (inner.target.clone(), inner.snapshot.precision)
            {
                inner.suspended =
                    Some((previous_target, previous_precision, inner.snapshot.priority));
            }
            let generation = inner.snapshot.generation.wrapping_add(1);
            inner.target = Some(target);
            inner.selection = None;
            inner.motion = RotationMotion::default();
            inner.overshoot = None;
            inner.reaction.reset();
            inner.operation_started_at = Instant::now();
            inner.snapshot = LookSnapshot {
                state: LookState::Looking,
                target: Some(label.clone()),
                precision: Some(precision),
                priority,
                started_at: Some(SystemTime::now()),
                generation,
                ..LookSnapshot::default()
            };
            generation
        };
        if let Err(error) = self.tick_generation(minecraft, generation).await {
            self.fail(generation, error.to_string()).await;
            return Err(error);
        }
        logging::info(format!("Looking at {label}"));
        Ok(())
    }
    pub async fn look_at_block_id(
        &self,
        minecraft: &MinecraftClient,
        block_id: String,
    ) -> Result<(), AppError> {
        let query = BlockSearchQuery {
            block_id: block_id.clone(),
            radius: 64,
            maximum_results: 1,
            vertical_range: 0,
        };
        let Some(block) = self
            .block_search
            .search_raw(minecraft, query)
            .await?
            .into_iter()
            .next()
        else {
            return Err(AppError::LookTargetUnloaded);
        };
        self.look_at_with_precision(
            minecraft,
            LookTarget::Block {
                position: block.position,
                block_id: Some(block_id),
                aim_mode: BlockAimMode::VisibleSurface,
            },
            LookPrecision::Natural,
        )
        .await
    }
    pub async fn cancel(&self) {
        let mut inner = self.inner.lock().await;
        if inner.snapshot.state == LookState::Looking {
            logging::info("Look cancelled");
        }
        inner.snapshot.state = LookState::Idle;
        inner.target = None;
        inner.selection = None;
        inner.overshoot = None;
        inner.motion = RotationMotion::default();
        inner.reaction.reset();
        inner.suspended = None;
    }

    /// Restore the user-requested look after a precise interaction releases
    /// camera ownership. It intentionally does nothing when no task was
    /// displaced, keeping normal cancellation independent from movement.
    pub async fn release_precise(&self, minecraft: &MinecraftClient) -> Result<bool, AppError> {
        let previous = { self.inner.lock().await.suspended.take() };
        let Some((target, precision, _priority)) = previous else {
            self.cancel().await;
            return Ok(false);
        };
        self.look_at_with_precision(minecraft, target, precision)
            .await?;
        Ok(true)
    }
    pub async fn tick(&self, minecraft: &MinecraftClient) {
        let (generation, already_completed) = {
            let inner = self.inner.lock().await;
            if !keeps_ticking(inner.snapshot.state, inner.snapshot.precision) {
                return;
            }
            (
                inner.snapshot.generation,
                inner.snapshot.state == LookState::Completed,
            )
        };
        if let Err(error) = self.tick_generation(minecraft, generation).await {
            if already_completed {
                // This generation already reached its target once and is
                // only still ticking to hold the aim steady during mining
                // (see `keeps_ticking`). The interaction controller is about
                // to call `release_precise` the moment it observes the same
                // world change (e.g. the target block finally breaking) --
                // that is success, not failure, so don't overwrite a
                // completed look with `Failed` or log a misleading warning
                // for it.
                return;
            }
            self.fail(generation, error.to_string()).await;
            logging::warning(format!("Look failed: {error}"));
        }
    }

    async fn tick_generation(
        &self,
        minecraft: &MinecraftClient,
        generation: u64,
    ) -> Result<(), AppError> {
        let (target, precision) = {
            let inner = self.inner.lock().await;
            if inner.snapshot.generation != generation
                || !keeps_ticking(inner.snapshot.state, inner.snapshot.precision)
            {
                return Err(AppError::LookCancelled);
            }
            (
                inner
                    .target
                    .clone()
                    .ok_or(AppError::LookTargetDisappeared)?,
                inner.snapshot.precision.unwrap_or(LookPrecision::Natural),
            )
        };
        let world = minecraft.world_state_snapshot().await;
        let (eye, yaw, pitch) = minecraft.look_data().await?;
        let context = resolve_context(minecraft, &world, &target).await?;
        let (target_position, speed_factor, aim_label) = {
            let mut inner = self.inner.lock().await;
            if inner.snapshot.generation != generation {
                return Err(AppError::LookCancelled);
            }
            let now = SystemTime::now();
            let retry_seconds = self.config.randomization.minimum_hold_time_ms as f64 / 1000.0;
            let retarget_chance = 1.0
                - (1.0 - self.config.randomization.retarget_chance_per_second).powf(retry_seconds);
            let renew = inner.selection.is_none_or(|selection| {
                now >= selection.hold_until && inner.rng.chance(retarget_chance)
            });
            if renew {
                let relative =
                    choose_relative(eye, &context, precision, &self.config, &mut inner.rng);
                inner.selection = Some(new_selection(
                    relative,
                    &self.config.randomization,
                    self.config.motion.speed_variation,
                    &mut inner.rng,
                ));
            } else if inner
                .selection
                .is_some_and(|selection| now >= selection.hold_until)
            {
                inner.selection.as_mut().expect("checked").hold_until =
                    now + Duration::from_millis(self.config.randomization.minimum_hold_time_ms);
            }
            let selection = inner.selection.ok_or_else(|| {
                AppError::LookUnavailableWithReason("aim selection was not initialized".into())
            })?;
            let raw_target = aim_position(selection.relative, context.base, context.hitbox);
            let elapsed = inner.operation_started_at.elapsed();
            let LookInner { reaction, rng, .. } = &mut *inner;
            let target_position = reaction.update(
                elapsed,
                raw_target,
                context.tracks_movement && precision == LookPrecision::Natural,
                &self.config,
                rng,
            );
            (
                target_position,
                selection.speed_factor,
                context.label.clone(),
            )
        };
        let desired = rotation_towards(eye, target_position);
        let current = Rotation { yaw, pitch };
        let (next, completed, yaw_speed, pitch_speed) = {
            let mut inner = self.inner.lock().await;
            if inner.snapshot.generation != generation {
                return Err(AppError::LookCancelled);
            }
            if precision == LookPrecision::Instant {
                (desired, true, 0.0, 0.0)
            } else {
                if inner.overshoot.is_none()
                    && !inner.motion.overshoot_used
                    && should_overshoot(
                        current,
                        desired,
                        precision,
                        &self.config.motion,
                        &mut inner.rng,
                    )
                {
                    inner.overshoot = Some(overshoot_rotation(
                        current,
                        desired,
                        &self.config.motion,
                        &mut inner.rng,
                    ));
                    inner.motion.overshoot_used = true;
                }
                let active_target = inner.overshoot.unwrap_or(desired);
                let next = advance_motion(
                    current,
                    active_target,
                    &mut inner.motion,
                    &self.config.motion,
                    precision,
                    speed_factor,
                    self.config.update_rate,
                );
                if inner.overshoot.is_some_and(|overshoot| {
                    within_tolerance(next, overshoot, tolerance(&self.config.motion, precision))
                }) {
                    inner.overshoot = None;
                    inner.motion.yaw_velocity = 0.0;
                    inner.motion.pitch_velocity = 0.0;
                }
                let completed = !context.tracks_movement
                    && inner.overshoot.is_none()
                    && within_tolerance(next, desired, tolerance(&self.config.motion, precision));
                (
                    next,
                    completed,
                    inner.motion.yaw_velocity,
                    inner.motion.pitch_velocity,
                )
            }
        };
        minecraft.set_look_direction(next.yaw, next.pitch).await?;
        let mut inner = self.inner.lock().await;
        if inner.snapshot.generation != generation {
            return Err(AppError::LookCancelled);
        }
        inner.snapshot.yaw = Some(next.yaw);
        inner.snapshot.pitch = Some(next.pitch);
        inner.snapshot.yaw_speed = Some(yaw_speed.abs());
        inner.snapshot.pitch_speed = Some(pitch_speed.abs());
        inner.snapshot.target_point = Some(aim_label);
        if completed {
            // Edge-triggered: a `Precise` target keeps ticking (and
            // therefore keeps recomputing `completed = true`) for as long as
            // the caller holds it, well past the moment it first reached the
            // target -- see `keeps_ticking`'s doc comment. Logging
            // unconditionally here used to print "Rotation completed" on
            // every single tick for the entire hold duration of any caller
            // that holds a precise look for a while. Only the first tick
            // that reaches it is newsworthy.
            let already_completed = inner.snapshot.state == LookState::Completed;
            inner.snapshot.state = LookState::Completed;
            if !already_completed {
                // `info`, not `success`: a look completes once per aim (every
                // block broken/placed during a `#get`/`#mine` run re-aims),
                // so tagging it a milestone would flood chat with one
                // identical line per item and risk a spam kick.
                logging::info("Rotation completed");
            }
        }
        Ok(())
    }
    async fn fail(&self, generation: u64, reason: String) {
        let mut inner = self.inner.lock().await;
        if inner.snapshot.generation == generation {
            inner.snapshot.state = LookState::Failed;
            inner.snapshot.failure_reason = Some(reason);
        }
    }
}

/// Whether the controller should still be actively re-aiming this tick.
/// Ordinarily ticking stops the moment a look reaches `Completed` -- correct
/// for a one-shot `/look`, which is done at that point. A precise
/// interaction target (block-breaking, placing, tilling) is different: the
/// caller keeps mining/interacting for as long as it likes *after* seeing
/// this controller reach `Completed` once, and nothing else re-derives the
/// yaw/pitch needed to stay on the exact hit point during that whole time.
/// Without this, any movement correction while mining (a knockback tick, a
/// stumble on uneven terrain) leaves the camera frozen at a stale rotation
/// that no longer points at the block at all -- so a `Precise` target keeps
/// ticking (and therefore keeps correcting for drift) past `Completed`,
/// until the caller cancels or replaces it.
fn keeps_ticking(state: LookState, precision: Option<LookPrecision>) -> bool {
    state == LookState::Looking
        || (state == LookState::Completed && precision == Some(LookPrecision::Precise))
}

#[derive(Clone)]
struct AimContext {
    base: [f64; 3],
    hitbox: Option<Hitbox>,
    block: Option<(BlockPosition, BlockAimMode)>,
    player: bool,
    tracks_movement: bool,
    label: String,
}

fn choose_relative(
    eye: [f64; 3],
    context: &AimContext,
    precision: LookPrecision,
    config: &LookConfig,
    rng: &mut SeededRng,
) -> RelativeAimPoint {
    let natural = precision == LookPrecision::Natural && config.randomization.enabled;
    if let Some((position, mode)) = context.block {
        return block_relative_point(
            eye,
            position,
            mode,
            natural && config.randomization.block_randomization,
            &config.randomization,
            rng,
        );
    }
    if context.hitbox.is_some() {
        return hitbox_relative_point(
            context.player,
            natural
                && if context.player {
                    config.randomization.player_randomization
                } else {
                    config.randomization.entity_randomization
                },
            &config.randomization,
            rng,
        );
    }
    RelativeAimPoint::Fixed
}

async fn resolve_context(
    minecraft: &MinecraftClient,
    world: &WorldStateSnapshot,
    target: &LookTarget,
) -> Result<AimContext, AppError> {
    match target {
        LookTarget::World(position) => Ok(AimContext {
            base: [position.x, position.y, position.z],
            hitbox: None,
            block: None,
            player: false,
            tracks_movement: false,
            label: "world position".into(),
        }),
        LookTarget::Block {
            position,
            block_id,
            aim_mode,
        } => {
            let cells = minecraft.block_ids_at(&[*position]).await?;
            let actual = cells.get(position).and_then(Option::as_deref);
            if actual.is_none()
                || block_id
                    .as_deref()
                    .is_some_and(|expected| Some(expected) != actual)
            {
                return Err(AppError::LookTargetUnloaded);
            }
            Ok(AimContext {
                base: [
                    f64::from(position.x),
                    f64::from(position.y),
                    f64::from(position.z),
                ],
                hitbox: None,
                block: Some((*position, *aim_mode)),
                player: false,
                tracks_movement: false,
                label: "randomized block surface".into(),
            })
        }
        LookTarget::BlockFacePoint {
            position,
            block_id,
            point,
        } => {
            let cells = minecraft.block_ids_at(&[*position]).await?;
            let actual = cells.get(position).and_then(Option::as_deref);
            if actual.is_none()
                || block_id
                    .as_deref()
                    .is_some_and(|expected| Some(expected) != actual)
            {
                return Err(AppError::LookTargetUnloaded);
            }
            Ok(AimContext {
                base: *point,
                hitbox: None,
                block: None,
                player: false,
                tracks_movement: false,
                label: "block face hit point".into(),
            })
        }
        LookTarget::Entity(entity_id) => {
            world
                .entities
                .iter()
                .find(|entity| entity.entity_id == *entity_id)
                .ok_or(AppError::LookTargetDisappeared)?;
            let hitbox = minecraft.entity_hitbox(*entity_id).await?;
            context_from_hitbox(hitbox, false)
        }
        LookTarget::Player(name) => {
            let player = world
                .find_player_by_name(name)
                .ok_or(AppError::LookTargetDisappeared)?;
            let hitbox = minecraft.player_hitbox(player.uuid).await?;
            context_from_hitbox(hitbox, true)
        }
        LookTarget::MovementDirection => {
            let position = world.bot.position.ok_or(AppError::LookUnavailable)?;
            let yaw = f64::from(world.bot.yaw.unwrap_or_default()).to_radians();
            let pitch = f64::from(world.bot.pitch.unwrap_or_default()).to_radians();
            Ok(AimContext {
                base: [
                    position.x + yaw.sin() * pitch.cos() * 10.0,
                    position.y + 1.62 - pitch.sin() * 10.0,
                    position.z - yaw.cos() * pitch.cos() * 10.0,
                ],
                hitbox: None,
                block: None,
                player: false,
                tracks_movement: true,
                label: "movement direction".into(),
            })
        }
    }
}
fn context_from_hitbox(hitbox: HitboxSnapshot, player: bool) -> Result<AimContext, AppError> {
    if !(hitbox.width.is_finite() && hitbox.height.is_finite())
        || hitbox.width < 0.0
        || hitbox.height < 0.0
    {
        return Err(AppError::LookTargetDisappeared);
    }
    Ok(AimContext {
        base: hitbox.position,
        hitbox: Some(Hitbox {
            width: hitbox.width.max(0.01),
            height: hitbox.height.max(0.01),
        }),
        block: None,
        player,
        tracks_movement: true,
        label: if player {
            "randomized player hitbox point".into()
        } else {
            "randomized entity hitbox point".into()
        },
    })
}
