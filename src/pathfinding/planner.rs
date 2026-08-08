//! Async path calculation: runs block-level A* off the bot's own thread, one
//! search at a time, cancellable at any moment.
//!
//! # Why this module exists at all
//!
//! The bot runs on a current-thread tokio runtime (see `main.rs`): every
//! controller tick, every packet, and the console all share one thread. A
//! second of A* on that thread is a second in which the bot does not react
//! to anything -- it would stand still mid-fight, ignore `/stop`, and stop
//! answering chat. So the search runs on `spawn_blocking`'s thread pool,
//! which exists precisely for CPU-bound work, and the controller polls for
//! the result on its normal tick.
//!
//! # Cancellation
//!
//! [`Planner::cancel`] flips an `AtomicBool` the search polls (both up front
//! and every few hundred expansions), so a new destination never waits
//! behind the old search. Cancelling also takes the task handle and drops
//! it, which is what makes a stale result *structurally* impossible rather
//! than merely unlikely: [`Planner::poll`] can only ever return the result
//! of the one search currently in flight, so there is nothing to
//! version-check.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tokio::task::JoinHandle;

use crate::{
    config::{PathfindingConfig, PathfindingCostConfig, VerticalNavigationConfig},
    minecraft::world_state::BlockPosition,
    pathfinding::{
        astar::{Goal, PathResult, SearchBudget, SearchRequest, search},
        cost::CostProfile,
        grid::TerrainGrid,
        moves::MovePolicy,
    },
};

/// A search that finished.
pub struct CompletedSearch {
    pub result: PathResult,
    /// Which segment this search was for, so the controller can apply it to
    /// the right slot rather than assuming it is still on the same one --
    /// the *next* segment is routinely searched while the current one is
    /// still being walked.
    pub segment_index: usize,
}

/// Owns the one in-flight search. Not `Clone`: exactly one of these exists
/// per navigation controller, which is what makes "one search at a time"
/// structural rather than a convention.
pub struct Planner {
    in_flight: Option<InFlight>,
}

struct InFlight {
    handle: JoinHandle<PathResult>,
    cancelled: Arc<AtomicBool>,
    segment_index: usize,
}

impl Default for Planner {
    fn default() -> Self {
        Self::new()
    }
}

impl Planner {
    #[must_use]
    pub fn new() -> Self {
        Self { in_flight: None }
    }

    #[must_use]
    pub fn is_searching(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Starts a search on the blocking pool, replacing (and cancelling) any
    /// search already running.
    #[expect(
        clippy::too_many_arguments,
        reason = "one call site, and every argument is an independent input to                   the search; bundling them into a struct here would only                   rebuild `SearchRequest`, which this already constructs"
    )]
    pub fn start(
        &mut self,
        grid: TerrainGrid,
        profile: CostProfile,
        policy: MovePolicy,
        budget: SearchBudget,
        start: BlockPosition,
        goal: Goal,
        segment_index: usize,
    ) {
        self.cancel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let request = SearchRequest {
            grid,
            profile,
            policy,
            budget,
            start,
            goal,
            cancelled: Arc::clone(&cancelled),
        };
        // `spawn_blocking` rather than `spawn`: this is CPU-bound work on a
        // current-thread runtime, so a plain task would simply occupy the
        // one thread everything else needs.
        let handle = tokio::task::spawn_blocking(move || search(&request));
        self.in_flight = Some(InFlight {
            handle,
            cancelled,
            segment_index,
        });
    }

    /// Abandons the in-flight search, if any. Returns whether there was one.
    ///
    /// Does not wait for the thread to notice: the flag is set and the handle
    /// is dropped, so whatever the search eventually produces has nowhere to
    /// go. Blocking here would defeat the point of running it off-thread in
    /// the first place.
    pub fn cancel(&mut self) -> bool {
        let Some(in_flight) = self.in_flight.take() else {
            return false;
        };
        in_flight.cancelled.store(true, Ordering::Relaxed);
        in_flight.handle.abort();
        true
    }

    /// Non-blocking poll for a finished search.
    ///
    /// Returns `None` while the search is still running -- this is called
    /// from the controller's tick, so it must never await the result.
    pub fn poll(&mut self) -> Option<CompletedSearch> {
        let in_flight = self.in_flight.as_mut()?;
        if !in_flight.handle.is_finished() {
            return None;
        }
        let in_flight = self.in_flight.take()?;
        let InFlight {
            handle,
            segment_index,
            ..
        } = in_flight;
        // The task is finished, so polling the handle resolves immediately;
        // a panicked or aborted search yields `Err`, which is reported as
        // "no result" and retried by the caller's own replan path rather
        // than propagated as a crash.
        let result = poll_finished(handle)?;
        Some(CompletedSearch {
            result,
            segment_index,
        })
    }
}

