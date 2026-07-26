//! The application's boundary around Azalea's Minecraft client.
//!
//! Azalea types are deliberately kept in this module. Callers observe only
//! our connection state and application errors.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use azalea::core::position::ChunkPos;
use azalea::world::find_blocks::find_blocks_in_chunk;
use azalea::{
    Client, Event,
    account::{Account, microsoft::MicrosoftAccountOpts},
    auto_reconnect::AutoReconnectDelay,
    block::BlockStates,
    core::entity_id::MinecraftEntityId,
    entity::{
        Dead, EntityKindComponent, EntityUuid, LocalEntity, LookDirection, Physics, Position,
        dimensions::EntityDimensions,
        inventory::Inventory,
        metadata::{Health, ItemItem},
    },
    pathfinder::{
        PathfinderClientExt, PathfinderOpts,
        goals::{BlockPosGoal, RadiusGoal},
    },
    player::GameProfileComponent,
    registry::builtin::BlockKind,
    world::WorldName,
};
use azalea_inventory::components::{Damage, Enchantments, MaxDamage, Tool};
use tokio::{
    sync::{Mutex, watch},
    task::JoinHandle,
    time::{sleep, timeout},
};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::{
    blocks::{
        block_query::{BlockSearchQuery, chunk_coordinate},
        block_snapshot::LoadedBlockCandidate,
    },
    config::{AccountMode, ConsoleConfig, MinecraftConfig, ReconnectConfig, WorldStateConfig},
    error::AppError,
    logging,
    minecraft::{
        events::handle_chat,
        inventory_actions::InventoryActionService,
        world_state::{
            BlockPosition, InventorySlot, InventorySnapshot, MovementSnapshot, PlayerSnapshot,
            PositionSnapshot, TaskSnapshot, WorldConnectionState, WorldState, WorldStateSnapshot,
        },
    },
    movement::NavigationMode,
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct HitboxSnapshot {
    pub position: [f64; 3],
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LookedBlockSnapshot {
    pub position: BlockPosition,
    pub face: RaycastFace,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RaycastFace {
    Up,
    Down,
    North,
    South,
    West,
    East,
}

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CHAT_MESSAGE_LENGTH: usize = 256;

enum DisconnectReason {
    Kicked(String),
    ConnectionFailure(String),
}

/// The lifecycle state of the Minecraft connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Authenticating,
    JoiningWorld,
    Connected,
    Reconnecting,
}

/// Owns the Azalea client and supervises its connection lifecycle.
pub struct MinecraftClient {
    minecraft: MinecraftConfig,
    reconnect: ReconnectConfig,
    console: ConsoleConfig,
    state: watch::Receiver<ConnectionState>,
    state_tx: watch::Sender<ConnectionState>,
    current_client: Arc<Mutex<Option<Client>>>,
    world_state: Arc<Mutex<WorldState>>,
    shutdown: CancellationToken,
    supervisor: Option<JoinHandle<()>>,
    inventory_actions: InventoryActionService,
}

#[derive(Clone, Debug)]
pub struct MinecraftStatus {
    pub connection_state: ConnectionState,
    pub server: String,
    pub username: String,
    pub account_mode: String,
    pub reconnect: ReconnectConfig,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NavigationStatus {
    pub calculating: bool,
    pub executing: bool,
    pub reached: bool,
}

#[derive(Clone, Copy, Debug)]
struct SearchCandidate {
    position: BlockPosition,
    distance_squared: f64,
}

impl PartialEq for SearchCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.distance_squared == other.distance_squared && self.position == other.position
    }
}
impl Eq for SearchCandidate {}
impl PartialOrd for SearchCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for SearchCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.distance_squared
            .total_cmp(&other.distance_squared)
            .then_with(|| {
                (self.position.x, self.position.y, self.position.z).cmp(&(
                    other.position.x,
                    other.position.y,
                    other.position.z,
                ))
            })
    }
}

fn block_search_cancelled(state: ConnectionState) -> bool {
    state != ConnectionState::Connected
}

struct SupervisorContext {
    minecraft: MinecraftConfig,
    reconnect: ReconnectConfig,
    current_client: Arc<Mutex<Option<Client>>>,
    world_state: Arc<Mutex<WorldState>>,
    console: ConsoleConfig,
    state_tx: watch::Sender<ConnectionState>,
    shutdown: CancellationToken,
}

impl MinecraftClient {
    pub fn new(
        minecraft: MinecraftConfig,
        reconnect: ReconnectConfig,
        console: ConsoleConfig,
        world_config: WorldStateConfig,
    ) -> Self {
        let (state_tx, state) = watch::channel(ConnectionState::Disconnected);
        let mut world = WorldState::with_limits(
            world_config.nearby_entity_radius,
            world_config.maximum_tracked_entities,
            world_config.stale_entity_seconds,
        )
        .expect("validated world-state configuration");
        world.set_connection_info(
            minecraft.server.clone(),
            minecraft.username.clone(),
            match minecraft.account_mode {
                AccountMode::Offline => "offline".into(),
                AccountMode::Microsoft => "microsoft".into(),
            },
        );
        Self {
            minecraft,
            reconnect,
            console,
            state,
            state_tx,
            current_client: Arc::new(Mutex::new(None)),
            world_state: Arc::new(Mutex::new(world)),
            shutdown: CancellationToken::new(),
            supervisor: None,
            inventory_actions: InventoryActionService::default(),
        }
    }

