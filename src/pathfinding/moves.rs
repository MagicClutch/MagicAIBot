//! Successor generation: from one standing position, which cells can the
//! bot legally reach in one movement, and what does each cost. Pure -- takes
//! an already-sampled [`TerrainGrid`] and a [`CostProfile`], touches nothing
//! else.
//!
//! This is the movement *model*: it decides what the bot is physically
//! capable of. It deliberately models slightly less than Azalea's own
//! executor can actually do (which also parkours and pillars): the planner's
//! job is to produce a route the executor can definitely follow, so being
//! conservative here costs a little route quality and buys reliability. The
//! reverse -- planning a jump the executor can't make -- would strand the
//! bot mid-segment and force a replan every time.

use crate::{
    minecraft::world_state::BlockPosition,
    pathfinding::{
        cost::{CostProfile, MoveKind},
        grid::TerrainGrid,
        terrain::TerrainClass,
    },
};

/// Height of the bot's body in whole blocks. Vanilla players are 1.8 blocks
/// tall, which occupies two block cells.
pub const BODY_HEIGHT: i32 = 2;

/// The four orthogonal horizontal directions, and then the four diagonals.
/// Ordered so orthogonal moves are generated first, which -- with equal f
/// scores -- makes A* prefer the simpler move.
const HORIZONTAL: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// What the bot is allowed to do while pathing, plus the world facts the
/// move generator can't read off the terrain grid.
#[derive(Clone, Debug)]
pub struct MovePolicy {
    /// Whether mining through blocks may be considered at all. When false,
    /// no [`MoveKind::Break`] successor is ever generated, whatever the cost
    /// profile says.
    pub allow_breaking: bool,
    /// Deepest drop the search will plan. Distinct from
    /// `CostProfile::max_safe_drop` (which prices a drop): this one forbids
    /// it outright, so a 60-block shaft is never a route however desperate
    /// the search gets.
    pub max_drop: i32,
    /// Whether the search may plan across a gap by placing blocks. Mirrors
    /// what the executor is actually permitted to build
    /// (`[vertical_navigation] allow_bridging`); planning a bridge the
    /// executor won't build would strand the bot at the edge.
    pub allow_bridging: bool,
    /// Whether swimming is allowed. Off for a bot with no way out of deep
    /// water is the safe setting, but the default is on -- rivers are
    /// unavoidable in practice.
    pub allow_swimming: bool,
    /// Whether the search may plan straight through lava. Off by default and
    /// independent of `CostProfile::lava_avoidance`: the cost makes lava
    /// unattractive, this makes it impossible.
    pub allow_lava: bool,
    /// Positions of known entity hazards (hostile mobs), with a radius each,
    /// as supplied by the caller from live world state. Cells within a
    /// radius take `CostProfile::entity_hazard`.
    pub entity_hazards: Vec<(BlockPosition, f64)>,
}

impl Default for MovePolicy {
    fn default() -> Self {
        Self {
            allow_breaking: true,
            allow_bridging: true,
            max_drop: 8,
            allow_swimming: true,
            allow_lava: false,
            entity_hazards: Vec::new(),
        }
    }
}

/// How far from a hostile mob of this kind counts as dangerous, in blocks,
/// or `None` for anything the bot has no reason to route around.
///
/// Radii rather than one flat number because the threat genuinely differs:
/// a creeper's whole danger is that it reaches you, so give it room; a
/// zombie only matters if the route walks into it. Passive mobs are absent
/// entirely -- routing around cows would make every farm impassable.
///
/// Not exhaustive, and deliberately cheap to extend: an unlisted mob simply
/// costs nothing, which is the same behavior as before this existed.
#[must_use]
pub fn hazard_radius(entity_type: &str) -> Option<f64> {
    match entity_type {
        "minecraft:creeper" => Some(8.0),
        "minecraft:warden" => Some(16.0),
        "minecraft:ravager" | "minecraft:ravenger" => Some(10.0),
        "minecraft:blaze" | "minecraft:ghast" | "minecraft:wither_skeleton" => Some(8.0),
        "minecraft:zombie"
        | "minecraft:husk"
        | "minecraft:drowned"
        | "minecraft:skeleton"
        | "minecraft:stray"
        | "minecraft:spider"
        | "minecraft:cave_spider"
        | "minecraft:witch"
        | "minecraft:pillager"
        | "minecraft:vindicator"
        | "minecraft:enderman"
        | "minecraft:piglin_brute"
        | "minecraft:hoglin"
        | "minecraft:zoglin"
        | "minecraft:magma_cube"
        | "minecraft:slime"
        | "minecraft:phantom"
        | "minecraft:evoker" => Some(5.0),
        _ => None,
    }
}

