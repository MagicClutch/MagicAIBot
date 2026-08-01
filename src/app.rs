use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    time::{Duration, Instant},
};

use crate::{
    ai::{
        self, AiAction, AiSession, AiSessionStatus, CapabilityResult, CapabilityStatus,
        InventoryItemSummary, InventorySummary,
        provider::{AiProviderError, AiRequest as AiProviderRequest, ChatMessage},
    },
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
        world_state::{BlockPosition, MovementStatus, PlayerSnapshot, WorldStateSnapshot},
    },
    movement::{MovementService, NavigationMode},
    navigation::BlockNavigationService,
    navigation::navigation_state::BlockNavigationState,
    tasks::{CancellationReason, TaskId, TaskService},
};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

/// How a dispatched AI action's real-world completion should be detected.
/// `WalkTo`/`BreakBlock`/`PlaceBlock` start an azalea/interaction state
/// machine that only advances on later ticks, so their result cannot be
/// known at dispatch time; everything else already completes synchronously.
#[derive(Clone)]
enum PendingAction {
    Instant(CapabilityResult),
    Movement,
    Interaction,
    /// Waits for the look controller to finish (or give up on) turning
    /// toward a player -- submitted via a fire-and-forget `look_at` -- then
    /// performs the actual drop. A blocking `sleep` here would freeze the
    /// same tick loop that drives `LookController::tick`, so it would never
    /// observe any progress; this defers the drop to `tick_ai_action_completion`
    /// instead, same as `Movement`/`Interaction`.
    LookThenDrop {
        item_id: String,
        count: u16,
    },
    /// Waits for `LookAt`'s fire-and-forget `look_at` submission to actually
    /// finish turning (or fail/time out) before reporting a result -- same
    /// fake-instant-completion problem `LookThenDrop` was built for.
    Look,
    /// Waits for the container state machine (driven by `ContainerService::tick`,
    /// called every interaction tick) to reach a terminal phase before
    /// reporting open/take/store/close as done.
    Container,
}

