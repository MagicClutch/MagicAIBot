use std::{sync::Arc, time::SystemTime};

use tokio::sync::Mutex;

use crate::{
    blocks::{block_query::BlockSearchQuery, block_search::BlockSearchService},
    config::LookConfig,
    error::AppError,
    logging,
    look::{
        interpolation::{interpolate, within_tolerance},
        look_target::LookTarget,
        rotation::{Rotation, rotation_towards},
    },
    minecraft::{
        client::MinecraftClient,
        world_state::{BlockPosition, PositionSnapshot},
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LookState {
    #[default]
    Idle,
    Looking,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct LookSnapshot {
    pub state: LookState,
    pub target: Option<String>,
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    pub started_at: Option<SystemTime>,
    pub failure_reason: Option<String>,
    pub generation: u64,
}

struct LookInner {
    snapshot: LookSnapshot,
    target: Option<LookTarget>,
}

#[derive(Clone)]
pub struct LookController {
    config: LookConfig,
    block_search: BlockSearchService,
    inner: Arc<Mutex<LookInner>>,
}

impl LookController {
    pub fn new(config: LookConfig, block_search: BlockSearchService) -> Self {
        Self {
            config,
            block_search,
            inner: Arc::new(Mutex::new(LookInner {
                snapshot: LookSnapshot::default(),
                target: None,
            })),
        }
    }

    pub async fn snapshot(&self) -> LookSnapshot {
        self.inner.lock().await.snapshot.clone()
    }

    pub async fn look_at(
        &self,
        minecraft: &MinecraftClient,
        target: LookTarget,
    ) -> Result<(), AppError> {
        let label = target.label();
        self.look_at_labeled(minecraft, target, label).await
    }

    async fn look_at_labeled(
        &self,
        minecraft: &MinecraftClient,
        target: LookTarget,
        label: String,
    ) -> Result<(), AppError> {
        let generation = {
            let mut inner = self.inner.lock().await;
            let generation = inner.snapshot.generation.wrapping_add(1);
            inner.target = Some(target);
            inner.snapshot = LookSnapshot {
                state: LookState::Looking,
                target: Some(label.clone()),
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
        self.look_at_labeled(
            minecraft,
            LookTarget::Block {
                position: block.position,
                block_id: Some(block_id.clone()),
            },
            block_id,
        )
        .await
    }

    pub async fn cancel(&self) {
        let mut inner = self.inner.lock().await;
        if inner.snapshot.state == LookState::Looking {
            inner.snapshot.state = LookState::Cancelled;
            logging::info("Look cancelled");
        }
        inner.snapshot.state = LookState::Idle;
        inner.target = None;
    }

    pub async fn tick(&self, minecraft: &MinecraftClient) {
        let generation = {
            let inner = self.inner.lock().await;
            if inner.snapshot.state != LookState::Looking {
                return;
            }
            inner.snapshot.generation
        };
        if let Err(error) = self.tick_generation(minecraft, generation).await {
            self.fail(generation, error.to_string()).await;
            logging::warning(format!("Look failed: {error}"));
        }
    }

    async fn tick_generation(
        &self,
        minecraft: &MinecraftClient,
        generation: u64,
    ) -> Result<(), AppError> {
        let target = {
            let inner = self.inner.lock().await;
            if inner.snapshot.generation != generation || inner.snapshot.state != LookState::Looking
            {
                return Err(AppError::LookCancelled);
            }
            inner
                .target
                .clone()
                .ok_or(AppError::LookTargetDisappeared)?
        };
        let world = minecraft.world_state_snapshot().await;
        let target_position = resolve_target(minecraft, &world, &target).await?;
        let (eye, yaw, pitch) = minecraft.look_data().await?;
        let desired = rotation_towards(eye, target_position);
        let current = Rotation { yaw, pitch };
        let next = interpolate(
            current,
            desired,
            self.config.maximum_yaw_speed as f32,
            self.config.maximum_pitch_speed as f32,
            self.config.update_rate as f32,
        );
        minecraft.set_look_direction(next.yaw, next.pitch).await?;
        let mut inner = self.inner.lock().await;
        if inner.snapshot.generation != generation {
            return Err(AppError::LookCancelled);
        }
        inner.snapshot.yaw = Some(next.yaw);
        inner.snapshot.pitch = Some(next.pitch);
        if within_tolerance(next, desired, self.config.arrival_tolerance as f32) {
            inner.snapshot.state = LookState::Completed;
            logging::success("Rotation completed");
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

async fn resolve_target(
    minecraft: &MinecraftClient,
    world: &crate::minecraft::world_state::WorldStateSnapshot,
    target: &LookTarget,
) -> Result<[f64; 3], AppError> {
    match target {
        LookTarget::World(position) => Ok([position.x, position.y, position.z]),
        LookTarget::Block { position, block_id } => {
            let cells = minecraft.block_ids_at(&[*position]).await?;
            let actual = cells.get(position).and_then(Option::as_deref);
            if actual.is_none()
                || block_id
                    .as_deref()
                    .is_some_and(|expected| Some(expected) != actual)
            {
                return Err(AppError::LookTargetUnloaded);
            }
            Ok([
                f64::from(position.x) + 0.5,
                f64::from(position.y) + 0.5,
                f64::from(position.z) + 0.5,
            ])
        }
        LookTarget::Entity(entity_id) => world
            .entities
            .iter()
            .find(|entity| entity.entity_id == *entity_id)
            .map(|entity| {
                [
                    entity.position.x,
                    entity.position.y + 0.5,
                    entity.position.z,
                ]
            })
            .ok_or(AppError::LookTargetDisappeared),
        LookTarget::Player(name) => world
            .find_player_by_name(name)
            .and_then(|player| player.position)
            .map(|position| [position.x, position.y + 1.62, position.z])
            .ok_or(AppError::LookTargetDisappeared),
        LookTarget::MovementDirection => {
            let position = world.bot.position.ok_or(AppError::LookUnavailable)?;
            let yaw = f64::from(world.bot.yaw.unwrap_or_default()).to_radians();
            let pitch = f64::from(world.bot.pitch.unwrap_or_default()).to_radians();
            Ok([
                position.x + yaw.sin() * pitch.cos() * 10.0,
                position.y + 1.62 - pitch.sin() * 10.0,
                position.z - yaw.cos() * pitch.cos() * 10.0,
            ])
        }
    }
}

#[allow(dead_code)]
fn _block_position_center(position: BlockPosition) -> PositionSnapshot {
    PositionSnapshot {
        x: f64::from(position.x) + 0.5,
        y: f64::from(position.y) + 0.5,
        z: f64::from(position.z) + 0.5,
    }
}
