//! Pure planning for `#drop <item> <amount> [player]`: given a live
//! inventory snapshot, decides which slots to throw items out of and
//! whether each throw should take a whole stack or a single item. No
//! Azalea types, no I/O -- mirrors `container::planner::plan_transfer`'s
//! split-stack approach, but throwing items into the world instead of
//! moving them between two menus.

use crate::container::model::DropClick;
use crate::minecraft::world_state::InventorySlot;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DropPlanError {
    /// Not enough of `item_id` in inventory. `available` is the total held
    /// (zero when the item isn't present at all).
    Insufficient { available: u32 },
}

/// Plans exactly enough throw clicks to drop `amount` of `item_id`, never
/// more: full stacks are thrown whole (one click each), and any remainder
/// smaller than a full stack is thrown one item at a time from a single
/// slot. Returns [`DropPlanError::Insufficient`] rather than a partial plan
/// if the inventory doesn't hold enough -- `#drop` never drops a partial
/// amount.
pub fn plan_drop(
    slots: &[InventorySlot],
    item_id: &str,
    amount: u32,
) -> Result<Vec<DropClick>, DropPlanError> {
    let mut matching: Vec<&InventorySlot> = slots
        .iter()
        .filter(|slot| slot.item_id.as_deref() == Some(item_id) && slot.count > 0)
        .collect();
    matching.sort_by_key(|slot| slot.slot);

    let available: u32 = matching.iter().map(|slot| slot.count).sum();
    if available < amount {
        return Err(DropPlanError::Insufficient { available });
    }
    if amount == 0 {
        return Ok(Vec::new());
    }

    let mut remaining = amount;
    let mut clicks = Vec::new();
    for slot in matching {
        if remaining == 0 {
            break;
        }
        if slot.count <= remaining {
            clicks.push(DropClick {
                slot: slot.slot,
                whole_stack: true,
            });
            remaining -= slot.count;
        } else {
            clicks.extend((0..remaining).map(|_| DropClick {
                slot: slot.slot,
                whole_stack: false,
            }));
            remaining = 0;
        }
    }
    Ok(clicks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(index: usize, item_id: &str, count: u32) -> InventorySlot {
        InventorySlot {
            slot: index,
            item_id: Some(item_id.into()),
            display_name: None,
            count,
        }
    }

    #[test]
    fn drops_exactly_the_requested_amount_from_a_single_stack() {
        let slots = vec![slot(9, "minecraft:diamond", 12)];
        let clicks = plan_drop(&slots, "minecraft:diamond", 5).unwrap();
        // 12 held, 5 requested: not the whole stack, so every click is a
        // single-item throw from the same slot.
        assert_eq!(clicks.len(), 5);
        assert!(clicks.iter().all(|c| c.slot == 9 && !c.whole_stack));
    }

    #[test]
    fn throws_a_full_stack_in_one_click() {
        let slots = vec![slot(9, "minecraft:cobblestone", 64)];
        let clicks = plan_drop(&slots, "minecraft:cobblestone", 64).unwrap();
        assert_eq!(
            clicks,
            vec![DropClick {
                slot: 9,
                whole_stack: true
            }]
        );
    }

    #[test]
    fn splits_across_multiple_stacks_preferring_lower_slots_first() {
        let slots = vec![
            slot(10, "minecraft:oak_log", 32),
            slot(9, "minecraft:oak_log", 32),
        ];
        // 64 requested across two 32-stacks: both get thrown whole, lowest
        // slot index first.
        let clicks = plan_drop(&slots, "minecraft:oak_log", 64).unwrap();
        assert_eq!(
            clicks,
            vec![
                DropClick {
                    slot: 9,
                    whole_stack: true
                },
                DropClick {
                    slot: 10,
                    whole_stack: true
                },
            ]
        );
    }

    #[test]
    fn splits_a_partial_remainder_after_a_full_stack() {
        let slots = vec![
            slot(9, "minecraft:oak_log", 32),
            slot(10, "minecraft:oak_log", 32),
        ];
        // 40 requested: first stack (32) goes whole, remaining 8 are
        // single-item throws from the second slot.
        let clicks = plan_drop(&slots, "minecraft:oak_log", 40).unwrap();
        assert_eq!(
            clicks[0],
            DropClick {
                slot: 9,
                whole_stack: true
            }
        );
        assert_eq!(clicks.len(), 9);
        assert!(clicks[1..].iter().all(|c| c.slot == 10 && !c.whole_stack));
    }

    #[test]
    fn insufficient_inventory_is_rejected_without_a_partial_plan() {
        let slots = vec![slot(9, "minecraft:diamond", 3)];
        assert_eq!(
            plan_drop(&slots, "minecraft:diamond", 5),
            Err(DropPlanError::Insufficient { available: 3 })
        );
    }

    #[test]
    fn missing_item_is_insufficient_with_zero_available() {
        let slots = vec![slot(9, "minecraft:diamond", 3)];
        assert_eq!(
            plan_drop(&slots, "minecraft:iron_ingot", 1),
            Err(DropPlanError::Insufficient { available: 0 })
        );
    }

    #[test]
    fn empty_slots_that_are_not_the_requested_item_are_ignored() {
        let slots = vec![
            slot(9, "minecraft:dirt", 64),
            InventorySlot {
                slot: 10,
                item_id: None,
                display_name: None,
                count: 0,
            },
        ];
        assert_eq!(
            plan_drop(&slots, "minecraft:dirt", 64).unwrap(),
            vec![DropClick {
                slot: 9,
                whole_stack: true
            }]
        );
    }
}