/// Extracts the value from an already-finished `JoinHandle` without
/// awaiting.
///
/// `poll` has already checked `is_finished`, so the future is guaranteed
/// ready and this cannot actually block; it is written as a manual poll with
/// a no-op waker rather than `block_on` precisely so that it can never
/// re-enter the runtime from inside a runtime thread.
fn poll_finished(mut handle: JoinHandle<PathResult>) -> Option<PathResult> {
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
    };

    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(std::ptr::null(), &VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    // SAFETY: the vtable's clone/wake/drop are all no-ops that ignore the
    // (null) data pointer, so no invalid pointer is ever dereferenced.
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut context = Context::from_waker(&waker);
    match Pin::new(&mut handle).poll(&mut context) {
        Poll::Ready(Ok(result)) => Some(result),
        // Panicked or aborted: the caller treats a missing result the same
        // as a failed search and replans.
        Poll::Ready(Err(_)) => None,
        Poll::Pending => None,
    }
}

/// Builds the search cost profile from user configuration.
#[must_use]
pub fn cost_profile(costs: &PathfindingCostConfig) -> CostProfile {
    CostProfile {
        walk: costs.walk,
        diagonal: costs.diagonal,
        jump_up: costs.jump_up,
        drop_per_block: costs.drop_per_block,
        max_safe_drop: costs.max_safe_drop,
        fall_damage_penalty: costs.fall_damage_penalty,
        swim: costs.swim,
        break_block: costs.break_block,
        bridge_block: costs.bridge_block,
        climb: costs.climb,
        hazard: costs.hazard,
        lava_avoidance: costs.lava_avoidance,
        hazard_proximity: costs.hazard_proximity,
        ledge_proximity: costs.ledge_proximity,
        entity_hazard: costs.entity_hazard,
    }
}

/// Builds the move policy from user configuration.
///
/// Bridging is the intersection of what the *planner* is allowed to route
/// (`[pathfinding] allow_bridging`) and what the *executor* is allowed to
/// build (`[vertical_navigation]`), rather than the planner's setting alone.
/// Planning a bridge the executor will refuse to build is the worst of both
/// worlds: the segment looks fine, the bot walks to the edge of the gap, and
/// then sits there until the stuck timer fires and it replans the same route
/// again. Deriving it means the two settings cannot drift apart.
///
/// `entity_hazards` is left empty here and filled in per search from live
/// world state -- it is the one part of the policy that changes between two
/// otherwise identical searches.
#[must_use]
pub fn move_policy(config: &PathfindingConfig, vertical: &VerticalNavigationConfig) -> MovePolicy {
    MovePolicy {
        allow_breaking: config.allow_breaking,
        allow_bridging: config.allow_bridging && vertical.enabled && vertical.allow_bridging,
        max_drop: config.max_drop,
        allow_swimming: config.allow_swimming,
        allow_lava: false,
        entity_hazards: Vec::new(),
    }
}

