//! Budgeted, cancellable A* over a [`TerrainGrid`]. Pure and synchronous --
//! it is the CPU-bound core that `crate::pathfinding::planner` hands to a
//! blocking thread, so it deliberately knows nothing about tokio, Azalea, or
//! the bot.
//!
//! Three properties matter more here than raw speed:
//!
//! 1. **It always answers.** Every search is bounded by a node budget *and*
//!    a wall-clock budget. Exceeding either is not a failure: the search
//!    returns the best partial route it found (see [`PathOutcome::Partial`]),
//!    which is what makes long-distance travel work at all -- Baritone's
//!    core trick is walking a good partial path while the next one is
//!    computed, rather than waiting for a perfect one.
//! 2. **It can be abandoned mid-flight.** A shared cancellation flag is
//!    polled every few hundred expansions, so a new destination doesn't wait
//!    for the old search to finish.
//! 3. **It never lies about unknown terrain.** Successor generation refuses
//!    to leave known cells (see `crate::pathfinding::moves`), so a search
//!    that runs out of *world* rather than out of budget reports
//!    [`PathOutcome::Partial`] too, and the caller walks to that frontier to
//!    make the server stream more chunks.

use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
    time::{Duration, Instant},
};

use crate::{
    minecraft::world_state::BlockPosition,
    pathfinding::{
        cost::{CostProfile, MoveKind},
        grid::{TerrainGrid, block_distance},
        moves::{BODY_HEIGHT, Move, MovePolicy, successors},
    },
};

/// How often (in expanded nodes) the cancellation flag and the wall-clock
/// deadline are checked. Checking every node would put a clock read in the
/// hot loop for no benefit; 256 nodes is well under a millisecond of work.
const BUDGET_CHECK_INTERVAL: usize = 256;

/// Limits on one search. Both are hard caps -- whichever is hit first ends
/// the search with the best partial route so far.
#[derive(Clone, Copy, Debug)]
pub struct SearchBudget {
    pub max_nodes: usize,
    pub max_duration: Duration,
}

impl Default for SearchBudget {
    fn default() -> Self {
        Self {
            max_nodes: 20_000,
            max_duration: Duration::from_millis(1500),
        }
    }
}

/// Why a search stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathOutcome {
    /// Reached the goal (or came within the goal radius).
    Complete,
    /// Ran out of budget, or out of known world, before reaching the goal.
    /// The path still leads somewhere genuinely useful -- the reachable cell
    /// with the best heuristic seen -- which the caller walks while the next
    /// search runs.
    Partial,
    /// Nothing was reachable at all: the start itself is enclosed, or every
    /// successor was unknown/forbidden. Distinct from `Partial` because
    /// there is nothing to walk.
    Unreachable,
    /// The caller flipped the cancellation flag.
    Cancelled,
}

impl PathOutcome {
    #[must_use]
    pub fn usable(self) -> bool {
        matches!(self, Self::Complete | Self::Partial)
    }
}

/// Result of one search.
#[derive(Clone, Debug)]
pub struct PathResult {
    /// Cells from the start (inclusive) to the last reached cell. Length 1
    /// (just the start) whenever the outcome is `Unreachable`/`Cancelled`.
    pub nodes: Vec<BlockPosition>,
    /// The move used to enter each node, parallel to `nodes[1..]` -- so
    /// `moves[i]` is how the bot gets from `nodes[i]` to `nodes[i + 1]`.
    /// This is what segment "required actions" are derived from.
    pub moves: Vec<MoveKind>,
    pub outcome: PathOutcome,
    /// Total cost of `nodes`, in the cost model's units.
    pub cost: f64,
    /// Nodes actually expanded -- surfaced for debug output and for tuning
    /// `SearchBudget` against real terrain.
    pub expanded: usize,
    pub elapsed: Duration,
    /// Whether the chosen route actually passes through or beside something
    /// harmful. The cost model already made that trade deliberately (a
    /// hazard is only ever crossed when every detour is worse); this reports
    /// it, so the segment can warn rather than walking the bot through fire
    /// silently. See `crate::pathfinding::segment::SegmentAction::AvoidDanger`.
    pub hazardous: bool,
}