    /// Connects and waits until Azalea reports that the bot has spawned.
    pub async fn connect(&mut self) -> Result<(), AppError> {
        if self.supervisor.is_some() {
            return Err(AppError::ConnectionFailure(
                "Minecraft client is already running".to_owned(),
            ));
        }

        self.set_state(ConnectionState::Connecting);

        let (client, events) = self.join_once().await?;
        self.disable_azalea_reconnect(&client);
        *self.current_client.lock().await = Some(client.clone());

        self.set_state(ConnectionState::JoiningWorld);
        let supervisor = tokio::task::spawn_local(supervise(
            events,
            SupervisorContext {
                minecraft: self.minecraft.clone(),
                reconnect: self.reconnect.clone(),
                current_client: self.current_client.clone(),
                world_state: self.world_state.clone(),
                console: self.console.clone(),
                state_tx: self.state_tx.clone(),
                shutdown: self.shutdown.clone(),
            },
        ));
        self.supervisor = Some(supervisor);

        let mut state = self.state.clone();
        let result = timeout(CONNECTION_TIMEOUT, async {
            loop {
                let current_state = *state.borrow();
                match current_state {
                    ConnectionState::Connected => return Ok(()),
                    ConnectionState::Disconnected => {
                        return Err(AppError::ConnectionFailure(
                            "connection closed before the bot joined the world".to_owned(),
                        ));
                    }
                    _ => state.changed().await.map_err(|_| {
                        AppError::ConnectionFailure("connection supervisor stopped".to_owned())
                    })?,
                }
            }
        })
        .await;

        match result {
            Ok(result) => result,
            Err(_) => Err(AppError::NetworkTimeout {
                seconds: CONNECTION_TIMEOUT.as_secs(),
            }),
        }
    }

