use super::*;
use crate::minecraft::world_state::{
    BotSnapshot, ConnectionSnapshot, EntitySnapshot, InventorySlot, MovementSnapshot,
};

fn world(items: &[(u32, &str, u32, f64)]) -> WorldStateSnapshot {
    let mut inv = InventorySnapshot {
        available: true,
        revision: 1,
        slots: (0..36)
            .map(|slot| InventorySlot {
                slot,
                item_id: None,
                display_name: None,
                count: 0,
            })
            .collect(),
        selected_hotbar_slot: None,
        total_counts: HashMap::new(),
    };
    inv.total_counts.clear();
    WorldStateSnapshot {
        connection: ConnectionSnapshot {
            state: WorldConnectionState::Connected,
            joined_world: true,
            ..Default::default()
        },
        bot: BotSnapshot {
            position: Some(PositionSnapshot::default()),
            dimension: Some("minecraft:overworld".into()),
            alive: Some(true),
            ..Default::default()
        },
        inventory: inv,
        entities: items
            .iter()
            .map(|(id, item, count, x)| EntitySnapshot {
                entity_id: *id,
                uuid: None,
                entity_type: "minecraft:item".into(),
                item_id: Some((*item).into()),
                item_count: *count,
                dimension: Some("minecraft:overworld".into()),
                position: PositionSnapshot {
                    x: *x,
                    y: 0.,
                    z: 0.,
                },
                distance: *x,
                alive: Some(true),
                health: None,
                custom_name: None,
                item: Some(crate::minecraft::world_state::DroppedItemSnapshot {
                    item_id: (*item).into(),
                    count: *count,
                }),
                last_seen: SystemTime::now(),
            })
            .collect(),
        players: vec![],
        last_received_chat: None,
        last_sent_chat: None,
        current_task: None,
        movement: MovementSnapshot::default(),
        last_updated_at: SystemTime::now(),
        ..Default::default()
    }
}
fn req() -> CollectRequest {
    CollectRequest::exact("minecraft:diamond".into(), 3)
}

