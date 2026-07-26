use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, SystemTime},
};

use tokio::sync::Mutex;
use tracing::debug;

use crate::{
    blocks::{block_query::BlockSearchQuery, block_search::BlockSearchService},
    config::InteractionConfig,
    error::AppError,
    interaction::{
        block_breaking::{KeepCurrentTool, ToolSelector},
        faces::{BlockFace, BlockFacePurpose, best_face, face_hit_points},
        placement_rules::{has_support, is_air, is_replaceable},
        progress::estimated_progress,
        reach::{distance_to_block_center, within_reach},
    },
    logging,
    look::{LookController, LookTarget, aim_point::LookPrecision},
    minecraft::{
        client::{MinecraftClient, RaycastFace},
        world_state::{BlockPosition, InventorySnapshot, PositionSnapshot},
    },
    movement::MovementService,
    navigation::{BlockNavigationService, navigation_state::BlockNavigationState},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InteractionState {
    #[default]
    Idle,
    Moving,
    Looking,
    Preparing,
    Breaking,
    Placing,
    Retrying,
    Completed,
    #[allow(dead_code)]
    Cancelled,
    Failed,
}
#[derive(Clone, Debug, Default)]
pub struct InteractionSnapshot {
    pub state: InteractionState,
    pub target: Option<String>,
    pub progress_percent: Option<u8>,
    pub distance: Option<f64>,
    pub started_at: Option<SystemTime>,
    pub retries: u32,
    pub failure_reason: Option<String>,
}
#[derive(Clone, Debug)]
enum Operation {
    Break {
        target: BlockPosition,
        expected: String,
        face: BlockFace,
    },
    Place {
        target: BlockPosition,
        item_id: String,
        support: BlockPosition,
        support_id: String,
        face: BlockFace,
    },
}
struct Inner {
    snapshot: InteractionSnapshot,
    operation: Option<Operation>,
    dispatched: bool,
    retry_at: Option<SystemTime>,
    restore_after_break: Option<(BlockPosition, String)>,
    failure_logged: bool,
    face_attempt: usize,
    hit_point_attempt: usize,
    failed_hit_points: HashSet<(BlockPosition, BlockFace, usize)>,
}
#[derive(Clone)]
pub struct InteractionController {
    config: InteractionConfig,
    search: BlockSearchService,
    navigation: BlockNavigationService,
    inner: Arc<Mutex<Inner>>,
    tool_selector: Arc<dyn ToolSelector>,
}

impl InteractionController {
    pub fn new(
        config: InteractionConfig,
        search: BlockSearchService,
        navigation: BlockNavigationService,
    ) -> Self {
        Self {
            config,
            search,
            navigation,
            inner: Arc::new(Mutex::new(Inner {
                snapshot: InteractionSnapshot::default(),
                operation: None,
                dispatched: false,
                retry_at: None,
                restore_after_break: None,
                failure_logged: false,
                face_attempt: 0,
                hit_point_attempt: 0,
                failed_hit_points: HashSet::new(),
            })),
            tool_selector: Arc::new(KeepCurrentTool),
        }
    }
    /// Inject Task 3's eventual tool policy without coupling block-breaking
    /// lifecycle code to inventory scoring.
    #[allow(dead_code)] // exercised by the next inventory-policy integration task
    pub fn with_tool_selector(mut self, selector: Arc<dyn ToolSelector>) -> Self {
        self.tool_selector = selector;
        self
    }
    pub async fn snapshot(&self) -> InteractionSnapshot {
        self.inner.lock().await.snapshot.clone()
    }
    pub async fn break_looked(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
    ) -> Result<(), AppError> {
        let target = minecraft.looked_block().await?.position;
        self.break_at(minecraft, movement, look, target).await
    }
    pub async fn break_nearest(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        block_id: String,
    ) -> Result<(), AppError> {
        let Some(candidate) = self
            .search
            .search_raw(
                minecraft,
                BlockSearchQuery {
                    block_id,
                    radius: 32,
                    maximum_results: 1,
                    vertical_range: 0,
                },
            )
            .await?
            .into_iter()
            .next()
        else {
            return Err(AppError::NoMatchingBlock);
        };
        self.break_at(minecraft, movement, look, candidate.position)
            .await
    }
    pub async fn test_oak_log(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
    ) -> Result<(), AppError> {
        let Some(candidate) = self
            .search
            .search_raw(
                minecraft,
                BlockSearchQuery {
                    block_id: "minecraft:oak_log".into(),
                    radius: 32,
                    maximum_results: 1,
                    vertical_range: 0,
                },
            )
            .await?
            .into_iter()
            .next()
        else {
            return Err(AppError::NoMatchingBlock);
        };
        let original = block_id(minecraft, candidate.position)
            .await?
            .ok_or(AppError::TargetChunkUnloaded)?;
        self.break_at(minecraft, movement, look, candidate.position)
            .await?;
        self.inner.lock().await.restore_after_break = Some((candidate.position, original));
        Ok(())
    }
    pub async fn break_at(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        target: BlockPosition,
    ) -> Result<(), AppError> {
        let id = block_id(minecraft, target)
            .await?
            .ok_or(AppError::TargetChunkUnloaded)?;
        if is_air(Some(&id)) {
            return Err(AppError::CannotBreakAir);
        }
        self.replace(
            Operation::Break {
                target,
                expected: id,
                face: best_face(target, world_position(minecraft).await?),
            },
            minecraft,
            movement,
            look,
        )
        .await
    }
    pub async fn place_looked(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        item_id: String,
    ) -> Result<(), AppError> {
        let support = minecraft.looked_block().await?.position;
        let world = minecraft.world_state_snapshot().await;
        let face = best_face(
            support,
            world
                .bot
                .position
                .map(|p| [p.x, p.y, p.z])
                .unwrap_or([0.0; 3]),
        );
        self.place_at(minecraft, movement, look, face.neighbor(support), item_id)
            .await
    }
    pub async fn place_at(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        target: BlockPosition,
        item_id: String,
    ) -> Result<(), AppError> {
        let world = minecraft.world_state_snapshot().await;
        if !world.inventory.has_item(&item_id, 1) {
            return Err(AppError::InteractionItemMissing(item_id));
        }
        if world.bot.block_position == Some(target) {
            return Err(AppError::CannotPlaceBlock(
                "target intersects the bot".into(),
            ));
        }
        if world.entities.iter().any(|entity| {
            entity.position.x.floor() as i32 == target.x
                && entity.position.y.floor() as i32 == target.y
                && entity.position.z.floor() as i32 == target.z
        }) || world
            .players
            .iter()
            .filter_map(|player| player.position)
            .any(|position| {
                position.x.floor() as i32 == target.x
                    && position.y.floor() as i32 == target.y
                    && position.z.floor() as i32 == target.z
            })
        {
            return Err(AppError::CannotPlaceBlock(
                "target intersects an entity".into(),
            ));
        }
        let target_id = block_id(minecraft, target).await?;
        if !is_replaceable(target_id.as_deref()) {
            return Err(AppError::CannotPlaceBlock(
                "target is not replaceable".into(),
            ));
        }
        let position = world
            .bot
            .position
            .ok_or(AppError::BlockSearchPositionUnavailable)?;
        let (support, face) = find_support(minecraft, target, position)
            .await?
            .ok_or_else(|| AppError::CannotPlaceBlock("no support block".into()))?;
        let support_id = block_id(minecraft, support)
            .await?
            .ok_or(AppError::TargetChunkUnloaded)?;
        self.replace(
            Operation::Place {
                target,
                item_id,
                support,
                support_id,
                face,
            },
            minecraft,
            movement,
            look,
        )
        .await
    }
    async fn replace(
        &self,
        operation: Operation,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
    ) -> Result<(), AppError> {
        self.cancel(minecraft, movement, look).await;
        let target = match &operation {
            Operation::Break { expected, .. } => expected.clone(),
            Operation::Place { item_id, .. } => item_id.clone(),
        };
        let block_position = match &operation {
            Operation::Break { target, .. } | Operation::Place { target, .. } => *target,
        };
        let world = minecraft.world_state_snapshot().await;
        let distance = world
            .bot
            .position
            .map(|position| distance_to_block_center(position, block_position));
        let mut inner = self.inner.lock().await;
        inner.snapshot = InteractionSnapshot {
            state: InteractionState::Preparing,
            target: Some(target),
            distance,
            started_at: Some(SystemTime::now()),
            ..InteractionSnapshot::default()
        };
        inner.operation = Some(operation);
        inner.dispatched = false;
        inner.retry_at = None;
        inner.failure_logged = false;
        inner.face_attempt = 0;
        inner.hit_point_attempt = 0;
        inner.failed_hit_points.clear();
        Ok(())
    }
    pub async fn cancel(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
    ) {
        // Interaction is the only subsystem allowed to stop the controls it
        // borrowed.  Console movement and look commands call this defensively
        // when replacing an interaction, so an idle controller must be a
        // no-op rather than cancelling an unrelated task.
        let active = {
            let inner = self.inner.lock().await;
            inner.operation.is_some()
                && matches!(
                    inner.snapshot.state,
                    InteractionState::Moving
                        | InteractionState::Looking
                        | InteractionState::Preparing
                        | InteractionState::Breaking
                        | InteractionState::Placing
                        | InteractionState::Retrying
                )
        };
        if !active {
            return;
        }
        let _ = movement.stop(minecraft).await;
        self.navigation.cancel(minecraft, movement).await;
        let _ = look.release_precise(minecraft).await;
        minecraft.stop_breaking().await;
        let mut inner = self.inner.lock().await;
        if !matches!(
            inner.snapshot.state,
            InteractionState::Idle | InteractionState::Completed | InteractionState::Failed
        ) {
            logging::info("Interaction cancelled");
        }
        inner.snapshot.state = InteractionState::Idle;
        inner.operation = None;
        inner.dispatched = false;
        inner.retry_at = None;
        inner.restore_after_break = None;
        inner.failure_logged = false;
        inner.face_attempt = 0;
        inner.hit_point_attempt = 0;
        inner.failed_hit_points.clear();
    }
    pub async fn tick(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
    ) {
        let (state, operation, dispatched, retry_at) = {
            let inner = self.inner.lock().await;
            (
                inner.snapshot.state,
                inner.operation.clone(),
                inner.dispatched,
                inner.retry_at,
            )
        };
        let Some(operation) = operation else {
            return;
        };
        if state == InteractionState::Retrying {
            if retry_at.is_some_and(|at| SystemTime::now() < at) {
                return;
            }
            self.prepare().await;
            return;
        }
        let target = match &operation {
            Operation::Break { target, .. } | Operation::Place { target, .. } => *target,
        };
        let world = minecraft.world_state_snapshot().await;
        if world.connection.state != crate::minecraft::world_state::WorldConnectionState::Connected
        {
            self.stop_and_fail(minecraft, movement, look, "disconnected during interaction")
                .await;
            return;
        }
        if world.bot.alive == Some(false) || world.bot.health.is_some_and(|health| health <= 0.0) {
            self.stop_and_fail(minecraft, movement, look, "bot died during interaction")
                .await;
            return;
        }
        let reach = if matches!(&operation, Operation::Break { .. }) {
            self.config.breaking_reach
        } else {
            self.config.placement_reach
        };
        let in_reach = within_reach(world.bot.position, target, reach);
        if !in_reach
            && matches!(
                state,
                InteractionState::Looking | InteractionState::Breaking
            )
        {
            self.stop_and_fail(
                minecraft,
                movement,
                look,
                "target moved out of interaction range",
            )
            .await;
            return;
        }
        if !in_reach
            && matches!(
                state,
                InteractionState::Preparing | InteractionState::Moving
            )
        {
            if !self.config.auto_navigate {
                self.fail("target is out of reach").await;
                return;
            }
            if state != InteractionState::Moving {
                let navigation_target = match &operation {
                    Operation::Break {
                        target, expected, ..
                    } => Some((*target, expected.clone())),
                    Operation::Place {
                        support,
                        support_id,
                        ..
                    } => Some((*support, support_id.clone())),
                };
                let Some((navigation_position, navigation_block_id)) = navigation_target else {
                    self.fail("navigation support disappeared").await;
                    return;
                };
                if let Err(error) = self
                    .navigation
                    .start_exact(
                        minecraft,
                        movement,
                        navigation_position,
                        navigation_block_id,
                    )
                    .await
                {
                    self.fail(&navigation_failure_reason(&error)).await;
                    return;
                }
                self.set_state(InteractionState::Moving).await;
                logging::info("Moving to interaction target");
            }
            return;
        }
        if state == InteractionState::Moving {
            let navigation = self.navigation.snapshot().await;
            let navigation_state = navigation.state;
            if navigation_state == BlockNavigationState::Failed {
                self.fail(
                    navigation
                        .failure_reason
                        .as_deref()
                        .unwrap_or("no safe reachable interaction position"),
                )
                .await;
            } else if navigation_state == BlockNavigationState::Reached && in_reach {
                if self.operation_still_valid(minecraft, &operation).await {
                    self.prepare().await;
                } else {
                    self.fail("target changed while navigating").await;
                }
            }
            return;
        }
        if state == InteractionState::Preparing {
            if self.config.auto_precise_look {
                let (look_position, expected_id, base_face) = match &operation {
                    Operation::Break {
                        target,
                        expected,
                        face,
                    } => (*target, Some(expected.clone()), *face),
                    Operation::Place {
                        support,
                        support_id,
                        face,
                        ..
                    } => (*support, Some(support_id.clone()), *face),
                };
                let (face_attempt, hit_point_attempt) = {
                    let inner = self.inner.lock().await;
                    (inner.face_attempt, inner.hit_point_attempt)
                };
                let face = match &operation {
                    Operation::Break { .. } => break_face_order(base_face)[face_attempt % 6],
                    Operation::Place { .. } => base_face,
                };
                let points = face_hit_points(
                    look_position,
                    face,
                    if matches!(&operation, Operation::Place { .. }) {
                        BlockFacePurpose::PlaceSupport
                    } else {
                        BlockFacePurpose::Break
                    },
                    self.config.face_targeting.face_inset,
                    self.config.face_targeting.edge_margin,
                    self.config.face_targeting.maximum_hit_points_per_face,
                );
                let point = points
                    .get(hit_point_attempt % points.len())
                    .copied()
                    .ok_or(());
                let Ok(point) = point else {
                    self.fail("no valid face hit point").await;
                    return;
                };
                let target_for_look = LookTarget::BlockFacePoint {
                    position: look_position,
                    block_id: expected_id,
                    point,
                };
                // Claim the phase before awaiting the look controller. This
                // prevents a later tick from starting the same child task.
                self.set_state(InteractionState::Looking).await;
                if look
                    .look_at_with_precision(minecraft, target_for_look, LookPrecision::Precise)
                    .await
                    .is_err()
                {
                    self.fail("precise look failed").await;
                }
            } else {
                self.dispatch(minecraft, &operation).await;
            }
            return;
        }
        if state == InteractionState::Looking {
            let status = look.snapshot().await;
            if status.state == crate::look::look_controller::LookState::Completed {
                let (expected_hit, expected_face) = match &operation {
                    Operation::Break { target, face, .. } => (*target, *face),
                    Operation::Place { support, face, .. } => (*support, *face),
                };
                match minecraft.looked_block().await {
                    // Breaking needs the target block. Unlike placement, the
                    // exact face is advisory and can legitimately vary after
                    // interpolation or a collision-shape raycast.
                    Ok(hit)
                        if matches!(&operation, Operation::Break { .. })
                            && hit.position == expected_hit =>
                    {
                        if self.operation_still_valid(minecraft, &operation).await {
                            self.dispatch(minecraft, &operation).await
                        } else {
                            self.retry_or_fail("target or support changed").await;
                        }
                    }
                    Ok(hit)
                        if hit.position == expected_hit
                            && hit.face == to_raycast_face(expected_face) =>
                    {
                        if self.operation_still_valid(minecraft, &operation).await {
                            self.dispatch(minecraft, &operation).await
                        } else {
                            self.retry_or_fail("target or support changed").await;
                        }
                    }
                    Ok(hit) if hit.position == expected_hit => {
                        self.retry_or_fail("raycast hit wrong face").await
                    }
                    Ok(hit) => {
                        if matches!(&operation, Operation::Break { .. }) {
                            let actual_id = block_id(minecraft, hit.position)
                                .await
                                .ok()
                                .flatten()
                                .unwrap_or_else(|| "unloaded".into());
                            debug!(expected = ?expected_hit, actual = ?hit.position, actual_id, "target face is obstructed; trying another acquisition candidate");
                            self.recover_break_raycast(minecraft, movement, &operation)
                                .await;
                        } else {
                            self.retry_or_fail("wrong support block is being looked at")
                                .await;
                        }
                    }
                    Err(_) => self.retry_or_fail("raycast target unavailable").await,
                }
            } else if status.state == crate::look::look_controller::LookState::Failed {
                self.retry_or_fail("look interrupted").await;
            }
            return;
        }
        if matches!(
            state,
            InteractionState::Breaking | InteractionState::Placing
        ) && dispatched
        {
            match &operation {
                Operation::Break {
                    target, expected, ..
                } => match block_id(minecraft, *target).await {
                    Ok(Some(id)) if id == *expected => {
                        let mut inner = self.inner.lock().await;
                        if let Some(start) = inner.snapshot.started_at {
                            inner.snapshot.progress_percent = Some(estimated_progress(
                                start,
                                Duration::from_millis(self.config.verification_timeout_ms),
                            ));
                        }
                        drop(inner);
                        if self.elapsed_exceeded().await {
                            self.retry_or_fail("break verification timed out").await;
                        }
                    }
                    Ok(Some(id)) if !is_air(Some(&id)) => {
                        self.stop_and_fail(
                            minecraft,
                            movement,
                            look,
                            &format!("target changed from {expected} to {id}"),
                        )
                        .await
                    }
                    Ok(_) => self.complete_break(minecraft, movement, look).await,
                    Err(_) => {
                        self.stop_and_fail(minecraft, movement, look, "chunk unloaded")
                            .await
                    }
                },
                Operation::Place {
                    target, item_id, ..
                } => match block_id(minecraft, *target).await {
                    Ok(Some(id)) if id == *item_id => {
                        self.complete("Block placed").await;
                        let _ = look.release_precise(minecraft).await;
                    }
                    Ok(_) if self.elapsed_exceeded().await => {
                        self.retry_or_fail("placement verification timed out").await
                    }
                    Err(_) => self.retry_or_fail("chunk unloaded").await,
                    _ => {}
                },
            }
        }
    }
    async fn dispatch(&self, minecraft: &MinecraftClient, operation: &Operation) {
        // Mark dispatch before touching Azalea so break/place cannot be
        // started twice if another completion signal arrives meanwhile.
        {
            let mut inner = self.inner.lock().await;
            if inner.dispatched
                || !matches!(
                    inner.snapshot.state,
                    InteractionState::Looking | InteractionState::Preparing
                )
            {
                return;
            }
            inner.dispatched = true;
            inner.snapshot.state = if matches!(operation, Operation::Break { .. }) {
                InteractionState::Breaking
            } else {
                InteractionState::Placing
            };
            inner.snapshot.started_at = Some(SystemTime::now());
        }
        let result = match operation {
            Operation::Break {
                target, expected, ..
            } => {
                if self.config.auto_tool_switch {
                    let inventory = minecraft.world_state_snapshot().await.inventory;
                    if let Some(slot) = self.tool_selector.select(&inventory, expected) {
                        if let Err(error) = minecraft.select_hotbar_slot(slot).await {
                            return self.retry_or_fail(&error.to_string()).await;
                        }
                        logging::info(format!("Selected hotbar slot {slot} for {expected}"));
                    } else {
                        debug!(block = expected, "tool policy kept the current item");
                    }
                }
                minecraft.begin_breaking(*target).await
            }
            Operation::Place { item_id, .. } => {
                match minecraft.select_item_in_hotbar(item_id).await {
                    Ok(true) => minecraft.use_item_at_look_target().await,
                    Ok(false) => Err(AppError::InteractionItemMissing(item_id.clone())),
                    Err(error) => Err(error),
                }
            }
        };
        match result {
            Ok(()) => {
                if matches!(operation, Operation::Break { .. }) {
                    logging::info("Beginning block break");
                } else {
                    logging::info("Placing block");
                }
            }
            Err(error) => {
                self.retry_or_fail(&error.to_string()).await;
            }
        }
    }
    async fn operation_still_valid(
        &self,
        minecraft: &MinecraftClient,
        operation: &Operation,
    ) -> bool {
        match operation {
            Operation::Break {
                target, expected, ..
            } => block_id(minecraft, *target).await.ok().flatten().as_deref() == Some(expected),
            Operation::Place {
                target,
                support,
                support_id,
                item_id,
                ..
            } => {
                is_replaceable(block_id(minecraft, *target).await.ok().flatten().as_deref())
                    && block_id(minecraft, *support)
                        .await
                        .ok()
                        .flatten()
                        .as_deref()
                        == Some(support_id)
                    && minecraft
                        .world_state_snapshot()
                        .await
                        .inventory
                        .has_item(item_id, 1)
            }
        }
    }
    async fn prepare(&self) {
        let mut inner = self.inner.lock().await;
        inner.snapshot.state = InteractionState::Preparing;
        inner.dispatched = false;
        inner.retry_at = None;
    }
    async fn set_state(&self, state: InteractionState) {
        self.inner.lock().await.snapshot.state = state;
    }
    async fn elapsed_exceeded(&self) -> bool {
        self.inner
            .lock()
            .await
            .snapshot
            .started_at
            .is_some_and(|at| {
                at.elapsed().unwrap_or_default()
                    >= Duration::from_millis(self.config.verification_timeout_ms)
            })
    }
    async fn retry_or_fail(&self, reason: &str) {
        let mut inner = self.inner.lock().await;
        if inner.snapshot.retries >= self.config.retry_limit {
            inner.snapshot.state = InteractionState::Failed;
            inner.snapshot.failure_reason = Some(reason.into());
            inner.operation = None;
            if !inner.failure_logged {
                inner.failure_logged = true;
                logging::warning(format!("Interaction failed: {reason}"));
            }
        } else {
            inner.snapshot.retries += 1;
            inner.snapshot.state = InteractionState::Retrying;
            inner.retry_at =
                Some(SystemTime::now() + Duration::from_millis(self.config.retry_delay_ms));
        }
    }
    /// Exhaust safe points on a face, then the remaining faces, before asking
    /// the existing block navigation service for another approach position.
    async fn recover_break_raycast(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        operation: &Operation,
    ) {
        let Operation::Break { target, face, .. } = operation else {
            return;
        };
        let reposition = {
            let mut inner = self.inner.lock().await;
            let hit_point_attempt = inner.hit_point_attempt;
            inner
                .failed_hit_points
                .insert((*target, *face, hit_point_attempt));
            inner.hit_point_attempt += 1;
            if inner.hit_point_attempt >= self.config.face_targeting.maximum_hit_points_per_face {
                inner.hit_point_attempt = 0;
                inner.face_attempt += 1;
            }
            if inner.face_attempt < 6 {
                inner.snapshot.state = InteractionState::Preparing;
                return;
            }
            inner.face_attempt = 0;
            inner.hit_point_attempt = 0;
            true
        };
        if reposition {
            logging::info("Repositioning for a clear view");
            match self
                .navigation
                .try_another_approach(minecraft, movement)
                .await
            {
                Ok(true) => self.set_state(InteractionState::Moving).await,
                Ok(false) => {
                    self.retry_or_fail("no clear reachable view from any safe interaction position")
                        .await
                }
                Err(error) => {
                    self.retry_or_fail(&format!("no clear reachable view: {error}"))
                        .await
                }
            }
        }
    }
    async fn fail(&self, reason: &str) {
        let mut inner = self.inner.lock().await;
        inner.snapshot.state = InteractionState::Failed;
        inner.snapshot.failure_reason = Some(reason.into());
        inner.operation = None;
        if !inner.failure_logged {
            inner.failure_logged = true;
            logging::warning(format!("Interaction failed: {reason}"));
        }
    }
    async fn stop_and_fail(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        reason: &str,
    ) {
        minecraft.stop_breaking().await;
        self.navigation.cancel(minecraft, movement).await;
        let _ = look.release_precise(minecraft).await;
        self.fail(reason).await;
    }
    async fn complete(&self, message: &str) {
        let mut inner = self.inner.lock().await;
        inner.snapshot.state = InteractionState::Completed;
        inner.snapshot.progress_percent = Some(100);
        inner.operation = None;
        logging::success(message);
    }
    async fn complete_break(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
    ) {
        let restore = {
            let mut inner = self.inner.lock().await;
            inner.snapshot.state = InteractionState::Completed;
            inner.snapshot.progress_percent = Some(100);
            inner.operation = None;
            inner.restore_after_break.take()
        };
        logging::success("Block broken");
        let _ = look.release_precise(minecraft).await;
        if let Some((position, block_id)) = restore {
            logging::info("Restoring original oak log");
            if let Err(error) = self
                .place_at(minecraft, movement, look, position, block_id)
                .await
            {
                self.fail(&format!("oak-log restoration failed: {error}"))
                    .await;
            }
        }
    }
}
async fn block_id(
    minecraft: &MinecraftClient,
    position: BlockPosition,
) -> Result<Option<String>, AppError> {
    Ok(minecraft
        .block_ids_at(&[position])
        .await?
        .remove(&position)
        .flatten())
}
async fn find_support(
    minecraft: &MinecraftClient,
    target: BlockPosition,
    bot: PositionSnapshot,
) -> Result<Option<(BlockPosition, BlockFace)>, AppError> {
    let preferred = best_face(target, [bot.x, bot.y, bot.z]);
    let faces = [
        preferred,
        BlockFace::Up,
        BlockFace::Down,
        BlockFace::North,
        BlockFace::South,
        BlockFace::West,
        BlockFace::East,
    ];
    for from_target in faces {
        let support = from_target.neighbor(target);
        let state = block_id(minecraft, support).await?;
        let loaded = state.is_some();
        let air = is_air(state.as_deref());
        let replaceable = is_replaceable(state.as_deref());
        let clickable = has_support(state.as_deref());
        let reachable = within_reach(Some(bot), support, 4.5);
        debug!(
            target_x = target.x, target_y = target.y, target_z = target.z,
            support_x = support.x, support_y = support.y, support_z = support.z,
            state = ?state, loaded, air, replaceable, clickable, reachable,
            face = ?from_target.opposite(),
            "placement support candidate (visibility is verified after precise look)"
        );
        if clickable {
            return Ok(Some((support, from_target.opposite())));
        }
    }
    Ok(None)
}
fn to_raycast_face(face: BlockFace) -> RaycastFace {
    match face {
        BlockFace::Up => RaycastFace::Up,
        BlockFace::Down => RaycastFace::Down,
        BlockFace::North => RaycastFace::North,
        BlockFace::South => RaycastFace::South,
        BlockFace::West => RaycastFace::West,
        BlockFace::East => RaycastFace::East,
    }
}
fn break_face_order(preferred: BlockFace) -> [BlockFace; 6] {
    let all = [
        BlockFace::Up,
        BlockFace::Down,
        BlockFace::North,
        BlockFace::South,
        BlockFace::West,
        BlockFace::East,
    ];
    let mut ordered = [preferred; 6];
    let mut index = 1;
    for face in all {
        if face != preferred {
            ordered[index] = face;
            index += 1;
        }
    }
    ordered
}
fn navigation_failure_reason(error: &AppError) -> String {
    match error {
        AppError::NoValidApproachPosition => "no safe reachable interaction position".into(),
        AppError::TargetBlockDisappeared => "target changed before navigation".into(),
        AppError::TargetChunkUnloaded | AppError::ChunkUnavailableDuringSearch => {
            "target chunk is not loaded".into()
        }
        AppError::MovementUnavailable => "bot is not ready for navigation".into(),
        AppError::MovementCancelled => "navigation was cancelled".into(),
        AppError::BlockNavigationTimeout => "navigation timed out".into(),
        AppError::MovementStalled => "movement stalled".into(),
        other => format!("navigation could not start: {other}"),
    }
}
async fn world_position(minecraft: &MinecraftClient) -> Result<[f64; 3], AppError> {
    let position = minecraft
        .world_state_snapshot()
        .await
        .bot
        .position
        .ok_or(AppError::BlockSearchPositionUnavailable)?;
    Ok([position.x, position.y, position.z])
}
#[allow(dead_code)]
pub fn held_item(inventory: &InventorySnapshot) -> Option<&str> {
    inventory.selected_item()?.item_id.as_deref()
}
#[allow(dead_code)]
pub fn find_placeable_block(inventory: &InventorySnapshot, item_id: &str) -> bool {
    inventory.has_item(item_id, 1)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_helpers_report_missing_and_held_items() {
        let i = InventorySnapshot::default();
        assert!(!find_placeable_block(&i, "minecraft:stone"));
        assert_eq!(held_item(&i), None);
    }
    #[test]
    fn break_recovery_exhausts_each_face_before_repositioning() {
        let faces = break_face_order(BlockFace::North);
        assert_eq!(faces[0], BlockFace::North);
        let unique: HashSet<_> = faces.into_iter().collect();
        assert_eq!(unique.len(), 6);
    }
    #[test]
    fn navigation_start_errors_keep_the_useful_reason() {
        assert_eq!(
            navigation_failure_reason(&AppError::NoValidApproachPosition),
            "no safe reachable interaction position"
        );
        assert_eq!(
            navigation_failure_reason(&AppError::TargetBlockDisappeared),
            "target changed before navigation"
        );
    }
}