    /// Disconnects the current Azalea client and stops supervision.
    pub async fn disconnect(&mut self) -> Result<(), AppError> {
        self.world_state
            .lock()
            .await
            .set_connection(WorldConnectionState::ShuttingDown);
        self.shutdown.cancel();
        if let Some(client) = self.current_client.lock().await.take() {
            client.disconnect();
        }
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.await;
        }
        let mut world = self.world_state.lock().await;
        world.set_joined_world(false);
        world.set_connection(WorldConnectionState::Disconnected);
        world.clear_session_state();
        self.set_state(ConnectionState::Disconnected);
        logging::info("Disconnected");
        Ok(())
    }

    /// Stops the current connection and establishes a fresh one.
    pub async fn reconnect(&mut self) -> Result<(), AppError> {
        if self.connection_state() == ConnectionState::Reconnecting {
            return Err(AppError::ReconnectAlreadyInProgress);
        }
        self.disconnect().await?;
        self.shutdown = CancellationToken::new();
        self.connect().await
    }

    #[must_use]
    pub fn connection_state(&self) -> ConnectionState {
        *self.state.borrow()
    }

    /// Sends a normal chat message through Azalea's supported chat API.
    pub async fn send_chat(&self, message: &str) -> Result<(), AppError> {
        let message = message.trim();
        if message.is_empty() {
            return Err(AppError::EmptyChatMessage);
        }
        let length = message.chars().count();
        if length > MAX_CHAT_MESSAGE_LENGTH {
            return Err(AppError::ChatMessageTooLong {
                length,
                maximum: MAX_CHAT_MESSAGE_LENGTH,
            });
        }
        if self.connection_state() != ConnectionState::Connected {
            return Err(AppError::DisconnectedChatSend);
        }

        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::DisconnectedChatSend)?;
        if !self
            .world_state
            .lock()
            .await
            .record_sent(message.to_owned())
        {
            return Err(AppError::DuplicateChatSend);
        }
        client.chat(message.to_owned());
        logging::chat_outgoing(&self.minecraft.username, message);
        Ok(())
    }

    #[must_use]
    pub async fn world_state_snapshot(&self) -> WorldStateSnapshot {
        self.world_state.lock().await.snapshot()
    }

    pub(crate) async fn scan_loaded_blocks(
        &self,
        query: &BlockSearchQuery,
    ) -> Result<Vec<LoadedBlockCandidate>, AppError> {
        let world_snapshot = self.world_state_snapshot().await;
        let center = world_snapshot
            .bot
            .position
            .ok_or(AppError::BlockSearchPositionUnavailable)?;
        let dimension = world_snapshot
            .bot
            .dimension
            .clone()
            .ok_or(AppError::BlockSearchDimensionUnavailable)?;
        if self.connection_state() != ConnectionState::Connected || !world_snapshot.joined_world() {
            return Err(AppError::BlockSearchUnavailable);
        }

        let block_kind = query
            .block_id
            .parse::<BlockKind>()
            .map_err(|_| AppError::UnknownBlockIdentifier(query.block_id.clone()))?;
        let block_states = BlockStates::from([block_kind]);
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::BlockSearchUnavailable)?;
        let world = client
            .world()
            .map_err(|error| AppError::WorldStateUpdateFailure(error.to_string()))?;

        let radius = f64::from(query.radius);
        let min_x = (center.x - radius).floor() as i32;
        let max_x = (center.x + radius).floor() as i32;
        let min_z = (center.z - radius).floor() as i32;
        let max_z = (center.z + radius).floor() as i32;
        let min_y = (center.y - f64::from(query.vertical_range)).floor() as i32;
        let max_y = (center.y + f64::from(query.vertical_range)).floor() as i32;
        let min_chunk = ChunkPos::new(chunk_coordinate(min_x), chunk_coordinate(min_z));
        let max_chunk = ChunkPos::new(chunk_coordinate(max_x), chunk_coordinate(max_z));

        let world_guard = world.read();
        let world_min_y = world_guard.chunks.min_y();
        let world_max_y = world_min_y + world_guard.chunks.height() as i32 - 1;
        let min_y = min_y.max(world_min_y);
        let max_y = max_y.min(world_max_y);
        if min_y > max_y {
            return Ok(Vec::new());
        }

        let mut nearest = std::collections::BinaryHeap::with_capacity(query.maximum_results);
        let mut loaded_chunks = 0;
        for chunk_x in min_chunk.x..=max_chunk.x {
            for chunk_z in min_chunk.z..=max_chunk.z {
                if block_search_cancelled(self.connection_state()) {
                    return Err(AppError::BlockSearchCancelled);
                }
                let chunk_pos = ChunkPos::new(chunk_x, chunk_z);
                let Some(chunk) = world_guard.chunks.get(&chunk_pos) else {
                    continue;
                };
                loaded_chunks += 1;
                let chunk_guard = chunk.read();
                find_blocks_in_chunk(
                    &block_states,
                    chunk_pos,
                    &chunk_guard,
                    world_min_y,
                    |position| {
                        if position.x < min_x
                            || position.x > max_x
                            || position.y < min_y
                            || position.y > max_y
                            || position.z < min_z
                            || position.z > max_z
                        {
                            return;
                        }
                        let dx = f64::from(position.x) + 0.5 - center.x;
                        let dy = f64::from(position.y) + 0.5 - center.y;
                        let dz = f64::from(position.z) + 0.5 - center.z;
                        let distance_squared = dx * dx + dy * dy + dz * dz;
                        if distance_squared > radius * radius {
                            return;
                        }
                        let candidate = SearchCandidate {
                            position: BlockPosition {
                                x: position.x,
                                y: position.y,
                                z: position.z,
                            },
                            distance_squared,
                        };
                        if nearest.len() < query.maximum_results {
                            nearest.push(candidate);
                        } else if nearest.peek().is_some_and(|farthest| candidate < *farthest) {
                            nearest.pop();
                            nearest.push(candidate);
                        }
                    },
                );
            }
        }

        if loaded_chunks == 0 {
            return Err(AppError::NoLoadedChunks);
        }

        let mut candidates: Vec<_> = nearest.into_vec();
        candidates.sort_by(|left, right| {
            left.distance_squared
                .total_cmp(&right.distance_squared)
                .then_with(|| {
                    (left.position.x, left.position.y, left.position.z).cmp(&(
                        right.position.x,
                        right.position.y,
                        right.position.z,
                    ))
                })
        });
        Ok(candidates
            .into_iter()
            .map(|candidate| LoadedBlockCandidate {
                position: candidate.position,
                block_id: query.block_id.clone(),
                distance_squared: candidate.distance_squared,
                dimension: Some(dimension.clone()),
                chunk_loaded: true,
            })
            .collect())
    }

    pub(crate) async fn block_ids_at(
        &self,
        positions: &[BlockPosition],
    ) -> Result<HashMap<BlockPosition, Option<String>>, AppError> {
        if self.connection_state() != ConnectionState::Connected {
            return Err(AppError::BlockSearchCancelled);
        }
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::BlockSearchUnavailable)?;
        let world = client
            .world()
            .map_err(|error| AppError::WorldStateUpdateFailure(error.to_string()))?;
        let world_guard = world.read();
        let mut result = HashMap::with_capacity(positions.len());
        for &position in positions {
            let state = world_guard
                .get_block_state(azalea::BlockPos::new(position.x, position.y, position.z));
            let block_id = state.map(|state| state.to_trait().as_block_kind().to_str().to_owned());
            result.insert(position, block_id);
        }
        Ok(result)
    }

    /// Reads the exact block kind and its generated property map while the
    /// chunk read lock is held. A missing entry means the observation is not
    /// reliable enough to act on.
    pub(crate) async fn block_state_at(
        &self,
        position: BlockPosition,
    ) -> Result<Option<(String, HashMap<String, String>)>, AppError> {
        if self.connection_state() != ConnectionState::Connected {
            return Err(AppError::BlockSearchCancelled);
        }
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::BlockSearchUnavailable)?;
        let world = client
            .world()
            .map_err(|e| AppError::WorldStateUpdateFailure(e.to_string()))?;
        let guard = world.read();
        let Some(state) =
            guard.get_block_state(azalea::BlockPos::new(position.x, position.y, position.z))
        else {
            return Ok(None);
        };
        let block = state.to_trait();
        Ok(Some((
            format!("minecraft:{}", block.id()),
            block
                .property_map()
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
        )))
    }

    pub(crate) async fn look_data(&self) -> Result<([f64; 3], f32, f32), AppError> {
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::LookUnavailable)?;
        client
            .query_self::<(&Position, &EntityDimensions, &LookDirection), _>(
                |(position, dimensions, direction)| {
                    let position: azalea::Vec3 = **position;
                    (
                        [
                            position.x,
                            position.y + f64::from(dimensions.eye_height),
                            position.z,
                        ],
                        direction.y_rot(),
                        direction.x_rot(),
                    )
                },
            )
            .map_err(|error| AppError::LookUnavailableWithReason(error.to_string()))
    }

    pub(crate) async fn entity_hitbox(&self, entity_id: u32) -> Result<HitboxSnapshot, AppError> {
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::LookUnavailable)?;
        let Some(entity) = client
            .entity_id_by_minecraft_id(MinecraftEntityId(entity_id as i32))
            .map_err(|error| AppError::LookUnavailableWithReason(error.to_string()))?
        else {
            return Err(AppError::LookTargetDisappeared);
        };
        client
            .query_entity::<(&Position, &EntityDimensions), _>(entity, |(position, dimensions)| {
                let position: azalea::Vec3 = **position;
                HitboxSnapshot {
                    position: [position.x, position.y, position.z],
                    width: f64::from(dimensions.width),
                    height: f64::from(dimensions.height),
                }
            })
            .map_err(|_| AppError::LookTargetDisappeared)
    }

    pub(crate) async fn player_hitbox(&self, uuid: uuid::Uuid) -> Result<HitboxSnapshot, AppError> {
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::LookUnavailable)?;
        let Some(entity) = client.entity_id_by_uuid(uuid) else {
            return Err(AppError::LookTargetDisappeared);
        };
        client
            .query_entity::<(&Position, &EntityDimensions), _>(entity, |(position, dimensions)| {
                let position: azalea::Vec3 = **position;
                HitboxSnapshot {
                    position: [position.x, position.y, position.z],
                    width: f64::from(dimensions.width),
                    height: f64::from(dimensions.height),
                }
            })
            .map_err(|_| AppError::LookTargetDisappeared)
    }

    pub(crate) async fn set_look_direction(&self, yaw: f32, pitch: f32) -> Result<(), AppError> {
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::LookUnavailable)?;
        client
            .set_direction(yaw, pitch)
            .map_err(|error| AppError::LookUpdateFailure(error.to_string()))
    }

    pub(crate) async fn looked_block(&self) -> Result<LookedBlockSnapshot, AppError> {
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::LookUnavailable)?;
        let hit = client
            .hit_result()
            .map_err(|error| AppError::LookUnavailableWithReason(error.to_string()))?;
        let block = hit
            .as_block_hit_result_if_not_miss()
            .ok_or(AppError::LookTargetUnloaded)?;
        Ok(LookedBlockSnapshot {
            position: BlockPosition {
                x: block.block_pos.x,
                y: block.block_pos.y,
                z: block.block_pos.z,
            },
            face: match block.direction {
                azalea::core::direction::Direction::Up => RaycastFace::Up,
                azalea::core::direction::Direction::Down => RaycastFace::Down,
                azalea::core::direction::Direction::North => RaycastFace::North,
                azalea::core::direction::Direction::South => RaycastFace::South,
                azalea::core::direction::Direction::West => RaycastFace::West,
                azalea::core::direction::Direction::East => RaycastFace::East,
            },
        })
    }

    pub(crate) async fn begin_breaking(&self, position: BlockPosition) -> Result<(), AppError> {
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::MovementUnavailable)?;
        client.start_mining(azalea::BlockPos::new(position.x, position.y, position.z));
        Ok(())
    }

    pub(crate) async fn stop_breaking(&self) {
        let Some(client) = self.current_client.lock().await.clone() else {
            return;
        };
        if client.is_mining() {
            client
                .ecs
                .write()
                .write_message(azalea_client::mining::StopMiningBlockEvent {
                    entity: client.entity,
                });
        }
    }

    /// Explicitly interacts with a validated block instead of relying on the
    /// currently ray-cast block. Azalea sends the normal use-on-block packet.
    pub(crate) async fn interact_block(&self, position: BlockPosition) -> Result<(), AppError> {
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::MovementUnavailable)?;
        client.block_interact(azalea::BlockPos::new(position.x, position.y, position.z));
        Ok(())
    }

    pub(crate) async fn any_container_open(&self) -> bool {
        let Some(client) = self.current_client.lock().await.clone() else {
            return false;
        };
        client
            .component::<Inventory>()
            .is_ok_and(|inventory| inventory.container_menu.is_some())
    }

    /// Maps Azalea's authoritative active menu into the application model.
    pub(crate) async fn chest_menu_snapshot(
        &self,
    ) -> Result<Option<crate::container::model::MenuSnapshot>, AppError> {
        use crate::container::model::{ContainerKind, MenuSnapshot, StackSnapshot};
        use azalea::inventory::item::MaxStackSizeExt;
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::InventoryUnavailable)?;
        let inventory = client
            .component::<Inventory>()
            .map_err(|_| AppError::InventoryUnavailable)?;
        let Some(menu) = inventory.container_menu.as_ref() else {
            return Ok(None);
        };
        let kind = match menu {
            azalea::inventory::Menu::Generic9x3 { .. } => ContainerKind::SingleChest,
            azalea::inventory::Menu::Generic9x6 { .. } => ContainerKind::DoubleChest,
            _ => return Ok(None),
        };
        let content_count = match kind {
            ContainerKind::SingleChest => 27,
            ContainerKind::DoubleChest => 54,
        };
        let map = |item: &azalea::inventory::ItemStack| {
            item.as_present().map(|item| StackSnapshot {
                item_id: item.kind.to_string(),
                count: item.count.max(0) as u32,
                max_count: item.kind.max_stack_size().max(1) as u32,
            })
        };
        let slots = menu.slots();
        Ok(Some(MenuSnapshot {
            kind,
            position: None,
            window_id: inventory.id,
            revision: inventory.state_id,
            container_slots: slots[..content_count].iter().map(map).collect(),
            player_slots: slots[content_count..].iter().map(map).collect(),
        }))
    }

    pub(crate) async fn container_click(
        &self,
        window_id: i32,
        click: crate::container::model::InventoryClick,
    ) -> Result<(), AppError> {
        use crate::container::model::ClickButton;
        use azalea::inventory::operations::PickupClick;
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::InventoryUnavailable)?;
        let inventory = client
            .component::<Inventory>()
            .map_err(|_| AppError::InventoryUnavailable)?;
        if inventory.id != window_id || inventory.container_menu.is_none() {
            return Err(AppError::InventoryUnavailable);
        }
        let handle = client
            .get_inventory()
            .map_err(|_| AppError::InventoryUnavailable)?;
        handle.click(match click.button {
            ClickButton::Left => PickupClick::Left {
                slot: Some(click.slot as u16),
            },
            ClickButton::Right => PickupClick::Right {
                slot: Some(click.slot as u16),
            },
        });
        Ok(())
    }

    pub(crate) async fn close_container(&self) -> Result<(), AppError> {
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::InventoryUnavailable)?;
        let inventory = client
            .component::<Inventory>()
            .map_err(|_| AppError::InventoryUnavailable)?;
        if inventory.container_menu.is_some() {
            client
                .get_inventory()
                .map_err(|_| AppError::InventoryUnavailable)?
                .close();
        }
        Ok(())
    }

    pub(crate) async fn use_item_at_look_target(&self) -> Result<(), AppError> {
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::MovementUnavailable)?;
        client.start_use_item();
        Ok(())
    }

    pub(crate) async fn select_item_in_hotbar(&self, item_id: &str) -> Result<bool, AppError> {
        self.inventory_actions
            .mutate(|| async {
                let client = self
                    .current_client
                    .lock()
                    .await
                    .clone()
                    .ok_or(AppError::InventoryUnavailable)?;
                let menu = client.menu().map_err(|_| AppError::InventoryUnavailable)?;
                let slot =
                    menu.hotbar_slots_range()
                        .enumerate()
                        .find_map(|(hotbar_slot, menu_slot)| {
                            menu.slots()
                                .get(menu_slot)
                                .filter(|item| {
                                    !item.is_empty() && item.kind().to_string() == item_id
                                })
                                .map(|_| hotbar_slot as u8)
                        });
                if let Some(slot) = slot {
                    client.set_selected_hotbar_slot(slot);
                    Ok(true)
                } else {
                    Ok(false)
                }
            })
            .await
    }

    /// Inspect Azalea's pinned block behavior and item data components, run the
    /// pure policy, then serialize only the selected-slot mutation.
    pub(crate) async fn select_tool_for_block(
        &self,
        block_id: &str,
        policy: &crate::interaction::tool_selection::ToolSelectionPolicy,
        protected_tools: &[String],
        reserved_tools: &[String],
    ) -> Result<crate::interaction::tool_selection::ToolSelection, AppError> {
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::InventoryUnavailable)?;
        let block_kind = block_id
            .parse::<BlockKind>()
            .map_err(|_| AppError::UnknownBlockIdentifier(block_id.into()))?;
        let block_states = BlockStates::from(block_kind);
        let block_trait = block_states
            .set
            .iter()
            .next()
            .expect("a registered block has a state")
            .to_trait();
        let requires_correct_tool = block_trait.behavior().requires_correct_tool_for_drops;
        let menu = client.menu().map_err(|_| AppError::InventoryUnavailable)?;
        let current = client
            .component::<Inventory>()
            .map_err(|_| AppError::InventoryUnavailable)?
            .selected_hotbar_slot;
        let menu_slots = menu.slots();
        let candidates = menu
            .hotbar_slots_range()
            .enumerate()
            .filter_map(|(hotbar_slot, menu_slot)| {
                let item = menu_slots.get(menu_slot)?;
                if item.is_empty() {
                    return None;
                }
                let id = item.kind().to_string();
                let tool = item.get_component::<Tool>();
                let (speed, correct) =
                    tool.as_ref().map_or((1.0, !requires_correct_tool), |tool| {
                        let rule = tool
                            .rules
                            .iter()
                            .find(|rule| rule.blocks.contains(block_kind));
                        (
                            rule.and_then(|rule| rule.speed)
                                .unwrap_or(tool.default_mining_speed),
                            rule.and_then(|rule| rule.correct_for_drops)
                                .unwrap_or(!requires_correct_tool),
                        )
                    });
                let remaining = item.get_component::<MaxDamage>().map(|maximum| {
                    let damage = item
                        .get_component::<Damage>()
                        .map_or(0, |damage| damage.amount);
                    maximum.amount.saturating_sub(damage).max(0) as u32
                });
                let efficiency = item
                    .get_component::<Enchantments>()
                    .and_then(|enchantments| {
                        enchantments.levels.iter().find_map(|(kind, level)| {
                            format!("{kind:?}")
                                .to_ascii_lowercase()
                                .contains("efficiency")
                                .then_some((*level).max(0) as u32)
                        })
                    });
                Some(crate::interaction::tool_selection::ToolCandidate {
                    hotbar_slot: hotbar_slot as u8,
                    item_id: id.clone(),
                    category: crate::interaction::tool_selection::category(&id),
                    tier: crate::interaction::tool_selection::tier(&id),
                    correct_for_drops: correct,
                    mining_speed: speed,
                    remaining_durability: remaining,
                    efficiency_level: efficiency,
                    protected: protected_tools.contains(&id),
                    reserved: reserved_tools.contains(&id),
                    held: hotbar_slot as u8 == current,
                })
            })
            .collect::<Vec<_>>();
        drop(menu);
        let selection = crate::interaction::tool_selection::select_tool(
            &crate::interaction::tool_selection::BlockKnowledge {
                block_id: block_id.into(),
                preferred_category: crate::interaction::tool_selection::preferred_category(
                    block_id,
                ),
                requires_correct_tool,
            },
            &candidates,
            policy,
        )
        .map_err(|outcome| AppError::NoSuitableTool {
            block_id: outcome.block_id,
            explanation: outcome.explanation,
        })?;
        if selection.hotbar_slot != current {
            let slot = selection.hotbar_slot;
            self.inventory_actions
                .mutate(|| async {
                    client.set_selected_hotbar_slot(slot);
                    Ok(())
                })
                .await?;
        }
        Ok(selection)
    }

    pub async fn set_movement_snapshot(&self, movement: MovementSnapshot) {
        self.world_state.lock().await.set_movement(movement);
    }

    pub(crate) async fn set_current_task(&self, task: TaskSnapshot) {
        self.world_state.lock().await.set_current_task(task);
    }

    pub(crate) async fn clear_current_task(&self) {
        self.world_state.lock().await.clear_current_task();
    }

    pub async fn start_navigation_to(
        &self,
        destination: PositionSnapshot,
        follow_distance: Option<f64>,
        mode: NavigationMode,
    ) -> Result<(), AppError> {
        if self.connection_state() != ConnectionState::Connected {
            return Err(AppError::MovementUnavailable);
        }
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::MovementUnavailable)?;
        if let Some(distance) = follow_distance {
            client.start_goto_with_opts(
                RadiusGoal::new(
                    azalea::Vec3::new(destination.x, destination.y, destination.z),
                    distance as f32,
                ),
                PathfinderOpts::new()
                    .allow_mining(mode.allows_mining())
                    .retry_on_no_path(true),
            );
        } else {
            let block = destination.block();
            client.start_goto_with_opts(
                BlockPosGoal(azalea::BlockPos::new(block.x, block.y, block.z)),
                PathfinderOpts::new()
                    .allow_mining(mode.allows_mining())
                    .retry_on_no_path(true),
            );
        }
        Ok(())
    }

    pub async fn stop_navigation(&self) -> Result<(), AppError> {
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::MovementUnavailable)?;
        client.force_stop_pathfinding();
        Ok(())
    }

    pub async fn navigation_status(&self) -> Result<NavigationStatus, AppError> {
        let client = self
            .current_client
            .lock()
            .await
            .clone()
            .ok_or(AppError::MovementUnavailable)?;
        Ok(NavigationStatus {
            calculating: client.is_calculating_path(),
            executing: client.is_executing_path(),
            reached: client.is_goto_target_reached(),
        })
    }

    #[must_use]
    pub fn status(&self) -> MinecraftStatus {
        MinecraftStatus {
            connection_state: self.connection_state(),
            server: self.minecraft.server.clone(),
            username: self.minecraft.username.clone(),
            account_mode: match self.minecraft.account_mode {
                AccountMode::Offline => "offline".to_owned(),
                AccountMode::Microsoft => "microsoft".to_owned(),
            },
            reconnect: self.reconnect.clone(),
        }
    }

    async fn join_once(
        &self,
    ) -> Result<(Client, tokio::sync::mpsc::UnboundedReceiver<Event>), AppError> {
        let account = self.account().await?;
        timeout(
            CONNECTION_TIMEOUT,
            Client::join(account, self.minecraft.server.as_str()),
        )
        .await
        .map_err(|_| AppError::NetworkTimeout {
            seconds: CONNECTION_TIMEOUT.as_secs(),
        })?
        .map_err(|error| {
            AppError::ConnectionFailure(format!("could not resolve server: {error:?}"))
        })
    }

    async fn account(&self) -> Result<Account, AppError> {
        match self.minecraft.account_mode {
            AccountMode::Offline => Ok(Account::offline(&self.minecraft.username)),
            AccountMode::Microsoft => {
                self.set_state(ConnectionState::Authenticating);
                debug!(username = %self.minecraft.username, "authenticating");
                let cache_file = auth_cache_file()?;
                tokio::fs::create_dir_all(cache_file.parent().ok_or_else(|| {
                    AppError::InvalidConfiguration("invalid auth cache path".to_owned())
                })?)
                .await?;
                Account::microsoft_with_opts(
                    &self.minecraft.username,
                    MicrosoftAccountOpts {
                        cache_file: Some(cache_file),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|error| AppError::AuthenticationFailure(error.to_string()))
            }
        }
    }

    fn disable_azalea_reconnect(&self, client: &Client) {
        client
            .ecs
            .write()
            .insert_resource(AutoReconnectDelay::new(Duration::MAX));
    }

    fn set_state(&self, state: ConnectionState) {
        let _ = self.state_tx.send(state);
        if let Ok(mut world) = self.world_state.try_lock() {
            world.set_connection(match state {
                ConnectionState::Disconnected => WorldConnectionState::Disconnected,
                ConnectionState::Connecting => WorldConnectionState::Connecting,
                ConnectionState::Authenticating => WorldConnectionState::Authenticating,
                ConnectionState::JoiningWorld => WorldConnectionState::JoiningWorld,
                ConnectionState::Connected => WorldConnectionState::Connected,
                ConnectionState::Reconnecting => WorldConnectionState::Reconnecting,
            });
        }
    }
}

