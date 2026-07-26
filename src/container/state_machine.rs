use super::model::*;

#[derive(Clone, Debug)]
pub struct ContainerMachine {
    pub status: ContainerStatus,
    owned: bool,
    expected_revision: Option<u32>,
}
impl Default for ContainerMachine {
    fn default() -> Self {
        Self {
            status: ContainerStatus::default(),
            owned: false,
            expected_revision: None,
        }
    }
}
impl ContainerMachine {
    pub fn begin(
        &mut self,
        target: crate::minecraft::world_state::BlockPosition,
    ) -> Result<(), ContainerOutcome> {
        if self.owned {
            return Err(ContainerOutcome::InventoryConflict);
        }
        self.owned = true;
        self.status = ContainerStatus {
            phase: ContainerPhase::Validating,
            target: Some(target),
            ..Default::default()
        };
        Ok(())
    }
    pub fn advance(&mut self, phase: ContainerPhase) {
        self.status.phase = phase;
    }
    pub fn menu_opened(&mut self, menu: &MenuSnapshot) -> Result<(), ContainerOutcome> {
        if !matches!(
            menu.kind,
            ContainerKind::SingleChest | ContainerKind::DoubleChest
        ) {
            return self.fail(ContainerOutcome::WrongMenu);
        }
        self.status.kind = Some(menu.kind);
        self.status.window_id = Some(menu.window_id);
        self.status.phase = ContainerPhase::Open;
        self.status.outcome = Some(ContainerOutcome::Opened);
        self.expected_revision = Some(menu.revision);
        Ok(())
    }
    pub fn begin_transfer(&mut self, plan: &TransferPlan) -> Result<(), ContainerOutcome> {
        if self.status.window_id != Some(plan.expected_window)
            || self.expected_revision != Some(plan.expected_revision)
        {
            return self.fail(ContainerOutcome::InventoryConflict);
        }
        self.status.phase = ContainerPhase::Transferring;
        self.status.requested = plan.requested;
        Ok(())
    }
    pub fn revision_changed(&self, revision: u32) -> bool {
        self.expected_revision != Some(revision)
    }
    pub fn confirm(
        &mut self,
        window: i32,
        revision: u32,
        actual: u32,
    ) -> Result<(), ContainerOutcome> {
        if self.status.window_id != Some(window) {
            return self.fail(ContainerOutcome::ContainerClosedUnexpectedly);
        }
        if Some(revision) == self.expected_revision || actual == 0 {
            return self.fail(ContainerOutcome::ServerRejected);
        }
        self.expected_revision = Some(revision);
        self.status.transferred += actual;
        self.status.phase = ContainerPhase::Open;
        self.status.outcome = Some(if self.status.transferred < self.status.requested {
            ContainerOutcome::TransferPartial
        } else {
            ContainerOutcome::TransferCompleted
        });
        Ok(())
    }
    pub fn fail<T>(&mut self, outcome: ContainerOutcome) -> Result<T, ContainerOutcome> {
        self.status.phase = ContainerPhase::Failed;
        self.status.outcome = Some(outcome.clone());
        self.owned = false;
        Err(outcome)
    }
    pub fn cancel(&mut self) {
        self.status.phase = ContainerPhase::Cancelled;
        self.status.outcome = Some(ContainerOutcome::Cancelled);
        self.owned = false;
    }
    pub fn close(&mut self) {
        self.status.phase = ContainerPhase::Completed;
        self.owned = false;
    }
    #[cfg(test)]
    pub fn owns_inventory(&self) -> bool {
        self.owned
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::world_state::BlockPosition;
    fn menu() -> MenuSnapshot {
        MenuSnapshot {
            kind: ContainerKind::SingleChest,
            position: None,
            window_id: 2,
            revision: 4,
            container_slots: vec![None; 27],
            player_slots: vec![None; 36],
        }
    }
    #[test]
    fn wrong_menu_and_revision_race_release_ownership() {
        let mut m = ContainerMachine::default();
        m.begin(BlockPosition::default()).unwrap();
        let mut x = menu();
        x.kind = ContainerKind::DoubleChest;
        x.container_slots = vec![None; 54];
        m.menu_opened(&x).unwrap();
        let p = TransferPlan {
            direction: TransferDirection::Take,
            item_id: "x".into(),
            requested: 1,
            planned: 1,
            clicks: vec![],
            expected_window: 2,
            expected_revision: 3,
        };
        assert_eq!(
            m.begin_transfer(&p),
            Err(ContainerOutcome::InventoryConflict)
        );
        assert!(!m.owns_inventory());
    }
    #[test]
    fn rejection_partial_cancel_and_cleanup() {
        let mut m = ContainerMachine::default();
        m.begin(BlockPosition::default()).unwrap();
        m.menu_opened(&menu()).unwrap();
        let p = TransferPlan {
            direction: TransferDirection::Take,
            item_id: "x".into(),
            requested: 4,
            planned: 2,
            clicks: vec![],
            expected_window: 2,
            expected_revision: 4,
        };
        m.begin_transfer(&p).unwrap();
        assert_eq!(m.confirm(2, 4, 0), Err(ContainerOutcome::ServerRejected));
        assert!(!m.owns_inventory());
        m.begin(BlockPosition::default()).unwrap();
        m.menu_opened(&menu()).unwrap();
        m.begin_transfer(&p).unwrap();
        m.confirm(2, 5, 2).unwrap();
        assert_eq!(m.status.outcome, Some(ContainerOutcome::TransferPartial));
        m.cancel();
        assert!(!m.owns_inventory());
    }
    #[test]
    fn disconnect_death_timeout_and_removed_are_representable() {
        for o in [
            ContainerOutcome::Disconnected,
            ContainerOutcome::Died,
            ContainerOutcome::TimedOut,
            ContainerOutcome::BlockedOrRemoved,
            ContainerOutcome::TargetUnreachable,
        ] {
            let mut m = ContainerMachine::default();
            m.begin(BlockPosition::default()).unwrap();
            let _: Result<(), _> = m.fail(o.clone());
            assert_eq!(m.status.outcome, Some(o));
            assert!(!m.owns_inventory());
        }
    }
}
