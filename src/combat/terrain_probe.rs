//! Turns a small sample of the blocks around the bot into the avoidance
//! vector and stop flags the combat movement controller consumes. Pure --
//! `crate::combat::executor` takes the sample, this decides what it means.
//!
//! # Deliberately not pathfinding
//!
//! A fight cannot afford a search. This looks at a 7x7 column around the
//! bot, scores the cells it would rather not be in, and returns a direction
//! to lean away from. It has no memory, no plan, and no idea what is more
//! than three blocks away -- which is the right trade: local avoidance
//! that runs every tick beats a correct route that arrives late, and
//! `crate::pathfinding` is still there for actually getting somewhere.
//!
//! It reuses `crate::pathfinding::terrain`'s block classification so combat
//! and navigation cannot disagree about what counts as lava.

use crate::{
    combat::movement::{LocalHazards, Vec2},
    minecraft::world_state::BlockPosition,
    pathfinding::{grid::TerrainGrid, moves::BODY_HEIGHT, terrain::TerrainClass},
};

/// How far from the bot a hazard still exerts a push, in blocks. Beyond
/// this it is not this tick's problem.
const INFLUENCE: f64 = 3.0;

/// Weight of a cell the bot must not enter (lava, fire, a drop) relative to
/// one that is merely in the way (a wall).
const LETHAL_WEIGHT: f64 = 3.0;

/// How far ahead the "am I about to walk off something" check looks.
///
/// Two samples rather than one: the near probe catches what the bot is
/// about to step into, the far one gives enough warning to stop before a
/// ledge at a sprint (about six ticks at 5.6 blocks a second). A single
/// probe at either distance misses one of the two cases -- a near-only
/// check walks off cliffs, a far-only check ignores the block right in
/// front of it.
const LOOKAHEAD_NEAR: f64 = 1.0;
const LOOKAHEAD_FAR: f64 = 1.8;

/// Drop, in blocks, that counts as a cliff worth stopping for rather than a
/// step worth taking.
const CLIFF_DEPTH: i32 = 4;

/// Evaluates the sampled terrain around `feet`.
///
/// `heading` is the direction the bot is currently steering, used to decide
/// whether the thing in front of it is actually in its way. A zero heading
/// (a bot that hasn't moved yet) yields no lookahead checks, only the
/// omnidirectional push.
#[must_use]
pub fn evaluate(
    grid: &TerrainGrid,
    feet: BlockPosition,
    bot: Vec2,
    target: Vec2,
    heading: Vec2,
) -> LocalHazards {
    let mut push = Vec2::zero();
    let bounds = grid.bounds();
    for x in bounds.min.x..bounds.max.x {
        for z in bounds.min.z..bounds.max.z {
            let column = BlockPosition { x, y: feet.y, z };
            let Some(weight) = column_weight(grid, column) else {
                continue;
            };
            let offset = Vec2::new(f64::from(x) + 0.5 - bot.x, f64::from(z) + 0.5 - bot.z);
            let distance = offset.length();
            if distance > INFLUENCE || distance < 1e-6 {
                continue;
            }
            // Linear falloff: a squared one collapses so fast that a wall
            // two blocks away exerts almost nothing, which is exactly the
            // range at which a fight actually needs to notice it.
            let urgency = (INFLUENCE - distance) / INFLUENCE;
            push = push.plus(offset.normalized().scaled(-weight * urgency));
        }
    }

    let direction = if heading.is_negligible() {
        target.minus(bot)
    } else {
        heading
    }
    .normalized();
    let probe = |reach: f64| {
        let ahead = bot.plus(direction.scaled(reach));
        BlockPosition {
            x: ahead.x.floor() as i32,
            y: feet.y,
            z: ahead.z.floor() as i32,
        }
    };
    let near = probe(LOOKAHEAD_NEAR);
    let far = probe(LOOKAHEAD_FAR);
    let looking = !direction.is_negligible();

    LocalHazards {
        avoidance: push.clamped(1.0),
        blocked_ahead: looking && (is_unsafe_ahead(grid, near) || is_unsafe_ahead(grid, far)),
        step_ahead: looking && (is_step_up(grid, near) || is_step_up(grid, far)),
    }
}

