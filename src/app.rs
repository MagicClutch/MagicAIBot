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
    crafting::RecipeBook,
    crafting::CraftService,
    container::{model::TransferDirection, service::ContainerService},
    error::AppError,
    food::{CollectFoodRequest, FoodCollector, FoodGoal},
    interaction::{InteractionController, interaction_controller::InteractionState},
    logging,
    look::{LookController, LookTarget, look_controller::LookState},
    minecraft::client::MinecraftClient,
    movement::{MovementService, NavigationMode},
    navigation::{BlockNavigationService, navigation_state::BlockNavigationState},
    tasks::{CancellationReason, GatherRequest, TaskId, TaskService},
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
    recipes: RecipeBook,
    crafting: CraftService,
    container: ContainerService,
    food_collector: FoodCollector,
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
            recipes: RecipeBook::fallback().map_err(AppError::RecipeData)?,
            crafting: CraftService::default(),
            container: ContainerService::default(),
            food_collector: FoodCollector::default(),
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
                            self.tasks.disconnected();
                            self.minecraft.clear_current_task().await;
                        }
                        continue;
                    }
                    self.session_ready = true;
                    if self.minecraft.world_state_snapshot().await.bot.alive == Some(false) {
                        self.tasks.player_died();
                    }
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
                _ = interaction_tick.tick() => {
                    self.interaction.tick(&self.minecraft, &self.movement, &self.look).await;
                    self.container.tick(&self.minecraft, &self.movement, &self.block_navigation, &self.look).await;
                    self.food_collector.tick(&self.minecraft, &self.movement, &self.interaction, &self.look).await;
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
                ConsoleCommand::Help => print_help(),
                ConsoleCommand::Status => self.print_status().await,
                ConsoleCommand::Chat { message } => {
                    if let Err(error) = self.minecraft.send_chat(&message).await {
                        println!("Chat error: {error}");
                    }
                }
                ConsoleCommand::Players => self.print_players().await,
                ConsoleCommand::Inventory => self.print_inventory().await,
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
                ConsoleCommand::Gather {
                    resource,
                    quantity,
                    deposit,
                } => {
                    println!(
                        "Gather request: {resource} x{quantity}{}",
                        if deposit { " (deposit enabled)" } else { "" }
                    );
                    println!(
                        "Gather not started: this build has no live Phase 2/3 crafting, storage, pickup, or smelting adapters; no world action was attempted."
                    );
                }
                ConsoleCommand::GatherStatus => self.print_task_status().await,
                ConsoleCommand::GatherCancel => {
                    self.interaction
                        .cancel(&self.minecraft, &self.movement, &self.look)
                        .await;
                    self.block_navigation
                        .cancel(&self.minecraft, &self.movement)
                        .await;
                    let _ = self.movement.stop(&self.minecraft).await;
                    self.tasks.cancel(&self.minecraft).await;
                    println!(
                        "Gather cancellation requested; movement and interaction stopped; confirmed partial inventory remains unchanged."
                    );
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
                ConsoleCommand::Gather { target, quantity } => {
                    let request = match target.as_str() {
                        "logs" | "log" => GatherRequest::Logs { quantity },
                        "stone" => GatherRequest::Stone { quantity },
                        "ores" | "ore" => GatherRequest::VisibleOres { quantity },
                        "food" => GatherRequest::Food { quantity },
                        item => GatherRequest::Items(crate::tasks::CollectItemsRequest {
                            item_ids: vec![if item.contains(':') {
                                item.to_owned()
                            } else {
                                format!("minecraft:{item}")
                            }],
                            quantity,
                            maximum_attempts: 32,
                        }),
                    };
                    match self
                        .tasks
                        .gather_visible_blocks(
                            &self.minecraft,
                            &self.movement,
                            &self.look,
                            &self.interaction,
                            request,
                        )
                        .await
                    {
                        Ok(result) => println!(
                            "Gathered {}/{} in {} attempts.",
                            result.collected, result.requested, result.attempts
                        ),
                        Err(error) => logging::warning(format!("Gather failed: {error}")),
                    }
                }
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
                ConsoleCommand::Craft { target, count } => println!(
                    "Craft request {target} x{count} rejected: this debug surface requires RecipeKnowledge to submit a resolved plan; it will not guess recipes or gather materials."
                ),
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
                ConsoleCommand::CollectFood {
                    item,
                    count,
                    food_value,
                } => {
                    let goal = if food_value {
                        FoodGoal::FoodValue(count)
                    } else {
                        FoodGoal::Count { item, count }
                    };
                    match self
                        .food_collector
                        .start(
                            CollectFoodRequest {
                                goal,
                                ..Default::default()
                            },
                            &self.minecraft,
                            &self.movement,
                            &self.block_search,
                        )
                        .await
                    {
                        Ok(()) => println!("Food collection started."),
                        Err(result) => println!(
                            "Food collection not started: {:?} ({})",
                            result.outcome,
                            result.detail.as_deref().unwrap_or("no detail")
                        ),
                    }
                }
                ConsoleCommand::CollectFoodStatus => {
                    let status = self.food_collector.status();
                    println!(
                        "Food collection: {} ({})",
                        if status.active { "active" } else { "idle" },
                        status.phase
                    );
                    if let Some(result) = status.result {
                        println!(
                            "Last result: {:?}; count {}, food value {}, sources {}",
                            result.outcome,
                            result.collected_count,
                            result.collected_food_value,
                            result.sources_attempted
                        );
                    }
                }
                ConsoleCommand::CollectFoodStop => {
                    self.food_collector
                        .stop(
                            &self.minecraft,
                            &self.movement,
                            &self.interaction,
                            &self.look,
                        )
                        .await;
                    println!("Food collection stopped.");
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

fn print_help() {
    println!("Local-console commands only. Success requires observed state confirmation.");
    println!("Cancellation: /stopinteraction, /stop, or /stopall (in increasing scope).");
    println!("/help       Show available commands");
    println!("/open-chest X Y Z | /take-item ITEM COUNT | /store-item ITEM COUNT");
    println!("/container-status | /close-container");
    println!("/status     Show connection and application status");
    println!("/chat TEXT  Send TEXT to Minecraft chat");
    println!("/players    Show known online players");
    println!("/inventory  Show inventory summary");
    println!("/recipe ID  Show a versioned read-only recipe");
    println!("/craft-check ITEM [COUNT] [DEPTH]  Plan with a virtual inventory");
    println!("/collect-food [ITEM COUNT|value POINTS]  Collect safe observable food");
    println!("/collect-food-status  Show collector status");
    println!("/collect-food-stop  Cancel food collection");
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
    println!("/gather RESOURCE QUANTITY [deposit]  Gather a bounded supported resource quantity");
    println!("/gatherstatus  Show gather/task progress and last failure");
    println!("/gathercancel  Cancel gathering and preserve confirmed partial progress");
    println!("/tasks      Show runtime summary");
    println!("/task status ID | /task cancel ID|all | /task recent");
    println!("/gather TARGET QUANTITY  Gather an item, logs, stone, visible ores, or food");
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
    println!("/select-tool ID  DEBUG: score the hotbar for a block, explain and select the winner");
    println!("/place ID  Place the held block beside the crosshair target");
    println!("/place X Y Z ID  Place a block at coordinates");
    println!("/placeblock ID [X Y Z]  Place through the reusable placement workflow");
    println!("/stopinteraction  Cancel block interaction");
    println!("/interactionstatus  Show interaction status");
    println!("/craft ITEM COUNT  Submit a crafting debug request (requires a resolved plan)");
    println!("/craft status | /craft stop  Inspect or cancel crafting");
    println!("/ensure-tool ID  Debug an ensure-tool request (/craft-tool is an alias)");
    println!("/testoaklog  Break and restore the nearest oak log");
    println!("/reconnect  Reconnect to the configured server");
    println!("/quit       Shut down the application");
}

async fn await_console_task(task: JoinHandle<()>) {
    let _ = task.await;
}
