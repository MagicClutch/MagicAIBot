use std::{collections::HashSet, sync::Arc, time::SystemTime};

use tokio::sync::Mutex;

use crate::{
    blocks::{
        block_query::BlockSearchQuery, block_search::BlockSearchService,
        block_snapshot::BlockSnapshot,
    },
    config::BlockNavigationConfig,
    error::AppError,
    logging,
    minecraft::{
        client::MinecraftClient,
        world_state::{BlockPosition, PositionSnapshot},
    },
    movement::{MovementService, NavigationMode},
    navigation::{
        approach_position::{approach_positions, is_valid_approach, required_cells},
        navigation_state::{
            BlockNavigationSnapshot, BlockNavigationState, arrival_valid, timed_out,
        },
        target_selector::next_candidate,
    },
};

struct NavigationInner {
    snapshot: BlockNavigationSnapshot,
    candidates: Vec<BlockSnapshot>,
    failed_blocks: HashSet<BlockPosition>,
    failed_approaches: HashSet<(BlockPosition, BlockPosition)>,
}

#[derive(Clone)]
pub struct BlockNavigationService {
    config: BlockNavigationConfig,
    search: BlockSearchService,
    inner: Arc<Mutex<NavigationInner>>,
}

impl BlockNavigationService {
    pub fn new(config: BlockNavigationConfig, search: BlockSearchService) -> Self {
        Self {
            config,
            search,
            inner: Arc::new(Mutex::new(NavigationInner {
                snapshot: BlockNavigationSnapshot::default(),
                candidates: Vec::new(),
                failed_blocks: HashSet::new(),
                failed_approaches: HashSet::new(),
            })),
        }
    }

    pub async fn snapshot(&self) -> BlockNavigationSnapshot {
        self.inner.lock().await.snapshot.clone()
    }

    pub async fn start(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        block_id: String,
        radius: u32,
        mode: NavigationMode,
    ) -> Result<(), AppError> {
        self.start_multi(minecraft, movement, vec![block_id], radius, mode)
            .await
    }

    /// Same contract as `start`, generalized to a set of block ids: scans
    /// for all of them and mines whichever candidate (of any of the
    /// requested ids) is nearest and reachable, falling back across
    /// candidates exactly as `start` does for a single id. This is what
    /// lets `#get` search every ore variant that produces a requested item
    /// (`diamond_ore` and `deepslate_diamond_ore` for `diamond`) and
    /// `#mine` accept multiple block targets in one call, without either
    /// duplicating this whole search/approach/retry pipeline.
    pub async fn start_multi(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        block_ids: Vec<String>,
        radius: u32,
        mode: NavigationMode,
    ) -> Result<(), AppError> {
        if radius == 0 || radius > self.config.maximum_search_radius {
            return Err(AppError::InvalidBlockSearchRadius {
                radius,
                maximum: self.config.maximum_search_radius,
            });
        }
        if block_ids.is_empty() {
            return Err(AppError::NoMatchingBlock);
        }
        let label = display_ids(&block_ids);
        let generation = {
            let mut inner = self.inner.lock().await;
            let generation = inner.snapshot.generation.wrapping_add(1);
            inner.snapshot = BlockNavigationSnapshot {
                state: BlockNavigationState::Searching,
                requested_block_id: block_ids.first().cloned(),
                requested_block_ids: block_ids.clone(),
                search_radius: Some(radius),
                start_time: Some(SystemTime::now()),
                last_progress_time: Some(SystemTime::now()),
                maximum_attempts: self.config.maximum_target_attempts,
                generation,
                mode,
                ..BlockNavigationSnapshot::default()
            };
            inner.candidates.clear();
            inner.failed_blocks.clear();
            inner.failed_approaches.clear();
            generation
        };

        let _ = movement.stop(minecraft).await;
        logging::info(format!("Searching for {label} within {radius} blocks"));
        let candidates = self.search_candidates(minecraft, &block_ids, radius).await;
        if candidates.is_empty() {
            self.fail(generation, "no matching block".into()).await;
            logging::warning(format!(
                "No loaded {label} block found within {radius} blocks"
            ));
            return Ok(());
        }
        {
            let mut inner = self.inner.lock().await;
            if inner.snapshot.generation != generation {
                return Ok(());
            }
            inner.candidates = candidates;
            inner.snapshot.state = BlockNavigationState::SelectingTarget;
        }
        if !self
            .try_next_target(minecraft, movement, generation)
            .await?
        {
            self.fail(generation, "no reachable approach position".into())
                .await;
            logging::warning(format!(
                "Cannot reach {label}: no reachable approach position"
            ));
        }
        Ok(())
    }

