//! Path slices and the manager that owns them. Pure -- no world access, no
//! async.
//!
//! A [`PathSegment`] is one bite-sized piece of a long trip. Its whole point
//! is that it exists in two very different states:
//!
//! - **Planned** (`waypoints` empty): all it holds is a start, an end, and a
//!   straight-line cost estimate. Every segment past the current one is like
//!   this, which is what keeps a 5000-block trip cheap -- 100 planned
//!   segments cost about as much memory as one sentence.
//! - **Calculated**: the block-level search has run, so it also holds the
//!   real waypoints, the real cost, and the [`SegmentAction`]s the route
//!   actually requires (jump here, bridge there, hazard on the way).
//!
//! [`SegmentPlan`] is the manager: it owns the ordered segments, tracks
//! which one is being walked, and -- the part that matters for dynamic
//! replanning -- knows how to throw away the affected ones without
//! disturbing the rest.

use crate::{
    minecraft::world_state::BlockPosition,
    pathfinding::{
        astar::{PathOutcome, PathResult},
        cost::MoveKind,
        grid::{block_distance, horizontal_distance},
    },
};

/// A thing the bot will have to *do* on a segment, as opposed to somewhere
/// it will have to be. Derived from the move kinds the block-level search
/// chose, so this is a report of the real route rather than a guess.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum SegmentAction {
    Walk,
    Jump,
    Climb,
    Descend,
    /// A drop far enough to hurt -- surfaced separately from `Descend`
    /// because it is the one action worth warning about.
    Fall,
    Swim,
    Bridge,
    Break,
    AvoidDanger,
}

impl SegmentAction {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Walk => "walk",
            Self::Jump => "jump",
            Self::Climb => "climb",
            Self::Descend => "descend",
            Self::Fall => "fall",
            Self::Swim => "swim",
            Self::Bridge => "bridge",
            Self::Break => "break",
            Self::AvoidDanger => "avoid danger",
        }
    }

    /// The action a single move implies. `max_safe_drop` decides where a
    /// descent stops being routine and becomes a `Fall`.
    #[must_use]
    pub fn of_move(kind: MoveKind, max_safe_drop: i32) -> Self {
        match kind {
            MoveKind::Walk | MoveKind::Diagonal => Self::Walk,
            MoveKind::JumpUp => Self::Jump,
            MoveKind::Drop { blocks } if blocks > max_safe_drop => Self::Fall,
            MoveKind::Drop { .. } => Self::Descend,
            MoveKind::Swim => Self::Swim,
            MoveKind::Break { .. } => Self::Break,
            MoveKind::Bridge { .. } => Self::Bridge,
            MoveKind::Climb { .. } => Self::Climb,
        }
    }
}

/// Where a segment is in its own little lifecycle.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SegmentState {
    /// Endpoints only -- no block-level search has run for it yet.
    #[default]
    Planned,
    /// Waypoints computed and ready to walk.
    Calculated,
    /// Currently being walked.
    Active,
    Completed,
    /// The world changed under it; it must be recalculated before use.
    Invalidated,
}

/// One slice of the overall path.
#[derive(Clone, Debug)]
pub struct PathSegment {
    /// Position in the plan, stable across invalidation so debug output can
    /// say "segment 5/24" and mean it.
    pub index: usize,
    pub start: BlockPosition,
    pub end: BlockPosition,
    /// Block-level route through this segment. Empty while `Planned`.
    pub waypoints: Vec<BlockPosition>,
    /// Cost from the block-level search once calculated; a straight-line
    /// estimate before that.
    pub estimated_cost: f64,
    /// What this segment requires, in the order first encountered.
    pub actions: Vec<SegmentAction>,
    /// Whether the calculated route stops short of where this segment was
    /// planned to end -- the search ran out of budget or of loaded world.
    ///
    /// It matters because `end` is *overwritten* with wherever the path
    /// really reaches, so once a partial path is applied there is no longer
    /// anything to compare against to notice. Every later segment's start
    /// was derived from the planned end, so walking a partial segment
    /// invalidates the rest of the plan -- see the controller's
    /// `complete_segment`.
    pub partial: bool,
    pub state: SegmentState,
}

