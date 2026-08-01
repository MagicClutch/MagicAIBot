use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    time::{Duration, Instant, SystemTime},
};

use crate::{
    blocks::{
        block_query::BlockSearchQuery,
        block_search::{BlockSearchService, format_find_results, format_nearest_result},
    },
    config::Config,
    console::{
        self,
        commands::{ConsoleCommand, ConsoleInput, plain_chat_message},
    },
    container::{model::TransferDirection, service::ContainerService},
    crafting::CraftService,
    crafting::RecipeBook,
    error::AppError,
    interaction::{InteractionController, interaction_controller::InteractionState},
    logging,
    look::{LookController, LookTarget, look_controller::LookState},
    minecraft::{
        client::MinecraftClient,
        world_state::{MovementStatus, TaskSnapshot},
    },
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

/// Bounded by `MovementConfig::maximum_navigation_seconds`. Azalea's
/// pathfinder is submitted with `retry_on_no_path(true)` (see
/// `MinecraftClient::start_navigation_to`), so a genuinely unreachable goal
/// (not enough scaffold material to finish a route, a destination behind
/// terrain the current policy can't cross) retries forever inside Azalea
/// without ever surfacing as a failure -- `MovementStatus` would just stay
/// `MovingToPosition` indefinitely. Without a deadline here, that would hang
/// this function forever, which hangs the single-threaded console/chat
/// command loop that calls it -- freezing the whole app (including `/stop`)
/// until the process is killed.
async fn await_movement_terminal(
    movement: &MovementService,
    minecraft: &MinecraftClient,
) -> Result<(), AppError> {
    let deadline = Duration::from_secs(movement.maximum_navigation_seconds());
    let started = Instant::now();
    loop {
        movement.tick(minecraft, false).await;
        let snapshot = movement.snapshot().await;
        match snapshot.status {
            MovementStatus::Completed | MovementStatus::Idle => return Ok(()),
            MovementStatus::Cancelled => return Err(AppError::MovementCancelled),
            MovementStatus::Failed => {
                return Err(AppError::PathfindingFailure(
                    snapshot
                        .failure_reason
                        .unwrap_or_else(|| "unknown reason".into()),
                ));
            }
            MovementStatus::MovingToPosition | MovementStatus::FollowingPlayer => {
                if started.elapsed() >= deadline {
                    let _ = movement.stop(minecraft).await;
                    return Err(AppError::PathfindingFailure(format!(
                        "movement timed out after {}s without reaching the destination or failing",
                        deadline.as_secs()
                    )));
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

async fn await_block_navigation_terminal(
    navigation: &BlockNavigationService,
    minecraft: &MinecraftClient,
    movement: &MovementService,
) -> Result<(), AppError> {
    loop {
        navigation.tick(minecraft, movement).await;
        let snapshot = navigation.snapshot().await;
        match snapshot.state {
            BlockNavigationState::Reached | BlockNavigationState::Idle => return Ok(()),
            BlockNavigationState::Cancelled => return Err(AppError::MovementCancelled),
            BlockNavigationState::Failed => {
                return Err(AppError::TaskRuntime(
                    snapshot
                        .failure_reason
                        .unwrap_or_else(|| "block navigation failed".into()),
                ));
            }
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
}

async fn await_look_terminal(
    look: &LookController,
    minecraft: &MinecraftClient,
) -> Result<(), AppError> {
    loop {
        look.tick(minecraft).await;
        let snapshot = look.snapshot().await;
        match snapshot.state {
            LookState::Completed | LookState::Idle => return Ok(()),
            LookState::Cancelled => return Err(AppError::LookCancelled),
            LookState::Failed => {
                return Err(AppError::LookUnavailableWithReason(
                    snapshot
                        .failure_reason
                        .unwrap_or_else(|| "look failed".into()),
                ));
            }
            LookState::Looking => tokio::time::sleep(Duration::from_millis(75)).await,
        }
    }
}

/// Interaction can internally hand off to block navigation (to get in range)
/// and to look (for precise aiming), so both must be driven alongside it or
/// the interaction state machine stalls waiting on a tick that never comes.
async fn await_interaction_terminal(
    interaction: &InteractionController,
    minecraft: &MinecraftClient,
    movement: &MovementService,
    block_navigation: &BlockNavigationService,
    look: &LookController,
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
                return Err(AppError::TaskRuntime(
                    snapshot
                        .failure_reason
                        .unwrap_or_else(|| "interaction failed".into()),
                ));
            }
            _ => tokio::time::sleep(Duration::from_millis(75)).await,
        }
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
    recipes: RecipeBook,
    crafting: CraftService,
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
            recipes: crate::crafting::RecipeBook::fallback().map_err(AppError::RecipeData)?,
            crafting: CraftService::default(),
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
                            self.block_navigation.cancel(&self.minecraft, &self.movement).await;
                            self.look.cancel().await;
                            let _ = self.movement.stop(&self.minecraft).await;
                            self.minecraft.clear_current_task().await;
                        }
                        continue;
                    }
                    self.session_ready = true;
                    self.tick_chat_commands().await;
                    let explicit_look = self.look.snapshot().await.state == LookState::Looking;
                    self.movement.tick(&self.minecraft, explicit_look).await;
                },
                _ = look_tick.tick() => {
                    let status = self.minecraft.navigation_status().await.ok();
                    if !status.is_some_and(|status| status.calculating || status.executing) {
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
                    self.container.tick(&self.minecraft, &self.movement, &self.block_navigation, &self.look).await;
                },
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
                ConsoleCommand::Recipe { id } => self.print_recipe(&id),
                ConsoleCommand::CraftCheck { item, count, depth } => {
                    self.print_craft_check(&item, count, depth).await
                }
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
                        .goto_and_wait("Go to position", destination, NavigationMode::AllowMining)
                        .await
                    {
                        Ok(()) => logging::success("Destination reached"),
                        Err(error) => logging::error(error),
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
                    if let Err(error) = self
                        .goto_and_wait("Go to position", destination, NavigationMode::AllowMining)
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
                    let radius =
                        search_radius.unwrap_or(self.config.block_navigation.default_search_radius);
                    if let Err(error) = self
                        .goto_block_and_wait(
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
                    logging::info(format!("Looking at ({x}, {y}, {z})"));
                    match self
                        .look_and_wait(
                            "Look at target",
                            LookTarget::World(crate::minecraft::world_state::PositionSnapshot {
                                x: f64::from(x),
                                y: f64::from(y),
                                z: f64::from(z),
                            }),
                        )
                        .await
                    {
                        Ok(()) => logging::success("Looking at target"),
                        Err(error) => logging::error(error),
                    }
                }
                ConsoleCommand::LookBlock { block_id } => {
                    self.interaction
                        .cancel(&self.minecraft, &self.movement, &self.look)
                        .await;
                    if let Err(error) = self.look_block_and_wait(block_id).await {
                        logging::warning(format!("Look failed: {error}"));
                    }
                }
                ConsoleCommand::LookPlayer { player } => {
                    self.interaction
                        .cancel(&self.minecraft, &self.movement, &self.look)
                        .await;
                    if let Err(error) = self
                        .look_and_wait("Look at player", LookTarget::Player(player))
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
                                .look_and_wait(
                                    "Look at entity",
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
                    if let Err(error) = self.break_looked_and_wait().await {
                        logging::warning(format!("Cannot break block: {error}"));
                    }
                }
                ConsoleCommand::Break { x, y, z } => {
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    logging::info(format!("Breaking block at ({x}, {y}, {z})"));
                    match self
                        .break_at_and_wait(crate::minecraft::world_state::BlockPosition { x, y, z })
                        .await
                    {
                        Ok(()) => logging::success("Block broken"),
                        Err(error) => logging::error(error),
                    }
                }
                ConsoleCommand::BreakNearest { block_id } => {
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    if let Err(error) = self.break_nearest_and_wait(block_id).await {
                        logging::warning(format!("Cannot break block: {error}"));
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
                        held_material_equivalence: self.config.interaction.held_tool_equivalence,
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
                    match self.place_looked_and_wait(block_id).await {
                        Ok(()) => logging::success("Block placed"),
                        Err(error) => logging::error(error),
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
                        )
                        .await
                    {
                        Ok(()) => logging::success("Block placed"),
                        Err(error) => logging::error(error),
                    }
                }
                ConsoleCommand::StopInteraction => {
                    self.interaction
                        .cancel(&self.minecraft, &self.movement, &self.look)
                        .await
                }
                ConsoleCommand::InteractionStatus => self.print_interaction_status().await,
                ConsoleCommand::Craft { target, count } => self.craft_item(target, count).await,
                ConsoleCommand::Equip { item } => self.equip_item(item).await,
                ConsoleCommand::CraftStatus => {
                    let status = self.crafting.status();
                    println!(
                        "Craft active: {}; recipe: {}; operations: {}; crafted: {}; last result: {:?}",
                        status.active,
                        status.recipe_id.as_deref().unwrap_or("none"),
                        status.completed_operations,
                        status.crafted,
                        status.last_status
                    );
                }
                ConsoleCommand::CraftStop => println!(
                    "{}",
                    if self.crafting.stop() {
                        "Craft cancellation requested."
                    } else {
                        "No craft is active."
                    }
                ),
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
                ConsoleCommand::EnsureTool { block_id } => {
                    println!(
                        "Ensure-tool planning is available, but no live crafting adapter is wired for {block_id}."
                    );
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
            },
            ConsoleInput::Empty => {}
        }
        Ok(false)
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

    async fn craft_item(&self, target: String, count: u32) {
        logging::info(format!("Crafting {count}x {target}"));
        match self
            .crafting
            .craft(&self.minecraft, &self.recipes, &target, count)
            .await
        {
            Ok(crafted) => logging::success(format!("Crafted {crafted} {target}")),
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
        if let Err(error) = self.execute_console_input(input).await {
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
    async fn tick_chat_commands(&mut self) {
        while let Some(chat) = self.minecraft.pop_incoming_player_chat().await {
            if chat.kind != crate::minecraft::world_state::ChatMessageKind::Player {
                continue;
            }
            if let Some(command_text) = chat.text.strip_prefix('#') {
                self.handle_chat_console_command(chat.sender, chat.sender_uuid, command_text)
                    .await;
            }
        }
    }

    fn print_recipe(&self, id: &str) {
        match self.recipes.recipe(id) {
            Ok(recipe) => {
                println!(
                    "Recipe {} -> {} x{}",
                    recipe.id, recipe.output, recipe.output_count
                );
                println!(
                    "layout={:?}; station={:?}; known={}; special={}",
                    recipe.layout, recipe.station, recipe.known, recipe.special
                );
                let source = self.recipes.source();
                println!(
                    "source={} protocol={} revision={} complete={}",
                    source.version, source.protocol, source.revision, source.complete
                );
            }
            Err(failure) => println!("Recipe unavailable: {failure:?}"),
        }
    }

    async fn print_craft_check(&self, item: &str, count: u32, depth: usize) {
        let world = self.minecraft.world_state_snapshot().await;
        if !world.inventory.available {
            println!("Craft check unavailable: inventory snapshot is unavailable");
            return;
        }
        // No crafting menu/station state is currently retained by the client
        // boundary. Reporting it unavailable is safer than navigating to or
        // claiming access to a table.
        let plan = self
            .recipes
            .plan(item, count, &world.inventory, false, depth);
        println!("Read-only craft plan for {item} x{count}:");
        if let Ok(recipe) = self.recipes.preferred(item) {
            let operations = count.div_ceil(recipe.output_count);
            let direct = self
                .recipes
                .availability(recipe, &world.inventory, false, operations);
            println!(
                "  direct: max_operations={}; missing={:?}; station={:?}",
                direct.maximum_crafts, direct.missing, direct.station_required
            );
        }
        for step in &plan.steps {
            println!(
                "  {}: {} operation(s) -> {} x{}",
                step.recipe_id, step.operations, step.output, step.produced
            );
        }
        match plan.failure {
            Some(failure) => println!("Unavailable: {failure:?}"),
            None => println!(
                "Available ({} step(s)); inventory was not modified.",
                plan.steps.len()
            ),
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

    async fn print_drops(&self, radius: Option<u32>) {
        use crate::minecraft::dropped_items::DroppedItemQuery;
        use std::time::{Duration, SystemTime};

        let world = self.minecraft.world_state_snapshot().await;
        let radius = f64::from(radius.unwrap_or(64));
        let query = DroppedItemQuery {
            radius: Some(radius),
            dimension: world.bot.dimension.clone(),
            maximum_age: Some(Duration::from_secs(
                self.config.world_state.stale_entity_seconds,
            )),
            limit: Some(64),
            ..Default::default()
        };
        let drops = query.search(&world.dropped_items, world.session_id, SystemTime::now());
        println!(
            "Dropped items: {} (session {}, radius {:.0})",
            drops.len(),
            world.session_id,
            radius
        );
        for drop in drops {
            println!(
                "#{} {} x{} | distance {:.1} | {:.2} {:.2} {:.2} | {} | uuid {}",
                drop.entity_id,
                drop.stack.item_id,
                drop.stack.count,
                drop.distance,
                drop.position.x,
                drop.position.y,
                drop.position.z,
                drop.dimension,
                drop.uuid
                    .map_or_else(|| "unavailable".into(), |uuid| uuid.to_string())
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
    ) -> Result<(), AppError> {
        self.minecraft.set_current_task(task_snapshot(name)).await;
        let result = async {
            self.movement
                .goto(&self.minecraft, destination, mode)
                .await?;
            await_movement_terminal(&self.movement, &self.minecraft).await
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
    ) -> Result<(), AppError> {
        self.minecraft
            .set_current_task(task_snapshot(format!("Go to {block_id}")))
            .await;
        let result = async {
            self.block_navigation
                .start(&self.minecraft, &self.movement, block_id, radius, mode)
                .await?;
            await_block_navigation_terminal(&self.block_navigation, &self.minecraft, &self.movement)
                .await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn look_and_wait(&self, name: &str, target: LookTarget) -> Result<(), AppError> {
        self.minecraft.set_current_task(task_snapshot(name)).await;
        let result = async {
            self.look.look_at(&self.minecraft, target).await?;
            await_look_terminal(&self.look, &self.minecraft).await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn look_block_and_wait(&self, block_id: String) -> Result<(), AppError> {
        self.minecraft
            .set_current_task(task_snapshot(format!("Look at {block_id}")))
            .await;
        let result = async {
            self.look
                .look_at_block_id(&self.minecraft, block_id)
                .await?;
            await_look_terminal(&self.look, &self.minecraft).await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn break_looked_and_wait(&self) -> Result<(), AppError> {
        self.minecraft
            .set_current_task(task_snapshot("Break looked block"))
            .await;
        let result = async {
            self.interaction
                .break_looked(&self.minecraft, &self.movement, &self.look)
                .await?;
            await_interaction_terminal(
                &self.interaction,
                &self.minecraft,
                &self.movement,
                &self.block_navigation,
                &self.look,
            )
            .await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn break_at_and_wait(
        &self,
        target: crate::minecraft::world_state::BlockPosition,
    ) -> Result<(), AppError> {
        self.minecraft
            .set_current_task(task_snapshot("Break block"))
            .await;
        let result = async {
            self.interaction
                .break_at(&self.minecraft, &self.movement, &self.look, target)
                .await?;
            await_interaction_terminal(
                &self.interaction,
                &self.minecraft,
                &self.movement,
                &self.block_navigation,
                &self.look,
            )
            .await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn break_nearest_and_wait(&self, block_id: String) -> Result<(), AppError> {
        self.minecraft
            .set_current_task(task_snapshot(format!("Break nearest {block_id}")))
            .await;
        let result = async {
            self.interaction
                .break_nearest(&self.minecraft, &self.movement, &self.look, block_id)
                .await?;
            await_interaction_terminal(
                &self.interaction,
                &self.minecraft,
                &self.movement,
                &self.block_navigation,
                &self.look,
            )
            .await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn place_looked_and_wait(&self, item: String) -> Result<(), AppError> {
        self.minecraft
            .set_current_task(task_snapshot(format!("Place {item}")))
            .await;
        let result = async {
            self.interaction
                .place_looked(&self.minecraft, &self.movement, &self.look, item)
                .await?;
            await_interaction_terminal(
                &self.interaction,
                &self.minecraft,
                &self.movement,
                &self.block_navigation,
                &self.look,
            )
            .await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
    }

    async fn place_at_and_wait(
        &self,
        target: crate::minecraft::world_state::BlockPosition,
        item: String,
    ) -> Result<(), AppError> {
        self.minecraft
            .set_current_task(task_snapshot(format!("Place {item}")))
            .await;
        let result = async {
            self.interaction
                .place_at(&self.minecraft, &self.movement, &self.look, target, item)
                .await?;
            await_interaction_terminal(
                &self.interaction,
                &self.minecraft,
                &self.movement,
                &self.block_navigation,
                &self.look,
            )
            .await
        }
        .await;
        self.minecraft.clear_current_task().await;
        result
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
    println!();
    println!("Inventory");
    println!("  /equip <item>              Equip an item to the active hotbar slot");
    println!("  /craft <item> [count]      Craft an item using the player crafting grid");
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
