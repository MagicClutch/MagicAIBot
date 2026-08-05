//! Application-owned, bounded Minecraft state.

use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant, SystemTime},
};

use uuid::Uuid;

use super::container::{ContainerObserver, ContainerSnapshot, MenuObservation};
use super::dropped_items::DroppedItemObservation;
use crate::error::AppError;

pub const DEFAULT_ENTITY_RADIUS: f64 = 64.0;
pub const DEFAULT_MAXIMUM_ENTITIES: usize = 256;
pub const DEFAULT_STALE_SECONDS: u64 = 30;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WorldConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Authenticating,
    JoiningWorld,
    Connected,
    Reconnecting,
    ShuttingDown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PositionSnapshot {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}
impl PositionSnapshot {
    pub fn block(self) -> BlockPosition {
        BlockPosition {
            x: self.x.floor() as i32,
            y: self.y.floor() as i32,
            z: self.z.floor() as i32,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct BlockPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[derive(Clone, Debug, Default)]
pub struct ConnectionSnapshot {
    pub state: WorldConnectionState,
    pub server: String,
    pub username: String,
    pub account_mode: String,
    pub joined_world: bool,
    pub last_disconnect_reason: Option<String>,
    pub last_connection_error: Option<String>,
    pub last_successful_join: Option<SystemTime>,
}

#[derive(Clone, Debug, Default)]
pub struct BotSnapshot {
    pub position: Option<PositionSnapshot>,
    pub block_position: Option<BlockPosition>,
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    pub dimension: Option<String>,
    pub health: Option<f32>,
    pub maximum_health: Option<f32>,
    pub food_level: Option<u32>,
    pub saturation: Option<f32>,
    pub experience_level: Option<u32>,
    pub selected_hotbar_slot: Option<u8>,
    pub alive: Option<bool>,
    pub on_ground: Option<bool>,
    pub last_position_update: Option<SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventorySlot {
    pub slot: usize,
    pub item_id: Option<String>,
    pub display_name: Option<String>,
    pub count: u32,
}
#[derive(Clone, Debug, Default)]
pub struct InventorySnapshot {
    pub available: bool,
    /// Monotonically increases only when slot contents change.
    pub revision: u64,
    pub slots: Vec<InventorySlot>,
    pub selected_hotbar_slot: Option<u8>,
    pub total_counts: HashMap<String, u32>,
}
impl InventorySnapshot {
    pub fn count_item(&self, id: &str) -> u32 {
        self.total_counts.get(id).copied().unwrap_or(0)
    }
    pub fn has_item(&self, id: &str, amount: u32) -> bool {
        self.count_item(id) >= amount
    }
    pub fn find_slots(&self, id: &str) -> Vec<&InventorySlot> {
        self.slots
            .iter()
            .filter(|s| s.item_id.as_deref() == Some(id))
            .collect()
    }
    pub fn selected_item(&self) -> Option<&InventorySlot> {
        let selected = usize::from(self.selected_hotbar_slot?);
        if self.slots.len() < 9 {
            return self.slots.iter().find(|slot| slot.slot == selected);
        }
        let first_hotbar = self.slots.len() - 9;
        self.slots
            .iter()
            .find(|slot| slot.slot == first_hotbar + selected)
    }
    pub fn item_is_in_hotbar(&self, id: &str) -> bool {
        let Some(first_hotbar) = self.slots.len().checked_sub(9) else {
            return false;
        };
        self.slots
            .iter()
            .any(|slot| slot.slot >= first_hotbar && slot.item_id.as_deref() == Some(id))
    }
    fn rebuild_counts(&mut self) {
        self.total_counts.clear();
        for slot in &self.slots {
            if let Some(id) = &slot.item_id {
                *self.total_counts.entry(id.clone()).or_default() += slot.count;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlayerSnapshot {
    pub uuid: Uuid,
    pub username: String,
    pub position: Option<PositionSnapshot>,
    pub distance: Option<f64>,
    pub dimension: Option<String>,
    pub loaded: bool,
    pub last_seen: SystemTime,
}
#[derive(Clone, Debug)]
pub struct EntitySnapshot {
    pub entity_id: u32,
    pub uuid: Option<Uuid>,
    pub entity_type: String,
    /// Present for Azalea's `minecraft:item` entities.
    pub item_id: Option<String>,
    pub item_count: u32,
    pub dimension: Option<String>,
    pub position: PositionSnapshot,
    pub distance: f64,
    pub alive: Option<bool>,
    pub health: Option<f32>,
    pub custom_name: Option<String>,
    /// Present only for `minecraft:item`; sourced from Azalea's authoritative
    /// item metadata component rather than inferred from the entity kind.
    pub item: Option<DroppedItemSnapshot>,
    pub last_seen: SystemTime,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DroppedItemSnapshot {
    pub item_id: String,
    pub count: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChatMessageKind {
    Player,
    System,
    ActionBar,
}
#[derive(Clone, Debug)]
pub struct ChatRecord {
    pub kind: ChatMessageKind,
    pub sender_uuid: Option<Uuid>,
    pub sender: Option<String>,
    pub text: String,
    pub timestamp: SystemTime,
}
/// Lightweight "what is the bot doing right now" display used by `/status`.
/// There is no task registry behind this -- callers set it before starting a
/// direct service call and clear it when the call finishes (see the
/// `*_and_wait` helpers in `src/app.rs`).
#[derive(Clone, Debug)]
pub struct TaskSnapshot {
    pub name: String,
    pub started_at: SystemTime,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MovementStatus {
    #[default]
    Idle,
    MovingToPosition,
    FollowingPlayer,
    Cancelled,
    Failed,
    Completed,
}

#[derive(Clone, Debug, Default)]
pub struct MovementSnapshot {
    pub status: MovementStatus,
    pub destination: Option<PositionSnapshot>,
    pub target_player: Option<String>,
    pub start_position: Option<PositionSnapshot>,
    pub started_at: Option<SystemTime>,
    pub estimated_distance: Option<f64>,
    pub last_movement_update: Option<SystemTime>,
    pub failure_reason: Option<String>,
    /// Per-goal override for "how close counts as arrived", used instead of
    /// `MovementConfig::arrival_distance` for the duration of this goal --
    /// e.g. a player approach that should stop a couple of blocks short
    /// rather than walking up against them. `None` means "use the configured
    /// default", which is what every plain `/goto`/mining-approach goal
    /// still gets. Also passed straight through to Azalea's own pathfinder
    /// as its stop distance (see `MovementService::goto_internal`), so the
    /// app-level arrival check and the pathfinder's own idea of "reached"
    /// never disagree.
    pub arrival_distance_override: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct WorldStateSnapshot {
    pub session_id: u64,
    pub connection: ConnectionSnapshot,
    pub bot: BotSnapshot,
    pub inventory: InventorySnapshot,
    pub container: ContainerSnapshot,
    pub players: Vec<PlayerSnapshot>,
    pub entities: Vec<EntitySnapshot>,
    pub dropped_items: Vec<DroppedItemObservation>,
    pub last_received_chat: Option<ChatRecord>,
    pub last_sent_chat: Option<ChatRecord>,
    pub current_task: Option<TaskSnapshot>,
    pub movement: MovementSnapshot,
    pub last_updated_at: SystemTime,
}
impl Default for WorldStateSnapshot {
    fn default() -> Self {
        Self {
            session_id: 0,
            connection: ConnectionSnapshot::default(),
            bot: BotSnapshot::default(),
            inventory: InventorySnapshot::default(),
            container: ContainerSnapshot::default(),
            players: Vec::new(),
            entities: Vec::new(),
            dropped_items: Vec::new(),
            last_received_chat: None,
            last_sent_chat: None,
            current_task: None,
            movement: MovementSnapshot::default(),
            last_updated_at: SystemTime::UNIX_EPOCH,
        }
    }
}
impl WorldStateSnapshot {
    pub fn joined_world(&self) -> bool {
        self.connection.joined_world
    }
    pub fn find_player_by_name(&self, name: &str) -> Option<&PlayerSnapshot> {
        self.players
            .iter()
            .find(|player| player.username.eq_ignore_ascii_case(name))
    }
    /// Like [`Self::find_player_by_name`], but only considers players Azalea
    /// currently has a loaded entity/position for (`loaded == true`) --
    /// `#drop <item> <amount> <player>` must not path toward a stale
    /// last-seen record for someone who has since logged off or moved out
    /// of range. Prefers an exact-case match over a case-insensitive one, so
    /// `Steve` and `steve` being online simultaneously resolves
    /// deterministically to whichever name was actually typed.
    pub fn find_loaded_player_by_name(&self, name: &str) -> Option<&PlayerSnapshot> {
        let mut case_insensitive = None;
        for player in self.players.iter().filter(|player| player.loaded) {
            if player.username == name {
                return Some(player);
            }
            if case_insensitive.is_none() && player.username.eq_ignore_ascii_case(name) {
                case_insensitive = Some(player);
            }
        }
        case_insensitive
    }
}

#[derive(Debug)]
pub struct WorldState {
    session_id: u64,
    pub(crate) connection: ConnectionSnapshot,
    bot: BotSnapshot,
    inventory: InventorySnapshot,
    container: ContainerObserver,
    players: HashMap<Uuid, PlayerSnapshot>,
    entities: HashMap<u32, EntitySnapshot>,
    dropped_items: HashMap<u32, DroppedItemObservation>,
    last_received_chat: Option<ChatRecord>,
    incoming_player_chat: VecDeque<ChatRecord>,
    incoming_player_chat_capacity: usize,
    last_sent_chat: Option<ChatRecord>,
    current_task: Option<TaskSnapshot>,
    movement: MovementSnapshot,
    last_updated_at: SystemTime,
    last_displayed_signature: Option<(String, Instant)>,
    pub(crate) entity_radius: f64,
    pub(crate) maximum_entities: usize,
    pub(crate) stale_entity_after: Duration,
}

impl Default for WorldState {
    fn default() -> Self {
        Self::with_limits(
            DEFAULT_ENTITY_RADIUS,
            DEFAULT_MAXIMUM_ENTITIES,
            DEFAULT_STALE_SECONDS,
        )
        .expect("default world-state limits are valid")
    }
}
impl WorldState {
    pub fn with_limits(
        radius: f64,
        maximum_entities: usize,
        stale_seconds: u64,
    ) -> Result<Self, AppError> {
        validate_limits(radius, maximum_entities, stale_seconds)?;
        Ok(Self {
            session_id: 0,
            connection: ConnectionSnapshot::default(),
            bot: BotSnapshot::default(),
            inventory: InventorySnapshot::default(),
            container: ContainerObserver::default(),
            players: HashMap::new(),
            entities: HashMap::new(),
            dropped_items: HashMap::new(),
            last_received_chat: None,
            incoming_player_chat: VecDeque::new(),
            incoming_player_chat_capacity: 64,
            last_sent_chat: None,
            current_task: None,
            movement: MovementSnapshot::default(),
            last_updated_at: SystemTime::now(),
            last_displayed_signature: None,
            entity_radius: radius,
            maximum_entities,
            stale_entity_after: Duration::from_secs(stale_seconds),
        })
    }
    fn touch(&mut self) {
        self.last_updated_at = SystemTime::now();
    }
    pub fn set_connection(&mut self, state: WorldConnectionState) {
        self.connection.state = state;
        self.touch();
    }
    pub fn set_connection_info(&mut self, server: String, username: String, account_mode: String) {
        self.connection.server = server;
        self.connection.username = username;
        self.connection.account_mode = account_mode;
        self.touch();
    }
    pub fn set_joined_world(&mut self, joined: bool) {
        self.connection.joined_world = joined;
        if joined {
            self.connection.last_successful_join = Some(SystemTime::now());
            self.container.begin_session(SystemTime::now());
        }
        self.touch();
    }
    pub fn set_disconnect_reason(&mut self, reason: impl Into<String>) {
        self.connection.last_disconnect_reason = Some(reason.into());
        self.touch();
    }
    pub fn set_connection_error(&mut self, error: impl Into<String>) {
        self.connection.last_connection_error = Some(error.into());
        self.touch();
    }
    pub fn update_bot(&mut self, mut bot: BotSnapshot) {
        bot.block_position = bot.position.map(PositionSnapshot::block);
        if bot.position.is_some() {
            bot.last_position_update = Some(SystemTime::now());
        }
        self.bot = bot;
        self.touch();
    }
    pub fn update_inventory(&mut self, mut inventory: InventorySnapshot) {
        inventory.slots.sort_by_key(|s| s.slot);
        inventory.rebuild_counts();
        inventory.revision = if inventory.slots != self.inventory.slots {
            self.inventory.revision.wrapping_add(1)
        } else {
            self.inventory.revision
        };
        self.inventory = inventory;
        self.touch();
    }
    pub(crate) fn observe_container(&mut self, menu: Option<MenuObservation>, alive: bool) {
        self.container.observe(menu, alive, SystemTime::now());
        self.touch();
    }
    pub fn container_snapshot_for_generation(&self, generation: u64) -> ContainerSnapshot {
        self.container.snapshot_for_generation(generation)
    }
    pub fn clear_inventory(&mut self) {
        self.inventory = InventorySnapshot::default();
        self.touch();
    }
    /// Drop observations that belong to a disconnected Azalea session.
    pub fn clear_session_state(&mut self) {
        self.session_id = self.session_id.wrapping_add(1);
        self.bot = BotSnapshot::default();
        self.players.clear();
        self.entities.clear();
        self.dropped_items.clear();
        self.inventory = InventorySnapshot::default();
        self.container.disconnect(SystemTime::now());
        self.bot = BotSnapshot::default();
        self.players.clear();
        self.entities.clear();
        self.inventory = InventorySnapshot::default();
        self.movement = MovementSnapshot::default();
        self.current_task = None;
        self.last_received_chat = None;
        self.incoming_player_chat.clear();
        self.touch();
    }
    pub fn upsert_player(&mut self, mut player: PlayerSnapshot) {
        player.distance = player
            .position
            .zip(self.bot.position)
            .map(|(a, b)| distance(a, b));
        self.players.insert(player.uuid, player);
        self.touch();
    }
    pub fn remove_player(&mut self, uuid: Uuid) {
        self.players.remove(&uuid);
        self.touch();
    }
    pub fn upsert_entity(&mut self, mut entity: EntitySnapshot) {
        if entity.alive == Some(false) || entity.health.is_some_and(|health| health <= 0.0) {
            self.entities.remove(&entity.entity_id);
            return;
        }
        if self
            .bot
            .position
            .is_some_and(|p| distance(p, entity.position) > self.entity_radius)
        {
            self.entities.remove(&entity.entity_id);
            return;
        }
        entity.distance = self
            .bot
            .position
            .map_or(entity.distance, |p| distance(p, entity.position));
        self.entities.insert(entity.entity_id, entity);
        self.trim_entities();
        self.touch();
    }
    pub fn remove_entity(&mut self, id: u32) {
        self.entities.remove(&id);
        self.dropped_items.remove(&id);
        self.touch();
    }
    pub fn remove_entities_not_in(&mut self, existing_ids: &std::collections::HashSet<u32>) {
        self.entities.retain(|id, _| existing_ids.contains(id));
        self.dropped_items.retain(|id, _| existing_ids.contains(id));
        self.touch();
    }
    pub fn remove_stale_entities(&mut self) {
        let now = SystemTime::now();
        self.entities.retain(|_, e| {
            now.duration_since(e.last_seen).unwrap_or_default() <= self.stale_entity_after
        });
        self.dropped_items.retain(|_, e| {
            now.duration_since(e.last_seen).unwrap_or_default() <= self.stale_entity_after
        });
        self.touch();
    }
    /// Atomically reconcile the item subset observed in the current ECS tick.
    pub fn replace_dropped_items(&mut self, items: Vec<DroppedItemObservation>) {
        let bot_position = self.bot.position;
        let bot_dimension = self.bot.dimension.as_deref();
        let mut items: Vec<_> = items
            .into_iter()
            .filter_map(|mut item| {
                if item.session_id != self.session_id
                    || bot_dimension.is_none_or(|dimension| item.dimension != dimension)
                {
                    return None;
                }
                if let Some(origin) = bot_position {
                    item.distance = distance(origin, item.position);
                }
                (item.distance <= self.entity_radius).then_some(item)
            })
            .collect();
        items.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then_with(|| a.entity_id.cmp(&b.entity_id))
        });
        items.truncate(self.maximum_entities);
        self.dropped_items = items
            .into_iter()
            .map(|item| (item.entity_id, item))
            .collect();
        self.touch();
    }

    pub fn session_id(&self) -> u64 {
        self.session_id
    }
    fn trim_entities(&mut self) {
        if self.entities.len() > self.maximum_entities {
            let mut ids: Vec<_> = self
                .entities
                .values()
                .map(|e| (e.distance, e.entity_id))
                .collect();
            ids.sort_by(|a, b| a.0.total_cmp(&b.0));
            let keep: std::collections::HashSet<_> = ids
                .into_iter()
                .take(self.maximum_entities)
                .map(|(_, id)| id)
                .collect();
            self.entities.retain(|id, _| keep.contains(id));
        }
    }
    pub fn record_received(
        &mut self,
        kind: ChatMessageKind,
        sender: Option<String>,
        text: String,
    ) -> bool {
        self.record_received_from(kind, None, sender, text)
    }
    pub fn record_received_from(
        &mut self,
        kind: ChatMessageKind,
        sender_uuid: Option<Uuid>,
        sender: Option<String>,
        text: String,
    ) -> bool {
        let now = SystemTime::now();
        self.last_received_chat = Some(ChatRecord {
            kind,
            sender_uuid,
            sender: sender.clone(),
            text: text.clone(),
            timestamp: now,
        });
        if kind == ChatMessageKind::Player {
            // Keep event handling bounded and non-blocking. AI consumes this
            // queue; last_received_chat remains a diagnostic snapshot.
            self.incoming_player_chat.push_back(ChatRecord {
                kind,
                sender_uuid,
                sender: sender.clone(),
                text: text.clone(),
                timestamp: now,
            });
            if self.incoming_player_chat.len() > self.incoming_player_chat_capacity {
                self.incoming_player_chat.pop_front();
            }
        }
        let signature = format!("{kind:?}|{sender:?}|{text}");
        let duplicate = self
            .last_displayed_signature
            .as_ref()
            .is_some_and(|(previous, at)| {
                previous == &signature && at.elapsed() < Duration::from_millis(100)
            });
        self.last_displayed_signature = Some((signature, Instant::now()));
        self.touch();
        !duplicate
    }
    pub fn record_sent(&mut self, text: String) -> bool {
        let now = SystemTime::now();
        let duplicate = self.last_sent_chat.as_ref().is_some_and(|p| {
            p.text == text
                && now.duration_since(p.timestamp).unwrap_or(Duration::MAX)
                    < Duration::from_millis(100)
        });
        if !duplicate {
            self.last_sent_chat = Some(ChatRecord {
                kind: ChatMessageKind::Player,
                sender_uuid: None,
                sender: None,
                text,
                timestamp: now,
            });
            self.touch();
        }
        !duplicate
    }
    pub fn pop_incoming_player_chat(&mut self) -> Option<ChatRecord> {
        self.incoming_player_chat.pop_front()
    }
    pub fn set_incoming_player_chat_capacity(&mut self, capacity: usize) {
        self.incoming_player_chat_capacity = capacity.max(1);
        while self.incoming_player_chat.len() > self.incoming_player_chat_capacity {
            self.incoming_player_chat.pop_front();
        }
    }
    pub fn find_player_by_name(&self, name: &str) -> Option<&PlayerSnapshot> {
        self.players
            .values()
            .find(|p| p.username.eq_ignore_ascii_case(name))
    }
    pub fn find_player_by_uuid(&self, uuid: Uuid) -> Option<&PlayerSnapshot> {
        self.players.get(&uuid)
    }
    pub fn nearest_player(&self) -> Option<&PlayerSnapshot> {
        self.players
            .values()
            .filter(|p| p.distance.is_some())
            .min_by(|a, b| a.distance.unwrap().total_cmp(&b.distance.unwrap()))
    }
    pub fn set_current_task(&mut self, task: TaskSnapshot) {
        self.current_task = Some(task);
        self.touch();
    }
    pub fn clear_current_task(&mut self) {
        self.current_task = None;
        self.touch();
    }
    pub fn set_movement(&mut self, movement: MovementSnapshot) {
        self.movement = movement;
        self.touch();
    }
    #[must_use]
    pub fn snapshot(&self) -> WorldStateSnapshot {
        let mut players: Vec<_> = self.players.values().cloned().collect();
        players.sort_by(|a, b| {
            a.distance
                .unwrap_or(f64::MAX)
                .total_cmp(&b.distance.unwrap_or(f64::MAX))
        });
        let mut entities: Vec<_> = self.entities.values().cloned().collect();
        entities.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        let mut dropped_items: Vec<_> = self.dropped_items.values().cloned().collect();
        dropped_items.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then_with(|| a.entity_id.cmp(&b.entity_id))
        });
        WorldStateSnapshot {
            session_id: self.session_id,
            connection: self.connection.clone(),
            bot: self.bot.clone(),
            inventory: self.inventory.clone(),
            container: self.container.snapshot(),
            players,
            entities,
            dropped_items,
            last_received_chat: self.last_received_chat.clone(),
            last_sent_chat: self.last_sent_chat.clone(),
            current_task: self.current_task.clone(),
            movement: self.movement.clone(),
            last_updated_at: self.last_updated_at,
        }
    }
}
pub fn validate_limits(
    radius: f64,
    maximum_entities: usize,
    stale_seconds: u64,
) -> Result<(), AppError> {
    if !(radius > 0.0 && radius <= 256.0) {
        return Err(AppError::InvalidWorldStateConfiguration(
            "nearby_entity_radius must be greater than zero and at most 256".into(),
        ));
    }
    if maximum_entities == 0 || maximum_entities > 4096 {
        return Err(AppError::InvalidWorldStateConfiguration(
            "maximum_tracked_entities must be between 1 and 4096".into(),
        ));
    }
    if stale_seconds == 0 || stale_seconds > 3600 {
        return Err(AppError::InvalidWorldStateConfiguration(
            "stale_entity_seconds must be between 1 and 3600".into(),
        ));
    }
    Ok(())
}
fn distance(a: PositionSnapshot, b: PositionSnapshot) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    fn uuid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }
    fn player(n: u128, name: &str, x: f64) -> PlayerSnapshot {
        PlayerSnapshot {
            uuid: uuid(n),
            username: name.into(),
            position: Some(PositionSnapshot { x, y: 0., z: 0. }),
            distance: None,
            dimension: Some("minecraft:overworld".into()),
            loaded: true,
            last_seen: SystemTime::now(),
        }
    }
    fn entity(id: u32, x: f64) -> EntitySnapshot {
        EntitySnapshot {
            entity_id: id,
            uuid: Some(uuid(u128::from(id))),
            entity_type: "minecraft:pig".into(),
            item_id: None,
            item_count: 0,
            dimension: Some("minecraft:overworld".into()),
            position: PositionSnapshot { x, y: 0., z: 0. },
            distance: 0.,
            alive: Some(true),
            health: Some(20.0),
            custom_name: None,
            item: None,
            last_seen: SystemTime::now(),
        }
    }

    #[test]
    fn default_state_is_empty_and_bounded() {
        let state = WorldState::default();
        let snapshot = state.snapshot();
        assert!(!snapshot.connection.joined_world);
        assert!(snapshot.players.is_empty());
        assert!(snapshot.entities.is_empty());
        assert_eq!(state.maximum_entities, 256);
    }
    #[test]
    fn connection_and_bot_updates_are_snapshot_safe() {
        let mut state = WorldState::default();
        state.set_connection(WorldConnectionState::Connected);
        state.set_joined_world(true);
        state.update_bot(BotSnapshot {
            position: Some(PositionSnapshot {
                x: 1.9,
                y: -2.1,
                z: 3.,
            }),
            ..Default::default()
        });
        let snapshot = state.snapshot();
        assert_eq!(snapshot.connection.state, WorldConnectionState::Connected);
        assert_eq!(
            snapshot.bot.block_position,
            Some(BlockPosition { x: 1, y: -3, z: 3 })
        );
    }
    #[test]
    fn inventory_groups_counts_and_finds_selected_item() {
        let mut state = WorldState::default();
        state.update_inventory(InventorySnapshot {
            available: true,
            revision: 0,
            selected_hotbar_slot: Some(1),
            slots: vec![
                InventorySlot {
                    slot: 1,
                    item_id: Some("minecraft:stone".into()),
                    display_name: None,
                    count: 3,
                },
                InventorySlot {
                    slot: 4,
                    item_id: Some("minecraft:stone".into()),
                    display_name: None,
                    count: 7,
                },
            ],
            total_counts: Default::default(),
        });
        assert_eq!(state.inventory.count_item("minecraft:stone"), 10);
        assert!(state.inventory.has_item("minecraft:stone", 10));
        assert_eq!(state.inventory.find_slots("minecraft:stone").len(), 2);
        assert_eq!(state.inventory.selected_item().unwrap().count, 3);
    }
    #[test]
    fn empty_inventory_is_supported() {
        let mut state = WorldState::default();
        state.update_inventory(InventorySnapshot::default());
        assert_eq!(state.inventory.count_item("minecraft:air"), 0);
        assert!(state.inventory.selected_item().is_none());
    }
    #[test]
    fn player_lookup_is_case_insensitive_and_uuid_based() {
        let mut state = WorldState::default();
        let p = player(1, "Alex", 2.);
        state.upsert_player(p.clone());
        assert_eq!(state.find_player_by_name("aLeX").unwrap().username, "Alex");
        assert_eq!(state.find_player_by_uuid(uuid(1)).unwrap().username, "Alex");
    }
    #[test]
    fn loaded_player_lookup_prefers_exact_case_over_case_insensitive() {
        let mut low = player(1, "steve", 0.);
        low.loaded = true;
        let mut high = player(2, "Steve", 0.);
        high.loaded = true;
        let snapshot = WorldStateSnapshot {
            players: vec![low, high],
            ..Default::default()
        };
        assert_eq!(
            snapshot.find_loaded_player_by_name("Steve").unwrap().uuid,
            uuid(2)
        );
    }
    #[test]
    fn loaded_player_lookup_falls_back_to_case_insensitive_match() {
        let snapshot = WorldStateSnapshot {
            players: vec![player(1, "5cat", 0.)],
            ..Default::default()
        };
        assert_eq!(
            snapshot
                .find_loaded_player_by_name("5CAT")
                .unwrap()
                .username,
            "5cat"
        );
    }
    #[test]
    fn loaded_player_lookup_ignores_unloaded_players() {
        let mut unloaded = player(1, "Ghost", 0.);
        unloaded.loaded = false;
        let snapshot = WorldStateSnapshot {
            players: vec![unloaded],
            ..Default::default()
        };
        assert!(snapshot.find_loaded_player_by_name("Ghost").is_none());
    }
    #[test]
    fn nearest_player_and_entities_are_sorted_and_filtered() {
        let mut state = WorldState::default();
        state.update_bot(BotSnapshot {
            position: Some(PositionSnapshot::default()),
            ..Default::default()
        });
        state.upsert_player(player(1, "Far", 10.));
        state.upsert_player(player(2, "Near", 2.));
        state.upsert_entity(entity(1, 4.));
        state.upsert_entity(entity(2, 1.));
        let snapshot = state.snapshot();
        assert_eq!(state.nearest_player().unwrap().username, "Near");
        assert_eq!(snapshot.entities[0].entity_id, 2);
        assert_eq!(snapshot.players[0].username, "Near");
    }
    #[test]
    fn entity_radius_limit_and_maximum_are_enforced() {
        let mut state = WorldState::with_limits(5., 2, 30).unwrap();
        state.update_bot(BotSnapshot {
            position: Some(PositionSnapshot::default()),
            ..Default::default()
        });
        state.upsert_entity(entity(1, 1.));
        state.upsert_entity(entity(2, 2.));
        state.upsert_entity(entity(3, 3.));
        state.upsert_entity(entity(4, 8.));
        assert_eq!(state.snapshot().entities.len(), 2);
        assert!(state.snapshot().entities.iter().all(|e| e.distance <= 5.));
    }
    #[test]
    fn stale_entities_are_removed() {
        let mut state = WorldState::with_limits(64., 256, 1).unwrap();
        let mut old = entity(1, 1.);
        old.last_seen = SystemTime::now() - Duration::from_secs(2);
        state.entities.insert(1, old);
        state.remove_stale_entities();
        assert!(state.snapshot().entities.is_empty());
    }

    #[test]
    fn despawn_removes_entity_immediately() {
        let mut state = WorldState::default();
        state.upsert_entity(entity(7, 1.));
        state.remove_entity(7);
        assert!(state.snapshot().entities.is_empty());
    }

    #[test]
    fn death_removes_entity_immediately() {
        let mut state = WorldState::default();
        state.upsert_entity(entity(7, 1.));
        let mut dead = entity(7, 1.);
        dead.alive = Some(false);
        state.upsert_entity(dead);
        assert!(state.snapshot().entities.is_empty());
    }

    #[test]
    fn zero_health_removes_entity_immediately() {
        let mut state = WorldState::default();
        state.upsert_entity(entity(7, 1.));
        let mut dead = entity(7, 1.);
        dead.health = Some(0.);
        state.upsert_entity(dead);
        assert!(state.snapshot().entities.is_empty());
    }

    #[test]
    fn missing_entity_is_removed_by_stale_refresh_fallback() {
        let mut state = WorldState::default();
        state.upsert_entity(entity(7, 1.));
        state.remove_entities_not_in(&HashSet::new());
        assert!(state.snapshot().entities.is_empty());
    }

    #[test]
    fn entity_id_reuse_replaces_the_complete_snapshot() {
        let mut state = WorldState::default();
        state.upsert_entity(entity(7, 1.));
        let mut replacement = entity(7, 2.);
        replacement.uuid = Some(uuid(999));
        replacement.entity_type = "minecraft:cow".into();
        replacement.health = Some(10.);
        state.upsert_entity(replacement);
        let snapshot = state.snapshot();
        assert_eq!(snapshot.entities.len(), 1);
        assert_eq!(snapshot.entities[0].entity_type, "minecraft:cow");
        assert_eq!(snapshot.entities[0].uuid, Some(uuid(999)));
        assert_eq!(snapshot.entities[0].position.x, 2.);
        assert_eq!(snapshot.entities[0].health, Some(10.));
    }
    #[test]
    fn task_set_and_clear_are_exposed() {
        let mut state = WorldState::default();
        state.set_current_task(TaskSnapshot {
            name: "test".into(),
            started_at: SystemTime::now(),
        });
        assert_eq!(state.snapshot().current_task.unwrap().name, "test");
        state.clear_current_task();
        assert!(state.snapshot().current_task.is_none());
    }

    #[test]
    fn session_reset_drops_stale_bot_and_task_state() {
        let mut state = WorldState::default();
        state.update_bot(BotSnapshot {
            position: Some(PositionSnapshot {
                x: 1.0,
                y: 64.0,
                z: 1.0,
            }),
            dimension: Some("minecraft:overworld".into()),
            ..BotSnapshot::default()
        });
        state.set_current_task(TaskSnapshot {
            name: "old".into(),
            started_at: SystemTime::now(),
        });
        state.clear_session_state();
        let snapshot = state.snapshot();
        assert!(snapshot.bot.position.is_none());
        assert!(snapshot.bot.dimension.is_none());
        assert!(snapshot.current_task.is_none());
    }
    #[test]
    fn snapshots_clone_without_internal_aliases() {
        let mut state = WorldState::default();
        state.upsert_player(player(1, "Alex", 1.));
        let mut snapshot = state.snapshot();
        snapshot.players[0].username = "Changed".into();
        assert_eq!(state.find_player_by_uuid(uuid(1)).unwrap().username, "Alex");
    }
    #[test]
    fn limits_validate() {
        assert!(validate_limits(0., 1, 1).is_err());
        assert!(validate_limits(1., 0, 1).is_err());
        assert!(validate_limits(1., 1, 0).is_err());
        assert!(validate_limits(64., 256, 30).is_ok());
    }

    #[test]
    fn incoming_player_chat_is_ordered_and_bounded() {
        let mut state = WorldState::default();
        state.set_incoming_player_chat_capacity(2);
        for text in ["!one", "!two", "!three"] {
            state.record_received_from(
                ChatMessageKind::Player,
                None,
                Some("Alex".into()),
                text.into(),
            );
        }
        assert_eq!(state.pop_incoming_player_chat().unwrap().text, "!two");
        assert_eq!(state.pop_incoming_player_chat().unwrap().text, "!three");
        assert!(state.pop_incoming_player_chat().is_none());
        state.record_received(ChatMessageKind::System, None, "system".into());
        assert!(state.pop_incoming_player_chat().is_none());
    }
}