impl PathSegment {
    /// A not-yet-calculated segment between two coarse waypoints.
    #[must_use]
    pub fn planned(
        index: usize,
        start: BlockPosition,
        end: BlockPosition,
        cost_per_block: f64,
    ) -> Self {
        Self {
            index,
            start,
            end,
            waypoints: Vec::new(),
            estimated_cost: block_distance(start, end) * cost_per_block,
            actions: Vec::new(),
            partial: false,
            state: SegmentState::Planned,
        }
    }

    /// Fills in a segment from a finished block-level search: keeps the real
    /// waypoints, cost, and actions, and moves `end` to where the path
    /// actually reaches (a partial path ends short of the planned end, and
    /// pretending otherwise would make the executor wait forever for an
    /// arrival that can't happen).
    pub fn apply_path(&mut self, path: &PathResult, max_safe_drop: i32, simplify_tolerance: f64) {
        self.waypoints = simplify(&path.nodes, simplify_tolerance);
        self.estimated_cost = path.cost;
        self.actions = actions_of(&path.moves, max_safe_drop);
        if path.hazardous && !self.actions.contains(&SegmentAction::AvoidDanger) {
            self.actions.push(SegmentAction::AvoidDanger);
        }
        self.end = path.destination();
        self.partial = path.outcome != PathOutcome::Complete;
        self.state = SegmentState::Calculated;
    }

    #[must_use]
    pub fn is_calculated(&self) -> bool {
        matches!(self.state, SegmentState::Calculated | SegmentState::Active)
    }

    /// Straight-line length of the segment, in blocks.
    #[must_use]
    pub fn length(&self) -> f64 {
        block_distance(self.start, self.end)
    }

    /// Whether any part of this segment passes within `radius` blocks
    /// (horizontally) of `position` -- how a chunk change decides which
    /// segments it actually affects.
    #[must_use]
    pub fn passes_near(&self, position: BlockPosition, radius: f64) -> bool {
        if horizontal_distance(self.start, position) <= radius
            || horizontal_distance(self.end, position) <= radius
        {
            return true;
        }
        self.waypoints
            .iter()
            .any(|waypoint| horizontal_distance(*waypoint, position) <= radius)
    }

    /// Marks a segment as needing recalculation, dropping the stale
    /// waypoints so nothing can accidentally walk them.
    pub fn invalidate(&mut self) {
        self.waypoints.clear();
        self.actions.clear();
        self.partial = false;
        self.state = SegmentState::Invalidated;
    }
}

/// Collapses a block-by-block path into the corners that actually matter:
/// consecutive nodes moving in the same direction become one waypoint.
///
/// A 48-block segment is ~48 nodes; handing 48 goals to the movement layer
/// one at a time would make the bot stutter at every block boundary. Keeping
/// only direction changes leaves the handful of points where the route
/// genuinely turns, and the executor walks smoothly between them.
/// `tolerance` is how far (in blocks) a node may sit off the straight line
/// between its neighbors before it counts as a real corner.
#[must_use]
pub fn simplify(nodes: &[BlockPosition], tolerance: f64) -> Vec<BlockPosition> {
    if nodes.len() <= 2 {
        return nodes.to_vec();
    }
    let mut simplified = vec![nodes[0]];
    let mut anchor = nodes[0];
    for window in nodes.windows(2).skip(1) {
        let (candidate, next) = (window[0], window[1]);
        // Keep `candidate` when dropping it would make the straight line
        // from the last kept node to `next` stray from the real route.
        if perpendicular_distance(candidate, anchor, next) > tolerance {
            simplified.push(candidate);
            anchor = candidate;
        }
    }
    let last = *nodes.last().expect("nodes is non-empty");
    if simplified.last().copied() != Some(last) {
        simplified.push(last);
    }
    simplified
}

