//! Baritone-style terrain-modifying movement primitives: bridging across
//! gaps and staircasing up terrain that's one block short of a landing.
//! Pillaring straight up lives in the sibling [`crate::pathfinder::moves::building::tower_up`]
//! module. These are the moves [`crate::pathfinder::moves::basic`]/
//! [`crate::pathfinder::moves::parkour`] cannot express because they only
//! ever mine, never place.
//!
//! Unchanged from before this file moved under
//! [`crate::pathfinder::moves::building`] -- this reorganization is a pure
//! file move (plus updating the `use` paths that changed because of it),
//! not a behavior change. See [`super::building_move`] for how this and
//! [`super::tower_up`] both plug into the pathfinder as ordinary movement
//! primitives.
//!
//! Each move only fires when the equivalent walk/mine move in
//! [`crate::pathfinder::moves::basic`] can't already handle the situation --
//! see each function's `cost_for_standing`/`cost_for_breaking_block` guard
//! -- so enabling this move set never makes an already-cheap route more
//! expensive or duplicates an edge `default_move` would already offer.
//!
//! Placement has no client-side prediction (see
//! `azalea_client::interact::handle_start_use_item_queued`), so every
//! `execute_*` function here re-checks live world state each tick rather
//! than assuming a placement dispatched on a previous tick has landed, and
//! every `is_reached` function requires `physics.on_ground()` (not just a
//! position match) so a path never advances past a step whose support block
//! hasn't actually been confirmed by the server yet.

use azalea_block::{BlockState, properties::SlabKind};
use azalea_client::WalkDirection;
use azalea_core::{
    direction::{CardinalDirection, Direction},
    position::{BlockPos, Vec3},
};
use azalea_inventory::ItemStack;
use tracing::info;

use crate::pathfinder::{
    astar,
    costs::*,
    moves::{Edge, ExecuteCtx, MoveData, MovesCtx, player_pos_to_block_pos, vertical_is_reached},
    policy::PathfindingPolicy,
    positions::RelBlockPos,
    vertical::PLACE_COST,
    world::is_block_state_passable,
};

pub fn build_move(ctx: &mut MovesCtx, node: RelBlockPos) {
    bridge_move(ctx, node);
    staircase_up_move(ctx, node);
}

/// Shared with [`super::tower_up`], which needs the same policy snapshot
/// for its own pillar-up move.
pub(super) fn policy_of(ctx: &MovesCtx) -> PathfindingPolicy {
    ctx.custom_state
        .get::<PathfindingPolicy>()
        .cloned()
        .unwrap_or_default()
}

/// Maps a single cardinal-axis step (each of `dx`/`dz` in `{-1, 0, 1}`, with
/// exactly one nonzero) to the [`Direction`] of the block face facing that
/// way. Used to click the correct side of a reference block instead of only
/// ever placing on top of it.
fn horizontal_face(dx: i32, dz: i32) -> Direction {
    match (dx.signum(), dz.signum()) {
        (0, -1) => Direction::North,
        (0, 1) => Direction::South,
        (1, 0) => Direction::East,
        (-1, 0) => Direction::West,
        _ => Direction::Up,
    }
}

/// Shared with [`super::tower_up`] for the same reason [`policy_of`] is.
pub(super) fn scaffold_predicate(item_id: String) -> impl Fn(&ItemStack) -> bool {
    move |item: &ItemStack| item.kind().to_string() == item_id
}

/// The absolute world coordinate of `block`'s boundary facing `face`.
///
/// A block at integer index `N` occupies the half-open range `[N, N + 1)`
/// on each horizontal axis. So the boundary facing the block at `N + 1`
/// (south/east) sits exactly at `N + 1.0` -- but a position *inside* that
/// neighboring block, right at its near edge, reads as `N + 0.999...`, not
/// `N + 1.0`: block `N + 1`'s own local coordinate starts just under 1.0
/// right at the shared boundary and counts down from there as you walk
/// further into it. The boundary facing the block at `N - 1` (north/west)
/// is simply `N` itself, with no such offset.
fn face_boundary(block: BlockPos, face: Direction) -> f64 {
    match face {
        Direction::North => block.z as f64,
        Direction::South => block.z as f64 + 1.0,
        Direction::East => block.x as f64 + 1.0,
        Direction::West => block.x as f64,
        _ => 0.0,
    }
}

/// How close to the true boundary (see [`face_boundary`]) the bot walks
/// before treating itself as "at the edge" and placing. Vanilla Minecraft
/// lets a sneaking player hang their hitbox out *past* this boundary
/// without falling (`maybeBackOffFromEdge`), but this vendored physics
/// engine does not implement that clamp -- sneaking here only affects
/// pose/speed, not collision -- so actually crossing the boundary is not
/// protected against falling. Empirically (see `test_bridge_across_gap`),
/// crossing it makes the bot get stuck rather than fall cleanly, so this
/// stays on the near side of the boundary: close enough to reliably
/// see/reach the side face without relying on a safety net this engine
/// doesn't have.
const EDGE_MARGIN: f64 = 0.05;

/// Fallback for [`PathfindingPolicy::fast_bridge_edge_threshold`] when it's
/// left at its `Default` value (`0.0`, meaning "unset"). Wider than
/// [`EDGE_MARGIN`] on purpose: fast bridging approaches at normal (not
/// sneak-slowed) walking speed, and a single tick's movement at that speed
/// can be wider than `EDGE_MARGIN` itself, so a margin that tight would let
/// the bot's position skip clean over the detection window in one tick and
/// walk straight off the edge without ever sneaking to place. See
/// `test_fast_bridge_edge_threshold_survives_full_speed_approach`.
const FAST_BRIDGE_EDGE_THRESHOLD_FALLBACK: f64 = 0.2;