/// How much the bot wants to stay out of this column, or `None` if it is
/// unremarkable. Only the cells at body height matter for the push -- what
/// is underfoot is handled by the drop checks.
fn column_weight(grid: &TerrainGrid, column: BlockPosition) -> Option<f64> {
    let mut weight = 0.0;
    for offset in 0..BODY_HEIGHT {
        let cell = grid.get(BlockPosition {
            y: column.y + offset,
            ..column
        });
        weight += match cell {
            TerrainClass::Lava => LETHAL_WEIGHT,
            TerrainClass::Hazard => LETHAL_WEIGHT * 0.6,
            // A wall is worth leaning away from, but only just: fights
            // happen in corridors, and a bot that refuses to go near stone
            // cannot fight in one.
            TerrainClass::Solid | TerrainClass::Unbreakable => 0.35,
            _ => 0.0,
        };
    }
    // A hole in the floor pulls as hard as lava -- falling out of a fight is
    // as good as losing it.
    if is_cliff(grid, column) {
        weight += LETHAL_WEIGHT;
    }
    (weight > 0.0).then_some(weight)
}

/// Whether the floor under this column drops away far enough to hurt.
///
/// Unknown cells are *not* treated as a drop: at the edge of the sampled
/// region everything is unknown, and treating that as a cliff would make the
/// bot refuse to leave the middle of its own probe.
fn is_cliff(grid: &TerrainGrid, column: BlockPosition) -> bool {
    if !grid.get(column).passable() {
        return false;
    }
    (1..=CLIFF_DEPTH).all(|depth| {
        let cell = grid.get(BlockPosition {
            y: column.y - depth,
            ..column
        });
        cell.known() && cell.passable() && !cell.lethal()
    })
}

/// Whether stepping into this column would put the bot in lava, in a
/// hazard, or off a cliff -- the cases that want the keys released rather
/// than the route bent.
fn is_unsafe_ahead(grid: &TerrainGrid, ahead: BlockPosition) -> bool {
    let cell = grid.get(ahead);
    if cell.lethal() || cell.damaging() {
        return true;
    }
    let floor = grid.get(BlockPosition {
        y: ahead.y - 1,
        ..ahead
    });
    if floor.lethal() || floor.damaging() {
        return true;
    }
    is_cliff(grid, ahead)
}

