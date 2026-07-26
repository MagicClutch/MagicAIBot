use super::{
    model::*,
    planner::{PlanError, plan_transfer},
    state_machine::ContainerMachine,
};
use crate::{
    error::AppError,
    look::{
        LookController,
        aim_point::{BlockAimMode, LookPrecision},
        look_controller::LookState,
        look_target::LookTarget,
    },
    minecraft::{
        client::{ConnectionState, MinecraftClient},
        world_state::BlockPosition,
    },
    movement::MovementService,
    navigation::{
        block_navigation::BlockNavigationService, navigation_state::BlockNavigationState,
    },
};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

struct Inner {
    machine: ContainerMachine,
    block_id: Option<String>,
    deadline: Instant,
    clicks: VecDeque<InventoryClick>,
    planned: u32,
    baseline: u32,
    direction: TransferDirection,
    item: String,
    last_click: Instant,
}
#[derive(Clone)]
pub struct ContainerService {
    inner: Arc<Mutex<Inner>>,
    timeout: Duration,
    click_delay: Duration,
    allow_partial: bool,
}
impl Default for ContainerService {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                machine: Default::default(),
                block_id: None,
                deadline: Instant::now(),
                clicks: VecDeque::new(),
                planned: 0,
                baseline: 0,
                direction: TransferDirection::Take,
                item: String::new(),
                last_click: Instant::now(),
            })),
            timeout: Duration::from_secs(5),
            click_delay: Duration::from_millis(75),
            allow_partial: true,
        }
    }
}
impl ContainerService {
    pub async fn status(&self) -> ContainerStatus {
        self.inner.lock().await.machine.status.clone()
    }
    pub async fn open(
        &self,
        mc: &MinecraftClient,
        movement: &MovementService,
        navigation: &BlockNavigationService,
        target: BlockPosition,
    ) -> Result<(), AppError> {
        let id = mc
            .block_ids_at(&[target])
            .await?
            .remove(&target)
            .flatten()
            .ok_or(AppError::TargetChunkUnloaded)?;
        if !matches!(id.as_str(), "minecraft:chest" | "minecraft:trapped_chest") {
            let mut i = self.inner.lock().await;
            let _: Result<(), _> = i.machine.fail(ContainerOutcome::TargetNotFound);
            return Err(AppError::TargetBlockDisappeared);
        }
        {
            let mut i = self.inner.lock().await;
            i.machine
                .begin(target)
                .map_err(|_| AppError::InventoryUnavailable)?;
            i.machine.advance(ContainerPhase::Navigating);
            i.block_id = Some(id.clone());
            i.deadline = Instant::now() + self.timeout;
        }
        navigation.start_exact(mc, movement, target, id).await
    }
    pub async fn transfer(
        &self,
        mc: &MinecraftClient,
        direction: TransferDirection,
        item: String,
        count: u32,
    ) -> Result<(), String> {
        let menu = mc
            .chest_menu_snapshot()
            .await
            .map_err(|e| e.to_string())?
            .ok_or("no chest menu is open")?;
        {
            self.inner
                .lock()
                .await
                .machine
                .advance(ContainerPhase::Planning);
        }
        let plan = match plan_transfer(&menu, direction, &item, count, self.allow_partial) {
            Ok(plan) => plan,
            Err(error) => {
                let outcome = match error {
                    PlanError::ItemNotFound => ContainerOutcome::ItemNotFound,
                    PlanError::DestinationFull => ContainerOutcome::DestinationFull,
                    PlanError::PartialOnly { .. } => ContainerOutcome::DestinationFull,
                    PlanError::InvalidCount => ContainerOutcome::ServerRejected,
                };
                self.fail(outcome).await;
                return Err(format!("{error:?}"));
            }
        };
        let baseline = count_matching(&menu, direction, &item);
        let mut i = self.inner.lock().await;
        i.machine
            .begin_transfer(&plan)
            .map_err(|e| format!("{e:?}"))?;
        i.clicks = plan.clicks.into();
        i.planned = plan.planned;
        i.baseline = baseline;
        i.direction = direction;
        i.item = item;
        i.deadline = Instant::now() + self.timeout;
        i.last_click = Instant::now() - self.click_delay;
        Ok(())
    }
    pub async fn close(&self, mc: &MinecraftClient) {
        let _ = mc.close_container().await;
        let mut i = self.inner.lock().await;
        i.machine.advance(ContainerPhase::Closing);
        i.machine.close();
        i.clicks.clear();
    }
    pub async fn cancel(
        &self,
        mc: &MinecraftClient,
        navigation: &BlockNavigationService,
        movement: &MovementService,
        look: &LookController,
    ) {
        navigation.cancel(mc, movement).await;
        look.cancel().await;
        let _ = mc.close_container().await;
        let mut i = self.inner.lock().await;
        i.machine.cancel();
        i.clicks.clear();
    }
    pub async fn tick(
        &self,
        mc: &MinecraftClient,
        _movement: &MovementService,
        navigation: &BlockNavigationService,
        look: &LookController,
    ) {
        let phase = self.inner.lock().await.machine.status.phase;
        if matches!(
            phase,
            ContainerPhase::Idle
                | ContainerPhase::Completed
                | ContainerPhase::Failed
                | ContainerPhase::Cancelled
        ) {
            return;
        }
        let world = mc.world_state_snapshot().await;
        if mc.connection_state() != ConnectionState::Connected {
            self.fail(ContainerOutcome::Disconnected).await;
            return;
        }
        if world.bot.alive == Some(false) {
            self.fail(ContainerOutcome::Died).await;
            return;
        }
        if Instant::now() > self.inner.lock().await.deadline {
            self.fail(ContainerOutcome::TimedOut).await;
            return;
        }
        match phase {
            ContainerPhase::Navigating => match navigation.snapshot().await.state {
                BlockNavigationState::Reached => {
                    let (target, id) = {
                        let mut i = self.inner.lock().await;
                        i.machine.advance(ContainerPhase::Aligning);
                        (i.machine.status.target.unwrap(), i.block_id.clone())
                    };
                    let _ = look
                        .look_at_with_precision(
                            mc,
                            LookTarget::Block {
                                position: target,
                                block_id: id,
                                aim_mode: BlockAimMode::Center,
                            },
                            LookPrecision::Precise,
                        )
                        .await;
                }
                BlockNavigationState::Failed | BlockNavigationState::Cancelled => {
                    self.fail(ContainerOutcome::TargetUnreachable).await
                }
                _ => {}
            },
            ContainerPhase::Aligning => match look.snapshot().await.state {
                LookState::Completed => {
                    let target = {
                        let mut i = self.inner.lock().await;
                        i.machine.advance(ContainerPhase::Interacting);
                        i.machine.status.target.unwrap()
                    };
                    if mc.interact_block(target).await.is_err() {
                        self.fail(ContainerOutcome::BlockedOrRemoved).await
                    } else {
                        self.inner
                            .lock()
                            .await
                            .machine
                            .advance(ContainerPhase::AwaitingMenu)
                    }
                }
                LookState::Failed | LookState::Cancelled => {
                    self.fail(ContainerOutcome::TargetUnreachable).await
                }
                _ => {}
            },
            ContainerPhase::AwaitingMenu => {
                if let Ok(Some(menu)) = mc.chest_menu_snapshot().await {
                    let mut i = self.inner.lock().await;
                    if i.machine.menu_opened(&menu).is_ok() {
                        i.deadline = Instant::now() + Duration::from_secs(3600);
                    }
                } else if mc.any_container_open().await {
                    self.fail(ContainerOutcome::WrongMenu).await
                }
            }
            ContainerPhase::Open => {
                if mc.chest_menu_snapshot().await.ok().flatten().is_none() {
                    self.fail(ContainerOutcome::ContainerClosedUnexpectedly)
                        .await
                }
            }
            ContainerPhase::Transferring => {
                let click = {
                    let mut i = self.inner.lock().await;
                    if i.last_click.elapsed() < self.click_delay {
                        return;
                    }
                    i.last_click = Instant::now();
                    i.clicks.pop_front()
                };
                if let Some(click) = click {
                    let window = self.inner.lock().await.machine.status.window_id.unwrap();
                    if mc.container_click(window, click).await.is_err() {
                        self.fail(ContainerOutcome::InventoryConflict).await
                    }
                } else if let Ok(Some(menu)) = mc.chest_menu_snapshot().await {
                    let mut i = self.inner.lock().await;
                    let current = count_matching(&menu, i.direction, &i.item);
                    let actual = i.baseline.saturating_sub(current).min(i.planned);
                    let window = menu.window_id;
                    let rev = menu.revision;
                    if i.machine.revision_changed(rev) {
                        let _ = i.machine.confirm(window, rev, actual);
                    }
                }
            }
            _ => {}
        }
    }
    async fn fail(&self, outcome: ContainerOutcome) {
        let mut i = self.inner.lock().await;
        let _: Result<(), _> = i.machine.fail(outcome);
        i.clicks.clear();
    }
}
fn count_matching(menu: &MenuSnapshot, direction: TransferDirection, item: &str) -> u32 {
    let slots = match direction {
        TransferDirection::Take => &menu.container_slots,
        TransferDirection::Store => &menu.player_slots,
    };
    slots
        .iter()
        .flatten()
        .filter(|s| item.is_empty() || s.item_id == item)
        .map(|s| s.count)
        .sum()
}