impl PathResult {
    fn trivial(
        start: BlockPosition,
        outcome: PathOutcome,
        expanded: usize,
        elapsed: Duration,
    ) -> Self {
        Self {
            nodes: vec![start],
            moves: Vec::new(),
            outcome,
            cost: 0.0,
            expanded,
            elapsed,
            hazardous: false,
        }
    }

    /// The last cell of the path -- where the bot ends up if it walks the
    /// whole thing.
    #[must_use]
    pub fn destination(&self) -> BlockPosition {
        *self.nodes.last().expect("a path always contains its start")
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }
}

/// The goal of a search: a cell plus how close counts as arriving.
#[derive(Clone, Copy, Debug)]
pub struct Goal {
    pub position: BlockPosition,
    /// Blocks of slack. A goal inside a wall (or one block above the floor)
    /// is otherwise unreachable and would burn the entire budget proving it.
    pub radius: f64,
}

impl Goal {
    #[must_use]
    pub fn within(position: BlockPosition, radius: f64) -> Self {
        Self { position, radius }
    }

    #[must_use]
    pub fn reached_by(&self, position: BlockPosition) -> bool {
        block_distance(position, self.position) <= self.radius.max(0.0) + 1e-9
    }
}

/// Node ordering for the open set: lowest f first, ties broken toward the
/// lower h (deeper into the search, which reaches a usable partial sooner).
#[derive(Clone, Copy, Debug)]
struct Candidate {
    position: BlockPosition,
    f_score: f64,
    h_score: f64,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Candidate {}
impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        // `BinaryHeap` is a max-heap; reverse so the smallest f pops first.
        other
            .f_score
            .total_cmp(&self.f_score)
            .then_with(|| other.h_score.total_cmp(&self.h_score))
    }
}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy)]
struct Visit {
    g_score: f64,
    parent: Option<(BlockPosition, MoveKind)>,
}

/// Everything one search needs, bundled so the signature stays readable and
/// so `planner` can build it on the async side and move it wholesale onto a
/// blocking thread.
pub struct SearchRequest {
    pub grid: TerrainGrid,
    pub profile: CostProfile,
    pub policy: MovePolicy,
    pub budget: SearchBudget,
    pub start: BlockPosition,
    pub goal: Goal,
    /// Flipped by the caller to abandon the search. Polled once up front
    /// and every [`BUDGET_CHECK_INTERVAL`] expansions after that.
    pub cancelled: Arc<AtomicBool>,
}

