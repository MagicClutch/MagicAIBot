//! High-level route planning: the coarse, chunk-resolution answer to "which
//! way is the destination", computed for the *whole* trip before a single
//! block-level search runs. Pure -- no world access; everything it knows
//! about terrain comes from the [`ChunkKnowledge`] the caller passes in.
//!
//! # Why a second, coarser search
//!
//! Block-level A* over 5000 blocks is not a search that finishes. But the
//! same 5000 blocks is only ~312 chunks across, and a 2D search over chunk
//! cells is trivial -- tens of thousands of nodes at worst, microseconds in
//! practice. So the route layer answers the cheap question (which chunks to
//! travel through, avoiding the ones already known to be impassable) and
//! leaves the expensive one (which blocks, exactly) to the segment that is
//! actually being walked right now.
//!
//! # Unknown chunks are optimistic *here* and forbidden *below*
//!
//! This is the deliberate asymmetry that makes long-distance travel work.
//! The coarse route happily plans through chunks nobody has ever seen --
//! priced at [`RouteProfile::unknown_chunk`] so known-good terrain still
//! wins when it exists, but never refused, because on a fresh server
//! *everything* past render distance is unknown and refusing it would mean
//! never leaving the spawn chunks. The block-level search underneath then
//! refuses unknown cells outright (see `crate::pathfinding::moves`), so the
//! bot only ever *walks* through verified terrain -- it just walks toward a
//! guess, which is what makes the server stream the next chunks in.

use std::collections::{BinaryHeap, HashMap};

use crate::{minecraft::world_state::BlockPosition, pathfinding::grid::horizontal_distance};

/// Chunk-space coordinate (one cell per 16x16 blocks).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct ChunkKey {
    pub x: i32,
    pub z: i32,
}

impl ChunkKey {
    #[must_use]
    pub fn of(position: BlockPosition) -> Self {
        Self {
            x: position.x.div_euclid(16),
            z: position.z.div_euclid(16),
        }
    }

    /// Block position at the center of this chunk, at `y`.
    #[must_use]
    pub fn center(self, y: i32) -> BlockPosition {
        BlockPosition {
            x: self.x * 16 + 8,
            y,
            z: self.z * 16 + 8,
        }
    }

    #[must_use]
    pub fn manhattan(self, other: Self) -> i32 {
        (self.x - other.x).abs() + (self.z - other.z).abs()
    }
}

/// What the route planner is allowed to know about a chunk. Supplied by
/// [`crate::pathfinding::world_cache::NavigationCache`]; kept as a trait so
/// this module stays pure and trivially testable with a hand-written map.
pub trait ChunkKnowledge {
    /// Whether anything has been sampled about this chunk at all.
    fn is_known(&self, key: ChunkKey) -> bool;
    /// Whether the chunk is known to be unroutable -- sampled, and found to
    /// contain no standable surface at all (solid rock, open void, a lava
    /// lake). Unknown chunks must answer `false`: not knowing is not the
    /// same as knowing it's bad.
    fn is_blocked(&self, key: ChunkKey) -> bool;
    /// Best guess at the surface Y for the chunk, when known -- used to give
    /// coarse waypoints a sensible height instead of the start's.
    fn surface_y(&self, key: ChunkKey) -> Option<i32>;
}

/// A [`ChunkKnowledge`] that knows nothing.
///
/// Test-only: in production the planner always consults the real
/// `NavigationCache`, which behaves exactly like this while it is still
/// empty -- so this exists to test that path without standing a cache up.
#[cfg(test)]
pub struct NoKnowledge;
#[cfg(test)]
impl ChunkKnowledge for NoKnowledge {
    fn is_known(&self, _key: ChunkKey) -> bool {
        false
    }
    fn is_blocked(&self, _key: ChunkKey) -> bool {
        false
    }
    fn surface_y(&self, _key: ChunkKey) -> Option<i32> {
        None
    }
}

/// Weights for the coarse search, in "chunks of easy travel" units.
#[derive(Clone, Copy, Debug)]
pub struct RouteProfile {
    /// Cost of crossing a chunk that is known and routable.
    pub known_chunk: f64,
    /// Cost of crossing a chunk nobody has sampled. Above `known_chunk` so a
    /// known-good corridor is preferred where one exists, but finite, so
    /// unexplored terrain is still traversable -- see this module's doc
    /// comment.
    pub unknown_chunk: f64,
    /// Cost of a diagonal chunk step.
    pub diagonal_multiplier: f64,
    /// Bound on the coarse search, in chunk nodes. A 5000-block trip is
    /// ~312 chunks; even a heavily obstructed corridor search stays far
    /// under this.
    pub max_nodes: usize,
}

