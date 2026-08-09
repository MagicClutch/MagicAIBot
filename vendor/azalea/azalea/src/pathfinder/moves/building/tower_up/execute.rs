//! What to do, tick by tick, once a pillar-up edge (see [`super::planner`])
//! is the active step of a route: the sneak/jump/place cycle and its tick
//! timing. See [`super`]'s module doc comment (one level up) for the full
//! walkthrough of the physics involved.

use azalea_client::WalkDirection;
use azalea_core::{direction::Direction, position::BlockPos};
use tracing::{info, warn};

use crate::pathfinder::{
    moves::ExecuteCtx,
    moves::building::bridging::scaffold_predicate,
    policy::PathfindingPolicy,
    world::is_block_state_passable,
};

/// Per-edge dedup marker so [`log_pillar_started`] logs exactly once per
/// pillar step rather than every tick it remains the front of the path
/// (`execute_pillar_up_move` is re-invoked every `GameTick` while its edge
/// is active).
#[derive(Clone, Default)]
struct LoggedPillarTarget(Option<BlockPos>);

// `try_write` (not `write`) for the same reason `refresh_pathfinding_policy`
// uses it: a read lock on this `RwLock` may be held for the duration of an
// in-flight background A* search (up to several seconds, per
// `CustomPathfinderState`'s own doc comment), and this runs on the game-tick
// thread -- a missed log on one tick is harmless since the next tick retries.
fn log_pillar_started(ctx: &ExecuteCtx) {
    let Some(mut state) = ctx.custom_state.try_write() else {
        return;
    };
    let logged = state
        .get::<LoggedPillarTarget>()
        .cloned()
        .unwrap_or_default();
    if logged.0 != Some(ctx.target) {
        info!("pillaring upward to {}", ctx.target);
        state.insert(LoggedPillarTarget(Some(ctx.target)));
    }
}

/// Updates (or starts fresh) this entity's [`crate::pathfinder::moves::PillarClimbBaseline`]
/// for this tick's step, and returns how tall the climb would be, in
/// blocks, once `target` is reached.
///
/// Reads the *current* baseline straight from `ctx.pillar_climb_baseline`
/// (a plain query field, populated fresh every tick -- no locking involved)
/// and persists the update via [`ExecuteCtx::set_pillar_climb_baseline`]
/// (a `Commands` insert, likewise lock-free). An earlier version stored
/// this in `CustomPathfinderState`'s shared map instead, gated behind
/// `try_write`; that map is guarded by a single `RwLock` a background A*
/// search can hold a *read* lock on for seconds while it searches (see
/// `MovesCtx::custom_state`'s doc comment), and multiple readers don't
/// block each other -- but `try_write` needs *exclusive* access, so it
/// silently failed on nearly every tick while any search was in flight
/// (routine whenever the goal is far away, which is exactly when a height
/// cap matters most). Every failed tick made the check a no-op rather than
/// "not yet confirmed over the limit" the way a missed *log* line would be,
/// so the bot kept climbing on almost every tick regardless of the
/// configured cap (caught by `test_pillar_respects_max_pillar_height_cap`
/// asserting on the actual number of blocks placed, not just the bot's
/// peak height).
///
/// The baseline only ever resets when `start` is *below* the last recorded
/// one, never merely because `start` doesn't exactly match whatever the
/// previous tick recorded as the climb's target: a route that gets patched
/// or replanned mid-climb (`check_node_reached`/`patch_path_from_timeout`)
/// can hand execution a technically-new edge for what is, physically, the
/// same continuous climb, and resetting on that alone reintroduces the same
/// "creeps past the cap" bug through a different door. Only resetting on a
/// genuine descent is a strictly weaker, more conservative condition that
/// can't be defeated by replanning: as long as the bot hasn't dropped below
/// where this climb began, it's still the same climb no matter how many
/// times the path got rebuilt underneath it.
fn pillar_climb_height(ctx: &mut ExecuteCtx) -> i32 {
    let baseline_y = match ctx.pillar_climb_baseline {
        Some(p) if ctx.start.y >= p.baseline_y => p.baseline_y,
        // Either no climb recorded yet, or `start` is now *below* the last
        // recorded baseline (the bot came back down, or this is an
        // unrelated climb starting somewhere new) -- (re)start counting
        // from here.
        _ => ctx.start.y,
    };
    ctx.set_pillar_climb_baseline(baseline_y);
    ctx.target.y - baseline_y
}