/// Runs one A* search to completion, to the budget, or to cancellation.
///
/// Blocking and CPU-bound by design -- call it from `spawn_blocking`, never
/// on the runtime thread.
#[must_use]
pub fn search(request: &SearchRequest) -> PathResult {
    let started = Instant::now();
    let SearchRequest {
        grid,
        profile,
        policy,
        budget,
        start,
        goal,
        cancelled,
    } = request;

    // Checked up front as well as every `BUDGET_CHECK_INTERVAL` expansions:
    // a short search over open terrain can finish inside one check interval,
    // and a search cancelled before it ever started must not deliver a path
    // into a plan that has already moved on.
    if cancelled.load(AtomicOrdering::Relaxed) {
        return PathResult::trivial(*start, PathOutcome::Cancelled, 0, started.elapsed());
    }

    // A start that isn't a legal standing position poisons everything after
    // it (no successor generator can leave a cell the body doesn't fit in),
    // so snap to the nearest legal cell rather than reporting a confusing
    // "unreachable" for what is really a half-block of Y drift.
    let start = grid
        .nearest_standable(*start, BODY_HEIGHT, 3)
        .or_else(|| grid.swimmable(*start, BODY_HEIGHT).then_some(*start))
        .unwrap_or(*start);

    let mut visited: HashMap<BlockPosition, Visit> = HashMap::new();
    let mut open = BinaryHeap::new();
    let mut successor_buffer: Vec<Move> = Vec::with_capacity(16);

    let start_h = profile.heuristic(block_distance(start, goal.position));
    visited.insert(
        start,
        Visit {
            g_score: 0.0,
            parent: None,
        },
    );
    open.push(Candidate {
        position: start,
        f_score: start_h,
        h_score: start_h,
    });

    // Best node seen by heuristic, which is what a partial path leads to.
    let mut best_node = start;
    let mut best_h = start_h;
    let mut expanded = 0usize;

    while let Some(current) = open.pop() {
        if goal.reached_by(current.position) {
            return reconstruct(
                grid,
                &visited,
                current.position,
                PathOutcome::Complete,
                expanded,
                started.elapsed(),
            );
        }

        // A stale heap entry for a node already reached more cheaply.
        let current_g = match visited.get(&current.position) {
            Some(visit) => visit.g_score,
            None => continue,
        };
        if current.f_score > current_g + current.h_score + 1e-9 {
            continue;
        }

        expanded += 1;
        if expanded.is_multiple_of(BUDGET_CHECK_INTERVAL) {
            if cancelled.load(AtomicOrdering::Relaxed) {
                return reconstruct(
                    grid,
                    &visited,
                    best_node,
                    PathOutcome::Cancelled,
                    expanded,
                    started.elapsed(),
                );
            }
            if started.elapsed() >= budget.max_duration {
                break;
            }
        }
        if expanded >= budget.max_nodes {
            break;
        }

        successors(
            grid,
            profile,
            policy,
            current.position,
            &mut successor_buffer,
        );
        for candidate_move in successor_buffer.iter().copied() {
            let tentative_g = current_g + candidate_move.cost;
            let known_better = visited
                .get(&candidate_move.destination)
                .is_some_and(|visit| visit.g_score <= tentative_g + 1e-9);
            if known_better {
                continue;
            }
            visited.insert(
                candidate_move.destination,
                Visit {
                    g_score: tentative_g,
                    parent: Some((current.position, candidate_move.kind)),
                },
            );
            let h = profile.heuristic(block_distance(candidate_move.destination, goal.position));
            if h < best_h {
                best_h = h;
                best_node = candidate_move.destination;
            }
            open.push(Candidate {
                position: candidate_move.destination,
                f_score: tentative_g + h,
                h_score: h,
            });
        }
    }

    if best_node == start {
        return PathResult::trivial(start, PathOutcome::Unreachable, expanded, started.elapsed());
    }
    reconstruct(
        grid,
        &visited,
        best_node,
        PathOutcome::Partial,
        expanded,
        started.elapsed(),
    )
}

fn reconstruct(
    grid: &TerrainGrid,
    visited: &HashMap<BlockPosition, Visit>,
    end: BlockPosition,
    outcome: PathOutcome,
    expanded: usize,
    elapsed: Duration,
) -> PathResult {
    let mut nodes = vec![end];
    let mut moves = Vec::new();
    let mut cursor = end;
    while let Some(Visit {
        parent: Some((parent, kind)),
        ..
    }) = visited.get(&cursor)
    {
        nodes.push(*parent);
        moves.push(*kind);
        cursor = *parent;
    }
    nodes.reverse();
    moves.reverse();
    let cost = visited.get(&end).map_or(0.0, |visit| visit.g_score);
    if nodes.len() <= 1 {
        return PathResult::trivial(end, outcome, expanded, elapsed);
    }
    let hazardous = nodes.iter().any(|node| is_hazardous(grid, *node));
    PathResult {
        nodes,
        moves,
        outcome,
        cost,
        expanded,
        elapsed,
        hazardous,
    }
}