/// Whether the column ahead is a single block step the bot should hop.
fn is_step_up(grid: &TerrainGrid, ahead: BlockPosition) -> bool {
    let at_feet = grid.get(ahead);
    if !at_feet.known() || at_feet.passable() {
        return false;
    }
    // Blocked at foot level, clear above: a step, not a wall.
    (1..=BODY_HEIGHT).all(|offset| {
        let cell = grid.get(BlockPosition {
            y: ahead.y + offset,
            ..ahead
        });
        cell.known() && cell.passable()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathfinding::grid::GridBounds;

    fn position(x: i32, y: i32, z: i32) -> BlockPosition {
        BlockPosition { x, y, z }
    }

    /// Flat ground at y=63, air above, spanning the probe region.
    fn plain() -> TerrainGrid {
        let bounds = GridBounds {
            min: position(-8, 58, -8),
            max: position(9, 70, 9),
        };
        let mut grid = TerrainGrid::empty(bounds);
        for x in -8..9 {
            for z in -8..9 {
                for y in 58..=63 {
                    grid.set(position(x, y, z), TerrainClass::Solid);
                }
                for y in 64..70 {
                    grid.set(position(x, y, z), TerrainClass::Air);
                }
            }
        }
        grid
    }

    fn feet() -> BlockPosition {
        position(0, 64, 0)
    }

    #[test]
    fn open_ground_produces_no_hazards_at_all() {
        let hazards = evaluate(
            &plain(),
            feet(),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.5, 6.0),
            Vec2::new(0.0, 1.0),
        );
        assert!(hazards.avoidance.is_negligible(), "{:?}", hazards.avoidance);
        assert!(!hazards.blocked_ahead);
        assert!(!hazards.step_ahead);
    }

    #[test]
    fn lava_pushes_the_bot_away_from_it() {
        let mut grid = plain();
        for x in 1..4 {
            grid.set(position(x, 64, 0), TerrainClass::Lava);
        }
        let hazards = evaluate(
            &grid,
            feet(),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.5, 6.0),
            Vec2::new(0.0, 1.0),
        );
        assert!(
            hazards.avoidance.x < -0.2,
            "should lean away from the lava: {:?}",
            hazards.avoidance
        );
    }

    #[test]
    fn walking_straight_at_lava_stops_the_bot_outright() {
        let mut grid = plain();
        for z in 1..4 {
            grid.set(position(0, 64, z), TerrainClass::Lava);
        }
        let hazards = evaluate(
            &grid,
            feet(),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.5, 6.0),
            Vec2::new(0.0, 1.0),
        );
        assert!(hazards.blocked_ahead);
    }

    #[test]
    fn a_cliff_ahead_stops_the_bot() {
        let mut grid = plain();
        // Carve the ground away past z=1.
        for x in -8..9 {
            for z in 2..9 {
                for y in 58..=63 {
                    grid.set(position(x, y, z), TerrainClass::Air);
                }
            }
        }
        let hazards = evaluate(
            &grid,
            feet(),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.5, 6.0),
            Vec2::new(0.0, 1.0),
        );
        assert!(hazards.blocked_ahead, "should not walk off the edge");
        assert!(
            hazards.avoidance.z < 0.0,
            "and should lean back from it: {:?}",
            hazards.avoidance
        );
    }

    #[test]
    fn a_shallow_drop_is_not_treated_as_a_cliff() {
        let mut grid = plain();
        // One block down, then ground again: a step, not a fall.
        for x in -8..9 {
            for z in 2..9 {
                grid.set(position(x, 63, z), TerrainClass::Air);
            }
        }
        let hazards = evaluate(
            &grid,
            feet(),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.5, 6.0),
            Vec2::new(0.0, 1.0),
        );
        assert!(
            !hazards.blocked_ahead,
            "a one-block drop is fine to fight on"
        );
    }

    #[test]
    fn a_wall_leans_the_bot_away_but_does_not_stop_it() {
        let mut grid = plain();
        for x in -8..9 {
            for y in 64..=66 {
                grid.set(position(x, y, 2), TerrainClass::Solid);
            }
        }
        let hazards = evaluate(
            &grid,
            feet(),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.5, 6.0),
            Vec2::new(0.0, 1.0),
        );
        assert!(hazards.avoidance.z < 0.0, "{:?}", hazards.avoidance);
        assert!(
            !hazards.blocked_ahead,
            "a wall is for steering around, not freezing at"
        );
        assert!(
            !hazards.step_ahead,
            "and a three-block wall is not something to hop either"
        );
    }

    #[test]
    fn a_single_block_step_is_reported_as_jumpable() {
        let mut grid = plain();
        grid.set(position(0, 64, 2), TerrainClass::Solid);
        let hazards = evaluate(
            &grid,
            feet(),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.5, 6.0),
            Vec2::new(0.0, 1.0),
        );
        assert!(hazards.step_ahead);
        assert!(!hazards.blocked_ahead);
    }

    #[test]
    fn a_two_block_wall_is_not_a_step() {
        let mut grid = plain();
        grid.set(position(0, 64, 2), TerrainClass::Solid);
        grid.set(position(0, 65, 2), TerrainClass::Solid);
        let hazards = evaluate(
            &grid,
            feet(),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.5, 6.0),
            Vec2::new(0.0, 1.0),
        );
        assert!(!hazards.step_ahead, "cannot hop a two-block wall");
    }

    #[test]
    fn the_lookahead_follows_the_heading_not_the_target() {
        let mut grid = plain();
        for z in 1..4 {
            grid.set(position(0, 64, z), TerrainClass::Lava);
        }
        // Target is through the lava, but the bot is circling sideways.
        let hazards = evaluate(
            &grid,
            feet(),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.5, 6.0),
            Vec2::new(-1.0, 0.0),
        );
        assert!(
            !hazards.blocked_ahead,
            "moving away from the lava is not blocked"
        );
    }

    #[test]
    fn with_no_heading_the_lookahead_falls_back_to_the_target_direction() {
        let mut grid = plain();
        for z in 1..4 {
            grid.set(position(0, 64, z), TerrainClass::Lava);
        }
        let hazards = evaluate(
            &grid,
            feet(),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.5, 6.0),
            Vec2::zero(),
        );
        assert!(hazards.blocked_ahead);
    }

    #[test]
    fn unknown_terrain_at_the_probe_edge_is_not_mistaken_for_a_cliff() {
        // An empty grid is entirely unknown -- the bot must not conclude it
        // is standing on the lip of the void.
        let grid = TerrainGrid::empty(GridBounds {
            min: position(-4, 60, -4),
            max: position(5, 70, 5),
        });
        let hazards = evaluate(
            &grid,
            feet(),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.5, 6.0),
            Vec2::new(0.0, 1.0),
        );
        assert!(!hazards.blocked_ahead);
        assert!(hazards.avoidance.is_negligible());
    }

    #[test]
    fn the_push_is_bounded_however_much_lava_there_is() {
        let mut grid = plain();
        for x in -8..9 {
            for z in -8..9 {
                grid.set(position(x, 64, z), TerrainClass::Lava);
            }
        }
        let hazards = evaluate(
            &grid,
            feet(),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.5, 6.0),
            Vec2::new(0.0, 1.0),
        );
        assert!(hazards.avoidance.length() <= 1.0 + 1e-9);
    }
}
