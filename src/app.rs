use std::{
    path::Path,
    time::{Duration, Instant},
};

use crate::{
    blocks::{
        BlockSearchService,
        block_query::BlockSearchQuery,
        block_search::{format_find_results, format_nearest_result},
    },
    config::Config,
    console::{
        self,
        commands::{ConsoleCommand, ConsoleInput, plain_chat_message},
    },
    error::AppError,
    interaction::{InteractionController, interaction_controller::InteractionState},
    logging,
    look::{LookController, LookTarget, look_controller::LookState},
    minecraft::client::MinecraftClient,
    movement::{MovementService, NavigationMode},
    navigation::{BlockNavigationService, navigation_state::BlockNavigationState},
    tasks::TaskService,
};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

/// Application composition root. Future services will be owned by this type.
pub struct App {
    config: Config,
    shutdown: CancellationToken,
    minecraft: MinecraftClient,
    movement: MovementService,
    block_search: BlockSearchService,
    block_navigation: BlockNavigationService,
    look: LookController,
    interaction: InteractionController,
    tasks: TaskService,
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
            BlockSearchService::new(
                config.block_search.maximum_radius,
                config.block_search.maximum_result_limit,
                config.block_search.default_vertical_range,
            ),
        );

        Ok(Self {
            minecraft: MinecraftClient::new(
                config.minecraft.clone(),
                config.reconnect.clone(),
                config.console.clone(),
                config.world_state.clone(),
            ),
            movement: MovementService::new(config.movement.clone(), config.multitasking.clone()),
            block_search: BlockSearchService::new(
                config.block_search.maximum_radius,
                config.block_search.maximum_result_limit,
                config.block_search.default_vertical_range,
            ),
            block_navigation: block_navigation.clone(),
            look: LookController::new(
                config.look.clone(),
                BlockSearchService::new(
                    config.block_search.maximum_radius,
                    config.block_search.maximum_result_limit,
                    config.block_search.default_vertical_range,
                ),
            ),
            interaction: InteractionController::new(
                config.interaction.clone(),
                BlockSearchService::new(
                    config.block_search.maximum_radius,
                    config.block_search.maximum_result_limit,
                    config.block_search.default_vertical_range,
                ),
                block_navigation,
            ),
            tasks: TaskService::default(),
            session_ready: false,
            config,
            shutdown: CancellationToken::new(),
            started_at: Instant::now(),
        })
    }

    /// Waits for Ctrl+C and performs the application's graceful shutdown.
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
                _ = movement_tick.tick() => {
                    if self.minecraft.connection_state() != crate::minecraft::client::ConnectionState::Connected {
                        if self.session_ready {
                            self.session_ready = false;
                            self.interaction.cancel(&self.minecraft, &self.movement, &self.look).await;
                            self.block_navigation.cancel(&self.minecraft, &self.movement).await;
                            self.look.cancel().await;
                            let _ = self.movement.stop(&self.minecraft).await;
                            self.tasks.cancel(&self.minecraft).await;
                        }
                        continue;
                    }
                    self.session_ready = true;
                    let explicit_look = self.look.snapshot().await.state == LookState::Looking;
                    self.movement.tick(&self.minecraft, explicit_look).await;
                    self.block_navigation.tick(&self.minecraft, &self.movement).await;
                },
                _ = look_tick.tick() => {
                    let status = self.minecraft.navigation_status().await.ok();
                    if !status.is_some_and(|status| status.calculating || status.executing) {
                        self.look.tick(&self.minecraft).await;
                    }
                },
                _ = interaction_tick.tick() => self.interaction.tick(&self.minecraft, &self.movement, &self.look).await,
                input = input_rx.recv() => match input {
                    Some(Ok(ConsoleInput::Empty)) => {}
                    Some(Ok(input)) => {
                        if self.execute_console_input(input).await? {
                            break Ok(());
                        }
                    }
                    Some(Err(error)) => println!("Console error: {error}"),
                    None => break Ok(()),
                }
            }
        };

        self.shutdown.cancel();
        self.block_navigation
            .cancel(&self.minecraft, &self.movement)
            .await;
        self.look.cancel().await;
        self.interaction
            .cancel(&self.minecraft, &self.movement, &self.look)
            .await;
        let _ = self.movement.stop(&self.minecraft).await;
        self.minecraft.disconnect().await?;
        if let Some(task) = console_task {
            await_console_task(task).await;
        }
        loop_result
    }

    async fn execute_console_input(&mut self, input: ConsoleInput) -> Result<bool, AppError> {
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
                ConsoleCommand::Chat { message } => {
                    if let Err(error) = self.minecraft.send_chat(&message).await {
                        println!("Chat error: {error}");
                    }
                }
                ConsoleCommand::Players => self.print_players().await,
                ConsoleCommand::Inventory => self.print_inventory().await,
                ConsoleCommand::ContainerStatus => self.print_container_status().await,
                ConsoleCommand::Entities { radius } => self.print_entities(radius).await,
                ConsoleCommand::Goto { x, y, z } => {
                    self.interaction
                        .cancel(&self.minecraft, &self.movement, &self.look)
                        .await;
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    if let Err(error) = self
                        .tasks
                        .goto_position(
                            &self.minecraft,
                            &self.movement,
                            crate::minecraft::world_state::PositionSnapshot {
                                x: f64::from(x),
                                y: f64::from(y),
                                z: f64::from(z),
                            },
                            NavigationMode::MovementOnly,
                        )
                        .await
                    {
                        println!("Movement error: {error}");
                    }
                }
                ConsoleCommand::GotoMine { x, y, z } => {
                    self.interaction
                        .cancel(&self.minecraft, &self.movement, &self.look)
                        .await;
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    if let Err(error) = self
                        .tasks
                        .goto_position(
                            &self.minecraft,
                            &self.movement,
                            crate::minecraft::world_state::PositionSnapshot {
                                x: f64::from(x),
                                y: f64::from(y),
                                z: f64::from(z),
                            },
                            NavigationMode::AllowMining,
                        )
                        .await
                    {
                        println!("Movement error: {error}");
                    }
                }
                ConsoleCommand::PathStatus => self.print_path_status().await,
                ConsoleCommand::Stop => {
                    // `/stop` is the movement channel's stop command. A
                    // separate look task remains active, even when it is
                    // tracking a target while the bot walks.
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    if let Err(error) = self.movement.stop(&self.minecraft).await {
                        println!("Movement error: {error}");
                    }
                }
                ConsoleCommand::StopAll => {
                    self.interaction
                        .cancel(&self.minecraft, &self.movement, &self.look)
                        .await;
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    if let Err(error) = self.movement.stop(&self.minecraft).await {
                        println!("Movement error: {error}");
                    }
                    self.look.cancel().await;
                    self.tasks.cancel(&self.minecraft).await;
                }
                ConsoleCommand::TaskStatus => self.print_task_status().await,
                ConsoleCommand::Follow { player } => {
                    self.interaction
                        .cancel(&self.minecraft, &self.movement, &self.look)
                        .await;
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    if let Err(error) = self.movement.follow(&self.minecraft, &player).await {
                        println!("Movement error: {error}");
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
                    let radius =
                        search_radius.unwrap_or(self.config.block_navigation.default_search_radius);
                    if let Err(error) = self
                        .tasks
                        .goto_block(
                            &self.minecraft,
                            &self.movement,
                            &self.block_navigation,
                            block_id,
                            radius,
                            if allow_mining {
                                NavigationMode::AllowMining
                            } else {
                                NavigationMode::MovementOnly
                            },
                        )
                        .await
                    {
                        logging::warning(format!("Block navigation failed: {error}"));
                    }
                }
                ConsoleCommand::GotoBlockStatus => self.print_block_navigation_status().await,
                ConsoleCommand::CancelGotoBlock => {
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                }
                ConsoleCommand::Look { x, y, z } => {
                    self.interaction
                        .cancel(&self.minecraft, &self.movement, &self.look)
                        .await;
                    if let Err(error) = self
                        .tasks
                        .look_at(
                            &self.minecraft,
                            &self.look,
                            LookTarget::World(crate::minecraft::world_state::PositionSnapshot {
                                x: f64::from(x),
                                y: f64::from(y),
                                z: f64::from(z),
                            }),
                        )
                        .await
                    {
                        logging::warning(format!("Look failed: {error}"));
                    }
                }
                ConsoleCommand::LookBlock { block_id } => {
                    self.interaction
                        .cancel(&self.minecraft, &self.movement, &self.look)
                        .await;
                    if let Err(error) = self
                        .tasks
                        .look_at_block(&self.minecraft, &self.look, block_id)
                        .await
                    {
                        logging::warning(format!("Look failed: {error}"));
                    }
                }
                ConsoleCommand::LookPlayer { player } => {
                    self.interaction
                        .cancel(&self.minecraft, &self.movement, &self.look)
                        .await;
                    if let Err(error) = self
                        .tasks
                        .look_at(&self.minecraft, &self.look, LookTarget::Player(player))
                        .await
                    {
                        logging::warning(format!("Look failed: {error}"));
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
                            if let Err(error) = self
                                .tasks
                                .look_at(
                                    &self.minecraft,
                                    &self.look,
                                    LookTarget::Entity(entity.entity_id),
                                )
                                .await
                            {
                                logging::warning(format!("Look failed: {error}"));
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
                    if let Err(error) = self
                        .tasks
                        .break_looked_block(
                            &self.minecraft,
                            &self.movement,
                            &self.look,
                            &self.interaction,
                        )
                        .await
                    {
                        logging::warning(format!("Cannot break block: {error}"));
                    }
                }
                ConsoleCommand::Break { x, y, z } => {
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    if let Err(error) = self
                        .tasks
                        .break_block(
                            &self.minecraft,
                            &self.movement,
                            &self.look,
                            &self.interaction,
                            crate::minecraft::world_state::BlockPosition { x, y, z },
                        )
                        .await
                    {
                        logging::warning(format!("Cannot break block: {error}"));
                    }
                }
                ConsoleCommand::BreakNearest { block_id } => {
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    if let Err(error) = self
                        .tasks
                        .break_nearest_block(
                            &self.minecraft,
                            &self.movement,
                            &self.look,
                            &self.interaction,
                            block_id,
                        )
                        .await
                    {
                        logging::warning(format!("Cannot break block: {error}"));
                    }
                }
                ConsoleCommand::PlaceLooked { block_id } => {
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    if let Err(error) = self
                        .tasks
                        .place_looked_block(
                            &self.minecraft,
                            &self.movement,
                            &self.look,
                            &self.interaction,
                            block_id,
                        )
                        .await
                    {
                        logging::warning(format!("Cannot place block: {error}"));
                    }
                }
                ConsoleCommand::PlaceAt { x, y, z, block_id } => {
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    if let Err(error) = self
                        .tasks
                        .place_block(
                            &self.minecraft,
                            &self.movement,
                            &self.look,
                            &self.interaction,
                            crate::minecraft::world_state::BlockPosition { x, y, z },
                            block_id,
                        )
                        .await
                    {
                        logging::warning(format!("Cannot place block: {error}"));
                    }
                }
                ConsoleCommand::StopInteraction => {
                    self.interaction
                        .cancel(&self.minecraft, &self.movement, &self.look)
                        .await
                }
                ConsoleCommand::InteractionStatus => self.print_interaction_status().await,
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
        Ok(false)
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
        match self
            .tasks
            .find_block(&self.minecraft, &self.block_search, query.clone())
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
            "Selected slot: {}",
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
        println!(
            "Used slots: {}",
            world
                .inventory
                .slots
                .iter()
                .filter(|s| s.item_id.is_some())
                .count()
        );
        println!("Total item stacks: {}", world.inventory.total_counts.len());
        let mut items: Vec<_> = world.inventory.total_counts.iter().collect();
        items.sort_by_key(|(id, _)| *id);
        for (id, count) in items {
            println!("{id} x{count}");
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

    async fn print_task_status(&self) {
        let workflow = self.tasks.status().await;
        let movement = self.movement.snapshot().await;
        let movement_text = match movement.status {
            crate::minecraft::world_state::MovementStatus::FollowingPlayer => format!(
                "Following {}",
                movement.target_player.as_deref().unwrap_or("unknown")
            ),
            crate::minecraft::world_state::MovementStatus::MovingToPosition => {
                movement.destination.map_or_else(
                    || "Navigating".to_owned(),
                    |position| {
                        format!(
                            "Navigating to {:.0} {:.0} {:.0}",
                            position.x, position.y, position.z
                        )
                    },
                )
            }
            crate::minecraft::world_state::MovementStatus::Completed => "Completed".to_owned(),
            crate::minecraft::world_state::MovementStatus::Cancelled => "Cancelled".to_owned(),
            crate::minecraft::world_state::MovementStatus::Failed => "Failed".to_owned(),
            crate::minecraft::world_state::MovementStatus::Idle => "Idle".to_owned(),
        };
        let look = self.look.snapshot().await;
        let look_text = match look.state {
            LookState::Looking => {
                format!("Tracking {}", look.target.as_deref().unwrap_or("target"))
            }
            LookState::Completed => {
                format!("Looking at {}", look.target.as_deref().unwrap_or("target"))
            }
            LookState::Idle | LookState::Cancelled | LookState::Failed => "Idle".to_owned(),
        };
        println!("Movement: {movement_text}");
        println!("Look: {look_text}");
        if let Some(name) = workflow.name {
            println!("Workflow: {name} ({:?})", workflow.state);
            if let Some(step) = workflow.current_step {
                println!("Current task: {step}");
            }
        }
    }
}

fn fmt_opt<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "unknown".into(), |v| v.to_string())
}

fn print_help() {
    println!("/help       Show available commands");
    println!("/status     Show connection and application status");
    println!("/chat TEXT  Send TEXT to Minecraft chat");
    println!("/players    Show known online players");
    println!("/inventory  Show inventory summary");
    println!("/containerstatus  Show read-only active-container debug state");
    println!("/entities [RADIUS]  Show nearby entities");
    println!("/findblock ID [RADIUS] [LIMIT]  Find loaded blocks");
    println!("/nearestblock ID [RADIUS]  Find nearest loaded block");
    println!("/gotoblock ID [RADIUS] [mine]  Navigate to a matching block");
    println!("/navigate-to-block ID [RADIUS] [mine]  Alias for /gotoblock");
    println!("/gotoblockstatus  Show block-navigation status");
    println!("/cancelgotoblock  Cancel block navigation");
    println!("/goto X Y Z  Walk to coordinates without mining");
    println!("/goto-mine X Y Z  Navigate with Azalea mining-aware routing");
    println!("/path-status  Show Azalea pathfinder status");
    println!("/stop or /stopmovement  Stop movement only");
    println!("/stopall    Stop movement and looking");
    println!("/taskstatus Show movement and look tasks");
    println!("/follow NAME Follow a player");
    println!("/movement   Show movement status");
    println!("/look X Y Z  Look at a world position");
    println!("/lookblock ID  Look at a loaded block");
    println!("/lookplayer NAME  Look at a player");
    println!("/lookentity TYPE  Look at an entity");
    println!("/lookstop   Stop looking");
    println!("/lookstatus Show look status");
    println!("/breakblock  Break the block in the crosshair");
    println!("/break X Y Z  Break a block");
    println!("/breaknearest ID  Find, navigate to, and break a block");
    println!("/place ID  Place the held block beside the crosshair target");
    println!("/place X Y Z ID  Place a block at coordinates");
    println!("/placeblock ID [X Y Z]  Place through the reusable placement workflow");
    println!("/stopinteraction  Cancel block interaction");
    println!("/interactionstatus  Show interaction status");
    println!("/testoaklog  Break and restore the nearest oak log");
    println!("/reconnect  Reconnect to the configured server");
    println!("/quit       Shut down the application");
}

async fn await_console_task(task: JoinHandle<()>) {
    let _ = task.await;
}
