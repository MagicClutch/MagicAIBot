use std::{collections::HashMap, time::Duration};

use async_trait::async_trait;
use tokio::{
    sync::Mutex,
    time::{Instant, sleep},
};
use tokio_util::sync::CancellationToken;

use super::planner;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuKind {
    Player,
    Container,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventorySlotView {
    pub menu_slot: u16,
    pub item_id: Option<String>,
    pub count: u32,
    pub max_stack: u32,
}
impl InventorySlotView {
    pub fn empty(menu_slot: u16) -> Self {
        Self {
            menu_slot,
            item_id: None,
            count: 0,
            max_stack: 64,
        }
    }
    pub fn stack(menu_slot: u16, id: impl Into<String>, count: u32, max_stack: u32) -> Self {
        Self {
            menu_slot,
            item_id: Some(id.into()),
            count,
            max_stack,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.item_id.is_none() || self.count == 0
    }
}

/// A server-observed inventory view. `revision` is Azalea's state id; slots
/// must only be consumed after that id advances following a click.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryView {
    pub session: u64,
    pub menu_id: i32,
    pub menu_kind: MenuKind,
    pub revision: u32,
    pub alive: bool,
    pub connected: bool,
    pub cursor: Option<InventorySlotView>,
    pub slots: Vec<InventorySlotView>,
    pub player_slots: Vec<u16>,
    pub hotbar_slots: [u16; 9],
    pub selected_hotbar: u8,
}
impl InventoryView {
    pub fn slot(&self, slot: u16) -> Option<&InventorySlotView> {
        self.slots.iter().find(|s| s.menu_slot == slot)
    }
    #[cfg(test)]
    pub fn test(kind: MenuKind, slots: Vec<InventorySlotView>) -> Self {
        Self {
            session: 1,
            menu_id: 0,
            menu_kind: kind,
            revision: 1,
            alive: true,
            connected: true,
            cursor: None,
            slots,
            player_slots: (9..45).collect(),
            hotbar_slots: [36, 37, 38, 39, 40, 41, 42, 43, 44],
            selected_hotbar: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryClick {
    Left(u16),
    Right(u16),
    QuickMove(u16),
    Swap { slot: u16, hotbar: u8 },
    DropOne(u16),
    DropStack(u16),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InventoryRequest {
    Select {
        hotbar: u8,
    },
    Swap {
        from: u16,
        to: u16,
    },
    Move {
        from: u16,
        to: u16,
        amount: u32,
    },
    Split {
        from: u16,
        to: u16,
        amount: u32,
    },
    Merge {
        from: u16,
        to: u16,
    },
    QuickMove {
        slot: u16,
    },
    Drop {
        slot: u16,
        amount: Option<u32>,
        authorization: Option<String>,
    },
    EnsureInHotbar {
        item_id: String,
        preferred: Option<u8>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Rejection {
    Busy,
    InvalidSlot(u16),
    InvalidHotbar(u8),
    InvalidAmount,
    Empty,
    DifferentItems,
    DropNotAuthorized,
    ReservationConflict(u16),
    ContainerUnsupported,
    Full,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InventoryOutcome {
    Completed {
        confirmed_steps: usize,
        revision: u32,
    },
    Partial {
        confirmed_steps: usize,
        reason: String,
    },
    Stale,
    Rejected(Rejection),
    Full,
    TimedOut {
        confirmed_steps: usize,
    },
    Cancelled {
        confirmed_steps: usize,
    },
    Died {
        confirmed_steps: usize,
    },
    Disconnected {
        confirmed_steps: usize,
    },
}

#[async_trait]
pub trait InventoryTransport: Send + Sync {
    async fn inventory_view(&self) -> Result<InventoryView, ()>;
    async fn click_inventory(&self, bound: &InventoryView, click: InventoryClick)
    -> Result<(), ()>;
    async fn select_hotbar(&self, bound: &InventoryView, slot: u8) -> Result<(), ()>;
}

/// The sole application-level serialization point for inventory mutations.
pub struct InventoryActionService {
    owner: Mutex<()>,
    reservations: Mutex<HashMap<u16, u64>>,
    timeout: Duration,
}
impl Default for InventoryActionService {
    fn default() -> Self {
        Self::new(Duration::from_secs(3))
    }
}
impl InventoryActionService {
    pub fn new(timeout: Duration) -> Self {
        Self {
            owner: Mutex::new(()),
            reservations: Mutex::new(HashMap::new()),
            timeout,
        }
    }
    pub async fn reserve(&self, owner: u64, slots: &[u16]) -> Result<(), Rejection> {
        let mut reservations = self.reservations.lock().await;
        if let Some(slot) = slots
            .iter()
            .find(|slot| reservations.get(slot).is_some_and(|v| *v != owner))
        {
            return Err(Rejection::ReservationConflict(*slot));
        }
        for slot in slots {
            reservations.insert(*slot, owner);
        }
        Ok(())
    }
    pub async fn release(&self, owner: u64) {
        self.reservations
            .lock()
            .await
            .retain(|_, value| *value != owner);
    }

    pub async fn execute<T: InventoryTransport>(
        &self,
        transport: &T,
        request: InventoryRequest,
        cancel: &CancellationToken,
    ) -> InventoryOutcome {
        let Ok(_guard) = self.owner.try_lock() else {
            return InventoryOutcome::Rejected(Rejection::Busy);
        };
        let Ok(start) = transport.inventory_view().await else {
            return InventoryOutcome::Disconnected { confirmed_steps: 0 };
        };
        if !start.connected {
            return InventoryOutcome::Disconnected { confirmed_steps: 0 };
        }
        if !start.alive {
            return InventoryOutcome::Died { confirmed_steps: 0 };
        }
        if start.menu_kind != MenuKind::Player {
            return InventoryOutcome::Rejected(Rejection::ContainerUnsupported);
        }
        if start.cursor.as_ref().is_some_and(|s| !s.is_empty()) {
            return InventoryOutcome::Rejected(Rejection::Busy);
        }
        let (steps, select) = match self.plan(&start, request).await {
            Ok(value) => value,
            Err(Rejection::Full) => return InventoryOutcome::Full,
            Err(error) => return InventoryOutcome::Rejected(error),
        };
        if let Some(slot) = select {
            if transport.select_hotbar(&start, slot).await.is_err() {
                return InventoryOutcome::Disconnected { confirmed_steps: 0 };
            }
            return self
                .confirm_selection(transport, &start, slot, cancel)
                .await;
        }
        self.run_steps(transport, start, steps, cancel).await
    }

    async fn plan(
        &self,
        view: &InventoryView,
        request: InventoryRequest,
    ) -> Result<(Vec<InventoryClick>, Option<u8>), Rejection> {
        let reserved = self.reservations.lock().await;
        let check = |slots: &[u16]| {
            slots.iter().find_map(|s| {
                reserved
                    .contains_key(s)
                    .then_some(Rejection::ReservationConflict(*s))
            })
        };
        let result = match request {
            InventoryRequest::Select { hotbar } => {
                if hotbar > 8 {
                    return Err(Rejection::InvalidHotbar(hotbar));
                }
                return Ok((vec![], Some(hotbar)));
            }
            InventoryRequest::Swap { from, to } => {
                if let Some(e) = check(&[from, to]) {
                    return Err(e);
                }
                planner::swap(
                    from,
                    to,
                    view.hotbar_slots
                        .iter()
                        .position(|s| *s == to)
                        .map(|v| v as u8),
                )
            }
            InventoryRequest::Move { from, to, amount }
            | InventoryRequest::Split { from, to, amount } => {
                if let Some(e) = check(&[from, to]) {
                    return Err(e);
                }
                planner::move_amount(view, from, to, amount)?
            }
            InventoryRequest::Merge { from, to } => {
                if let Some(e) = check(&[from, to]) {
                    return Err(e);
                }
                let count = view.slot(from).ok_or(Rejection::InvalidSlot(from))?.count;
                planner::move_amount(view, from, to, count)?
            }
            InventoryRequest::QuickMove { slot } => {
                if let Some(e) = check(&[slot]) {
                    return Err(e);
                }
                vec![InventoryClick::QuickMove(slot)]
            }
            InventoryRequest::Drop {
                slot,
                amount,
                authorization,
            } => {
                if authorization.as_deref() != Some("allow-drop") {
                    return Err(Rejection::DropNotAuthorized);
                }
                if let Some(e) = check(&[slot]) {
                    return Err(e);
                }
                let stack = view.slot(slot).ok_or(Rejection::InvalidSlot(slot))?;
                if stack.is_empty() {
                    return Err(Rejection::Empty);
                };
                match amount {
                    None => vec![InventoryClick::DropStack(slot)],
                    Some(0) => return Err(Rejection::InvalidAmount),
                    Some(n) if n >= stack.count => vec![InventoryClick::DropStack(slot)],
                    Some(n) => vec![InventoryClick::DropOne(slot); n as usize],
                }
            }
            InventoryRequest::EnsureInHotbar { item_id, preferred } => {
                if let Some((index, _)) = view.hotbar_slots.iter().enumerate().find(|(_, s)| {
                    view.slot(**s).and_then(|v| v.item_id.as_deref()) == Some(item_id.as_str())
                }) {
                    return Ok((vec![], Some(index as u8)));
                }
                let source = view
                    .player_slots
                    .iter()
                    .copied()
                    .find(|s| {
                        view.slot(*s).and_then(|v| v.item_id.as_deref()) == Some(item_id.as_str())
                    })
                    .ok_or(Rejection::Empty)?;
                let target = preferred.unwrap_or(view.selected_hotbar);
                if target > 8 {
                    return Err(Rejection::InvalidHotbar(target));
                };
                if let Some(e) = check(&[source, view.hotbar_slots[target as usize]]) {
                    return Err(e);
                }
                vec![InventoryClick::Swap {
                    slot: source,
                    hotbar: target,
                }]
            }
        };
        Ok((result, None))
    }

    async fn run_steps<T: InventoryTransport>(
        &self,
        transport: &T,
        mut bound: InventoryView,
        steps: Vec<InventoryClick>,
        cancel: &CancellationToken,
    ) -> InventoryOutcome {
        let mut confirmed = 0;
        for click in steps {
            if cancel.is_cancelled() {
                return self
                    .interrupted(transport, &bound, confirmed, "cancelled")
                    .await;
            }
            if transport.click_inventory(&bound, click).await.is_err() {
                return InventoryOutcome::Disconnected {
                    confirmed_steps: confirmed,
                };
            }
            match self.wait_revision(transport, &bound, cancel).await {
                Ok(next) => {
                    bound = next;
                    confirmed += 1;
                }
                Err(outcome) => {
                    return if confirmed > 0 && matches!(outcome, InventoryOutcome::Stale) {
                        InventoryOutcome::Partial {
                            confirmed_steps: confirmed,
                            reason: "inventory binding changed".into(),
                        }
                    } else {
                        outcome
                    };
                }
            }
        }
        InventoryOutcome::Completed {
            confirmed_steps: confirmed,
            revision: bound.revision,
        }
    }
    async fn confirm_selection<T: InventoryTransport>(
        &self,
        transport: &T,
        bound: &InventoryView,
        slot: u8,
        cancel: &CancellationToken,
    ) -> InventoryOutcome {
        let deadline = Instant::now() + self.timeout;
        loop {
            if cancel.is_cancelled() {
                return InventoryOutcome::Cancelled { confirmed_steps: 0 };
            };
            let Ok(next) = transport.inventory_view().await else {
                return InventoryOutcome::Disconnected { confirmed_steps: 0 };
            };
            if !next.alive {
                return InventoryOutcome::Died { confirmed_steps: 0 };
            }
            if !next.connected {
                return InventoryOutcome::Disconnected { confirmed_steps: 0 };
            }
            if next.session != bound.session || next.menu_id != bound.menu_id {
                return InventoryOutcome::Stale;
            }
            if next.selected_hotbar == slot {
                return InventoryOutcome::Completed {
                    confirmed_steps: 1,
                    revision: next.revision,
                };
            }
            if Instant::now() >= deadline {
                return InventoryOutcome::TimedOut { confirmed_steps: 0 };
            }
            sleep(Duration::from_millis(10)).await;
        }
    }
    async fn wait_revision<T: InventoryTransport>(
        &self,
        transport: &T,
        bound: &InventoryView,
        cancel: &CancellationToken,
    ) -> Result<InventoryView, InventoryOutcome> {
        let deadline = Instant::now() + self.timeout;
        loop {
            if cancel.is_cancelled() {
                return Err(self.interrupted(transport, bound, 0, "cancelled").await);
            };
            let next = transport
                .inventory_view()
                .await
                .map_err(|_| InventoryOutcome::Disconnected { confirmed_steps: 0 })?;
            if !next.connected {
                return Err(InventoryOutcome::Disconnected { confirmed_steps: 0 });
            }
            if !next.alive {
                return Err(InventoryOutcome::Died { confirmed_steps: 0 });
            }
            if next.session != bound.session || next.menu_id != bound.menu_id {
                return Err(InventoryOutcome::Stale);
            }
            if next.revision != bound.revision {
                return Ok(next);
            }
            if Instant::now() >= deadline {
                return Err(InventoryOutcome::TimedOut { confirmed_steps: 0 });
            }
            sleep(Duration::from_millis(10)).await;
        }
    }
    async fn interrupted<T: InventoryTransport>(
        &self,
        transport: &T,
        bound: &InventoryView,
        confirmed: usize,
        _reason: &str,
    ) -> InventoryOutcome {
        // If a confirmed step left a cursor stack, return it only when the same
        // binding is still authoritative. The cleanup click is itself not
        // treated as confirmed until a later server revision.
        if let Ok(now) = transport.inventory_view().await
            && now.session == bound.session
            && now.menu_id == bound.menu_id
            && now.cursor.as_ref().is_some_and(|s| !s.is_empty())
        {
            if let Some(slot) = now
                .player_slots
                .iter()
                .copied()
                .find(|s| now.slot(*s).is_some_and(|v| v.is_empty()))
            {
                let _ = transport
                    .click_inventory(&now, InventoryClick::Left(slot))
                    .await;
            }
        }
        InventoryOutcome::Cancelled {
            confirmed_steps: confirmed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex as StdMutex};
    struct Mock {
        views: StdMutex<VecDeque<InventoryView>>,
        clicks: StdMutex<Vec<(u32, InventoryClick)>>,
    }
    #[async_trait]
    impl InventoryTransport for Mock {
        async fn inventory_view(&self) -> Result<InventoryView, ()> {
            let mut q = self.views.lock().unwrap();
            if q.len() > 1 {
                Ok(q.pop_front().unwrap())
            } else {
                q.front().cloned().ok_or(())
            }
        }
        async fn click_inventory(&self, b: &InventoryView, c: InventoryClick) -> Result<(), ()> {
            self.clicks.lock().unwrap().push((b.revision, c));
            Ok(())
        }
        async fn select_hotbar(&self, _: &InventoryView, _: u8) -> Result<(), ()> {
            Ok(())
        }
    }
    fn mock(views: Vec<InventoryView>) -> Mock {
        Mock {
            views: StdMutex::new(views.into()),
            clicks: StdMutex::default(),
        }
    }
    #[tokio::test]
    async fn sends_one_step_only_after_each_acknowledgement() {
        let a = InventoryView::test(
            MenuKind::Player,
            vec![
                InventorySlotView::stack(9, "stone", 1, 64),
                InventorySlotView::empty(10),
            ],
        );
        let mut b = a.clone();
        b.revision = 2;
        b.slots[0] = InventorySlotView::empty(9);
        b.cursor = Some(InventorySlotView::stack(0, "stone", 1, 64));
        let mut c = b.clone();
        c.revision = 3;
        c.cursor = None;
        c.slots[1] = InventorySlotView::stack(10, "stone", 1, 64);
        let m = mock(vec![a.clone(), a, b, c]);
        let out = InventoryActionService::new(Duration::from_millis(50))
            .execute(
                &m,
                InventoryRequest::Move {
                    from: 9,
                    to: 10,
                    amount: 1,
                },
                &CancellationToken::new(),
            )
            .await;
        assert!(matches!(
            out,
            InventoryOutcome::Completed {
                confirmed_steps: 2,
                ..
            }
        ));
        assert_eq!(
            m.clicks
                .lock()
                .unwrap()
                .iter()
                .map(|x| x.0)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }
    #[tokio::test]
    async fn revision_race_is_stale() {
        let a = InventoryView::test(
            MenuKind::Player,
            vec![InventorySlotView::stack(9, "stone", 1, 64)],
        );
        let mut b = a.clone();
        b.session = 2;
        let m = mock(vec![a, b]);
        assert_eq!(
            InventoryActionService::new(Duration::from_millis(20))
                .execute(
                    &m,
                    InventoryRequest::QuickMove { slot: 9 },
                    &CancellationToken::new()
                )
                .await,
            InventoryOutcome::Stale
        );
    }
    #[tokio::test]
    async fn cancellation_before_dispatch_is_typed() {
        let a = InventoryView::test(
            MenuKind::Player,
            vec![InventorySlotView::stack(9, "stone", 1, 64)],
        );
        let m = mock(vec![a]);
        let c = CancellationToken::new();
        c.cancel();
        assert_eq!(
            InventoryActionService::new(Duration::from_millis(20))
                .execute(&m, InventoryRequest::QuickMove { slot: 9 }, &c)
                .await,
            InventoryOutcome::Cancelled { confirmed_steps: 0 }
        );
    }
    #[test]
    fn player_mapping_is_explicit() {
        let v = InventoryView::test(MenuKind::Player, vec![]);
        assert_eq!(v.hotbar_slots, [36, 37, 38, 39, 40, 41, 42, 43, 44]);
        assert_eq!(v.player_slots.len(), 36);
    }
}
