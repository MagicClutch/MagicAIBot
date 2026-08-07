//! Public API for `#kill <player>`/`/kill <player>`: [`KillController`],
//! the controller `App` drives exactly like every other task-shaped
//! controller in this codebase (`mobs::combat::CombatController`,
//! `InteractionController`, ...) -- `start`/`cancel`/`snapshot`/`tick`.
//! The actual per-tick logic lives in `crate::combat::executor`; this file
//! is just the public surface and the state-machine edges `App` needs
//! (start, cancel, terminal-state polling).

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::sync::Mutex;

use crate::{
    combat::{executor, state::KillSnapshot, state::KillState},
    config::KillbotConfig,
    error::AppError,
    logging,
    look::LookController,
    minecraft::client::MinecraftClient,
    movement::MovementService,
};

#[derive(Clone)]
pub struct KillController {
    inner: Arc<Mutex<executor::Inner>>,
}

impl Default for KillController {
    fn default() -> Self {
        Self::new(KillbotConfig::default())
    }
}

impl KillController {
    pub fn new(config: KillbotConfig) -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self {
            inner: Arc::new(Mutex::new(executor::Inner::new(seed.max(1), config))),
        }
    }

    pub async fn snapshot(&self) -> KillSnapshot {
        self.inner.lock().await.snapshot.clone()
    }

    /// Starts (or restarts, replacing whatever fight was previously
    /// running) a `#kill <player>` task. Fails immediately, without
    /// touching any state, if `name` isn't a currently-tracked player --
    /// the spec's "target cannot be found" completion condition, checked
    /// up front rather than only discovered a tick later.
    pub async fn start(&self, minecraft: &MinecraftClient, name: String) -> Result<(), AppError> {
        let world = minecraft.world_state_snapshot().await;
        let player = world
            .find_player_by_name(&name)
            .ok_or_else(|| AppError::UnknownPlayer(name.clone()))?;
        {
            let mut inner = self.inner.lock().await;
            inner.reset_for_new_fight();
            inner.snapshot = KillSnapshot {
                state: KillState::Running,
                // Flips to `Engage` on the very first successful `tick()`
                // (see `executor::tick`) -- this reflects the instant
                // between a successful lookup here and that first tick.
                phase: crate::combat::state::CombatPhase::AcquireTarget,
                target_name: Some(name.clone()),
                target_uuid: Some(player.uuid),
                target_position: player.position,
                started_at: Some(SystemTime::now()),
                ..KillSnapshot::default()
            };
        }
        logging::milestone(format!("Kill task started: {name}"));
        logging::milestone("Target acquired");
        Ok(())
    }

    /// An idle (or already-terminal) controller is a no-op, the same
    /// contract every other controller in this codebase follows -- callers
    /// defensively cancel before starting something new.
    ///
    /// `movement` is taken because a fight cancelled during its approach may
    /// still hold a pathfinding goal (see `crate::combat::movement`'s
    /// long-distance handoff); releasing only the raw combat input would
    /// leave the bot walking to where the target used to be.
    pub async fn cancel(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
    ) {
        let active = { self.inner.lock().await.snapshot.state == KillState::Running };
        if !active {
            return;
        }
        executor::stop_all(minecraft, movement, look).await;
        let mut inner = self.inner.lock().await;
        inner.snapshot.state = KillState::Cancelled;
        inner.snapshot.failure_reason = None;
        logging::info("Kill task cancelled");
    }

    pub async fn tick(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
    ) {
        let mut inner = self.inner.lock().await;
        executor::tick(minecraft, movement, look, &mut inner).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        blocks::BlockSearchService,
        config::{
            AccountMode, BridgingConfig, ConsoleConfig, LookConfig, MinecraftConfig,
            ReconnectConfig, VerticalNavigationConfig, WorldStateConfig,
        },
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
        MovementService::new(
            crate::config::MovementConfig::default(),
            crate::config::MultitaskingConfig::default(),
        )
    }

    fn look() -> LookController {
        LookController::new(LookConfig::default(), BlockSearchService::new(32, 20, 32))
    }

    #[tokio::test]
    async fn starts_created() {
        let kill = KillController::default();
        assert_eq!(kill.snapshot().await.state, KillState::Created);
    }

    #[tokio::test]
    async fn cancel_on_an_idle_controller_is_a_no_op() {
        let kill = KillController::default();
        kill.cancel(&minecraft(), &movement(), &look()).await;
        assert_eq!(kill.snapshot().await.state, KillState::Created);
    }

    #[tokio::test]
    async fn starting_against_an_untracked_player_fails_without_touching_state() {
        let kill = KillController::default();
        let result = kill.start(&minecraft(), "5cat".to_owned()).await;
        assert!(matches!(result, Err(AppError::UnknownPlayer(name)) if name == "5cat"));
        assert_eq!(kill.snapshot().await.state, KillState::Created);
    }

    #[tokio::test]
    async fn ticking_an_untouched_controller_does_not_panic() {
        let kill = KillController::default();
        kill.tick(&minecraft(), &movement(), &look()).await;
        assert_eq!(kill.snapshot().await.state, KillState::Created);
    }
}