/// Builds the search budget from user configuration.
#[must_use]
pub fn search_budget(config: &PathfindingConfig) -> SearchBudget {
    SearchBudget {
        max_nodes: config.max_search_nodes,
        max_duration: std::time::Duration::from_millis(config.search_timeout_ms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathfinding::{grid::GridBounds, terrain::TerrainClass};

    fn position(x: i32, y: i32, z: i32) -> BlockPosition {
        BlockPosition { x, y, z }
    }

    fn plain(radius: i32) -> TerrainGrid {
        let bounds = GridBounds {
            min: position(-radius, 55, -radius),
            max: position(radius, 75, radius),
        };
        let mut grid = TerrainGrid::empty(bounds);
        for x in -radius..radius {
            for z in -radius..radius {
                for y in 55..=63 {
                    grid.set(position(x, y, z), TerrainClass::Solid);
                }
                for y in 64..75 {
                    grid.set(position(x, y, z), TerrainClass::Air);
                }
            }
        }
        grid
    }

    fn start_search(planner: &mut Planner, radius: i32, goal: BlockPosition, index: usize) {
        planner.start(
            plain(radius),
            CostProfile::default(),
            MovePolicy::default(),
            SearchBudget::default(),
            position(0, 64, 0),
            Goal::within(goal, 0.0),
            index,
        );
    }

    #[tokio::test]
    async fn a_fresh_planner_is_idle() {
        let mut planner = Planner::new();
        assert!(!planner.is_searching());
        assert!(planner.poll().is_none());
        assert!(!planner.cancel());
    }

    #[tokio::test]
    async fn a_search_runs_off_thread_and_delivers_its_result() {
        let mut planner = Planner::new();
        start_search(&mut planner, 32, position(12, 64, 0), 3);
        assert!(planner.is_searching());
        let completed = loop {
            if let Some(completed) = planner.poll() {
                break completed;
            }
            tokio::task::yield_now().await;
        };
        assert_eq!(completed.segment_index, 3);
        assert_eq!(completed.result.destination(), position(12, 64, 0));
        assert!(!planner.is_searching());
    }

    #[tokio::test]
    async fn polling_never_blocks_while_a_search_is_still_running() {
        let mut planner = Planner::new();
        // A big grid and a far goal so the search is genuinely still going.
        start_search(&mut planner, 128, position(120, 64, 120), 0);
        let started = std::time::Instant::now();
        let _ = planner.poll();
        assert!(
            started.elapsed() < std::time::Duration::from_millis(50),
            "poll must return immediately whether or not the search is done"
        );
        planner.cancel();
    }

    #[tokio::test]
    async fn cancelling_stops_reporting_the_search_as_in_flight() {
        let mut planner = Planner::new();
        start_search(&mut planner, 128, position(120, 64, 120), 0);
        assert!(planner.cancel());
        assert!(!planner.is_searching());
        assert!(planner.poll().is_none());
    }

    #[tokio::test]
    async fn starting_a_new_search_supersedes_the_previous_one() {
        let mut planner = Planner::new();
        start_search(&mut planner, 128, position(120, 64, 120), 0);
        start_search(&mut planner, 32, position(8, 64, 0), 1);
        let completed = loop {
            if let Some(completed) = planner.poll() {
                break completed;
            }
            tokio::task::yield_now().await;
        };
        assert_eq!(
            completed.segment_index, 1,
            "the delivered result must be the newer search's"
        );
        assert!(!planner.is_searching());
    }

    #[tokio::test]
    async fn a_cancelled_search_can_never_deliver_its_result() {
        let mut planner = Planner::new();
        start_search(&mut planner, 128, position(120, 64, 120), 0);
        planner.cancel();
        // Give the blocking thread every chance to finish anyway; its result
        // has nowhere to go now that the handle is gone.
        for _ in 0..50 {
            tokio::task::yield_now().await;
            assert!(planner.poll().is_none());
        }
    }

    #[test]
    fn configuration_maps_onto_the_cost_profile_one_to_one() {
        let costs = PathfindingCostConfig {
            break_block: 42.0,
            lava_avoidance: 999.0,
            ..PathfindingCostConfig::default()
        };
        let profile = cost_profile(&costs);
        assert_eq!(profile.break_block, 42.0);
        assert_eq!(profile.lava_avoidance, 999.0);
        assert_eq!(profile.walk, costs.walk);
    }

    #[test]
    fn configuration_maps_onto_the_move_policy() {
        let config = PathfindingConfig {
            allow_breaking: false,
            allow_swimming: false,
            max_drop: 2,
            ..PathfindingConfig::default()
        };
        let policy = move_policy(&config, &VerticalNavigationConfig::default());
        assert!(!policy.allow_breaking);
        assert!(!policy.allow_swimming);
        assert_eq!(policy.max_drop, 2);
        assert!(
            !policy.allow_lava,
            "lava is never opened up by configuration"
        );
    }

    #[test]
    fn bridging_needs_both_the_planner_and_the_executor_to_allow_it() {
        let config = PathfindingConfig {
            allow_bridging: true,
            ..PathfindingConfig::default()
        };
        let vertical = VerticalNavigationConfig::default();
        assert_eq!(
            move_policy(&config, &vertical).allow_bridging,
            vertical.enabled && vertical.allow_bridging
        );

        let no_building = VerticalNavigationConfig {
            enabled: false,
            ..VerticalNavigationConfig::default()
        };
        assert!(
            !move_policy(&config, &no_building).allow_bridging,
            "the planner must not route a bridge the executor won't build"
        );

        let planner_off = PathfindingConfig {
            allow_bridging: false,
            ..PathfindingConfig::default()
        };
        assert!(!move_policy(&planner_off, &vertical).allow_bridging);
    }

    #[test]
    fn configuration_maps_onto_the_search_budget() {
        let config = PathfindingConfig {
            max_search_nodes: 1234,
            search_timeout_ms: 777,
            ..PathfindingConfig::default()
        };
        let budget = search_budget(&config);
        assert_eq!(budget.max_nodes, 1234);
        assert_eq!(budget.max_duration, std::time::Duration::from_millis(777));
    }
}