async fn supervise(
    mut events: tokio::sync::mpsc::UnboundedReceiver<Event>,
    context: SupervisorContext,
) {
    let SupervisorContext {
        minecraft,
        reconnect,
        current_client,
        world_state,
        console,
        state_tx,
        shutdown,
    } = context;
    loop {
        let outcome = wait_for_disconnect(
            &mut events,
            &state_tx,
            &shutdown,
            &world_state,
            &console,
            &current_client,
        )
        .await;
        if shutdown.is_cancelled() {
            return;
        }

        let Some(reason) = outcome else {
            let _ = state_tx.send(ConnectionState::Disconnected);
            return;
        };
        let _ = state_tx.send(ConnectionState::Disconnected);
        {
            let mut world = world_state.lock().await;
            world.set_connection(WorldConnectionState::Disconnected);
            world.set_joined_world(false);
            world.clear_session_state();
            match &reason {
                DisconnectReason::Kicked(message) => world.set_disconnect_reason(message.clone()),
                DisconnectReason::ConnectionFailure(message) => {
                    world.set_connection_error(message.clone())
                }
            }
        }
        logging::info("Disconnected");
        match &reason {
            DisconnectReason::Kicked(reason) => {
                let kick_error = AppError::KickedByServer(reason.clone());
                logging::warning(format!("Disconnected ({kick_error})"));
            }
            DisconnectReason::ConnectionFailure(reason) => {
                let connection_error = AppError::ConnectionFailure(reason.clone());
                logging::warning(format!("Connection failed ({connection_error})"));
            }
        }

        if !reconnect.enabled {
            return;
        }

        let mut reconnected = false;
        logging::info("Reconnecting...");
        for _attempt in 1..=reconnect.maximum_attempts {
            let _ = state_tx.send(ConnectionState::Reconnecting);
            world_state
                .lock()
                .await
                .set_connection(WorldConnectionState::Reconnecting);
            if wait_for_reconnect_delay(reconnect.delay_seconds, &shutdown).await {
                return;
            }

            match join_for_reconnect(&minecraft, &state_tx, &shutdown).await {
                Ok((new_client, new_events)) => {
                    new_client
                        .ecs
                        .write()
                        .insert_resource(AutoReconnectDelay::new(Duration::MAX));
                    *current_client.lock().await = Some(new_client.clone());
                    events = new_events;
                    if wait_for_spawn(&new_client, &mut events, &shutdown, &world_state, &console)
                        .await
                    {
                        let _ = state_tx.send(ConnectionState::Connected);
                        world_state
                            .lock()
                            .await
                            .set_connection(WorldConnectionState::Connected);
                        logging::success("Reconnected");
                        reconnected = true;
                        break;
                    }
                }
                Err(error) => {
                    debug!(error = %error, "reconnect attempt failed");
                }
            }
        }

        if !reconnected {
            logging::warning("Reconnect failed (attempts exhausted)");
            let _ = state_tx.send(ConnectionState::Disconnected);
            return;
        }
    }
}