fn at_edge_toward(position: Vec3, block: BlockPos, face: Direction, margin: f64) -> bool {
    let boundary = face_boundary(block, face);
    match face {
        Direction::North => position.z <= boundary + margin,
        Direction::West => position.x <= boundary + margin,
        Direction::South => position.z >= boundary - margin,
        Direction::East => position.x >= boundary - margin,
        _ => true,
    }
}

/// The world-space midpoint of the specific side of `block` facing `face`
/// (e.g. for `Direction::North`, the middle of the block's north face).
///
/// `ExecuteCtx::look_at` locks pitch level with the bot's own eyes, which is
/// wrong for these moves: the reference block being clicked is the one the
/// bot is standing on (or the floor ahead), always below eye height, so a
/// level look aims out over it instead of down at it. Looking at this point
/// with `look_at_exact` instead points the bot down *and* toward the actual
/// face it's about to click, rather than out at the horizon.
fn face_center(block: BlockPos, face: Direction) -> Vec3 {
    let center = block.center();
    match face {
        Direction::North => Vec3 {
            z: block.z as f64,
            ..center
        },
        Direction::South => Vec3 {
            z: block.z as f64 + 1.0,
            ..center
        },
        Direction::East => Vec3 {
            x: block.x as f64 + 1.0,
            ..center
        },
        Direction::West => Vec3 {
            x: block.x as f64,
            ..center
        },
        Direction::Up => Vec3 {
            y: block.y as f64 + 1.0,
            ..center
        },
        Direction::Down => Vec3 {
            y: block.y as f64,
            ..center
        },
    }
}

/// Extra cost added to a bridge/staircase edge that turns away from
/// whichever horizontal axis the current search has traveled furthest along
/// from its own origin (`pos`'s own coordinates are already relative to it,
/// see [`RelBlockPos`]).
///
/// Purely cardinal build-move segments cost exactly the same in total
/// whether they're ordered as one straight run or interleaved between
/// axes, so without this bias A* has no reason to prefer one ordering over
/// the other and can end up alternating X/Z steps into a staircase-shaped
/// "diagonal" bridge instead of a straight line toward the goal. This is a
/// soft penalty, not a hard restriction: a genuinely necessary turn (e.g.
/// working around an obstacle the straight axis can't cross) still costs
/// less than failing to find a route at all, so it stays available -- it's
/// just no longer free to pick arbitrarily when going straight would do.
///
/// This deliberately does *not* persist any state across repaths (an
/// earlier version tracked "which axis was actually built most recently" in
/// `CustomPathfinderState`, updated every tick a build move executed). That
/// broke obstruction detection: `check_path_obstructed`
/// (`execute/patching.rs`) re-evaluates every path edge's cost every tick
/// and treats any cost *increase* since the edge was first computed as "the
/// path is obstructed," triggering an expensive repatch. Since that
/// execution-time state changed on every placement, the same edge's cost
/// could differ from one tick to the next with nothing in the world having
/// changed, causing continuous false-positive obstruction detection --
/// dozens of spurious repatches per second in practice, which is what
/// caused the "stops after a few blocks" / "lags in the air" bug. Being
/// purely a function of `pos` (which only depends on the search's own fixed
/// origin, never execution history) keeps edge costs stable between
/// evaluations, at the cost of the straightness bias resetting on each
/// repath instead of persisting across the whole route.
const TURN_PENALTY: f32 = 1.0;
fn turn_penalty(pos: RelBlockPos, dx: i16, dz: i16) -> f32 {
    if pos.x == 0 && pos.z == 0 {
        // No committed direction yet -- this is the first step away from
        // the search's own origin, so either axis is an equally valid start.
        return 0.0;
    }
    let dominant_axis_is_x = pos.x.unsigned_abs() >= pos.z.unsigned_abs();
    let on_dominant_axis = if dominant_axis_is_x { dx != 0 } else { dz != 0 };
    if on_dominant_axis { 0.0 } else { TURN_PENALTY }
}

/// Per-edge dedup marker so [`log_bridge_started`] logs exactly once when a
/// build move begins executing rather than every tick it remains the front
/// of the path (`execute_*` is re-invoked every `GameTick` while its edge is
/// active, unlike the rest of the pathfinder's tracing which is already
/// naturally one-shot at node-reached/replan transitions).
#[derive(Clone, Default)]
struct LoggedBridgeTarget(Option<BlockPos>);

// `try_write` (not `write`) for the same reason `refresh_pathfinding_policy`
// uses it: a read lock on this `RwLock` may be held for the duration of an
// in-flight background A* search (up to several seconds, per
// `CustomPathfinderState`'s own doc comment), and this runs on the game-tick
// thread -- a missed log on one tick is harmless since the next tick retries.
fn log_bridge_started(ctx: &ExecuteCtx) {
    let Some(mut state) = ctx.custom_state.try_write() else {
        return;
    };
    let logged = state
        .get::<LoggedBridgeTarget>()
        .cloned()
        .unwrap_or_default();
    if logged.0 != Some(ctx.target) {
        info!("bridging gap to {}", ctx.target);
        state.insert(LoggedBridgeTarget(Some(ctx.target)));
    }
}

