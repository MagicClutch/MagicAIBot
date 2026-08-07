use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    time::{Duration, Instant, SystemTime},
};

use crate::{
    blocks::{
        self,
        block_query::BlockSearchQuery,
        block_search::{BlockSearchService, format_find_results, format_nearest_result},
    },
    combat::{KillController, state::KillState},
    config::Config,
    console::{
        self,
        commands::{
            ConsoleCommand, ConsoleInput, OutputModeChange, OutputModeTarget, plain_chat_message,
        },
    },
    container::{model::TransferDirection, service::ContainerService},
    control::{EmergencyStop, stop::StopTargets, stop::execute as execute_emergency_stop},
    equipment::{EquipmentService, HotbarEquipmentService},
    error::AppError,
    interaction::{InteractionController, interaction_controller::InteractionState},
    items::drop_plan::{self, DropPlanError},
    logging,
    look::{
        LookController, LookTarget,
        look_controller::{LookPriority, LookSnapshot, LookState},
    },
    minecraft::{
        client::MinecraftClient,
        world_state::{MovementStatus, TaskSnapshot},
    },
    mobs::{self, CombatController, CombatState},
    movement::{MovementService, NavigationMode},
    navigation::BlockNavigationService,
    navigation::navigation_state::BlockNavigationState,
    pathfinding::{NavigationState, PathfindingController},
    survival::SurvivalController,
};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

// ---- Completion polling ------------------------------------------------------
//
// Movement, block navigation, interaction, and look are all tick-driven state
// machines: submitting a goal (`goto`, `start`, `break_at`, `look_at`, ...)
// only enqueues work and returns as soon as the state transitions away from
// idle -- it does not wait for a terminal state. The application's own tick
// loop normally drives these forward, but a console/chat caller that awaits
// the submission alone would report success (or failure) before the bot has
// actually done anything. These helpers drive the same `tick()` methods the
// application loop uses, in a private loop, so a caller that awaits one of
// them genuinely blocks until Completed/Reached, Failed, or Cancelled.
//
// Because that wait runs inline inside `execute_console_input`, which itself
// runs inline inside the single `tokio::select!` loop in `App::run`, nothing
// else in that loop -- including a newly typed `/stop` -- would normally be
// read until the wait finishes. Each loop below races its poll sleep against
// `input_rx` so new console input is never left stuck behind a long-running
// (or genuinely stalled) `/goto`, `/break`, `/place`, `/look`, or `/interact`.
// Read-only queries are answered immediately without disturbing the wait;
// anything else interrupts it and is handed back to `execute_console_input`.

type InputReceiver = mpsc::Receiver<Result<ConsoleInput, AppError>>;

/// How close `#goto <player>` and `#drop <item> <amount> <player>` stop to
/// the target player -- close enough to interact/throw, far enough not to
/// crowd or try to stand on top of them. Passed to
/// `MovementService::goto_player_approach` as both Azalea's own pathfinder
/// stop distance and the app-level arrival threshold.
const PLAYER_APPROACH_DISTANCE: f64 = 2.0;

/// Bound on how long `#drop <item> <amount> <player>` waits for its aim to
/// settle before dropping anyway. A `LookTarget::Player` aim tracks
/// movement, so `LookState` never reaches `Completed` for it (see
/// `look_at_player_briefly`'s doc comment) -- without a bound, waiting for
/// that state would hang forever.
const PLAYER_LOOK_SETTLE_TIMEOUT: Duration = Duration::from_millis(800);

/// Outcome of a blocking wait: either it ran to completion, or new console
/// input arrived that must be handled by the caller instead.
enum WaitOutcome {
    Finished(Result<(), AppError>),
    Interrupted(ConsoleInput),
}

/// Two-tier timeout, mirroring `BlockNavigationService`'s own
/// `stuck_timeout_seconds`/`maximum_navigation_seconds` split: the *primary*
/// deadline (`stuck_timeout_seconds`, short) only fires when the bot's
/// distance to its destination stops improving -- so a bot that is still
/// actively making progress, however slowly (a long-distance `/goto`, a
/// multi-minute bridge build), is never cut off no matter how long the
/// whole trip takes. `maximum_navigation_seconds` is a much longer,
/// deliberately rare absolute backstop: Azalea's pathfinder is submitted
/// with `retry_on_no_path(true)` (see `MinecraftClient::start_navigation_to`),
/// so a genuinely unreachable goal retries forever inside Azalea without
/// ever surfacing as a failure on its own -- but a goal that's truly
/// unreachable also never gets any closer, so `stuck_timeout_seconds` alone
/// already catches that case quickly; the absolute backstop exists only for
/// the pathological case of a route that keeps inching closer forever
/// without ever actually arriving.
async fn await_movement_terminal(app: &App, input_rx: &mut InputReceiver) -> WaitOutcome {
    let maximum_navigation = Duration::from_secs(app.movement.maximum_navigation_seconds());
    let stuck_timeout = Duration::from_secs(app.movement.stuck_timeout_seconds());
    let started = Instant::now();
    let mut best_distance: Option<f64> = None;
    let mut last_progress = started;
    loop {
        app.movement.tick(&app.minecraft, false).await;
        app.survival
            .tick(
                &app.minecraft,
                &app.movement,
                &app.look,
                &app.interaction,
                &app.combat,
            )
            .await;
        let snapshot = app.movement.snapshot().await;
        match snapshot.status {
            MovementStatus::Completed | MovementStatus::Idle => {
                return WaitOutcome::Finished(Ok(()));
            }
            MovementStatus::Cancelled => {
                return WaitOutcome::Finished(Err(AppError::MovementCancelled));
            }
            MovementStatus::Failed => {
                return WaitOutcome::Finished(Err(AppError::PathfindingFailure(
                    snapshot
                        .failure_reason
                        .unwrap_or_else(|| "unknown reason".into()),
                )));
            }
            MovementStatus::MovingToPosition | MovementStatus::FollowingPlayer => {
                if let Some(distance) = snapshot.estimated_distance {
                    // More than a stride's worth of closing distance counts
                    // as real progress -- the same tolerance
                    // `BlockNavigationService` uses for its own positional
                    // stuck check, so a single slow mining/build step along
                    // the way doesn't false-positive as "stalled".
                    let improved = best_distance.is_none_or(|best| distance < best - 0.1);
                    if improved {
                        best_distance = Some(distance);
                        last_progress = Instant::now();
                    }
                }
                if last_progress.elapsed() >= stuck_timeout {
                    let _ = app.movement.stop(&app.minecraft).await;
                    return WaitOutcome::Finished(Err(AppError::PathfindingFailure(format!(
                        "movement stalled: no progress toward the destination in {}s",
                        stuck_timeout.as_secs()
                    ))));
                }
                if started.elapsed() >= maximum_navigation {
                    let _ = app.movement.stop(&app.minecraft).await;
                    return WaitOutcome::Finished(Err(AppError::PathfindingFailure(format!(
                        "movement exceeded the {}s absolute limit",
                        maximum_navigation.as_secs()
                    ))));
                }
                let poll = survival_poll_interval(app, Duration::from_millis(200)).await;
                if let Some(input) = wait_tick(app, input_rx, poll).await {
                    return WaitOutcome::Interrupted(input);
                }
            }
        }
    }
}

async fn await_block_navigation_terminal(app: &App, input_rx: &mut InputReceiver) -> WaitOutcome {
    loop {
        // `BlockNavigationService::tick` submits a goto once when it selects
        // a candidate/approach and afterward only *reads*
        // `MovementService`'s snapshot -- it never resubmits or refreshes
        // it. `MovementService::tick` is what actually drives that
        // (`refresh_navigation_goal`'s periodic resubmission, failure/
        // timeout detection via Azalea's own `navigation_status`) and
        // publishes the snapshot `block_navigation.tick` reads. Without
        // calling it here too, a path that needs resubmission (common once
        // mining is involved) just silently stalls: nothing is left driving
        // it, and nothing ever notices it failed.
        app.movement.tick(&app.minecraft, false).await;
        app.block_navigation
            .tick(&app.minecraft, &app.movement)
            .await;
        app.survival
            .tick(
                &app.minecraft,
                &app.movement,
                &app.look,
                &app.interaction,
                &app.combat,
            )
            .await;
        let snapshot = app.block_navigation.snapshot().await;
        match snapshot.state {
            BlockNavigationState::Reached | BlockNavigationState::Idle => {
                return WaitOutcome::Finished(Ok(()));
            }
            BlockNavigationState::Cancelled => {
                return WaitOutcome::Finished(Err(AppError::MovementCancelled));
            }
            BlockNavigationState::Failed => {
                return WaitOutcome::Finished(Err(AppError::TaskRuntime(
                    snapshot
                        .failure_reason
                        .unwrap_or_else(|| "block navigation failed".into()),
                )));
            }
            _ => {
                let poll = survival_poll_interval(app, Duration::from_millis(200)).await;
                if let Some(input) = wait_tick(app, input_rx, poll).await {
                    return WaitOutcome::Interrupted(input);
                }
            }
        }
    }
}

async fn await_look_terminal(app: &App, input_rx: &mut InputReceiver) -> WaitOutcome {
    loop {
        app.look.tick(&app.minecraft).await;
        let snapshot = app.look.snapshot().await;
        match snapshot.state {
            LookState::Completed | LookState::Idle => return WaitOutcome::Finished(Ok(())),
            LookState::Cancelled => return WaitOutcome::Finished(Err(AppError::LookCancelled)),
            LookState::Failed => {
                return WaitOutcome::Finished(Err(AppError::LookUnavailableWithReason(
                    snapshot
                        .failure_reason
                        .unwrap_or_else(|| "look failed".into()),
                )));
            }
            LookState::Looking => {
                if let Some(input) = wait_tick(app, input_rx, Duration::from_millis(75)).await {
                    return WaitOutcome::Interrupted(input);
                }
            }
        }
    }
}

/// Interaction can internally hand off to block navigation (to get in range)
/// and to look (for precise aiming), so both must be driven alongside it or
/// the interaction state machine stalls waiting on a tick that never comes.
/// Movement must be driven too, for the same reason
/// `await_block_navigation_terminal` now does -- block navigation only reads
/// `MovementService`'s snapshot, it never refreshes it.
async fn await_interaction_terminal(app: &App, input_rx: &mut InputReceiver) -> WaitOutcome {
    loop {
        app.movement.tick(&app.minecraft, false).await;
        app.block_navigation
            .tick(&app.minecraft, &app.movement)
            .await;
        app.look.tick(&app.minecraft).await;
        app.interaction
            .tick(&app.minecraft, &app.movement, &app.look)
            .await;
        app.survival
            .tick(
                &app.minecraft,
                &app.movement,
                &app.look,
                &app.interaction,
                &app.combat,
            )
            .await;
        let snapshot = app.interaction.snapshot().await;
        match snapshot.state {
            InteractionState::Completed | InteractionState::Idle => {
                return WaitOutcome::Finished(Ok(()));
            }
            InteractionState::Cancelled => {
                return WaitOutcome::Finished(Err(AppError::InteractionCancelled));
            }
            InteractionState::Failed => {
                return WaitOutcome::Finished(Err(AppError::TaskRuntime(
                    snapshot
                        .failure_reason
                        .unwrap_or_else(|| "interaction failed".into()),
                )));
            }
            _ => {
                let poll = survival_poll_interval(app, Duration::from_millis(75)).await;
                if let Some(input) = wait_tick(app, input_rx, poll).await {
                    return WaitOutcome::Interrupted(input);
                }
            }
        }
    }
}

/// Combat can internally drive movement (chasing a mob) and look (aiming at
/// it), so both must be driven alongside it here for the same reason
/// `await_interaction_terminal` drives block navigation and look -- otherwise
/// the combat state machine stalls waiting on a tick that never comes.
async fn await_combat_terminal(app: &App, input_rx: &mut InputReceiver) -> WaitOutcome {
    loop {
        app.movement.tick(&app.minecraft, false).await;
        app.look.tick(&app.minecraft).await;
        app.combat
            .tick(&app.minecraft, &app.movement, &app.look)
            .await;
        app.survival
            .tick(
                &app.minecraft,
                &app.movement,
                &app.look,
                &app.interaction,
                &app.combat,
            )
            .await;
        let snapshot = app.combat.snapshot().await;
        match snapshot.state {
            CombatState::Completed | CombatState::Idle => {
                return WaitOutcome::Finished(Ok(()));
            }
            CombatState::Cancelled => {
                return WaitOutcome::Finished(Err(AppError::CombatCancelled));
            }
            CombatState::Failed => {
                return WaitOutcome::Finished(Err(AppError::TaskRuntime(
                    snapshot
                        .failure_reason
                        .unwrap_or_else(|| "combat failed".into()),
                )));
            }
            _ => {
                let poll = survival_poll_interval(app, Duration::from_millis(75)).await;
                if let Some(input) = wait_tick(app, input_rx, poll).await {
                    return WaitOutcome::Interrupted(input);
                }
            }
        }
    }
}

/// Drives `App::pvp` (`crate::combat::KillController`) to a terminal state.
/// Nothing else runs while this loop is blocking (see `wait_tick`'s doc
/// comment), so everything the fight itself depends on -- the look
/// controller for aiming, and `survival` for fall-damage safety mid-fight
/// -- must be ticked alongside it here, the same reason every other
/// `await_*_terminal` in this file drives its own dependencies. `app.combat`
/// (mob combat, unrelated to and untouched by a PvP fight) is passed to
/// `survival.tick` purely because that's its fixed signature; it stays idle
/// for the whole fight.
/// Blocking wait for a long-distance `/goto` running through
/// `crate::pathfinding`. Structurally identical to
/// [`await_movement_terminal`] -- same poll cadence, same interruption
/// handling -- but it watches the *navigation* state machine rather than the
/// movement one, because with the segmented planner in charge the movement
/// layer legitimately reaches `Completed` at the end of every waypoint hop,
/// dozens of times per trip, and none of those mean the journey is over.
///
/// It also needs no stuck timeout of its own: the planner detects a blocked
/// segment (`segment_stuck_seconds`) and either replans or fails, so the
/// only way out of this loop is a terminal navigation state.
async fn await_navigation_terminal(app: &App, input_rx: &mut InputReceiver) -> WaitOutcome {
    loop {
        app.pathfinding.tick(&app.minecraft, &app.movement).await;
        app.movement.tick(&app.minecraft, false).await;
        app.survival
            .tick(
                &app.minecraft,
                &app.movement,
                &app.look,
                &app.interaction,
                &app.combat,
            )
            .await;
        let snapshot = app.pathfinding.snapshot().await;
        match snapshot.state {
            NavigationState::Arrived | NavigationState::Idle => {
                return WaitOutcome::Finished(Ok(()));
            }
            NavigationState::Cancelled => {
                return WaitOutcome::Finished(Err(AppError::MovementCancelled));
            }
            NavigationState::Failed => {
                return WaitOutcome::Finished(Err(AppError::PathfindingFailure(
                    snapshot
                        .failure
                        .map(|failure| failure.to_string())
                        .unwrap_or_else(|| "unknown reason".into()),
                )));
            }
            NavigationState::Planning
            | NavigationState::FollowingSegment
            | NavigationState::Recalculating => {
                let poll = survival_poll_interval(app, Duration::from_millis(75)).await;
                if let Some(input) = wait_tick(app, input_rx, poll).await {
                    return WaitOutcome::Interrupted(input);
                }
            }
        }
    }
}