/// One generated successor: where it lands, how it got there, what it cost.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Move {
    pub destination: BlockPosition,
    pub kind: MoveKind,
    pub cost: f64,
}

/// Every legal move out of `from` (a standing or swimming position).
///
/// Pushes into `out` rather than allocating a `Vec` per expansion: A* calls
/// this once per node it expands, tens of thousands of times per search, and
/// reusing one buffer keeps the whole search allocation-free after warm-up.
pub fn successors(
    grid: &TerrainGrid,
    profile: &CostProfile,
    policy: &MovePolicy,
    from: BlockPosition,
    out: &mut Vec<Move>,
) {
    out.clear();
    for (index, (dx, dz)) in HORIZONTAL.iter().enumerate() {
        let diagonal = index >= 4;
        if diagonal && !diagonal_corners_clear(grid, from, *dx, *dz) {
            continue;
        }
        horizontal_move(grid, profile, policy, from, *dx, *dz, diagonal, out);
    }
    vertical_water_moves(grid, profile, policy, from, out);
    climb_moves(grid, profile, policy, from, out);
}

/// A diagonal step is only legal when both orthogonal cells it cuts past are
/// themselves clear -- vanilla physics won't squeeze a player through the
/// corner between two blocks, and a route that assumes otherwise wedges the
/// bot against the corner forever.
fn diagonal_corners_clear(grid: &TerrainGrid, from: BlockPosition, dx: i32, dz: i32) -> bool {
    let along_x = BlockPosition {
        x: from.x + dx,
        y: from.y,
        z: from.z,
    };
    let along_z = BlockPosition {
        x: from.x,
        y: from.y,
        z: from.z + dz,
    };
    grid.body_fits(along_x, BODY_HEIGHT) && grid.body_fits(along_z, BODY_HEIGHT)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one move generator over a fixed set of terrain inputs; splitting \
              it would only move the same arguments one call deeper"
)]
fn horizontal_move(
    grid: &TerrainGrid,
    profile: &CostProfile,
    policy: &MovePolicy,
    from: BlockPosition,
    dx: i32,
    dz: i32,
    diagonal: bool,
    out: &mut Vec<Move>,
) {
    let level = BlockPosition {
        x: from.x + dx,
        y: from.y,
        z: from.z + dz,
    };
    let flat_kind = if diagonal {
        MoveKind::Diagonal
    } else {
        MoveKind::Walk
    };

    // Same level: the ordinary case.
    if grid.body_fits(level, BODY_HEIGHT) {
        if grid.floor_below(level).supports_standing() {
            push_move(grid, profile, policy, level, flat_kind, out);
        } else if policy.allow_swimming && grid.get(level) == TerrainClass::Water {
            push_move(grid, profile, policy, level, MoveKind::Swim, out);
        } else {
            // Nothing to stand on: either bridge across the gap or fall into
            // it. Both are offered, and the cost model picks -- bridging is
            // expensive but keeps altitude, dropping is cheap but may not be
            // recoverable.
            if policy.allow_bridging && grid.floor_below(level).passable() {
                push_move(
                    grid,
                    profile,
                    policy,
                    level,
                    MoveKind::Bridge { blocks: 1 },
                    out,
                );
            }
            drop_move(grid, profile, policy, level, out);
        }
        // A cell the body fits in can't also be climbed into or broken
        // through, so nothing below applies.
        return;
    }

    // Blocked at this level: try stepping up one block.
    let up = BlockPosition {
        x: level.x,
        y: level.y + 1,
        z: level.z,
    };
    let headroom = BlockPosition {
        x: from.x,
        y: from.y + BODY_HEIGHT,
        z: from.z,
    };
    let can_jump = grid.get(headroom).known() && grid.get(headroom).passable();
    if can_jump && grid.standable(up, BODY_HEIGHT) && !diagonal {
        push_move(grid, profile, policy, up, MoveKind::JumpUp, out);
        return;
    }

    // Still blocked: consider mining through, but never diagonally (mining
    // out a diagonal corner needs two blocks removed *and* leaves the bot
    // walking a corner it can still get wedged on).
    if !policy.allow_breaking || diagonal {
        return;
    }
    let breakable_cells = (0..BODY_HEIGHT)
        .map(|offset| {
            grid.get(BlockPosition {
                x: level.x,
                y: level.y + offset,
                z: level.z,
            })
        })
        .collect::<Vec<_>>();
    if breakable_cells.iter().any(|cell| !cell.known()) {
        return;
    }
    let to_break = breakable_cells
        .iter()
        .filter(|cell| cell.breakable())
        .count() as i32;
    let all_clearable = breakable_cells
        .iter()
        .all(|cell| cell.breakable() || cell.passable());
    if !all_clearable || to_break == 0 {
        return;
    }
    if !grid.floor_below(level).supports_standing() {
        // Mining into thin air just drops the bot into whatever is below;
        // let the plain drop/bridge cases own that instead of pricing it as
        // a break.
        return;
    }
    push_move(
        grid,
        profile,
        policy,
        level,
        MoveKind::Break { blocks: to_break },
        out,
    );
}