/// Dedup marker for the height-limit warning, mirroring
/// [`LoggedPillarTarget`]: logs once when the cap is actually hit rather
/// than every tick the bot sits held at the capped height. `try_write`
/// against the shared custom-state map is fine here (unlike for the height
/// check itself) since a missed attempt just means one repeated log line,
/// not a bypassed safety cap.
#[derive(Clone, Default)]
struct LoggedHeightLimit(bool);

fn log_height_limit_reached(ctx: &ExecuteCtx, max_pillar_height: u32) {
    let Some(mut state) = ctx.custom_state.try_write() else {
        return;
    };
    let logged = state.get::<LoggedHeightLimit>().cloned().unwrap_or_default();
    if !logged.0 {
        warn!("pillar height limit ({max_pillar_height} blocks) reached, stopping tower");
        state.insert(LoggedHeightLimit(true));
    }
}

pub(super) fn execute_pillar_up_move(mut ctx: ExecuteCtx) {
    let start = ctx.start;
    log_pillar_started(&ctx);
    ctx.clear_blocked_on_scaffold();

    let policy = ctx.custom::<PathfindingPolicy>();

    // Safety ceiling: refuse to climb further once this continuous climb
    // has already reached `max_pillar_height` blocks tall. `0` means
    // "unset" (uncapped), matching every other "0 means unset" field on
    // `PathfindingPolicy` -- short-circuited so an uncapped climb (the
    // common case) never even touches `pillar_climb_height`'s bookkeeping.
    if policy.max_pillar_height > 0
        && pillar_climb_height(&mut ctx) > policy.max_pillar_height as i32
    {
        // Hold position (don't jump or place further) rather than keep
        // climbing. `timeout_movement` notices this carve-out and cancels
        // the route rather than stall forever, the same way running out of
        // scaffold material is handled.
        log_height_limit_reached(&ctx, policy.max_pillar_height);
        ctx.mark_blocked_on_scaffold();
        ctx.sneak();
        ctx.walk(WalkDirection::None);
        return;
    }

    // Step 1 of the tick-timing sequence in this module's doc comment:
    // move exactly to the center of the column and hold there, sneaking,
    // for the whole jump-and-place cycle.
    //
    // An earlier version of this function tried to walk the bot to
    // `start`'s exact center first (mirroring `ExecuteCtx::mine_while_at_start`)
    // before allowing the sneak/jump/place cycle to proceed, gated on
    // `ctx.position` being within a small horizontal tolerance of
    // `start.center()`. In practice this reintroduced the cycle it was
    // trying to perfect: any per-tick drift that crossed the tolerance
    // mid-jump (residual velocity, a look-direction change while any
    // velocity remained -- Minecraft movement input is relative to camera
    // yaw, so repeatedly re-aiming at the center while airborne can itself
    // nudge world-space velocity) flipped the check back to "not centered",
    // which un-sneaked and walked instead of continuing the jump/place
    // sequence -- observed live as the bot sneaking and un-sneaking forever
    // at the base of the tower, never jumping or placing a single block.
    // `look_at_exact` + holding still is enough in practice; do not gate
    // the cycle on a separate centering pass.
    ctx.sneak();
    ctx.walk(WalkDirection::None);
    ctx.look_at_exact(start.center());

    if !is_block_state_passable(ctx.get_block_state(start)) {
        // Already placed (possibly from a previous tick); nothing left to do
        // but wait for physics to actually land us there (is_reached handles
        // that).
        ctx.clear_placing();
        return;
    }

    if !ctx.physics.on_ground() {
        // Step 2/3: airborne from the jump below. Wait for the apex
        // (velocity has stopped rising) before placing -- see this module's
        // doc comment for the tick math and why this isn't load-bearing for
        // correctness here, just for matching real jump timing.
        if ctx.physics.velocity.y > 0.0 {
            return;
        }

        let Some(item_id) = policy.scaffold_item else {
            ctx.mark_blocked_on_scaffold();
            return;
        };
        ctx.place(start.down(1), Direction::Up, scaffold_predicate(item_id));
        return;
    }

    // Still grounded: jump to briefly vacate `start` so the placement below
    // isn't rejected for intersecting our own hitbox.
    ctx.jump();
}