/// Whether standing at `node` means standing in, on, or right beside
/// something harmful.
fn is_hazardous(grid: &TerrainGrid, node: BlockPosition) -> bool {
    let here = grid.get(node);
    let floor = grid.floor_below(node);
    if here.damaging() || here.lethal() || floor.damaging() || floor.lethal() {
        return true;
    }
    [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dz)| {
        let neighbor = grid.get(BlockPosition {
            x: node.x + dx,
            y: node.y,
            z: node.z + dz,
        });
        neighbor.lethal() || neighbor.damaging()
    })
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

    /// A goal that must be reached exactly -- the shape most of these tests
    /// want, and deliberately not part of the production API, where every
    /// goal carries the arrival slack its caller configured.
    fn exact(position: BlockPosition) -> Goal {
        Goal::within(position, 0.0)
    }

    fn request(grid: TerrainGrid, start: BlockPosition, goal: Goal) -> SearchRequest {
        SearchRequest {
            grid,
            profile: CostProfile::default(),
            policy: MovePolicy::default(),
            budget: SearchBudget::default(),
            start,
            goal,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn finds_a_straight_route_across_open_ground() {
        let result = search(&request(
            plain(32),
            position(0, 64, 0),
            exact(position(10, 64, 0)),
        ));
        assert_eq!(result.outcome, PathOutcome::Complete);
        assert_eq!(result.destination(), position(10, 64, 0));
        assert_eq!(
            result.nodes.len(),
            11,
            "one node per block of a straight walk"
        );
        assert_eq!(result.moves.len(), result.nodes.len() - 1);
    }

    #[test]
    fn prefers_diagonals_over_an_l_shaped_route() {
        let result = search(&request(
            plain(32),
            position(0, 64, 0),
            exact(position(8, 64, 8)),
        ));
        assert_eq!(result.outcome, PathOutcome::Complete);
        assert_eq!(
            result.nodes.len(),
            9,
            "eight diagonal steps, not sixteen orthogonal ones"
        );
        assert!(result.moves.iter().all(|kind| *kind == MoveKind::Diagonal));
    }

    #[test]
    fn routes_around_a_wall_rather_than_through_it_when_mining_is_off() {
        let mut grid = plain(32);
        // A wall across x=5, with a gap at z=3.
        for z in -32..32 {
            if z == 3 {
                continue;
            }
            for y in 64..=66 {
                grid.set(position(5, y, z), TerrainClass::Solid);
            }
        }
        let mut req = request(grid, position(0, 64, 0), exact(position(10, 64, 0)));
        req.policy.allow_breaking = false;
        let result = search(&req);
        assert_eq!(result.outcome, PathOutcome::Complete);
        assert!(
            result.nodes.iter().any(|node| node.z == 3),
            "the only way through is the gap at z=3"
        );
    }

    #[test]
    fn mines_through_a_thin_wall_when_going_around_is_far_enough() {
        let mut grid = plain(32);
        for z in -32..32 {
            for y in 64..=66 {
                grid.set(position(5, y, z), TerrainClass::Solid);
            }
        }
        let result = search(&request(
            grid,
            position(0, 64, 0),
            exact(position(10, 64, 0)),
        ));
        assert_eq!(result.outcome, PathOutcome::Complete);
        assert!(
            result
                .moves
                .iter()
                .any(|kind| matches!(kind, MoveKind::Break { .. })),
            "a fully-enclosing wall leaves mining as the only route"
        );
    }

    #[test]
    fn walks_around_lava_instead_of_through_it() {
        let mut grid = plain(32);
        for z in -2..=2 {
            grid.set(position(5, 64, z), TerrainClass::Lava);
        }
        let result = search(&request(
            grid,
            position(0, 64, 0),
            exact(position(10, 64, 0)),
        ));
        assert_eq!(result.outcome, PathOutcome::Complete);
        assert!(
            result
                .nodes
                .iter()
                .all(|node| !(node.x == 5 && (-2..=2).contains(&node.z))),
            "no node may sit in the lava"
        );
    }

    #[test]
    fn an_enclosed_start_is_unreachable_rather_than_partial() {
        let mut grid = plain(8);
        for (dx, dz) in [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ] {
            for y in 64..=65 {
                grid.set(position(dx, y, dz), TerrainClass::Unbreakable);
            }
        }
        grid.set(position(0, 66, 0), TerrainClass::Unbreakable);
        let result = search(&request(
            grid,
            position(0, 64, 0),
            exact(position(6, 64, 0)),
        ));
        assert_eq!(result.outcome, PathOutcome::Unreachable);
        assert!(result.is_empty());
    }

    #[test]
    fn a_goal_outside_the_known_world_yields_a_partial_path_to_the_frontier() {
        let grid = plain(16);
        let result = search(&request(
            grid,
            position(0, 64, 0),
            // Far outside the sampled grid: the search can only get as far
            // as the edge of what is known.
            exact(position(500, 64, 0)),
        ));
        assert_eq!(result.outcome, PathOutcome::Partial);
        assert!(!result.is_empty(), "a partial path must still be walkable");
        assert!(
            result.destination().x >= 14,
            "the partial path should end near the frontier, got {:?}",
            result.destination()
        );
    }

    #[test]
    fn a_goal_radius_lets_an_unstandable_goal_still_succeed() {
        let mut grid = plain(16);
        // Bury the goal cell in stone: only a radius makes it reachable.
        for y in 64..=66 {
            grid.set(position(8, y, 0), TerrainClass::Unbreakable);
        }
        let exact = search(&request(
            grid.clone(),
            position(0, 64, 0),
            exact(position(8, 64, 0)),
        ));
        assert_ne!(exact.outcome, PathOutcome::Complete);

        let loose = search(&request(
            grid,
            position(0, 64, 0),
            Goal::within(position(8, 64, 0), 2.0),
        ));
        assert_eq!(loose.outcome, PathOutcome::Complete);
    }

    #[test]
    fn the_node_budget_caps_the_work_and_still_returns_something_walkable() {
        let mut req = request(plain(64), position(0, 64, 0), exact(position(60, 64, 60)));
        req.budget = SearchBudget {
            max_nodes: 50,
            max_duration: Duration::from_secs(30),
        };
        let result = search(&req);
        assert_eq!(result.outcome, PathOutcome::Partial);
        assert!(result.expanded <= 50);
        assert!(!result.is_empty());
    }

    #[test]
    fn a_pre_cancelled_search_gives_up_immediately() {
        let req = request(plain(64), position(0, 64, 0), exact(position(60, 64, 60)));
        req.cancelled.store(true, AtomicOrdering::Relaxed);
        let result = search(&req);
        assert_eq!(result.outcome, PathOutcome::Cancelled);
        assert!(result.expanded <= BUDGET_CHECK_INTERVAL);
    }

    #[test]
    fn a_start_that_is_slightly_off_the_floor_snaps_onto_it() {
        let result = search(&request(
            plain(16),
            // One block above the real surface -- what a bot mid-jump or a
            // rounded position produces.
            position(0, 65, 0),
            exact(position(6, 64, 0)),
        ));
        assert_eq!(result.outcome, PathOutcome::Complete);
        assert_eq!(result.nodes[0], position(0, 64, 0));
    }

    #[test]
    fn the_reconstructed_path_is_contiguous() {
        let result = search(&request(
            plain(32),
            position(0, 64, 0),
            exact(position(12, 64, 7)),
        ));
        assert_eq!(result.outcome, PathOutcome::Complete);
        for window in result.nodes.windows(2) {
            let step = block_distance(window[0], window[1]);
            assert!(
                step <= 2.0,
                "consecutive path nodes must be adjacent, got {step} between {:?} and {:?}",
                window[0],
                window[1]
            );
        }
    }

    #[test]
    fn a_route_beside_a_hazard_reports_itself_as_hazardous() {
        let clear = search(&request(
            plain(32),
            position(0, 64, 0),
            exact(position(10, 64, 0)),
        ));
        assert!(!clear.hazardous);

        // A wall of fire with a single one-block gap: the only route is
        // straight between two burning cells.
        let mut grid = plain(32);
        for z in -32..32 {
            if z == 0 {
                continue;
            }
            for y in 64..=66 {
                grid.set(position(5, y, z), TerrainClass::Unbreakable);
            }
        }
        grid.set(position(5, 64, 1), TerrainClass::Hazard);
        grid.set(position(5, 64, -1), TerrainClass::Hazard);
        let result = search(&request(
            grid,
            position(0, 64, 0),
            exact(position(10, 64, 0)),
        ));
        assert_eq!(result.outcome, PathOutcome::Complete);
        assert!(result.hazardous, "squeezing between two fires is hazardous");
    }

    #[test]
    fn the_reported_cost_matches_the_sum_of_the_moves_taken() {
        let result = search(&request(
            plain(32),
            position(0, 64, 0),
            exact(position(9, 64, 0)),
        ));
        let profile = CostProfile::default();
        let expected: f64 = result.moves.iter().map(|kind| profile.cost_of(*kind)).sum();
        assert!((result.cost - expected).abs() < 1e-6);
    }
}