async fn await_kill_terminal(app: &App, input_rx: &mut InputReceiver) -> WaitOutcome {
    loop {
        app.look.tick(&app.minecraft).await;
        app.pvp.tick(&app.minecraft, &app.look).await;
        app.survival
            .tick(
                &app.minecraft,
                &app.movement,
                &app.look,
                &app.interaction,
                &app.combat,
            )
            .await;
        let snapshot = app.pvp.snapshot().await;
        match snapshot.state {
            KillState::Completed | KillState::Created => {
                return WaitOutcome::Finished(Ok(()));
            }
            KillState::Cancelled => {
                return WaitOutcome::Finished(Err(AppError::KillCancelled));
            }
            KillState::Failed => {
                return WaitOutcome::Finished(Err(AppError::TaskRuntime(
                    snapshot
                        .failure_reason
                        .unwrap_or_else(|| "kill task failed".into()),
                )));
            }
            KillState::Running => {
                let poll = survival_poll_interval(app, Duration::from_millis(75)).await;
                if let Some(input) = wait_tick(app, input_rx, poll).await {
                    return WaitOutcome::Interrupted(input);
                }
            }
        }
    }
}

/// Poll cadence for the blocking-wait loops above while a water-bucket
/// clutch is actively mid-flight (`SurvivalController::is_active`). Their
/// normal cadence (75-200ms, sized for driving movement/interaction/combat
/// state machines that change over whole seconds) is coarse enough to step
/// straight from "still falling" to "already on the ground" without ever
/// observing the placement window in between -- fast enough here to give
/// several chances inside even a short one.
const SURVIVAL_ACTIVE_POLL: Duration = Duration::from_millis(20);

/// `fallback` normally, or [`SURVIVAL_ACTIVE_POLL`] whenever the clutch is
/// actively mid-flight -- see `SURVIVAL_ACTIVE_POLL`'s doc comment.
async fn survival_poll_interval(app: &App, fallback: Duration) -> Duration {
    if app.survival.is_active().await {
        SURVIVAL_ACTIVE_POLL
    } else {
        fallback
    }
}

/// Sleeps for `duration`, racing new console input the whole time. A
/// read-only query (or a chat message, or a console parse error) is handled
/// immediately and does not interrupt the wait -- `None` tells the caller to
/// keep waiting. Anything else (another action command, `/stop`, `/quit`, or
/// the channel closing) is returned so the caller can stop waiting and hand
/// it to `execute_console_input`.
async fn wait_tick(
    app: &App,
    input_rx: &mut InputReceiver,
    duration: Duration,
) -> Option<ConsoleInput> {
    // Checked before racing the normal sleep/local-input select below so an
    // in-game `#stop`/`#stopall` is caught even while `App::tick_chat_commands`
    // -- the normal chat-command drain loop -- can't run, because it's
    // gated behind whatever blocking command loop is currently calling this
    // very function. See `emergency_stop_from_chat`'s doc comment.
    if let Some(input) = emergency_stop_from_chat(app).await {
        return Some(input);
    }
    flush_outgoing_chat(&app.minecraft).await;
    let emergency = app.emergency_stop.token();
    tokio::select! {
        () = tokio::time::sleep(duration) => None,
        // Independent of `input_rx` entirely: this wakes every blocking
        // wait racing it (in every subsystem, everywhere `wait_tick` is
        // called) the instant `EmergencyStop::trigger` fires, regardless of
        // whether local console input is even being read right now -- see
        // `crate::control`.
        () = emergency.cancelled() => Some(ConsoleInput::Command(ConsoleCommand::Stop)),
        received = input_rx.recv() => match received {
            None => Some(ConsoleInput::Command(ConsoleCommand::Quit)),
            Some(Err(error)) => {
                println!("Console error: {error}");
                None
            }
            Some(Ok(ConsoleInput::Empty)) => None,
            Some(Ok(input)) => {
                if app.handle_inert_input(&input).await {
                    None
                } else {
                    Some(input)
                }
            }
        },
    }
}

/// Sends at most one status line `logging::milestone`/`progress`/etc. has
/// queued for chat delivery, if `logging::pop_outgoing_chat`'s own rate
/// limit allows it right now -- `logging` itself never touches the network,
/// so this is the only place that actually calls `send_chat` for them.
/// Deliberately never drains the whole queue in one call (a single
/// `#get`/`#mine` run can queue dozens of lines in one tick): calling this
/// on every tick lets a backlog drain out at a steady, spam-kick-safe pace
/// instead of being blasted at the server all at once. Best-effort: a
/// failed send (not yet connected, disconnected) is silently dropped rather
/// than requeued, the same way every other best-effort network call in
/// this file behaves.
async fn flush_outgoing_chat(minecraft: &MinecraftClient) {
    if let Some(line) = logging::pop_outgoing_chat() {
        let _ = minecraft.send_chat(&line).await;
    }
}

/// Detects `#stop`/`#stopall` sitting in the incoming player chat queue and,
/// if the sender passes the normal chat-command access check, treats it
/// exactly like local `/stop` input. Deliberately bypasses
/// `App::tick_chat_commands`'s normal per-command rate limit -- an
/// emergency stop must never be throttled -- while still respecting who is
/// allowed to run it at all. Every other queued chat message is left
/// untouched for `tick_chat_commands` to process normally once it's next
/// reachable (see `WorldState::take_matching_incoming_player_chat`'s doc
/// comment for why this can't just wait for that).
async fn emergency_stop_from_chat(app: &App) -> Option<ConsoleInput> {
    let chat = app
        .minecraft
        .take_matching_incoming_player_chat(|chat| {
            chat.kind == crate::minecraft::world_state::ChatMessageKind::Player
                && matches!(
                    chat.text.trim().to_ascii_lowercase().as_str(),
                    "#stop" | "#stopall"
                )
        })
        .await?;
    let sender = chat.sender?;
    if !app.chat_access_allowed(&sender) {
        logging::warning(format!(
            "[Chat] Emergency stop from {sender} rejected: access denied"
        ));
        return None;
    }
    logging::info(format!("[Chat] {sender} ran: /stop"));
    Some(ConsoleInput::Command(ConsoleCommand::Stop))
}

/// Application composition root.
pub struct App {
    config: Config,
    shutdown: CancellationToken,
    minecraft: MinecraftClient,
    movement: MovementService,
    block_navigation: BlockNavigationService,
    look: LookController,
    interaction: InteractionController,
    combat: CombatController,
    /// Standalone PvP (`#kill <player>`/`/kill <player>`) -- see
    /// `crate::combat`'s module doc comment for why this is a wholly
    /// separate controller from `combat` (mob-hunting) above rather than an
    /// extension of it.
    pvp: KillController,
    /// Baritone-style long-distance navigation (`/goto <x> <y> <z>` past
    /// `pathfinding.long_distance_threshold`) -- see `crate::pathfinding`.
    /// Plans the route and slices it into segments; each segment is still
    /// executed through `movement` below.
    pathfinding: PathfindingController,
    container: ContainerService,
    equipment: EquipmentService,
    hotbar_equipment: HotbarEquipmentService,
    survival: SurvivalController,
    emergency_stop: EmergencyStop,
    chat_rate_limits: HashMap<String, VecDeque<Instant>>,
    session_ready: bool,
    started_at: Instant,
}

impl App {
    /// Loads configuration and initializes logging without connecting to Minecraft.
    pub async fn initialize() -> Result<Self, AppError> {
        let _ = dotenvy::dotenv();
        println!("Starting Magic AI Bot...\n");
        println!("Loading configuration...\n");
        let config = Config::load(Path::new("config.toml"))?;
        crate::config::init_logging(&config.logging)?;
        logging::configure(config.output.console, config.output.chat);
        // Catches a hand-maintained table entry naming an item Minecraft
        // doesn't have (the "Raw Beef" != `raw_beef` bug class) before it
        // can surface later as "#get never finishes" -- non-fatal, since a
        // bad entry for a resource this session never requests shouldn't
        // block startup.
        for problem in crate::items::audit_registered_items() {
            logging::warning(format!("Invalid item registration: {problem}"));
        }
        let block_navigation = BlockNavigationService::new(
            config.block_navigation.clone(),
            crate::blocks::BlockSearchService::new(
                config.block_search.maximum_radius,
                config.block_search.maximum_result_limit,
                config.block_search.default_vertical_range,
            ),
        );

        let minecraft = MinecraftClient::new(
            config.minecraft.clone(),
            config.reconnect.clone(),
            config.console.clone(),
            config.world_state.clone(),
            config.vertical_navigation.clone(),
            config.bridging.clone(),
        );
        // Bounded buffer for incoming player chat messages (currently only
        // consumed by the `#`-prefixed direct console command feature, see
        // `App::handle_chat_console_command`); not user-configurable.
        minecraft.set_incoming_player_chat_capacity(64).await;
        Ok(Self {
            minecraft,
            movement: MovementService::new(config.movement.clone(), config.multitasking.clone()),
            block_navigation: block_navigation.clone(),
            look: LookController::new(
                config.look.clone(),
                crate::blocks::BlockSearchService::new(
                    config.block_search.maximum_radius,
                    config.block_search.maximum_result_limit,
                    config.block_search.default_vertical_range,
                ),
            ),
            interaction: InteractionController::new(
                config.interaction.clone(),
                crate::blocks::BlockSearchService::new(
                    config.block_search.maximum_radius,
                    config.block_search.maximum_result_limit,
                    config.block_search.default_vertical_range,
                ),
                block_navigation,
            ),
            // Mob combat has no view of the held weapon, so an automatic
            // cadence resolves to a sword's -- which is what `#get <mob>`
            // equips before fighting anyway.
            combat: CombatController::new(
                config
                    .killbot
                    .attack_cooldown()
                    .unwrap_or(crate::combat::crits::SWORD_COOLDOWN),
            ),
            pvp: KillController::new(config.killbot.clone()),
            pathfinding: PathfindingController::new(
                config.pathfinding.clone(),
                config.vertical_navigation.clone(),
            ),
            container: ContainerService::default(),
            equipment: EquipmentService::new(config.equipment.clone()),
            hotbar_equipment: HotbarEquipmentService::new(
                config.equipment.hotbar.clone(),
                config.equipment.tools.clone(),
                config.equipment.autodrop.clone(),
            ),
            survival: SurvivalController::new(config.survival.clone()),
            emergency_stop: EmergencyStop::new(),
            chat_rate_limits: HashMap::new(),
            session_ready: false,
            config,
            shutdown: CancellationToken::new(),
            started_at: Instant::now(),
        })
    }