    /// Scans every id in `block_ids` and merges the results into one
    /// distance-sorted candidate list. Per-id search failures (an unloaded
    /// world, an invalid id slipping through) are treated as "no candidates
    /// from that id" rather than aborting the whole multi-id search --
    /// exactly one truly failing id must not prevent finding a perfectly
    /// good candidate from another.
    async fn search_candidates(
        &self,
        minecraft: &MinecraftClient,
        block_ids: &[String],
        radius: u32,
    ) -> Vec<BlockSnapshot> {
        let mut merged = Vec::new();
        for block_id in block_ids {
            let query = BlockSearchQuery {
                block_id: block_id.clone(),
                radius,
                maximum_results: self.config.candidate_limit,
                vertical_range: 0,
            };
            if let Ok(candidates) = self.search.search_raw(minecraft, query).await {
                merged.extend(candidates);
            }
        }
        crate::blocks::block_search::sort_deduplicate_limit(merged, self.config.candidate_limit)
    }

    /// Navigate to the existing exact block position using the same safe
    /// approach generation and pathfinding flow as `/gotoblock`.
    pub async fn start_exact(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        position: BlockPosition,
        block_id: String,
    ) -> Result<(), AppError> {
        let cells = minecraft.block_ids_at(&[position]).await?;
        if cells.get(&position).and_then(Option::as_ref) != Some(&block_id) {
            return Err(AppError::TargetBlockDisappeared);
        }
        let dimension = minecraft.world_state_snapshot().await.bot.dimension;
        let generation = {
            let mut inner = self.inner.lock().await;
            let generation = inner.snapshot.generation.wrapping_add(1);
            inner.snapshot = BlockNavigationSnapshot {
                state: BlockNavigationState::SelectingTarget,
                requested_block_id: Some(block_id.clone()),
                requested_block_ids: vec![block_id.clone()],
                search_radius: Some(0),
                start_time: Some(SystemTime::now()),
                last_progress_time: Some(SystemTime::now()),
                maximum_attempts: self.config.maximum_target_attempts,
                generation,
                // Interaction targets such as tree logs can be enclosed by
                // leaves or a lower trunk. Let Azalea mine only when its
                // route requires it; the interaction controller still owns
                // the requested target block.
                mode: NavigationMode::AllowMining,
                ..BlockNavigationSnapshot::default()
            };
            inner.candidates = vec![BlockSnapshot {
                position,
                block_id: block_id.clone(),
                distance: 0.0,
                dimension,
                chunk_loaded: true,
            }];
            inner.failed_blocks.clear();
            inner.failed_approaches.clear();
            generation
        };
        let _ = movement.stop(minecraft).await;
        if !self
            .try_next_target(minecraft, movement, generation)
            .await?
        {
            self.fail(generation, "no reachable approach position".into())
                .await;
            return Err(AppError::NoValidApproachPosition);
        }
        Ok(())
    }

    pub async fn cancel(&self, minecraft: &MinecraftClient, movement: &MovementService) {
        // An idle controller must be a no-op, the same contract
        // `InteractionController::cancel` follows -- console movement/look
        // commands call this defensively before starting something new, and
        // unconditionally stopping movement here races with an
        // immediately-following `/goto` submission: Azalea processes the
        // stale `StopPathfindingEvent` and the fresh `GotoEvent` on the same
        // tick, and if the stop handler happens to run after `goto_listener`
        // it silently drops the just-spawned path computation before its
        // result is ever collected, permanently stalling navigation.
        let mut inner = self.inner.lock().await;
        let was_active = matches!(
            inner.snapshot.state,
            BlockNavigationState::Searching
                | BlockNavigationState::SelectingTarget
                | BlockNavigationState::Moving
                | BlockNavigationState::Repathing
        );
        if was_active {
            inner.snapshot.state = BlockNavigationState::Cancelled;
            logging::info("Block navigation cancelled");
        }
        inner.snapshot.state = BlockNavigationState::Idle;
        inner.snapshot.selected_block_position = None;
        inner.snapshot.selected_approach_position = None;
        inner.candidates.clear();
        inner.failed_blocks.clear();
        inner.failed_approaches.clear();
        drop(inner);
        if was_active {
            let _ = movement.stop(minecraft).await;
        }
    }

