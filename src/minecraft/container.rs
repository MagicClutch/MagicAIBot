//! Read-only observations of the server-owned container menu.
//!
//! This module intentionally contains no packet sending or click operations.  It
//! turns Azalea's active-menu ordering into an application-owned snapshot.

use std::time::SystemTime;

use super::world_state::{BlockPosition, InventorySlot};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainerLayout {
    Generic9xN { rows: u8 },
}

impl ContainerLayout {
    pub fn generic(rows: u8) -> Option<Self> {
        (1..=6).contains(&rows).then_some(Self::Generic9xN { rows })
    }

    pub fn container_slot_count(self) -> usize {
        match self {
            Self::Generic9xN { rows } => usize::from(rows) * 9,
        }
    }

    pub fn total_slot_count(self) -> usize {
        self.container_slot_count() + 36
    }

    /// Maps a server menu slot to the canonical player inventory index.
    ///
    /// Server order is container, main inventory (canonical 9..35), then
    /// hotbar (canonical 0..8). No sorting of either slot collection occurs.
    pub fn player_inventory_index(self, menu_slot: usize) -> Option<usize> {
        let offset = menu_slot.checked_sub(self.container_slot_count())?;
        match offset {
            0..=26 => Some(offset + 9),
            27..=35 => Some(offset - 27),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerIdentity {
    pub window_id: i32,
    pub menu_type: String,
    pub title: Option<String>,
    pub world_position: Option<BlockPosition>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainerSyncState {
    Synced,
    ExternalMutation,
    UnexpectedlyClosed,
    UnsupportedLayout,
    Died,
    Disconnected,
    StaleSession,
}

#[derive(Clone, Debug)]
pub struct ContainerSnapshot {
    pub identity: Option<ContainerIdentity>,
    pub layout: Option<ContainerLayout>,
    pub revision: Option<u32>,
    pub cursor: Option<InventorySlot>,
    pub container_slots: Vec<InventorySlot>,
    pub player_slots: Vec<InventorySlot>,
    pub is_open: bool,
    pub is_synced: bool,
    pub sync_state: ContainerSyncState,
    pub opened_at: Option<SystemTime>,
    pub observed_at: SystemTime,
    pub closed_at: Option<SystemTime>,
    pub session_generation: u64,
}

impl Default for ContainerSnapshot {
    fn default() -> Self {
        Self {
            identity: None,
            layout: None,
            revision: None,
            cursor: None,
            container_slots: Vec::new(),
            player_slots: Vec::new(),
            is_open: false,
            is_synced: true,
            sync_state: ContainerSyncState::Synced,
            opened_at: None,
            observed_at: SystemTime::UNIX_EPOCH,
            closed_at: None,
            session_generation: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MenuObservation {
    pub identity: ContainerIdentity,
    pub layout: Option<ContainerLayout>,
    pub revision: u32,
    pub cursor: InventorySlot,
    pub slots: Vec<InventorySlot>,
}

#[derive(Debug, Default)]
pub(crate) struct ContainerObserver {
    snapshot: ContainerSnapshot,
}

impl ContainerObserver {
    pub fn snapshot(&self) -> ContainerSnapshot {
        self.snapshot.clone()
    }

    pub fn begin_session(&mut self, now: SystemTime) {
        let generation = self.snapshot.session_generation.saturating_add(1);
        self.snapshot = ContainerSnapshot {
            session_generation: generation,
            observed_at: now,
            ..ContainerSnapshot::default()
        };
    }

    pub fn observe(&mut self, menu: Option<MenuObservation>, alive: bool, now: SystemTime) {
        let was_open = self.snapshot.is_open;
        let previous_identity = self.snapshot.identity.clone();
        let previous_revision = self.snapshot.revision;
        let previous_slots = (&self.snapshot.container_slots, &self.snapshot.player_slots);

        let Some(menu) = menu else {
            self.snapshot.observed_at = now;
            self.snapshot.cursor = None;
            self.snapshot.is_open = false;
            self.snapshot.is_synced = !was_open;
            if !alive {
                self.snapshot.sync_state = ContainerSyncState::Died;
            } else if was_open {
                self.snapshot.sync_state = ContainerSyncState::UnexpectedlyClosed;
                self.snapshot.closed_at = Some(now);
            }
            return;
        };

        let new_menu = previous_identity.as_ref() != Some(&menu.identity) || !was_open;
        let Some(layout) = menu.layout else {
            self.snapshot = ContainerSnapshot {
                identity: Some(menu.identity),
                revision: Some(menu.revision),
                cursor: Some(menu.cursor),
                is_open: true,
                is_synced: false,
                sync_state: ContainerSyncState::UnsupportedLayout,
                opened_at: Some(now),
                observed_at: now,
                session_generation: self.snapshot.session_generation,
                ..ContainerSnapshot::default()
            };
            return;
        };

        let expected = layout.total_slot_count();
        if menu.slots.len() != expected {
            self.snapshot = ContainerSnapshot {
                identity: Some(menu.identity),
                layout: Some(layout),
                revision: Some(menu.revision),
                cursor: Some(menu.cursor),
                is_open: true,
                is_synced: false,
                sync_state: ContainerSyncState::UnsupportedLayout,
                opened_at: Some(now),
                observed_at: now,
                session_generation: self.snapshot.session_generation,
                ..ContainerSnapshot::default()
            };
            return;
        }
        let split = layout.container_slot_count();
        let container_slots = menu.slots[..split].to_vec();
        let player_slots = menu.slots[split..]
            .iter()
            .enumerate()
            .map(|(offset, slot)| {
                let mut mapped = slot.clone();
                mapped.slot = layout
                    .player_inventory_index(split + offset)
                    .expect("validated layout");
                mapped
            })
            .collect::<Vec<_>>();
        let mutated = !new_menu
            && (previous_revision != Some(menu.revision)
                || previous_slots.0 != &container_slots
                || previous_slots.1 != &player_slots);
        self.snapshot = ContainerSnapshot {
            identity: Some(menu.identity),
            layout: Some(layout),
            revision: Some(menu.revision),
            cursor: Some(menu.cursor),
            container_slots,
            player_slots,
            is_open: true,
            is_synced: !mutated,
            sync_state: if mutated {
                ContainerSyncState::ExternalMutation
            } else {
                ContainerSyncState::Synced
            },
            opened_at: if new_menu {
                Some(now)
            } else {
                self.snapshot.opened_at
            },
            observed_at: now,
            closed_at: None,
            session_generation: self.snapshot.session_generation,
        };
    }

    pub fn disconnect(&mut self, now: SystemTime) {
        let was_open = self.snapshot.is_open;
        self.snapshot.is_open = false;
        self.snapshot.is_synced = false;
        self.snapshot.sync_state = ContainerSyncState::Disconnected;
        self.snapshot.closed_at = was_open.then_some(now).or(self.snapshot.closed_at);
        self.snapshot.observed_at = now;
    }

    pub fn snapshot_for_generation(&self, generation: u64) -> ContainerSnapshot {
        let mut snapshot = self.snapshot();
        if generation != snapshot.session_generation {
            snapshot.is_synced = false;
            snapshot.sync_state = ContainerSyncState::StaleSession;
        }
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(n: usize) -> InventorySlot {
        InventorySlot {
            slot: n,
            item_id: Some(format!("item:{n}")),
            display_name: None,
            count: 1,
        }
    }
    fn menu(rows: u8, revision: u32) -> MenuObservation {
        let layout = ContainerLayout::generic(rows).unwrap();
        MenuObservation {
            identity: ContainerIdentity {
                window_id: 7,
                menu_type: format!("generic_9x{rows}"),
                title: Some("Chest".into()),
                world_position: None,
            },
            layout: Some(layout),
            revision,
            cursor: slot(999),
            slots: (0..layout.total_slot_count()).map(slot).collect(),
        }
    }

    #[test]
    fn maps_single_chest_and_player_offsets_without_reordering_container() {
        let mut observer = ContainerObserver::default();
        observer.begin_session(SystemTime::UNIX_EPOCH);
        observer.observe(Some(menu(3, 1)), true, SystemTime::UNIX_EPOCH);
        let s = observer.snapshot();
        assert_eq!(
            s.container_slots.iter().map(|s| s.slot).collect::<Vec<_>>(),
            (0..27).collect::<Vec<_>>()
        );
        assert_eq!(
            s.player_slots.iter().map(|s| s.slot).collect::<Vec<_>>(),
            (9..36).chain(0..9).collect::<Vec<_>>()
        );
        assert!(s.is_open && s.is_synced);
    }
    #[test]
    fn maps_double_chest() {
        let mut o = ContainerObserver::default();
        o.begin_session(SystemTime::UNIX_EPOCH);
        o.observe(Some(menu(6, 2)), true, SystemTime::UNIX_EPOCH);
        assert_eq!(o.snapshot().container_slots.len(), 54);
        assert_eq!(o.snapshot().player_slots.len(), 36);
    }
    #[test]
    fn revision_change_is_external_mutation() {
        let mut o = ContainerObserver::default();
        o.begin_session(SystemTime::UNIX_EPOCH);
        o.observe(Some(menu(3, 1)), true, SystemTime::UNIX_EPOCH);
        o.observe(Some(menu(3, 2)), true, SystemTime::UNIX_EPOCH);
        assert_eq!(
            o.snapshot().sync_state,
            ContainerSyncState::ExternalMutation
        );
    }
    #[test]
    fn detects_closure_death_and_stale_session() {
        let mut o = ContainerObserver::default();
        o.begin_session(SystemTime::UNIX_EPOCH);
        let generation = o.snapshot().session_generation;
        o.observe(Some(menu(3, 1)), true, SystemTime::UNIX_EPOCH);
        o.observe(None, true, SystemTime::UNIX_EPOCH);
        assert_eq!(
            o.snapshot().sync_state,
            ContainerSyncState::UnexpectedlyClosed
        );
        o.begin_session(SystemTime::UNIX_EPOCH);
        assert_eq!(
            o.snapshot_for_generation(generation).sync_state,
            ContainerSyncState::StaleSession
        );
        o.observe(Some(menu(3, 1)), true, SystemTime::UNIX_EPOCH);
        o.observe(None, false, SystemTime::UNIX_EPOCH);
        assert_eq!(o.snapshot().sync_state, ContainerSyncState::Died);
    }
    #[test]
    fn rejects_unsupported_menu_and_bad_generic_size() {
        let mut o = ContainerObserver::default();
        o.begin_session(SystemTime::UNIX_EPOCH);
        let mut unsupported = menu(3, 1);
        unsupported.layout = None;
        o.observe(Some(unsupported), true, SystemTime::UNIX_EPOCH);
        assert_eq!(
            o.snapshot().sync_state,
            ContainerSyncState::UnsupportedLayout
        );
        let mut bad = menu(3, 1);
        bad.slots.pop();
        o.observe(Some(bad), true, SystemTime::UNIX_EPOCH);
        assert_eq!(
            o.snapshot().sync_state,
            ContainerSyncState::UnsupportedLayout
        );
        assert!(ContainerLayout::generic(7).is_none());
    }
    #[test]
    fn detects_disconnect() {
        let mut o = ContainerObserver::default();
        o.begin_session(SystemTime::UNIX_EPOCH);
        o.observe(Some(menu(6, 1)), true, SystemTime::UNIX_EPOCH);
        o.disconnect(SystemTime::UNIX_EPOCH);
        assert_eq!(o.snapshot().sync_state, ContainerSyncState::Disconnected);
        assert!(!o.snapshot().is_open);
    }
}