/// Dedup marker for [`log_fast_bridge_direction`], keyed by cardinal
/// direction rather than by target block: fast bridging can cross many
/// blocks in a single straight run, and logging "Fast bridging east" once
/// per block would violate the "keep logging concise" requirement this
/// move set follows. Logs once per direction *segment* instead -- once
/// when a new direction starts, and once ("Bridge completed") when it
/// changes to a different one.
#[derive(Clone, Default)]
struct LoggedFastBridgeDirection(Option<Direction>);

fn direction_name(face: Direction) -> &'static str {
    match face {
        Direction::North => "north",
        Direction::South => "south",
        Direction::East => "east",
        Direction::West => "west",
        Direction::Up => "up",
        Direction::Down => "down",
    }
}

fn log_fast_bridge_direction(ctx: &ExecuteCtx, face: Direction) {
    let Some(mut state) = ctx.custom_state.try_write() else {
        return;
    };
    let logged = state
        .get::<LoggedFastBridgeDirection>()
        .cloned()
        .unwrap_or_default();
    if logged.0 != Some(face) {
        if logged.0.is_some() {
            info!("Bridge completed");
        }
        info!("Fast bridging {}", direction_name(face));
        state.insert(LoggedFastBridgeDirection(Some(face)));
    }
}

// ---------------------------------------------------------------------
// Bridge: place a block ahead at the current height to cross a gap with no
// floor. Fires only where `cost_for_standing` is infinite (no floor, and
// nothing there to mine into one), so it never competes with `forward_move`.
// ---------------------------------------------------------------------

fn bridge_move(ctx: &mut MovesCtx, pos: RelBlockPos) {
    let policy = policy_of(ctx);
    if !policy.allow_bridging || !policy.can_place() {
        return;
    }

    for dir in CardinalDirection::iter() {
        let offset = RelBlockPos::new(dir.x(), 0, dir.z());
        let new_pos = pos + offset;

        if ctx.world.cost_for_standing(new_pos, ctx.mining_cache) != f32::INFINITY {
            // forward_move (walking or mining onto existing ground) already
            // covers this.
            continue;
        }
        if !ctx.world.is_passable(new_pos) {
            continue;
        }
        // The cell below must be an actual placeable gap (air), not liquid
        // (deliberately conservative -- crossing lava/water is left for
        // future work) or an already-solid obstruction `forward_move` would
        // have handled.
        if !ctx.world.is_block_passable(new_pos.down(1)) {
            continue;
        }

        let cost = WALK_ONE_BLOCK_COST + PLACE_COST as f32 + turn_penalty(pos, dir.x(), dir.z());

        ctx.edges.push(Edge {
            movement: astar::Movement {
                target: new_pos,
                data: MoveData {
                    execute: &execute_bridge_move,
                    is_reached: &vertical_is_reached,
                },
            },
            cost,
        });
    }
}

/// The look target for the placement instant of a bridge move: a point
/// clearly offset *behind* `reference` (not just its center, which sits
/// almost directly under the bot with near-zero horizontal offset) so the
/// resulting yaw is well-defined -- an ambiguous near-zero offset here was
/// the root cause the first time this was tried and made the bot look/walk
/// the wrong way entirely.
///
/// `sideways_position` locks the axis perpendicular to travel to the bot's
/// own live position rather than the reference block's fixed center:
/// physics wobble while approaching the edge means the bot is rarely
/// sitting exactly centered on that axis, so anchoring to the block's
/// center adds a small sideways component to the look vector -- enough to
/// skew yaw off the true 90 degrees to the face. That reads as approaching/
/// placing at an angle instead of straight-on, and can miss the face
/// cleanly enough that the placement doesn't land.
///
/// This must stay locked to live position unconditionally, never falling
/// back to the block's fixed center even when live position is far off of
/// it: `WalkDirection::Backward` moves the bot *away* from whatever it's
/// looking at, so a look target with a nonzero sideways offset from the
/// bot's actual position pushes it further sideways, not back toward
/// center -- centering only the look target while walking backward is a
/// self-reinforcing drift, not a correction (confirmed empirically: an
/// earlier version of this function that recentered when live position was
/// far from center made corner drift *worse*, not better -- see git
/// history/`test_fast_bridge_single_corner`). Locking the sideways
/// component to wherever the bot actually is keeps that offset at exactly
/// zero every tick by construction, so the look mechanism itself can never
/// introduce sideways drift, regardless of how far off-axis the bot
/// happens to be when a segment starts.
fn look_behind_point(reference: BlockPos, face: Direction, sideways_position: Vec3) -> Vec3 {
    // A smaller offset pitches the look down steeper (ideally close to
    // vertical, ~80 degrees), but empirically (see `test_bridge_across_gap`/
    // `test_combined_bridge_and_pillar_route`) anything below ~0.45 makes
    // the classic bridge move's walk-backward-to-move-forward technique
    // break -- the resulting yaw becomes too dominated by the vertical
    // component for `WalkDirection::Backward` to resolve reliably in this
    // engine. 0.5 is the steepest value that stayed reliable across a full
    // test-suite run (~65 degrees pitch, short of the ~80 that was asked
    // for, but this is the real ceiling this engine's movement resolution
    // allows without breaking direction). Fast bridging doesn't walk during
    // this instant (see `execute_fast_bridge_move`), so it isn't bound by
    // that same constraint, but reuses the same proven-reliable value
    // rather than an unvalidated steeper one.
    const LOOK_BEHIND_OFFSET: f64 = 0.5;
    let mut behind = reference.center() + face.opposite().normal_vec3() * LOOK_BEHIND_OFFSET;
    match face {
        Direction::North | Direction::South => behind.x = sideways_position.x,
        Direction::East | Direction::West => behind.z = sideways_position.z,
        _ => {}
    }
    behind
}