    /// Reuse the existing bounded approach selection after a caller discovers
    /// that the current standing position has no clear interaction ray.
    pub async fn try_another_approach(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
    ) -> Result<bool, AppError> {
        let snapshot = self.snapshot().await;
        let (Some(target), Some(approach)) = (
            snapshot.selected_block_position,
            snapshot.selected_approach_position,
        ) else {
            return Ok(false);
        };
        self.mark_approach_failed(snapshot.generation, target, approach)
            .await;
        self.try_next_target(minecraft, movement, snapshot.generation)
            .await
    }

    pub async fn tick(&self, minecraft: &MinecraftClient, movement: &MovementService) {
        let snapshot = self.snapshot().await;
        if !matches!(
            snapshot.state,
            BlockNavigationState::Moving | BlockNavigationState::Repathing
        ) {
            return;
        }
        if snapshot.requested_block_ids.is_empty() {
            return;
        }
        let Some(target) = snapshot.selected_block_position else {
            return;
        };
        let Some(approach) = snapshot.selected_approach_position else {
            return;
        };
        // The specific id of the *currently selected* candidate, not the
        // whole requested set -- see `BlockNavigationSnapshot::selected_block_id`'s
        // doc comment for why re-validation must key off this one.
        let Some(selected_block_id) = snapshot.selected_block_id.clone() else {
            return;
        };
        let label = display_ids(&snapshot.requested_block_ids);
        let world = minecraft.world_state_snapshot().await;
        if world.bot.alive == Some(false) || !world.joined_world() {
            self.cancel(minecraft, movement).await;
            return;
        }
        if timed_out(snapshot.start_time, self.config.maximum_navigation_seconds) {
            self.fail(snapshot.generation, "navigation timed out".into())
                .await;
            let _ = movement.stop(minecraft).await;
            logging::warning(format!("Cannot reach {label}: timeout"));
            return;
        }

        let mut positions = vec![target];
        positions.extend(required_cells(approach));
        let Ok(cells) = minecraft.block_ids_at(&positions).await else {
            self.retry_or_fail(
                minecraft,
                movement,
                snapshot.generation,
                "target chunk unloaded",
            )
            .await;
            return;
        };
        if !is_valid_approach(
            target,
            approach,
            &selected_block_id,
            &cells,
            self.config.interaction_distance,
        ) {
            self.mark_approach_failed(snapshot.generation, target, approach)
                .await;
            self.retry_or_fail(
                minecraft,
                movement,
                snapshot.generation,
                "no reachable approach position",
            )
            .await;
            return;
        }

        // Tracks the last tick that made *positional* progress, separate
        // from `maximum_navigation_seconds`'s much longer overall ceiling --
        // defaults to `start_time` so a target that never moves at all from
        // the very first tick is still caught. Updated locally (not just via
        // the shared snapshot write below) so a meaningful change observed
        // on *this* tick is reflected in the stuck check on *this* same
        // tick, rather than one tick late.
        let mut effective_last_progress = snapshot.last_progress_time.or(snapshot.start_time);
        if let Some(position) = world.bot.position {
            if arrival_valid(
                Some(position),
                approach,
                target,
                self.config.arrival_distance,
                self.config.interaction_distance,
            ) {
                let _ = movement.stop(minecraft).await;
                let mut inner = self.inner.lock().await;
                inner.snapshot.state = BlockNavigationState::Reached;
                logging::success(format!(
                    "Reached {selected_block_id} at ({}, {}, {})",
                    target.x, target.y, target.z
                ));
                return;
            }
            let meaningful_change = snapshot
                .last_position
                .is_none_or(|previous| distance(previous, position) >= 0.1);
            if meaningful_change {
                let now = SystemTime::now();
                let mut inner = self.inner.lock().await;
                inner.snapshot.last_position = Some(position);
                inner.snapshot.last_progress_time = Some(now);
                drop(inner);
                effective_last_progress = Some(now);
            }
        }
        // `stuck_timeout_seconds` (much shorter than
        // `maximum_navigation_seconds`) exists specifically so a target that
        // stops making positional progress -- Azalea's pathfinder genuinely
        // stalled, not just slow -- gets abandoned for the next candidate
        // quickly instead of occupying the whole `#get`/`/gotoblock` attempt
        // for the full navigation ceiling while visibly doing nothing. Note
        // this only tracks *position*: a single mining move that takes a
        // few seconds to break one block is well within the default 12s and
        // won't false-positive, but an unusually slow multi-block dig could;
        // that's a tuning question for `stuck_timeout_seconds`, not a reason
        // to skip detecting a genuine stall.
        if timed_out(effective_last_progress, self.config.stuck_timeout_seconds) {
            self.mark_approach_failed(snapshot.generation, target, approach)
                .await;
            self.retry_or_fail(
                minecraft,
                movement,
                snapshot.generation,
                "no progress toward target",
            )
            .await;
            return;
        }

        let movement_snapshot = movement.snapshot().await;
        if matches!(
            movement_snapshot.status,
            crate::minecraft::world_state::MovementStatus::Failed
                | crate::minecraft::world_state::MovementStatus::Completed
        ) {
            self.mark_approach_failed(snapshot.generation, target, approach)
                .await;
            self.retry_or_fail(
                minecraft,
                movement,
                snapshot.generation,
                "no reachable approach position",
            )
            .await;
        }
        // Do not restart Azalea on an application timer. Its executor owns
        // mining, waits for block updates, stuck recovery and replanning; a
        // second start_goto here used to cancel those in-flight actions.
    }

