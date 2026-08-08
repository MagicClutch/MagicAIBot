//! Tower-up-specific configuration surface.
//!
//! Deliberately thin: whether pillaring is allowed at all
//! (`PathfindingPolicy::allow_pillaring`) and how tall a single climb may
//! get (`PathfindingPolicy::max_pillar_height`) live on the shared
//! [`crate::pathfinder::policy::PathfindingPolicy`] in `pathfinder::policy`,
//! not duplicated here -- bridging and staircasing pull from that exact
//! same struct (and the exact same scaffold-material inventory state), so a
//! route that mixes tower-up with either of them needs one consistent
//! source of truth for "can we place right now" rather than three
//! independently-configured copies that could disagree with each other.
//!
//! What *does* live here is the one constant that's purely an internal
//! implementation detail of how tower-up validates a climb, not something a
//! caller should ever need to tune.

/// How far below a candidate column
/// [`super::planner::has_real_ground_anchor_below`] will scan looking for
/// solid, unmodified world state before giving up. Generous (taller than
/// any sane single pillar climb, and independent of
/// [`crate::pathfinder::policy::PathfindingPolicy::max_pillar_height`]) so
/// it doesn't itself act as a hidden height limit -- it exists purely to
/// bound the scan's own cost, not to cap how high a climb may go.
pub(super) const MAX_ANCHOR_SCAN_DEPTH: i32 = 64;