/// The vertical range (as a fraction of a full block -- `0.0` is the
/// bottom, `1.0` is the top) that actually has solid, clickable collision
/// for a slab in the given [`SlabKind`]. A top slab's hitbox only occupies
/// the *upper* half; aiming anywhere below `0.5` (e.g. a full block's
/// vertical center, which is exactly the lower boundary of this range)
/// lands in empty space and the placement ray never lands.
fn slab_hitbox_y_range(kind: SlabKind) -> (f64, f64) {
    match kind {
        SlabKind::Bottom => (0.0, 0.5),
        SlabKind::Top => (0.5, 1.0),
        SlabKind::Double => (0.0, 1.0),
    }
}

/// How far below a slab's true top edge ([`slab_hitbox_y_range`]'s upper
/// bound) the fast-bridge aim point starts out, so it lands solidly inside
/// the slab's collision instead of skimming the exact boundary -- the same
/// reasoning as [`EDGE_MARGIN`], applied to the vertical edge instead of a
/// horizontal one. Halved by [`find_inset_click_offset`] on each retry if
/// the previous inset undershot past the *bottom* of the hitbox (i.e. the
/// range turned out narrower than this margin allows) -- shrinking moves
/// the candidate back *toward* the true edge, which is the only direction
/// that can ever recover an undershoot, unlike growing the inset further
/// (which only moves further away from the edge it already overshot past).
const SLAB_AIM_INSET: f64 = 0.05;
const SLAB_AIM_MAX_ATTEMPTS: u32 = 4;

/// Searches for a click position (as the same `0.0`-`1.0` fraction
/// [`slab_hitbox_y_range`] uses) that lands strictly inside `(lo, hi)`,
/// starting `SLAB_AIM_INSET` below `hi` and halving that inset on each
/// retry if the previous attempt undershot past `lo`. This is the
/// raycast-validation-and-retry step in isolation from any particular
/// [`SlabKind`], so it can be exercised directly with narrow synthetic
/// ranges (see this module's tests) as well as via [`slab_click_height`].
///
/// Returns `None` if no attempt up to [`SLAB_AIM_MAX_ATTEMPTS`] lands inside
/// the range -- i.e. the range is narrower than even a fully-shrunk inset
/// allows. Callers must not place when this happens, and should hold
/// position instead (mirroring how a missing scaffold item is already
/// handled) until the support becomes solid again or the bridge is
/// abandoned.
fn find_inset_click_offset(lo: f64, hi: f64) -> Option<f64> {
    let mut inset = SLAB_AIM_INSET;
    for _ in 0..SLAB_AIM_MAX_ATTEMPTS {
        let candidate = hi - inset;
        if candidate > lo && candidate < hi {
            return Some(candidate);
        }
        inset /= 2.0;
    }
    None
}

/// Computes the absolute world Y to aim/click at on a slab support block's
/// side face so a fast-bridge placement lands on real collision instead of
/// skimming past a top slab's empty lower half -- the root cause this move
/// exists to fix, since aiming at a full block's vertical center (as
/// [`look_behind_point`] does) lands exactly on a top slab's own lower
/// boundary, which is *not* solid.
///
/// Takes the live block state rather than caching anything, so a slab that
/// changed between ticks (merged into a double slab by a previous bridge
/// step, replaced by something else) is picked up immediately -- see the
/// module doc comment's "re-check everything, every tick" rule.
///
/// Returns `None` if `state` isn't a slab at all, or if [`find_inset_click_offset`]
/// couldn't find a valid offset. This is the raycast-validation step:
/// callers must not place when this returns `None`.
fn slab_click_height(reference: BlockPos, state: BlockState) -> Option<f64> {
    let kind = state.property::<SlabKind>()?;
    let (lo, hi) = slab_hitbox_y_range(kind);
    let offset = find_inset_click_offset(lo, hi)?;
    Some(reference.y as f64 + offset)
}

/// Look target for slab-aware fast bridging: unlike [`look_behind_point`]
/// (which anchors its horizontal offset to the fixed reference block, and
/// only pins the *sideways* axis to live position), this anchors the
/// *entire* horizontal offset to the bot's own live position every tick.
///
/// A fixed-block anchor was tried first and broke both camera stability and
/// safety:
///
/// - **Stability**: the bot's distance from a fixed anchor grows over an
///   entire segment (it walks from one end of the block to the other), so
///   the resolved pitch swings by tens of degrees within a single segment
///   (observed: ~89 down to ~54 and back, every block) -- exactly the
///   "per-block oscillation" this move must not have. Anchoring to live
///   position instead keeps the horizontal offset from the eye to the aim
///   point exactly [`SLAB_LOOK_BEHIND_OFFSET`] every tick by construction,
///   so pitch stays essentially constant for the whole segment (only
///   wobbling the few degrees that toggling sneak's eye height causes
///   during the brief placement pulse).
/// - **Safety**: a fixed anchor's offset has to be large enough (`>= 0.5`,
///   a full block's center-to-edge distance) to guarantee the aim point
///   never ends up *ahead* of wherever the bot happens to be standing when
///   a new segment begins close to its near edge (segments can start
///   there, not just at the block's center -- `vertical_is_reached` only
///   requires the *column* to match). Anchoring to live position instead
///   makes that guarantee unconditional and offset-independent: the aim
///   point is always exactly `SLAB_LOOK_BEHIND_OFFSET` behind wherever the
///   bot currently is, for any offset value, by construction.
///
/// This is safe to decouple from the reference block entirely because the
/// look target only ever controls camera direction here -- the actual
/// placement targets `reference`/`face` directly as separate, independent
/// parameters to [`azalea_client`]'s placement event (see
/// [`super::ExecuteCtx::place`]), so it doesn't depend on the camera ray
/// visually intersecting that exact block.
///
/// With the aim point's vertical component near the top of the slab
/// (`click_y`, a drop of standing eye height 1.62 plus a small inset below
/// the bot's own feet) and the same `0.5` horizontal offset
/// [`look_behind_point`] uses, `atan2(1.67, 0.5)` ≈ 73 degrees -- steeper
/// than [`look_behind_point`]'s ~65, and closer to (if, for the same reason
/// its doc comment gives, still short of) the ~78 degrees slab bridging is
/// asked for.
const SLAB_LOOK_BEHIND_OFFSET: f64 = 0.5;