    async fn try_next_target(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        generation: u64,
    ) -> Result<bool, AppError> {
        loop {
            let candidate = {
                let mut inner = self.inner.lock().await;
                if inner.snapshot.generation != generation
                    || inner.snapshot.current_attempt >= self.config.maximum_target_attempts
                {
                    return Ok(false);
                }
                let Some(candidate) =
                    next_candidate(&inner.candidates, &inner.failed_blocks).cloned()
                else {
                    return Ok(false);
                };
                inner.snapshot.candidates_checked += 1;
                candidate
            };
            for approach in approach_positions(candidate.position) {
                let approach_key = (candidate.position, approach);
                if self
                    .inner
                    .lock()
                    .await
                    .failed_approaches
                    .contains(&approach_key)
                {
                    continue;
                }
                let mut positions = vec![candidate.position];
                positions.extend(required_cells(approach));
                let cells = minecraft.block_ids_at(&positions).await?;
                if !is_valid_approach(
                    candidate.position,
                    approach,
                    &candidate.block_id,
                    &cells,
                    self.config.interaction_distance,
                ) {
                    self.mark_approach_failed(generation, candidate.position, approach)
                        .await;
                    continue;
                }
                let attempt = {
                    let mut inner = self.inner.lock().await;
                    inner.snapshot.current_attempt += 1;
                    inner.snapshot.current_attempt
                };
                let mode = self.inner.lock().await.snapshot.mode;
                if mode == NavigationMode::AllowMining {
                    logging::info("Pathfinding with mining enabled");
                }
                if movement
                    .goto_for_block_navigation(minecraft, center(approach), mode)
                    .await
                    .is_err()
                {
                    self.mark_approach_failed(generation, candidate.position, approach)
                        .await;
                    continue;
                }
                let mut inner = self.inner.lock().await;
                inner.snapshot.state = BlockNavigationState::Moving;
                inner.snapshot.selected_block_position = Some(candidate.position);
                inner.snapshot.selected_approach_position = Some(approach);
                inner.snapshot.selected_block_id = Some(candidate.block_id.clone());
                inner.snapshot.last_progress_time = Some(SystemTime::now());
                inner.snapshot.last_position = None;
                logging::info(format!(
                    "Going to {} at ({}, {}, {})",
                    candidate.block_id,
                    candidate.position.x,
                    candidate.position.y,
                    candidate.position.z
                ));
                logging::info(format!(
                    "Selected interaction position: {} {} {}",
                    approach.x, approach.y, approach.z
                ));
                let _ = attempt;
                return Ok(true);
            }
            self.mark_target_failed(generation, candidate.position)
                .await;
        }
    }

