use crate::minecraft::world_state::BlockPosition;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainerKind {
    SingleChest,
    DoubleChest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StackSnapshot {
    pub item_id: String,
    pub count: u32,
    pub max_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MenuSnapshot {
    pub kind: ContainerKind,
    pub position: Option<BlockPosition>,
    pub window_id: i32,
    pub revision: u32,
    pub container_slots: Vec<Option<StackSnapshot>>,
    pub player_slots: Vec<Option<StackSnapshot>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferDirection {
    Take,
    Store,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClickButton {
    Left,
    Right,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InventoryClick {
    pub slot: usize,
    pub button: ClickButton,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferPlan {
    pub direction: TransferDirection,
    pub item_id: String,
    pub requested: u32,
    pub planned: u32,
    pub clicks: Vec<InventoryClick>,
    pub expected_window: i32,
    pub expected_revision: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainerPhase {
    Idle,
    Validating,
    Navigating,
    Aligning,
    Interacting,
    AwaitingMenu,
    Open,
    Planning,
    Transferring,
    Closing,
    Completed,
    Failed,
    Cancelled,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContainerOutcome {
    Opened,
    TransferCompleted,
    TransferPartial,
    ItemNotFound,
    DestinationFull,
    TargetNotFound,
    TargetUnreachable,
    WrongMenu,
    ContainerClosedUnexpectedly,
    InventoryConflict,
    ServerRejected,
    TimedOut,
    Cancelled,
    Disconnected,
    Died,
    BlockedOrRemoved,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerStatus {
    pub phase: ContainerPhase,
    pub target: Option<BlockPosition>,
    pub kind: Option<ContainerKind>,
    pub window_id: Option<i32>,
    pub requested: u32,
    pub transferred: u32,
    pub outcome: Option<ContainerOutcome>,
    pub detail: Option<String>,
}
impl Default for ContainerStatus {
    fn default() -> Self {
        Self {
            phase: ContainerPhase::Idle,
            target: None,
            kind: None,
            window_id: None,
            requested: 0,
            transferred: 0,
            outcome: None,
            detail: None,
        }
    }
}