/// Falling into `landing_column` from above: scans down for the first
/// standable (or swimmable) cell within `policy.max_drop`.
fn drop_move(
    grid: &TerrainGrid,
    profile: &CostProfile,
    policy: &MovePolicy,
    landing_column: BlockPosition,
    out: &mut Vec<Move>,
) {
    for depth in 1..=policy.max_drop {
        let candidate = BlockPosition {
            x: landing_column.x,
            y: landing_column.y - depth,
            z: landing_column.z,
        };
        let cell = grid.get(candidate);
        if !cell.known() {
            return;
        }
        if policy.allow_swimming && cell == TerrainClass::Water {
            // Water breaks the fall entirely -- no fall-damage penalty.
            push_move(grid, profile, policy, candidate, MoveKind::Swim, out);
            return;
        }
        if !cell.passable() {
            // Hit something solid: the standable cell is the one above it.
            let landing = BlockPosition {
                x: candidate.x,
                y: candidate.y + 1,
                z: candidate.z,
            };
            if grid.standable(landing, BODY_HEIGHT) {
                push_move(
                    grid,
                    profile,
                    policy,
                    landing,
                    MoveKind::Drop { blocks: depth - 1 },
                    out,
                );
            }
            return;
        }
    }
}

/// Swimming up and down a water column, and climbing out onto a shore.
fn vertical_water_moves(
    grid: &TerrainGrid,
    profile: &CostProfile,
    policy: &MovePolicy,
    from: BlockPosition,
    out: &mut Vec<Move>,
) {
    if !policy.allow_swimming || grid.get(from) != TerrainClass::Water {
        return;
    }
    for direction in [1, -1] {
        let candidate = BlockPosition {
            x: from.x,
            y: from.y + direction,
            z: from.z,
        };
        if grid.swimmable(candidate, BODY_HEIGHT) || grid.standable(candidate, BODY_HEIGHT) {
            push_move(grid, profile, policy, candidate, MoveKind::Swim, out);
        }
    }
}