/// Application composition root. Exposes primitive capabilities to the AI.
pub struct App {
    config: Config,
    shutdown: CancellationToken,
    minecraft: MinecraftClient,
    movement: MovementService,
    block_navigation: BlockNavigationService,
    look: LookController,
    interaction: InteractionController,
    vertical_construction: crate::navigation::VerticalConstructionService,
    tasks: TaskService,
    recipes: RecipeBook,
    crafting: CraftService,
    container: ContainerService,
    ai_session: Option<AiSession>,
    chat_rate_limits: HashMap<String, VecDeque<Instant>>,
    session_ready: bool,
    started_at: Instant,
    ai_request_in_flight: bool,
    player_was_dead: bool,
    physical_action_active: bool,
    pending_action: Option<PendingAction>,
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
        );
        minecraft
            .set_incoming_player_chat_capacity(config.ai.chat.incoming_queue_capacity)
            .await;
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
            vertical_construction: crate::navigation::VerticalConstructionService::default(),
            tasks: TaskService::default(),
            recipes: crate::crafting::RecipeBook::fallback().map_err(AppError::RecipeData)?,
            crafting: CraftService::default(),
            container: ContainerService::default(),
            ai_session: None,
            chat_rate_limits: HashMap::new(),
            session_ready: false,
            ai_request_in_flight: false,
            physical_action_active: false,
            pending_action: None,
            player_was_dead: false,
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
                            self.tasks.disconnected();
                            self.minecraft.clear_current_task().await;
                        }
                        continue;
                    }
                    self.session_ready = true;
                    self.enforce_ai_limits().await;
                    self.tick_ai_provider().await;
                    self.tick_ai_chat().await;
                    let currently_dead = self.minecraft.world_state_snapshot().await.bot.alive == Some(false);
                    if currently_dead && !self.player_was_dead {
                        self.player_was_dead = true;
                        self.tasks.player_died();
                    } else if !currently_dead {
                        self.player_was_dead = false;
                    }
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
                    self.tick_ai_action_completion().await;
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
        self.vertical_construction.cancel();
        self.container
            .cancel(
                &self.minecraft,
                &self.block_navigation,
                &self.movement,
                &self.look,
            )
            .await;
        self.tasks.shutdown();
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
                ConsoleCommand::Ai { request } => self.start_ai(request).await,
                ConsoleCommand::AiCancel => self.cancel_ai().await,
                ConsoleCommand::AiStatus => self.print_ai_status(),
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
                    match self.goto_with_terrain_assist(destination).await {
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
                    self.vertical_construction.cancel();
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    match self.movement.stop(&self.minecraft).await {
                        Ok(()) => logging::success("Movement stopped"),
                        Err(error) => logging::error(error),
                    }
                }
                ConsoleCommand::StopAll => {
                    println!("This command is no longer available; use AI capabilities instead.");
                }
                ConsoleCommand::TaskStatus => self.print_task_status().await,
                ConsoleCommand::GatherStatus => self.print_task_status().await,
                ConsoleCommand::GatherCancel => {
                    println!("This command is no longer available; use AI capabilities instead.");
                }
                ConsoleCommand::TaskStatusById { id } => self.print_task_by_id(TaskId(id)),
                ConsoleCommand::TaskCancel { id } => {
                    if let Some(id) = id {
                        println!(
                            "Cancellation requested: {}",
                            self.tasks.cancel_task(TaskId(id), CancellationReason::User)
                        );
                    } else {
                        self.tasks.cancel_all(CancellationReason::User);
                        println!("Cancellation requested for all active tasks.");
                    }
                }
                ConsoleCommand::TaskRecent => self.print_recent_tasks(),
                ConsoleCommand::Gather { .. } => {
                    println!("This command is no longer available; use AI capabilities instead.");
                }
                ConsoleCommand::Follow { player } => {
                    self.interaction
                        .cancel(&self.minecraft, &self.movement, &self.look)
                        .await;
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    self.close_initial_gap_to_player(&player).await;
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
                    logging::info(format!("Looking at ({x}, {y}, {z})"));
                    match self
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
                        Ok(()) => logging::success("Looking at target"),
                        Err(error) => logging::error(error),
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
                            &self.block_navigation,
                        )
                        .await
                    {
                        logging::warning(format!("Cannot break block: {error}"));
                    }
                }
                ConsoleCommand::MineOre { .. } => {
                    println!("This command is no longer available; use AI capabilities instead.");
                }
                ConsoleCommand::MineOreStatus => {
                    println!("This command is no longer available; use AI capabilities instead.");
                }
                ConsoleCommand::MineOreStop => {
                    println!("This command is no longer available; use AI capabilities instead.");
                }
                ConsoleCommand::Break { x, y, z } => {
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    logging::info(format!("Breaking block at ({x}, {y}, {z})"));
                    match self
                        .tasks
                        .break_block(
                            &self.minecraft,
                            &self.movement,
                            &self.look,
                            &self.interaction,
                            &self.block_navigation,
                            crate::minecraft::world_state::BlockPosition { x, y, z },
                        )
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
                    if let Err(error) = self
                        .tasks
                        .break_nearest_block(
                            &self.minecraft,
                            &self.movement,
                            &self.look,
                            &self.interaction,
                            &self.block_navigation,
                            block_id,
                        )
                        .await
                    {
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
                    match self
                        .tasks
                        .place_looked_block(
                            &self.minecraft,
                            &self.movement,
                            &self.look,
                            &self.interaction,
                            &self.block_navigation,
                            block_id,
                        )
                        .await
                    {
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
                        .tasks
                        .place_block(
                            &self.minecraft,
                            &self.movement,
                            &self.look,
                            &self.interaction,
                            &self.block_navigation,
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
                ConsoleCommand::CollectItem(_) => {
                    println!("This command is no longer available; use AI capabilities instead.");
                }
                ConsoleCommand::CollectItemStatus => {
                    println!("This command is no longer available; use AI capabilities instead.");
                }
                ConsoleCommand::CollectItemStop => {
                    println!("This command is no longer available; use AI capabilities instead.");
                }
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
                ConsoleCommand::CollectFood { .. } => {
                    println!("This command is no longer available; use AI capabilities instead.");
                }
                ConsoleCommand::CollectFoodStatus => {
                    println!("This command is no longer available; use AI capabilities instead.");
                }
                ConsoleCommand::CollectFoodStop => {
                    println!("This command is no longer available; use AI capabilities instead.");
                }
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
                ConsoleCommand::ChopTree { .. } => {
                    println!("This command is no longer available; use AI capabilities instead.");
                }
                ConsoleCommand::ChopTreeStatus => {
                    println!("This command is no longer available; use AI capabilities instead.");
                }
                ConsoleCommand::ChopTreeStop => {
                    println!("This command is no longer available; use AI capabilities instead.");
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

    /// Walks to `destination`, escalating through movement strategies in
    /// order of cost/risk when the previous one fails: (1) a plain walk/jump
    /// path -- Azalea's own pathfinder, unmodified; (2) the same pathfinder
    /// with mining enabled, so obstructing blocks are broken automatically
    /// (real, tested upstream Azalea capability); (3) terrain construction
    /// (staircasing, bridging, pillaring, or digging) to prepare ground
    /// Azalea can't cross on its own, then retrying with mining enabled.
    /// Bounded so a route that genuinely cannot be completed still fails
    /// instead of looping forever.
    async fn goto_with_terrain_assist(
        &self,
        destination: crate::minecraft::world_state::PositionSnapshot,
    ) -> Result<(), AppError> {
        const MAX_ATTEMPTS: u32 = 8;
        let target = crate::minecraft::world_state::BlockPosition {
            x: destination.x.round() as i32,
            y: destination.y.round() as i32,
            z: destination.z.round() as i32,
        };
        let mut last_error = None;
        let mut tried_mining = false;
        for attempt in 0..MAX_ATTEMPTS {
            let mode = if tried_mining {
                NavigationMode::AllowMining
            } else {
                NavigationMode::MovementOnly
            };
            match self
                .tasks
                .goto_position(&self.minecraft, &self.movement, destination, mode)
                .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    last_error = Some(error);
                    if !tried_mining {
                        // Retry immediately with mining enabled before
                        // reaching for terrain construction -- cheap and
                        // already fully implemented by Azalea.
                        tried_mining = true;
                        continue;
                    }
                    if !self.config.vertical_navigation.enabled || attempt + 1 == MAX_ATTEMPTS {
                        break;
                    }
                    if self
                        .vertical_construction
                        .assist_toward(
                            &self.minecraft,
                            &self.movement,
                            &self.look,
                            &self.interaction,
                            &self.block_navigation,
                            &self.config.vertical_navigation,
                            target,
                        )
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        Err(last_error.unwrap_or(AppError::PathfindingFailure("unknown reason".into())))
    }

    /// Best-effort: prepare terrain toward a followed player once before
    /// starting the continuous follow, so following onto a tower, across a
    /// gap, or into a pit does not immediately stall on the first repath.
    /// Mid-follow terrain changes (the player climbing further while already
    /// being followed) are not handled -- see the limitation in the final
    /// report.
    async fn close_initial_gap_to_player(&self, player: &str) {
        if !self.config.vertical_navigation.enabled {
            return;
        }
        let world = self.minecraft.world_state_snapshot().await;
        let Some(target) = world
            .find_player_by_name(player)
            .and_then(|player| player.position)
        else {
            return;
        };
        let target = crate::minecraft::world_state::BlockPosition {
            x: target.x.round() as i32,
            y: target.y.round() as i32,
            z: target.z.round() as i32,
        };
        let _ = self
            .vertical_construction
            .assist_toward(
                &self.minecraft,
                &self.movement,
                &self.look,
                &self.interaction,
                &self.block_navigation,
                &self.config.vertical_navigation,
                target,
            )
            .await;
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
        match self
            .tasks
            .find_block(&self.minecraft, &block_search, query.clone())
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

    async fn start_ai(&mut self, request: String) {
        if !self.config.ai.chat.accept_console_ai_command {
            println!("[AI] Console AI requests are disabled by configuration.");
            return;
        }
        self.submit_ai_request(crate::ai::AiRequest {
            text: request,
            requester: None,
            source: crate::ai::AiRequestSource::Console,
        })
        .await;
    }
    fn can_accept_ai_request(&self) -> Result<(), &'static str> {
        match self.config.ai.busy_behavior {
            crate::config::AiBusyBehavior::Reject => {
                if self.ai_session.as_ref().is_some_and(AiSession::is_active) {
                    Err("busy")
                } else {
                    Ok(())
                }
            }
        }
    }

    fn chat_access_allowed(&self, requester: &crate::ai::AiRequester) -> bool {
        let access = &self.config.ai.chat.access;
        // Azalea does not expose a stable operator permission signal here.
        // Do not guess: operators_only rejects until a trusted adapter exists.
        if access.operators_only
            || access
                .blocked_players
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&requester.name))
        {
            return false;
        }
        access.allowed_players.is_empty()
            || access
                .allowed_players
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&requester.name))
    }

    fn consume_chat_rate_limit(&mut self, requester: &crate::ai::AiRequester) -> bool {
        let limit = &self.config.ai.chat.rate_limit;
        if !limit.enabled {
            return true;
        }
        let key = requester
            .uuid
            .map_or_else(|| requester.name.to_ascii_lowercase(), |id| id.to_string());
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

    async fn send_ai_chat_response(&self, message: &str) {
        if !self.config.ai.chat.respond_in_chat {
            return;
        }
        let message = message.trim().trim_start_matches('/').trim();
        if message.is_empty() {
            return;
        }
        let prefixed = if message.starts_with("[AI]") {
            message.to_owned()
        } else {
            format!("[AI] {message}")
        };
        for chunk in ai_chat_chunks(&prefixed, crate::minecraft::client::MAX_CHAT_MESSAGE_LENGTH) {
            if let Err(error) = self.minecraft.send_chat(&chunk).await {
                logging::warning(format!(
                    "[AI] Chat response could not be delivered: {error}"
                ));
                break;
            }
        }
    }

    /// Format current inventory as a structured summary for AI consumption.
    async fn inventory_summary(&self) -> InventorySummary {
        let world = self.minecraft.world_state_snapshot().await;
        let mut slots = Vec::new();
        for slot in &world.inventory.slots {
            if let Some(ref id) = slot.item_id {
                let display_name = slot.display_name.as_deref().unwrap_or(id).to_owned();
                slots.push(InventoryItemSummary {
                    item_id: id.clone(),
                    display_name,
                    count: slot.count,
                    slot: slot.slot as u16,
                });
            }
        }
        let total_slots: usize = 41; // 36 inventory + 5 armor/offhand
        let empty_slots = total_slots.saturating_sub(slots.len());
        InventorySummary {
            slots,
            empty_slots,
            selected_hotbar_slot: world.inventory.selected_hotbar_slot.unwrap_or(0),
        }
    }

    /// Execute a read-only tool call (knowledge query or state query).
    /// Returns a JSON-serializable value to feed back to the AI.
    /// `session` is passed explicitly rather than read from `self.ai_session`
    /// because the only caller (`run_ai_turn`) holds the session as a local
    /// variable for the whole turn (`self.ai_session.take()` at the top) --
    /// `self.ai_session` is `None` for the entire duration this can be
    /// called, which previously made `get_active_action` permanently report
    /// "no active action" regardless of real state.
    async fn execute_readonly_tool(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
        session: &AiSession,
    ) -> serde_json::Value {
        match tool_name {
            "query_inventory" => {
                let summary = self.inventory_summary().await;
                serde_json::to_value(&summary)
                    .unwrap_or(serde_json::json!({"error": "inventory unavailable"}))
            }
            "get_bot_position" | "get_bot_health" => {
                let world = self.minecraft.world_state_snapshot().await;
                let pos = world.bot.position.unwrap_or_default();
                serde_json::json!({
                    "position": { "x": pos.x, "y": pos.y, "z": pos.z },
                    "health": world.bot.health,
                    "dimension": world.bot.dimension,
                    "alive": world.bot.alive,
                })
            }
            "get_food_level" => {
                let world = self.minecraft.world_state_snapshot().await;
                serde_json::json!({
                    "food": world.bot.food_level,
                    "saturation": world.bot.saturation,
                })
            }
            "get_equipment" => {
                let world = self.minecraft.world_state_snapshot().await;
                let selected = world.inventory.selected_item().map(|item| {
                    serde_json::json!({
                        "item_id": item.item_id,
                        "count": item.count,
                        "slot": item.slot,
                    })
                });
                serde_json::json!({
                    "selected_hotbar_slot": world.inventory.selected_hotbar_slot,
                    "selected_item": selected,
                })
            }
            "get_recipe" => {
                let item = arguments
                    .get("item")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let recipe = self.recipes.recipe(item);
                match recipe {
                    Ok(r) => {
                        let ingredients: Vec<serde_json::Value> = match &r.layout {
                            crate::crafting::RecipeLayout::Shaped { ingredients, .. }
                            | crate::crafting::RecipeLayout::Shapeless { ingredients } => {
                                ingredients
                                    .iter()
                                    .filter_map(|is| {
                                        let name = match &is.ingredient {
                                            crate::crafting::Ingredient::Empty => return None,
                                            crate::crafting::Ingredient::Item(id) => id.clone(),
                                            crate::crafting::Ingredient::Alternatives(alts) => {
                                                alts.first()?.clone()
                                            }
                                            crate::crafting::Ingredient::Tag(t) => {
                                                format!("#{}", t)
                                            }
                                            crate::crafting::Ingredient::Special(s) => s.clone(),
                                        };
                                        Some(serde_json::json!({"item": name, "count": is.count}))
                                    })
                                    .collect()
                            }
                        };
                        let shape = match &r.layout {
                            crate::crafting::RecipeLayout::Shaped { width, height, .. } => {
                                serde_json::json!({"type": "shaped", "width": width, "height": height})
                            }
                            crate::crafting::RecipeLayout::Shapeless { .. } => {
                                serde_json::json!({"type": "shapeless"})
                            }
                        };
                        serde_json::json!({
                            "item": r.output,
                            "count": r.output_count,
                            "ingredients": ingredients,
                            "station": r.station,
                            "shape": shape,
                        })
                    }
                    Err(_) => serde_json::json!({"error": format!("No recipe found for {}", item)}),
                }
            }
            "get_item_information" => {
                let item = arguments
                    .get("item")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let recipe = self.recipes.recipe(item);
                match recipe {
                    Ok(r) => serde_json::json!({
                        "item": r.output,
                        "display_name": r.output,
                        "stack_size": 64,
                        "craftable": true,
                        "station": r.station,
                    }),
                    Err(_) => serde_json::json!({
                        "item": item,
                        "display_name": item,
                        "stack_size": 64,
                        "craftable": false,
                    }),
                }
            }
            "get_block_information" => {
                let block = arguments
                    .get("block")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let (hardness, tool) = match block {
                    b if b.contains("stone") || b.contains("cobblestone") => (1.5, "pickaxe"),
                    b if b.contains("iron_ore") => (3.0, "stone_pickaxe"),
                    b if b.contains("diamond_ore") => (3.0, "iron_pickaxe"),
                    b if b.contains("obsidian") => (50.0, "diamond_pickaxe"),
                    b if b.contains("log") || b.contains("wood") || b.contains("oak") => {
                        (2.0, "axe")
                    }
                    b if b.contains("dirt")
                        || b.contains("grass")
                        || b.contains("sand")
                        || b.contains("gravel") =>
                    {
                        (0.5, "shovel")
                    }
                    _ => (1.0, "hand"),
                };
                serde_json::json!({
                    "block": block,
                    "hardness": hardness,
                    "required_tool": tool,
                })
            }
            "get_available_capabilities" => {
                let exec_names: Vec<&str> = ai::registry::command_registry()
                    .iter()
                    .filter(|c| c.enabled)
                    .map(|c| c.name)
                    .collect();
                let readonly_names = ai::readonly_tool_names();
                serde_json::json!({
                    "read_only_capabilities": readonly_names,
                    "executable_capabilities": exec_names,
                })
            }
            "get_active_action" => {
                let movement = self.movement.snapshot().await;
                let follow = (movement.status
                    == crate::minecraft::world_state::MovementStatus::FollowingPlayer)
                    .then(|| {
                        serde_json::json!({
                            "following": movement.target_player,
                            "status": format!("{:?}", movement.status),
                        })
                    });
                if let Some(active) = &session.active_action {
                    serde_json::json!({
                        "active": true,
                        "action": format!("{:?}", active.action),
                        "status": format!("{:?}", active.status),
                        "elapsed_seconds": active.started_at.elapsed().as_secs_f64(),
                        "background_follow": follow,
                    })
                } else {
                    serde_json::json!({
                        "active": false,
                        "session_status": format!("{:?}", session.status),
                        "background_follow": follow,
                    })
                }
            }
            "get_nearby_entities" => {
                let radius = arguments
                    .get("radius")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(32) as f64;
                let world = self.minecraft.world_state_snapshot().await;
                let in_range: Vec<_> = world
                    .entities
                    .iter()
                    .filter(|e| {
                        let dx = e.position.x - world.bot.position.unwrap_or_default().x;
                        let dz = e.position.z - world.bot.position.unwrap_or_default().z;
                        (dx * dx + dz * dz).sqrt() <= radius
                    })
                    .collect();
                const MAX_REPORTED: usize = 20;
                let nearby: Vec<serde_json::Value> = in_range
                    .iter()
                    .take(MAX_REPORTED)
                    .map(|e| {
                        serde_json::json!({
                            "id": e.entity_id,
                            "kind": e.entity_type,
                            "position": { "x": e.position.x, "y": e.position.y, "z": e.position.z },
                        })
                    })
                    .collect();
                serde_json::json!({
                    "entities": nearby,
                    "count": nearby.len(),
                    "total_in_range": in_range.len(),
                    "truncated": in_range.len() > MAX_REPORTED,
                })
            }
            "get_block" => {
                let x = arguments
                    .get("x")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0) as i32;
                let y = arguments
                    .get("y")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0) as i32;
                let z = arguments
                    .get("z")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0) as i32;
                match self.minecraft.block_id_at(BlockPosition { x, y, z }).await {
                    // `None` here means the chunk isn't loaded/tracked, not
                    // that the block is air -- air is a real, present block
                    // state (`minecraft:air`) that `block_id_at` reports
                    // like any other when the chunk *is* loaded.
                    Ok(Some(block_id)) => serde_json::json!({
                        "position": { "x": x, "y": y, "z": z },
                        "block_id": block_id,
                    }),
                    Ok(None) => serde_json::json!({
                        "position": { "x": x, "y": y, "z": z },
                        "error": "chunk not loaded at this position",
                    }),
                    Err(e) => serde_json::json!({
                        "position": { "x": x, "y": y, "z": z },
                        "error": format!("block data unavailable: {e}"),
                    }),
                }
            }
            "find_blocks" => {
                let block_ids: Vec<String> = arguments
                    .get("block_ids")
                    .and_then(serde_json::Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                let radius = arguments
                    .get("radius")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(32) as u32;
                let max_results = arguments
                    .get("maximum_results")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(16) as usize;
                if block_ids.is_empty() {
                    return serde_json::json!({"blocks": [], "count": 0, "error": "No block IDs specified"});
                }
                let mut all_results = Vec::new();
                let mut errors = Vec::new();
                let block_search = BlockSearchService::new(
                    self.config.block_search.maximum_radius,
                    self.config.block_search.maximum_result_limit,
                    self.config.block_search.default_vertical_range,
                );
                for block_id in &block_ids {
                    // Without normalizing (lowercase, add "minecraft:" if
                    // missing -- same as every other block-id path in this
                    // codebase), a bare id like "oak_log" fails to parse
                    // downstream and previously vanished into an ignored
                    // `Err`, indistinguishable from "genuinely nothing here".
                    let normalized = match crate::blocks::block_query::normalize_block_id(block_id)
                    {
                        Ok(id) => id,
                        Err(e) => {
                            errors.push(format!("{block_id}: {e}"));
                            continue;
                        }
                    };
                    let query = BlockSearchQuery {
                        block_id: normalized,
                        radius,
                        maximum_results: max_results,
                        vertical_range: self.config.block_search.default_vertical_range,
                    };
                    match self
                        .tasks
                        .find_block(&self.minecraft, &block_search, query)
                        .await
                    {
                        Ok(found) => all_results.extend(found),
                        Err(e) => errors.push(format!("{block_id}: {e}")),
                    }
                }
                all_results.sort_by(|a, b| {
                    a.distance
                        .partial_cmp(&b.distance)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                all_results.truncate(max_results);
                let blocks: Vec<serde_json::Value> = all_results
                    .iter()
                    .map(|b| {
                        serde_json::json!({
                            "position": { "x": b.position.x, "y": b.position.y, "z": b.position.z },
                            "block_id": b.block_id,
                            "distance": b.distance,
                        })
                    })
                    .collect();
                let mut response = serde_json::json!({"blocks": blocks, "count": blocks.len()});
                if !errors.is_empty() {
                    response["errors"] = serde_json::json!(errors);
                }
                response
            }
            _ => serde_json::json!({"error": format!("Unknown read-only tool: {}", tool_name)}),
        }
    }

    /// Entry point for a brand-new user request (console/chat). Starts a
    /// fresh session and hands off to `run_ai_turn`, which is also the
    /// resumption point after every action completes -- see
    /// `complete_ai_action` for why that re-entrancy is required at all.
    async fn submit_ai_request(&mut self, incoming: crate::ai::AiRequest) {
        if !ai::active_provider_enabled(&self.config) {
            println!(
                "[AI] Selected provider ({:?}) is disabled in its config section.",
                self.config.ai.provider
            );
            return;
        }

        let has_active_session = self.ai_session.as_ref().is_some_and(AiSession::is_active);
        let text_lower = incoming.text.to_ascii_lowercase();
        let is_conversation_or_query = !has_active_session
            || text_lower.starts_with("what")
            || text_lower.starts_with("how")
            || text_lower.starts_with("do you")
            || text_lower.starts_with("are you")
            || text_lower.contains("inventory")
            || text_lower.contains("health")
            || text_lower.contains("position")
            || text_lower.contains("status")
            || text_lower.contains("doing")
            || text_lower.contains("nearby");

        if has_active_session && !is_conversation_or_query {
            println!("[AI] Another AI task is already active.");
            if incoming.source == crate::ai::AiRequestSource::MinecraftChat {
                let reply =
                    "I am already completing another task. Please ask for status or say stop.";
                self.send_ai_chat_response(reply).await;
            }
            return;
        }

        let mut session = AiSession::new(
            incoming.text.clone(),
            self.config.groq.limits.max_actions_per_session,
        );
        if let Some(requester) = incoming.requester.as_ref() {
            session.requester = Some(requester.name.clone());
        }
        self.ai_session = Some(session);
        self.run_ai_turn(None).await;
    }

    /// Advances `self.ai_session` by one or more provider round-trips until
    /// either an executable action is dispatched (returning control to the
    /// tick loop, which resumes this function via `complete_ai_action` once
    /// the action's real-world result is known) or the session reaches a
    /// terminal state. `previous_result` is a human-readable summary of the
    /// action that just completed, fed back to the model as context.
    async fn run_ai_turn(&mut self, previous_result: Option<String>) {
        let Some(mut session) = self.ai_session.take() else {
            return;
        };

        let provider = match ai::build_provider(&self.config) {
            Ok(p) => p,
            Err(e) => {
                println!("[AI] Failed to create AI provider: {e}");
                session.status = AiSessionStatus::Failed;
                self.ai_session = Some(session);
                self.finalize_ai_task().await;
                self.ai_request_in_flight = false;
                return;
            }
        };
        logging::info("[AI] Request received");
        self.ai_request_in_flight = true;

        let request = session.original_request.clone();
        let requester = session
            .requester
            .clone()
            .map(|name| crate::ai::AiRequester {
                name,
                uuid: None,
                source: crate::ai::AiRequestSource::Internal,
            });
        let binding_request = crate::ai::AiRequest {
            text: request.clone(),
            requester: requester.clone(),
            source: crate::ai::AiRequestSource::Internal,
        };

        let mut messages: Vec<ChatMessage> = Vec::new();
        // Readonly tool round-trips do not advance `session.current_step`, so
        // they need their own bound to keep a model that only ever calls
        // read-only tools from looping forever.
        let mut provider_calls: u32 = 0;
        let max_provider_calls = (self.config.groq.limits.max_actions_per_session as u32 + 1) * 6;

        loop {
            if session.current_step >= self.config.groq.limits.max_actions_per_session
                || session.current_step >= self.config.groq.max_request_retries as usize * 10
            {
                println!("[AI] Action limit reached.");
                session.status = AiSessionStatus::Failed;
                self.ai_session = Some(session);
                self.finalize_ai_task().await;
                self.ai_request_in_flight = false;
                return;
            }
            provider_calls += 1;
            if provider_calls > max_provider_calls {
                println!("[AI] Too many provider round-trips without a physical action.");
                session.status = AiSessionStatus::Failed;
                self.ai_session = Some(session);
                self.finalize_ai_task().await;
                self.ai_request_in_flight = false;
                return;
            }

            let world = self.minecraft.world_state_snapshot().await;
            let bot = world.bot.position.unwrap_or_default();

            let requester_info = requester.as_ref().map(|r| {
                format!("The trusted requester is player {}. Words such as me, my, and I refer to this player.", r.name)
            }).unwrap_or_default();

            let context = format!(
                "User request: {}\n{}\nBot position: {:.1} {:.1} {:.1}\nHealth: {:?}\nHunger: {:?}\nInventory items: {}\nNearby players: {}\nPrevious action result: {}\nStep: {}/{}{}",
                request,
                requester_info,
                bot.x,
                bot.y,
                bot.z,
                world.bot.health,
                world.bot.food_level,
                world.inventory.total_counts.len(),
                world
                    .players
                    .iter()
                    .take(8)
                    .map(|p| p.username.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                previous_result.as_deref().unwrap_or("none"),
                session.current_step + 1,
                self.config.groq.limits.max_actions_per_session,
                "",
            );

            messages.push(ChatMessage::user(context));

            let ai_request = AiProviderRequest {
                messages: messages.clone(),
                tools: ai::generate_all_tool_definitions(),
                system_prompt: Some(ai::build_system_prompt()),
            };

            match provider.chat(ai_request).await {
                Ok(response) => {
                    if response.finish_reason == "tool_calls" && !response.tool_calls.is_empty() {
                        messages.push(ChatMessage::assistant_tool_calls(
                            response.tool_calls.clone(),
                        ));
                    } else {
                        messages.push(ChatMessage::assistant_text(response.message.clone()));
                    }

                    if response.finish_reason == "tool_calls" && !response.tool_calls.is_empty() {
                        for tc in &response.tool_calls {
                            if ai::is_readonly_tool(&tc.name) {
                                println!("[AI] > {}", format_tool_call(&tc.name, &tc.arguments));
                                let result = self
                                    .execute_readonly_tool(&tc.name, &tc.arguments, &session)
                                    .await;
                                let result_str =
                                    serde_json::to_string(&result).unwrap_or_else(|_| "{}".into());
                                messages.push(ChatMessage::tool_result(tc.id.clone(), result_str));
                                continue;
                            }

                            match ai::tool_call_to_action(&tc.name, &tc.arguments) {
                                Ok(mut action) => {
                                    println!(
                                        "[AI] > {}",
                                        format_tool_call(&tc.name, &tc.arguments)
                                    );
                                    ai::bind_requester_intent(&mut action, &binding_request);
                                    let should_respond = !response.message.is_empty()
                                        && !session.responded_to_player
                                        && matches!(action, AiAction::Finish { .. });
                                    if should_respond {
                                        session.responded_to_player = true;
                                    }
                                    self.ai_session = Some(session);
                                    if should_respond {
                                        self.send_ai_chat_response(&response.message).await;
                                    }
                                    self.ai_request_in_flight = false;
                                    self.execute_ai_action(action).await;
                                    return;
                                }
                                Err(e) => {
                                    let error_json = serde_json::json!({
                                        "success": false,
                                        "error": format!("Tool call error: {e}")
                                    });
                                    let result_str = serde_json::to_string(&error_json)
                                        .unwrap_or_else(|_| r#"{"success":false}"#.into());
                                    messages
                                        .push(ChatMessage::tool_result(tc.id.clone(), result_str));
                                    logging::warning(format!("[AI] {e}"));
                                }
                            }
                        }
                        continue;
                    } else {
                        if !response.message.is_empty() && !session.responded_to_player {
                            session.responded_to_player = true;
                            println!("[AI] {}", response.message);
                            self.send_ai_chat_response(&response.message).await;
                        }
                        session.status = AiSessionStatus::Completed;
                        logging::info("[AI] Session completed");
                        self.ai_session = Some(session);
                        self.finalize_ai_task().await;
                        self.ai_request_in_flight = false;
                        return;
                    }
                }
                Err(e) => match &e {
                    AiProviderError::RateLimited(info) => {
                        let retry_after = info.retry_after.unwrap_or(Duration::from_secs(30)).min(
                            Duration::from_secs(
                                self.config.groq.rate_limits.maximum_retry_delay_seconds,
                            ),
                        );
                        let model = info
                            .model
                            .clone()
                            .unwrap_or_else(|| self.config.groq.model.clone());
                        let retry_at = Instant::now() + retry_after;
                        println!(
                            "[AI] Provider rate limited. Retrying at {:?} with model '{}'",
                            retry_at, model
                        );
                        session.status = AiSessionStatus::WaitingForProvider {
                            retry_at,
                            model: model.clone(),
                        };
                        self.ai_session = Some(session);
                        self.ai_request_in_flight = false;
                        logging::warning(format!(
                            "[AI] Provider rate limited, retrying in {:?}",
                            retry_after
                        ));
                        return;
                    }
                    AiProviderError::UnknownModel(m) => {
                        println!("[AI] Unknown model: {m}");
                        session.status = AiSessionStatus::Failed;
                        self.ai_session = Some(session);
                        self.finalize_ai_task().await;
                        self.ai_request_in_flight = false;
                        return;
                    }
                    AiProviderError::Permission(msg)
                    | AiProviderError::AuthenticationError(msg) => {
                        println!("[AI] Permission/Auth error: {msg}");
                        session.status = AiSessionStatus::Failed;
                        self.ai_session = Some(session);
                        self.finalize_ai_task().await;
                        self.ai_request_in_flight = false;
                        return;
                    }
                    _ => {
                        println!("[AI] Provider error: {e}");
                        session.status = AiSessionStatus::Failed;
                        self.ai_session = Some(session);
                        self.finalize_ai_task().await;
                        self.ai_request_in_flight = false;
                        return;
                    }
                },
            }
        }
    }

    async fn tick_ai_chat(&mut self) {
        if !self.config.ai.chat.enabled {
            return;
        }
        while let Some(chat) = self.minecraft.pop_incoming_player_chat().await {
            if chat.kind != crate::minecraft::world_state::ChatMessageKind::Player
                || (self.config.ai.chat.require_prefix
                    && !chat.text.starts_with(&self.config.ai.chat.prefix))
            {
                continue;
            }
            let Some(name) = chat.sender else {
                continue;
            };
            let requester = crate::ai::AiRequester {
                name,
                uuid: chat.sender_uuid,
                source: crate::ai::AiRequestSource::MinecraftChat,
            };
            let request = match crate::ai::chat_request(
                &chat.text,
                requester,
                &self.config.ai.chat.prefix,
                self.config.ai.chat.require_prefix,
                self.config.ai.chat.strip_prefix_whitespace,
                self.config.ai.chat.max_request_length,
            ) {
                Ok(request) => request,
                Err(error) => {
                    logging::warning(format!("[AI] Chat request rejected: {error}"));
                    continue;
                }
            };
            if matches!(
                request.text.to_ascii_lowercase().as_str(),
                "stop" | "cancel"
            ) {
                self.cancel_ai().await;
                self.send_ai_chat_response("Current task cancelled.").await;
                continue;
            }
            if !self.chat_access_allowed(
                request
                    .requester
                    .as_ref()
                    .expect("chat request has requester"),
            ) {
                logging::warning("[AI] Request rejected: access denied");
                self.send_ai_chat_response("You are not allowed to use this bot.")
                    .await;
                continue;
            }
            let text_lower = request.text.to_ascii_lowercase();
            let is_state_or_conversation = text_lower.starts_with("what")
                || text_lower.starts_with("how")
                || text_lower.starts_with("do you")
                || text_lower.starts_with("are you")
                || text_lower.contains("inventory")
                || text_lower.contains("health")
                || text_lower.contains("position")
                || text_lower.contains("status")
                || text_lower.contains("doing")
                || text_lower.contains("nearby")
                || text_lower.contains("hi")
                || text_lower.contains("hello")
                || text_lower.contains("hey");

            if self.can_accept_ai_request().is_err() && !is_state_or_conversation {
                logging::warning("[AI] Request rejected: bot busy");
                let reply = if let Some(s) = self.ai_session.as_ref() {
                    "I am already completing another task. Tell me to stop first, or ask what I'm doing.".to_owned()
                } else {
                    "I am already completing another task.".into()
                };
                self.send_ai_chat_response(&reply).await;
                continue;
            }
            if !self.consume_chat_rate_limit(
                request
                    .requester
                    .as_ref()
                    .expect("chat request has requester"),
            ) {
                logging::warning("[AI] Request rejected: rate limited");
                self.send_ai_chat_response("You are sending requests too quickly.")
                    .await;
                continue;
            }
            logging::info("[AI] Chat request received");
            if self.config.ai.chat.acknowledge_requests {
                self.send_ai_chat_response("Request received.").await;
            }
            self.submit_ai_request(request).await;
        }
    }

    async fn enforce_ai_limits(&mut self) {
        let timed_out = self.ai_session.as_ref().is_some_and(|session| {
            session.is_active()
                && session.started_at.elapsed()
                    >= Duration::from_secs(self.config.groq.limits.max_session_seconds)
        });
        if timed_out {
            if let Some(session) = self.ai_session.as_mut() {
                session.active_action = None;
                session.pending_action = None;
                session.generation = session.generation.wrapping_add(1);
                session.status = AiSessionStatus::TimedOut;
            }
            self.finalize_ai_task().await;
            self.ai_request_in_flight = false;
            logging::warning("[AI] Session timed out");
            self.send_ai_chat_response("Session timed out.").await;
        }
    }

    async fn tick_ai_provider(&mut self) {
        if self.ai_request_in_flight {
            return;
        }
        let Some(session) = self.ai_session.as_ref() else {
            return;
        };
        if !session.is_waiting_for_provider() {
            return;
        }
        let (should_retry, _model) = match &session.status {
            AiSessionStatus::WaitingForProvider { retry_at, model } => {
                (Instant::now() >= *retry_at, model.clone())
            }
            _ => (false, String::new()),
        };
        if !should_retry {
            return;
        }
        let _ = session;
        let Some(mut session) = self.ai_session.take() else {
            return;
        };
        session.status = AiSessionStatus::Interpreting;
        self.ai_session = Some(session);
        // Resume the existing session directly rather than routing through
        // `submit_ai_request`, which would build a brand-new session and
        // discard this one's action history / step count / generation.
        self.run_ai_turn(None).await;
    }

    /// Client-side reach check for entity actions. Azalea's `attack()`/
    /// `entity_interact()` send their packets with no range or
    /// line-of-sight validation of their own, so without this the bot
    /// would report false success for targets the server silently rejects.
    async fn entity_in_reach(&self, entity_id: i32) -> Result<(), String> {
        let world = self.minecraft.world_state_snapshot().await;
        let Some(entity) = world
            .entities
            .iter()
            .find(|e| i64::from(e.entity_id) == i64::from(entity_id))
        else {
            return Err(format!(
                "entity {entity_id} is not currently visible/tracked"
            ));
        };
        let reach = self.config.interaction.maximum_reach;
        if entity.distance > reach {
            return Err(format!(
                "entity {entity_id} is {:.1} blocks away, out of reach ({reach:.1})",
                entity.distance
            ));
        }
        Ok(())
    }

    /// Moves `count` items (0 = "as many as are there") from one player
    /// inventory slot to another, honoring the exact count rather than
    /// always moving the whole stack, and never silently swapping into a
    /// slot that holds a different item.
    ///
    /// Vanilla's click protocol has no "move exactly N" operation -- only
    /// whole-stack pickup/place (left-click) and place-one-at-a-time
    /// (right-click). A partial move therefore has to: pick up the whole
    /// source stack, right-click the destination `count` times, then
    /// left-click the (now empty) source to return whatever remains held.
    /// If the destination can't take everything placed at it (e.g. it hits
    /// its own max stack size), the click can leave an item stuck on the
    /// cursor rather than in any visible slot -- `carried_item`/recovery
    /// below exists specifically to catch and undo that rather than losing
    /// the item silently, which is the failure mode the old two-click
    /// implementation had.
    async fn move_inventory_item(
        &mut self,
        from_slot: u16,
        to_slot: u16,
        count: u16,
    ) -> CapabilityResult {
        use crate::container::model::{ClickButton, InventoryClick};

        if from_slot == to_slot {
            return CapabilityResult::failed(
                CapabilityStatus::InvalidTarget,
                "Source and destination slots are the same",
            );
        }

        let before = self.minecraft.world_state_snapshot().await.inventory;
        let from = from_slot as usize;
        let to = to_slot as usize;

        let Some(source) = before.slots.iter().find(|s| s.slot == from) else {
            return CapabilityResult::failed(
                CapabilityStatus::InvalidTarget,
                format!("Source slot {from_slot} does not exist"),
            );
        };
        let Some(source_item) = source.item_id.clone().filter(|_| source.count > 0) else {
            return CapabilityResult::failed(
                CapabilityStatus::MissingItem,
                format!("Source slot {from_slot} is empty"),
            );
        };
        let available = source.count;

        let dest_before = before.slots.iter().find(|s| s.slot == to);
        let Some(dest_before) = dest_before else {
            return CapabilityResult::failed(
                CapabilityStatus::InvalidTarget,
                format!("Destination slot {to_slot} does not exist"),
            );
        };
        if let Some(dest_item) = &dest_before.item_id
            && *dest_item != source_item
        {
            return CapabilityResult::failed(
                CapabilityStatus::InvalidTarget,
                format!(
                    "Destination slot {to_slot} already holds {dest_item}, not {source_item}; clear or pick a different slot first"
                ),
            );
        }
        let dest_before_count = dest_before.count;

        let move_count = if count == 0 {
            available
        } else {
            u32::from(count).min(available)
        };

        if let Err(e) = self
            .minecraft
            .container_click(
                0,
                InventoryClick {
                    slot: from,
                    button: ClickButton::Left,
                },
            )
            .await
        {
            return CapabilityResult::failed(
                CapabilityStatus::Failed,
                format!("Could not pick up item from slot {from_slot}: {e}"),
            );
        }

        let place_result = if move_count >= available {
            self.minecraft
                .container_click(
                    0,
                    InventoryClick {
                        slot: to,
                        button: ClickButton::Left,
                    },
                )
                .await
        } else {
            let mut result = Ok(());
            for _ in 0..move_count {
                result = self
                    .minecraft
                    .container_click(
                        0,
                        InventoryClick {
                            slot: to,
                            button: ClickButton::Right,
                        },
                    )
                    .await;
                if result.is_err() {
                    break;
                }
            }
            result
        };
        if let Err(e) = place_result {
            self.recover_carried_item(from).await;
            return CapabilityResult::failed(
                CapabilityStatus::Failed,
                format!("Could not place item into slot {to_slot}: {e}"),
            );
        }

        // Return any leftover (partial move, or destination couldn't take
        // everything) to the source rather than leaving it on the cursor.
        self.recover_carried_item(from).await;

        let after = self.minecraft.world_state_snapshot().await.inventory;
        let dest_after_count = after
            .slots
            .iter()
            .find(|s| s.slot == to)
            .map_or(0, |s| s.count);
        let actually_moved = dest_after_count.saturating_sub(dest_before_count);

        if actually_moved == 0 {
            return CapabilityResult::failed(
                CapabilityStatus::Failed,
                format!("Move had no effect; slot {to_slot} is unchanged"),
            );
        }
        if actually_moved < move_count {
            return CapabilityResult::completed(
                format!(
                    "Moved only {actually_moved} of the requested {move_count} {source_item} (destination likely hit its stack limit)"
                ),
                serde_json::json!({"from_slot": from_slot, "to_slot": to_slot, "requested": move_count, "moved": actually_moved}),
            );
        }
        CapabilityResult::completed(
            format!(
                "Moved {actually_moved} of {source_item} from slot {from_slot} to slot {to_slot}"
            ),
            serde_json::json!({"from_slot": from_slot, "to_slot": to_slot, "moved": actually_moved}),
        )
    }

    /// If a click sequence left an item stuck on the cursor, click the
    /// given slot to return it there instead of leaving it invisible (which
    /// also silently breaks later clicks/throws -- see `move_inventory_item`).
    async fn recover_carried_item(&self, return_slot: usize) {
        use crate::container::model::{ClickButton, InventoryClick};
        if matches!(self.minecraft.carried_item().await, Ok(Some(_))) {
            let _ = self
                .minecraft
                .container_click(
                    0,
                    InventoryClick {
                        slot: return_slot,
                        button: ClickButton::Left,
                    },
                )
                .await;
        }
    }

    async fn execute_ai_action(&mut self, action: AiAction) {
        if self.physical_action_active {
            println!("[AI] A physical action is already executing; rejecting new action.");
            let result = CapabilityResult::failed(
                CapabilityStatus::Failed,
                "A physical action is already active. Wait for it to complete or use cancel_action.",
            );
            if let Some(session) = self.ai_session.as_mut() {
                if let Some(id) = session.active_action.as_ref().map(|a| a.execution_id) {
                    session.complete_action(
                        id,
                        crate::ai::AiActionStatus::Failed,
                        serde_json::to_value(&result).unwrap_or_default(),
                    );
                }
                self.finalize_ai_task().await;
            }
            return;
        }

        let Some(session) = self.ai_session.as_ref() else {
            return;
        };
        if session.active_action.is_some() {
            println!("[AI] A previous action is still running; next action was not started.");
            return;
        }
        let current_step = session.current_step;
        if current_step >= self.config.groq.limits.max_actions_per_session {
            if let Some(session) = self.ai_session.as_mut() {
                session.status = AiSessionStatus::Failed;
            }
            println!("[AI] Action limit reached.");
            return;
        }
        let bot = self
            .minecraft
            .world_state_snapshot()
            .await
            .bot
            .position
            .unwrap_or_default();
        if let Err(e) =
            ai::validate_action(&action, &self.config.groq.limits, (bot.x, bot.y, bot.z))
        {
            println!("[AI] Rejected planner action: {e}");
            let result = CapabilityResult::failed(CapabilityStatus::InvalidTarget, e);
            if let Some(session) = self.ai_session.as_mut() {
                if let Some(id) = session.active_action.as_ref().map(|a| a.execution_id) {
                    session.complete_action(
                        id,
                        crate::ai::AiActionStatus::Failed,
                        serde_json::to_value(&result).unwrap_or_default(),
                    );
                }
                session.status = AiSessionStatus::Replanning;
            }
            return;
        }
        if let Some(session) = self.ai_session.as_mut() {
            session.current_step += 1;
            if let Err(error) = session.begin_action(action.clone()) {
                println!("[AI] {error}");
                return;
            }
        }
        // `tick_ai_action_completion` re-derives the execution id from
        // `session.active_action` once the result is known, so it is not
        // threaded through the pending-action dispatch below.
        logging::info(format!(
            "[AI] Executing action {}: {:?}",
            current_step + 1,
            action
        ));

        // Every action, fast or slow, is considered "pending" until
        // `tick_ai_action_completion` (driven by the interaction tick, every
        // ~50ms) observes a terminal result and calls `complete_ai_action`.
        // WalkTo/BreakBlock/PlaceBlock only *start* an azalea/interaction
        // state machine here -- their real result is only known once that
        // state machine reaches a terminal state on a later tick, so they
        // report `PendingAction::Movement`/`Interaction` instead of a result
        // computed at dispatch time. Every other action already completes
        // synchronously by the time control returns here.
        self.physical_action_active = true;

        match action.clone() {
            // --- Movement ---
            AiAction::WalkTo { x, y, z } => {
                let result = self
                    .tasks
                    .goto_position(
                        &self.minecraft,
                        &self.movement,
                        crate::minecraft::world_state::PositionSnapshot { x, y, z },
                        NavigationMode::MovementOnly,
                    )
                    .await;
                self.pending_action = Some(match result {
                    Ok(()) => PendingAction::Movement,
                    Err(e) => PendingAction::Instant(CapabilityResult::failed(
                        CapabilityStatus::Unreachable,
                        format!("Could not reach position: {e}"),
                    )),
                });
            }
            AiAction::FollowEntity { player } => {
                let result = self.movement.follow(&self.minecraft, &player).await;
                let capability_result = match result {
                    Ok(()) => CapabilityResult::completed(
                        format!("Following {player}"),
                        serde_json::json!({"following": player}),
                    ),
                    Err(e) => CapabilityResult::failed(
                        CapabilityStatus::TargetNotFound,
                        format!("Could not follow {player}: {e}"),
                    ),
                };
                self.pending_action = Some(PendingAction::Instant(capability_result));
            }
            AiAction::LookAt { x, y, z } => {
                let result = self
                    .tasks
                    .look_at(
                        &self.minecraft,
                        &self.look,
                        LookTarget::World(crate::minecraft::world_state::PositionSnapshot {
                            x,
                            y,
                            z,
                        }),
                    )
                    .await;
                self.pending_action = Some(match result {
                    // `look_at` only submits the target and applies one
                    // interpolation step; the turn isn't actually finished
                    // yet (see `PendingAction::Look`).
                    Ok(()) => PendingAction::Look,
                    Err(e) => PendingAction::Instant(CapabilityResult::failed(
                        CapabilityStatus::Failed,
                        format!("Could not look at target: {e}"),
                    )),
                });
            }
            AiAction::StopMovement => {
                let _ = self.movement.stop(&self.minecraft).await;
                self.pending_action = Some(PendingAction::Instant(CapabilityResult::completed(
                    "Movement stopped",
                    serde_json::json!({}),
                )));
            }
            // --- Block interaction ---
            AiAction::BreakBlock { x, y, z } => {
                let target = BlockPosition { x, y, z };
                let result = self
                    .tasks
                    .break_block(
                        &self.minecraft,
                        &self.movement,
                        &self.look,
                        &self.interaction,
                        &self.block_navigation,
                        target,
                    )
                    .await;
                self.pending_action = Some(match result {
                    Ok(()) => PendingAction::Interaction,
                    Err(e) => PendingAction::Instant(CapabilityResult::failed(
                        CapabilityStatus::Unreachable,
                        format!("Could not break block: {e}"),
                    )),
                });
            }
            AiAction::PlaceBlock { x, y, z, block_id } => {
                let target = BlockPosition { x, y, z };
                let result = self
                    .tasks
                    .place_block(
                        &self.minecraft,
                        &self.movement,
                        &self.look,
                        &self.interaction,
                        &self.block_navigation,
                        target,
                        block_id.clone(),
                    )
                    .await;
                self.pending_action = Some(match result {
                    Ok(()) => PendingAction::Interaction,
                    Err(e) => PendingAction::Instant(CapabilityResult::failed(
                        CapabilityStatus::Failed,
                        format!("Could not place block: {e}"),
                    )),
                });
            }
            AiAction::UseBlock { x, y, z } => {
                // `interact_block` sends a raw, unchecked-by-azalea use
                // packet (it can "click through walls" per its own doc
                // comment) -- without this check the bot would report
                // success for a block it can't actually reach, since the
                // server silently drops/ignores an out-of-range interact.
                let world = self.minecraft.world_state_snapshot().await;
                let target = BlockPosition { x, y, z };
                let capability_result = if !crate::interaction::reach::within_reach(
                    world.bot.position,
                    target,
                    self.config.interaction.maximum_reach,
                ) {
                    CapabilityResult::failed(
                        CapabilityStatus::OutOfRange,
                        "Block is out of reach; walk closer first",
                    )
                } else {
                    match self.minecraft.interact_block(target).await {
                        Ok(()) => CapabilityResult::completed(
                            "Block used",
                            serde_json::json!({"block": {"x": x, "y": y, "z": z}}),
                        ),
                        Err(e) => CapabilityResult::failed(
                            CapabilityStatus::Failed,
                            format!("Could not use block: {e}"),
                        ),
                    }
                };
                self.pending_action = Some(PendingAction::Instant(capability_result));
            }
            // --- Inventory ---
            AiAction::EquipItem { item_id } => {
                let capability_result = match self.minecraft.select_item_in_hotbar(&item_id).await {
                    Ok(true) => CapabilityResult::completed(
                        format!("Equipped {item_id}"),
                        serde_json::json!({"item_id": item_id}),
                    ),
                    Ok(false) => CapabilityResult::failed(
                        CapabilityStatus::MissingItem,
                        format!("{item_id} not found in hotbar"),
                    ),
                    Err(e) => CapabilityResult::failed(
                        CapabilityStatus::Failed,
                        format!("Failed to equip {item_id}: {e}"),
                    ),
                };
                self.pending_action = Some(PendingAction::Instant(capability_result));
            }
            AiAction::MoveInventoryItem {
                from_slot,
                to_slot,
                count,
            } => {
                let capability_result = self.move_inventory_item(from_slot, to_slot, count).await;
                self.pending_action = Some(PendingAction::Instant(capability_result));
            }
            AiAction::DropItem { item_id, count } => {
                let world = self.minecraft.world_state_snapshot().await;
                if let Some(nearest) = nearest_player(&world) {
                    // Fire-and-forget: only submits the target. The actual
                    // turn happens on later `look_tick`s, so the drop must
                    // wait for it via `tick_ai_action_completion` rather
                    // than blocking here.
                    let _ = self
                        .tasks
                        .look_at(
                            &self.minecraft,
                            &self.look,
                            LookTarget::Player(nearest.username.clone()),
                        )
                        .await;
                    self.pending_action = Some(PendingAction::LookThenDrop { item_id, count });
                } else {
                    let capability_result =
                        drop_item_result(self.minecraft.drop_item(&item_id, count).await, &item_id);
                    self.pending_action = Some(PendingAction::Instant(capability_result));
                }
            }
            AiAction::UseItem => {
                let result = self.minecraft.use_item_at_look_target().await;
                let capability_result = match result {
                    Ok(()) => CapabilityResult::completed("Item used", serde_json::json!({})),
                    Err(e) => CapabilityResult::failed(
                        CapabilityStatus::Failed,
                        format!("Could not use item: {e}"),
                    ),
                };
                self.pending_action = Some(PendingAction::Instant(capability_result));
            }
            // --- Crafting ---
            AiAction::CraftItem { item, quantity } => {
                let world = self.minecraft.world_state_snapshot().await;
                let plan = self
                    .recipes
                    .plan(&item, quantity, &world.inventory, false, 3);
                // NOTE: crafting execution (actually clicking a crafting grid)
                // is not implemented anywhere in this codebase yet -- only
                // planning is. Report *why* accurately instead of always
                // blaming a missing station, which sent the AI on pointless
                // "walk to a table" loops even when materials were on hand.
                let capability_result = match plan.failure {
                    None => CapabilityResult::failed(
                        CapabilityStatus::Failed,
                        format!(
                            "Materials for {item} are available, but crafting execution is not implemented yet. Do not retry craft_item for this item; tell the requester it cannot be crafted automatically right now."
                        ),
                    ),
                    Some(crate::crafting::Failure::StationUnavailable(_)) => {
                        CapabilityResult::failed(
                            CapabilityStatus::MissingItem,
                            format!(
                                "Cannot craft {item}: requires a crafting table, and crafting execution is not implemented yet regardless."
                            ),
                        )
                    }
                    Some(failure) => CapabilityResult::failed(
                        CapabilityStatus::MissingItem,
                        format!("Cannot craft {item}: {failure:?}"),
                    ),
                };
                self.pending_action = Some(PendingAction::Instant(capability_result));
            }
            // --- Containers ---
            AiAction::OpenContainer { x, y, z } => {
                let result = self
                    .container
                    .open(
                        &self.minecraft,
                        &self.movement,
                        &self.block_navigation,
                        BlockPosition { x, y, z },
                    )
                    .await;
                // `open()` only submits (navigate/align/interact/await-menu
                // is a multi-tick state machine driven by `ContainerService::tick`);
                // the menu isn't actually open yet when this returns `Ok(())`.
                self.pending_action = Some(match result {
                    Ok(()) => PendingAction::Container,
                    Err(e) => PendingAction::Instant(CapabilityResult::failed(
                        CapabilityStatus::Failed,
                        format!("Could not open container: {e}"),
                    )),
                });
            }
            AiAction::TakeItem { item, quantity } => {
                let result = self
                    .container
                    .transfer(
                        &self.minecraft,
                        TransferDirection::Take,
                        item.clone(),
                        quantity,
                    )
                    .await;
                // `transfer()` only queues clicks for `tick()` to drain
                // (75ms apart); nothing has actually moved yet.
                self.pending_action = Some(match result {
                    Ok(()) => PendingAction::Container,
                    Err(e) => PendingAction::Instant(CapabilityResult::failed(
                        CapabilityStatus::MissingItem,
                        format!("Could not take {item}: {e}"),
                    )),
                });
            }
            AiAction::StoreItem { item, quantity } => {
                let result = self
                    .container
                    .transfer(
                        &self.minecraft,
                        TransferDirection::Store,
                        item.clone(),
                        quantity,
                    )
                    .await;
                self.pending_action = Some(match result {
                    Ok(()) => PendingAction::Container,
                    Err(e) => PendingAction::Instant(CapabilityResult::failed(
                        CapabilityStatus::MissingItem,
                        format!("Could not store {item}: {e}"),
                    )),
                });
            }
            AiAction::CloseContainer => {
                self.container.close(&self.minecraft).await;
                self.pending_action = Some(PendingAction::Container);
            }
            // --- Entities / Combat ---
            AiAction::AttackEntity { entity_id } => {
                // Azalea's `attack()` performs no range/visibility check at
                // all (its own doc comment: "might trigger anticheats") --
                // without this, the bot would report a successful attack on
                // an entity 30 blocks away that the server silently drops.
                let capability_result = match self.entity_in_reach(entity_id).await {
                    Ok(()) => match self.minecraft.attack_entity(entity_id).await {
                        Ok(()) => CapabilityResult::completed(
                            format!("Attacked entity {entity_id}"),
                            serde_json::json!({"entity_id": entity_id}),
                        ),
                        Err(e) => CapabilityResult::failed(
                            CapabilityStatus::InvalidTarget,
                            format!("Could not attack entity: {e}"),
                        ),
                    },
                    Err(reason) => CapabilityResult::failed(CapabilityStatus::OutOfRange, reason),
                };
                self.pending_action = Some(PendingAction::Instant(capability_result));
            }
            AiAction::InteractWithEntity { entity_id } => {
                // Same missing-validation issue as AttackEntity: Azalea's
                // `entity_interact()` doc comment warns it "can click
                // through walls."
                let capability_result = match self.entity_in_reach(entity_id).await {
                    Ok(()) => match self.minecraft.interact_with_entity(entity_id).await {
                        Ok(()) => CapabilityResult::completed(
                            format!("Interacted with entity {entity_id}"),
                            serde_json::json!({"entity_id": entity_id}),
                        ),
                        Err(e) => CapabilityResult::failed(
                            CapabilityStatus::InvalidTarget,
                            format!("Could not interact with entity: {e}"),
                        ),
                    },
                    Err(reason) => CapabilityResult::failed(CapabilityStatus::OutOfRange, reason),
                };
                self.pending_action = Some(PendingAction::Instant(capability_result));
            }
            // --- Control ---
            AiAction::Wait { milliseconds } => {
                tokio::time::sleep(Duration::from_millis(milliseconds)).await;
                self.pending_action = Some(PendingAction::Instant(CapabilityResult::completed(
                    format!("Waited {milliseconds}ms"),
                    serde_json::json!({"waited_ms": milliseconds}),
                )));
            }
            AiAction::CancelAction => {
                self.pending_action = Some(PendingAction::Instant(CapabilityResult::completed(
                    "Action cancelled",
                    serde_json::json!({}),
                )));
            }
            AiAction::Finish { summary } => {
                if let Some(s) = summary {
                    println!("[AI] {s}");
                }
                self.pending_action = Some(PendingAction::Instant(CapabilityResult::completed(
                    "Task finished",
                    serde_json::json!({}),
                )));
            }
            AiAction::GetStatus => {
                self.print_status().await;
                self.pending_action = Some(PendingAction::Instant(CapabilityResult::completed(
                    "Status retrieved",
                    serde_json::json!({}),
                )));
            }
            AiAction::GetInventory => {
                self.print_inventory().await;
                self.pending_action = Some(PendingAction::Instant(CapabilityResult::completed(
                    "Inventory retrieved",
                    serde_json::json!({}),
                )));
            }
        }
    }
    /// Records an action's real-world result on the session, decides the
    /// session's next status, and -- if the session is still active -- feeds
    /// the result back to the model by re-entering `run_ai_turn`. This is
    /// the sole place that continues a multi-step AI plan; without it the
    /// session would sit in `Replanning` forever after the first action
    /// (see `PendingAction` / `execute_ai_action` for why completion is
    /// detected here rather than inline in `execute_ai_action`, which would
    /// require calling back into itself through `run_ai_turn` -- Rust does
    /// not allow that recursive `async fn` cycle without heap-boxing it).
    async fn complete_ai_action(
        &mut self,
        execution_id: Option<crate::ai::AiExecutionId>,
        result: CapabilityResult,
    ) {
        self.physical_action_active = false;
        self.pending_action = None;
        let summary = summarize_capability_result(&result);
        println!("[AI] < {summary}");
        let result_json = serde_json::to_value(&result).unwrap_or_default();
        let mut continue_session = false;
        if let Some(session) = self.ai_session.as_mut() {
            let completed_action = session.active_action.as_ref().map(|a| a.action.clone());
            if let Some(id) = execution_id {
                let status = if result.success {
                    crate::ai::AiActionStatus::Succeeded
                } else {
                    crate::ai::AiActionStatus::Failed
                };
                session.complete_action(id, status, result_json);
            }
            session.status = match completed_action {
                Some(AiAction::Finish { .. }) => AiSessionStatus::Completed,
                Some(AiAction::CancelAction) => AiSessionStatus::Cancelled,
                // `complete_action` already set this to `Replanning`.
                _ => session.status.clone(),
            };
            continue_session = session.is_active();
        }
        self.ai_request_in_flight = false;
        if !continue_session {
            self.finalize_ai_task().await;
            return;
        }
        self.run_ai_turn(Some(summary)).await;
    }

    /// Runs on the interaction tick cadence (~50ms). Checks whether the
    /// currently pending action has reached a terminal, real-world result
    /// yet, and if so hands it to `complete_ai_action`.
    async fn tick_ai_action_completion(&mut self) {
        if !self.physical_action_active {
            return;
        }
        let Some(kind) = self.pending_action.clone() else {
            self.physical_action_active = false;
            return;
        };
        let Some(session) = self.ai_session.as_ref() else {
            self.physical_action_active = false;
            self.pending_action = None;
            return;
        };
        let Some(active) = session.active_action.as_ref() else {
            self.physical_action_active = false;
            self.pending_action = None;
            return;
        };
        let execution_id = active.execution_id;
        let action = active.action.clone();
        let started_at = active.started_at;

        const MOVEMENT_TIMEOUT: Duration = Duration::from_secs(45);
        const INTERACTION_TIMEOUT: Duration = Duration::from_secs(30);
        const LOOK_TIMEOUT: Duration = Duration::from_millis(1200);
        const CONTAINER_TIMEOUT: Duration = Duration::from_secs(8);

        let result = match kind {
            PendingAction::Instant(result) => Some(result),
            PendingAction::Movement => {
                let snapshot = self.movement.snapshot().await;
                match snapshot.status {
                    MovementStatus::Completed => Some(CapabilityResult::completed(
                        "Reached the target position",
                        walk_to_result_data(&action),
                    )),
                    MovementStatus::Failed | MovementStatus::Cancelled => {
                        Some(CapabilityResult::failed(
                            CapabilityStatus::Unreachable,
                            snapshot
                                .failure_reason
                                .unwrap_or_else(|| "movement failed".into()),
                        ))
                    }
                    _ if started_at.elapsed() > MOVEMENT_TIMEOUT => Some(CapabilityResult::failed(
                        CapabilityStatus::TimedOut,
                        "navigation timed out before reaching the target",
                    )),
                    _ => None,
                }
            }
            PendingAction::Interaction => {
                let snapshot = self.interaction.snapshot().await;
                match snapshot.state {
                    InteractionState::Completed => {
                        let (message, data) = interaction_result_data(&action);
                        Some(CapabilityResult::completed(message, data))
                    }
                    InteractionState::Failed | InteractionState::Cancelled => {
                        Some(CapabilityResult::failed(
                            CapabilityStatus::Failed,
                            snapshot
                                .failure_reason
                                .unwrap_or_else(|| "interaction failed".into()),
                        ))
                    }
                    _ if started_at.elapsed() > INTERACTION_TIMEOUT => {
                        Some(CapabilityResult::failed(
                            CapabilityStatus::TimedOut,
                            "interaction timed out",
                        ))
                    }
                    _ => None,
                }
            }
            PendingAction::LookThenDrop { item_id, count } => {
                let snapshot = self.look.snapshot().await;
                let ready = matches!(
                    snapshot.state,
                    LookState::Completed | LookState::Failed | LookState::Cancelled
                ) || started_at.elapsed() > LOOK_TIMEOUT;
                if ready {
                    let drop_result = self.minecraft.drop_item(&item_id, count).await;
                    Some(drop_item_result(drop_result, &item_id))
                } else {
                    // Still turning; check again on the next tick.
                    None
                }
            }
            PendingAction::Look => {
                let snapshot = self.look.snapshot().await;
                match snapshot.state {
                    LookState::Completed => Some(CapabilityResult::completed(
                        "Looked at target",
                        look_at_result_data(&action),
                    )),
                    LookState::Failed | LookState::Cancelled => Some(CapabilityResult::failed(
                        CapabilityStatus::Failed,
                        snapshot
                            .failure_reason
                            .unwrap_or_else(|| "look failed".into()),
                    )),
                    _ if started_at.elapsed() > LOOK_TIMEOUT => Some(CapabilityResult::failed(
                        CapabilityStatus::TimedOut,
                        "turning toward the target timed out",
                    )),
                    _ => None,
                }
            }
            PendingAction::Container => {
                let status = self.container.status().await;
                use crate::container::model::ContainerPhase;
                match status.phase {
                    ContainerPhase::Open | ContainerPhase::Completed => {
                        Some(container_result(&status, &action))
                    }
                    ContainerPhase::Failed | ContainerPhase::Cancelled => {
                        Some(container_result(&status, &action))
                    }
                    _ if started_at.elapsed() > CONTAINER_TIMEOUT => {
                        Some(CapabilityResult::failed(
                            CapabilityStatus::TimedOut,
                            "container action timed out",
                        ))
                    }
                    _ => None,
                }
            }
        };
        let Some(result) = result else {
            // Still in progress; check again on the next tick.
            return;
        };
        self.complete_ai_action(Some(execution_id), result).await;
    }
    async fn cancel_ai(&mut self) {
        if let Some(s) = self.ai_session.as_mut() {
            s.cancel_active_action();
        }
        self.physical_action_active = false;
        self.pending_action = None;
        self.ai_request_in_flight = false;
        self.finalize_ai_task().await;
    }

    /// Central cleanup for all AI task terminal states.
    async fn finalize_ai_task(&mut self) {
        self.physical_action_active = false;
        self.pending_action = None;
        self.interaction
            .cancel(&self.minecraft, &self.movement, &self.look)
            .await;
        self.block_navigation
            .cancel(&self.minecraft, &self.movement)
            .await;
        let _ = self.movement.stop(&self.minecraft).await;
        self.tasks.cancel(&self.minecraft).await;
    }

    fn print_ai_status(&self) {
        match &self.ai_session {
            Some(s) => println!(
                "AI session: {:?}\nRequester: {}\nObjective: {:?}\nStep: {}/{}\nElapsed: {} seconds",
                s.status,
                s.requester.as_deref().unwrap_or("unknown"),
                s.objective,
                s.current_step,
                s.max_steps,
                s.started_at.elapsed().as_secs()
            ),
            None => println!("No AI session is active."),
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
        let vertical = self.vertical_construction.status();
        if vertical.state
            != crate::navigation::vertical_construction::VerticalConstructionState::Idle
        {
            println!(
                "Vertical construction: {:?}{}{}",
                vertical.state,
                vertical
                    .last_action
                    .map_or_else(String::new, |a| format!(" (last: {a})")),
                vertical
                    .failure_reason
                    .map_or_else(String::new, |r| format!(" ({r})")),
            );
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
        println!(
            "Workflow: #{} {} ({:?})",
            workflow.metadata.id, workflow.metadata.task_type, workflow.state
        );
        if !workflow.progress.phase.is_empty() {
            println!(
                "Progress: {} {}/{}",
                workflow.progress.phase,
                workflow.progress.completed_units,
                workflow
                    .progress
                    .total_units
                    .map_or_else(|| "?".into(), |n| n.to_string())
            );
        }
        println!(
            "Active tasks: {}; recent tasks: {}",
            self.tasks.active().len(),
            self.tasks.recent().len()
        );
    }

    fn print_task_by_id(&self, id: TaskId) {
        match self.tasks.query(id) {
            Some(task) => {
                println!("Task #{} {}: {:?}", id, task.metadata.task_type, task.state);
                println!(
                    "Source: {}; correlation: {}",
                    task.metadata.source_command, task.metadata.correlation_id
                );
                println!(
                    "Progress: {} {}/{}",
                    task.progress.phase,
                    task.progress.completed_units,
                    task.progress
                        .total_units
                        .map_or_else(|| "?".into(), |n| n.to_string())
                );
                if let Some(failure) = task.failure {
                    println!(
                        "Failure: {:?}: {} (partial {})",
                        failure.category, failure.message, failure.partial_completed
                    );
                }
            }
            None => println!("Unknown task #{id}."),
        }
    }

    fn print_recent_tasks(&self) {
        let recent = self.tasks.recent();
        if recent.is_empty() {
            println!("No recently completed tasks.");
            return;
        }
        for task in recent {
            println!(
                "#{} {} {:?} {}/{}",
                task.metadata.id,
                task.metadata.task_type,
                task.state,
                task.progress.completed_units,
                task.progress
                    .total_units
                    .map_or_else(|| "?".into(), |n| n.to_string())
            );
        }
    }
}

fn fmt_opt<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "unknown".into(), |v| v.to_string())
}

/// Builds the AI-facing result for a `drop_item` call from the raw client
/// result, shared by the immediate (no nearby player) and deferred
/// (look-then-drop) dispatch paths.
fn drop_item_result(result: Result<u16, AppError>, item_id: &str) -> CapabilityResult {
    match result {
        Ok(dropped) => CapabilityResult::completed(
            format!("Dropped {dropped} of {item_id}"),
            serde_json::json!({"item_id": item_id, "count": dropped}),
        ),
        Err(e) => CapabilityResult::failed(
            CapabilityStatus::MissingItem,
            format!("Could not drop {item_id}: {e}"),
        ),
    }
}

/// Closest tracked player with a known distance, if any -- used to face the
/// bot toward someone before an interaction like dropping an item.
fn nearest_player(world: &WorldStateSnapshot) -> Option<&PlayerSnapshot> {
    world
        .players
        .iter()
        .filter(|p| p.distance.is_some())
        .min_by(|a, b| a.distance.unwrap().total_cmp(&b.distance.unwrap()))
}

/// Renders a model tool call as `name(arg=value, ...)` for the console, so
/// every command the AI decides to run -- read-only lookups and physical
/// actions alike -- is visible as it happens, not just in verbose logs.
fn format_tool_call(name: &str, arguments: &serde_json::Value) -> String {
    let args = arguments
        .as_object()
        .filter(|obj| !obj.is_empty())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    format!("{name}({args})")
}

/// Human-readable text fed back to the model as "Previous action result"
/// once a dispatched action reaches a terminal, real-world outcome.
fn summarize_capability_result(result: &CapabilityResult) -> String {
    if result.success {
        result.message.clone()
    } else {
        format!("FAILED ({:?}): {}", result.status, result.message)
    }
}

/// Rebuilds `walk_to`'s result payload from the stored action once real
/// movement completion is observed on a later tick (see `PendingAction`).
fn walk_to_result_data(action: &AiAction) -> serde_json::Value {
    match action {
        AiAction::WalkTo { x, y, z } => serde_json::json!({"position": {"x": x, "y": y, "z": z}}),
        _ => serde_json::json!({}),
    }
}

/// Rebuilds `look_at`'s result payload from the stored action once the real
/// turn completes (see `PendingAction::Look`).
fn look_at_result_data(action: &AiAction) -> serde_json::Value {
    match action {
        AiAction::LookAt { x, y, z } => serde_json::json!({"target": {"x": x, "y": y, "z": z}}),
        _ => serde_json::json!({}),
    }
}

/// Builds the AI-facing result for a container action (open/take/store)
/// once `ContainerService`'s state machine reaches a terminal phase, using
/// the server-confirmed `status.transferred` amount rather than the
/// originally requested quantity -- a partial transfer (e.g. destination
/// ran out of space) must not be reported as if the full amount moved.
fn container_result(
    status: &crate::container::model::ContainerStatus,
    action: &AiAction,
) -> CapabilityResult {
    use crate::container::model::{ContainerOutcome, ContainerPhase};
    let failed = |message: String| CapabilityResult::failed(CapabilityStatus::Failed, message);
    match action {
        AiAction::OpenContainer { x, y, z } => match status.phase {
            ContainerPhase::Open => CapabilityResult::completed(
                "Container opened",
                serde_json::json!({"container": {"x": x, "y": y, "z": z}}),
            ),
            _ => failed(format!(
                "Could not open container: {}",
                status
                    .detail
                    .clone()
                    .or_else(|| status.outcome.as_ref().map(|o| format!("{o:?}")))
                    .unwrap_or_else(|| "unknown failure".into())
            )),
        },
        AiAction::TakeItem { item, .. } | AiAction::StoreItem { item, .. } => {
            let verb = if matches!(action, AiAction::TakeItem { .. }) {
                "Took"
            } else {
                "Stored"
            };
            match status.outcome {
                Some(ContainerOutcome::TransferCompleted) => CapabilityResult::completed(
                    format!("{verb} {} of {item}", status.transferred),
                    serde_json::json!({"item": item, "requested": status.requested, "transferred": status.transferred}),
                ),
                Some(ContainerOutcome::TransferPartial) => CapabilityResult::completed(
                    format!(
                        "{verb} only {} of the requested {} {item} (destination or source limited it)",
                        status.transferred, status.requested
                    ),
                    serde_json::json!({"item": item, "requested": status.requested, "transferred": status.transferred, "partial": true}),
                ),
                _ => failed(format!(
                    "Could not transfer {item}: {}",
                    status
                        .detail
                        .clone()
                        .or_else(|| status.outcome.as_ref().map(|o| format!("{o:?}")))
                        .unwrap_or_else(|| "unknown failure".into())
                )),
            }
        }
        AiAction::CloseContainer => match status.phase {
            ContainerPhase::Completed | ContainerPhase::Idle => {
                CapabilityResult::completed("Container closed", serde_json::json!({}))
            }
            _ => failed("Could not close container".into()),
        },
        _ => failed("unexpected action for container completion".into()),
    }
}

/// Rebuilds `break_block`/`place_block`'s success message and result
/// payload from the stored action once real interaction completion is
/// observed on a later tick (see `PendingAction`).
fn interaction_result_data(action: &AiAction) -> (String, serde_json::Value) {
    match action {
        AiAction::BreakBlock { x, y, z } => (
            "Block broken".into(),
            serde_json::json!({"block": {"x": x, "y": y, "z": z}}),
        ),
        AiAction::PlaceBlock { x, y, z, block_id } => (
            format!("Placed {block_id}"),
            serde_json::json!({"block": {"x": x, "y": y, "z": z, "block_id": block_id}}),
        ),
        _ => ("Interaction completed".into(), serde_json::json!({})),
    }
}

fn ai_chat_chunks(message: &str, maximum: usize) -> Vec<String> {
    if maximum == 0 {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    for character in message.chars() {
        if chunk.chars().count() == maximum {
            chunks.push(std::mem::take(&mut chunk));
        }
        chunk.push(character);
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    chunks
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
    println!("  /task                      Show task status, or /task status <id>");
    println!("  /tasks                     Show all task status");
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
    println!("AI");
    println!("  /ai <message>              Send a request to the configured AI provider");
    println!();
    println!("Other");
    println!("  /help                      Show this message");
    println!("  /status                    Show full connection/application status");
    println!("  /chat <text>               Send text to Minecraft chat");
    println!("  /aicancel | /aistatus      Cancel or inspect the active AI session");
    println!("  /task cancel <id|all>      Cancel a task");
    println!("  /reconnect                 Reconnect to the configured server");
    println!("  /quit                      Shut down the application");
    println!(
        "  Additional lower-level debug commands (findblock, gotoblock, lookblock, breaknearest, placeblock, ...) remain available; see src/console/commands.rs for the complete set."
    );
}

async fn await_console_task(task: JoinHandle<()>) {
    let _ = task.await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::block_snapshot::BlockSnapshot;

    fn log(x: i32, y: i32, z: i32) -> BlockSnapshot {
        BlockSnapshot {
            position: BlockPosition { x, y, z },
            block_id: "minecraft:oak_log".into(),
            distance: 0.0,
            dimension: Some("minecraft:overworld".into()),
            chunk_loaded: true,
        }
    }

    #[test]
    fn long_ai_chat_message_is_split_on_utf8_boundaries() {
        let message = format!("[AI] {} 😀", "x".repeat(300));
        let chunks = ai_chat_chunks(&message, 64);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 64));
        assert_eq!(chunks.concat().replace(' ', ""), message.replace(' ', ""));
    }

    #[test]
    fn empty_ai_chat_message_has_no_chunks() {
        assert!(ai_chat_chunks("", 256).is_empty());
    }
}
