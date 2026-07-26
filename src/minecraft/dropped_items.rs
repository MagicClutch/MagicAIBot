//! Pure queries over immutable, session-scoped dropped-item observations.

use std::{
    collections::BTreeSet,
    time::{Duration, SystemTime},
};

use serde_json::Value;
use uuid::Uuid;

use super::world_state::PositionSnapshot;

/// Everything about a stack exposed by the pinned Azalea `ItemItem` metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct ObservableItemStack {
    pub item_id: String,
    pub count: u32,
    /// Azalea's serialized component patch (damage, name, enchantments, etc.).
    pub components: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DroppedItemObservation {
    pub session_id: u64,
    pub entity_id: u32,
    pub uuid: Option<Uuid>,
    pub stack: ObservableItemStack,
    pub position: PositionSnapshot,
    pub distance: f64,
    pub dimension: String,
    pub last_seen: SystemTime,
}

#[derive(Clone, Debug, Default)]
pub struct DroppedItemQuery {
    pub entity_id: Option<u32>,
    pub item_ids: Option<BTreeSet<String>>,
    pub radius: Option<f64>,
    pub dimension: Option<String>,
    pub maximum_age: Option<Duration>,
    pub limit: Option<usize>,
}

impl DroppedItemQuery {
    pub fn exact_item(item_id: impl Into<String>) -> Self {
        Self {
            item_ids: Some([item_id.into()].into()),
            ..Self::default()
        }
    }

    pub fn item_set(item_ids: impl IntoIterator<Item = String>) -> Self {
        Self {
            item_ids: Some(item_ids.into_iter().collect()),
            ..Self::default()
        }
    }

    /// Groups are resolved by the caller/configuration and use the same exact set semantics.
    pub fn item_group(item_ids: impl IntoIterator<Item = String>) -> Self {
        Self::item_set(item_ids)
    }

    pub fn entity(entity_id: u32) -> Self {
        Self {
            entity_id: Some(entity_id),
            ..Self::default()
        }
    }

    pub fn within_radius(radius: f64) -> Self {
        Self {
            radius: Some(radius),
            ..Self::default()
        }
    }

