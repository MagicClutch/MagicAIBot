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
    logging,
    look::{LookController, LookTarget, look_controller::LookState},
    minecraft::client::MinecraftClient,
    movement::MovementService,
    navigation::{BlockNavigationService, navigation_state::BlockNavigationState},
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

        Ok(Self {
            minecraft: MinecraftClient::new(
                config.minecraft.clone(),
                config.reconnect.clone(),
                config.console.clone(),
                config.world_state.clone(),
            ),
            movement: MovementService::new(config.movement.clone()),
            block_search: BlockSearchService::new(
                config.block_search.maximum_radius,
                config.block_search.maximum_result_limit,
                config.block_search.default_vertical_range,
            ),
            block_navigation: BlockNavigationService::new(
                config.block_navigation.clone(),
                BlockSearchService::new(
                    config.block_search.maximum_radius,
                    config.block_search.maximum_result_limit,
                    config.block_search.default_vertical_range,
                ),
            ),
            look: LookController::new(
                config.look.clone(),
                BlockSearchService::new(
                    config.block_search.maximum_radius,
                    config.block_search.maximum_result_limit,
                    config.block_search.default_vertical_range,
                ),
            ),
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
                    self.movement.tick(&self.minecraft).await;
                    self.block_navigation.tick(&self.minecraft, &self.movement).await;
                },
                _ = look_tick.tick() => self.look.tick(&self.minecraft).await,
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
                ConsoleCommand::Entities { radius } => self.print_entities(radius).await,
                ConsoleCommand::Goto { x, y, z } => {
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    if let Err(error) = self
                        .movement
                        .goto(
                            &self.minecraft,
                            crate::minecraft::world_state::PositionSnapshot {
                                x: f64::from(x),
                                y: f64::from(y),
                                z: f64::from(z),
                            },
                        )
                        .await
                    {
                        println!("Movement error: {error}");
                    }
                }
                ConsoleCommand::Stop => {
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    if let Err(error) = self.movement.stop(&self.minecraft).await {
                        println!("Movement error: {error}");
                    }
                }
                ConsoleCommand::Follow { player } => {
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
                } => {
                    let radius =
                        search_radius.unwrap_or(self.config.block_navigation.default_search_radius);
                    if let Err(error) = self
                        .block_navigation
                        .start(&self.minecraft, &self.movement, block_id, radius)
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
                    if let Err(error) = self
                        .look
                        .look_at(
                            &self.minecraft,
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
                    if let Err(error) = self.look.look_at_block_id(&self.minecraft, block_id).await
                    {
                        logging::warning(format!("Look failed: {error}"));
                    }
                }
                ConsoleCommand::LookPlayer { player } => {
                    if let Err(error) = self
                        .look
                        .look_at(&self.minecraft, LookTarget::Player(player))
                        .await
                    {
                        logging::warning(format!("Look failed: {error}"));
                    }
                }
                ConsoleCommand::LookEntity { entity_type } => {
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
                                .look
                                .look_at(&self.minecraft, LookTarget::Entity(entity.entity_id))
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
                ConsoleCommand::Reconnect => {
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    self.look.cancel().await;
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
            .block_search
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
        println!("Yaw: {}", fmt_opt(snapshot.yaw));
        println!("Pitch: {}", fmt_opt(snapshot.pitch));
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
        if let Some(reason) = movement.failure_reason {
            println!("Failure reason: {reason}");
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
    println!("/entities [RADIUS]  Show nearby entities");
    println!("/findblock ID [RADIUS] [LIMIT]  Find loaded blocks");
    println!("/nearestblock ID [RADIUS]  Find nearest loaded block");
    println!("/gotoblock ID [RADIUS]  Navigate to a matching block");
    println!("/gotoblockstatus  Show block-navigation status");
    println!("/cancelgotoblock  Cancel block navigation");
    println!("/goto X Y Z  Walk to coordinates");
    println!("/stop       Stop movement immediately");
    println!("/follow NAME Follow a player");
    println!("/movement   Show movement status");
    println!("/look X Y Z  Look at a world position");
    println!("/lookblock ID  Look at a loaded block");
    println!("/lookplayer NAME  Look at a player");
    println!("/lookentity TYPE  Look at an entity");
    println!("/lookstop   Stop looking");
    println!("/lookstatus Show look status");
    println!("/reconnect  Reconnect to the configured server");
    println!("/quit       Shut down the application");
}

async fn await_console_task(task: JoinHandle<()>) {
    let _ = task.await;
}