async fn join_for_reconnect(
    minecraft: &MinecraftConfig,
    state_tx: &watch::Sender<ConnectionState>,
    shutdown: &CancellationToken,
) -> Result<(Client, tokio::sync::mpsc::UnboundedReceiver<Event>), AppError> {
    let account = match minecraft.account_mode {
        AccountMode::Offline => Account::offline(&minecraft.username),
        AccountMode::Microsoft => {
            let cache_file = auth_cache_file()?;
            tokio::fs::create_dir_all(cache_file.parent().ok_or_else(|| {
                AppError::InvalidConfiguration("invalid auth cache path".to_owned())
            })?)
            .await?;
            let _ = state_tx.send(ConnectionState::Authenticating);
            Account::microsoft_with_opts(
                &minecraft.username,
                MicrosoftAccountOpts {
                    cache_file: Some(cache_file),
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| AppError::AuthenticationFailure(error.to_string()))?
        }
    };

    let join = Client::join(account, minecraft.server.as_str());
    tokio::select! {
        _ = shutdown.cancelled() => Err(AppError::ConnectionFailure("shutdown requested".to_owned())),
        result = timeout(CONNECTION_TIMEOUT, join) => result
            .map_err(|_| AppError::NetworkTimeout { seconds: CONNECTION_TIMEOUT.as_secs() })?
            .map_err(|error| AppError::ConnectionFailure(format!("could not resolve server: {error:?}"))),
    }
}

async fn wait_for_disconnect(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    state_tx: &watch::Sender<ConnectionState>,
    shutdown: &CancellationToken,
    world_state: &Arc<Mutex<WorldState>>,
    console: &ConsoleConfig,
    current_client: &Arc<Mutex<Option<Client>>>,
) -> Option<DisconnectReason> {
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return None,
            event = events.recv() => match event? {
                Event::Spawn => {
                    let _ = state_tx.send(ConnectionState::Connected);
                    let mut world = world_state.lock().await;
                    world.set_connection(WorldConnectionState::Connected);
                    world.set_joined_world(true);
                }
                Event::AddPlayer(info) | Event::UpdatePlayer(info) => {
                    world_state.lock().await.upsert_player(crate::minecraft::world_state::PlayerSnapshot { uuid: info.uuid, username: info.profile.name, position: None, distance: None, dimension: None, loaded: false, last_seen: SystemTime::now() });
                }
                Event::RemovePlayer(info) => world_state.lock().await.remove_player(info.uuid),
                Event::Packet(packet) => {
                    if let azalea::protocol::packets::game::ClientboundGamePacket::RemoveEntities(packet) = &*packet {
                        let mut world = world_state.lock().await;
                        for entity_id in &packet.entity_ids {
                            world.remove_entity(entity_id.0 as u32);
                        }
                    }
                }
                Event::Tick => refresh_ecs_state(current_client, world_state).await,
                Event::Chat(packet) => {
                    let (bot_uuid, bot_username) = current_client
                        .lock()
                        .await
                        .as_ref()
                        .map(|client| (Some(client.uuid()), client.username()))
                        .unwrap_or((None, String::new()));
                    let mut world = world_state.lock().await;
                    handle_chat(
                        &packet,
                        console,
                        bot_uuid,
                        &bot_username,
                        &mut world,
                    );
                }
                Event::Disconnect(reason) => return Some(DisconnectReason::Kicked(reason.map_or_else(|| "server closed the connection".to_owned(), |reason| reason.to_string()))),
                Event::ConnectionFailed(error) => return Some(DisconnectReason::ConnectionFailure(error.to_string())),
                _ => {}
            }
        }
    }
}

