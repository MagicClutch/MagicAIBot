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
    config::Config,
    console::{
        self,
        commands::{ConsoleCommand, ConsoleInput, plain_chat_message},
    },
    container::{model::TransferDirection, service::ContainerService},
    error::AppError,
    interaction::{InteractionController, interaction_controller::InteractionState},
    logging,
    look::{LookController, LookTarget, look_controller::LookState},
    minecraft::{
        client::MinecraftClient,
        world_state::{MovementStatus, TaskSnapshot},
    },
    mobs::{self, CombatController, CombatState},
    movement::{MovementService, NavigationMode},
    navigation::BlockNavigationService,
    navigation::navigation_state::BlockNavigationState,
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

/// Outcome of a blocking wait: either it ran to completion, or new console
/// input arrived that must be handled by the caller instead.
enum WaitOutcome {
    Finished(Result<(), AppError>),
    Interrupted(ConsoleInput),
}

/// Bounded by `MovementConfig::maximum_navigation_seconds`. Azalea's
/// pathfinder is submitted with `retry_on_no_path(true)` (see
/// `MinecraftClient::start_navigation_to`), so a genuinely unreachable goal
/// (not enough scaffold material to finish a route, a destination behind
/// terrain the current policy can't cross) retries forever inside Azalea
/// without ever surfacing as a failure -- `MovementStatus` would just stay
/// `MovingToPosition` indefinitely. Without a deadline here, that would hang
/// this function forever.
async fn await_movement_terminal(app: &App, input_rx: &mut InputReceiver) -> WaitOutcome {
    let deadline = Duration::from_secs(app.movement.maximum_navigation_seconds());
    let started = Instant::now();
    loop {
        app.movement.tick(&app.minecraft, false).await;
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
                if started.elapsed() >= deadline {
                    let _ = app.movement.stop(&app.minecraft).await;
                    return WaitOutcome::Finished(Err(AppError::PathfindingFailure(format!(
                        "movement timed out after {}s without reaching the destination or failing",
                        deadline.as_secs()
                    ))));
                }
                if let Some(input) = wait_tick(app, input_rx, Duration::from_millis(200)).await {
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
                if let Some(input) = wait_tick(app, input_rx, Duration::from_millis(200)).await {
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
                if let Some(input) = wait_tick(app, input_rx, Duration::from_millis(75)).await {
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
                if let Some(input) = wait_tick(app, input_rx, Duration::from_millis(75)).await {
                    return WaitOutcome::Interrupted(input);
                }
            }
        }
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
    tokio::select! {
        () = tokio::time::sleep(duration) => None,
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
    container: ContainerService,
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
            combat: CombatController::new(),
            container: ContainerService::default(),
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
                            self.block_navigation.cancel(&self.minecraft, &self.movement).await;
                            self.look.cancel().await;
                            let _ = self.movement.stop(&self.minecraft).await;
                            self.minecraft.clear_current_task().await;
                        }
                        continue;
                    }
                    self.session_ready = true;
                    self.tick_chat_commands(&mut input_rx).await;
                    let explicit_look = self.look.snapshot().await.state == LookState::Looking;
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
                    // Block navigation owns interaction approach/repath state.
                    // Tick it on the fast interaction cadence so an interaction
                    // target does not wait for the slower movement repath timer
                    // before the next path is selected.
                    self.block_navigation.tick(&self.minecraft, &self.movement).await;
                    self.interaction.tick(&self.minecraft, &self.movement, &self.look).await;
                    self.combat.tick(&self.minecraft, &self.movement, &self.look).await;
                    self.container.tick(&self.minecraft, &self.movement, &self.block_navigation, &self.look).await;
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
        self.interaction
            .cancel(&self.minecraft, &self.movement, &self.look)
            .await;
        self.combat
            .cancel(&self.minecraft, &self.movement, &self.look)
            .await;
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
                ConsoleInput::Command(command) => match command {
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
                    ConsoleCommand::ObservedContainerStatus => self.print_container_status().await,
                    ConsoleCommand::Entities { radius } => self.print_entities(radius).await,
                    ConsoleCommand::Goto { x, y, z } => {
                        self.interaction
                            .cancel(&self.minecraft, &self.movement, &self.look)
                            .await;
                        self.block_navigation
                            .cancel(&self.minecraft, &self.movement)
                            .await;
                        logging::info(format!("Navigating to ({x}, {y}, {z})"));
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
                        // `/stop` is the movement channel's stop command. A
                        // separate look task remains active, even when it is
                        // tracking a target while the bot walks.
                        let description = self.active_stop_description().await;
                        self.block_navigation
                            .cancel(&self.minecraft, &self.movement)
                            .await;
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
                            Err(AppError::UnknownPlayer(_)) => logging::error("Player not found"),
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
                    ConsoleCommand::GotoBlockStatus => self.print_block_navigation_status().await,
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
                                // message format inside `run_get_block` /
                                // `run_get_mob`.
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
                            WaitOutcome::Finished(Ok(())) => logging::success("Looking at target"),
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
                            .look_and_wait("Look at player", LookTarget::Player(player), input_rx)
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
                                logging::warning(format!("Cannot interact with block: {error}"));
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
                    ConsoleCommand::CloseContainer => self.container.close(&self.minecraft).await,
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
                },
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

        None
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

    async fn goto_and_wait(
        &self,
        name: &str,
        destination: crate::minecraft::world_state::PositionSnapshot,
        mode: NavigationMode,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        self.minecraft.set_current_task(task_snapshot(name)).await;
        let result = async {
            if let Err(error) = self.movement.goto(&self.minecraft, destination, mode).await {
                return WaitOutcome::Finished(Err(error));
            }
            await_movement_terminal(self, input_rx).await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
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

    /// Universal Baritone-style `/get <resource> <amount>` (also reachable as
    /// `#get <resource> <amount>` from Minecraft chat). `resource` can name
    /// either a block or a mob drop; `resolve_and_run_get_resource` is the
    /// single place that decides which, via `mobs::resolve_resource`, and
    /// dispatches to `run_get_block` or `run_get_mob` accordingly -- the
    /// caller here stays resource-kind-agnostic.
    async fn get_resource_and_wait(
        &self,
        resource_id: String,
        amount: u32,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        self.minecraft
            .set_current_task(task_snapshot(format!("Get {amount} {resource_id}")))
            .await;
        logging::info(format!("Get task started: {resource_id} x{amount}"));
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
            Ok(mobs::ResourceKind::Block(block_id)) => {
                self.run_get_block(&block_id, amount, input_rx).await
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
                logging::info("Get task cancelled");
                WaitOutcome::Finished(Err(error))
            }
        }
    }

    /// Deliberately not a separate gathering system: each iteration reuses
    /// `BlockNavigationService::start` (nearest-*reachable* block, already
    /// falling back across candidates and never retrying an approach it just
    /// proved impossible -- see that type's `try_next_target`) to walk to a
    /// fresh scan of loaded chunks, then `InteractionController::break_at`
    /// (tool selection, precise look, break, verified removal) on the exact
    /// block navigation just reached. No block list is hardcoded anywhere in
    /// this path -- any block accepted by `normalize_block_id` (i.e. any
    /// block in Azalea's registry) works here.
    ///
    /// Mining and inventory-counting deliberately target different ids:
    /// `block_id` (what gets scanned for, navigated to, and broken) is
    /// always the block the caller asked for, but many blocks drop an item
    /// with a different registry id (`minecraft:iron_ore` -> `raw_iron`,
    /// `minecraft:stone` -> `cobblestone`, `minecraft:deepslate_diamond_ore`
    /// -> `diamond`, ...). `drop_item` (resolved once up front via
    /// `blocks::drop_item_for_block` -- see that module for why this can't
    /// come from real loot-table data) is what inventory is actually counted
    /// against; using `block_id` there would wait on a count that can never
    /// increase for any block with a differing drop.
    async fn run_get_block(
        &self,
        block_id: &str,
        amount: u32,
        input_rx: &mut InputReceiver,
    ) -> WaitOutcome {
        let radius = self.config.block_navigation.maximum_search_radius;
        let drop_item = blocks::drop_item_for_block(block_id).to_owned();
        let block_label = blocks::bare_id(block_id).to_owned();
        let drop_label = blocks::bare_id(&drop_item).to_owned();
        let mut consecutive_failures: u32 = 0;
        loop {
            let current = self
                .minecraft
                .world_state_snapshot()
                .await
                .inventory
                .count_item(&drop_item);
            logging::info(format!("Inventory: {current}/{amount}"));
            if get_resource_satisfied(current, amount) {
                if drop_item == block_id {
                    logging::success(format!("Successfully got {amount} {block_label}"));
                } else {
                    logging::success(format!(
                        "Collected {amount} {drop_label} from {block_label}"
                    ));
                }
                return WaitOutcome::Finished(Ok(()));
            }

            logging::info(format!("Scanning loaded chunks for {block_label}..."));
            if let Err(error) = self
                .block_navigation
                .start(
                    &self.minecraft,
                    &self.movement,
                    block_id.to_owned(),
                    radius,
                    NavigationMode::AllowMining,
                )
                .await
            {
                return self.fail_get_block(&block_label, error).await;
            }
            match await_block_navigation_terminal(self, input_rx).await {
                WaitOutcome::Finished(Ok(())) => {}
                WaitOutcome::Finished(Err(error)) => {
                    return self.fail_get_block(&block_label, error).await;
                }
                WaitOutcome::Interrupted(next) => return WaitOutcome::Interrupted(next),
            }

            let target = self
                .block_navigation
                .snapshot()
                .await
                .selected_block_position;
            let Some(target) = target else {
                return self
                    .fail_get_block(&block_label, AppError::NoMatchingBlock)
                    .await;
            };

            logging::info(format!("Looking at {block_label}"));
            logging::info(format!("Breaking {block_label}"));
            if let Err(error) = self
                .interaction
                .break_at(&self.minecraft, &self.movement, &self.look, target)
                .await
            {
                consecutive_failures += 1;
                logging::warning(format!("Could not break {block_label}: {error}"));
                if get_resource_should_abort(consecutive_failures) {
                    return self.fail_get_block(&block_label, error).await;
                }
                continue;
            }
            match await_interaction_terminal(self, input_rx).await {
                WaitOutcome::Finished(Ok(())) => {
                    consecutive_failures = 0;
                    if let Some(next) = self.collect_drop_at(target, input_rx).await {
                        return WaitOutcome::Interrupted(next);
                    }
                    let new_count = self
                        .minecraft
                        .world_state_snapshot()
                        .await
                        .inventory
                        .count_item(&drop_item);
                    logging::success(format!("Collected {drop_label} ({new_count}/{amount})"));
                }
                WaitOutcome::Finished(Err(error)) => {
                    consecutive_failures += 1;
                    logging::warning(format!("Could not break {block_label}: {error}"));
                    if get_resource_should_abort(consecutive_failures) {
                        return self.fail_get_block(&block_label, error).await;
                    }
                }
                WaitOutcome::Interrupted(next) => return WaitOutcome::Interrupted(next),
            }
        }
    }

    /// Walks onto the just-broken block's position so vanilla's proximity
    /// item pickup actually triggers before the next search starts.
    /// Breaking happens from pickaxe reach (up to `interaction_distance`,
    /// ~4.5 blocks), which is well beyond the pickup radius -- without this,
    /// the drop is frequently left sitting on the ground uncollected, the
    /// inventory count never advances, and every subsequent iteration has to
    /// search further and further outward for still-unmined ore instead of
    /// registering the progress that already happened. Bounded by its own
    /// short timeout (not `MovementConfig::maximum_navigation_seconds`,
    /// which is far too generous for a one-or-two-block walk) so a pickup
    /// that can't complete for some reason -- another player grabs it first,
    /// the item despawns, the spot is unreachable -- can't stall the whole
    /// `#get` run; the caller just moves on and re-checks inventory as
    /// usual. Returns `Some` only if the wait was interrupted by new console
    /// input, mirroring the `WaitOutcome::Interrupted` contract.
    async fn collect_drop_at(
        &self,
        target: crate::minecraft::world_state::BlockPosition,
        input_rx: &mut InputReceiver,
    ) -> Option<ConsoleInput> {
        let destination = crate::minecraft::world_state::PositionSnapshot {
            x: f64::from(target.x) + 0.5,
            y: f64::from(target.y),
            z: f64::from(target.z) + 0.5,
        };
        if self
            .movement
            .goto(&self.minecraft, destination, NavigationMode::AllowMining)
            .await
            .is_err()
        {
            return None;
        }
        const COLLECT_TIMEOUT: Duration = Duration::from_secs(5);
        let started = Instant::now();
        loop {
            self.movement.tick(&self.minecraft, false).await;
            let status = self.movement.snapshot().await.status;
            if !matches!(status, MovementStatus::MovingToPosition)
                || started.elapsed() >= COLLECT_TIMEOUT
            {
                let _ = self.movement.stop(&self.minecraft).await;
                return None;
            }
            if let Some(input) = wait_tick(self, input_rx, Duration::from_millis(75)).await {
                return Some(input);
            }
        }
    }

    /// Common failure exit for `run_get_block`: stops movement/navigation,
    /// reports the block-not-found message the `#get` contract requires, and
    /// hands the underlying error back to the caller. `block_label` is the
    /// bare mined-block name (not the drop item) -- "not found" is about the
    /// block search, which always operates on the block id.
    async fn fail_get_block(&self, block_label: &str, error: AppError) -> WaitOutcome {
        self.block_navigation
            .cancel(&self.minecraft, &self.movement)
            .await;
        logging::error(format!("Block not found: {block_label}"));
        logging::info("Get task cancelled");
        WaitOutcome::Finished(Err(error))
    }

    /// Mob-drop counterpart of `run_get_block`, following the exact same
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
                    logging::success(format!("Collected {resource_id} ({new_count}/{amount})"));
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
        logging::info("Get task cancelled");
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

fn task_snapshot(name: impl Into<String>) -> TaskSnapshot {
    TaskSnapshot {
        name: name.into(),
        started_at: SystemTime::now(),
    }
}

fn fmt_opt<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "unknown".into(), |v| v.to_string())
}

fn print_help() {
    println!("Movement");
    println!("  /goto <x> <y> <z>          Walk to a position and wait until it is reached");
    println!("  /follow <player>           Follow a player");
    println!("  /stop                      Stop movement");
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
        "  /get <resource> <amount>   Gather <amount> of any loaded block or mob drop (nearest-reachable, repeats until satisfied)"
    );
    println!(
        "  /interact <block> <item[,item...]> [radius]  Right-click the nearest matching block with a held item (e.g. till dirt with a hoe)"
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
        "  A player can also run any command from Minecraft chat by prefixing it with #, e.g. \"#goto 100 64 20\"."
    );
    println!(
        "  Additional lower-level debug commands (findblock, gotoblock, lookblock, breaknearest, placeblock, ...) remain available; see src/console/commands.rs for the complete set."
    );
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
}
