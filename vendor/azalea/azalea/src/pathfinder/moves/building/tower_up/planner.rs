//! Decides *whether* a pillar-up edge can be offered from a given position.
//! This is the `SuccessorsFn` entry point A* actually calls -- see
//! [`super`]'s module doc comment (one level up) for the full tick-timing
//! walkthrough of what happens once the edge this generates is executed
//! (that part lives in [`super::execute`]).

use crate::pathfinder::{
    astar, costs::*,
    moves::{Edge, MoveData, MovesCtx, building::bridging::policy_of, vertical_is_reached},
    positions::RelBlockPos,
    vertical::PILLAR_COST,
    world::CachedWorld,
};

use super::{config::MAX_ANCHOR_SCAN_DEPTH, execute::execute_pillar_up_move};

/// Whether `pos` sits directly above real, already-solid ground -- possibly
/// with several cells of open air in between (representing earlier
/// hypothetical pillar placements in the same climb that this same A*
/// search proposed but that haven't actually executed yet), but eventually
/// landing on genuine solid ground within [`MAX_ANCHOR_SCAN_DEPTH`] blocks.
///
/// See [`pillar_up_move`]'s doc comment for why this, not
/// `CachedWorld::is_standable`, is what gates offering a pillar edge.
pub(super) fn has_real_ground_anchor_below(world: &CachedWorld, pos: RelBlockPos) -> bool {
    let mut below = pos.down(1);
    for _ in 0..MAX_ANCHOR_SCAN_DEPTH {
        if !world.is_block_passable(below) {
            return true;
        }
        below = below.down(1);
    }
    false
}

/// Generates a single "climb one block straight up" edge from `pos`, if
/// tower building is currently allowed and possible there. A* composes as
/// many of these edges in a row as needed, interleaved with every other
/// move this pathfinder knows, so it automatically chooses to tower up
/// (rather than route around, bridge, or fail outright) whenever that turns
/// out to be the cheapest way to reach a goal above the bot -- no special
/// "goal is higher than me" branch is needed anywhere else, and `/goto` and
/// `/follow` get this automatically since both submit routes through this
/// same `successors_fn`.
///
/// This intentionally does **not** enforce
/// [`crate::pathfinder::policy::PathfindingPolicy::max_pillar_height`] --
/// see [`super::execute::execute_pillar_up_move`] for where and why that
/// cap actually lives.
pub fn pillar_up_move(ctx: &mut MovesCtx, pos: RelBlockPos) {
    let policy = policy_of(ctx);
    if !policy.allow_pillaring || !policy.can_place() {
        return;
    }
    // Not `CachedWorld::is_standable(pos)` -- that checks whether `pos`
    // itself is standable *right now*, in the live, unmodified world. That
    // holds for the bot's real starting position (the very first pillar
    // step in any climb), but is always false for the second step onward:
    // `pos` there is a hypothetical column that only becomes real once an
    // *earlier* pillar edge in this same search actually executes and
    // places its block, and the search only ever sees today's real world
    // state, where that block doesn't exist yet. That capped every single
    // search at exactly one pillar edge no matter how high the goal was,
    // forcing a full repath (with its own multi-hundred-millisecond-to-
    // multi-second minimum search time, see `PathfinderOpts::min_timeout`)
    // after *every single block* -- what actually made towering look
    // stalled/unresponsive in practice, not a jump or camera bug.
    //
    // `has_real_ground_anchor_below` accepts that same hypothetical column
    // by tolerating the (also hypothetical, still-air) gap below it, as
    // long as scanning down through it eventually reaches genuine solid
    // ground -- which is exactly what "continuing the same climb" means.
    // Dropping the check entirely (rather than replacing it with this) was
    // tried first and made pillaring available from *any* reached node,
    // including ones reached by bridging or walking with nothing solid
    // underneath for dozens of blocks -- correct in principle (you genuinely
    // could place a block under your feet there) but it exploded the
    // branching factor of every build-capable search, even ones that never
    // needed to consider pillaring at all (caught by
    // `test_fast_bridge_path_cancellation` timing out before finding any
    // path at all for a purely horizontal 12-block bridge). Requiring a
    // bounded real anchor keeps chaining working for genuine vertical
    // climbs while excluding the vast majority of nodes that have no solid
    // ground within a sane distance below them.
    if !has_real_ground_anchor_below(ctx.world, pos) {
        return;
    }
    // Room to jump into, and to stand once the block below has been filled
    // -- the anti-suffocation check: refuses to even offer this move if the
    // bot's hitbox (roughly 2 blocks tall) wouldn't fit.
    if !ctx.world.is_block_passable(pos.up(1)) || !ctx.world.is_block_passable(pos.up(2)) {
        return;
    }

    let cost =
        f32::max(WALK_ONE_BLOCK_COST, *JUMP_ONE_BLOCK_COST) + JUMP_PENALTY + PILLAR_COST as f32;

    ctx.edges.push(Edge {
        movement: astar::Movement {
            target: pos.up(1),
            data: MoveData {
                execute: &execute_pillar_up_move,
                is_reached: &vertical_is_reached,
            },
        },
        cost,
    });
}