#[test]
fn exact_set_and_group_matching() {
    assert!(req().filter.matches("minecraft:diamond"));
    assert!(ItemFilter::AnyOf(vec!["a".into(), "b".into()]).matches("b"));
    assert!(ItemFilter::Group(ItemGroup::Ores).matches("minecraft:raw_iron"));
    assert!(!ItemFilter::Group(ItemGroup::Logs).matches("minecraft:diamond"));
}
#[test]
fn ranking_is_safe_then_distance_then_stable_id() {
    let now = Instant::now();
    let w = world(&[
        (9, "minecraft:diamond", 64, 1.),
        (2, "minecraft:diamond", 1, 3.),
        (1, "minecraft:diamond", 1, 3.),
    ]);
    let mut w = w;
    let observed = SystemTime::now();
    for entity in &mut w.entities {
        entity.last_seen = observed;
    }
    let mut safety = HashMap::new();
    safety.insert(9, TargetSafety::Unknown);
    safety.insert(2, TargetSafety::Safe);
    safety.insert(1, TargetSafety::Safe);
    let mut c = DropCollector::new(Default::default());
    c.start(req(), now);
    assert_eq!(
        c.tick(&w, &safety, &HashSet::new(), false, now),
        CollectDirective::Navigate(PositionSnapshot {
            x: 3.,
            y: 0.,
            z: 0.
        })
    );
    assert_eq!(c.status().1, Some(1));
}
#[test]
fn unsafe_and_unreachable_terminate() {
    let now = Instant::now();
    let w = world(&[(1, "minecraft:diamond", 1, 2.)]);
    let mut c = DropCollector::new(Default::default());
    c.start(req(), now);
    let safety = HashMap::from([(1, TargetSafety::Unsafe)]);
    assert!(matches!(
        c.tick(&w, &safety, &HashSet::new(), false, now),
        CollectDirective::Finished(CollectResult {
            outcome: CollectOutcome::AllUnsafe,
            ..
        })
    ));
}
#[test]
fn capacity_counts_empty_and_partial_slots() {
    let mut w = world(&[]);
    w.inventory.slots.truncate(2);
    w.inventory.slots[0] = InventorySlot {
        slot: 0,
        item_id: Some("minecraft:diamond".into()),
        display_name: None,
        count: 60,
    };
    assert_eq!(inventory_capacity(&w.inventory, &req().filter), 68);
}
#[test]
fn moving_target_replans_are_bounded() {
    let now = Instant::now();
    let mut cfg = CollectorConfig::default();
    cfg.minimum_replan_interval = Duration::ZERO;
    cfg.maximum_replans = 1;
    let mut c = DropCollector::new(cfg);
    c.start(req(), now);
    let mut w = world(&[(1, "minecraft:diamond", 3, 3.)]);
    c.tick(&w, &HashMap::new(), &HashSet::new(), false, now);
    w.entities[0].position.x = 5.;
    assert!(matches!(
        c.tick(&w, &HashMap::new(), &HashSet::new(), false, now),
        CollectDirective::Navigate(_)
    ));
    w.entities[0].position.x = 7.;
    assert!(matches!(
        c.tick(&w, &HashMap::new(), &HashSet::new(), false, now),
        CollectDirective::Finished(_)
    ));
}
#[test]
fn disappearance_requires_inventory_revision_and_delta() {
    let now = Instant::now();
    let mut c = DropCollector::new(Default::default());
    c.start(req(), now);
    let mut w = world(&[(1, "minecraft:diamond", 3, 2.)]);
    c.tick(&w, &HashMap::new(), &HashSet::new(), false, now);
    w.entities.clear();
    w.inventory.revision = 2;
    w.inventory
        .total_counts
        .insert("minecraft:diamond".into(), 3);
    assert!(matches!(
        c.tick(&w, &HashMap::new(), &HashSet::new(), false, now),
        CollectDirective::Finished(CollectResult {
            outcome: CollectOutcome::Completed,
            collected: 3,
            ..
        })
    ));
}
#[test]
fn despawn_without_delta_is_lost_and_bounded() {
    let now = Instant::now();
    let mut c = DropCollector::new(Default::default());
    c.start(req(), now);
    let mut w = world(&[(1, "minecraft:diamond", 3, 2.)]);
    c.tick(&w, &HashMap::new(), &HashSet::new(), false, now);
    w.entities.clear();
    assert!(matches!(
        c.tick(&w, &HashMap::new(), &HashSet::new(), false, now),
        CollectDirective::Finished(CollectResult {
            outcome: CollectOutcome::ItemLost,
            entities_lost: 1,
            ..
        })
    ));
}
#[test]
fn cancellation_death_disconnect_and_timeout_finish() {
    for outcome in [
        CollectOutcome::Cancelled,
        CollectOutcome::Died,
        CollectOutcome::Disconnected,
        CollectOutcome::TimedOut,
    ] {
        let now = Instant::now();
        let mut c = DropCollector::new(Default::default());
        let mut r = req();
        r.timeout = Duration::from_millis(1);
        c.start(r, now);
        let mut w = world(&[(1, "minecraft:diamond", 3, 2.)]);
        match outcome {
            CollectOutcome::Cancelled => c.cancel(),
            CollectOutcome::Died => w.bot.alive = Some(false),
            CollectOutcome::Disconnected => w.connection.state = WorldConnectionState::Disconnected,
            CollectOutcome::TimedOut => {}
            _ => {}
        };
        let at = if outcome == CollectOutcome::TimedOut {
            now + Duration::from_millis(2)
        } else {
            now
        };
        assert!(
            matches!(c.tick(&w,&HashMap::new(),&HashSet::new(),false,at),CollectDirective::Finished(CollectResult{outcome:o,..}) if o==outcome)
        );
    }
}
#[test]
fn path_failure_uses_failure_memory_and_next_candidate() {
    let now = Instant::now();
    let mut c = DropCollector::new(Default::default());
    c.start(req(), now);
    let w = world(&[
        (1, "minecraft:diamond", 3, 1.),
        (2, "minecraft:diamond", 3, 2.),
    ]);
    c.tick(&w, &HashMap::new(), &HashSet::new(), false, now);
    c.tick(&w, &HashMap::new(), &HashSet::new(), true, now);
    assert_eq!(c.status().1, Some(2));
}
