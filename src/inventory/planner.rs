use super::service::{InventoryClick, InventoryView, Rejection};

pub(super) fn move_amount(
    view: &InventoryView,
    from: u16,
    to: u16,
    amount: u32,
) -> Result<Vec<InventoryClick>, Rejection> {
    let source = view.slot(from).ok_or(Rejection::InvalidSlot(from))?;
    if source.is_empty() || amount == 0 || amount > source.count {
        return Err(Rejection::InvalidAmount);
    }
    let target = view.slot(to).ok_or(Rejection::InvalidSlot(to))?;
    if !target.is_empty() && target.item_id != source.item_id {
        return Err(Rejection::DifferentItems);
    }
    let capacity = source.max_stack.saturating_sub(target.count);
    if capacity == 0 {
        return Err(Rejection::Full);
    }
    let amount = amount.min(capacity);
    if amount == source.count && target.is_empty() {
        return Ok(vec![InventoryClick::Left(from), InventoryClick::Left(to)]);
    }
    // Pick up the stack, place one item per confirmed right click, then return
    // the remainder. This is deliberately simple and interruption-safe.
    let mut clicks = vec![InventoryClick::Left(from)];
    clicks.extend((0..amount).map(|_| InventoryClick::Right(to)));
    if amount < source.count {
        clicks.push(InventoryClick::Left(from));
    }
    Ok(clicks)
}

pub(super) fn swap(from: u16, to: u16, hotbar: Option<u8>) -> Vec<InventoryClick> {
    hotbar.map_or_else(
        || {
            vec![
                InventoryClick::Left(from),
                InventoryClick::Left(to),
                InventoryClick::Left(from),
            ]
        },
        |slot| {
            vec![InventoryClick::Swap {
                slot: from,
                hotbar: slot,
            }]
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::service::{InventorySlotView, MenuKind};

    fn view(slots: Vec<InventorySlotView>) -> InventoryView {
        InventoryView::test(MenuKind::Player, slots)
    }

    #[test]
    fn planner_splits_with_confirmable_single_item_steps() {
        let plan = move_amount(
            &view(vec![
                InventorySlotView::stack(9, "stone", 8, 64),
                InventorySlotView::empty(10),
            ]),
            9,
            10,
            3,
        )
        .unwrap();
        assert_eq!(
            plan,
            vec![
                InventoryClick::Left(9),
                InventoryClick::Right(10),
                InventoryClick::Right(10),
                InventoryClick::Right(10),
                InventoryClick::Left(9)
            ]
        );
    }

    #[test]
    fn planner_rejects_full_and_incompatible_targets() {
        let full = view(vec![
            InventorySlotView::stack(9, "stone", 8, 64),
            InventorySlotView::stack(10, "stone", 64, 64),
        ]);
        assert_eq!(move_amount(&full, 9, 10, 1), Err(Rejection::Full));
        let mixed = view(vec![
            InventorySlotView::stack(9, "stone", 8, 64),
            InventorySlotView::stack(10, "dirt", 1, 64),
        ]);
        assert_eq!(
            move_amount(&mixed, 9, 10, 1),
            Err(Rejection::DifferentItems)
        );
    }
}
