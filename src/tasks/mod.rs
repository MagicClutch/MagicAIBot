//! Reusable task entry points. Tasks own orchestration state only; Minecraft
//! behavior remains in the existing search, navigation, look and interaction
//! services.

use std::{sync::Arc, time::SystemTime};

use tokio::sync::Mutex;

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
    movement::MovementService,
    navigation::BlockNavigationService,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TaskState {
    #[default]
    Idle,
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct TaskStatus {
    pub id: u64,
    pub name: Option<String>,
    pub state: TaskState,
    pub current_step: Option<String>,
    pub started_at: Option<SystemTime>,
    pub failure: Option<String>,
}

#[derive(Clone, Default)]
pub struct TaskService {
    status: Arc<Mutex<TaskStatus>>,
}

impl TaskService {
    pub async fn status(&self) -> TaskStatus {
        self.status.lock().await.clone()
    }

    async fn begin(
        &self,
        minecraft: &MinecraftClient,
        name: impl Into<String>,
        step: impl Into<String>,
    ) {
        let (id, name, started_at) = {
            let mut status = self.status.lock().await;
            status.id = status.id.wrapping_add(1);
            status.name = Some(name.into());
            status.state = TaskState::Running;
            status.current_step = Some(step.into());
            status.started_at = Some(SystemTime::now());
            status.failure = None;
            (
                status.id,
                status.name.clone().unwrap_or_default(),
                status.started_at.unwrap_or(SystemTime::now()),
            )
        };
        minecraft
            .set_current_task(TaskSnapshot {
                name,
                id: id.to_string(),
                status: "running".into(),
                started_at,
            })
            .await;
    }

    async fn fail(&self, minecraft: &MinecraftClient, error: &AppError) {
        let mut status = self.status.lock().await;
        status.state = TaskState::Failed;
        status.failure = Some(error.to_string());
        minecraft.clear_current_task().await;
    }
    async fn complete(&self, minecraft: &MinecraftClient) {
        self.status.lock().await.state = TaskState::Completed;
        minecraft.clear_current_task().await;
    }

    pub async fn find_block(
        &self,
        minecraft: &MinecraftClient,
        search: &BlockSearchService,
        query: BlockSearchQuery,
    ) -> Result<Vec<BlockSnapshot>, AppError> {
        self.begin(
            minecraft,
            format!("Find {}", query.block_id),
            "Searching loaded chunks",
        )
        .await;
        let result = search.search_nearby(minecraft, query).await;
        match &result {
            Ok(_) => self.complete(minecraft).await,
            Err(error) => self.fail(minecraft, error).await,
        }
        result
    }

    pub async fn goto_position(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        position: PositionSnapshot,
    ) -> Result<(), AppError> {
        self.begin(minecraft, "Go to position", "Navigating").await;
        let result = movement.goto(minecraft, position).await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
    }

    pub async fn goto_block(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        navigation: &BlockNavigationService,
        block_id: String,
        radius: u32,
    ) -> Result<(), AppError> {
        self.begin(
            minecraft,
            format!("Go to {block_id}"),
            "Finding safe approach",
        )
        .await;
        let result = navigation
            .start(minecraft, movement, block_id, radius)
            .await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
    }

    pub async fn look_at(
        &self,
        minecraft: &MinecraftClient,
        look: &LookController,
        target: LookTarget,
    ) -> Result<(), AppError> {
        self.begin(minecraft, "Look at target", "Rotating").await;
        let result = look.look_at(minecraft, target).await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
    }

    pub async fn look_at_block(
        &self,
        minecraft: &MinecraftClient,
        look: &LookController,
        block_id: String,
    ) -> Result<(), AppError> {
        self.begin(
            minecraft,
            format!("Look at {block_id}"),
            "Selecting visible face",
        )
        .await;
        let result = look.look_at_block_id(minecraft, block_id).await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
    }

    pub async fn break_block(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
        target: BlockPosition,
    ) -> Result<(), AppError> {
        self.begin(
            minecraft,
            "Break block",
            "Validate → navigate → look → break",
        )
        .await;
        let result = interaction
            .break_at(minecraft, movement, look, target)
            .await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
    }

    pub async fn break_looked_block(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
    ) -> Result<(), AppError> {
        self.begin(
            minecraft,
            "Break looked block",
            "Validate → navigate → look → break",
        )
        .await;
        let result = interaction.break_looked(minecraft, movement, look).await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
    }

    pub async fn break_nearest_block(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
        block_id: String,
    ) -> Result<(), AppError> {
        self.begin(
            minecraft,
            format!("Break nearest {block_id}"),
            "Find → navigate → look → break",
        )
        .await;
        let result = interaction
            .break_nearest(minecraft, movement, look, block_id)
            .await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
    }

    pub async fn place_block(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
        target: BlockPosition,
        item: String,
    ) -> Result<(), AppError> {
        self.begin(
            minecraft,
            format!("Place {item}"),
            "Validate → navigate → look → place",
        )
        .await;
        let result = interaction
            .place_at(minecraft, movement, look, target, item)
            .await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
    }

    pub async fn place_looked_block(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        look: &LookController,
        interaction: &InteractionController,
        item: String,
    ) -> Result<(), AppError> {
        self.begin(
            minecraft,
            format!("Place {item}"),
            "Validate support → navigate → look → place",
        )
        .await;
        let result = interaction
            .place_looked(minecraft, movement, look, item)
            .await;
        if let Err(error) = &result {
            self.fail(minecraft, error).await;
        }
        result
    }

    pub async fn cancel(&self, minecraft: &MinecraftClient) {
        self.status.lock().await.state = TaskState::Cancelled;
        minecraft.clear_current_task().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_status_starts_idle() {
        assert_eq!(TaskStatus::default().state, TaskState::Idle);
    }
}