    /// Searches one consistent snapshot. Results rank by distance, then larger stack, then ID.
    #[must_use]
    pub fn search<'a>(
        &self,
        observations: &'a [DroppedItemObservation],
        session_id: u64,
        now: SystemTime,
    ) -> Vec<&'a DroppedItemObservation> {
        if self.radius.is_some_and(|r| !r.is_finite() || r < 0.0) || self.limit == Some(0) {
            return Vec::new();
        }
        let mut matches: Vec<_> = observations
            .iter()
            .filter(|item| {
                item.session_id == session_id
                    && self.entity_id.is_none_or(|id| item.entity_id == id)
                    && self
                        .item_ids
                        .as_ref()
                        .is_none_or(|ids| ids.contains(&item.stack.item_id))
                    && self.radius.is_none_or(|radius| item.distance <= radius)
                    && self
                        .dimension
                        .as_ref()
                        .is_none_or(|dimension| &item.dimension == dimension)
                    && self.maximum_age.is_none_or(|age| {
                        now.duration_since(item.last_seen).unwrap_or_default() <= age
                    })
            })
            .collect();
        matches.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then_with(|| b.stack.count.cmp(&a.stack.count))
                .then_with(|| a.entity_id.cmp(&b.entity_id))
        });
        matches.truncate(self.limit.unwrap_or(usize::MAX));
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::world_state::{BotSnapshot, WorldState};

    fn item(
        session: u64,
        id: u32,
        kind: &str,
        count: u32,
        x: f64,
        seen: SystemTime,
    ) -> DroppedItemObservation {
        DroppedItemObservation {
            session_id: session,
            entity_id: id,
            uuid: Some(Uuid::from_u128(u128::from(id))),
            stack: ObservableItemStack {
                item_id: kind.into(),
                count,
                components: serde_json::json!({"damage": id}),
            },
            position: PositionSnapshot { x, y: 64.0, z: 0.0 },
            distance: x.abs(),
            dimension: "minecraft:overworld".into(),
            last_seen: seen,
        }
    }

    #[test]
    fn movement_and_merge_replace_the_complete_observation() {
        let now = SystemTime::now();
        let mut state = WorldState::with_limits(64.0, 8, 30).unwrap();
        state.update_bot(BotSnapshot {
            position: Some(PositionSnapshot {
                x: 0.0,
                y: 64.0,
                z: 0.0,
            }),
            dimension: Some("minecraft:overworld".into()),
            ..Default::default()
        });
        state.replace_dropped_items(vec![item(0, 7, "minecraft:diamond", 2, 8.0, now)]);
        state.replace_dropped_items(vec![item(0, 7, "minecraft:diamond", 5, 3.0, now)]);
        let snapshot = state.snapshot();
        assert_eq!(snapshot.dropped_items.len(), 1);
        assert_eq!(snapshot.dropped_items[0].position.x, 3.0);
        assert_eq!(snapshot.dropped_items[0].stack.count, 5);
        assert_eq!(
            snapshot.dropped_items[0].stack.components,
            serde_json::json!({"damage": 7})
        );
    }

    #[test]
    fn despawn_and_ecs_absence_remove_items() {
        let now = SystemTime::now();
        let mut state = WorldState::default();
        state.replace_dropped_items(vec![item(0, 9, "minecraft:stick", 1, 1.0, now)]);
        state.remove_entity(9);
        assert!(state.snapshot().dropped_items.is_empty());
        state.replace_dropped_items(vec![item(0, 10, "minecraft:stick", 1, 1.0, now)]);
        state.replace_dropped_items(Vec::new());
        assert!(state.snapshot().dropped_items.is_empty());
    }

    #[test]
    fn snapshot_bounds_keep_nearest_with_stable_id_ties() {
        let now = SystemTime::now();
        let mut state = WorldState::with_limits(5.0, 2, 30).unwrap();
        state.update_bot(BotSnapshot {
            position: Some(PositionSnapshot {
                x: 0.0,
                y: 64.0,
                z: 0.0,
            }),
            dimension: Some("minecraft:overworld".into()),
            ..Default::default()
        });
        state.replace_dropped_items(vec![
            item(0, 3, "minecraft:stone", 1, 2.0, now),
            item(0, 2, "minecraft:stone", 1, 2.0, now),
            item(0, 1, "minecraft:stone", 1, 7.0, now),
        ]);
        let ids: Vec<_> = state
            .snapshot()
            .dropped_items
            .into_iter()
            .map(|i| i.entity_id)
            .collect();
        assert_eq!(ids, [2, 3]);
    }

    #[test]
    fn exact_set_group_radius_dimension_and_id_filters_are_pure() {
        let now = SystemTime::now();
        let items = vec![
            item(4, 1, "minecraft:diamond", 1, 4.0, now),
            item(4, 2, "minecraft:iron_ingot", 8, 4.0, now),
            item(4, 3, "minecraft:dirt", 64, 9.0, now),
        ];
        assert_eq!(
            DroppedItemQuery::exact_item("minecraft:diamond").search(&items, 4, now)[0].entity_id,
            1
        );
        let mut group = DroppedItemQuery::item_group([
            "minecraft:diamond".into(),
            "minecraft:iron_ingot".into(),
        ]);
        group.radius = Some(4.0);
        group.dimension = Some("minecraft:overworld".into());
        assert_eq!(
            group
                .search(&items, 4, now)
                .iter()
                .map(|i| i.entity_id)
                .collect::<Vec<_>>(),
            [2, 1]
        );
        assert_eq!(
            DroppedItemQuery::entity(3).search(&items, 4, now)[0]
                .stack
                .item_id,
            "minecraft:dirt"
        );
    }

    #[test]
    fn stale_records_and_other_sessions_are_isolated() {
        let now = SystemTime::now();
        let old = now - Duration::from_secs(31);
        let items = vec![
            item(1, 1, "minecraft:apple", 1, 1.0, old),
            item(2, 2, "minecraft:apple", 1, 1.0, now),
        ];
        let query = DroppedItemQuery {
            maximum_age: Some(Duration::from_secs(30)),
            ..DroppedItemQuery::exact_item("minecraft:apple")
        };
        assert!(query.search(&items, 1, now).is_empty());
        assert_eq!(query.search(&items, 2, now)[0].entity_id, 2);

        let mut state = WorldState::default();
        state.replace_dropped_items(vec![item(0, 4, "minecraft:apple", 1, 1.0, now)]);
        state.clear_session_state();
        assert_eq!(state.snapshot().session_id, 1);
        assert!(state.snapshot().dropped_items.is_empty());
        state.replace_dropped_items(vec![item(0, 5, "minecraft:apple", 1, 1.0, now)]);
        assert!(state.snapshot().dropped_items.is_empty());
    }
}