fn slab_look_behind_point(face: Direction, live_position: Vec3, click_y: f64) -> Vec3 {
    let mut behind = live_position + face.opposite().normal_vec3() * SLAB_LOOK_BEHIND_OFFSET;
    behind.y = click_y;
    behind
}

/// Dedup marker for [`log_slab_bridge_mode`], mirroring
/// [`LoggedFastBridgeDirection`]. Logs "Slab bridge mode enabled" once when
/// the current support block becomes a top/bottom slab, and otherwise stays
/// silent (including when it stops being one) -- keeping logging minimal,
/// per the module's requirements.
#[derive(Clone, Default)]
struct LoggedSlabBridgeMode(bool);

fn log_slab_bridge_mode(ctx: &ExecuteCtx, is_slab: bool) {
    let Some(mut state) = ctx.custom_state.try_write() else {
        return;
    };
    let logged = state
        .get::<LoggedSlabBridgeMode>()
        .cloned()
        .unwrap_or_default();
    if logged.0 != is_slab {
        if is_slab {
            info!("Slab bridge mode enabled");
        }
        state.insert(LoggedSlabBridgeMode(is_slab));
    }
}

fn execute_bridge_move(ctx: ExecuteCtx) {
    let policy = ctx.custom::<PathfindingPolicy>();
    if policy.fast_bridge_enabled {
        // Logged per direction segment, not per block/target -- see
        // `log_fast_bridge_direction`'s doc comment.
        execute_fast_bridge_move(ctx, &policy);
    } else {
        log_bridge_started(&ctx);
        execute_classic_bridge_move(ctx, &policy);
    }
}

/// Permanent-sneak bridging: sneaks (and reverse-walks, see
/// [`look_behind_point`]) for the entire approach-place-step cycle, not
/// just the placement instant. This is what a cautious player does when
/// lining up a tricky placement, and stays available as
/// [`PathfindingPolicy::fast_bridge_enabled`]'s off setting -- but see
/// [`execute_fast_bridge_move`] for the default, quicker technique.
fn execute_classic_bridge_move(mut ctx: ExecuteCtx, policy: &PathfindingPolicy) {
    ctx.clear_blocked_on_scaffold();
    let target = ctx.target;
    let start = ctx.start;
    let place_target = target.down(1);
    let face = horizontal_face(target.x - start.x, target.z - start.z);
    let reference = start.down(1);

    // Reverse-bridge the whole sequence: look, and walk, backward (opposite
    // `face`, the direction of travel). `WalkDirection` here is yaw-relative,
    // so "backward" while facing backward is what actually moves the bot
    // forward -- the real technique a cautious player uses to keep their
    // view (and a real raycast) meeting the reference block's forward-facing
    // side instead of staring into the gap being crossed.
    let behind = look_behind_point(reference, face, ctx.position);
    ctx.sneak();
    ctx.look_at_exact(behind);

    if !is_block_state_passable(ctx.get_block_state(place_target)) {
        ctx.clear_placing();
        ctx.walk(WalkDirection::Backward);
        return;
    }

    let Some(item_id) = policy.scaffold_item.clone() else {
        ctx.mark_blocked_on_scaffold();
        return;
    };

    if !at_edge_toward(ctx.position, start, face, EDGE_MARGIN) {
        // Not at the edge yet -- sneak-walk toward it first. Sneaking stops
        // the bot right at the boundary instead of falling into the gap, so
        // this is safe to just walk into every tick until it lands within
        // `EDGE_MARGIN`, the same way a player edges up to a ledge before
        // bridging off it.
        ctx.walk(WalkDirection::Backward);
        return;
    }

    ctx.walk(WalkDirection::None);
    if !ctx.physics.on_ground() {
        // Knocked/launched off the ground mid-approach (e.g. by a Wind
        // Charge/Wind Burst combo, knockback, an explosion) -- `reference`
        // was only ever valid to click while standing on solid ground right
        // next to it, so placing blind here would put a block wherever
        // `reference` happens to be, floating and disconnected from wherever
        // the bot actually lands. Hold position and wait to touch back down
        // before placing.
        return;
    }
    ctx.place(reference, face, scaffold_predicate(item_id));
}

