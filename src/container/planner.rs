use super::model::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanError {
    ItemNotFound,
    DestinationFull,
    PartialOnly { available: u32 },
    InvalidCount,
}

/// Plans conservative pickup clicks. Slot numbers are protocol menu slots:
/// container slots first, followed by the 36 player inventory slots.
pub fn plan_transfer(
    menu: &MenuSnapshot,
    direction: TransferDirection,
    item: &str,
    count: u32,
    allow_partial: bool,
) -> Result<TransferPlan, PlanError> {
    if count == 0 {
        return Err(PlanError::InvalidCount);
    }
    let split = menu.container_slots.len();
    let (sources, destinations, source_offset, destination_offset) = match direction {
        TransferDirection::Take => (&menu.container_slots, &menu.player_slots, 0, split),
        TransferDirection::Store => (&menu.player_slots, &menu.container_slots, split, 0),
    };
    let available: u32 = sources
        .iter()
        .flatten()
        .filter(|s| s.item_id == item)
        .map(|s| s.count)
        .sum();
    if available == 0 {
        return Err(PlanError::ItemNotFound);
    }
    let item_max = sources
        .iter()
        .flatten()
        .find(|s| s.item_id == item)
        .map_or(64, |s| s.max_count);
    let capacity: u32 = destinations
        .iter()
        .map(|slot| match slot {
            None => item_max,
            Some(s) if s.item_id == item => s.max_count.saturating_sub(s.count),
            Some(_) => 0,
        })
        .sum();
    if capacity == 0 {
        return Err(PlanError::DestinationFull);
    }
    let feasible = count.min(available).min(capacity);
    if feasible < count && !allow_partial {
        return Err(PlanError::PartialOnly {
            available: feasible,
        });
    }
    let mut remaining = feasible;
    let mut destination_capacities: Vec<u32> = destinations
        .iter()
        .map(|slot| match slot {
            None => item_max,
            Some(s) if s.item_id == item => s.max_count.saturating_sub(s.count),
            Some(_) => 0,
        })
        .collect();
    let mut clicks = Vec::new();
    for (source_index, source) in sources.iter().enumerate() {
        let Some(source) = source.as_ref().filter(|s| s.item_id == item) else {
            continue;
        };
        let mut source_remaining = source.count;
        let mut from_stack = source.count.min(remaining);
        while from_stack > 0 {
            let Some(destination_index) = destination_capacities
                .iter()
                .position(|capacity| *capacity > 0)
            else {
                break;
            };
            let destination_capacity = destination_capacities[destination_index];
            let moving = from_stack.min(destination_capacity).min(remaining);
            let source_slot = source_offset + source_index;
            let destination_slot = destination_offset + destination_index;
            clicks.push(InventoryClick {
                slot: source_slot,
                button: ClickButton::Left,
            });
            if moving == source_remaining && moving <= destination_capacity {
                clicks.push(InventoryClick {
                    slot: destination_slot,
                    button: ClickButton::Left,
                });
            } else {
                clicks.extend((0..moving).map(|_| InventoryClick {
                    slot: destination_slot,
                    button: ClickButton::Right,
                }));
                clicks.push(InventoryClick {
                    slot: source_slot,
                    button: ClickButton::Left,
                });
            }
            source_remaining -= moving;
            from_stack -= moving;
            remaining -= moving;
            destination_capacities[destination_index] -= moving;
            if remaining == 0 {
                break;
            }
        }
        if remaining == 0 {
            break;
        }
    }
    Ok(TransferPlan {
        direction,
        item_id: item.into(),
        requested: count,
        planned: feasible - remaining,
        clicks,
        expected_window: menu.window_id,
        expected_revision: menu.revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stack(id: &str, n: u32) -> Option<StackSnapshot> {
        Some(StackSnapshot {
            item_id: id.into(),
            count: n,
            max_count: 64,
        })
    }
    fn chest(
        contents: Vec<Option<StackSnapshot>>,
        player: Vec<Option<StackSnapshot>>,
    ) -> MenuSnapshot {
        MenuSnapshot {
            kind: ContainerKind::SingleChest,
            position: None,
            window_id: 4,
            revision: 7,
            container_slots: contents,
            player_slots: player,
        }
    }
    #[test]
    fn maps_chest_and_player_slots() {
        let m = chest(vec![stack("x", 3); 27], vec![None; 36]);
        let p = plan_transfer(&m, TransferDirection::Take, "x", 3, false).unwrap();
        assert_eq!(p.clicks[0].slot, 0);
        assert_eq!(p.clicks[1].slot, 27);
    }
    #[test]
    fn exact_partial_stack_uses_right_clicks_and_returns_cursor() {
        let p = plan_transfer(
            &chest(vec![stack("x", 10)], vec![None]),
            TransferDirection::Take,
            "x",
            3,
            false,
        )
        .unwrap();
        assert_eq!(p.planned, 3);
        assert_eq!(p.clicks.len(), 5);
        assert_eq!(p.clicks.last().unwrap().slot, 0);
    }
    #[test]
    fn full_stack_is_two_clicks() {
        let p = plan_transfer(
            &chest(vec![stack("x", 64)], vec![None]),
            TransferDirection::Take,
            "x",
            64,
            false,
        )
        .unwrap();
        assert_eq!(p.clicks.len(), 2);
    }
    #[test]
    fn missing_and_full_are_distinct() {
        assert_eq!(
            plan_transfer(
                &chest(vec![None], vec![None]),
                TransferDirection::Take,
                "x",
                1,
                true
            ),
            Err(PlanError::ItemNotFound)
        );
        assert_eq!(
            plan_transfer(
                &chest(vec![stack("x", 1)], vec![stack("y", 64)]),
                TransferDirection::Take,
                "x",
                1,
                true
            ),
            Err(PlanError::DestinationFull)
        );
    }
    #[test]
    fn partial_policy_is_enforced() {
        let m = chest(vec![stack("x", 2)], vec![None]);
        assert_eq!(
            plan_transfer(&m, TransferDirection::Take, "x", 3, false),
            Err(PlanError::PartialOnly { available: 2 })
        );
        assert_eq!(
            plan_transfer(&m, TransferDirection::Take, "x", 3, true)
                .unwrap()
                .planned,
            2
        );
    }
    #[test]
    fn store_offsets_player_source() {
        let p = plan_transfer(
            &chest(vec![None; 27], vec![stack("x", 2)]),
            TransferDirection::Store,
            "x",
            2,
            false,
        )
        .unwrap();
        assert_eq!(p.clicks[0].slot, 27);
        assert_eq!(p.clicks[1].slot, 0);
    }
    #[test]
    fn fills_multiple_destination_slots_deterministically() {
        let p = plan_transfer(
            &chest(vec![stack("x", 64), stack("x", 1)], vec![None, None]),
            TransferDirection::Take,
            "x",
            65,
            false,
        )
        .unwrap();
        assert_eq!(p.planned, 65);
        assert!(p.clicks.iter().any(|click| click.slot == 3));
    }
}