// The Azalea ECS guard is explicitly dropped before the first async state lock
// below; the lint cannot prove this through the query construction.
#[allow(clippy::await_holding_lock)]
async fn refresh_ecs_state(
    current_client: &Arc<Mutex<Option<Client>>>,
    world_state: &Arc<Mutex<WorldState>>,
) {
    let Some(client) = current_client.lock().await.clone() else {
        return;
    };
    let mut ecs = client.ecs.write();
    let mut bot = crate::minecraft::world_state::BotSnapshot::default();
    let mut bot_entity = None;
    let mut bot_inventory = None;
    let mut query = ecs.query_filtered::<(
        azalea::ecs::entity::Entity,
        Option<&Position>,
        Option<&LookDirection>,
        Option<&Health>,
        Option<&azalea::local_player::Hunger>,
        Option<&azalea::local_player::Experience>,
        Option<&Inventory>,
        Option<&WorldName>,
        Option<&Physics>,
        Option<&Dead>,
    ), azalea::ecs::query::With<LocalEntity>>();
    if let Some((
        entity,
        position,
        direction,
        health,
        hunger,
        experience,
        inventory,
        dimension,
        physics,
        dead,
    )) = query.iter(&ecs).next()
    {
        bot_entity = Some(entity);
        bot.position = position.map(|p| {
            let v: azalea::Vec3 = **p;
            crate::minecraft::world_state::PositionSnapshot {
                x: v.x,
                y: v.y,
                z: v.z,
            }
        });
        bot.yaw = direction.map(LookDirection::y_rot);
        bot.pitch = direction.map(LookDirection::x_rot);
        bot.health = health.map(|h| h.0);
        bot.food_level = hunger.map(|h| h.food);
        bot.saturation = hunger.map(|h| h.saturation);
        bot.experience_level = experience.map(|e| e.level);
        bot.selected_hotbar_slot = inventory.map(|i| i.selected_hotbar_slot);
        bot_inventory = inventory;
        bot.dimension = dimension.map(ToString::to_string);
        bot.on_ground = physics.map(Physics::on_ground);
        bot.alive = Some(dead.is_none());
    }
    let inventory_snapshot = inventory_from_component(&bot_inventory);
    let mut entities = ecs.query::<(
        azalea::ecs::entity::Entity,
        &MinecraftEntityId,
        &EntityUuid,
        &EntityKindComponent,
        &Position,
        Option<&Dead>,
        Option<&Health>,
        Option<&WorldName>,
        Option<&ItemItem>,
    )>();
    let mut updates = Vec::new();
    let mut existing_ids = HashSet::new();
    for (entity, minecraft_id, uuid, kind, position, dead, health, _dimension, item) in
        entities.iter(&ecs)
    {
        if Some(entity) == bot_entity {
            continue;
        }
        if dead.is_some() || health.is_some_and(|health| health.0 <= 0.0) {
            continue;
        }
        let v: azalea::Vec3 = **position;
        let entity_id = minecraft_id.0 as u32;
        existing_ids.insert(entity_id);
        updates.push(crate::minecraft::world_state::EntitySnapshot {
            entity_id,
            uuid: Some(**uuid),
            entity_type: kind.0.to_string(),
            position: crate::minecraft::world_state::PositionSnapshot {
                x: v.x,
                y: v.y,
                z: v.z,
            },
            distance: 0.0,
            alive: Some(true),
            health: health.map(|health| health.0),
            custom_name: None,
            item: item.and_then(|item| item.0.as_present()).map(|stack| {
                crate::minecraft::world_state::DroppedItemSnapshot {
                    item_id: stack.kind.to_string(),
                    count: stack.count.max(0) as u32,
                }
            }),
            last_seen: SystemTime::now(),
        });
    }
    let mut player_updates = Vec::new();
    let mut players = ecs.query::<(
        &EntityUuid,
        &GameProfileComponent,
        Option<&Position>,
        Option<&WorldName>,
    )>();
    for (uuid, profile, position, dimension) in players.iter(&ecs) {
        let position = position.map(|p| {
            let v: azalea::Vec3 = **p;
            PositionSnapshot {
                x: v.x,
                y: v.y,
                z: v.z,
            }
        });
        player_updates.push(PlayerSnapshot {
            uuid: **uuid,
            username: profile.0.name.clone(),
            position,
            distance: None,
            dimension: dimension.map(ToString::to_string),
            loaded: position.is_some(),
            last_seen: SystemTime::now(),
        });
    }
    drop(ecs);
    let mut world = world_state.lock().await;
    if bot_entity.is_some() {
        if let Some(inventory) = inventory_snapshot {
            world.update_inventory(inventory);
        }
        world.update_bot(bot);
    }
    for entity in updates {
        world.upsert_entity(entity);
    }
    world.remove_entities_not_in(&existing_ids);
    for player in player_updates {
        world.upsert_player(player);
    }
    world.remove_stale_entities();
}

