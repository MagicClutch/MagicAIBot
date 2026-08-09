//! Terrain-modifying movement primitives the pathfinder can weave into any
//! route automatically -- bridging gaps and towering straight up -- grouped
//! here as a single "Building" capability area. Neither submodule is a
//! separate command: both are ordinary A* graph edges considered by
//! [`crate::pathfinder::moves::combined_move`] alongside walking, jumping,
//! and climbing, so `/goto`, `/follow`, and every other navigation task get
//! them for free, automatically, with no special-cased "the goal is above
//! me" branch anywhere in the command layer.
//!
//! - [`bridging`]: place a block ahead to cross a gap, and stair-step up
//!   terrain that's one block short of a landing. Unchanged from before
//!   this module existed -- see its own doc comment.
//! - [`tower_up`]: pillar straight up by placing a block underfoot while
//!   airborne. See its own doc comment for the full jump-timing walkthrough.
//!
//! Which one (if either) ends up in a given route isn't decided by a fixed
//! rule table here -- it's whatever combination of edges A* finds cheapest
//! for the actual terrain it's exploring. Both submodules only ever offer
//! an edge where the cheaper walk/jump moves in
//! [`crate::pathfinder::moves::basic`]/[`crate::pathfinder::moves::parkour`]
//! can't already reach, so a single route can freely start with a bridge
//! segment and finish with a tower-up climb (or the reverse) without either
//! submodule needing to know the other exists.

pub mod bridging;
pub mod tower_up;

use crate::pathfinder::{moves::MovesCtx, positions::RelBlockPos};

/// The combined "Building" successors: every bridging/staircasing edge
/// [`bridging::build_move`] can offer, plus every tower-up edge
/// [`tower_up::pillar_up_move`] can offer, from the same position. Called
/// once per candidate node by [`crate::pathfinder::moves::combined_move`],
/// the same way [`crate::pathfinder::moves::default_move`] calls the
/// walking/jumping/climbing move sets.
pub fn building_move(ctx: &mut MovesCtx, node: RelBlockPos) {
    bridging::build_move(ctx, node);
    tower_up::pillar_up_move(ctx, node);
}
