//! Bounded, session-local task runtime and reusable Phase 3 orchestration.
//!
//! The runtime owns lifecycle, cancellation, leases, progress and history.  It
//! deliberately does not own gameplay: leaf operations are delegated through
//! [`PrimitiveExecutor`] (or directly to an existing subsystem by the legacy
//! compatibility entry points at the end of this module).
#![allow(dead_code)] // Phase 3 public API consumed by the upcoming Task 21 integration.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

pub mod lifecycle;
use lifecycle::ResourceLease;
pub use lifecycle::{
    ActionFailure, ActionResource, ActionResult, ActionState, Invalidation, OperationGuard,
    OperationId, ResourceManager,
    collections::{BTreeSet, HashMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, SystemTime},
};

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::{
    blocks::{
        block_query::BlockSearchQuery, block_search::BlockSearchService,
        block_snapshot::BlockSnapshot,
    },
    error::AppError,
    interaction::InteractionController,
    look::{LookController, LookTarget},
    minecraft::{
        client::MinecraftClient,
        world_state::{BlockPosition, PositionSnapshot, TaskSnapshot},
    },
    movement::{MovementService, NavigationMode},
    navigation::BlockNavigationService,
};

#[allow(dead_code)] // Lifecycle API lands before composed production tasks use it.
pub mod runtime;
pub mod gather;
pub mod lifecycle;
pub use lifecycle::{ActionFailure, Invalidation, OperationId};
pub const DEFAULT_HISTORY_LIMIT: usize = 64;
pub const DEFAULT_LEASE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TaskId(pub u64);
impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
pub mod ensure_tool;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TaskState {
    #[default]
    Created,
    Queued,
    WaitingForResources,
    Running,
    Paused,
    Cancelling,
    Cancelled,
    Succeeded,
    Failed,
}
impl TaskState {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Succeeded | Self::Failed)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TaskResource {
    Movement,
    Rotation,
    Interaction,
    InventoryMutation,
    ContainerAccess,
    Combat,
    ExclusivePlayerControl,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TaskProgress {
    pub phase: String,
    pub completed_units: u32,
    pub total_units: Option<u32>,
    pub current_target: Option<String>,
    pub current_child: Option<TaskId>,
    pub warning: Option<String>,
    pub updated_at: Option<SystemTime>,
}
impl TaskProgress {
    pub fn percent(&self) -> Option<u8> {
        self.total_units
            .filter(|n| *n > 0)
            .map(|n| ((self.completed_units.min(n) * 100) / n) as u8)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CancellationReason {
    User,
    ParentCancelled(TaskId),
    Shutdown,
    Disconnected,
    PlayerDied,
    Preempted,
    Replaced,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskErrorCategory {
    Validation,
    ResourceTimeout,
    Cancelled,
    MissingPrerequisite,
    Unsupported,
    Subsystem,
    Disconnected,
    PlayerDied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskFailure {
    pub category: TaskErrorCategory,
    pub message: String,
    pub child: Option<TaskId>,
    pub partial_completed: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TaskResult {
    Unit,
    Blocks(Vec<BlockSnapshot>),
    Gather(GatherResult),
    EnsureTool(EnsureToolResult),
    Inventory(InventoryOperationResult),
    Recovery(RecoveryResult),
}

#[derive(Clone, Debug)]
pub struct TaskMetadata {
    pub id: TaskId,
    pub task_type: String,
    pub source_command: String,
    pub requesting_user: Option<String>,
    pub priority: i32,
    pub parent_id: Option<TaskId>,
    pub goal_id: Option<String>,
    pub correlation_id: String,
    pub created_at: SystemTime,
}

#[derive(Clone, Debug)]
pub struct TaskStatus {
    pub metadata: TaskMetadata,
    pub state: TaskState,
    pub progress: TaskProgress,
    pub started_at: Option<SystemTime>,
    pub failure: Option<String>,
    pub operation_id: Option<OperationId>,
    pub resources: Vec<ActionResource>,
    pub completed_at: Option<SystemTime>,
    pub result: Option<TaskResult>,
    pub failure: Option<TaskFailure>,
    pub cancellation: Option<CancellationReason>,
    pub children: Vec<TaskId>,
}

#[derive(Clone, Debug)]
pub struct TaskSubmission {
    pub task_type: String,
    pub source_command: String,
    pub requesting_user: Option<String>,
    pub priority: i32,
    pub parent_id: Option<TaskId>,
    pub goal_id: Option<String>,
}
impl TaskSubmission {
    pub fn command(task_type: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            task_type: task_type.into(),
            source_command: command.into(),
            requesting_user: None,
            priority: 0,
            parent_id: None,
            goal_id: None,
        }
    }
}

struct RuntimeState {
    next_id: u64,
    active: HashMap<TaskId, TaskStatus>,
    recent: VecDeque<TaskStatus>,
    cancellations: HashMap<TaskId, CancellationToken>,
    resource_owners: HashMap<TaskResource, TaskId>,
    shutting_down: bool,
}
struct RuntimeInner {
    state: StdMutex<RuntimeState>,
    resource_changed: Notify,
    history_limit: usize,
    lease_timeout: Duration,
}

#[derive(Clone)]
pub struct TaskService {
    status: Arc<Mutex<TaskStatus>>,
    resources: ResourceManager,
    active: Arc<Mutex<Option<(OperationGuard, ResourceLease)>>>,
    session: Arc<AtomicU64>,
    inner: Arc<RuntimeInner>,
}
impl Default for TaskService {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY_LIMIT, DEFAULT_LEASE_TIMEOUT)
    }
}

pub struct TaskLease {
    service: TaskService,
    id: TaskId,
    resources: Vec<TaskResource>,
}
impl Drop for TaskLease {
    fn drop(&mut self) {
        let mut state = self
            .service
            .inner
            .state
            .lock()
            .expect("task runtime lock poisoned");
        for resource in &self.resources {
            if state.resource_owners.get(resource) == Some(&self.id) {
                state.resource_owners.remove(resource);
            }
        }
        drop(state);
        self.service.inner.resource_changed.notify_waiters();
    }
}

impl TaskService {
    pub fn new(history_limit: usize, lease_timeout: Duration) -> Self {
        assert!(
            history_limit > 0,
            "history must be bounded to a positive size"
        );
        Self {
            inner: Arc::new(RuntimeInner {
                state: StdMutex::new(RuntimeState {
                    next_id: 1,
                    active: HashMap::new(),
                    recent: VecDeque::new(),
                    cancellations: HashMap::new(),
                    resource_owners: HashMap::new(),
                    shutting_down: false,
                }),
                resource_changed: Notify::new(),
                history_limit,
                lease_timeout,
            }),
        }
    }

    pub fn submit(&self, submission: TaskSubmission) -> Result<TaskId, TaskFailure> {
        let mut state = self.inner.state.lock().expect("task runtime lock poisoned");
        if state.shutting_down {
            return Err(TaskFailure {
                category: TaskErrorCategory::Cancelled,
                message: "runtime is shutting down".into(),
                child: None,
                partial_completed: 0,
            });
        }
        let id = TaskId(state.next_id);
        state.next_id = state.next_id.wrapping_add(1).max(1);
        let metadata = TaskMetadata {
            id,
            correlation_id: format!("task-{id}"),
            task_type: submission.task_type,
            source_command: submission.source_command,
            requesting_user: submission.requesting_user,
            priority: submission.priority,
            parent_id: submission.parent_id,
            goal_id: submission.goal_id,
            created_at: SystemTime::now(),
        };
        state.active.insert(
            id,
            TaskStatus {
                metadata,
                state: TaskState::Queued,
                progress: TaskProgress::default(),
                started_at: None,
                completed_at: None,
                result: None,
                failure: None,
                cancellation: None,
                children: Vec::new(),
            },
        );
        state.cancellations.insert(id, CancellationToken::new());
        if let Some(parent) = submission.parent_id
            && let Some(status) = state.active.get_mut(&parent)
        {
            status.children.push(id);
            status.progress.current_child = Some(id);
        }
        Ok(id)
    }

    pub fn query(&self, id: TaskId) -> Option<TaskStatus> {
        let s = self.inner.state.lock().ok()?;
        s.active
            .get(&id)
            .or_else(|| s.recent.iter().find(|x| x.metadata.id == id))
            .cloned()
    }
    pub fn active(&self) -> Vec<TaskStatus> {
        let s = self.inner.state.lock().expect("task runtime lock poisoned");
        let mut v: Vec<_> = s.active.values().cloned().collect();
        v.sort_by_key(|x| x.metadata.id);
        v
    }
    pub fn recent(&self) -> Vec<TaskStatus> {
        self.inner
            .state
            .lock()
            .expect("task runtime lock poisoned")
            .recent
            .iter()
            .rev()
            .cloned()
            .collect()
    }
    pub fn resources(&self) -> Vec<(TaskResource, TaskId)> {
        self.inner
            .state
            .lock()
            .expect("task runtime lock poisoned")
            .resource_owners
            .iter()
            .map(|(r, id)| (*r, *id))
            .collect()
    }

    pub fn update_progress(&self, id: TaskId, mut progress: TaskProgress) {
        progress.updated_at = Some(SystemTime::now());
        if let Some(s) = self
            .inner
            .state
            .lock()
            .expect("task runtime lock poisoned")
            .active
            .get_mut(&id)
        {
            s.progress = progress;
        }
    }

    pub fn cancel_task(&self, id: TaskId, reason: CancellationReason) -> bool {
        let children = {
            let mut state = self.inner.state.lock().expect("task runtime lock poisoned");
            let Some(status) = state.active.get_mut(&id) else {
                return false;
            };
            status.state = TaskState::Cancelling;
            status.cancellation = Some(reason.clone());
            let children = status.children.clone();
            if let Some(token) = state.cancellations.get(&id) {
                token.cancel();
            }
            children
        };
        for child in children {
            self.cancel_task(child, CancellationReason::ParentCancelled(id));
        }
        true
    }
    pub fn cancel_all(&self, reason: CancellationReason) {
        let ids: Vec<_> = self.active().into_iter().map(|s| s.metadata.id).collect();
        for id in ids {
            self.cancel_task(id, reason.clone());
        }
    }
    pub fn shutdown(&self) {
        self.inner
            .state
            .lock()
            .expect("task runtime lock poisoned")
            .shutting_down = true;
        self.cancel_all(CancellationReason::Shutdown);
    }
    pub fn disconnected(&self) {
        self.cancel_all(CancellationReason::Disconnected);
    }
    pub fn player_died(&self) {
        self.cancel_all(CancellationReason::PlayerDied);
    }

    async fn acquire(
        &self,
        id: TaskId,
        resources: &[TaskResource],
    ) -> Result<TaskLease, TaskFailure> {
        let resources: Vec<_> = resources
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self.set_state(id, TaskState::WaitingForResources);
        let token = self
            .inner
            .state
            .lock()
            .expect("task runtime lock poisoned")
            .cancellations
            .get(&id)
            .cloned()
            .ok_or_else(|| failure(TaskErrorCategory::Validation, "unknown task"))?;
        let waiting = async {
            loop {
                {
                    let mut state = self.inner.state.lock().expect("task runtime lock poisoned");
                    if resources
                        .iter()
                        .all(|r| !state.resource_owners.contains_key(r))
                    {
                        for r in &resources {
                            state.resource_owners.insert(*r, id);
                        }
                        return Ok(TaskLease {
                            service: self.clone(),
                            id,
                            resources,
                        });
                    }
                }
                tokio::select! { _ = token.cancelled() => return Err(failure(TaskErrorCategory::Cancelled, "cancelled while waiting for resources")), _ = self.inner.resource_changed.notified() => {} }
            }
        };
        tokio::time::timeout(self.inner.lease_timeout, waiting)
            .await
            .unwrap_or_else(|_| {
                Err(failure(
                    TaskErrorCategory::ResourceTimeout,
                    "timed out acquiring task resources",
                ))
            })
    }

    pub async fn run<F, Fut>(
        &self,
        id: TaskId,
        resources: &[TaskResource],
        operation: F,
    ) -> Result<TaskResult, TaskFailure>
    where
        F: FnOnce(TaskContext) -> Fut,
        Fut: Future<Output = Result<TaskResult, TaskFailure>>,
    {
        let lease = match self.acquire(id, resources).await {
            Ok(lease) => lease,
            Err(error) => {
                self.finish(id, Err(error.clone()));
                return Err(error);
            }
        };
        let token = self
            .inner
            .state
            .lock()
            .expect("task runtime lock poisoned")
            .cancellations
            .get(&id)
            .cloned()
            .ok_or_else(|| failure(TaskErrorCategory::Validation, "unknown task"))?;
        {
            let mut state = self.inner.state.lock().expect("task runtime lock poisoned");
            if let Some(s) = state.active.get_mut(&id) {
                s.state = TaskState::Running;
                s.started_at.get_or_insert(SystemTime::now());
            }
        }
        let context = TaskContext {
            id,
            runtime: self.clone(),
            cancellation: token.clone(),
        };
        let result = tokio::select! { _ = token.cancelled() => Err(failure(TaskErrorCategory::Cancelled, "task cancelled")), result = operation(context) => result };
        drop(lease);
        self.finish(id, result.clone());
        result
    }

    fn set_state(&self, id: TaskId, next: TaskState) {
        if let Some(s) = self
            .inner
            .state
            .lock()
            .expect("task runtime lock poisoned")
            .active
            .get_mut(&id)
        {
            s.state = next;
        }
    }
    fn finish(&self, id: TaskId, result: Result<TaskResult, TaskFailure>) {
        let mut state = self.inner.state.lock().expect("task runtime lock poisoned");
        let Some(mut status) = state.active.remove(&id) else {
            return;
        };
        status.completed_at = Some(SystemTime::now());
        match result {
            Ok(value) => {
                status.state = TaskState::Succeeded;
                status.result = Some(value);
            }
            Err(error) if error.category == TaskErrorCategory::Cancelled => {
                status.state = TaskState::Cancelled;
                status.failure = Some(error);
            }
            Err(error) => {
                status.state = TaskState::Failed;
                status.failure = Some(error);
            }
        }
        state.cancellations.remove(&id);
        state.recent.push_back(status);
        while state.recent.len() > self.inner.history_limit {
            state.recent.pop_front();
        }
    }

    pub async fn run_child<F, Fut>(
        &self,
        parent: TaskId,
        submission: TaskSubmission,
        resources: &[TaskResource],
        operation: F,
    ) -> Result<TaskResult, TaskFailure>
    where
        F: FnOnce(TaskContext) -> Fut,
        Fut: Future<Output = Result<TaskResult, TaskFailure>>,
    {
        let mut submission = submission;
        submission.parent_id = Some(parent);
        let child = self.submit(submission)?;
        let result = self.run(child, resources, operation).await;
        if let Err(error) = &result {
            let mut e = error.clone();
            e.child = Some(child);
            return Err(e);
        }
        result
    }

    pub async fn status(&self) -> TaskStatus {
        self.active()
            .into_iter()
            .next()
            .or_else(|| self.recent().into_iter().next())
            .unwrap_or_else(|| TaskStatus {
                metadata: TaskMetadata {
                    id: TaskId(0),
                    task_type: "idle".into(),
                    source_command: String::new(),
                    requesting_user: None,
                    priority: 0,
                    parent_id: None,
                    goal_id: None,
                    correlation_id: "idle".into(),
                    created_at: SystemTime::now(),
                },
                state: TaskState::Created,
                progress: TaskProgress::default(),
                started_at: None,
                completed_at: None,
                result: None,
                failure: None,
                cancellation: None,
                children: vec![],
            })
    }
    pub async fn cancel(&self, minecraft: &MinecraftClient) {
        self.cancel_all(CancellationReason::User);
        minecraft.clear_current_task().await;
    }
}

#[derive(Clone)]
pub struct TaskContext {
    pub id: TaskId,
    pub runtime: TaskService,
    pub cancellation: CancellationToken,
}
impl TaskContext {
    pub fn progress(&self, progress: TaskProgress) {
        self.runtime.update_progress(self.id, progress);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

fn failure(category: TaskErrorCategory, message: impl Into<String>) -> TaskFailure {
    TaskFailure {
        category,
        message: message.into(),
        child: None,
        partial_completed: 0,
    }
}

// ---- Reusable, deliberately bounded orchestration requests/results ----------

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectItemsRequest {
    pub item_ids: Vec<String>,
    pub quantity: u32,
    pub maximum_attempts: u8,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GatherRequest {
    Items(CollectItemsRequest),
    Logs { quantity: u32 },
    Stone { quantity: u32 },
    VisibleOres { quantity: u32 },
    Food { quantity: u32 },
}
impl GatherRequest {
    pub fn normalize(self) -> Result<CollectItemsRequest, TaskFailure> {
        let (ids, quantity) = match self {
            Self::Items(r) => return validate_collect(r),
            Self::Logs { quantity } => (
                vec![
                    "minecraft:oak_log",
                    "minecraft:birch_log",
                    "minecraft:spruce_log",
                ],
                quantity,
            ),
            Self::Stone { quantity } => {
                (vec!["minecraft:stone", "minecraft:cobblestone"], quantity)
            }
            Self::VisibleOres { quantity } => (
                vec![
                    "minecraft:coal_ore",
                    "minecraft:iron_ore",
                    "minecraft:copper_ore",
                    "minecraft:gold_ore",
                    "minecraft:diamond_ore",
                ],
                quantity,
            ),
            Self::Food { quantity } => (
                vec![
                    "minecraft:beef",
                    "minecraft:porkchop",
                    "minecraft:chicken",
                    "minecraft:mutton",
                    "minecraft:bread",
                ],
                quantity,
            ),
        };
        validate_collect(CollectItemsRequest {
            item_ids: ids.into_iter().map(str::to_owned).collect(),
            quantity,
            maximum_attempts: 32,
        })
    }
}
fn validate_collect(r: CollectItemsRequest) -> Result<CollectItemsRequest, TaskFailure> {
    if r.quantity == 0 || r.item_ids.is_empty() || r.maximum_attempts == 0 {
        Err(failure(
            TaskErrorCategory::Validation,
            "quantity, item IDs, and maximum attempts must be non-zero",
        ))
    } else {
        Ok(r)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatherResult {
    pub requested: u32,
    pub collected: u32,
    pub matching_inventory_total: u32,
    pub attempts: u8,
    pub child_failures: Vec<TaskFailure>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnsureToolRequest {
    pub tool_tag: String,
    pub allow_recovery: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnsureToolResult {
    pub item_id: String,
    pub already_available: bool,
    pub recovered_prerequisites: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanInventoryRequest {
    pub keep_item_ids: Vec<String>,
    pub minimum_free_slots: u8,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DepositExcessRequest {
    pub keep_quantities: HashMap<String, u32>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryOperationResult {
    pub moved: HashMap<String, u32>,
    pub free_slots: u8,
    pub warnings: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoverPrerequisiteRequest {
    pub missing: String,
    pub maximum_depth: u8,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryResult {
    pub prerequisite: String,
    pub recovered: bool,
    pub steps: Vec<String>,
}

pub type PrimitiveFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, TaskFailure>> + 'a>>;
/// Implemented by subsystem adapters. Each method is a leaf capability owned by
/// inventory, gathering, crafting, or storage—not by the task runtime.
pub trait PrimitiveExecutor {
    fn matching_item_count<'a>(&'a mut self, item_ids: &'a [String]) -> PrimitiveFuture<'a, u32>;
    fn collect_one<'a>(
        &'a mut self,
        item_ids: &'a [String],
        cancellation: &'a CancellationToken,
    ) -> PrimitiveFuture<'a, u32>;
    fn ensure_tool<'a>(
        &'a mut self,
        request: &'a EnsureToolRequest,
        cancellation: &'a CancellationToken,
    ) -> PrimitiveFuture<'a, EnsureToolResult>;
    fn clean_inventory<'a>(
        &'a mut self,
        request: &'a CleanInventoryRequest,
        cancellation: &'a CancellationToken,
    ) -> PrimitiveFuture<'a, InventoryOperationResult>;
    fn deposit_excess<'a>(
        &'a mut self,
        request: &'a DepositExcessRequest,
        cancellation: &'a CancellationToken,
    ) -> PrimitiveFuture<'a, InventoryOperationResult>;
    fn recover_prerequisite<'a>(
        &'a mut self,
        request: &'a RecoverPrerequisiteRequest,
        cancellation: &'a CancellationToken,
    ) -> PrimitiveFuture<'a, RecoveryResult>;
}

pub async fn collect_items<E: PrimitiveExecutor>(
    context: &TaskContext,
    executor: &mut E,
    request: CollectItemsRequest,
) -> Result<GatherResult, TaskFailure> {
    let request = validate_collect(request)?;
    let initial = executor.matching_item_count(&request.item_ids).await?;
    let target = initial.saturating_add(request.quantity);
    let mut attempts = 0;
    let mut failures = vec![];
    while attempts < request.maximum_attempts {
        if context.is_cancelled() {
            let mut e = failure(TaskErrorCategory::Cancelled, "collection cancelled");
            e.partial_completed = executor
                .matching_item_count(&request.item_ids)
                .await?
                .saturating_sub(initial);
            return Err(e);
        }
        let current = executor.matching_item_count(&request.item_ids).await?;
        if current >= target {
            return Ok(GatherResult {
                requested: request.quantity,
                collected: current - initial,
                matching_inventory_total: current,
                attempts,
                child_failures: failures,
            });
        }
        context.progress(TaskProgress {
            phase: "collecting".into(),
            completed_units: current.saturating_sub(initial),
            total_units: Some(request.quantity),
            current_target: Some(request.item_ids.join("|")),
            current_child: None,
            warning: None,
            updated_at: None,
        });
        attempts += 1;
        if let Err(error) = executor
            .collect_one(&request.item_ids, &context.cancellation)
            .await
        {
            failures.push(error);
        }
    }
    let current = executor.matching_item_count(&request.item_ids).await?;
    let collected = current.saturating_sub(initial);
    if collected >= request.quantity {
        Ok(GatherResult {
            requested: request.quantity,
            collected,
            matching_inventory_total: current,
            attempts,
            child_failures: failures,
        })
    } else {
        Err(TaskFailure {
            category: TaskErrorCategory::Subsystem,
            message: format!("collection stopped after {attempts} attempts"),
            child: None,
            partial_completed: collected,
        })
    }
}

// ---- Compatibility wrappers: delegate existing commands to owning services --

impl TaskService {
    async fn tracked<F, Fut>(
        &self,
        minecraft: &MinecraftClient,
        name: impl Into<String>,
        step: impl Into<String>,
        required: impl IntoIterator<Item = ActionResource>,
    ) -> Result<(), AppError> {
        let guard = OperationGuard::new(self.session.load(Ordering::Acquire), None);
        if let Some((old, _)) = self.active.lock().await.take() {
            old.cancel();
        }
        let lease = self.resources.acquire(guard.id(), required)?;
        let leased = lease.resources().collect::<Vec<_>>();
        let (id, name, started_at) = {
            let mut status = self.status.lock().await;
            status.id = status.id.wrapping_add(1);
            status.name = Some(name.into());
            status.state = TaskState::Running;
            status.current_step = Some(step.into());
            status.started_at = Some(SystemTime::now());
            status.failure = None;
            status.operation_id = Some(guard.id());
            status.resources = leased;
            (
                status.id,
                status.name.clone().unwrap_or_default(),
                status.started_at.unwrap_or(SystemTime::now()),
            )
        };
        name: String,
        resources: &[TaskResource],
        f: F,
    ) -> Result<TaskResult, AppError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<TaskResult, AppError>>,
    {
        let id = self
            .submit(TaskSubmission::command(name.clone(), name.clone()))
            .map_err(|e| AppError::TaskRuntime(e.message))?;
        minecraft
            .set_current_task(TaskSnapshot {
                name,
                id: id.to_string(),
                status: "running".into(),
                started_at: SystemTime::now(),
            })
            .await;
        let result = self
            .run(id, resources, |_| async {
                f().await
                    .map_err(|e| failure(TaskErrorCategory::Subsystem, e.to_string()))
            })
            .await;
        *self.active.lock().await = Some((guard, lease));
        Ok(())
    }

    async fn fail(&self, minecraft: &MinecraftClient, error: &AppError) {
        let mut status = self.status.lock().await;
        status.state = TaskState::Failed;
        status.failure = Some(error.to_string());
        status.resources.clear();
        self.active.lock().await.take();
        minecraft.clear_current_task().await;
    }
    async fn complete(&self, minecraft: &MinecraftClient) {
        let mut status = self.status.lock().await;
        status.state = TaskState::Completed;
        status.resources.clear();
        drop(status);
        self.active.lock().await.take();
        minecraft.clear_current_task().await;
        result.map_err(|e| AppError::TaskRuntime(e.message))
    }
    pub async fn find_block(
        &self,
        m: &MinecraftClient,
        s: &BlockSearchService,
        q: BlockSearchQuery,
    ) -> Result<Vec<BlockSnapshot>, AppError> {
        self.begin(
            minecraft,
            format!("Find {}", query.block_id),
            "Searching loaded chunks",
            [],
        )
        .await?;
        let result = search.search_nearby(minecraft, query).await;
        match &result {
            Ok(_) => self.complete(minecraft).await,
            Err(error) => self.fail(minecraft, error).await,
        match self
            .tracked(m, format!("Find {}", q.block_id), &[], || async {
                Ok(TaskResult::Blocks(s.search_nearby(m, q).await?))
            })
            .await?
        {
            TaskResult::Blocks(v) => Ok(v),
            _ => unreachable!(),
        }
    }
    pub async fn goto_position(
        &self,
        m: &MinecraftClient,
        s: &MovementService,
        p: PositionSnapshot,
        mode: NavigationMode,
    ) -> Result<(), AppError> {
        self.begin(
            minecraft,
            "Go to position",
            "Navigating",
            [ActionResource::Movement],
        )
        .await?;
        let result = movement.goto(minecraft, position, mode).await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
        self.tracked(
            m,
            "Go to position".into(),
            &[TaskResource::Movement],
            || async {
                s.goto(m, p, mode).await?;
                Ok(TaskResult::Unit)
            },
        )
        .await
        .map(|_| ())
    }
    pub async fn goto_block(
        &self,
        m: &MinecraftClient,
        mv: &MovementService,
        n: &BlockNavigationService,
        b: String,
        r: u32,
        mode: NavigationMode,
    ) -> Result<(), AppError> {
        self.begin(
            minecraft,
            format!("Go to {block_id}"),
            "Finding safe approach",
            [ActionResource::Movement],
        )
        .await?;
        let result = navigation
            .start(minecraft, movement, block_id, radius, mode)
            .await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
        self.tracked(
            m,
            format!("Go to {b}"),
            &[TaskResource::Movement],
            || async {
                n.start(m, mv, b, r, mode).await?;
                Ok(TaskResult::Unit)
            },
        )
        .await
        .map(|_| ())
    }
    pub async fn look_at(
        &self,
        m: &MinecraftClient,
        l: &LookController,
        t: LookTarget,
    ) -> Result<(), AppError> {
        self.begin(
            minecraft,
            "Look at target",
            "Rotating",
            [ActionResource::Rotation],
        )
        .await?;
        let result = look.look_at(minecraft, target).await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
        self.tracked(
            m,
            "Look at target".into(),
            &[TaskResource::Rotation],
            || async {
                l.look_at(m, t).await?;
                Ok(TaskResult::Unit)
            },
        )
        .await
        .map(|_| ())
    }
    pub async fn look_at_block(
        &self,
        m: &MinecraftClient,
        l: &LookController,
        b: String,
    ) -> Result<(), AppError> {
        self.begin(
            minecraft,
            format!("Look at {block_id}"),
            "Selecting visible face",
            [ActionResource::Rotation],
        )
        .await?;
        let result = look.look_at_block_id(minecraft, block_id).await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
        self.tracked(
            m,
            format!("Look at {b}"),
            &[TaskResource::Rotation],
            || async {
                l.look_at_block_id(m, b).await?;
                Ok(TaskResult::Unit)
            },
        )
        .await
        .map(|_| ())
    }
    pub async fn break_block(
        &self,
        m: &MinecraftClient,
        mv: &MovementService,
        l: &LookController,
        i: &InteractionController,
        t: BlockPosition,
    ) -> Result<(), AppError> {
        self.begin(
            minecraft,
            "Break block",
            "Validate → navigate → look → break",
            [
                ActionResource::Movement,
                ActionResource::Rotation,
                ActionResource::Interaction,
            ],
        )
        .await?;
        let result = interaction
            .break_at(minecraft, movement, look, target)
            .await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
        self.tracked(
            m,
            "Break block".into(),
            &[
                TaskResource::Movement,
                TaskResource::Rotation,
                TaskResource::Interaction,
                TaskResource::InventoryMutation,
            ],
            || async {
                i.break_at(m, mv, l, t).await?;
                Ok(TaskResult::Unit)
            },
        )
        .await
        .map(|_| ())
    }
    pub async fn break_looked_block(
        &self,
        m: &MinecraftClient,
        mv: &MovementService,
        l: &LookController,
        i: &InteractionController,
    ) -> Result<(), AppError> {
        self.begin(
            minecraft,
            "Break looked block",
            "Validate → navigate → look → break",
            [
                ActionResource::Movement,
                ActionResource::Rotation,
                ActionResource::Interaction,
            ],
        )
        .await?;
        let result = interaction.break_looked(minecraft, movement, look).await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
        self.tracked(
            m,
            "Break looked block".into(),
            &[
                TaskResource::Movement,
                TaskResource::Rotation,
                TaskResource::Interaction,
                TaskResource::InventoryMutation,
            ],
            || async {
                i.break_looked(m, mv, l).await?;
                Ok(TaskResult::Unit)
            },
        )
        .await
        .map(|_| ())
    }
    pub async fn break_nearest_block(
        &self,
        m: &MinecraftClient,
        mv: &MovementService,
        l: &LookController,
        i: &InteractionController,
        b: String,
    ) -> Result<(), AppError> {
        self.begin(
            minecraft,
            format!("Break nearest {block_id}"),
            "Find → navigate → look → break",
            [
                ActionResource::Movement,
                ActionResource::Rotation,
                ActionResource::Interaction,
            ],
        )
        .await?;
        let result = interaction
            .break_nearest(minecraft, movement, look, block_id)
            .await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
        self.tracked(
            m,
            format!("Break nearest {b}"),
            &[
                TaskResource::Movement,
                TaskResource::Rotation,
                TaskResource::Interaction,
                TaskResource::InventoryMutation,
            ],
            || async {
                i.break_nearest(m, mv, l, b).await?;
                Ok(TaskResult::Unit)
            },
        )
        .await
        .map(|_| ())
    }
    pub async fn place_block(
        &self,
        m: &MinecraftClient,
        mv: &MovementService,
        l: &LookController,
        i: &InteractionController,
        t: BlockPosition,
        item: String,
    ) -> Result<(), AppError> {
        self.tracked(
            m,
            format!("Place {item}"),
            "Validate → navigate → look → place",
            [
                ActionResource::Movement,
                ActionResource::Rotation,
                ActionResource::Interaction,
                ActionResource::InventoryMutation,
            ],
        )
        .await?;
        let result = interaction
            .place_at(minecraft, movement, look, target, item)
            .await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
            &[
                TaskResource::Movement,
                TaskResource::Rotation,
                TaskResource::Interaction,
                TaskResource::InventoryMutation,
            ],
            || async {
                i.place_at(m, mv, l, t, item).await?;
                Ok(TaskResult::Unit)
            },
        )
        .await
        .map(|_| ())
    }
    pub async fn place_looked_block(
        &self,
        m: &MinecraftClient,
        mv: &MovementService,
        l: &LookController,
        i: &InteractionController,
        item: String,
    ) -> Result<(), AppError> {
        self.tracked(
            m,
            format!("Place {item}"),
            "Validate support → navigate → look → place",
            [
                ActionResource::Movement,
                ActionResource::Rotation,
                ActionResource::Interaction,
                ActionResource::InventoryMutation,
            ],
        )
        .await?;
        let result = interaction
            .place_looked(minecraft, movement, look, item)
            .await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
    }

    pub async fn cancel(&self, minecraft: &MinecraftClient) {
        if let Some((guard, _)) = self.active.lock().await.take() {
            guard.cancel();
        }
        let mut status = self.status.lock().await;
        status.state = TaskState::Cancelled;
        status.resources.clear();
        drop(status);
        minecraft.clear_current_task().await;
    }

    pub fn resource_manager(&self) -> ResourceManager {
        self.resources.clone()
    }

    /// Starts a new connection generation. Async completions carrying an old
    /// generation can then be rejected rather than applied after reconnect.
    pub fn start_session(&self) -> u64 {
        self.session.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }

    pub async fn guard_active(&self, connected: bool, alive: bool) -> Result<(), ActionFailure> {
        let active = self.active.lock().await;
        if let Some((guard, _)) = active.as_ref() {
            guard.check(self.session.load(Ordering::Acquire), connected, alive)
        } else {
            Ok(())
        }
    }

    pub async fn observe_terminal(
        &self,
        minecraft: &MinecraftClient,
        state: ActionState,
        failure: Option<String>,
    ) {
        if !state.is_terminal() {
            return;
        }
        let mut status = self.status.lock().await;
        if status.state != ActionState::Running {
            return;
        }
        status.state = state;
        status.failure = failure;
        status.resources.clear();
        drop(status);
        self.active.lock().await.take();
        minecraft.clear_current_task().await;
            &[
                TaskResource::Movement,
                TaskResource::Rotation,
                TaskResource::Interaction,
                TaskResource::InventoryMutation,
            ],
            || async {
                i.place_looked(m, mv, l, item).await?;
                Ok(TaskResult::Unit)
            },
        )
        .await
        .map(|_| ())
    }
    pub async fn gather_visible_blocks(
        &self,
        m: &MinecraftClient,
        mv: &MovementService,
        l: &LookController,
        i: &InteractionController,
        request: GatherRequest,
    ) -> Result<GatherResult, AppError> {
        let request = request
            .normalize()
            .map_err(|e| AppError::TaskRuntime(e.message))?;
        let ids = request.item_ids.clone();
        let initial_inventory = m.world_state_snapshot().await.inventory;
        let initial: u32 = ids.iter().map(|id| initial_inventory.count_item(id)).sum();
        let quantity = request.quantity;
        let maximum_attempts = request.maximum_attempts;
        let result=self.tracked(m,"Gather resources".into(),&[TaskResource::Movement,TaskResource::Rotation,TaskResource::Interaction,TaskResource::InventoryMutation],||async{
            let mut attempts=0;let mut child_failures=Vec::new();
            while attempts<maximum_attempts {let inventory=m.world_state_snapshot().await.inventory;let current:u32=ids.iter().map(|id|inventory.count_item(id)).sum();if current.saturating_sub(initial)>=quantity{return Ok(TaskResult::Gather(GatherResult{requested:quantity,collected:current-initial,matching_inventory_total:current,attempts,child_failures}));}attempts+=1;let mut broke=false;for block in &ids {match i.break_nearest(m,mv,l,block.clone()).await{Ok(())=>{broke=true;break},Err(error)=>child_failures.push(failure(TaskErrorCategory::Subsystem,error.to_string()))}}if !broke{break}}
            let inventory=m.world_state_snapshot().await.inventory;let current:u32=ids.iter().map(|id|inventory.count_item(id)).sum();let collected=current.saturating_sub(initial);if collected>=quantity{Ok(TaskResult::Gather(GatherResult{requested:quantity,collected,matching_inventory_total:current,attempts,child_failures}))}else{Err(AppError::TaskRuntime(format!("gather stopped after {attempts} attempts with {collected}/{quantity}; {} primitive failures",child_failures.len()))) }
        }).await?;
        match result {
            TaskResult::Gather(result) => Ok(result),
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Mock {
        count: u32,
        fail_at: Option<u32>,
    }
    impl PrimitiveExecutor for Mock {
        fn matching_item_count<'a>(&'a mut self, _: &'a [String]) -> PrimitiveFuture<'a, u32> {
            Box::pin(async move { Ok(self.count) })
        }
        fn collect_one<'a>(
            &'a mut self,
            _: &'a [String],
            _: &'a CancellationToken,
        ) -> PrimitiveFuture<'a, u32> {
            Box::pin(async move {
                if self.fail_at == Some(self.count) {
                    self.fail_at = None;
                    Err(failure(TaskErrorCategory::Subsystem, "mock failure"))
                } else {
                    self.count += 1;
                    Ok(1)
                }
            })
        }
        fn ensure_tool<'a>(
            &'a mut self,
            _: &'a EnsureToolRequest,
            _: &'a CancellationToken,
        ) -> PrimitiveFuture<'a, EnsureToolResult> {
            Box::pin(async { Err(failure(TaskErrorCategory::Unsupported, "mock")) })
        }
        fn clean_inventory<'a>(
            &'a mut self,
            _: &'a CleanInventoryRequest,
            _: &'a CancellationToken,
        ) -> PrimitiveFuture<'a, InventoryOperationResult> {
            Box::pin(async { Err(failure(TaskErrorCategory::Unsupported, "mock")) })
        }
        fn deposit_excess<'a>(
            &'a mut self,
            _: &'a DepositExcessRequest,
            _: &'a CancellationToken,
        ) -> PrimitiveFuture<'a, InventoryOperationResult> {
            Box::pin(async { Err(failure(TaskErrorCategory::Unsupported, "mock")) })
        }
        fn recover_prerequisite<'a>(
            &'a mut self,
            _: &'a RecoverPrerequisiteRequest,
            _: &'a CancellationToken,
        ) -> PrimitiveFuture<'a, RecoveryResult> {
            Box::pin(async { Err(failure(TaskErrorCategory::Unsupported, "mock")) })
        }
    }
    #[tokio::test]
    async fn lifecycle_and_typed_result_enter_history() {
        let rt = TaskService::new(2, Duration::from_millis(50));
        let id = rt.submit(TaskSubmission::command("test", "/test")).unwrap();
        rt.run(id, &[TaskResource::Movement], |_| async {
            Ok(TaskResult::Unit)
        })
        .await
        .unwrap();
        let s = rt.query(id).unwrap();
        assert_eq!(s.state, TaskState::Succeeded);
        assert_eq!(s.result, Some(TaskResult::Unit));
    }
    #[tokio::test]
    async fn cancellation_propagates_to_children() {
        let rt = TaskService::default();
        let p = rt.submit(TaskSubmission::command("parent", "p")).unwrap();
        let mut c = TaskSubmission::command("child", "c");
        c.parent_id = Some(p);
        let child = rt.submit(c).unwrap();
        assert!(rt.cancel_task(p, CancellationReason::User));
        assert_eq!(rt.query(child).unwrap().state, TaskState::Cancelling);
    }
    #[tokio::test]
    async fn recent_history_is_bounded() {
        let rt = TaskService::new(2, Duration::from_millis(50));
        for n in 0..3 {
            let id = rt
                .submit(TaskSubmission::command("x", n.to_string()))
                .unwrap();
            rt.run(id, &[], |_| async { Ok(TaskResult::Unit) })
                .await
                .unwrap();
        }
        assert_eq!(rt.recent().len(), 2);
    }
    #[tokio::test]
    async fn leases_are_ordered_and_waits_are_bounded() {
        let rt = TaskService::new(2, Duration::from_millis(5));
        let first = rt
            .submit(TaskSubmission::command("first", "first"))
            .unwrap();
        let second = rt
            .submit(TaskSubmission::command("second", "second"))
            .unwrap();
        let lease = rt
            .acquire(
                first,
                &[TaskResource::InventoryMutation, TaskResource::Movement],
            )
            .await
            .unwrap();
        assert_eq!(
            lease.resources,
            vec![TaskResource::Movement, TaskResource::InventoryMutation]
        );
        let error = match rt.acquire(second, &[TaskResource::Movement]).await {
            Ok(_) => panic!("conflicting lease unexpectedly acquired"),
            Err(error) => error,
        };
        assert_eq!(error.category, TaskErrorCategory::ResourceTimeout);
    }
    #[tokio::test]
    async fn mocked_collection_reports_progress_and_child_failures() {
        let rt = TaskService::default();
        let id = rt
            .submit(TaskSubmission::command("gather", "/gather"))
            .unwrap();
        let mut mock = Mock {
            count: 0,
            fail_at: Some(1),
        };
        let result = rt
            .run(
                id,
                &[TaskResource::Movement, TaskResource::InventoryMutation],
                |ctx| async move {
                    Ok(TaskResult::Gather(
                        collect_items(
                            &ctx,
                            &mut mock,
                            CollectItemsRequest {
                                item_ids: vec!["minecraft:stone".into()],
                                quantity: 2,
                                maximum_attempts: 3,
                            },
                        )
                        .await?,
                    ))
                },
            )
            .await
            .unwrap();
        let TaskResult::Gather(g) = result else {
            panic!()
        };
        assert_eq!(g.collected, 2);
        assert_eq!(g.child_failures.len(), 1);
    }
    #[test]
    fn normalized_gather_presets_are_bounded() {
        let r = GatherRequest::VisibleOres { quantity: 3 }
            .normalize()
            .unwrap();
        assert_eq!(r.quantity, 3);
        assert_eq!(r.maximum_attempts, 32);
        assert!(r.item_ids.contains(&"minecraft:diamond_ore".into()));
    }
}
