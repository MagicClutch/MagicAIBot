//! What an emergency stop actually does to every subsystem once
//! [`crate::control::EmergencyStop::trigger`] has fired -- the "hard reset"
//! half of the emergency-stop system (see this crate's `control` module
//! doc comment for the split between the signal and this).
//!
//! Deliberately not a negotiated shutdown: every controller's own
//! `cancel()` already forcibly flips its internal state synchronously
//! rather than waiting for anything in-flight to acknowledge it (see e.g.
//! `InteractionController::cancel`'s doc comment -- an idle controller is a
//! no-op, an active one is torn down immediately). [`execute`] just calls
//! every one of them, in an order chosen so a controller that hands control
//! back to another (interaction releasing a precise look back to whatever
//! it suspended, for instance) is reset *before* the thing it hands back to,
//! so nothing reasserts itself after being told to stop.

use crate::{
    combat::KillController, interaction::InteractionController, logging, look::LookController,
    minecraft::client::MinecraftClient, mobs::CombatController, movement::MovementService,
    navigation::BlockNavigationService, pathfinding::PathfindingController,
    survival::SurvivalController,
};

use super::EmergencyStop;

/// Everything a running command loop needs a handle to in order to be
/// force-reset. Borrowed, never owned -- `App` keeps the real controllers;
/// this is just a bundle of references so `execute` doesn't need a dozen
/// positional parameters.
pub struct StopTargets<'a> {
    pub minecraft: &'a MinecraftClient,
    pub movement: &'a MovementService,
    pub block_navigation: &'a BlockNavigationService,
    pub look: &'a LookController,
    pub interaction: &'a InteractionController,
    pub combat: &'a CombatController,
    /// Standalone PvP (`#kill`/`/kill`) -- see `crate::combat`'s module
    /// doc comment for why this is separate from `combat` above.
    pub pvp: &'a KillController,
    pub container: &'a crate::container::service::ContainerService,
    /// Long-distance segmented navigation -- see `crate::pathfinding`. Reset
    /// before `movement` below hands anything back: a live segment follower
    /// would otherwise resubmit a waypoint goal on its very next tick,
    /// walking the bot on after the stop.
    pub pathfinding: &'a PathfindingController,
    pub survival: &'a SurvivalController,
}

/// Runs the full emergency stop: fires `emergency` (instantly waking every
/// blocking wait currently racing against it, in any subsystem, before a
/// single controller is even touched below), then force-resets every
/// controller directly. Never waits for graceful completion -- every step
/// here either already is synchronous or is a best-effort, fire-and-forget
/// network call (`let _ = ...`) whose result doesn't gate the next step, so
/// this always runs to completion in bounded time regardless of what state
/// any individual subsystem was stuck in.
pub async fn execute(targets: &StopTargets<'_>, emergency: &EmergencyStop) {
    logging::milestone("Emergency stop requested");
    // Fire first: every loop currently blocked in `App::wait_tick` (a
    // stuck goto, a stuck break/place, a stuck combat chase, a stuck
    // gather/mine iteration, ...) wakes immediately and unwinds back to
    // the console dispatcher on its own, in parallel with everything
    // below -- this alone is what makes the stop "hard" rather than
    // dependent on each controller's own cancellation being reachable.
    emergency.trigger();

    logging::info("Cancelling all active tasks");
    targets.minecraft.clear_current_task().await;

    logging::info("Stopping movement");
    targets
        .pathfinding
        .cancel(targets.minecraft, targets.movement)
        .await;
    targets
        .block_navigation
        .cancel(targets.minecraft, targets.movement)
        .await;
    let _ = targets.movement.stop(targets.minecraft).await;

    logging::info("Stopping pathfinding");
    // Already torn down by `movement.stop` above (it calls this same
    // primitive internally) -- called again explicitly, and independently
    // of that result, so an emergency stop's pathfinder-abort guarantee
    // never silently depends on `MovementService`'s own success path.
    let _ = targets.minecraft.stop_navigation().await;

    logging::info("Stopping combat");
    targets
        .combat
        .cancel(targets.minecraft, targets.movement, targets.look)
        .await;
    targets
        .pvp
        .cancel(targets.minecraft, targets.movement, targets.look)
        .await;

    logging::info("Stopping interactions");
    targets
        .interaction
        .cancel(targets.minecraft, targets.movement, targets.look)
        .await;
    targets
        .container
        .cancel(
            targets.minecraft,
            targets.block_navigation,
            targets.movement,
            targets.look,
        )
        .await;
    targets
        .survival
        .emergency_stop(targets.minecraft, targets.look)
        .await;

    logging::info("Clearing queued actions");
    // Last: `InteractionController::cancel`/`ContainerService::cancel`
    // above can hand a suspended look target back via
    // `LookController::release_precise`, which would re-arm a *new* aim
    // rather than leave the camera idle. Cancelling the look controller
    // only after every other controller has already been reset guarantees
    // nothing reasserts a target afterward.
    targets.look.cancel().await;
    targets.minecraft.clear_current_task().await;

    logging::success("Emergency stop complete");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        blocks::BlockSearchService,
        config::{
            AccountMode, BridgingConfig, ConsoleConfig, InteractionConfig, LookConfig,
            MinecraftConfig, MovementConfig, MultitaskingConfig, ReconnectConfig, SurvivalConfig,
            VerticalNavigationConfig, WorldStateConfig,
        },
        container::service::ContainerService,
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

    #[tokio::test]
    async fn executes_fully_without_a_connection_and_leaves_the_signal_re_armed() {
        // No live connection -- every network call inside `execute` must
        // fail gracefully (they're all `let _ = ...` or already tolerant of
        // "nothing to cancel"), never panic, and never hang.
        let minecraft = minecraft();
        let movement =
            MovementService::new(MovementConfig::default(), MultitaskingConfig::default());
        let search = BlockSearchService::new(32, 20, 32);
        let block_navigation = BlockNavigationService::new(Default::default(), search.clone());
        let look = LookController::new(LookConfig::default(), search.clone());
        let interaction = InteractionController::new(
            InteractionConfig::default(),
            search.clone(),
            block_navigation.clone(),
        );
        let combat = CombatController::default();
        let pvp = KillController::default();
        let container = ContainerService::default();
        let pathfinding = PathfindingController::new(Default::default(), Default::default());
        let survival = SurvivalController::new(SurvivalConfig::default());
        let emergency = EmergencyStop::new();
        let token_before = emergency.token();

        let targets = StopTargets {
            minecraft: &minecraft,
            movement: &movement,
            block_navigation: &block_navigation,
            look: &look,
            interaction: &interaction,
            combat: &combat,
            pvp: &pvp,
            container: &container,
            pathfinding: &pathfinding,
            survival: &survival,
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            execute(&targets, &emergency),
        )
        .await
        .expect("emergency stop must never hang, even with no live connection");

        assert!(
            token_before.is_cancelled(),
            "trigger() must fire the token every waiting loop was holding"
        );
        assert!(
            !emergency.token().is_cancelled(),
            "the signal must be re-armed for the next stop"
        );
    }
}