/// Distance from `point` to the segment `start`-`end`, in 3D.
fn perpendicular_distance(point: BlockPosition, start: BlockPosition, end: BlockPosition) -> f64 {
    let (px, py, pz) = (
        f64::from(point.x - start.x),
        f64::from(point.y - start.y),
        f64::from(point.z - start.z),
    );
    let (ex, ey, ez) = (
        f64::from(end.x - start.x),
        f64::from(end.y - start.y),
        f64::from(end.z - start.z),
    );
    let length_squared = ex * ex + ey * ey + ez * ez;
    if length_squared < 1e-9 {
        return (px * px + py * py + pz * pz).sqrt();
    }
    let projection = ((px * ex + py * ey + pz * ez) / length_squared).clamp(0.0, 1.0);
    let (dx, dy, dz) = (
        px - ex * projection,
        py - ey * projection,
        pz - ez * projection,
    );
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// The distinct actions a move sequence implies, first-seen order preserved.
#[must_use]
pub fn actions_of(moves: &[MoveKind], max_safe_drop: i32) -> Vec<SegmentAction> {
    let mut actions: Vec<SegmentAction> = Vec::new();
    for kind in moves {
        let action = SegmentAction::of_move(*kind, max_safe_drop);
        if !actions.contains(&action) {
            actions.push(action);
        }
    }
    actions
}

/// The ordered segments of one trip, plus which one is current.
#[derive(Clone, Debug)]
pub struct SegmentPlan {
    segments: Vec<PathSegment>,
    current: usize,
}

impl SegmentPlan {
    /// Builds a plan from coarse route waypoints. `start` is the bot's
    /// current position; each waypoint becomes one segment boundary.
    #[must_use]
    pub fn from_waypoints(
        start: BlockPosition,
        waypoints: &[BlockPosition],
        cost_per_block: f64,
    ) -> Self {
        let mut segments = Vec::with_capacity(waypoints.len());
        let mut previous = start;
        for (index, waypoint) in waypoints.iter().enumerate() {
            segments.push(PathSegment::planned(
                index,
                previous,
                *waypoint,
                cost_per_block,
            ));
            previous = *waypoint;
        }
        Self {
            segments,
            current: 0,
        }
    }

    #[must_use]
    pub fn segments(&self) -> &[PathSegment] {
        &self.segments
    }

    /// Mutable access by index -- how a finished search is applied to the
    /// segment it was started for, which is not necessarily the current one
    /// (see the controller's prefetch of the *next* segment).
    pub fn segment_mut(&mut self, index: usize) -> Option<&mut PathSegment> {
        self.segments.get_mut(index)
    }

    #[must_use]
    pub fn total_segments(&self) -> usize {
        self.segments.len()
    }

    /// One-based index of the current segment, for display ("5/24").
    #[must_use]
    pub fn current_number(&self) -> usize {
        (self.current + 1).min(self.segments.len())
    }

    #[must_use]
    pub fn current(&self) -> Option<&PathSegment> {
        self.segments.get(self.current)
    }

    #[must_use]
    pub fn current_mut(&mut self) -> Option<&mut PathSegment> {
        self.segments.get_mut(self.current)
    }

    /// Whether every segment has been walked.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.current >= self.segments.len()
    }

    /// Marks the current segment done and moves to the next one.
    pub fn advance(&mut self) {
        if let Some(segment) = self.segments.get_mut(self.current) {
            segment.state = SegmentState::Completed;
        }
        self.current += 1;
    }

    /// Sum of the estimated costs of everything not yet walked.
    #[must_use]
    pub fn remaining_cost(&self) -> f64 {
        self.segments
            .iter()
            .skip(self.current)
            .map(|segment| segment.estimated_cost)
            .sum()
    }

    /// Straight-line blocks left along the planned segment chain.
    #[must_use]
    pub fn remaining_distance(&self) -> f64 {
        self.segments
            .iter()
            .skip(self.current)
            .map(PathSegment::length)
            .sum()
    }

    /// Invalidates the current segment and everything after it, keeping
    /// completed work intact. Used when the bot is displaced (a fall, a
    /// teleport) so what is ahead is recomputed but the plan's shape
    /// survives.
    pub fn invalidate_from_current(&mut self) {
        for segment in self.segments.iter_mut().skip(self.current) {
            segment.invalidate();
        }
    }

    /// Invalidates only the segments that pass near `position` -- the
    /// surgical case the spec asks for: a chunk changed, so discard the
    /// segments that actually cross it and keep every other one.
    ///
    /// Returns how many were invalidated, so the caller can skip the whole
    /// replan when a change turns out to affect nothing.
    pub fn invalidate_near(&mut self, position: BlockPosition, radius: f64) -> usize {
        let current = self.current;
        self.segments
            .iter_mut()
            .enumerate()
            .filter(|(index, segment)| {
                *index >= current
                    && !matches!(segment.state, SegmentState::Completed)
                    && segment.passes_near(position, radius)
            })
            .map(|(_, segment)| segment.invalidate())
            .count()
    }

    /// Rewrites the tail of the plan from `from_index` onward with fresh
    /// waypoints, keeping everything before it untouched. This is what a
    /// "better route appeared" replan does -- the completed prefix is still
    /// completed.
    pub fn replace_tail(
        &mut self,
        from_index: usize,
        start: BlockPosition,
        waypoints: &[BlockPosition],
        cost_per_block: f64,
    ) {
        let from_index = from_index.min(self.segments.len());
        self.segments.truncate(from_index);
        let mut previous = start;
        for (offset, waypoint) in waypoints.iter().enumerate() {
            self.segments.push(PathSegment::planned(
                from_index + offset,
                previous,
                *waypoint,
                cost_per_block,
            ));
            previous = *waypoint;
        }
        self.current = self.current.min(self.segments.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathfinding::astar::PathOutcome;

    fn position(x: i32, y: i32, z: i32) -> BlockPosition {
        BlockPosition { x, y, z }
    }

    fn plan_of(count: i32) -> SegmentPlan {
        let waypoints: Vec<_> = (1..=count).map(|i| position(i * 48, 64, 0)).collect();
        SegmentPlan::from_waypoints(position(0, 64, 0), &waypoints, 1.0)
    }

    #[test]
    fn a_plan_starts_on_its_first_segment() {
        let plan = plan_of(5);
        assert_eq!(plan.total_segments(), 5);
        assert_eq!(plan.current_number(), 1);
        assert_eq!(plan.current().unwrap().start, position(0, 64, 0));
        assert_eq!(plan.current().unwrap().end, position(48, 64, 0));
        assert!(!plan.finished());
    }

    #[test]
    fn future_segments_stay_planned_rather_than_calculated() {
        let plan = plan_of(24);
        assert!(
            plan.segments()
                .iter()
                .all(|s| s.state == SegmentState::Planned)
        );
        assert!(plan.segments().iter().all(|s| s.waypoints.is_empty()));
        assert!(
            plan.segments()[10].estimated_cost > 0.0,
            "still has a cost estimate"
        );
    }

    #[test]
    fn advancing_completes_segments_in_order_and_finishes() {
        let mut plan = plan_of(3);
        plan.advance();
        assert_eq!(plan.current_number(), 2);
        assert_eq!(plan.segments()[0].state, SegmentState::Completed);
        plan.advance();
        plan.advance();
        assert!(plan.finished());
        assert!(plan.current().is_none());
    }

    #[test]
    fn remaining_cost_and_distance_shrink_as_segments_complete() {
        let mut plan = plan_of(4);
        let before = plan.remaining_cost();
        let distance_before = plan.remaining_distance();
        plan.advance();
        assert!(plan.remaining_cost() < before);
        assert!(plan.remaining_distance() < distance_before);
    }

    #[test]
    fn applying_a_search_result_fills_in_waypoints_actions_and_cost() {
        let mut segment = PathSegment::planned(0, position(0, 64, 0), position(4, 64, 0), 1.0);
        let path = PathResult {
            nodes: vec![
                position(0, 64, 0),
                position(1, 64, 0),
                position(2, 65, 0),
                position(3, 65, 0),
            ],
            moves: vec![MoveKind::Walk, MoveKind::JumpUp, MoveKind::Walk],
            outcome: PathOutcome::Complete,
            cost: 12.5,
            expanded: 40,
            elapsed: std::time::Duration::from_millis(3),
            hazardous: false,
        };
        segment.apply_path(&path, 3, 0.4);
        assert_eq!(segment.state, SegmentState::Calculated);
        assert_eq!(segment.estimated_cost, 12.5);
        assert!(segment.actions.contains(&SegmentAction::Jump));
        assert!(segment.actions.contains(&SegmentAction::Walk));
        assert_eq!(
            segment.end,
            position(3, 65, 0),
            "a partial path moves the segment end to where it really reaches"
        );
    }

    #[test]
    fn a_partial_path_marks_the_segment_as_ending_short() {
        let mut segment = PathSegment::planned(0, position(0, 64, 0), position(48, 64, 0), 1.0);
        let path = PathResult {
            nodes: vec![position(0, 64, 0), position(1, 64, 0), position(20, 64, 0)],
            moves: vec![MoveKind::Walk, MoveKind::Walk],
            outcome: PathOutcome::Partial,
            cost: 20.0,
            expanded: 500,
            elapsed: std::time::Duration::from_millis(20),
            hazardous: false,
        };
        segment.apply_path(&path, 3, 0.4);
        assert!(segment.partial);
        assert_eq!(
            segment.end,
            position(20, 64, 0),
            "the segment now ends where the path really reaches"
        );

        let mut complete = PathSegment::planned(0, position(0, 64, 0), position(48, 64, 0), 1.0);
        complete.apply_path(
            &PathResult {
                outcome: PathOutcome::Complete,
                ..path
            },
            3,
            0.4,
        );
        assert!(!complete.partial);
    }

    #[test]
    fn a_hazardous_route_reports_an_avoid_danger_action() {
        let mut segment = PathSegment::planned(0, position(0, 64, 0), position(2, 64, 0), 1.0);
        let path = PathResult {
            nodes: vec![position(0, 64, 0), position(1, 64, 0), position(2, 64, 0)],
            moves: vec![MoveKind::Walk, MoveKind::Walk],
            outcome: PathOutcome::Complete,
            cost: 2.0,
            expanded: 8,
            elapsed: std::time::Duration::from_millis(1),
            hazardous: true,
        };
        segment.apply_path(&path, 3, 0.4);
        assert!(segment.actions.contains(&SegmentAction::AvoidDanger));
    }

    #[test]
    fn invalidating_clears_waypoints_so_nothing_walks_a_stale_route() {
        let mut segment = PathSegment::planned(0, position(0, 64, 0), position(4, 64, 0), 1.0);
        segment.waypoints = vec![position(1, 64, 0), position(4, 64, 0)];
        segment.state = SegmentState::Calculated;
        segment.invalidate();
        assert_eq!(segment.state, SegmentState::Invalidated);
        assert!(segment.waypoints.is_empty());
        assert!(!segment.is_calculated());
    }

    #[test]
    fn a_chunk_change_invalidates_only_the_segments_that_pass_through_it() {
        let mut plan = plan_of(6);
        // Segment 3 spans x=96..144; a change at x=120 must hit it and its
        // neighbors only if they are genuinely within the radius.
        let invalidated = plan.invalidate_near(position(120, 64, 0), 24.0);
        assert!(invalidated >= 1);
        let touched: Vec<_> = plan
            .segments()
            .iter()
            .filter(|s| s.state == SegmentState::Invalidated)
            .map(|s| s.index)
            .collect();
        assert!(
            touched.contains(&2),
            "the segment containing x=120: {touched:?}"
        );
        assert!(
            !touched.contains(&5),
            "a distant segment must be left alone: {touched:?}"
        );
    }

    #[test]
    fn an_irrelevant_chunk_change_invalidates_nothing() {
        let mut plan = plan_of(6);
        let invalidated = plan.invalidate_near(position(0, 64, 5000), 24.0);
        assert_eq!(invalidated, 0);
        assert!(
            plan.segments()
                .iter()
                .all(|s| s.state == SegmentState::Planned)
        );
    }

    #[test]
    fn completed_segments_are_never_invalidated_by_a_nearby_change() {
        let mut plan = plan_of(6);
        plan.advance();
        plan.advance();
        let invalidated = plan.invalidate_near(position(0, 64, 0), 32.0);
        assert_eq!(
            invalidated, 0,
            "a change behind the bot must not disturb finished work"
        );
        assert_eq!(plan.segments()[0].state, SegmentState::Completed);
    }

    #[test]
    fn replacing_the_tail_keeps_the_walked_prefix() {
        let mut plan = plan_of(6);
        plan.advance();
        plan.advance();
        let new_waypoints = vec![position(200, 70, 50), position(260, 70, 90)];
        plan.replace_tail(2, position(96, 64, 0), &new_waypoints, 1.0);
        assert_eq!(plan.total_segments(), 4);
        assert_eq!(plan.segments()[0].state, SegmentState::Completed);
        assert_eq!(plan.segments()[1].state, SegmentState::Completed);
        assert_eq!(plan.current_number(), 3);
        assert_eq!(
            plan.segments().last().map(|segment| segment.end),
            Some(position(260, 70, 90))
        );
    }

    #[test]
    fn invalidate_from_current_leaves_the_past_alone() {
        let mut plan = plan_of(5);
        plan.advance();
        plan.invalidate_from_current();
        assert_eq!(plan.segments()[0].state, SegmentState::Completed);
        assert!(
            plan.segments()[1..]
                .iter()
                .all(|s| s.state == SegmentState::Invalidated)
        );
    }

    #[test]
    fn simplify_collapses_a_straight_run_to_its_endpoints() {
        let straight: Vec<_> = (0..20).map(|x| position(x, 64, 0)).collect();
        let simplified = simplify(&straight, 0.4);
        assert_eq!(simplified, vec![position(0, 64, 0), position(19, 64, 0)]);
    }

    #[test]
    fn simplify_keeps_real_corners() {
        let mut nodes: Vec<_> = (0..10).map(|x| position(x, 64, 0)).collect();
        nodes.extend((1..10).map(|z| position(9, 64, z)));
        let simplified = simplify(&nodes, 0.4);
        assert!(
            simplified.contains(&position(9, 64, 0)),
            "the corner survives"
        );
        assert!(simplified.len() < nodes.len() / 2);
        assert_eq!(simplified.first(), nodes.first());
        assert_eq!(simplified.last(), nodes.last());
    }

    #[test]
    fn simplify_preserves_vertical_changes() {
        let nodes = vec![
            position(0, 64, 0),
            position(1, 64, 0),
            position(2, 65, 0),
            position(3, 65, 0),
        ];
        let simplified = simplify(&nodes, 0.4);
        assert!(
            simplified.contains(&position(2, 65, 0)) || simplified.contains(&position(1, 64, 0))
        );
        assert_eq!(simplified.last().copied(), Some(position(3, 65, 0)));
    }

    #[test]
    fn a_deep_drop_is_reported_as_a_fall_and_a_shallow_one_as_a_descent() {
        assert_eq!(
            SegmentAction::of_move(MoveKind::Drop { blocks: 5 }, 3),
            SegmentAction::Fall
        );
        assert_eq!(
            SegmentAction::of_move(MoveKind::Drop { blocks: 2 }, 3),
            SegmentAction::Descend
        );
    }

    #[test]
    fn actions_are_deduplicated_in_first_seen_order() {
        let actions = actions_of(
            &[
                MoveKind::Walk,
                MoveKind::Walk,
                MoveKind::JumpUp,
                MoveKind::Walk,
                MoveKind::Break { blocks: 1 },
            ],
            3,
        );
        assert_eq!(
            actions,
            vec![
                SegmentAction::Walk,
                SegmentAction::Jump,
                SegmentAction::Break
            ]
        );
    }
}