    async fn mark_target_failed(&self, generation: u64, target: BlockPosition) {
        let mut inner = self.inner.lock().await;
        if inner.snapshot.generation == generation {
            inner.failed_blocks.insert(target);
        }
    }

    async fn retry_or_fail(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        generation: u64,
        reason: &str,
    ) {
        if self
            .try_next_target(minecraft, movement, generation)
            .await
            .unwrap_or(false)
        {
            return;
        }
        let (block_ids, radius) = {
            let inner = self.inner.lock().await;
            (
                inner.snapshot.requested_block_ids.clone(),
                inner.snapshot.search_radius,
            )
        };
        if let Some(radius) = radius
            && !block_ids.is_empty()
        {
            let candidates = self.search_candidates(minecraft, &block_ids, radius).await;
            {
                let mut inner = self.inner.lock().await;
                for candidate in candidates {
                    if !inner.failed_blocks.contains(&candidate.position)
                        && !inner
                            .candidates
                            .iter()
                            .any(|existing| existing.position == candidate.position)
                    {
                        inner.candidates.push(candidate);
                    }
                }
            }
            if self
                .try_next_target(minecraft, movement, generation)
                .await
                .unwrap_or(false)
            {
                return;
            }
        }
        {
            self.fail(generation, reason.into()).await;
            let _ = movement.stop(minecraft).await;
            let label = display_ids(&self.snapshot().await.requested_block_ids);
            logging::warning(format!("Cannot reach {label}: {reason}"));
        }
    }

    async fn mark_approach_failed(
        &self,
        generation: u64,
        target: BlockPosition,
        approach: BlockPosition,
    ) {
        let mut inner = self.inner.lock().await;
        if inner.snapshot.generation == generation {
            inner.failed_approaches.insert((target, approach));
        }
    }

    async fn fail(&self, generation: u64, reason: String) {
        let mut inner = self.inner.lock().await;
        if inner.snapshot.generation == generation {
            inner.snapshot.state = BlockNavigationState::Failed;
            inner.snapshot.failure_reason = Some(reason);
        }
    }
}

/// Console-facing label for a set of requested block ids -- just the id
/// itself for the (overwhelmingly common) single-id case, so every existing
/// single-id log message is byte-for-byte unchanged; a comma-joined list
/// once `#get`/`#mine` are searching for more than one.
fn display_ids(block_ids: &[String]) -> String {
    block_ids.join(", ")
}

fn center(position: BlockPosition) -> PositionSnapshot {
    PositionSnapshot {
        x: f64::from(position.x) + 0.5,
        y: f64::from(position.y),
        z: f64::from(position.z) + 0.5,
    }
}

fn distance(a: PositionSnapshot, b: PositionSnapshot) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_ids_matches_single_id_wording_exactly() {
        // Single-id calls (the overwhelmingly common case: `/gotoblock`,
        // `InteractionController`'s auto-navigate, a plain `#get oak_log`)
        // must produce byte-identical log text to before multi-id support
        // existed -- no comma, no brackets.
        assert_eq!(
            display_ids(&["minecraft:oak_log".to_owned()]),
            "minecraft:oak_log"
        );
    }

    #[test]
    fn display_ids_joins_multiple_ids_for_multi_block_searches() {
        assert_eq!(
            display_ids(&[
                "minecraft:diamond_ore".to_owned(),
                "minecraft:deepslate_diamond_ore".to_owned(),
            ]),
            "minecraft:diamond_ore, minecraft:deepslate_diamond_ore"
        );
    }
}