impl Default for RouteProfile {
    fn default() -> Self {
        Self {
            known_chunk: 1.0,
            unknown_chunk: 1.35,
            diagonal_multiplier: std::f64::consts::SQRT_2,
            max_nodes: 40_000,
        }
    }
}

/// The coarse plan: a chunk corridor plus the block-space waypoints derived
/// from it.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    /// Chunk cells from the start chunk to the destination chunk.
    pub chunks: Vec<ChunkKey>,
    /// Block positions along that corridor, spaced roughly
    /// `segment_length` apart, ending exactly at the destination. These
    /// become segment boundaries.
    pub waypoints: Vec<BlockPosition>,
    /// Whether the coarse search actually reached the destination chunk. A
    /// `false` here means the corridor is blocked by *known* terrain and the
    /// waypoints only lead as far as the search got.
    pub complete: bool,
}

#[derive(Clone, Copy)]
struct CoarseCandidate {
    key: ChunkKey,
    f_score: f64,
}
impl PartialEq for CoarseCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.f_score == other.f_score
    }
}
impl Eq for CoarseCandidate {}
impl Ord for CoarseCandidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.f_score.total_cmp(&self.f_score)
    }
}
impl PartialOrd for CoarseCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

const NEIGHBORS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// Plans the coarse route from `start` to `destination`.
///
/// Falls back to a straight line whenever the chunk search can't do better
/// -- which is the common case for a long trip into unexplored terrain, and
/// is the *correct* answer there: with no knowledge, the shortest route is
/// the straight one, and the segment layer will discover the real terrain as
/// it goes.
#[must_use]
pub fn plan(
    start: BlockPosition,
    destination: BlockPosition,
    knowledge: &impl ChunkKnowledge,
    profile: &RouteProfile,
    segment_length: f64,
) -> Route {
    let start_key = ChunkKey::of(start);
    let goal_key = ChunkKey::of(destination);
    let chunks = if start_key == goal_key {
        vec![start_key]
    } else {
        coarse_search(start_key, goal_key, knowledge, profile)
            .unwrap_or_else(|| straight_chunk_line(start_key, goal_key))
    };
    let complete = chunks.last().copied() == Some(goal_key);
    let waypoints = waypoints_along(
        start,
        destination,
        &chunks,
        knowledge,
        segment_length,
        complete,
    );
    Route {
        chunks,
        waypoints,
        complete,
    }
}

/// A* over chunk cells. Returns `None` when the goal chunk can't be reached
/// within the node budget, which the caller turns into a straight line --
/// better to head in the right direction and replan than to refuse to move.
fn coarse_search(
    start: ChunkKey,
    goal: ChunkKey,
    knowledge: &impl ChunkKnowledge,
    profile: &RouteProfile,
) -> Option<Vec<ChunkKey>> {
    let mut parents: HashMap<ChunkKey, ChunkKey> = HashMap::new();
    let mut g_scores: HashMap<ChunkKey, f64> = HashMap::new();
    let mut open = BinaryHeap::new();
    g_scores.insert(start, 0.0);
    open.push(CoarseCandidate {
        key: start,
        f_score: 0.0,
    });
    let mut expanded = 0usize;

    while let Some(current) = open.pop() {
        if current.key == goal {
            let mut chunks = vec![goal];
            let mut cursor = goal;
            while let Some(parent) = parents.get(&cursor) {
                chunks.push(*parent);
                cursor = *parent;
            }
            chunks.reverse();
            return Some(chunks);
        }
        expanded += 1;
        if expanded >= profile.max_nodes {
            return None;
        }
        let current_g = g_scores.get(&current.key).copied().unwrap_or(f64::MAX);
        for (dx, dz) in NEIGHBORS {
            let neighbor = ChunkKey {
                x: current.key.x + dx,
                z: current.key.z + dz,
            };
            // A chunk *known* to have nowhere to stand is refused outright;
            // the goal chunk itself is exempt, since refusing it would fail
            // the whole plan instead of getting as close as possible.
            if neighbor != goal && knowledge.is_blocked(neighbor) {
                continue;
            }
            let step = if knowledge.is_known(neighbor) {
                profile.known_chunk
            } else {
                profile.unknown_chunk
            } * if dx != 0 && dz != 0 {
                profile.diagonal_multiplier
            } else {
                1.0
            };
            let tentative = current_g + step;
            if g_scores
                .get(&neighbor)
                .is_some_and(|existing| *existing <= tentative)
            {
                continue;
            }
            g_scores.insert(neighbor, tentative);
            parents.insert(neighbor, current.key);
            let heuristic = f64::from(neighbor.manhattan(goal)) * profile.known_chunk;
            open.push(CoarseCandidate {
                key: neighbor,
                f_score: tentative + heuristic,
            });
        }
    }
    None
}

