//! Application-owned, bounded Minecraft state.

use std::{
    collections::HashMap,
    time::{Duration, Instant, SystemTime},
};

use uuid::Uuid;

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

/// Stable application regions. These do not change when another menu is open.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InventoryRegion {
    CraftResult,
    Crafting,
    Armor,
    Main,
    Hotbar,
    Offhand,
}

/// Stable player-inventory slot ID. Its numeric value is deliberately the
/// vanilla/Azalea `Menu::Player` slot: result 0, craft 1..=4, armor 5..=8
/// (head to feet), main 9..=35, hotbar 36..=44, and offhand 45.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InventorySlotId(pub u8);
impl InventorySlotId {
    pub const SLOT_COUNT: usize = 46;
    pub const STORAGE_CAPACITY: usize = 36;
    pub fn new(value: usize) -> Option<Self> {
        (value < Self::SLOT_COUNT).then_some(Self(value as u8))
    }
    pub fn region(self) -> InventoryRegion {
        match self.0 {
            0 => InventoryRegion::CraftResult,
            1..=4 => InventoryRegion::Crafting,
            5..=8 => InventoryRegion::Armor,
            9..=35 => InventoryRegion::Main,
            36..=44 => InventoryRegion::Hotbar,
            45 => InventoryRegion::Offhand,
            _ => unreachable!("validated inventory slot ID"),
        }
    }
    pub fn hotbar(index: u8) -> Option<Self> {
        (index < 9).then_some(Self(36 + index))
    }
    pub fn is_storage(self) -> bool {
        matches!(
            self.region(),
            InventoryRegion::Main | InventoryRegion::Hotbar
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemMetadata {
    /// Effective values include Azalea's item defaults, not only server patches.
    pub custom_name: Option<String>,
    pub item_name: Option<String>,
    pub damage: Option<u32>,
    pub maximum_durability: Option<u32>,
    pub remaining_durability: Option<u32>,
    pub enchantments: Vec<(String, u32)>,
    /// JSON representation of the server-provided component patch. Azalea does
    /// not expose a lossless, stable application-level component iterator.
    pub component_patch: Option<String>,
    /// Kind + component patch identity; count is intentionally excluded.
    pub stack_identity: Option<String>,
    pub maximum_stack_size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventorySlot {
    pub id: InventorySlotId,
    /// Verified Azalea `Menu::Player` index retained for diagnostics.
    pub azalea_menu_slot: usize,
    pub region: InventoryRegion,
    pub item_id: Option<String>,
    pub display_name: Option<String>,
    pub count: u32,
    pub metadata: ItemMetadata,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InventorySyncState {
    #[default]
    Unavailable,
    Synchronized,
}
#[derive(Clone, Debug, Default)]
pub struct InventorySnapshot {
    pub available: bool,
    pub sync_state: InventorySyncState,
    pub revision: u64,
    pub server_state_id: Option<u32>,
    pub slots: Vec<InventorySlot>,
    pub selected_hotbar_slot: Option<u8>,
    pub selected_slot: Option<InventorySlotId>,
    pub held_item: Option<InventorySlot>,
    pub total_counts: HashMap<String, u32>,
    pub used_storage_slots: usize,
    pub storage_capacity: usize,
    pub empty_storage_slots: usize,
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
        self.selected_slot
            .and_then(|id| self.slots.iter().find(|s| s.id == id))
    }
    fn rebuild_counts(&mut self) {
        self.total_counts.clear();
        for slot in &self.slots {
            if let Some(id) = &slot.item_id {
                *self.total_counts.entry(id.clone()).or_default() += slot.count;
            }
        }
        self.selected_slot = self.selected_hotbar_slot.and_then(InventorySlotId::hotbar);
        self.held_item = self
            .selected_item()
            .filter(|slot| slot.item_id.is_some())
            .cloned();
        self.storage_capacity = InventorySlotId::STORAGE_CAPACITY;
        self.used_storage_slots = self
            .slots
            .iter()
            .filter(|s| s.id.is_storage() && s.item_id.is_some())
            .count();
        self.empty_storage_slots = self
            .storage_capacity
            .saturating_sub(self.used_storage_slots);
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
#[derive(Clone, Debug)]
pub struct TaskSnapshot {
    pub name: String,
    pub id: String,
    pub status: String,
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
}

#[derive(Clone, Debug)]
pub struct WorldStateSnapshot {
    pub connection: ConnectionSnapshot,
    pub bot: BotSnapshot,
    pub inventory: InventorySnapshot,
    pub players: Vec<PlayerSnapshot>,
    pub entities: Vec<EntitySnapshot>,
    pub last_received_chat: Option<ChatRecord>,
    pub last_sent_chat: Option<ChatRecord>,
    pub current_task: Option<TaskSnapshot>,
    pub movement: MovementSnapshot,
    pub last_updated_at: SystemTime,
}
impl Default for WorldStateSnapshot {
    fn default() -> Self {
        Self {
            connection: ConnectionSnapshot::default(),
            bot: BotSnapshot::default(),
            inventory: InventorySnapshot::default(),
            players: Vec::new(),
            entities: Vec::new(),
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
}

#[derive(Debug)]
pub struct WorldState {
    pub(crate) connection: ConnectionSnapshot,
    bot: BotSnapshot,
    inventory: InventorySnapshot,
    players: HashMap<Uuid, PlayerSnapshot>,
    entities: HashMap<u32, EntitySnapshot>,
    last_received_chat: Option<ChatRecord>,
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
            connection: ConnectionSnapshot::default(),
            bot: BotSnapshot::default(),
            inventory: InventorySnapshot::default(),
            players: HashMap::new(),
            entities: HashMap::new(),
            last_received_chat: None,
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
        inventory.slots.sort_by_key(|s| s.id);
        inventory.revision = self.inventory.revision.saturating_add(1);
        inventory.rebuild_counts();
        self.inventory = inventory;
        self.touch();
    }
    pub fn clear_inventory(&mut self) {
        let revision = self.inventory.revision.saturating_add(1);
        self.inventory = InventorySnapshot {
            revision,
            ..Default::default()
        };
        self.touch();
    }
    /// Drop observations that belong to a disconnected Azalea session.
    pub fn clear_session_state(&mut self) {
        self.bot = BotSnapshot::default();
        self.players.clear();
        self.entities.clear();
        let revision = self.inventory.revision.saturating_add(1);
        self.inventory = InventorySnapshot {
            revision,
            ..Default::default()
        };
        self.movement = MovementSnapshot::default();
        self.current_task = None;
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
        self.touch();
    }
    pub fn remove_entities_not_in(&mut self, existing_ids: &std::collections::HashSet<u32>) {
        self.entities.retain(|id, _| existing_ids.contains(id));
        self.touch();
    }
    pub fn remove_stale_entities(&mut self) {
        let now = SystemTime::now();
        self.entities.retain(|_, e| {
            now.duration_since(e.last_seen).unwrap_or_default() <= self.stale_entity_after
        });
        self.touch();
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
        WorldStateSnapshot {
            connection: self.connection.clone(),
            bot: self.bot.clone(),
            inventory: self.inventory.clone(),
            players,
            entities,
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
            selected_hotbar_slot: Some(1),
            slots: vec![
                InventorySlot {
                    id: InventorySlotId(37),
                    azalea_menu_slot: 37,
                    region: InventoryRegion::Hotbar,
                    item_id: Some("minecraft:stone".into()),
                    display_name: None,
                    count: 3,
                    metadata: ItemMetadata::default(),
                },
                InventorySlot {
                    id: InventorySlotId(9),
                    azalea_menu_slot: 9,
                    region: InventoryRegion::Main,
                    item_id: Some("minecraft:stone".into()),
                    display_name: None,
                    count: 7,
                    metadata: ItemMetadata::default(),
                },
            ],
            ..Default::default()
        });
        assert_eq!(state.inventory.count_item("minecraft:stone"), 10);
        assert!(state.inventory.has_item("minecraft:stone", 10));
        assert_eq!(state.inventory.find_slots("minecraft:stone").len(), 2);
        assert_eq!(state.inventory.selected_item().unwrap().count, 3);
        assert_eq!(state.inventory.selected_slot, Some(InventorySlotId(37)));
        assert_eq!(state.inventory.used_storage_slots, 2);
        assert_eq!(state.inventory.empty_storage_slots, 34);
    }
    #[test]
    fn player_slot_mapping_covers_every_region_and_fixes_hotbar_offset() {
        assert_eq!(
            InventorySlotId::new(0).unwrap().region(),
            InventoryRegion::CraftResult
        );
        assert_eq!(
            InventorySlotId::new(1).unwrap().region(),
            InventoryRegion::Crafting
        );
        assert_eq!(
            InventorySlotId::new(5).unwrap().region(),
            InventoryRegion::Armor
        );
        assert_eq!(
            InventorySlotId::new(9).unwrap().region(),
            InventoryRegion::Main
        );
        assert_eq!(InventorySlotId::hotbar(0), Some(InventorySlotId(36)));
        assert_eq!(InventorySlotId::hotbar(8), Some(InventorySlotId(44)));
        assert_eq!(
            InventorySlotId::new(45).unwrap().region(),
            InventoryRegion::Offhand
        );
        assert_eq!(InventorySlotId::hotbar(9), None);
        assert_eq!(InventorySlotId::new(46), None);
    }
    #[test]
    fn inventory_revisions_and_disconnect_invalidation_are_monotonic() {
        let mut state = WorldState::default();
        state.update_inventory(InventorySnapshot {
            available: true,
            ..Default::default()
        });
        assert_eq!(state.inventory.revision, 1);
        state.clear_session_state();
        assert_eq!(state.inventory.revision, 2);
        assert!(!state.inventory.available);
        assert_eq!(state.inventory.sync_state, InventorySyncState::Unavailable);
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
            id: "1".into(),
            status: "queued".into(),
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
            id: "1".into(),
            status: "running".into(),
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
}