fn inventory_from_component(inventory: &Option<&Inventory>) -> Option<InventorySnapshot> {
    let inventory = (*inventory)?;
    let slots = inventory
        .menu()
        .slots()
        .iter()
        .enumerate()
        .map(|(slot, item)| {
            let (item_id, count) = if item.is_empty() {
                (None, 0)
            } else {
                (Some(item.kind().to_string()), item.count().max(0) as u32)
            };
            InventorySlot {
                slot,
                item_id,
                display_name: None,
                count,
            }
        })
        .collect();
    Some(InventorySnapshot {
        available: true,
        slots,
        selected_hotbar_slot: Some(inventory.selected_hotbar_slot),
        total_counts: Default::default(),
    })
}

async fn wait_for_spawn(
    client: &Client,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    shutdown: &CancellationToken,
    world_state: &Arc<Mutex<WorldState>>,
    console: &ConsoleConfig,
) -> bool {
    matches!(
        timeout(CONNECTION_TIMEOUT, async {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => return false,
                    event = events.recv() => match event {
                        Some(Event::Spawn) => {
                            let mut world = world_state.lock().await;
                            world.set_connection(WorldConnectionState::Connected);
                            world.set_joined_world(true);
                            return true;
                        }
                        Some(Event::Chat(packet)) => {
                            let bot_uuid = Some(client.uuid());
                            let bot_username = client.username();
                            let mut world = world_state.lock().await;
                            handle_chat(
                                &packet,
                                console,
                                bot_uuid,
                                &bot_username,
                                &mut world,
                            );
                        }
                        Some(Event::Disconnect(reason)) => {
                            debug!(reason = ?reason, "reconnect connection dropped before spawn");
                            return false;
                        }
                        Some(Event::ConnectionFailed(error)) => {
                            debug!(error = %error, "reconnect connection failed before spawn");
                            return false;
                        }
                        Some(_) => {}
                        None => return false,
                    }
                }
            }
        })
        .await,
        Ok(true)
    )
}