/// Bresenham-ish chunk line, used when the coarse search gives up.
fn straight_chunk_line(start: ChunkKey, goal: ChunkKey) -> Vec<ChunkKey> {
    let steps = (goal.x - start.x)
        .abs()
        .max((goal.z - start.z).abs())
        .max(1);
    (0..=steps)
        .map(|step| {
            let fraction = f64::from(step) / f64::from(steps);
            ChunkKey {
                x: start.x + ((f64::from(goal.x - start.x) * fraction).round() as i32),
                z: start.z + ((f64::from(goal.z - start.z) * fraction).round() as i32),
            }
        })
        .collect()
}

/// Turns the chunk corridor into block-space waypoints roughly
/// `segment_length` blocks apart.
///
/// Walks the corridor accumulating distance and drops a waypoint whenever
/// the accumulated distance passes the segment length, so segments stay a
/// consistent size regardless of how twisty the corridor is. Y comes from
/// the cache's surface estimate where one exists and is interpolated
/// otherwise -- a coarse waypoint's Y is a hint for the block-level search
/// (which snaps it to real ground), never a commitment.
fn waypoints_along(
    start: BlockPosition,
    destination: BlockPosition,
    chunks: &[ChunkKey],
    knowledge: &impl ChunkKnowledge,
    segment_length: f64,
    complete: bool,
) -> Vec<BlockPosition> {
    let mut waypoints = Vec::new();
    let segment_length = segment_length.max(8.0);
    let mut accumulated = 0.0;
    let mut previous = start;
    for (index, key) in chunks.iter().enumerate() {
        let fraction = if chunks.len() <= 1 {
            1.0
        } else {
            index as f64 / (chunks.len() - 1) as f64
        };
        let interpolated_y = start.y + ((f64::from(destination.y - start.y) * fraction) as i32);
        let center = key.center(knowledge.surface_y(*key).unwrap_or(interpolated_y));
        accumulated += horizontal_distance(previous, center);
        previous = center;
        if accumulated >= segment_length {
            accumulated = 0.0;
            waypoints.push(center);
        }
    }
    // The destination is always the final waypoint when the corridor
    // actually reaches it; when it doesn't, the last corridor cell is as far
    // as this plan claims to go and inventing a waypoint at the unreachable
    // destination would only produce a segment that can never complete.
    if complete {
        if waypoints
            .last()
            .is_some_and(|last| horizontal_distance(*last, destination) < segment_length * 0.5)
        {
            waypoints.pop();
        }
        waypoints.push(destination);
    } else if waypoints.is_empty()
        && let Some(last) = chunks.last()
    {
        waypoints.push(last.center(destination.y));
    }
    waypoints
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn position(x: i32, y: i32, z: i32) -> BlockPosition {
        BlockPosition { x, y, z }
    }

    /// Knowledge with an explicit set of known and blocked chunks.
    struct TestKnowledge {
        known: HashSet<ChunkKey>,
        blocked: HashSet<ChunkKey>,
    }
    impl ChunkKnowledge for TestKnowledge {
        fn is_known(&self, key: ChunkKey) -> bool {
            self.known.contains(&key)
        }
        fn is_blocked(&self, key: ChunkKey) -> bool {
            self.blocked.contains(&key)
        }
        fn surface_y(&self, _key: ChunkKey) -> Option<i32> {
            None
        }
    }

    #[test]
    fn chunk_keys_handle_negative_coordinates() {
        assert_eq!(ChunkKey::of(position(0, 0, 0)), ChunkKey { x: 0, z: 0 });
        assert_eq!(ChunkKey::of(position(15, 0, 15)), ChunkKey { x: 0, z: 0 });
        assert_eq!(ChunkKey::of(position(-1, 0, -1)), ChunkKey { x: -1, z: -1 });
        assert_eq!(ChunkKey::of(position(-17, 0, 16)), ChunkKey { x: -2, z: 1 });
    }

    #[test]
    fn a_route_with_no_knowledge_is_a_straight_corridor() {
        let route = plan(
            position(0, 64, 0),
            position(1000, 64, 0),
            &NoKnowledge,
            &RouteProfile::default(),
            48.0,
        );
        assert!(route.complete);
        assert_eq!(route.chunks.first().copied(), Some(ChunkKey { x: 0, z: 0 }));
        assert_eq!(route.chunks.last().copied(), Some(ChunkKey { x: 62, z: 0 }));
        assert!(
            route.chunks.iter().all(|key| key.z == 0),
            "a straight east-west trip should not wander in z"
        );
    }

    #[test]
    fn a_thousand_block_route_is_sliced_into_many_segments() {
        let route = plan(
            position(0, 64, 0),
            position(1000, 64, 0),
            &NoKnowledge,
            &RouteProfile::default(),
            48.0,
        );
        assert!(
            route.waypoints.len() >= 15,
            "1000 blocks at 48 per segment should be ~20 waypoints, got {}",
            route.waypoints.len()
        );
        assert_eq!(route.waypoints.last().copied(), Some(position(1000, 64, 0)));
    }

    #[test]
    fn waypoints_are_spaced_close_to_the_requested_segment_length() {
        let route = plan(
            position(0, 64, 0),
            position(2000, 64, 0),
            &NoKnowledge,
            &RouteProfile::default(),
            64.0,
        );
        let mut previous = position(0, 64, 0);
        for waypoint in &route.waypoints[..route.waypoints.len() - 1] {
            let gap = horizontal_distance(previous, *waypoint);
            assert!(
                (32.0..=128.0).contains(&gap),
                "segment length {gap} strayed far from the requested 64"
            );
            previous = *waypoint;
        }
    }

    #[test]
    fn a_destination_inside_the_current_chunk_is_a_single_waypoint() {
        let route = plan(
            position(2, 64, 2),
            position(10, 64, 9),
            &NoKnowledge,
            &RouteProfile::default(),
            48.0,
        );
        assert_eq!(route.chunks, vec![ChunkKey { x: 0, z: 0 }]);
        assert_eq!(route.waypoints, vec![position(10, 64, 9)]);
        assert!(route.complete);
    }

    #[test]
    fn the_route_detours_around_chunks_known_to_be_blocked() {
        // A wall of blocked chunks at x=3, with a gap at z=2.
        let blocked = (-4..=4)
            .filter(|z| *z != 2)
            .map(|z| ChunkKey { x: 3, z })
            .collect::<HashSet<_>>();
        let knowledge = TestKnowledge {
            known: blocked.iter().copied().collect(),
            blocked,
        };
        let route = plan(
            position(0, 64, 0),
            position(100, 64, 0),
            &knowledge,
            &RouteProfile::default(),
            48.0,
        );
        assert!(route.complete);
        assert!(
            route.chunks.iter().any(|key| key.x == 3 && key.z == 2),
            "the corridor must use the one gap in the wall: {:?}",
            route.chunks
        );
    }

    #[test]
    fn known_good_chunks_are_preferred_over_unexplored_ones() {
        // A known corridor along z=1 costs less per chunk than the unknown
        // straight line along z=0, so the route should bend into it.
        let known = (0..=6)
            .map(|x| ChunkKey { x, z: 1 })
            .collect::<HashSet<_>>();
        let knowledge = TestKnowledge {
            known,
            blocked: HashSet::new(),
        };
        let route = plan(
            position(0, 64, 0),
            position(100, 64, 24),
            &knowledge,
            &RouteProfile::default(),
            48.0,
        );
        assert!(
            route.chunks.iter().filter(|key| key.z == 1).count() >= 3,
            "should follow the known corridor: {:?}",
            route.chunks
        );
    }

    #[test]
    fn a_fully_walled_destination_still_produces_a_route_toward_it() {
        // Every neighbor of the goal chunk is blocked, so the coarse search
        // cannot reach it -- but the plan must still lead the bot in the
        // right direction rather than refusing to move.
        let mut blocked = HashSet::new();
        for x in 5..=7 {
            for z in -1..=1 {
                blocked.insert(ChunkKey { x, z });
            }
        }
        let goal_chunk = ChunkKey { x: 6, z: 0 };
        blocked.remove(&goal_chunk);
        let knowledge = TestKnowledge {
            known: blocked.iter().copied().collect(),
            blocked,
        };
        let route = plan(
            position(0, 64, 0),
            position(100, 64, 0),
            &knowledge,
            &RouteProfile::default(),
            48.0,
        );
        assert!(!route.waypoints.is_empty());
    }

    #[test]
    fn the_coarse_route_scales_to_thousands_of_blocks_quickly() {
        let started = std::time::Instant::now();
        let route = plan(
            position(0, 100, 0),
            position(5000, 100, -3000),
            &NoKnowledge,
            &RouteProfile::default(),
            48.0,
        );
        assert!(route.complete);
        assert_eq!(
            route.waypoints.last().copied(),
            Some(position(5000, 100, -3000))
        );
        assert!(
            started.elapsed() < std::time::Duration::from_millis(250),
            "coarse planning must stay cheap even for a 5000-block trip"
        );
    }

    #[test]
    fn a_descending_destination_interpolates_waypoint_height() {
        let route = plan(
            position(0, 120, 0),
            position(400, 20, 0),
            &NoKnowledge,
            &RouteProfile::default(),
            48.0,
        );
        let first = route.waypoints.first().copied().unwrap();
        let last = route.waypoints.last().copied().unwrap();
        assert!(first.y < 120 && first.y > 20, "got {}", first.y);
        assert_eq!(last.y, 20);
    }
}