/// Climbing a ladder/vine column: up if the bot is standing in or directly
/// under one, down if the cell below is climbable. Modelled one block at a
/// time (rather than as a single multi-block climb) so the route can leave
/// the column at any height, which is what a ladder in a mineshaft is
/// usually for.
fn climb_moves(
    grid: &TerrainGrid,
    profile: &CostProfile,
    policy: &MovePolicy,
    from: BlockPosition,
    out: &mut Vec<Move>,
) {
    let above = BlockPosition {
        x: from.x,
        y: from.y + 1,
        z: from.z,
    };
    if grid.get(from).climbable() && grid.body_fits(above, BODY_HEIGHT) {
        push_move(
            grid,
            profile,
            policy,
            above,
            MoveKind::Climb { blocks: 1 },
            out,
        );
    }
    let below = BlockPosition {
        x: from.x,
        y: from.y - 1,
        z: from.z,
    };
    if grid.get(below).climbable() && grid.body_fits(below, BODY_HEIGHT) {
        push_move(
            grid,
            profile,
            policy,
            below,
            MoveKind::Climb { blocks: 1 },
            out,
        );
    }
}

/// Applies every per-cell penalty on top of a move's base cost and records
/// it -- the one place terrain, floor, proximity, and entity penalties are
/// combined, so no successor can accidentally skip one.
fn push_move(
    grid: &TerrainGrid,
    profile: &CostProfile,
    policy: &MovePolicy,
    destination: BlockPosition,
    kind: MoveKind,
    out: &mut Vec<Move>,
) {
    let destination_class = grid.get(destination);
    if destination_class.lethal() && !policy.allow_lava {
        return;
    }
    let floor = grid.floor_below(destination);
    if floor.lethal() && !policy.allow_lava {
        return;
    }
    let (hazards, lava) = adjacent_danger(grid, destination);
    let cost = profile.cost_of(kind)
        + profile.terrain_penalty(destination_class)
        + profile.floor_penalty(floor)
        + profile.proximity_penalty(hazards, lava)
        + profile.ledge_proximity * adjacent_ledges(grid, destination) as f64
        + entity_penalty(profile, policy, destination);
    out.push(Move {
        destination,
        kind,
        cost,
    });
}

/// Counts hazardous cells horizontally adjacent to `position` (at foot
/// level), which is what the proximity penalty prices. Horizontal only: a
/// hazard above or below is either the floor (already priced) or out of
/// reach.
fn adjacent_danger(grid: &TerrainGrid, position: BlockPosition) -> (usize, usize) {
    let mut hazards = 0;
    let mut lava = 0;
    for (dx, dz) in HORIZONTAL.iter().take(4) {
        let cell = grid.get(BlockPosition {
            x: position.x + dx,
            y: position.y,
            z: position.z + dz,
        });
        if cell.lethal() {
            lava += 1;
        } else if cell.damaging() {
            hazards += 1;
        }
    }
    (hazards, lava)
}

/// Counts horizontally adjacent cells the bot could fall out of -- open air
/// with nothing standable under it. Walking a block in from the edge costs
/// almost nothing extra and removes a whole class of "physics slid me off
/// the cliff I was walking along" failures.
fn adjacent_ledges(grid: &TerrainGrid, position: BlockPosition) -> usize {
    HORIZONTAL
        .iter()
        .take(4)
        .filter(|(dx, dz)| {
            let neighbor = BlockPosition {
                x: position.x + dx,
                y: position.y,
                z: position.z + dz,
            };
            let floor = grid.floor_below(neighbor);
            grid.get(neighbor).passable() && floor.known() && !floor.supports_standing()
        })
        .count()
}

