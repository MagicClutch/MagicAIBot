//! Live execution of terrain-modifying movement maneuvers (pillaring, safe
//! digging-down, bridging across gaps, advancing staircases) that Azalea's
//! own pathfinder cannot perform on its own. Azalea's move graph has real,
//! tested support for mining through obstructions (see
//! `NavigationMode::AllowMining`), but no block-placement primitive at all,
//! so any route that requires building has no upstream capability to reuse.
//!
//! This module composes the same movement, interaction, look, and inventory
//! primitives every other console command uses -- it does not introduce a
//! competing movement system. It decides *whether* to act using the pure,
//! tested policy in [`crate::navigation::vertical`]; every physical action
//! (breaking, placing, jumping, selecting hotbar items) goes through the
//! existing controllers, including their automatic tool selection.

use std::{
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crate::{
    config::VerticalNavigationConfig,
    error::AppError,
    interaction::{
        InteractionController, interaction_controller::InteractionState, placement_rules,
    },
    logging,
    look::{LookController, LookTarget},
    minecraft::{
        client::MinecraftClient,
        world_state::{BlockPosition, PositionSnapshot},
    },
    movement::MovementService,
    navigation::{
        BlockNavigationService,
        vertical::{self, VerticalAction, VerticalFailure, VerticalLimits, VerticalSafety},
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VerticalConstructionState {
    #[default]
    Idle,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Default)]
pub struct VerticalConstructionStatus {
    pub state: VerticalConstructionState,
    pub last_action: Option<String>,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Default)]
struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    fn reset(&self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Bounds how many primitive terrain actions a single `assist_toward` call
/// will attempt, independent of the configured height/depth limits, so a
/// policy bug can never spin forever. Each call only prepares a short run of
/// terrain ahead of the bot; the caller's own retry loop re-invokes this
/// after letting Azalea's pathfinder actually walk through what was
/// prepared, so this bound does not limit how far a route can ultimately go.
const MAX_STEPS: u32 = 24;

#[derive(Clone, Default)]
pub struct VerticalConstructionService {
    status: Arc<StdMutex<VerticalConstructionStatus>>,
    cancellation: Cancellation,
}

impl VerticalConstructionService {
    pub fn status(&self) -> VerticalConstructionStatus {
        self.status
            .lock()
            .expect("vertical construction status poisoned")
            .clone()
    }
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Prepares terrain toward `target`, one primitive action at a time:
    /// pillaring or digging in place when the remaining gap is purely
    /// vertical, an advancing staircase when both height and distance
    /// remain, or a single bridge block when the next step is a gap at the
    /// same height. Plain 1-block steps and horizontal-only travel are left
    /// to Azalea's own pathfinder (optionally with mining enabled), which
    /// already handles them; the caller is expected to retry its normal
    /// `goto` after each call to actually move the bot through whatever was
    /// just prepared.
    #[allow(clippy::too_many_arguments)]
    pub async fn assist_toward(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
        block_navigation: &BlockNavigationService,
        config: &VerticalNavigationConfig,
        target: BlockPosition,
    ) -> Result<(), AppError> {
        if !config.enabled {
            return Err(AppError::TaskRuntime(
                "vertical construction is disabled in configuration".into(),
            ));
        }
        self.cancellation.reset();
        self.set_state(VerticalConstructionState::Running, None);

        let world = minecraft.world_state_snapshot().await;
        let Some(mut cursor) = world.bot.block_position else {
            self.fail("bot position unavailable");
            return Err(AppError::BlockSearchPositionUnavailable);
        };

        for _ in 0..MAX_STEPS {
            if self.cancellation.is_cancelled() {
                logging::info("Vertical construction cancelled");
                self.set_state(VerticalConstructionState::Cancelled, None);
                return Err(AppError::MovementCancelled);
            }
            let delta_y = target.y - cursor.y;
            let horizontal_remaining = (target.x - cursor.x).unsigned_abs() as i64
                + (target.z - cursor.z).unsigned_abs() as i64;
            if delta_y.abs() <= 1 && horizontal_remaining <= 1 {
                self.set_state(VerticalConstructionState::Completed, None);
                return Ok(());
            }

            if horizontal_remaining == 0 {
                // Pure vertical gap: pillar or dig in place.
                let safety = self.build_safety(minecraft, config, cursor, delta_y).await;
                let limits = VerticalLimits {
                    max_pillar_height: config.max_pillar_height,
                    max_dig_depth: config.max_dig_depth,
                    max_fall: 3,
                };
                match vertical::choose_vertical_action(delta_y, safety, limits) {
                    Ok(VerticalAction::PillarUp) => {
                        logging::info("Building vertical pillar");
                        self.pillar_up_one(minecraft, look, config, cursor).await?;
                        cursor.y += 1;
                    }
                    Ok(VerticalAction::MineDown) => {
                        logging::info("Path blocked, excavating");
                        self.dig_down_one(
                            minecraft,
                            movement,
                            look,
                            interaction,
                            block_navigation,
                            cursor,
                        )
                        .await?;
                        cursor.y -= 1;
                    }
                    Ok(VerticalAction::DigStaircaseDown) => {
                        logging::info("Mining staircase downward");
                        self.dig_down_one(
                            minecraft,
                            movement,
                            look,
                            interaction,
                            block_navigation,
                            cursor,
                        )
                        .await?;
                        cursor.y -= 1;
                    }
                    Ok(VerticalAction::Walk | VerticalAction::Jump) => {
                        self.set_state(VerticalConstructionState::Completed, None);
                        return Ok(());
                    }
                    Err(reason) => {
                        let message = describe_failure(reason);
                        self.fail(message);
                        return Err(AppError::TaskRuntime(message.into()));
                    }
                }
                continue;
            }

            // Horizontal progress is still needed. Advance along whichever
            // axis has the larger remaining distance, one block at a time.
            let (step_x, step_z) = vertical::dominant_step_toward(cursor, target);
            let ahead = BlockPosition {
                x: cursor.x + step_x,
                y: cursor.y,
                z: cursor.z + step_z,
            };

            if delta_y > 0 {
                logging::info("Creating staircase upward");
                self.staircase_up_one(
                    minecraft,
                    movement,
                    look,
                    interaction,
                    block_navigation,
                    config,
                    cursor,
                    ahead,
                )
                .await?;
                cursor = BlockPosition {
                    x: ahead.x,
                    y: ahead.y + 1,
                    z: ahead.z,
                };
            } else if delta_y < 0 {
                logging::info("Creating staircase downward");
                self.staircase_down_one(
                    minecraft,
                    movement,
                    look,
                    interaction,
                    block_navigation,
                    cursor,
                    ahead,
                )
                .await?;
                cursor = BlockPosition {
                    x: ahead.x,
                    y: ahead.y - 1,
                    z: ahead.z,
                };
            } else {
                let floor = BlockPosition {
                    x: ahead.x,
                    y: ahead.y - 1,
                    z: ahead.z,
                };
                let floor_id = minecraft.block_id_at(floor).await.ok().flatten();
                let is_gap = floor_id
                    .as_deref()
                    .is_none_or(|id| placement_rules::is_replaceable(Some(id)));
                if is_gap {
                    logging::info("Bridging across gap");
                    self.bridge_one(
                        minecraft,
                        movement,
                        look,
                        interaction,
                        block_navigation,
                        config,
                        floor,
                    )
                    .await?;
                    cursor = ahead;
                } else {
                    // A plain wall or already-walkable ground: Azalea's own
                    // (optionally mining-enabled) pathfinder handles this.
                    self.set_state(VerticalConstructionState::Completed, None);
                    return Ok(());
                }
            }
        }
        self.set_state(VerticalConstructionState::Completed, None);
        Ok(())
    }

    async fn build_safety(
        &self,
        minecraft: &MinecraftClient,
        config: &VerticalNavigationConfig,
        current: BlockPosition,
        delta: i32,
    ) -> VerticalSafety {
        let world = minecraft.world_state_snapshot().await;
        let blocks = vertical::select_scaffold_block(config, &world.inventory)
            .map(|item| world.inventory.count_item(&item))
            .unwrap_or(0);
        // A pillaring player needs one clear block above the head to jump
        // into; treat an unloaded chunk as "not clear" rather than guessing.
        let headroom = matches!(
            minecraft
                .block_id_at(BlockPosition {
                    x: current.x,
                    y: current.y + 2,
                    z: current.z,
                })
                .await,
            Ok(Some(id)) if placement_rules::is_replaceable(Some(&id))
        );
        let (lava_below, safe_fall) = if delta < 0 {
            let exposed = BlockPosition {
                x: current.x,
                y: current.y - 2,
                z: current.z,
            };
            let lava = matches!(
                minecraft.block_id_at(exposed).await,
                Ok(Some(id)) if id == "minecraft:lava"
            );
            (lava, 1)
        } else {
            (false, 0)
        };
        VerticalSafety {
            enabled: config.enabled,
            pillar: config.allow_pillaring,
            dig_down: config.allow_digging_down,
            staircase: config.prefer_staircase_descent,
            blocks,
            headroom,
            lava_below,
            safe_fall,
        }
    }

    /// Places one scaffold block directly beneath the bot's current feet by
    /// jumping to briefly vacate that space, then clicking the top face of
    /// the block that was supporting it -- the same technique a player uses
    /// to pillar up. The exact timing (how long to wait for the jump to
    /// clear the hitbox before clicking) is an empirical constant; it has
    /// not been tuned against a live server.
    async fn pillar_up_one(
        &self,
        minecraft: &MinecraftClient,
        look: &LookController,
        config: &VerticalNavigationConfig,
        current: BlockPosition,
    ) -> Result<(), AppError> {
        let scaffold = self.equip_scaffold(minecraft, config).await?;
        let support = BlockPosition {
            x: current.x,
            y: current.y - 1,
            z: current.z,
        };
        let look_point = PositionSnapshot {
            x: f64::from(current.x) + 0.5,
            y: f64::from(current.y) - 1.0,
            z: f64::from(current.z) + 0.5,
        };
        let _ = look.look_at(minecraft, LookTarget::World(look_point)).await;
        minecraft.jump().await?;
        for _ in 0..10 {
            if minecraft.world_state_snapshot().await.bot.on_ground == Some(false) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        minecraft.interact_block(support).await?;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if minecraft
                .block_id_at(current)
                .await
                .ok()
                .flatten()
                .as_deref()
                == Some(scaffold.as_str())
            {
                self.set_last_action(format!("placed {scaffold}"));
                return Ok(());
            }
        }
        let reason = "pillar placement was not confirmed by the server";
        self.fail(reason);
        Err(AppError::TaskRuntime(reason.into()))
    }

    /// Breaks the block directly beneath the bot's feet, reusing the same
    /// tested breaking machinery every other console command uses. The lava
    /// check in `build_safety` runs first, so this is only reached once the
    /// exposed block has been confirmed not to be lava.
    async fn dig_down_one(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
        block_navigation: &BlockNavigationService,
        current: BlockPosition,
    ) -> Result<(), AppError> {
        let below = BlockPosition {
            x: current.x,
            y: current.y - 1,
            z: current.z,
        };
        self.break_and_wait(
            minecraft,
            movement,
            look,
            interaction,
            block_navigation,
            below,
        )
        .await?;
        self.set_last_action("dug down one block".into());
        Ok(())
    }

    /// Advances one step while climbing: clears the feet/head space ahead if
    /// solid, ensures a floor exists at the new step, then lets the caller's
    /// normal `goto` retry actually walk (and jump) onto it.
    #[allow(clippy::too_many_arguments)]
    async fn staircase_up_one(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
        block_navigation: &BlockNavigationService,
        config: &VerticalNavigationConfig,
        cursor: BlockPosition,
        ahead: BlockPosition,
    ) -> Result<(), AppError> {
        let step_floor = BlockPosition {
            x: ahead.x,
            y: cursor.y,
            z: ahead.z,
        };
        let step_feet = BlockPosition {
            x: ahead.x,
            y: cursor.y + 1,
            z: ahead.z,
        };
        let step_head = BlockPosition {
            x: ahead.x,
            y: cursor.y + 2,
            z: ahead.z,
        };
        for position in [step_feet, step_head] {
            if let Ok(Some(id)) = minecraft.block_id_at(position).await
                && !placement_rules::is_replaceable(Some(&id))
            {
                self.break_and_wait(
                    minecraft,
                    movement,
                    look,
                    interaction,
                    block_navigation,
                    position,
                )
                .await?;
            }
        }
        let floor_id = minecraft.block_id_at(step_floor).await.ok().flatten();
        if floor_id
            .as_deref()
            .is_none_or(|id| placement_rules::is_replaceable(Some(id)))
        {
            let scaffold = self.equip_scaffold(minecraft, config).await?;
            self.place_and_wait(
                minecraft,
                movement,
                look,
                interaction,
                block_navigation,
                step_floor,
                scaffold,
            )
            .await?;
        }
        self.set_last_action("built staircase step upward".into());
        Ok(())
    }

    /// Advances one step while descending: after confirming the step below
    /// is solid ground (not a deeper drop) and not lava, clears the space one
    /// level down so Azalea's own descend move can step into it. Deeper or
    /// unconfirmed drops are left alone rather than guessed at.
    #[allow(clippy::too_many_arguments)]
    async fn staircase_down_one(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
        block_navigation: &BlockNavigationService,
        cursor: BlockPosition,
        ahead: BlockPosition,
    ) -> Result<(), AppError> {
        let step_floor = BlockPosition {
            x: ahead.x,
            y: cursor.y - 2,
            z: ahead.z,
        };
        let step_feet = BlockPosition {
            x: ahead.x,
            y: cursor.y - 1,
            z: ahead.z,
        };
        if matches!(
            minecraft.block_id_at(step_floor).await,
            Ok(Some(id)) if id == "minecraft:lava"
        ) {
            let reason = "lava detected below; refusing to dig further";
            self.fail(reason);
            return Err(AppError::TaskRuntime(reason.into()));
        }
        let floor_solid = matches!(
            minecraft.block_id_at(step_floor).await,
            Ok(Some(id)) if !placement_rules::is_replaceable(Some(&id))
        );
        if !floor_solid {
            let reason = "next step down has no confirmed solid floor; refusing an unsafe drop";
            self.fail(reason);
            return Err(AppError::TaskRuntime(reason.into()));
        }
        if let Ok(Some(id)) = minecraft.block_id_at(step_feet).await
            && !placement_rules::is_replaceable(Some(&id))
        {
            self.break_and_wait(
                minecraft,
                movement,
                look,
                interaction,
                block_navigation,
                step_feet,
            )
            .await?;
        }
        self.set_last_action("built staircase step downward".into());
        Ok(())
    }

    /// Places one scaffold block to extend the floor across a gap, reusing
    /// the same tested placement machinery every other console command uses
    /// (including its auto-navigate-into-range and support-face search).
    #[allow(clippy::too_many_arguments)]
    async fn bridge_one(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
        block_navigation: &BlockNavigationService,
        config: &VerticalNavigationConfig,
        floor_target: BlockPosition,
    ) -> Result<(), AppError> {
        let scaffold = self.equip_scaffold(minecraft, config).await?;
        self.place_and_wait(
            minecraft,
            movement,
            look,
            interaction,
            block_navigation,
            floor_target,
            scaffold,
        )
        .await?;
        self.set_last_action("placed a bridge block".into());
        Ok(())
    }

    async fn equip_scaffold(
        &self,
        minecraft: &MinecraftClient,
        config: &VerticalNavigationConfig,
    ) -> Result<String, AppError> {
        let world = minecraft.world_state_snapshot().await;
        let Some(scaffold) = vertical::select_scaffold_block(config, &world.inventory) else {
            let reason = "no scaffold blocks available";
            self.fail(reason);
            return Err(AppError::TaskRuntime(reason.into()));
        };
        match minecraft.select_item_in_hotbar(&scaffold).await {
            Ok(true) => Ok(scaffold),
            Ok(false) => {
                let reason = format!("{scaffold} is not available in the hotbar");
                self.fail(&reason);
                Err(AppError::TaskRuntime(reason))
            }
            Err(error) => {
                self.fail(&error.to_string());
                Err(error)
            }
        }
    }

    /// Submits a break and drives `interaction`/`block_navigation`/`look`
    /// ticks itself until it reaches a terminal state. Interaction can hand
    /// off to block navigation (to get in range) and to look (for precise
    /// aiming), so both are driven here too, exactly like the polling
    /// helpers `tasks::mod` uses for the same reason.
    #[allow(clippy::too_many_arguments)]
    async fn break_and_wait(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
        block_navigation: &BlockNavigationService,
        target: BlockPosition,
    ) -> Result<(), AppError> {
        interaction
            .break_at(minecraft, movement, look, target)
            .await?;
        self.await_interaction(minecraft, movement, look, interaction, block_navigation)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn place_and_wait(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
        block_navigation: &BlockNavigationService,
        target: BlockPosition,
        item: String,
    ) -> Result<(), AppError> {
        interaction
            .place_at(minecraft, movement, look, target, item)
            .await?;
        self.await_interaction(minecraft, movement, look, interaction, block_navigation)
            .await
    }

    async fn await_interaction(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
        block_navigation: &BlockNavigationService,
    ) -> Result<(), AppError> {
        loop {
            block_navigation.tick(minecraft, movement).await;
            look.tick(minecraft).await;
            interaction.tick(minecraft, movement, look).await;
            let snapshot = interaction.snapshot().await;
            match snapshot.state {
                InteractionState::Completed | InteractionState::Idle => return Ok(()),
                InteractionState::Cancelled => return Err(AppError::InteractionCancelled),
                InteractionState::Failed => {
                    let reason = snapshot
                        .failure_reason
                        .unwrap_or_else(|| "interaction failed".into());
                    self.fail(&reason);
                    return Err(AppError::TaskRuntime(reason));
                }
                _ => tokio::time::sleep(Duration::from_millis(75)).await,
            }
        }
    }

    fn set_state(&self, state: VerticalConstructionState, failure_reason: Option<String>) {
        let mut status = self
            .status
            .lock()
            .expect("vertical construction status poisoned");
        status.state = state;
        status.failure_reason = failure_reason;
    }
    fn set_last_action(&self, action: String) {
        self.status
            .lock()
            .expect("vertical construction status poisoned")
            .last_action = Some(action);
    }
    fn fail(&self, reason: &str) {
        logging::error(reason);
        self.set_state(VerticalConstructionState::Failed, Some(reason.to_owned()));
    }
}

fn describe_failure(failure: VerticalFailure) -> &'static str {
    match failure {
        VerticalFailure::Disabled => "vertical construction is disabled for this route",
        VerticalFailure::NoBlocks => "not enough scaffold blocks to pillar the remaining height",
        VerticalFailure::HeightLimit => "requested pillar height exceeds the configured limit",
        VerticalFailure::DigLimit => "requested dig depth exceeds the configured limit",
        VerticalFailure::Lava => "lava detected below; refusing to dig further",
        VerticalFailure::UnsafeFall => "the drop below is not safe to descend",
        VerticalFailure::NoHeadroom => "not enough headroom above to pillar safely",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disconnected_client() -> MinecraftClient {
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
        )
    }
    fn search() -> crate::blocks::BlockSearchService {
        crate::blocks::BlockSearchService::new(64, 32, 8)
    }
    fn navigation() -> BlockNavigationService {
        BlockNavigationService::new(Default::default(), search())
    }

    fn enabled_config() -> VerticalNavigationConfig {
        VerticalNavigationConfig {
            enabled: true,
            allow_pillaring: true,
            allow_digging_down: true,
            prefer_staircase_descent: true,
            max_pillar_height: 8,
            max_dig_depth: 16,
            minimum_building_blocks: 4,
            allowed_building_blocks: vec!["minecraft:dirt".into()],
            denied_building_blocks: vec![],
        }
    }

    #[tokio::test]
    async fn disabled_config_is_rejected_before_touching_the_client() {
        let service = VerticalConstructionService::default();
        let client = disconnected_client();
        let movement = MovementService::new(Default::default(), Default::default());
        let look = LookController::new(Default::default(), search());
        let interaction = InteractionController::new(Default::default(), search(), navigation());
        let mut config = enabled_config();
        config.enabled = false;
        let error = service
            .assist_toward(
                &client,
                &movement,
                &look,
                &interaction,
                &navigation(),
                &config,
                BlockPosition { x: 0, y: 80, z: 0 },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("disabled"));
        assert!(!service.cancellation.is_cancelled());
    }

    #[test]
    fn cancellation_flag_round_trips() {
        let cancellation = Cancellation::default();
        assert!(!cancellation.is_cancelled());
        cancellation.cancel();
        assert!(cancellation.is_cancelled());
        cancellation.reset();
        assert!(!cancellation.is_cancelled());
    }

    #[test]
    fn status_defaults_to_idle() {
        let service = VerticalConstructionService::default();
        assert_eq!(service.status().state, VerticalConstructionState::Idle);
    }

    // A disconnected client has no bot position at all, so every target
    // (regardless of which branch -- pillar, staircase, bridge -- it would
    // otherwise select) fails at the same "position unavailable" check
    // before any branch-specific logic runs. That still meaningfully
    // regression-tests "does not hang", which is what these assert; genuinely
    // distinguishing the staircase/bridge/pillar branches from each other
    // needs a live position feed that only a real connection provides -- see
    // the limitation in the final report.
    #[tokio::test]
    async fn assist_toward_fails_promptly_without_a_live_position_regardless_of_target_shape() {
        let service = VerticalConstructionService::default();
        let client = disconnected_client();
        let movement = MovementService::new(Default::default(), Default::default());
        let look = LookController::new(Default::default(), search());
        let interaction = InteractionController::new(Default::default(), search(), navigation());
        for target in [
            BlockPosition { x: 0, y: 90, z: 0 }, // pure vertical: pillar-in-place shape
            BlockPosition { x: 5, y: 70, z: 0 }, // height + distance: staircase-up shape
            BlockPosition { x: 5, y: 50, z: 0 }, // depth + distance: staircase-down shape
            BlockPosition { x: 5, y: 64, z: 0 }, // distance only: bridge shape
        ] {
            let error = tokio::time::timeout(
                Duration::from_secs(2),
                service.assist_toward(
                    &client,
                    &movement,
                    &look,
                    &interaction,
                    &navigation(),
                    &enabled_config(),
                    target,
                ),
            )
            .await
            .expect("assist_toward must not hang when disconnected")
            .unwrap_err();
            assert!(!error.to_string().is_empty());
        }
    }

    #[test]
    fn describe_failure_covers_every_variant() {
        for failure in [
            VerticalFailure::Disabled,
            VerticalFailure::NoBlocks,
            VerticalFailure::HeightLimit,
            VerticalFailure::DigLimit,
            VerticalFailure::Lava,
            VerticalFailure::UnsafeFall,
            VerticalFailure::NoHeadroom,
        ] {
            assert!(!describe_failure(failure).is_empty());
        }
    }
}