/// Baritone-style "speed bridging": the same reverse-walk technique
/// [`execute_classic_bridge_move`] uses -- camera held steady on the
/// reference block's face (looking down and backward, never rotating
/// forward to look where it's walking), walking backward the entire
/// segment -- but *not* sneaking for the whole approach. Sneaking is only
/// pulsed on for the brief moment needed to place each block once close to
/// the edge, then released immediately, rather than held for the entire
/// approach-place-step cycle. This is significantly faster than permanent-
/// sneak bridging since normal (backward) walking isn't slowed by
/// sneaking, and is what `PathfindingPolicy::fast_bridge_enabled` selects
/// by default.
///
/// The camera target is recomputed every tick (like every other move in
/// this module), but only its sideways component tracks the bot's live
/// position to correct for physics wobble (see [`look_behind_point`]) --
/// the yaw/pitch it resolves to stays effectively fixed for the whole
/// segment, satisfying "don't rotate forward, don't snap between angles"
/// without needing separate "have I already looked here" state.
///
/// State isn't tracked explicitly between ticks -- which phase (walking vs.
/// placing) is live each tick is derived fresh from world state and
/// position, the same "re-check everything, every tick" approach every
/// other move in this module uses (see the module doc comment), so a tick
/// that gets skipped or repeated (lag, a missed placement confirmation)
/// can't desync the cycle.
fn execute_fast_bridge_move(mut ctx: ExecuteCtx, policy: &PathfindingPolicy) {
    ctx.clear_blocked_on_scaffold();
    let target = ctx.target;
    let start = ctx.start;
    let place_target = target.down(1);
    let face = horizontal_face(target.x - start.x, target.z - start.z);
    let reference = start.down(1);
    log_fast_bridge_direction(&ctx, face);
    let threshold = if policy.fast_bridge_edge_threshold > 0.0 {
        policy.fast_bridge_edge_threshold
    } else {
        FAST_BRIDGE_EDGE_THRESHOLD_FALLBACK
    };

    // Dynamic slab detection: a top slab needs a different aim point than
    // an ordinary full block, since its solid hitbox only occupies the
    // upper half of the cell -- see `slab_click_height`.
    let reference_state = ctx.get_block_state(reference);
    let is_slab_support = reference_state.property::<SlabKind>().is_some();
    log_slab_bridge_mode(&ctx, is_slab_support);
    let slab_click_y = slab_click_height(reference, reference_state);

    // Camera stays on the reference block's face the whole segment -- see
    // this function's doc comment. `WalkDirection` is yaw-relative, so
    // "backward" while facing backward is what actually moves the bot
    // forward along the bridge.
    let behind = match slab_click_y {
        Some(click_y) => slab_look_behind_point(face, ctx.position, click_y),
        None => look_behind_point(reference, face, ctx.position),
    };
    ctx.look_at_exact(behind);

    if !is_block_state_passable(ctx.get_block_state(place_target)) {
        ctx.clear_placing();
        // `margin: 0.5` against `target` (rather than `start`) lands exactly
        // on the target block's center along the travel axis -- see
        // `at_edge_toward`'s boundary math.
        if at_edge_toward(ctx.position, target, face, 0.5) {
            // Reached (or just passed) the new block's center -- stop and
            // let momentum settle instead of continuing to walk
            // indefinitely. Sneaking here also engages Minecraft's
            // ledge-safety collision as a backstop against overshooting off
            // the far edge.
            //
            // This matters specifically for the last edge of a path: the
            // stricter last-edge `is_reached` check in `check_node_reached`
            // requires velocity-predicted drift from the target's center to
            // be small, which a full-speed (non-sneaking) approach can
            // permanently fail to satisfy unless it actually walks close to
            // center before stopping -- stopping right at the near edge
            // (e.g. as soon as `block_pos_matches`) leaves it too far off
            // center to ever pass that check, and never stopping at all
            // means nothing keeps it from sliding off the far edge into the
            // void. Walking to center first and stopping there satisfies
            // both.
            ctx.sneak();
            ctx.walk(WalkDirection::None);
        } else {
            // Still crossing toward the new block's center -- keep walking.
            ctx.walk(WalkDirection::Backward);
        }
        return;
    }

    let edge = at_edge_toward(ctx.position, start, face, threshold);
    if !edge {
        // Walking phase: continuous backward walk, not sneaking. This is
        // the "brief sneak pulse only at the edge" behavior -- sneaking is
        // deliberately *not* engaged here.
        ctx.walk(WalkDirection::Backward);
        return;
    }

    // Raycast validation: a slab support with no valid clickable geometry
    // right now (e.g. replaced by something with no hitbox between ticks)
    // must not be placed against blind. Hold position -- same carve-out as
    // a missing scaffold item below -- rather than continue the bridge;
    // this is re-checked every tick, so it clears itself the instant the
    // support is solid again, and `timeout_movement` cancels the route
    // outright if it never does.
    if is_slab_support && slab_click_y.is_none() {
        ctx.mark_blocked_on_scaffold();
        ctx.sneak();
        ctx.walk(WalkDirection::None);
        return;
    }

    // Edge phase: brief sneak pulse to place the next block.
    let Some(item_id) = policy.scaffold_item.clone() else {
        // No material to place with -- hold position and keep sneaking
        // rather than walk backward into the gap. `timeout_movement` will
        // notice `mark_blocked_on_scaffold` and cancel the route rather
        // than let this stall forever.
        ctx.mark_blocked_on_scaffold();
        ctx.sneak();
        ctx.walk(WalkDirection::None);
        return;
    };
    ctx.sneak();
    ctx.walk(WalkDirection::None);
    if !ctx.physics.on_ground() {
        // Same airborne guard as the classic technique above -- see that
        // branch's comment. Fast bridging reaches this point at normal
        // walking speed, so it's just as reachable mid-knockback as the
        // sneak-the-whole-way technique is.
        return;
    }
    ctx.place(reference, face, scaffold_predicate(item_id));
    if let Some(click_y) = slab_click_y {
        // `ExecuteCtx::place` re-aims at `reference.center()` for the
        // general case, which would put the crosshair back in a top slab's
        // empty lower half for this one tick -- restore the slab-specific
        // aim point so the camera doesn't jump (no forward snapping, no
        // per-block oscillation).
        ctx.look_at_exact(slab_look_behind_point(face, ctx.position, click_y));
    }
}