fn entity_penalty(profile: &CostProfile, policy: &MovePolicy, position: BlockPosition) -> f64 {
    policy
        .entity_hazards
        .iter()
        .filter(|(hazard, radius)| {
            crate::pathfinding::grid::block_distance(*hazard, position) <= *radius
        })
        .count() as f64
        * profile.entity_hazard
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathfinding::grid::GridBounds;

    fn position(x: i32, y: i32, z: i32) -> BlockPosition {
        BlockPosition { x, y, z }
    }

    /// A 32x32 flat stone plain with its surface at y=63, air above.
    fn plain() -> TerrainGrid {
        let bounds = GridBounds {
            min: position(-16, 55, -16),
            max: position(16, 75, 16),
        };
        let mut grid = TerrainGrid::empty(bounds);
        for x in -16..16 {
            for z in -16..16 {
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

    fn moves_from(grid: &TerrainGrid, policy: &MovePolicy, from: BlockPosition) -> Vec<Move> {
        let mut out = Vec::new();
        successors(grid, &CostProfile::default(), policy, from, &mut out);
        out
    }

    fn find(moves: &[Move], destination: BlockPosition) -> Option<Move> {
        moves.iter().copied().find(|m| m.destination == destination)
    }

    #[test]
    fn flat_ground_yields_all_eight_neighbors() {
        let grid = plain();
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        assert_eq!(moves.len(), 8);
        assert!(find(&moves, position(1, 64, 0)).is_some());
        assert!(find(&moves, position(1, 64, 1)).is_some());
    }

    #[test]
    fn a_diagonal_is_refused_when_a_corner_is_blocked() {
        let mut grid = plain();
        grid.set(position(1, 64, 0), TerrainClass::Solid);
        grid.set(position(1, 65, 0), TerrainClass::Solid);
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        assert!(
            find(&moves, position(1, 64, 1)).is_none(),
            "cutting the corner past a solid block is not a legal step"
        );
        assert!(find(&moves, position(0, 64, 1)).is_some());
    }

    #[test]
    fn a_one_block_step_up_is_a_jump() {
        let mut grid = plain();
        grid.set(position(1, 64, 0), TerrainClass::Solid);
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        let step = find(&moves, position(1, 65, 0)).expect("should step up onto the ledge");
        assert_eq!(step.kind, MoveKind::JumpUp);
    }

    #[test]
    fn a_step_up_is_refused_with_no_headroom() {
        let mut grid = plain();
        grid.set(position(1, 64, 0), TerrainClass::Solid);
        grid.set(position(0, 66, 0), TerrainClass::Solid);
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        assert!(find(&moves, position(1, 65, 0)).is_none());
    }

    #[test]
    fn walking_off_a_ledge_becomes_a_drop_to_the_floor_below() {
        let mut grid = plain();
        for y in 60..=63 {
            grid.set(position(1, y, 0), TerrainClass::Air);
        }
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        let drop = find(&moves, position(1, 60, 0)).expect("should drop into the hole");
        assert_eq!(drop.kind, MoveKind::Drop { blocks: 4 });
    }

    #[test]
    fn a_drop_deeper_than_the_policy_allows_is_not_generated() {
        let mut grid = plain();
        for y in 55..=63 {
            grid.set(position(1, y, 0), TerrainClass::Air);
        }
        // Bridging off as well, so this tests the drop limit alone: with it
        // on, the bot would simply bridge across the hole instead.
        let policy = MovePolicy {
            max_drop: 2,
            allow_bridging: false,
            ..MovePolicy::default()
        };
        let moves = moves_from(&grid, &policy, position(0, 64, 0));
        assert!(
            moves
                .iter()
                .all(|m| m.destination.x != 1 || m.destination.z != 0)
        );
    }

    #[test]
    fn a_deep_but_allowed_drop_costs_more_than_a_shallow_one() {
        let mut grid = plain();
        for y in 60..=63 {
            grid.set(position(1, y, 0), TerrainClass::Air);
        }
        grid.set(position(-1, 63, 0), TerrainClass::Air);
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        let deep = find(&moves, position(1, 60, 0)).unwrap();
        let shallow = find(&moves, position(-1, 63, 0)).unwrap();
        assert!(deep.cost > shallow.cost);
    }

    #[test]
    fn unknown_terrain_is_never_a_successor() {
        let mut grid = plain();
        grid.set(position(1, 64, 0), TerrainClass::Unknown);
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        assert!(find(&moves, position(1, 64, 0)).is_none());
    }

    #[test]
    fn the_grid_edge_is_a_frontier_not_a_wall_to_walk_through() {
        let grid = plain();
        // At the very edge, the cells beyond are outside the sampled bounds
        // and therefore Unknown -- no successor should lead out of the grid.
        let moves = moves_from(&grid, &MovePolicy::default(), position(15, 64, 0));
        assert!(moves.iter().all(|m| m.destination.x <= 15));
    }

    #[test]
    fn mining_through_a_wall_is_offered_only_when_breaking_is_allowed() {
        let mut grid = plain();
        grid.set(position(1, 64, 0), TerrainClass::Solid);
        grid.set(position(1, 65, 0), TerrainClass::Solid);
        grid.set(position(1, 66, 0), TerrainClass::Solid);
        let allowed = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        let through = find(&allowed, position(1, 64, 0)).expect("should offer to mine through");
        assert_eq!(through.kind, MoveKind::Break { blocks: 2 });

        let policy = MovePolicy {
            allow_breaking: false,
            ..MovePolicy::default()
        };
        let refused = moves_from(&grid, &policy, position(0, 64, 0));
        assert!(find(&refused, position(1, 64, 0)).is_none());
    }

    #[test]
    fn bedrock_is_never_mined_through() {
        let mut grid = plain();
        grid.set(position(1, 64, 0), TerrainClass::Unbreakable);
        grid.set(position(1, 65, 0), TerrainClass::Unbreakable);
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        assert!(find(&moves, position(1, 64, 0)).is_none());
    }

    #[test]
    fn lava_is_refused_outright_by_default_and_priced_when_allowed() {
        let mut grid = plain();
        grid.set(position(1, 64, 0), TerrainClass::Lava);
        let refused = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        assert!(find(&refused, position(1, 64, 0)).is_none());

        let policy = MovePolicy {
            allow_lava: true,
            ..MovePolicy::default()
        };
        let allowed = moves_from(&grid, &policy, position(0, 64, 0));
        let into_lava = find(&allowed, position(1, 64, 0)).expect("allowed, but expensive");
        assert!(into_lava.cost > CostProfile::default().lava_avoidance);
    }

    #[test]
    fn standing_on_lava_is_refused_the_same_as_standing_in_it() {
        let mut grid = plain();
        grid.set(position(1, 63, 0), TerrainClass::Lava);
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        assert!(find(&moves, position(1, 64, 0)).is_none());
    }

    #[test]
    fn walking_beside_lava_costs_more_than_walking_in_the_open() {
        let mut grid = plain();
        grid.set(position(2, 64, 0), TerrainClass::Lava);
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        let beside = find(&moves, position(1, 64, 0)).unwrap();
        let away = find(&moves, position(-1, 64, 0)).unwrap();
        assert!(beside.cost > away.cost);
    }

    #[test]
    fn a_hazard_cell_is_entered_only_at_a_steep_premium() {
        let mut grid = plain();
        grid.set(position(1, 64, 0), TerrainClass::Hazard);
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        let into_hazard = find(&moves, position(1, 64, 0)).unwrap();
        let clear = find(&moves, position(-1, 64, 0)).unwrap();
        assert!(into_hazard.cost > clear.cost + CostProfile::default().hazard - 1e-9);
    }

    #[test]
    fn water_is_swum_through_rather_than_walked_and_can_be_forbidden() {
        let mut grid = plain();
        grid.set(position(1, 63, 0), TerrainClass::Water);
        grid.set(position(1, 64, 0), TerrainClass::Water);
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        let swim = find(&moves, position(1, 64, 0)).expect("should swim in");
        assert_eq!(swim.kind, MoveKind::Swim);

        let policy = MovePolicy {
            allow_swimming: false,
            ..MovePolicy::default()
        };
        let refused = moves_from(&grid, &policy, position(1, 64, 0));
        assert!(refused.iter().all(|m| m.kind != MoveKind::Swim));
    }

    #[test]
    fn swimming_can_move_up_and_down_a_water_column() {
        let mut grid = plain();
        for y in 60..=66 {
            grid.set(position(1, y, 0), TerrainClass::Water);
        }
        let moves = moves_from(&grid, &MovePolicy::default(), position(1, 63, 0));
        assert!(find(&moves, position(1, 64, 0)).is_some());
        assert!(find(&moves, position(1, 62, 0)).is_some());
    }

    #[test]
    fn falling_into_water_is_not_penalized_as_a_fall() {
        let mut grid = plain();
        for y in 57..=63 {
            grid.set(position(1, y, 0), TerrainClass::Air);
        }
        grid.set(position(1, 57, 0), TerrainClass::Water);
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        let splash = find(&moves, position(1, 57, 0)).expect("should drop into the water");
        assert_eq!(splash.kind, MoveKind::Swim);
        assert!(splash.cost < CostProfile::default().fall_damage_penalty);
    }

    #[test]
    fn a_ladder_can_be_climbed_up_and_down() {
        let mut grid = plain();
        for y in 64..=68 {
            grid.set(position(1, y, 0), TerrainClass::Climbable);
        }
        let moves = moves_from(&grid, &MovePolicy::default(), position(1, 65, 0));
        let up = find(&moves, position(1, 66, 0)).expect("climb up the ladder");
        let down = find(&moves, position(1, 64, 0)).expect("climb down the ladder");
        assert_eq!(up.kind, MoveKind::Climb { blocks: 1 });
        assert_eq!(down.kind, MoveKind::Climb { blocks: 1 });
    }

    #[test]
    fn nothing_is_climbed_without_a_ladder() {
        let grid = plain();
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        assert!(
            moves
                .iter()
                .all(|m| !matches!(m.kind, MoveKind::Climb { .. }))
        );
    }

    #[test]
    fn a_gap_can_be_bridged_and_bridging_can_be_forbidden() {
        let mut grid = plain();
        // A one-block-wide chasm at x=1 with a floor far below.
        for y in 56..=63 {
            grid.set(position(1, y, 0), TerrainClass::Air);
        }
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        let bridge = find(&moves, position(1, 64, 0)).expect("bridge across the gap");
        assert_eq!(bridge.kind, MoveKind::Bridge { blocks: 1 });

        let policy = MovePolicy {
            allow_bridging: false,
            ..MovePolicy::default()
        };
        let refused = moves_from(&grid, &policy, position(0, 64, 0));
        assert!(
            refused
                .iter()
                .all(|m| !matches!(m.kind, MoveKind::Bridge { .. }))
        );
    }

    #[test]
    fn bridging_costs_more_than_walking_around_the_gap() {
        let mut grid = plain();
        for y in 56..=63 {
            grid.set(position(1, y, 0), TerrainClass::Air);
        }
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        let bridge = find(&moves, position(1, 64, 0)).unwrap();
        let around = find(&moves, position(0, 64, 1)).unwrap();
        assert!(bridge.cost > around.cost);
    }

    #[test]
    fn walking_along_a_cliff_edge_costs_more_than_walking_inland() {
        let mut grid = plain();
        // Carve away everything at z >= 2, making z=1 a cliff edge.
        for x in -32..32 {
            for z in 2..32 {
                for y in 55..=63 {
                    grid.set(position(x, y, z), TerrainClass::Air);
                }
            }
        }
        let moves = moves_from(&grid, &MovePolicy::default(), position(0, 64, 0));
        let along_edge = find(&moves, position(0, 64, 1)).unwrap();
        let inland = find(&moves, position(0, 64, -1)).unwrap();
        assert!(
            along_edge.cost > inland.cost,
            "the cell beside the drop should be the more expensive one"
        );
    }

    #[test]
    fn hostile_mobs_have_a_danger_radius_and_passive_ones_do_not() {
        assert_eq!(hazard_radius("minecraft:creeper"), Some(8.0));
        assert!(
            hazard_radius("minecraft:creeper") > hazard_radius("minecraft:zombie"),
            "a creeper deserves more room than a zombie"
        );
        assert_eq!(hazard_radius("minecraft:cow"), None);
        assert_eq!(hazard_radius("minecraft:item"), None);
        assert_eq!(hazard_radius("minecraft:player"), None);
    }

    #[test]
    fn entity_hazards_make_nearby_cells_more_expensive() {
        let grid = plain();
        let policy = MovePolicy {
            entity_hazards: vec![(position(3, 64, 0), 3.0)],
            ..MovePolicy::default()
        };
        let moves = moves_from(&grid, &policy, position(0, 64, 0));
        let toward = find(&moves, position(1, 64, 0)).unwrap();
        let away = find(&moves, position(-1, 64, 0)).unwrap();
        assert!(toward.cost > away.cost);
        assert!((toward.cost - away.cost - CostProfile::default().entity_hazard).abs() < 1e-9);
    }
}