async fn wait_for_reconnect_delay(seconds: u64, shutdown: &CancellationToken) -> bool {
    tokio::select! {
        _ = shutdown.cancelled() => true,
        _ = sleep(Duration::from_secs(seconds)) => false,
    }
}

fn auth_cache_file() -> Result<PathBuf, AppError> {
    Ok(PathBuf::from("auth-cache").join("azalea-auth.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> MinecraftClient {
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
        )
    }

    #[tokio::test]
    async fn rejects_empty_chat() {
        assert!(matches!(
            client().send_chat("   ").await,
            Err(AppError::EmptyChatMessage)
        ));
    }

    #[tokio::test]
    async fn rejects_chat_longer_than_protocol_limit() {
        let message = "x".repeat(MAX_CHAT_MESSAGE_LENGTH + 1);
        assert!(matches!(
            client().send_chat(&message).await,
            Err(AppError::ChatMessageTooLong { .. })
        ));
    }

    #[tokio::test]
    async fn rejects_chat_when_disconnected() {
        assert!(matches!(
            client().send_chat("hello").await,
            Err(AppError::DisconnectedChatSend)
        ));
    }

    #[test]
    fn connection_state_transitions_are_runtime_independent() {
        let client = client();
        assert_eq!(client.connection_state(), ConnectionState::Disconnected);
        client.set_state(ConnectionState::Connecting);
        assert_eq!(client.connection_state(), ConnectionState::Connecting);
        client.set_state(ConnectionState::Connected);
        assert_eq!(client.connection_state(), ConnectionState::Connected);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn disconnect_signals_shutdown_and_rejects_duplicate_connection() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let mut client = client();
                let shutdown = client.shutdown.clone();
                client.supervisor = Some(tokio::task::spawn_local(async {}));

                assert!(matches!(
                    client.connect().await,
                    Err(AppError::ConnectionFailure(message))
                        if message.contains("already running")
                ));
                client
                    .disconnect()
                    .await
                    .expect("disconnect should succeed");
                assert!(shutdown.is_cancelled());
                assert_eq!(client.connection_state(), ConnectionState::Disconnected);
            })
            .await;
    }
}
#[test]
fn block_search_cancels_when_connection_is_not_connected() {
    assert!(block_search_cancelled(ConnectionState::Disconnected));
    assert!(block_search_cancelled(ConnectionState::Reconnecting));
    assert!(!block_search_cancelled(ConnectionState::Connected));
}