// ---------------------------------------------------------------------
// Staircase up: extend a floor that's one block short of a landing by
// placing on top of it, then step/jump up onto the new block. Fires only
// when `ascend_move` can't (no floor to land on) but the current-height
// floor continues far enough to place a step on top of -- e.g. climbing a
// ledge or natural terrace that's missing its last block.
//
// This intentionally does not attempt to build a step over open air with no
// solid reference at all; that shape is instead reached by the search
// composing `pillar_up_move` with ordinary walk/mine moves, which is both
// simpler and does not need a second, harder-to-verify placement technique.
// ---------------------------------------------------------------------

fn staircase_up_move(ctx: &mut MovesCtx, pos: RelBlockPos) {
    let policy = policy_of(ctx);
    if !policy.allow_staircase_building || !policy.can_place() {
        return;
    }
    if !ctx.world.is_block_solid(pos.down(1)) {
        return;
    }

    let break_cost_1 = ctx
        .world
        .cost_for_breaking_block(pos.up(2), ctx.mining_cache);
    if break_cost_1 == f32::INFINITY {
        return;
    }
    let base_cost = f32::max(WALK_ONE_BLOCK_COST, *JUMP_ONE_BLOCK_COST)
        + JUMP_PENALTY
        + break_cost_1
        + PLACE_COST as f32;

    for dir in CardinalDirection::iter() {
        let offset = RelBlockPos::new(dir.x(), 1, dir.z());
        let target = pos + offset;

        if ctx.world.cost_for_standing(target, ctx.mining_cache) != f32::INFINITY {
            // ascend_move already covers this.
            continue;
        }
        let step_reference = pos.down(1) + RelBlockPos::new(dir.x(), 0, dir.z());
        if !ctx.world.is_block_solid(step_reference) {
            continue;
        }
        if !ctx.world.is_block_passable(target) || !ctx.world.is_block_passable(target.up(1)) {
            continue;
        }

        let cost = base_cost + turn_penalty(pos, dir.x(), dir.z());
        ctx.edges.push(Edge {
            movement: astar::Movement {
                target,
                data: MoveData {
                    execute: &execute_staircase_up_move,
                    is_reached: &vertical_is_reached,
                },
            },
            cost,
        });
    }
}

fn execute_staircase_up_move(mut ctx: ExecuteCtx) {
    ctx.clear_blocked_on_scaffold();
    let target = ctx.target;
    let start = ctx.start;

    // Sneak for the whole staircase sequence (mining the step, approaching,
    // and placing), same reasoning as pillaring/bridging: keeps the bot
    // anchored at the edge of its current block instead of walking off it.
    ctx.sneak();
    ctx.jump_if_in_water();

    if ctx.mine_while_at_start(target.up(1)) {
        return;
    }
    if ctx.mine_while_at_start(target) {
        return;
    }

    let step_floor = target.down(1);
    if is_block_state_passable(ctx.get_block_state(step_floor)) {
        let policy = ctx.custom::<PathfindingPolicy>();
        let Some(item_id) = policy.scaffold_item else {
            ctx.mark_blocked_on_scaffold();
            return;
        };
        let face = horizontal_face(target.x - start.x, target.z - start.z);
        let reference = start.down(1).offset_with_direction(face);
        // Look down at the top face actually being clicked, not level with
        // the bot's own eyes (see `face_center`'s doc comment).
        ctx.look_at_exact(face_center(reference, Direction::Up));
        if !ctx.physics.on_ground() {
            // Same airborne guard as the bridge moves above -- see
            // `execute_classic_bridge_move`'s comment.
            return;
        }
        ctx.place(reference, Direction::Up, scaffold_predicate(item_id));
        return;
    }
    ctx.clear_placing();

    let target_center = target.center();
    ctx.look_at(target_center);
    ctx.walk(WalkDirection::Forward);

    if player_pos_to_block_pos(ctx.position) == start && target.y as f64 - ctx.position.y > 0.5 {
        ctx.jump();
    }
}

#[cfg(test)]
mod tests {
    use azalea_block::blocks::CobblestoneSlab;
    use azalea_registry::builtin::BlockKind;

    use super::*;

    fn top_slab_state() -> BlockState {
        CobblestoneSlab {
            kind: SlabKind::Top,
            waterlogged: false,
        }
        .into()
    }

    fn bottom_slab_state() -> BlockState {
        CobblestoneSlab {
            kind: SlabKind::Bottom,
            waterlogged: false,
        }
        .into()
    }

    fn double_slab_state() -> BlockState {
        CobblestoneSlab {
            kind: SlabKind::Double,
            waterlogged: false,
        }
        .into()
    }

    #[test]
    fn test_slab_hitbox_y_range_matches_kind() {
        assert_eq!(slab_hitbox_y_range(SlabKind::Bottom), (0.0, 0.5));
        assert_eq!(slab_hitbox_y_range(SlabKind::Top), (0.5, 1.0));
        assert_eq!(slab_hitbox_y_range(SlabKind::Double), (0.0, 1.0));
    }