    /// Waits for Ctrl+C (or, on Unix, SIGTERM -- what Docker/Pelican Panel
    /// send to stop a container) and performs the application's graceful
    /// shutdown.
    pub async fn run(mut self) -> Result<(), AppError> {
        logging::info(format!("Connecting to {}", self.config.minecraft.server));
        if let Err(error) = self.minecraft.connect().await {
            logging::warning(format!("Connection failed ({error})"));
            return Err(error);
        }
        logging::info("Connected");
        logging::info("Joined world");

        let (input_tx, mut input_rx) = mpsc::channel(32);
        let mut movement_tick = tokio::time::interval(Duration::from_millis(
            self.config.movement.repath_interval_ms,
        ));
        let mut look_tick = tokio::time::interval(Duration::from_millis(
            1000 / u64::from(self.config.look.update_rate),
        ));
        let mut interaction_tick = tokio::time::interval(Duration::from_millis(50));
        let console_task = self.config.console.enabled.then(|| {
            tokio::task::spawn_local(console::read_input(input_tx, self.shutdown.child_token()))
        });

        let loop_result = loop {
            tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    result?;
                    break Ok(());
                }
                result = wait_for_terminate_signal() => {
                    result?;
                    logging::info("Received SIGTERM, shutting down");
                    break Ok(());
                }
                _ = movement_tick.tick() => {
                    if self.minecraft.connection_state() != crate::minecraft::client::ConnectionState::Connected {
                        if self.session_ready {
                            self.session_ready = false;
                            self.interaction.cancel(&self.minecraft, &self.movement, &self.look).await;
                            self.combat.cancel(&self.minecraft, &self.movement, &self.look).await;
                            self.pvp.cancel(&self.minecraft, &self.look).await;
                            self.pathfinding.cancel(&self.minecraft, &self.movement).await;
                            self.block_navigation.cancel(&self.minecraft, &self.movement).await;
                            self.look.cancel().await;
                            self.survival.reset().await;
                            let _ = self.movement.stop(&self.minecraft).await;
                            self.minecraft.clear_current_task().await;
                        }
                        continue;
                    }
                    self.session_ready = true;
                    self.tick_chat_commands(&mut input_rx).await;
                    let explicit_look = self.look.snapshot().await.state == LookState::Looking;
                    // Before `movement`: the pathfinder decides which
                    // waypoint the movement layer should be walking toward,
                    // so it submits the goal that this same tick then
                    // refreshes.
                    self.pathfinding.tick(&self.minecraft, &self.movement).await;
                    self.movement.tick(&self.minecraft, explicit_look).await;
                },
                _ = look_tick.tick() => {
                    // Azalea's own pathfinder drives the camera to face the
                    // direction of travel while a path is calculating or
                    // executing, so back off here to avoid the two fighting
                    // over yaw/pitch during ordinary walking. A precise
                    // interaction look (breaking/placing/interacting) must
                    // never be skipped this way, though: it does not rely on
                    // Azalea's travel-facing camera at all, and if this
                    // branch is skipped while `navigation_status()` still
                    // briefly reports calculating/executing right as
                    // navigation hands off to interaction, the precise look
                    // would never start ticking -- stalling the whole
                    // break/place/interact flow, which is waiting on it to
                    // reach `Completed` before it can dispatch.
                    let precise = self.look.is_precise_active().await;
                    let status = self.minecraft.navigation_status().await.ok();
                    if precise || !status.is_some_and(|status| status.calculating || status.executing) {
                        self.look.tick(&self.minecraft).await;
                    }
                },
                _ = interaction_tick.tick() => {
                    flush_outgoing_chat(&self.minecraft).await;
                    // Block navigation owns interaction approach/repath state.
                    // Tick it on the fast interaction cadence so an interaction
                    // target does not wait for the slower movement repath timer
                    // before the next path is selected.
                    self.block_navigation.tick(&self.minecraft, &self.movement).await;
                    self.interaction.tick(&self.minecraft, &self.movement, &self.look).await;
                    self.combat.tick(&self.minecraft, &self.movement, &self.look).await;
                    self.pvp.tick(&self.minecraft, &self.look).await;
                    self.container.tick(&self.minecraft, &self.movement, &self.block_navigation, &self.look).await;
                    self.equipment.tick(&self.minecraft).await;
                    self.hotbar_equipment.tick(&self.minecraft).await;
                    // Also on this fast, fixed 50ms cadence rather than
                    // `movement_tick`'s slower, configurable
                    // `repath_interval_ms` -- a fall's placement window can
                    // be only a handful of ticks wide (see
                    // `survival_poll_interval`'s doc comment for the same
                    // reasoning applied to the blocking command-wait loops).
                    self.survival
                        .tick(&self.minecraft, &self.movement, &self.look, &self.interaction, &self.combat)
                        .await;
                },
                input = input_rx.recv() => match input {
                    Some(Ok(ConsoleInput::Empty)) => {}
                    Some(Ok(input)) => {
                        if self.execute_console_input(input, &mut input_rx).await? {
                            break Ok(());
                        }
                    }
                    Some(Err(error)) => println!("Console error: {error}"),
                    None => break Ok(()),
                }
            }
        };

        self.shutdown.cancel();
        self.container
            .cancel(
                &self.minecraft,
                &self.block_navigation,
                &self.movement,
                &self.look,
            )
            .await;
        self.block_navigation
            .cancel(&self.minecraft, &self.movement)
            .await;
        self.look.cancel().await;
        self.survival.reset().await;
        self.interaction
            .cancel(&self.minecraft, &self.movement, &self.look)
            .await;
        self.combat
            .cancel(&self.minecraft, &self.movement, &self.look)
            .await;
        self.pvp.cancel(&self.minecraft, &self.look).await;
        let _ = self.movement.stop(&self.minecraft).await;
        self.minecraft.disconnect().await?;
        if let Some(task) = console_task {
            await_console_task(task).await;
        }
        loop_result
    }

    async fn execute_console_input(
        &mut self,
        mut input: ConsoleInput,
        input_rx: &mut InputReceiver,
    ) -> Result<bool, AppError> {
        loop {
            match input {
                ConsoleInput::ChatMessage(message) => {
                    if let Some(message) = plain_chat_message(
                        &ConsoleInput::ChatMessage(message),
                        self.config.console.send_plain_input_to_chat,
                    ) {
                        if let Err(error) = self.minecraft.send_chat(message).await {
                            println!("Chat error: {error}");
                        }
                    } else {
                        println!("Plain console input forwarding is disabled.");
                    }
                }
                ConsoleInput::Command(command) => {
                    // A `#kill`/`/kill` fight keeps ticking in the
                    // background (see `App::pvp`'s periodic tick in the
                    // main select loop) until something explicitly cancels
                    // it -- previously only a handful of commands
                    // (`#get`/`#mine`/`#drop`/`/follow`/`/stop`) did, so
                    // starting almost any other action (`/goto`, `/break`,
                    // `/place`, `/look`, `/equip`, ...) while a fight was
                    // still running left both systems fighting over raw
                    // movement input, the selected hotbar slot, and the
                    // look controller at once -- which looked exactly like
                    // the bot being unable to jump, build, or break blocks
                    // at all. Cancel it up front for every command except
                    // read-only status queries (harmless to run mid-fight)
                    // and the ones that already manage `pvp` themselves
                    // (`KillPlayer` restarts it; `Stop` already cancels it
                    // via `StopTargets`; `StopMovement` is documented to
                    // deliberately leave combat alone, matching how it
                    // already treats `combat` the same way).
                    if !is_read_only_query(&command)
                        && !matches!(
                            command,
                            ConsoleCommand::KillPlayer { .. }
                                | ConsoleCommand::Stop
                                | ConsoleCommand::StopMovement
                        )
                    {
                        self.pvp.cancel(&self.minecraft, &self.look).await;
                    }
                    // Exactly the same hazard for long-distance navigation
                    // (`crate::pathfinding`), which also keeps ticking in the
                    // background: a `/goto 5000 100 -3000` that the user
                    // interrupts with new console input leaves the segment
                    // follower alive, resubmitting a movement goal every
                    // tick, so the next command would spend its whole life
                    // being dragged toward the abandoned destination. `Stop`
                    // is excluded only because it already cancels this
                    // through `StopTargets`; `Goto` is not excluded, since
                    // starting a new trip should supersede the old one and
                    // `start` does not itself stop the movement layer.
                    if !is_read_only_query(&command) && !matches!(command, ConsoleCommand::Stop) {
                        self.pathfinding
                            .cancel(&self.minecraft, &self.movement)
                            .await;
                    }
                    match command {
                        ConsoleCommand::Help => print_help(),
                        ConsoleCommand::Status => self.print_status().await,
                        ConsoleCommand::Where => self.print_where().await,
                        ConsoleCommand::Health => self.print_health().await,
                        ConsoleCommand::Chat { message } => {
                            if let Err(error) = self.minecraft.send_chat(&message).await {
                                println!("Chat error: {error}");
                            }
                        }
                        ConsoleCommand::Players => self.print_players().await,
                        ConsoleCommand::Inventory => self.print_inventory().await,
                        ConsoleCommand::ObservedContainerStatus => {
                            self.print_container_status().await
                        }
                        ConsoleCommand::Entities { radius } => self.print_entities(radius).await,
                        ConsoleCommand::OutputMode { change } => match change {
                            None => print_output_mode_status(),
                            Some(change) => apply_output_mode_change(change),
                        },
                        ConsoleCommand::Explanation => print_output_mode_explanation(),
                        ConsoleCommand::Goto { x, y, z } => {
                            self.interaction
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            let destination = crate::minecraft::world_state::PositionSnapshot {
                                x: f64::from(x),
                                y: f64::from(y),
                                z: f64::from(z),
                            };
                            // The segmented planner prints its own richer
                            // start line (and, with `debug_pathfinding`, the
                            // whole route plan), so only the direct path
                            // announces itself here.
                            if !self.use_segmented_navigation(destination).await {
                                logging::info(format!("Navigating to ({x}, {y}, {z})"));
                            }
                            match self
                                .goto_and_wait(
                                    "Go to position",
                                    destination,
                                    NavigationMode::AllowMining,
                                    input_rx,
                                )
                                .await
                            {
                                WaitOutcome::Finished(Ok(())) => {
                                    logging::success("Destination reached")
                                }
                                WaitOutcome::Finished(Err(error)) => logging::error(error),
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::GotoPlayer { player } => {
                            self.interaction
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            match self.goto_player_and_wait(player, input_rx).await {
                                WaitOutcome::Finished(Ok(())) => {}
                                WaitOutcome::Finished(Err(_)) => {
                                    // Already reported inside `goto_player_and_wait`.
                                }
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::GotoMine { x, y, z } => {
                            self.interaction
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            let destination = crate::minecraft::world_state::PositionSnapshot {
                                x: f64::from(x),
                                y: f64::from(y),
                                z: f64::from(z),
                            };
                            match self
                                .goto_and_wait(
                                    "Go to position",
                                    destination,
                                    NavigationMode::AllowMining,
                                    input_rx,
                                )
                                .await
                            {
                                WaitOutcome::Finished(Ok(())) => {}
                                WaitOutcome::Finished(Err(error)) => {
                                    println!("Movement error: {error}")
                                }
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::PathStatus => self.print_path_status().await,
                        ConsoleCommand::Stop => {
                            // The global emergency stop (`#stop`/`/stop`/`stopall`,
                            // all identical -- see `crate::control`): force-cancels
                            // every controller, not just movement/look. Genuinely
                            // independent of the normal per-command cancellation
                            // paths above -- it fires `self.emergency_stop` first,
                            // which alone is enough to unstick any blocking wait
                            // loop anywhere in this file (including, in
                            // particular, whatever loop is currently awaiting
                            // *this very command* -- see `wait_tick`), before a
                            // single controller is touched.
                            let targets = StopTargets {
                                minecraft: &self.minecraft,
                                movement: &self.movement,
                                block_navigation: &self.block_navigation,
                                look: &self.look,
                                interaction: &self.interaction,
                                combat: &self.combat,
                                pvp: &self.pvp,
                                container: &self.container,
                                pathfinding: &self.pathfinding,
                                survival: &self.survival,
                            };
                            execute_emergency_stop(&targets, &self.emergency_stop).await;
                        }
                        ConsoleCommand::StopMovement => {
                            // The original, narrower stop: only the movement
                            // channel and any interruptible look. Also cancels
                            // any active explicit-priority look (a plain
                            // `/look`/`/lookplayer`, or a task's own "look at"
                            // step, e.g. `#drop <item> <amount> <player>`'s
                            // "Looking at player" phase) -- see
                            // `interruptible_look`. A `PreciseInteraction`-priority
                            // look is left alone; that belongs to
                            // `InteractionController`'s own break/place lifecycle
                            // and is only released by finishing, `/stopinteraction`,
                            // or the full `/stop` above.
                            let description = self.active_stop_description().await;
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            if self.interruptible_look().await.is_some() {
                                self.look.cancel().await;
                            }
                            match self.movement.stop(&self.minecraft).await {
                                Ok(()) => match description {
                                    Some(description) => {
                                        logging::success(format!("Bot stopped ({description})"))
                                    }
                                    None => logging::info("Bot has no task to stop"),
                                },
                                Err(error) => logging::error(error),
                            }
                        }
                        ConsoleCommand::Follow { player } => {
                            self.interaction
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            match self.movement.follow(&self.minecraft, &player).await {
                                Ok(()) => logging::info(format!("Following player {player}")),
                                Err(AppError::UnknownPlayer(_)) => {
                                    logging::error("Player not found")
                                }
                                Err(error) => logging::error(error),
                            }
                        }
                        ConsoleCommand::KillPlayer { player } => {
                            match self.pvp.start(&self.minecraft, player.clone()).await {
                                Ok(()) => match await_kill_terminal(self, input_rx).await {
                                    WaitOutcome::Finished(Ok(())) => {}
                                    WaitOutcome::Finished(Err(AppError::KillCancelled)) => {}
                                    WaitOutcome::Finished(Err(_)) => {
                                        logging::error(format!("Target lost: {player}"));
                                    }
                                    WaitOutcome::Interrupted(next) => {
                                        input = next;
                                        continue;
                                    }
                                },
                                Err(AppError::UnknownPlayer(_)) => {
                                    logging::error(format!("Player not found: {player}"));
                                }
                                Err(error) => logging::error(error),
                            }
                        }
                        ConsoleCommand::Movement => self.print_movement().await,
                        ConsoleCommand::FindBlock {
                            block_id,
                            radius,
                            limit,
                        } => {
                            self.find_blocks(block_id, radius, limit).await;
                        }
                        ConsoleCommand::NearestBlock { block_id, radius } => {
                            self.find_blocks(block_id, radius, Some(1)).await;
                        }
                        ConsoleCommand::GotoBlock {
                            block_id,
                            search_radius,
                            allow_mining,
                        } => {
                            self.interaction
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            let radius = search_radius
                                .unwrap_or(self.config.block_navigation.default_search_radius);
                            match self
                                .goto_block_and_wait(
                                    block_id,
                                    radius,
                                    if allow_mining {
                                        NavigationMode::AllowMining
                                    } else {
                                        NavigationMode::MovementOnly
                                    },
                                    input_rx,
                                )
                                .await
                            {
                                WaitOutcome::Finished(Ok(())) => {}
                                WaitOutcome::Finished(Err(error)) => {
                                    logging::warning(format!("Block navigation failed: {error}"));
                                }
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::GotoBlockStatus => {
                            self.print_block_navigation_status().await
                        }
                        ConsoleCommand::CancelGotoBlock => {
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                        }
                        ConsoleCommand::GetResource {
                            resource_id,
                            amount,
                        } => {
                            self.interaction
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            self.combat
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            self.pvp.cancel(&self.minecraft, &self.look).await;
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            match self
                                .get_resource_and_wait(resource_id, amount, input_rx)
                                .await
                            {
                                WaitOutcome::Finished(Ok(())) => {}
                                WaitOutcome::Finished(Err(_)) => {
                                    // Already reported with the required `#get`
                                    // message format inside `run_get_item` /
                                    // `run_get_mob`.
                                }
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::Mine { block_ids, amount } => {
                            self.interaction
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            self.combat
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            self.pvp.cancel(&self.minecraft, &self.look).await;
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            match self.mine_and_wait(block_ids, amount, input_rx).await {
                                WaitOutcome::Finished(Ok(())) => {}
                                WaitOutcome::Finished(Err(_)) => {
                                    // Already reported inside `run_mine`.
                                }
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::Drop {
                            item_id,
                            amount,
                            player,
                        } => {
                            self.interaction
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            self.combat
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            self.pvp.cancel(&self.minecraft, &self.look).await;
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            match self.drop_and_wait(item_id, amount, player, input_rx).await {
                                WaitOutcome::Finished(Ok(())) => {}
                                WaitOutcome::Finished(Err(_)) => {
                                    // Already reported inside `run_drop_without_player` /
                                    // `run_drop_to_player`.
                                }
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::Look { x, y, z } => {
                            self.interaction
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            logging::info(format!("Looking at ({x}, {y}, {z})"));
                            match self
                                .look_and_wait(
                                    "Look at target",
                                    LookTarget::World(
                                        crate::minecraft::world_state::PositionSnapshot {
                                            x: f64::from(x),
                                            y: f64::from(y),
                                            z: f64::from(z),
                                        },
                                    ),
                                    input_rx,
                                )
                                .await
                            {
                                WaitOutcome::Finished(Ok(())) => {
                                    logging::success("Looking at target")
                                }
                                WaitOutcome::Finished(Err(error)) => logging::error(error),
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::LookBlock { block_id } => {
                            self.interaction
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            match self.look_block_and_wait(block_id, input_rx).await {
                                WaitOutcome::Finished(Ok(())) => {}
                                WaitOutcome::Finished(Err(error)) => {
                                    logging::warning(format!("Look failed: {error}"));
                                }
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::LookPlayer { player } => {
                            self.interaction
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            match self
                                .look_and_wait(
                                    "Look at player",
                                    LookTarget::Player(player),
                                    input_rx,
                                )
                                .await
                            {
                                WaitOutcome::Finished(Ok(())) => {}
                                WaitOutcome::Finished(Err(error)) => {
                                    logging::warning(format!("Look failed: {error}"));
                                }
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::LookEntity { entity_type } => {
                            self.interaction
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            let world = self.minecraft.world_state_snapshot().await;
                            let entity = world.entities.iter().find(|entity| {
                                entity
                                    .entity_type
                                    .rsplit(':')
                                    .next()
                                    .is_some_and(|kind| kind.eq_ignore_ascii_case(&entity_type))
                            });
                            match entity {
                                Some(entity) => {
                                    match self
                                        .look_and_wait(
                                            "Look at entity",
                                            LookTarget::Entity(entity.entity_id),
                                            input_rx,
                                        )
                                        .await
                                    {
                                        WaitOutcome::Finished(Ok(())) => {}
                                        WaitOutcome::Finished(Err(error)) => {
                                            logging::warning(format!("Look failed: {error}"));
                                        }
                                        WaitOutcome::Interrupted(next) => {
                                            input = next;
                                            continue;
                                        }
                                    }
                                }
                                None => logging::warning(format!("Unknown entity: {entity_type}")),
                            }
                        }
                        ConsoleCommand::LookStop => self.look.cancel().await,
                        ConsoleCommand::LookStatus => self.print_look_status().await,
                        ConsoleCommand::BreakBlock => {
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            match self.break_looked_and_wait(input_rx).await {
                                WaitOutcome::Finished(Ok(())) => {}
                                WaitOutcome::Finished(Err(error)) => {
                                    logging::warning(format!("Cannot break block: {error}"));
                                }
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::Break { x, y, z } => {
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            logging::info(format!("Breaking block at ({x}, {y}, {z})"));
                            match self
                                .break_at_and_wait(
                                    crate::minecraft::world_state::BlockPosition { x, y, z },
                                    input_rx,
                                )
                                .await
                            {
                                WaitOutcome::Finished(Ok(())) => logging::success("Block broken"),
                                WaitOutcome::Finished(Err(error)) => logging::error(error),
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::BreakNearest { block_id } => {
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            match self.break_nearest_and_wait(block_id, input_rx).await {
                                WaitOutcome::Finished(Ok(())) => {}
                                WaitOutcome::Finished(Err(error)) => {
                                    logging::warning(format!("Cannot break block: {error}"));
                                }
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::SelectTool { block_id } => {
                            let policy = crate::interaction::tool_selection::ToolSelectionPolicy {
                                minimum_remaining_durability: self
                                    .config
                                    .interaction
                                    .minimum_tool_durability,
                                fallback: if self.config.interaction.allow_hand_fallback {
                                    crate::interaction::tool_selection::ToolFallbackPolicy::AllowHand
                                } else {
                                    crate::interaction::tool_selection::ToolFallbackPolicy::RequireSuitableTool
                                },
                                held_material_equivalence: self
                                    .config
                                    .interaction
                                    .held_tool_equivalence,
                            };
                            match self
                                .minecraft
                                .select_tool_for_block(
                                    &block_id,
                                    &policy,
                                    &self.config.interaction.protected_tools,
                                    &self.config.interaction.reserved_tools,
                                )
                                .await
                            {
                                Ok(selection) => println!("{}", selection.explanation),
                                Err(error) => println!("Tool selection error: {error}"),
                            }
                        }
                        ConsoleCommand::PlaceLooked { block_id } => {
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            logging::info(format!("Placing {block_id}"));
                            match self.place_looked_and_wait(block_id, input_rx).await {
                                WaitOutcome::Finished(Ok(())) => logging::success("Block placed"),
                                WaitOutcome::Finished(Err(error)) => logging::error(error),
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::PlaceAt { x, y, z, block_id } => {
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            logging::info(format!("Placing {block_id} at ({x}, {y}, {z})"));
                            match self
                                .place_at_and_wait(
                                    crate::minecraft::world_state::BlockPosition { x, y, z },
                                    block_id,
                                    input_rx,
                                )
                                .await
                            {
                                WaitOutcome::Finished(Ok(())) => logging::success("Block placed"),
                                WaitOutcome::Finished(Err(error)) => logging::error(error),
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::InteractNearest {
                            block_id,
                            items,
                            radius,
                        } => {
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            match self
                                .interact_nearest_and_wait(block_id, items, radius, input_rx)
                                .await
                            {
                                WaitOutcome::Finished(Ok(())) => {}
                                WaitOutcome::Finished(Err(error)) => {
                                    logging::warning(format!(
                                        "Cannot interact with block: {error}"
                                    ));
                                }
                                WaitOutcome::Interrupted(next) => {
                                    input = next;
                                    continue;
                                }
                            }
                        }
                        ConsoleCommand::StopInteraction => {
                            self.interaction
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await
                        }
                        ConsoleCommand::InteractionStatus => self.print_interaction_status().await,
                        ConsoleCommand::Equip { item } => self.equip_item(item).await,
                        ConsoleCommand::OpenChest { x, y, z } => {
                            if let Err(error) = self
                                .container
                                .open(
                                    &self.minecraft,
                                    &self.movement,
                                    &self.block_navigation,
                                    crate::minecraft::world_state::BlockPosition { x, y, z },
                                )
                                .await
                            {
                                println!("Open chest failed: {error}");
                            }
                        }
                        ConsoleCommand::TakeItem { item_id, count } => {
                            if let Err(error) = self
                                .container
                                .transfer(&self.minecraft, TransferDirection::Take, item_id, count)
                                .await
                            {
                                println!("Take failed: {error}");
                            }
                        }
                        ConsoleCommand::StoreItem { item_id, count } => {
                            if let Err(error) = self
                                .container
                                .transfer(&self.minecraft, TransferDirection::Store, item_id, count)
                                .await
                            {
                                println!("Store failed: {error}");
                            }
                        }
                        ConsoleCommand::ContainerStatus => {
                            let s = self.container.status().await;
                            println!(
                                "Container: {:?}; target={:?}; menu={:?}; transferred={}/{}; outcome={:?}{}",
                                s.phase,
                                s.target,
                                s.window_id,
                                s.transferred,
                                s.requested,
                                s.outcome,
                                s.detail.map(|d| format!(" ({d})")).unwrap_or_default()
                            );
                        }
                        ConsoleCommand::CloseContainer => {
                            self.container.close(&self.minecraft).await
                        }
                        ConsoleCommand::TestOakLog => {
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            if let Err(error) = self
                                .interaction
                                .test_oak_log(&self.minecraft, &self.movement, &self.look)
                                .await
                            {
                                logging::warning(format!("Oak-log test failed: {error}"));
                            }
                        }
                        ConsoleCommand::Reconnect => {
                            let _ = self.movement.stop(&self.minecraft).await;
                            self.block_navigation
                                .cancel(&self.minecraft, &self.movement)
                                .await;
                            self.look.cancel().await;
                            self.interaction
                                .cancel(&self.minecraft, &self.movement, &self.look)
                                .await;
                            match self.minecraft.reconnect().await {
                                Ok(()) => println!("Reconnect successful."),
                                Err(error) => println!("Reconnect failed: {error}"),
                            }
                        }
                        ConsoleCommand::Quit => return Ok(true),
                    }
                }
                ConsoleInput::Empty => {}
            }
            return Ok(false);
        }
    }

    /// Human-readable description of whatever `/stop` is about to cancel, or
    /// `None` if there's genuinely nothing active -- read *before* cancelling
    /// so the message reflects what was actually running rather than always
    /// claiming success. `/goto`/`/goto-mine`/`/goto-block` await their
    /// `*_and_wait` helper directly, which blocks the console loop until they
    /// finish, so `/stop` can never race one of those; this mainly covers
    /// `/follow` (which returns immediately after submitting its goal) and
    /// an in-progress `/goto-block` search.
    async fn active_stop_description(&self) -> Option<String> {
        let movement = self.movement.snapshot().await;
        match movement.status {
            MovementStatus::FollowingPlayer => {
                return Some(match movement.target_player {
                    Some(player) => format!("following {player}"),
                    None => "following a player".to_owned(),
                });
            }
            MovementStatus::MovingToPosition => return Some("moving to position".to_owned()),
            _ => {}
        }

        let block_navigation = self.block_navigation.snapshot().await;
        if matches!(
            block_navigation.state,
            BlockNavigationState::Searching
                | BlockNavigationState::SelectingTarget
                | BlockNavigationState::Moving
                | BlockNavigationState::Repathing
        ) {
            return Some(match block_navigation.requested_block_id {
                Some(block_id) => format!("navigating to {block_id}"),
                None => "block navigation".to_owned(),
            });
        }

        if let Some(look) = self.interruptible_look().await {
            return Some(match look.target {
                Some(target) => format!("looking at {target}"),
                None => "looking".to_owned(),
            });
        }

        None
    }

    /// The current look, if it is one `/stop` is allowed to cancel: an
    /// active, `ExplicitCommand`-priority look (a plain `/look`/
    /// `/lookplayer`/`/lookblock`/`/lookentity`, or a task's own "look at"
    /// step). A `PreciseInteraction`-priority look -- `InteractionController`
    /// driving a precise aim mid-break/place -- is never returned here; that
    /// lifecycle is only ever ended by finishing or by `/stopinteraction`.
    async fn interruptible_look(&self) -> Option<LookSnapshot> {
        let snapshot = self.look.snapshot().await;
        (snapshot.state == LookState::Looking
            && snapshot.priority != LookPriority::PreciseInteraction)
            .then_some(snapshot)
    }

    async fn print_container_status(&self) {
        let snapshot = self.minecraft.world_state_snapshot().await.container;
        println!("Container observer (read-only):");
        println!("  session generation: {}", snapshot.session_generation);
        println!(
            "  open: {}  synced: {}  state: {:?}",
            snapshot.is_open, snapshot.is_synced, snapshot.sync_state
        );
        if let Some(identity) = snapshot.identity {
            println!(
                "  window: {}  type: {}  title: {}",
                identity.window_id,
                identity.menu_type,
                identity.title.as_deref().unwrap_or("unknown")
            );
            println!(
                "  position: {}",
                identity
                    .world_position
                    .map_or_else(|| "unknown".into(), |p| format!("{} {} {}", p.x, p.y, p.z))
            );
        }
        println!(
            "  revision: {}  container slots: {}  player slots: {}",
            snapshot
                .revision
                .map_or_else(|| "unknown".into(), |revision| revision.to_string()),
            snapshot.container_slots.len(),
            snapshot.player_slots.len()
        );
        println!(
            "  cursor: {}",
            snapshot
                .cursor
                .as_ref()
                .and_then(|slot| slot.item_id.as_deref())
                .map_or("empty".into(), |id| format!(
                    "{} x{}",
                    id,
                    snapshot.cursor.as_ref().map_or(0, |slot| slot.count)
                ))
        );
        println!(
            "  opened: {:?}  observed: {:?}  closed: {:?}",
            snapshot.opened_at, snapshot.observed_at, snapshot.closed_at
        );
    }

    async fn find_blocks(&self, block_id: String, radius: Option<u32>, limit: Option<usize>) {
        let radius = radius.unwrap_or(self.config.block_search.default_radius);
        let limit = limit.unwrap_or(self.config.block_search.default_result_limit);
        let query = BlockSearchQuery {
            block_id,
            radius,
            maximum_results: limit,
            vertical_range: self.config.block_search.default_vertical_range,
        };
        let nearest_only = limit == 1;
        let block_search = BlockSearchService::new(
            self.config.block_search.maximum_radius,
            self.config.block_search.maximum_result_limit,
            self.config.block_search.default_vertical_range,
        );
        match block_search
            .search_nearby(&self.minecraft, query.clone())
            .await
        {
            Ok(results) if nearest_only => {
                println!(
                    "{}",
                    format_nearest_result(&query.block_id, radius, results.first())
                );
            }
            Ok(results) => println!("{}", format_find_results(&query.block_id, radius, &results)),
            Err(error) => logging::warning(format!("Block search failed: {error}")),
        }
    }

    async fn print_block_navigation_status(&self) {
        let snapshot = self.block_navigation.snapshot().await;
        if matches!(snapshot.state, BlockNavigationState::Idle) {
            println!("No block navigation task is active.");
            return;
        }
        let state = match snapshot.state {
            BlockNavigationState::Moving | BlockNavigationState::Repathing => "Moving",
            BlockNavigationState::Searching => "Searching",
            BlockNavigationState::SelectingTarget => "SelectingTarget",
            BlockNavigationState::Reached => "Reached",
            BlockNavigationState::Cancelled => "Cancelled",
            BlockNavigationState::Failed => "Failed",
            BlockNavigationState::Idle => "Idle",
        };
        let position = self.minecraft.world_state_snapshot().await.bot.position;
        let distance =
            position
                .zip(snapshot.selected_approach_position)
                .map(|(current, target)| {
                    ((current.x - f64::from(target.x)).powi(2)
                        + (current.y - f64::from(target.y)).powi(2)
                        + (current.z - f64::from(target.z)).powi(2))
                    .sqrt()
                });
        println!("State: {state}");
        println!(
            "Block: {}",
            snapshot.requested_block_id.as_deref().unwrap_or("unknown")
        );
        println!(
            "Search radius: {}",
            snapshot
                .search_radius
                .map_or_else(|| "unknown".into(), |value| value.to_string())
        );
        println!(
            "Target block: {}",
            snapshot
                .selected_block_position
                .map_or_else(|| "unknown".into(), |p| format!("{} {} {}", p.x, p.y, p.z))
        );
        println!(
            "Approach position: {}",
            snapshot
                .selected_approach_position
                .map_or_else(|| "unknown".into(), |p| format!("{} {} {}", p.x, p.y, p.z))
        );
        println!(
            "Current position: {}",
            position.map_or_else(
                || "unknown".into(),
                |p| format!("{:.1} {:.1} {:.1}", p.x, p.y, p.z)
            )
        );
        println!(
            "Distance to approach: {}",
            distance.map_or_else(|| "unknown".into(), |value| format!("{value:.1}"))
        );
        println!("Candidates checked: {}", snapshot.candidates_checked);
        println!(
            "Attempts: {}/{}",
            snapshot.current_attempt, snapshot.maximum_attempts
        );
        println!(
            "Elapsed: {} seconds",
            snapshot
                .start_time
                .map_or(0, |started| started.elapsed().unwrap_or_default().as_secs())
        );
        if let Some(reason) = snapshot.failure_reason {
            println!("Failure reason: {reason}");
        }
    }

    async fn print_path_status(&self) {
        // The segmented planner first when it has anything to say: while it
        // is running, Azalea's own pathfinder status below describes only
        // the current waypoint hop, which on its own reads as a confusingly
        // short trip.
        let navigation = self.pathfinding.snapshot().await;
        if navigation.state != NavigationState::Idle {
            println!("{}", crate::pathfinding::debug::format_status(&navigation));
        }
        match self.minecraft.navigation_status().await {
            Ok(status) if status.calculating => println!("Pathfinder: calculating"),
            Ok(status) if status.executing => println!("Pathfinder: following path"),
            Ok(status) if status.reached => println!("Pathfinder: completed"),
            Ok(_) => println!("Pathfinder: idle or no path"),
            Err(error) => println!("Pathfinder unavailable: {error}"),
        }
    }

    async fn print_look_status(&self) {
        let snapshot = self.look.snapshot().await;
        if snapshot.state == LookState::Idle {
            println!("No look task is active.");
            return;
        }
        let state = match snapshot.state {
            LookState::Looking => "Looking",
            LookState::Completed => "Completed",
            LookState::Cancelled => "Cancelled",
            LookState::Failed => "Failed",
            LookState::Idle => "Idle",
        };
        println!("State: {state}");
        println!(
            "Target: {}",
            snapshot.target.as_deref().unwrap_or("unknown")
        );
        println!(
            "Precision: {}",
            snapshot
                .precision
                .map_or("unknown", |precision| match precision {
                    crate::look::aim_point::LookPrecision::Natural => "Natural",
                    crate::look::aim_point::LookPrecision::Precise => "Precise",
                    crate::look::aim_point::LookPrecision::Instant => "Instant",
                })
        );
        println!("Priority: {:?}", snapshot.priority);
        println!(
            "Target point: {}",
            snapshot.target_point.as_deref().unwrap_or("not available")
        );
        println!("Yaw: {}", fmt_opt(snapshot.yaw));
        println!("Pitch: {}", fmt_opt(snapshot.pitch));
        println!("Yaw speed: {} deg/s", fmt_opt(snapshot.yaw_speed));
        println!("Pitch speed: {} deg/s", fmt_opt(snapshot.pitch_speed));
        println!(
            "Elapsed: {:.1}s",
            snapshot.started_at.map_or(0.0, |started| started
                .elapsed()
                .unwrap_or_default()
                .as_secs_f64())
        );
        if let Some(reason) = snapshot.failure_reason {
            println!("Failure reason: {reason}");
        }
    }

    async fn print_interaction_status(&self) {
        let snapshot = self.interaction.snapshot().await;
        if snapshot.state == InteractionState::Idle {
            println!("No interaction is active.");
            return;
        }
        println!("State: {:?}", snapshot.state);
        println!(
            "Target: {}",
            snapshot.target.as_deref().unwrap_or("unknown")
        );
        println!(
            "Progress: {}",
            snapshot
                .progress_percent
                .map_or_else(|| "not available".into(), |value| format!("{value}%"))
        );
        println!(
            "Distance: {}",
            snapshot
                .distance
                .map_or_else(|| "unknown".into(), |value| format!("{value:.1}"))
        );
        println!(
            "Elapsed: {:.1}s",
            snapshot.started_at.map_or(0.0, |started| started
                .elapsed()
                .unwrap_or_default()
                .as_secs_f64())
        );
        println!("Retries: {}", snapshot.retries);
        if let Some(reason) = snapshot.failure_reason {
            println!("Failure reason: {reason}");
        }
    }

    async fn print_status(&self) {
        let status = self.minecraft.status();
        let world = self.minecraft.world_state_snapshot().await;
        println!("Connection state: {:?}", status.connection_state);
        println!("Bot username: {}", status.username);
        println!("Server address: {}", status.server);
        println!("Account mode: {}", status.account_mode);
        println!("Joined world: {}", world.joined_world());
        println!(
            "Position: {}",
            world.bot.position.map_or_else(
                || "unknown".into(),
                |p| format!("{:.2} {:.2} {:.2}", p.x, p.y, p.z)
            )
        );
        println!(
            "Block position: {}",
            world
                .bot
                .block_position
                .map_or_else(|| "unknown".into(), |p| format!("{} {} {}", p.x, p.y, p.z))
        );
        println!(
            "Yaw/Pitch: {}/{}",
            fmt_opt(world.bot.yaw),
            fmt_opt(world.bot.pitch)
        );
        println!(
            "Dimension: {}",
            world.bot.dimension.as_deref().unwrap_or("unknown")
        );
        println!(
            "Health: {}/{}",
            fmt_opt(world.bot.health),
            fmt_opt(world.bot.maximum_health)
        );
        println!(
            "Food: {}",
            world
                .bot
                .food_level
                .map_or_else(|| "unknown".into(), |v| v.to_string())
        );
        println!(
            "Selected hotbar slot: {}",
            world
                .bot
                .selected_hotbar_slot
                .map_or_else(|| "unknown".into(), |v| v.to_string())
        );
        println!(
            "Inventory item count: {}",
            world.inventory.total_counts.len()
        );
        println!("Nearby players: {}", world.players.len());
        println!("Nearby entities: {}", world.entities.len());
        println!(
            "Current task: {}",
            world
                .current_task
                .as_ref()
                .map_or("none", |t| t.name.as_str())
        );
        println!(
            "Time since last state update: {} seconds",
            world
                .last_updated_at
                .elapsed()
                .unwrap_or_default()
                .as_secs()
        );
        println!("Reconnect enabled: {}", status.reconnect.enabled);
        println!(
            "Reconnect delay: {} seconds",
            status.reconnect.delay_seconds
        );
        println!(
            "Reconnect maximum attempts: {}",
            status.reconnect.maximum_attempts
        );
        println!(
            "Application uptime: {} seconds",
            self.started_at.elapsed().as_secs()
        );
    }

    async fn print_where(&self) {
        let world = self.minecraft.world_state_snapshot().await;
        match world.bot.position {
            Some(p) => logging::info(format!(
                "Position: {:.2} {:.2} {:.2} (dimension: {})",
                p.x,
                p.y,
                p.z,
                world.bot.dimension.as_deref().unwrap_or("unknown")
            )),
            None => logging::error("Position unavailable"),
        }
    }

    async fn print_health(&self) {
        let world = self.minecraft.world_state_snapshot().await;
        logging::info(format!(
            "Health: {}/{}  Food: {}",
            fmt_opt(world.bot.health),
            fmt_opt(world.bot.maximum_health),
            world
                .bot
                .food_level
                .map_or_else(|| "unknown".into(), |v| v.to_string())
        ));
    }

    async fn equip_item(&self, item: String) {
        match self.minecraft.select_item_in_hotbar(&item).await {
            Ok(true) => logging::success(format!("Equipped {item}")),
            Ok(false) => logging::error(format!("{item} not found in hotbar")),
            Err(error) => logging::error(error),
        }
    }

    async fn print_players(&self) {
        let world = self.minecraft.world_state_snapshot().await;
        if world.players.is_empty() {
            println!("No known players.");
            return;
        }
        for player in world.players {
            let distance = player
                .distance
                .map_or_else(|| "unknown".into(), |d| format!("{d:.1}"));
            let position = player.position.map_or_else(
                || "unknown".into(),
                |p| format!("{:.1} {:.1} {:.1}", p.x, p.y, p.z),
            );
            println!(
                "{} | {} | {} | {} | {}",
                player.username, player.uuid, distance, position, player.loaded
            );
        }
    }

    async fn print_inventory(&self) {
        let world = self.minecraft.world_state_snapshot().await;
        println!(
            "Inventory available: {} | revision {}",
            world.inventory.available, world.inventory.revision
        );
        println!(
            "Selected hotbar slot: {}",
            world
                .inventory
                .selected_hotbar_slot
                .map_or_else(|| "unknown".into(), |v| v.to_string())
        );
        println!(
            "Selected item: {}",
            world.inventory.selected_item().map_or_else(
                || "unknown".into(),
                |i| format!("{} x{}", i.item_id.as_deref().unwrap_or("unknown"), i.count)
            )
        );
        let used_slots = world
            .inventory
            .slots
            .iter()
            .filter(|slot| slot.item_id.is_some())
            .count();
        println!(
            "Occupied slots: {} / {}",
            used_slots,
            world.inventory.slots.len()
        );
        println!(
            "Distinct item kinds: {}",
            world.inventory.total_counts.len()
        );
        let mut items: Vec<_> = world.inventory.total_counts.iter().collect();
        items.sort_by_key(|(id, _)| *id);
        for (id, count) in items {
            println!("{id} x{count}");
        }
        for slot in world
            .inventory
            .slots
            .iter()
            .filter(|slot| slot.item_id.is_some())
        {
            let name = slot
                .display_name
                .as_deref()
                .map_or_else(String::new, |name| format!(" name={name:?}"));
            println!(
                "slot {} {} x{}{}",
                slot.slot,
                slot.item_id.as_deref().unwrap_or("unknown"),
                slot.count,
                name
            );
        }
    }

    /// Lets a player run a real console command directly from Minecraft chat
    /// by prefixing it with `#` -- e.g. typing `#goto 100 64 20` in chat runs
    /// exactly what typing `/goto 100 64 20` in the local console would.
    /// `/quit` is hard-blocked regardless of access config: shutting the
    /// whole bot down from a stray chat message should never be possible.
    async fn handle_chat_console_command(
        &mut self,
        sender: Option<String>,
        sender_uuid: Option<uuid::Uuid>,
        command_text: &str,
        input_rx: &mut InputReceiver,
    ) {
        let command_text = command_text.trim();
        if command_text.is_empty() {
            return;
        }
        let Some(name) = sender else {
            return;
        };
        if !self.chat_access_allowed(&name) {
            logging::warning(format!(
                "[Chat] Command from {name} rejected: access denied"
            ));
            return;
        }
        if !self.consume_chat_rate_limit(&name, sender_uuid) {
            logging::warning(format!("[Chat] Command from {name} rejected: rate limited"));
            return;
        }
        let input_line = format!("/{command_text}");
        let input = match console::commands::parse_input(&input_line) {
            Ok(input) => input,
            Err(error) => {
                logging::warning(format!(
                    "[Chat] Invalid command from {name} ({input_line}): {error}"
                ));
                return;
            }
        };
        if matches!(input, ConsoleInput::Command(ConsoleCommand::Quit)) {
            logging::warning(format!(
                "[Chat] {name} tried to run /quit from chat; ignored"
            ));
            return;
        }
        logging::info(format!("[Chat] {name} ran: {input_line}"));
        if let Err(error) = self.execute_console_input(input, input_rx).await {
            logging::warning(format!("[Chat] Command error: {error}"));
        }
    }

    fn chat_access_allowed(&self, player_name: &str) -> bool {
        let access = &self.config.chat_commands.access;
        // Azalea does not expose a stable operator permission signal here.
        // Do not guess: operators_only rejects until a trusted adapter exists.
        if access.operators_only
            || access
                .blocked_players
                .iter()
                .any(|name| name.eq_ignore_ascii_case(player_name))
        {
            return false;
        }
        access.allowed_players.is_empty()
            || access
                .allowed_players
                .iter()
                .any(|name| name.eq_ignore_ascii_case(player_name))
    }

    fn consume_chat_rate_limit(
        &mut self,
        player_name: &str,
        player_uuid: Option<uuid::Uuid>,
    ) -> bool {
        let limit = &self.config.chat_commands.rate_limit;
        if !limit.enabled {
            return true;
        }
        let key = player_uuid.map_or_else(|| player_name.to_ascii_lowercase(), |id| id.to_string());
        let now = Instant::now();
        let entries = self.chat_rate_limits.entry(key).or_default();
        while entries
            .front()
            .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(limit.window_seconds))
        {
            entries.pop_front();
        }
        if entries.len() >= limit.requests {
            return false;
        }
        entries.push_back(now);
        true
    }

    /// Drains incoming player chat and dispatches `#`-prefixed messages to
    /// `handle_chat_console_command`. Everything else is ignored.
    async fn tick_chat_commands(&mut self, input_rx: &mut InputReceiver) {
        while let Some(chat) = self.minecraft.pop_incoming_player_chat().await {
            if chat.kind != crate::minecraft::world_state::ChatMessageKind::Player {
                continue;
            }
            if let Some(command_text) = chat.text.strip_prefix('#') {
                self.handle_chat_console_command(
                    chat.sender,
                    chat.sender_uuid,
                    command_text,
                    input_rx,
                )
                .await;
            }
        }
    }

    async fn print_entities(&self, radius: Option<u32>) {
        let world = self.minecraft.world_state_snapshot().await;
        let radius = f64::from(radius.unwrap_or(64));
        if !(radius > 0.0 && radius <= 256.0) {
            println!("Entity query error: radius must be between 0 and 256");
            return;
        }
        for entity in world
            .entities
            .iter()
            .filter(|e| e.alive != Some(false) && e.health.is_none_or(|health| health > 0.0))
            .filter(|e| e.distance <= radius)
            .take(64)
        {
            println!(
                "{} | distance {:.1} | {:.2} {:.2} {:.2}",
                entity.entity_type,
                entity.distance,
                entity.position.x,
                entity.position.y,
                entity.position.z
            );
        }
    }

    async fn print_movement(&self) {
        let world = self.minecraft.world_state_snapshot().await;
        let movement = world.movement;
        let local_input = self.movement.local_input().await;
        println!("Movement state: {:?}", movement.status);
        println!(
            "Destination: {}",
            movement.destination.map_or_else(
                || "unknown".into(),
                |p| format!("{:.2} {:.2} {:.2}", p.x, p.y, p.z)
            )
        );
        println!(
            "Current position: {}",
            world.bot.position.map_or_else(
                || "unknown".into(),
                |p| format!("{:.2} {:.2} {:.2}", p.x, p.y, p.z)
            )
        );
        println!(
            "Remaining distance: {}",
            movement
                .estimated_distance
                .map_or_else(|| "unknown".into(), |d| format!("{d:.2}"))
        );
        println!(
            "Target player: {}",
            movement.target_player.as_deref().unwrap_or("none")
        );
        println!(
            "Elapsed time: {} seconds",
            movement
                .started_at
                .map_or(0, |at| at.elapsed().unwrap_or_default().as_secs())
        );
        println!(
            "Movement adaptation: forward={} backward={} left={} right={} sprint={} speed={:.0}%",
            local_input.forward,
            local_input.backward,
            local_input.left,
            local_input.right,
            local_input.sprint,
            local_input.speed_multiplier * 100.0,
        );
        if let Some(reason) = movement.failure_reason {
            println!("Failure reason: {reason}");
        }
    }

    // ---- Direct service calls with completion polling ----------------------
    //
    // Each of these calls the owning service directly (no task queue, no
    // resource leasing, no task IDs -- "the bot does it directly"), waits for
    // it to reach a terminal state via the `await_*_terminal` pollers above,
    // and shows a lightweight name in `/status`'s "Current task" line for the
    // duration (purely a display aid; nothing reads it back programmatically).

    /// Whether a destination is far enough away to be worth the segmented
    /// planner (see `crate::pathfinding`). Short hops go straight to the
    /// movement layer as they always have: slicing a 40-block walk into
    /// segments only adds planning latency to something Azalea's own
    /// pathfinder already does reliably. Distance is measured from the bot's
    /// live position, so `/goto` on a nearby coordinate behaves exactly as
    /// it did before this system existed.
    async fn use_segmented_navigation(
        &self,
        destination: crate::minecraft::world_state::PositionSnapshot,
    ) -> bool {
        if !self.config.pathfinding.enabled {
            return false;
        }
        let world = self.minecraft.world_state_snapshot().await;
        let Some(position) = world.bot.position else {
            return false;
        };
        let dx = destination.x - position.x;
        let dy = destination.y - position.y;
        let dz = destination.z - position.z;
        (dx * dx + dy * dy + dz * dz).sqrt() > self.config.pathfinding.long_distance_threshold
    }

    async fn goto_and_wait(
        &self,
        name: &str,
        destination: crate::minecraft::world_state::PositionSnapshot,
        mode: NavigationMode,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        self.minecraft.set_current_task(task_snapshot(name)).await;
        let segmented = self.use_segmented_navigation(destination).await;
        let result = async {
            if segmented {
                if let Err(error) = self.pathfinding.start(&self.minecraft, destination).await {
                    return WaitOutcome::Finished(Err(error));
                }
                return await_navigation_terminal(self, input_rx).await;
            }
            if let Err(error) = self.movement.goto(&self.minecraft, destination, mode).await {
                return WaitOutcome::Finished(Err(error));
            }
            await_movement_terminal(self, input_rx).await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    /// `/goto <player>` (also reachable as `#goto <player>`): a one-shot
    /// walk to wherever the named player currently is, then stop -- unlike
    /// `/follow`, this never re-tracks them after arriving. Shares
    /// `approach_player` with `#drop <item> <amount> <player>`'s approach
    /// step.
    async fn goto_player_and_wait(
        &self,
        player: String,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        self.minecraft
            .set_current_task(task_snapshot(format!("Go to player {player}")))
            .await;
        let result = async {
            match self.approach_player(player, input_rx).await {
                Ok(resolved_name) => {
                    logging::success(format!("Reached player {resolved_name}"));
                    WaitOutcome::Finished(Ok(()))
                }
                Err(outcome) => outcome,
            }
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    /// Resolves `player` among currently loaded players
    /// (`WorldStateSnapshot::find_loaded_player_by_name`) and walks to
    /// within `PLAYER_APPROACH_DISTANCE` blocks of them -- close enough to
    /// interact, never all the way onto their exact tile. Shared by
    /// `#goto <player>` and `#drop <item> <amount> <player>`. On success,
    /// returns the resolved (correct-case) username; on failure, the
    /// `WaitOutcome` the caller should return directly (already logged).
    async fn approach_player(
        &self,
        player: String,
        input_rx: &mut InputReceiver,
    ) -> Result<String, WaitOutcome> {
        logging::info(format!("Searching for player {player}..."));
        let world = self.minecraft.world_state_snapshot().await;
        let target = world
            .find_loaded_player_by_name(&player)
            .filter(|target| target.position.is_some());
        let Some(target) = target else {
            logging::error(format!("Player not found: {player}"));
            return Err(WaitOutcome::Finished(Err(AppError::UnknownPlayer(player))));
        };
        let resolved_name = target.username.clone();
        let destination = target.position.expect("checked by filter above");

        logging::info(format!(
            "Going to player {resolved_name} at ({:.0}, {:.0}, {:.0})",
            destination.x, destination.y, destination.z
        ));
        if let Err(error) = self
            .movement
            .goto_player_approach(
                &self.minecraft,
                destination,
                NavigationMode::AllowMining,
                PLAYER_APPROACH_DISTANCE,
            )
            .await
        {
            logging::error(format!("Could not reach {resolved_name}: {error}"));
            return Err(WaitOutcome::Finished(Err(error)));
        }
        match await_movement_terminal(self, input_rx).await {
            WaitOutcome::Finished(Ok(())) => Ok(resolved_name),
            WaitOutcome::Finished(Err(error)) => {
                logging::error(format!("Could not reach {resolved_name}: {error}"));
                Err(WaitOutcome::Finished(Err(error)))
            }
            interrupted @ WaitOutcome::Interrupted(_) => Err(interrupted),
        }
    }

    async fn goto_block_and_wait(
        &self,
        block_id: String,
        radius: u32,
        mode: NavigationMode,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        self.minecraft
            .set_current_task(task_snapshot(format!("Go to {block_id}")))
            .await;
        let result = async {
            if let Err(error) = self
                .block_navigation
                .start(&self.minecraft, &self.movement, block_id, radius, mode)
                .await
            {
                return WaitOutcome::Finished(Err(error));
            }
            await_block_navigation_terminal(self, input_rx).await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    /// Item-based `/get <item> <amount>` (also reachable as `#get <item>
    /// <amount>` from Minecraft chat). `item` never names a block to mine
    /// directly -- `resolve_and_run_get_resource` is the single place that
    /// decides how it's obtained, via `mobs::resolve_resource`, and
    /// dispatches to `run_get_item` (ore/conversion source blocks) or
    /// `run_get_mob` (a mob drop) accordingly -- the caller here stays
    /// resource-kind-agnostic. Contrast with `/mine`
    /// (`run_mine`/`mine_and_wait`), which targets a block directly and
    /// never resolves anything.
    async fn get_resource_and_wait(
        &self,
        resource_id: String,
        amount: u32,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        self.minecraft
            .set_current_task(task_snapshot(format!("Get {amount} {resource_id}")))
            .await;
        logging::milestone(format!("Get task started: {resource_id} x{amount}"));
        let result = self
            .resolve_and_run_get_resource(&resource_id, amount, input_rx)
            .await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn resolve_and_run_get_resource(
        &self,
        resource_id: &str,
        amount: u32,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        match mobs::resolve_resource(resource_id) {
            Ok(mobs::ResourceKind::Ore {
                resource_id,
                blocks,
            }) => {
                self.run_get_item(&resource_id, &blocks, amount, input_rx)
                    .await
            }
            Ok(mobs::ResourceKind::Mob { mob_id, .. }) => {
                self.run_get_mob(resource_id, &mob_id, amount, input_rx)
                    .await
            }
            // Unreachable in practice: `resource_id` was already validated
            // by the same `resolve_resource` call at parse time (see
            // `console::commands::parse_get`). Handled defensively anyway
            // rather than assumed, per "never crash".
            Err(error) => {
                logging::error(format!("Block not found: {resource_id}"));
                logging::milestone("Get task cancelled");
                WaitOutcome::Finished(Err(error))
            }
        }
    }

    /// Deliberately not a separate gathering system: each iteration reuses
    /// `BlockNavigationService::start_multi` (nearest-*reachable* block
    /// among every candidate source, already falling back across candidates
    /// and never retrying an approach it just proved impossible -- see that
    /// type's `try_next_target`) to walk to a fresh scan of loaded chunks,
    /// then `InteractionController::break_at` (tool selection, precise
    /// look, break, verified removal) on the exact block navigation just
    /// reached.
    ///
    /// Mining and inventory-counting deliberately target different ids:
    /// `block_ids` (what gets scanned for, navigated to, and broken -- one
    /// or more candidate source blocks, e.g. `diamond_ore` and
    /// `deepslate_diamond_ore` for `diamond`, resolved once up front by
    /// `mobs::resolve_resource`) is never what inventory is counted
    /// against. `resource_id` is; using a mined block's id there would wait
    /// on a count that can never increase for any block whose drop differs
    /// from itself.
    async fn run_get_item(
        &self,
        resource_id: &str,
        block_ids: &[String],
        amount: u32,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        let radius = self.config.block_navigation.maximum_search_radius;
        let resource_label = blocks::bare_id(resource_id).to_owned();
        let block_label = join_labels(block_ids, "/");
        if !(block_ids.len() == 1 && block_ids[0] == resource_id) {
            logging::info(format!(
                "Gathering {resource_label} by mining {block_label}"
            ));
        }
        let mut consecutive_failures: u32 = 0;
        loop {
            let current = self
                .minecraft
                .world_state_snapshot()
                .await
                .inventory
                .count_item(resource_id);
            logging::info(format!("Inventory: {current}/{amount}"));
            if get_resource_satisfied(current, amount) {
                logging::success(format!("Collected {amount} {resource_label}"));
                return WaitOutcome::Finished(Ok(()));
            }

            logging::info(format!("Scanning loaded chunks for {block_label}..."));
            if let Err(error) = self
                .block_navigation
                .start_multi(
                    &self.minecraft,
                    &self.movement,
                    block_ids.to_vec(),
                    radius,
                    NavigationMode::AllowMining,
                )
                .await
            {
                return self.fail_get_item(&block_label, error).await;
            }
            match await_block_navigation_terminal(self, input_rx).await {
                WaitOutcome::Finished(Ok(())) => {}
                WaitOutcome::Finished(Err(error)) => {
                    return self.fail_get_item(&block_label, error).await;
                }
                WaitOutcome::Interrupted(next) => return WaitOutcome::Interrupted(next),
            }

            let navigation_snapshot = self.block_navigation.snapshot().await;
            let Some(target) = navigation_snapshot.selected_block_position else {
                return self
                    .fail_get_item(&block_label, AppError::NoMatchingBlock)
                    .await;
            };
            // The block actually reached this iteration, not the whole
            // candidate set -- `#mine diamond_ore deepslate_diamond_ore`-style
            // multi-source runs must say which one is actually being broken.
            let mined_label = navigation_snapshot
                .selected_block_id
                .as_deref()
                .map(blocks::bare_id)
                .unwrap_or(&block_label)
                .to_owned();

            logging::info(format!("Looking at {mined_label}"));
            logging::info(format!("Breaking {mined_label}"));
            if let Err(error) = self
                .interaction
                .break_at(&self.minecraft, &self.movement, &self.look, target)
                .await
            {
                consecutive_failures += 1;
                logging::warning(format!("Could not break {mined_label}: {error}"));
                if get_resource_should_abort(consecutive_failures) {
                    return self.fail_get_item(&block_label, error).await;
                }
                continue;
            }
            match await_interaction_terminal(self, input_rx).await {
                WaitOutcome::Finished(Ok(())) => {
                    consecutive_failures = 0;
                    if let Some(next) = self.collect_drop_at(target, resource_id, input_rx).await {
                        return WaitOutcome::Interrupted(next);
                    }
                    let new_count = self
                        .minecraft
                        .world_state_snapshot()
                        .await
                        .inventory
                        .count_item(resource_id);
                    logging::progress(format!("Collected {resource_label} ({new_count}/{amount})"));
                }
                WaitOutcome::Finished(Err(error)) => {
                    consecutive_failures += 1;
                    logging::warning(format!("Could not break {mined_label}: {error}"));
                    if get_resource_should_abort(consecutive_failures) {
                        return self.fail_get_item(&block_label, error).await;
                    }
                }
                WaitOutcome::Interrupted(next) => return WaitOutcome::Interrupted(next),
            }
        }
    }

    /// Common failure exit for `run_get_item`: stops movement/navigation,
    /// reports the block-not-found message the `#get` contract requires
    /// (naming every candidate source block that was searched for), and
    /// hands the underlying error back to the caller.
    async fn fail_get_item(&self, block_label: &str, error: AppError) -> WaitOutcome {
        self.block_navigation
            .cancel(&self.minecraft, &self.movement)
            .await;
        logging::error(format!("Block not found: {block_label}"));
        logging::milestone("Get task cancelled");
        WaitOutcome::Finished(Err(error))
    }

    /// Direct `/mine <block> [block...] <amount>` (also reachable as `#mine
    /// ...` from Minecraft chat). The counterpart to `run_get_item` with
    /// the item-resolution step removed entirely: `block_ids` is exactly
    /// what the caller typed, searched for and broken as-is, and progress
    /// is a plain count of blocks destroyed -- never inventory, since a
    /// block's drop (or lack of one -- silk touch, fortune, "drops
    /// nothing") is deliberately none of `/mine`'s concern. Otherwise
    /// structurally identical to `run_get_item`: same fresh-search-every-
    /// iteration loop over `BlockNavigationService::start_multi`, same
    /// `InteractionController::break_at` mining pipeline, same bounded
    /// consecutive-failure abort.
    async fn mine_and_wait(
        &self,
        block_ids: Vec<String>,
        amount: u32,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        let label = join_labels(&block_ids, ", ");
        self.minecraft
            .set_current_task(task_snapshot(format!("Mine {amount} {label}")))
            .await;
        logging::milestone(format!("Mining {label}"));
        let result = self.run_mine(&block_ids, amount, input_rx).await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn run_mine(
        &self,
        block_ids: &[String],
        amount: u32,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        let radius = self.config.block_navigation.maximum_search_radius;
        let label = join_labels(block_ids, ", ");
        let mut mined: u32 = 0;
        let mut consecutive_failures: u32 = 0;
        loop {
            if mined >= amount {
                logging::success(format!("Mined {mined} {label} blocks"));
                return WaitOutcome::Finished(Ok(()));
            }

            logging::info(format!("Scanning loaded chunks for {label}..."));
            if let Err(error) = self
                .block_navigation
                .start_multi(
                    &self.minecraft,
                    &self.movement,
                    block_ids.to_vec(),
                    radius,
                    NavigationMode::AllowMining,
                )
                .await
            {
                return self.fail_mine(&label, mined, error).await;
            }
            match await_block_navigation_terminal(self, input_rx).await {
                WaitOutcome::Finished(Ok(())) => {}
                WaitOutcome::Finished(Err(error)) => {
                    return self.fail_mine(&label, mined, error).await;
                }
                WaitOutcome::Interrupted(next) => return WaitOutcome::Interrupted(next),
            }

            let navigation_snapshot = self.block_navigation.snapshot().await;
            let Some(target) = navigation_snapshot.selected_block_position else {
                return self
                    .fail_mine(&label, mined, AppError::NoMatchingBlock)
                    .await;
            };
            let mined_label = navigation_snapshot
                .selected_block_id
                .as_deref()
                .map(blocks::bare_id)
                .unwrap_or(&label)
                .to_owned();

            logging::info(format!("Looking at {mined_label}"));
            logging::info(format!("Mining {mined_label}"));
            if let Err(error) = self
                .interaction
                .break_at(&self.minecraft, &self.movement, &self.look, target)
                .await
            {
                consecutive_failures += 1;
                logging::warning(format!("Could not mine {mined_label}: {error}"));
                if get_resource_should_abort(consecutive_failures) {
                    return self.fail_mine(&label, mined, error).await;
                }
                continue;
            }
            match await_interaction_terminal(self, input_rx).await {
                WaitOutcome::Finished(Ok(())) => {
                    consecutive_failures = 0;
                    mined += 1;
                    logging::progress(format!("Mined {mined_label} ({mined}/{amount})"));
                }
                WaitOutcome::Finished(Err(error)) => {
                    consecutive_failures += 1;
                    logging::warning(format!("Could not mine {mined_label}: {error}"));
                    if get_resource_should_abort(consecutive_failures) {
                        return self.fail_mine(&label, mined, error).await;
                    }
                }
                WaitOutcome::Interrupted(next) => return WaitOutcome::Interrupted(next),
            }
        }
    }

    /// Common failure exit for `run_mine`: stops movement/navigation,
    /// reports how many blocks were actually destroyed before the failure
    /// (if any -- a partial run is still real progress, not a total loss),
    /// and hands the underlying error back to the caller.
    async fn fail_mine(&self, label: &str, mined: u32, error: AppError) -> WaitOutcome {
        self.block_navigation
            .cancel(&self.minecraft, &self.movement)
            .await;
        if mined > 0 {
            logging::warning(format!("Mined {mined} {label} blocks before stopping"));
        }
        logging::error(format!("Block not found: {label}"));
        logging::milestone("Mine task cancelled");
        WaitOutcome::Finished(Err(error))
    }

    /// Baritone-style `/drop <item> <amount> [player]` (also reachable as
    /// `#drop <item> <amount> [player]` from Minecraft chat). Reuses the
    /// same inventory (`world_state_snapshot().inventory`), pathfinding
    /// (`MovementService::goto_for_block_navigation` -- the same one-shot
    /// "walk to a fixed destination and stop" primitive `/goto` uses), and
    /// look (`LookController` via `look_and_wait`) systems as every other
    /// command; only `items::drop_plan::plan_drop` (which slots to throw
    /// from, whole-stack vs single-item) and `MinecraftClient::drop_click`
    /// (the actual throw) are new. See `run_drop_without_player` and
    /// `run_drop_to_player` for the two branches.
    async fn drop_and_wait(
        &self,
        item_id: String,
        amount: u32,
        player: Option<String>,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        let label = blocks::bare_id(&item_id).to_owned();
        self.minecraft
            .set_current_task(task_snapshot(format!("Drop {amount} {label}")))
            .await;
        logging::milestone(format!("Drop task started: {label} x{amount}"));
        let result = match player {
            Some(player) => {
                self.run_drop_to_player(&item_id, &label, amount, player, input_rx)
                    .await
            }
            None => self.run_drop_without_player(&item_id, &label, amount).await,
        };
        self.minecraft.clear_current_task().await;
        result
    }

    /// `#drop <item> <amount>`: no navigation or look involved at all --
    /// just count, verify, and throw straight out of wherever the bot is
    /// currently standing.
    async fn run_drop_without_player(
        &self,
        item_id: &str,
        label: &str,
        amount: u32,
    ) -> WaitOutcome {
        let have = self
            .minecraft
            .world_state_snapshot()
            .await
            .inventory
            .count_item(item_id);
        logging::info(format!("Inventory: {have} {label}"));
        match self.execute_drop(item_id, label, amount).await {
            Ok(()) => {
                logging::success(format!("Dropped {amount} {label}"));
                WaitOutcome::Finished(Ok(()))
            }
            Err(error) => {
                logging::error(&error);
                WaitOutcome::Finished(Err(error))
            }
        }
    }

    /// `#drop <item> <amount> <player>`: bails out before doing anything else
    /// if the inventory can't satisfy `amount` (see `check_drop_available`
    /// -- no point walking anywhere first), then locates the player and
    /// walks to within `PLAYER_APPROACH_DISTANCE` of them (`approach_player`,
    /// shared with `#goto <player>`), aims at them (`look_at_player_briefly`
    /// -- bounded, since a player look target never reaches a terminal
    /// "aimed" state on its own), then throws. Bails out with the exact
    /// `Player not found: {name}` message the contract requires the moment
    /// the player can't be resolved -- never guessing a stale/unloaded
    /// position.
    async fn run_drop_to_player(
        &self,
        item_id: &str,
        label: &str,
        amount: u32,
        player: String,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        if let Err(error) = self.check_drop_available(item_id, label, amount).await {
            logging::error(&error);
            return WaitOutcome::Finished(Err(error));
        }

        let resolved_name = match self.approach_player(player, input_rx).await {
            Ok(name) => name,
            Err(outcome) => return outcome,
        };

        match self.look_at_player_briefly(&resolved_name, input_rx).await {
            WaitOutcome::Finished(Ok(())) => {}
            WaitOutcome::Finished(Err(error)) => {
                logging::error(format!("Could not look at {resolved_name}: {error}"));
                return WaitOutcome::Finished(Err(error));
            }
            WaitOutcome::Interrupted(next) => return WaitOutcome::Interrupted(next),
        }

        match self.execute_drop(item_id, label, amount).await {
            Ok(()) => {
                logging::success(format!("Dropped {amount} {label} to {resolved_name}"));
                WaitOutcome::Finished(Ok(()))
            }
            Err(error) => {
                logging::error(&error);
                WaitOutcome::Finished(Err(error))
            }
        }
    }

    /// Aims at `player` for up to `PLAYER_LOOK_SETTLE_TIMEOUT` and then
    /// proceeds regardless. A player look target tracks movement, so
    /// `LookState` never reaches `Completed` for it --
    /// `look_controller::tick_generation`'s `completed` computation is
    /// gated on `!context.tracks_movement`, which is false for a player.
    /// Waiting for that terminal state the way `look_and_wait` does for a
    /// fixed target (block/position) would hang forever: this is exactly
    /// what made `#drop <item> <amount> <player>` freeze the whole
    /// console/chat command pipeline (the wait never completed on its own,
    /// and chat-issued `#stop` can't reach a pipeline stuck awaiting inside
    /// a chat-issued command). The aim itself is still live and keeps
    /// refining every subsequent tick regardless of this bound.
    async fn look_at_player_briefly(
        &self,
        player: &str,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        self.minecraft
            .set_current_task(task_snapshot("Look at player"))
            .await;
        let result = async {
            if let Err(error) = self
                .look
                .look_at(&self.minecraft, LookTarget::Player(player.to_owned()))
                .await
            {
                return WaitOutcome::Finished(Err(error));
            }
            let deadline = Instant::now() + PLAYER_LOOK_SETTLE_TIMEOUT;
            loop {
                self.look.tick(&self.minecraft).await;
                let snapshot = self.look.snapshot().await;
                match snapshot.state {
                    LookState::Completed | LookState::Idle => {
                        return WaitOutcome::Finished(Ok(()));
                    }
                    LookState::Cancelled => {
                        return WaitOutcome::Finished(Err(AppError::LookCancelled));
                    }
                    LookState::Failed => {
                        return WaitOutcome::Finished(Err(AppError::LookUnavailableWithReason(
                            snapshot
                                .failure_reason
                                .unwrap_or_else(|| "look failed".into()),
                        )));
                    }
                    LookState::Looking => {}
                }
                if Instant::now() >= deadline {
                    return WaitOutcome::Finished(Ok(()));
                }
                if let Some(input) = wait_tick(self, input_rx, Duration::from_millis(50)).await {
                    return WaitOutcome::Interrupted(input);
                }
            }
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    /// Fails fast with the exact `Item not found: {label}` (nothing held at
    /// all) or `Not enough {label} in inventory (have X, need Y)` (held,
    /// but not enough) message the contract requires -- checked against a
    /// live inventory read, without planning or throwing anything.
    async fn check_drop_available(
        &self,
        item_id: &str,
        label: &str,
        amount: u32,
    ) -> Result<(), AppError> {
        let inventory = self.minecraft.world_state_snapshot().await.inventory;
        match drop_plan::plan_drop(&inventory.slots, item_id, amount) {
            Ok(_) => Ok(()),
            Err(DropPlanError::Insufficient { available }) => {
                Err(drop_insufficient_error(label, amount, available))
            }
        }
    }

    /// Shared final step for both `#drop` branches: plans the exact throw
    /// clicks for `amount` of `item_id` against a fresh inventory read (so a
    /// player-drop's walk-and-look never operates on a stale count from
    /// before it moved), fires them via `MinecraftClient::drop_click`, and
    /// polls briefly for the inventory count to confirm the drop actually
    /// landed. Never drops a partial amount -- an insufficient inventory is
    /// rejected by `items::drop_plan::plan_drop` before anything is thrown.
    async fn execute_drop(&self, item_id: &str, label: &str, amount: u32) -> Result<(), AppError> {
        let inventory = self.minecraft.world_state_snapshot().await.inventory;
        let have_before = inventory.count_item(item_id);
        let clicks = drop_plan::plan_drop(&inventory.slots, item_id, amount).map_err(
            |DropPlanError::Insufficient { available }| {
                drop_insufficient_error(label, amount, available)
            },
        )?;

        logging::info(format!("Dropping {amount} {label}"));
        for click in clicks {
            self.minecraft.drop_click(0, click).await?;
        }

        // Client-side prediction applies each throw immediately against the
        // live ECS (see `MinecraftClient::drop_click`'s doc comment), but
        // `world_state_snapshot` only mirrors that once per external tick --
        // give it a few ticks to catch up rather than reporting success
        // before the cached count actually reflects it.
        let expected = have_before.saturating_sub(amount);
        for _ in 0..10 {
            if self
                .minecraft
                .world_state_snapshot()
                .await
                .inventory
                .count_item(item_id)
                <= expected
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        logging::warning(format!(
            "Dropped {amount} {label}, but inventory has not confirmed it yet"
        ));
        Ok(())
    }

    /// Scans for the dropped item entity the just-broken block spawned and
    /// walks onto wherever it actually landed, so vanilla's proximity item
    /// pickup triggers before the next search starts. Breaking happens from
    /// pickaxe reach (up to `interaction_distance`, ~4.5 blocks), which is
    /// well beyond the pickup radius -- without walking over, the drop is
    /// frequently left sitting on the ground uncollected, the inventory
    /// count never advances, and every subsequent iteration has to search
    /// further and further outward for still-unmined ore instead of
    /// registering the progress that already happened.
    ///
    /// The walk targets the drop's own live position
    /// (`nearest_dropped_item_position`), not the mined block's center --
    /// physics can carry an item off the block entirely (it rolls, bounces
    /// off a neighboring block, or falls into an opening the break just
    /// exposed), and standing over an empty block while the item sits a
    /// pace away never collects it. Re-scanned every loop iteration so the
    /// destination tracks the item while it's still settling; falls back to
    /// the block's own center only when no matching drop has been observed
    /// yet (the entity's spawn packet simply hasn't arrived this tick).
    ///
    /// Bounded by its own short timeout (not
    /// `MovementConfig::maximum_navigation_seconds`, which is far too
    /// generous for a one-or-two-block walk) so a pickup that can't
    /// complete for some reason -- another player grabs it first, the item
    /// despawns, the spot is unreachable -- can't stall the whole `#get`
    /// run; the caller just moves on and re-checks inventory as usual.
    /// Returns `Some` only if the wait was interrupted by new console input,
    /// mirroring the `WaitOutcome::Interrupted` contract.
    async fn collect_drop_at(
        &self,
        target: crate::minecraft::world_state::BlockPosition,
        resource_id: &str,
        input_rx: &mut InputReceiver,
    ) -> Option<ConsoleInput> {
        let block_center = crate::minecraft::world_state::PositionSnapshot {
            x: f64::from(target.x) + 0.5,
            y: f64::from(target.y),
            z: f64::from(target.z) + 0.5,
        };
        // Vanilla's actual item pickup range is much tighter than
        // `MovementConfig::arrival_distance` (1.5 blocks by default, tuned
        // for "close enough to interact/mine" -- not "standing on top of a
        // dropped item"). `MovementService::tick_goto` stops Azalea's
        // pathfinder the moment *that* looser threshold is satisfied, which
        // is frequently still too far away to actually trigger pickup --
        // the walk would report "done" while the drop stays on the ground.
        // Poll the bot's live position against a tight threshold instead of
        // trusting `MovementStatus::Completed`.
        const PICKUP_DISTANCE: f64 = 0.6;
        const COLLECT_TIMEOUT: Duration = Duration::from_secs(5);
        const ITEM_SCAN_RADIUS: f64 = 3.0;
        // Re-issue movement once the drop's live position has moved more
        // than this from the last goto -- avoids fighting Azalea's
        // in-flight path computation by resubmitting a goto every single
        // tick while the item is still settling.
        const DESTINATION_DRIFT_TOLERANCE: f64 = 0.5;
        let started = Instant::now();
        let mut dispatched = false;
        let mut last_goto_destination: Option<crate::minecraft::world_state::PositionSnapshot> =
            None;
        loop {
            let world = self.minecraft.world_state_snapshot().await;
            let destination =
                nearest_dropped_item_position(&world, block_center, resource_id, ITEM_SCAN_RADIUS)
                    .unwrap_or(block_center);
            let close_enough = world
                .bot
                .position
                .is_some_and(|position| collect_distance(position, destination) <= PICKUP_DISTANCE);
            if close_enough || started.elapsed() >= COLLECT_TIMEOUT {
                let _ = self.movement.stop(&self.minecraft).await;
                return None;
            }
            let status = self.movement.snapshot().await.status;
            let drifted = last_goto_destination.is_none_or(|previous| {
                collect_distance(previous, destination) > DESTINATION_DRIFT_TOLERANCE
            });
            if !dispatched
                || drifted
                || matches!(
                    status,
                    MovementStatus::Completed | MovementStatus::Idle | MovementStatus::Failed
                )
            {
                if self
                    .movement
                    .goto(&self.minecraft, destination, NavigationMode::AllowMining)
                    .await
                    .is_err()
                {
                    return None;
                }
                dispatched = true;
                last_goto_destination = Some(destination);
            }
            self.movement.tick(&self.minecraft, false).await;
            self.survival
                .tick(
                    &self.minecraft,
                    &self.movement,
                    &self.look,
                    &self.interaction,
                    &self.combat,
                )
                .await;
            let poll = survival_poll_interval(self, Duration::from_millis(75)).await;
            if let Some(input) = wait_tick(self, input_rx, poll).await {
                return Some(input);
            }
        }
    }

    /// Mob-drop counterpart of `run_get_item`, following the exact same
    /// shape (fresh search every iteration, bounded consecutive-failure
    /// abort, same terminal messages) but over `CombatController::kill_nearest`
    /// instead of block navigation + breaking. `resource_id` is what's
    /// counted in inventory (the drop item, e.g. `minecraft:leather`);
    /// `mob_id` is what's searched for and attacked (the entity type, e.g.
    /// `minecraft:cow`).
    async fn run_get_mob(
        &self,
        resource_id: &str,
        mob_id: &str,
        amount: u32,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        let radius = self.config.block_navigation.maximum_search_radius;
        let label = mobs::mob_label(mob_id);
        let mut consecutive_failures: u32 = 0;
        loop {
            let current = self
                .minecraft
                .world_state_snapshot()
                .await
                .inventory
                .count_item(resource_id);
            logging::info(format!("Inventory: {current}/{amount}"));
            if get_resource_satisfied(current, amount) {
                logging::success(format!("Successfully got {amount} {resource_id}"));
                return WaitOutcome::Finished(Ok(()));
            }

            logging::info(format!("Searching for nearest {label}..."));
            if let Err(error) = self
                .combat
                .kill_nearest(&self.minecraft, &self.movement, mob_id.to_owned(), radius)
                .await
            {
                return self.fail_get_mob(mob_id, error).await;
            }
            match await_combat_terminal(self, input_rx).await {
                WaitOutcome::Finished(Ok(())) => {
                    consecutive_failures = 0;
                    let new_count = self
                        .minecraft
                        .world_state_snapshot()
                        .await
                        .inventory
                        .count_item(resource_id);
                    logging::progress(format!("Collected {resource_id} ({new_count}/{amount})"));
                }
                WaitOutcome::Finished(Err(error)) => {
                    consecutive_failures += 1;
                    logging::warning(format!("Could not kill {label}: {error}"));
                    if get_resource_should_abort(consecutive_failures) {
                        return self.fail_get_mob(mob_id, error).await;
                    }
                }
                WaitOutcome::Interrupted(next) => {
                    self.combat
                        .cancel(&self.minecraft, &self.movement, &self.look)
                        .await;
                    return WaitOutcome::Interrupted(next);
                }
            }
        }
    }

    /// Common failure exit for `run_get_mob`: cancels combat, reports the
    /// mob-not-found message the `#get` contract requires, and hands the
    /// underlying error back to the caller.
    async fn fail_get_mob(&self, mob_id: &str, error: AppError) -> WaitOutcome {
        self.combat
            .cancel(&self.minecraft, &self.movement, &self.look)
            .await;
        logging::error(format!("Mob not found: {}", mobs::mob_label(mob_id)));
        logging::milestone("Get task cancelled");
        WaitOutcome::Finished(Err(error))
    }

    async fn look_and_wait(
        &self,
        name: &str,
        target: LookTarget,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        self.minecraft.set_current_task(task_snapshot(name)).await;
        let result = async {
            if let Err(error) = self.look.look_at(&self.minecraft, target).await {
                return WaitOutcome::Finished(Err(error));
            }
            await_look_terminal(self, input_rx).await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn look_block_and_wait(
        &self,
        block_id: String,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        self.minecraft
            .set_current_task(task_snapshot(format!("Look at {block_id}")))
            .await;
        let result = async {
            if let Err(error) = self.look.look_at_block_id(&self.minecraft, block_id).await {
                return WaitOutcome::Finished(Err(error));
            }
            await_look_terminal(self, input_rx).await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn break_looked_and_wait(&self, input_rx: &mut InputReceiver) -> WaitOutcome {
        self.minecraft
            .set_current_task(task_snapshot("Break looked block"))
            .await;
        let result = async {
            if let Err(error) = self
                .interaction
                .break_looked(&self.minecraft, &self.movement, &self.look)
                .await
            {
                return WaitOutcome::Finished(Err(error));
            }
            await_interaction_terminal(self, input_rx).await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn break_at_and_wait(
        &self,
        target: crate::minecraft::world_state::BlockPosition,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        self.minecraft
            .set_current_task(task_snapshot("Break block"))
            .await;
        let result = async {
            if let Err(error) = self
                .interaction
                .break_at(&self.minecraft, &self.movement, &self.look, target)
                .await
            {
                return WaitOutcome::Finished(Err(error));
            }
            await_interaction_terminal(self, input_rx).await
        }
        .await;
        // The bot just changed the world at `target`: forget what the
        // navigation cache believed about that chunk, and discard any
        // planned segment whose route passes through it. Almost always a
        // no-op (nothing is usually navigating while a block is being
        // broken) and cheap when it is -- see
        // `PathfindingController::notify_world_change`.
        self.pathfinding.notify_world_change(target).await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn break_nearest_and_wait(
        &self,
        block_id: String,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        self.minecraft
            .set_current_task(task_snapshot(format!("Break nearest {block_id}")))
            .await;
        let result = async {
            if let Err(error) = self
                .interaction
                .break_nearest(&self.minecraft, &self.movement, &self.look, block_id)
                .await
            {
                return WaitOutcome::Finished(Err(error));
            }
            await_interaction_terminal(self, input_rx).await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn interact_nearest_and_wait(
        &self,
        block_id: String,
        items: Vec<String>,
        radius: u32,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        self.minecraft
            .set_current_task(task_snapshot(format!("Interact with nearest {block_id}")))
            .await;
        let result = async {
            if let Err(error) = self
                .interaction
                .interact_nearest(
                    &self.minecraft,
                    &self.movement,
                    &self.look,
                    block_id,
                    items,
                    radius,
                )
                .await
            {
                return WaitOutcome::Finished(Err(error));
            }
            await_interaction_terminal(self, input_rx).await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn place_looked_and_wait(
        &self,
        item: String,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        self.minecraft
            .set_current_task(task_snapshot(format!("Place {item}")))
            .await;
        let result = async {
            if let Err(error) = self
                .interaction
                .place_looked(&self.minecraft, &self.movement, &self.look, item)
                .await
            {
                return WaitOutcome::Finished(Err(error));
            }
            await_interaction_terminal(self, input_rx).await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn place_at_and_wait(
        &self,
        target: crate::minecraft::world_state::BlockPosition,
        item: String,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        self.minecraft
            .set_current_task(task_snapshot(format!("Place {item}")))
            .await;
        let result = async {
            if let Err(error) = self
                .interaction
                .place_at(&self.minecraft, &self.movement, &self.look, target, item)
                .await
            {
                return WaitOutcome::Finished(Err(error));
            }
            await_interaction_terminal(self, input_rx).await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    /// Handles a console input that arrived while a blocking `*_and_wait` was
    /// already polling for another operation's completion. Read-only queries
    /// (and chat) are safe to answer without disturbing whatever is running,
    /// so they are handled here and the wait continues; returning `false`
    /// tells the caller this input instead conflicts with (or replaces) the
    /// in-flight operation and must interrupt the wait.
    async fn handle_inert_input(&self, input: &ConsoleInput) -> bool {
        match input {
            ConsoleInput::ChatMessage(message) => {
                if let Some(message) = plain_chat_message(
                    &ConsoleInput::ChatMessage(message.clone()),
                    self.config.console.send_plain_input_to_chat,
                ) {
                    if let Err(error) = self.minecraft.send_chat(message).await {
                        println!("Chat error: {error}");
                    }
                } else {
                    println!("Plain console input forwarding is disabled.");
                }
                true
            }
            ConsoleInput::Command(command) => match command {
                ConsoleCommand::Help => {
                    print_help();
                    true
                }
                ConsoleCommand::Status => {
                    self.print_status().await;
                    true
                }
                ConsoleCommand::Where => {
                    self.print_where().await;
                    true
                }
                ConsoleCommand::Health => {
                    self.print_health().await;
                    true
                }
                ConsoleCommand::Chat { message } => {
                    if let Err(error) = self.minecraft.send_chat(message).await {
                        println!("Chat error: {error}");
                    }
                    true
                }
                ConsoleCommand::Players => {
                    self.print_players().await;
                    true
                }
                ConsoleCommand::Inventory => {
                    self.print_inventory().await;
                    true
                }
                ConsoleCommand::ObservedContainerStatus => {
                    self.print_container_status().await;
                    true
                }
                ConsoleCommand::Entities { radius } => {
                    self.print_entities(*radius).await;
                    true
                }
                ConsoleCommand::PathStatus => {
                    self.print_path_status().await;
                    true
                }
                ConsoleCommand::OutputMode { change } => {
                    match change {
                        None => print_output_mode_status(),
                        Some(change) => apply_output_mode_change(*change),
                    }
                    true
                }
                ConsoleCommand::Explanation => {
                    print_output_mode_explanation();
                    true
                }
                ConsoleCommand::Movement => {
                    self.print_movement().await;
                    true
                }
                ConsoleCommand::GotoBlockStatus => {
                    self.print_block_navigation_status().await;
                    true
                }
                ConsoleCommand::LookStatus => {
                    self.print_look_status().await;
                    true
                }
                ConsoleCommand::InteractionStatus => {
                    self.print_interaction_status().await;
                    true
                }
                ConsoleCommand::ContainerStatus => {
                    let s = self.container.status().await;
                    println!(
                        "Container: {:?}; target={:?}; menu={:?}; transferred={}/{}; outcome={:?}{}",
                        s.phase,
                        s.target,
                        s.window_id,
                        s.transferred,
                        s.requested,
                        s.outcome,
                        s.detail.map(|d| format!(" ({d})")).unwrap_or_default()
                    );
                    true
                }
                _ => false,
            },
            ConsoleInput::Empty => true,
        }
    }
}

/// After this many consecutive break/navigation failures within one `/get`
/// run, stop instead of retrying forever -- bounds the "target block
/// disappeared" / "target became unreachable" recovery path so a block that
/// keeps failing (a persistently obstructed face, a tool that keeps breaking,
/// a target that keeps disappearing right as it's reached) cannot spin the
/// task indefinitely, per the "never enter an infinite loop" requirement.
const GET_RESOURCE_MAX_CONSECUTIVE_FAILURES: u32 = 5;

fn get_resource_satisfied(current: u32, amount: u32) -> bool {
    current >= amount
}

fn get_resource_should_abort(consecutive_failures: u32) -> bool {
    consecutive_failures >= GET_RESOURCE_MAX_CONSECUTIVE_FAILURES
}

fn collect_distance(
    a: crate::minecraft::world_state::PositionSnapshot,
    b: crate::minecraft::world_state::PositionSnapshot,
) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

/// Nearest dropped-item entity to `near` (the block just broken) within
/// `radius`, preferring an exact match on `resource_id` -- what this `#get`
/// run actually wants -- but falling back to any dropped item in range so a
/// block whose observed drop id doesn't exactly match what
/// `mobs::resolve_resource` predicted (fortune, an unmodeled secondary
/// drop) still gets walked to rather than ignored. `None` means nothing has
/// been observed near the block yet -- the entity's spawn packet may
/// simply not have arrived this tick -- and the caller falls back to the
/// block's own position.
fn nearest_dropped_item_position(
    world: &crate::minecraft::world_state::WorldStateSnapshot,
    near: crate::minecraft::world_state::PositionSnapshot,
    resource_id: &str,
    radius: f64,
) -> Option<crate::minecraft::world_state::PositionSnapshot> {
    let nearest = |only_matching_id: bool| {
        world
            .dropped_items
            .iter()
            .filter(|item| !only_matching_id || item.stack.item_id == resource_id)
            .map(|item| item.position)
            .filter(|position| collect_distance(*position, near) <= radius)
            .min_by(|a, b| collect_distance(*a, near).total_cmp(&collect_distance(*b, near)))
    };
    nearest(true).or_else(|| nearest(false))
}

/// Bare (non-namespaced), `separator`-joined console label for a set of
/// block/item ids, e.g. `["minecraft:diamond_ore", "minecraft:deepslate_diamond_ore"]`
/// with `"/"` -> `"diamond_ore/deepslate_diamond_ore"`.
fn join_labels(ids: &[String], separator: &str) -> String {
    ids.iter()
        .map(|id| blocks::bare_id(id))
        .collect::<Vec<_>>()
        .join(separator)
}

/// Maps a `#drop`-planning shortfall to the exact wording the contract
/// requires: nothing held at all is `Item not found: {label}`, distinct
/// from holding *some* but not enough (`Not enough {label} in inventory
/// (have X, need Y)`).
fn drop_insufficient_error(label: &str, amount: u32, available: u32) -> AppError {
    if available == 0 {
        AppError::ItemNotFoundForDrop(label.to_owned())
    } else {
        AppError::InsufficientItemsForDrop {
            item: label.to_owned(),
            have: available,
            need: amount,
        }
    }
}

fn task_snapshot(name: impl Into<String>) -> TaskSnapshot {
    TaskSnapshot {
        name: name.into(),
        started_at: SystemTime::now(),
    }
}

fn fmt_opt<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "unknown".into(), |v| v.to_string())
}

/// Whether `command` is read-only (a status/info query) rather than
/// something that starts, redirects, or stops an actual action -- see the
/// `#kill`-cancellation comment at this function's one call site for why
/// the distinction matters. Deliberately a positive list, not a negative
/// one: a future command added to `ConsoleCommand` and forgotten here
/// defaults to "cancel the fight first", which is always safe (worst case,
/// a fight gets cancelled it didn't strictly need to be), whereas defaulting
/// new commands to "don't cancel" could silently reintroduce this exact bug.
fn is_read_only_query(command: &ConsoleCommand) -> bool {
    matches!(
        command,
        ConsoleCommand::Help
            | ConsoleCommand::Status
            | ConsoleCommand::Where
            | ConsoleCommand::Health
            | ConsoleCommand::Players
            | ConsoleCommand::Inventory
            | ConsoleCommand::ObservedContainerStatus
            | ConsoleCommand::Entities { .. }
            | ConsoleCommand::PathStatus
            | ConsoleCommand::Movement
            | ConsoleCommand::GotoBlockStatus
            | ConsoleCommand::LookStatus
            | ConsoleCommand::InteractionStatus
            | ConsoleCommand::ContainerStatus
            | ConsoleCommand::OutputMode { .. }
            | ConsoleCommand::Explanation
            | ConsoleCommand::Chat { .. }
    )
}

fn print_help() {
    println!("Movement");
    println!("  /goto <x> <y> <z>          Walk to a position and wait until it is reached");
    println!("  /follow <player>           Follow a player");
    println!(
        "  /stop                      Emergency stop: force-cancel everything (alias: stopall, #stop in chat)"
    );
    println!("  /stopmovement              Stop movement/pathfinding only");
    println!("  /look <x> <y> <z>          Look at a position and wait until aimed");
    println!();
    println!("Status");
    println!("  /where                     Show the bot's current position");
    println!("  /health                    Show health and food level");
    println!("  /inventory                 Show inventory contents");
    println!("  /movement                  Show movement status");
    println!();
    println!("World");
    println!("  /break <x> <y> <z>         Break a block and wait until it is broken");
    println!("  /place <block> <x> <y> <z> Place a block and wait until it is placed");
    println!("  /find <block> [radius]     Find the nearest matching block");
    println!(
        "  /get <item> <amount>       Gather <amount> of an item -- resolves ore/conversion sources or a mob drop automatically, mines/hunts whichever is nearest"
    );
    println!(
        "  /mine <block> [block...] <amount>  Mine exactly the given block(s) (whichever is nearer), counting blocks destroyed, not items received"
    );
    println!(
        "  /interact <block> <item[,item...]> [radius]  Right-click the nearest matching block with a held item (e.g. till dirt with a hoe)"
    );
    println!();
    println!("Combat");
    println!(
        "  /kill <player>             Fight a player until they die, disconnect, or the task is cancelled (standalone PvP, alias: #kill in chat)"
    );
    println!();
    println!("Inventory");
    println!("  /equip <item>              Equip an item to the active hotbar slot");
    println!();
    println!("Other");
    println!("  /help                      Show this message");
    println!("  /status                    Show full connection/application status");
    println!("  /chat <text>               Send text to Minecraft chat");
    println!("  /reconnect                 Reconnect to the configured server");
    println!("  /quit                      Shut down the application");
    println!(
        "  /outputmode [console|chat|both] [none|light|info|debug|fulldebug]  Show or change how much status narration is printed/sent to chat"
    );
    println!("  /explanation               Explain what each output mode shows (alias: /explain)");
    println!(
        "  A player can also run any command from Minecraft chat by prefixing it with #, e.g. \"#goto 100 64 20\"."
    );
    println!(
        "  Additional lower-level debug commands (findblock, gotoblock, lookblock, breaknearest, placeblock, ...) remain available; see src/console/commands.rs for the complete set."
    );
}

fn print_output_mode_status() {
    println!(
        "Output mode -- console: {}, chat: {}",
        logging::console_mode().as_str(),
        logging::chat_mode().as_str()
    );
}

fn apply_output_mode_change(change: OutputModeChange) {
    match change.target {
        OutputModeTarget::Console => logging::set_console_mode(change.mode),
        OutputModeTarget::Chat => logging::set_chat_mode(change.mode),
        OutputModeTarget::Both => logging::configure(change.mode, change.mode),
    }
    print_output_mode_status();
}

/// `/explanation` (`/explain`, `#explanation` in chat) -- see
/// `crate::config::OutputMode`'s doc comment for the source of truth this
/// mirrors for a user who doesn't want to go read the config file.
fn print_output_mode_explanation() {
    println!(
        "Output modes control how much of the bot's own status narration (task started/progress/finished -- not raw player chat) is shown, independently for the console (this window) and Minecraft chat."
    );
    println!();
    println!("  none       nothing at all");
    println!("  light      only a task's start and its final outcome");
    println!("             e.g. \"Get task started: diamond x5\", \"Collected 5 diamond\"");
    println!("  info       light, plus a running progress report as a task makes headway");
    println!(
        "             e.g. \"Collected diamond (3/5)\" -- the default for both console and chat"
    );
    println!(
        "  debug      info, plus every other diagnostic line the bot prints (connection state, per-step narration, retries, ...)"
    );
    println!(
        "  fulldebug  debug, but also disables repeat-collapsing so a stuck retry loop prints every repetition -- for troubleshooting only, not everyday use"
    );
    println!();
    println!("Change with: /outputmode <console|chat|both> <mode>");
    println!("Show current settings with: /outputmode");
}

async fn await_console_task(task: JoinHandle<()>) {
    let _ = task.await;
}

/// Waits for SIGTERM on Unix (what `docker stop` / Pterodactyl-Pelican eggs
/// send by default to stop a container) so the same graceful-shutdown path
/// used for Ctrl+C also runs there. On non-Unix platforms this simply never
/// resolves, leaving Ctrl+C as the only shutdown signal.
#[cfg(unix)]
async fn wait_for_terminate_signal() -> std::io::Result<()> {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    sigterm.recv().await;
    Ok(())
}
#[cfg(not(unix))]
async fn wait_for_terminate_signal() -> std::io::Result<()> {
    std::future::pending().await
}

#[cfg(test)]
mod get_resource_tests {
    use super::*;

    #[test]
    fn already_satisfied_inventory_finishes_immediately() {
        assert!(get_resource_satisfied(15, 15));
        assert!(get_resource_satisfied(20, 15));
        assert!(!get_resource_satisfied(0, 15));
        assert!(!get_resource_satisfied(14, 15));
    }

    #[test]
    fn aborts_only_once_consecutive_failures_reach_the_cap() {
        for count in 0..GET_RESOURCE_MAX_CONSECUTIVE_FAILURES {
            assert!(
                !get_resource_should_abort(count),
                "should not abort at {count} consecutive failures"
            );
        }
        assert!(get_resource_should_abort(
            GET_RESOURCE_MAX_CONSECUTIVE_FAILURES
        ));
        assert!(get_resource_should_abort(
            GET_RESOURCE_MAX_CONSECUTIVE_FAILURES + 1
        ));
    }

    #[test]
    fn join_labels_strips_namespaces_and_joins_with_the_given_separator() {
        assert_eq!(
            join_labels(&["minecraft:diamond_ore".into()], "/"),
            "diamond_ore"
        );
        assert_eq!(
            join_labels(
                &[
                    "minecraft:diamond_ore".into(),
                    "minecraft:deepslate_diamond_ore".into(),
                ],
                "/"
            ),
            "diamond_ore/deepslate_diamond_ore"
        );
        assert_eq!(
            join_labels(
                &[
                    "minecraft:diamond_ore".into(),
                    "minecraft:deepslate_diamond_ore".into(),
                ],
                ", "
            ),
            "diamond_ore, deepslate_diamond_ore"
        );
    }

    fn dropped_item(
        item_id: &str,
        position: crate::minecraft::world_state::PositionSnapshot,
    ) -> crate::minecraft::dropped_items::DroppedItemObservation {
        crate::minecraft::dropped_items::DroppedItemObservation {
            session_id: 0,
            entity_id: 0,
            uuid: None,
            stack: crate::minecraft::dropped_items::ObservableItemStack {
                item_id: item_id.into(),
                count: 1,
                components: serde_json::json!({}),
            },
            position,
            distance: 0.0,
            dimension: "minecraft:overworld".into(),
            last_seen: SystemTime::now(),
        }
    }

    fn position(x: f64, y: f64, z: f64) -> crate::minecraft::world_state::PositionSnapshot {
        crate::minecraft::world_state::PositionSnapshot { x, y, z }
    }

    #[test]
    fn walks_to_the_matching_drop_even_when_it_rolled_off_the_mined_block() {
        let block_center = position(0.5, 64.0, 0.5);
        let world = crate::minecraft::world_state::WorldStateSnapshot {
            dropped_items: vec![dropped_item("minecraft:diamond", position(2.0, 64.0, 0.5))],
            ..Default::default()
        };
        assert_eq!(
            nearest_dropped_item_position(&world, block_center, "minecraft:diamond", 3.0),
            Some(position(2.0, 64.0, 0.5))
        );
    }

    #[test]
    fn prefers_the_matching_item_id_over_a_closer_unrelated_drop() {
        let block_center = position(0.5, 64.0, 0.5);
        let world = crate::minecraft::world_state::WorldStateSnapshot {
            dropped_items: vec![
                dropped_item("minecraft:cobblestone", position(0.6, 64.0, 0.5)),
                dropped_item("minecraft:diamond", position(1.5, 64.0, 0.5)),
            ],
            ..Default::default()
        };
        assert_eq!(
            nearest_dropped_item_position(&world, block_center, "minecraft:diamond", 3.0),
            Some(position(1.5, 64.0, 0.5))
        );
    }

    #[test]
    fn falls_back_to_any_drop_when_no_id_matches() {
        let block_center = position(0.5, 64.0, 0.5);
        let world = crate::minecraft::world_state::WorldStateSnapshot {
            dropped_items: vec![dropped_item("minecraft:flint", position(1.0, 64.0, 0.5))],
            ..Default::default()
        };
        assert_eq!(
            nearest_dropped_item_position(&world, block_center, "minecraft:gravel", 3.0),
            Some(position(1.0, 64.0, 0.5))
        );
    }

    #[test]
    fn ignores_drops_outside_the_scan_radius() {
        let block_center = position(0.5, 64.0, 0.5);
        let world = crate::minecraft::world_state::WorldStateSnapshot {
            dropped_items: vec![dropped_item("minecraft:diamond", position(50.0, 64.0, 0.5))],
            ..Default::default()
        };
        assert_eq!(
            nearest_dropped_item_position(&world, block_center, "minecraft:diamond", 3.0),
            None
        );
    }

    #[test]
    fn no_observations_yet_returns_none() {
        let world = crate::minecraft::world_state::WorldStateSnapshot::default();
        assert_eq!(
            nearest_dropped_item_position(
                &world,
                position(0.5, 64.0, 0.5),
                "minecraft:diamond",
                3.0
            ),
            None
        );
    }
}

#[cfg(test)]
mod drop_tests {
    use super::*;

    #[test]
    fn zero_available_reports_item_not_found() {
        assert!(matches!(
            drop_insufficient_error("diamond", 5, 0),
            AppError::ItemNotFoundForDrop(item) if item == "diamond"
        ));
    }

    #[test]
    fn partial_availability_reports_have_and_need() {
        assert!(matches!(
            drop_insufficient_error("diamond", 5, 3),
            AppError::InsufficientItemsForDrop { item, have: 3, need: 5 } if item == "diamond"
        ));
    }
}