    // "top slab edge targeting" -- the aim point must land inside the top
    // slab's solid upper half, close to its top edge, not at a full
    // block's vertical center (which is exactly the empty lower half).
    #[test]
    fn test_slab_click_height_targets_top_edge_of_top_slab() {
        let reference = BlockPos::new(5, 70, 5);
        let click_y = slab_click_height(reference, top_slab_state())
            .expect("top slab should have a valid click height");
        let fraction = click_y - reference.y as f64;
        assert!(
            (0.5..1.0).contains(&fraction),
            "click height {fraction} should be within the top slab's hitbox"
        );
        assert!(
            fraction > 0.9,
            "click height {fraction} should be close to the top edge"
        );
    }

    #[test]
    fn test_slab_click_height_targets_lower_half_of_bottom_slab() {
        let reference = BlockPos::new(5, 70, 5);
        let click_y = slab_click_height(reference, bottom_slab_state())
            .expect("bottom slab should have a valid click height");
        let fraction = click_y - reference.y as f64;
        assert!(
            (0.0..0.5).contains(&fraction),
            "click height {fraction} should be within the bottom slab's hitbox"
        );
    }

    #[test]
    fn test_slab_click_height_double_slab_behaves_like_full_block() {
        let reference = BlockPos::new(5, 70, 5);
        let click_y = slab_click_height(reference, double_slab_state())
            .expect("double slab should have a valid click height");
        let fraction = click_y - reference.y as f64;
        assert!((0.0..1.0).contains(&fraction));
    }

    // "raycast validation on slabs" -- a support block with no slab
    // property at all (an ordinary full block) must not get a slab-shaped
    // aim point; callers fall back to `look_behind_point` for it instead.
    #[test]
    fn test_slab_click_height_non_slab_returns_none() {
        let reference = BlockPos::new(5, 70, 5);
        let full_block = BlockState::from(BlockKind::Stone);
        assert_eq!(slab_click_height(reference, full_block), None);
    }

    #[test]
    fn test_find_inset_click_offset_picks_first_valid_inset_for_real_slabs() {
        // A real slab's hitbox (0.5 tall) is much wider than the default
        // inset, so this should succeed without ever needing to retry --
        // the click lands exactly `SLAB_AIM_INSET` below `hi`.
        let click = find_inset_click_offset(0.5, 1.0).expect("should find a valid offset");
        assert_eq!(click, 1.0 - SLAB_AIM_INSET);
    }

    // "placement retry" -- the default inset overshoots past `lo` here
    // (1.0 - 0.05 = 0.95, below 0.97), so this only succeeds if the retry
    // (shrinking the inset toward the true edge) actually ran.
    #[test]
    fn test_find_inset_click_offset_retries_on_narrow_range() {
        let click = find_inset_click_offset(0.97, 1.0).expect("should recover via a smaller inset");
        assert!(
            click > 1.0 - SLAB_AIM_INSET,
            "click {click} should have shrunk from the default inset"
        );
        assert!(click > 0.97 && click < 1.0);
    }

    // "placement retry" (exhaustion case) -- a hitbox thinner than any
    // inset reachable within `SLAB_AIM_MAX_ATTEMPTS` retries must not
    // place blind; it has to report failure so the caller holds position
    // instead (see `slab_click_height`'s doc comment).
    #[test]
    fn test_find_inset_click_offset_gives_up_on_impossibly_narrow_range() {
        assert_eq!(find_inset_click_offset(0.4999, 0.5), None);
    }

    // Camera requirement: pitch stays close to the requested ~78 degrees
    // downward for slab bridging, and -- since the aim point is anchored
    // to live position rather than a fixed block -- comes out to *exactly*
    // the same value no matter where within the segment the bot currently
    // is (checked at two very different live positions below).
    #[test]
    fn test_slab_look_behind_point_pitch_is_steep_and_stable() {
        let reference = BlockPos::new(5, 70, 5);
        let click_y = slab_click_height(reference, top_slab_state()).unwrap();

        let pitch_at = |live_position: Vec3| {
            let target = slab_look_behind_point(Direction::East, live_position, click_y);
            let eye = live_position.up(1.62);
            let delta = target - eye;
            let horizontal_distance = (delta.x * delta.x + delta.z * delta.z).sqrt();
            (-delta.y).atan2(horizontal_distance).to_degrees()
        };

        let near_start = pitch_at(Vec3 {
            x: 5.1,
            y: 71.0,
            z: 5.5,
        });
        let near_far_edge = pitch_at(Vec3 {
            x: 5.9,
            y: 71.0,
            z: 5.5,
        });

        for pitch_degrees in [near_start, near_far_edge] {
            assert!(
                (65.0..80.0).contains(&pitch_degrees),
                "pitch {pitch_degrees} should be steeply downward, close to 78 degrees"
            );
        }
        assert!(
            (near_start - near_far_edge).abs() < 0.01,
            "pitch should not drift across a segment: {near_start} vs {near_far_edge}"
        );
    }

    // Camera requirement: no per-block oscillation -- the horizontal
    // offset must track live position exactly, never a fixed block anchor
    // (see `slab_look_behind_point`'s doc comment for why a fixed anchor
    // caused a per-block pitch sawtooth).
    #[test]
    fn test_slab_look_behind_point_locks_to_live_position() {
        let click_y = 70.95;
        let live_position = Vec3 {
            x: 5.73,
            y: 71.0,
            z: 5.5,
        };
        let target = slab_look_behind_point(Direction::East, live_position, click_y);
        assert_eq!(target.z, live_position.z);
        assert_eq!(target.x, live_position.x - SLAB_LOOK_BEHIND_OFFSET);
    }
}
