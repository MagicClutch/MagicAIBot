//! The owned cuboid terrain snapshot every search runs against. Pure: no
//! Azalea types, no I/O, no locks.
//!
//! This is the single most important boundary in the pathfinding layer.
//! Azalea's world lives behind a lock on the client, and a long A* run must
//! not hold that lock (nor can it, from a background thread). So the
//! sampler copies a bounded region out of the world *once*, into one flat
//! `Vec<u8>` -- one byte per block, see [`crate::pathfinding::terrain::
//! TerrainClass::as_byte`] -- and every later question the search asks is
//! answered from that owned copy. A 5x5-chunk region with a 48-block
//! vertical window is ~190KB, cheap to hold and cheap to `Send` to a
//! blocking thread.
//!
//! Positions outside the grid read back as `Unknown` rather than panicking
//! or wrapping: a search that walks off the edge of what was sampled must
//! see "I don't know what's there" (and stop), which is exactly the
//! frontier behavior the segment planner relies on.

use crate::{
    minecraft::world_state::{BlockPosition, PositionSnapshot},
    pathfinding::terrain::TerrainClass,
};

/// An inclusive-min, exclusive-max block-space cuboid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GridBounds {
    pub min: BlockPosition,
    /// Exclusive upper corner: `max.x - min.x` is the width.
    pub max: BlockPosition,
}

impl GridBounds {
    /// A cuboid covering both `from` and `to` plus `margin` blocks of slack
    /// around them -- what a segment search needs, since the useful search
    /// area is the corridor between two points rather than a disc around
    /// one.
    #[must_use]
    pub fn spanning(
        from: BlockPosition,
        to: BlockPosition,
        margin: i32,
        vertical_margin: i32,
        world_min_y: i32,
        world_max_y: i32,
    ) -> Self {
        let min_y = (from.y.min(to.y) - vertical_margin).max(world_min_y);
        let max_y = (from.y.max(to.y) + vertical_margin).min(world_max_y);
        Self {
            min: BlockPosition {
                x: from.x.min(to.x) - margin,
                y: min_y,
                z: from.z.min(to.z) - margin,
            },
            max: BlockPosition {
                x: from.x.max(to.x) + margin + 1,
                y: (max_y + 1).max(min_y),
                z: from.z.max(to.z) + margin + 1,
            },
        }
    }

    #[must_use]
    pub fn width(self) -> i32 {
        (self.max.x - self.min.x).max(0)
    }
    #[must_use]
    pub fn height(self) -> i32 {
        (self.max.y - self.min.y).max(0)
    }
    #[must_use]
    pub fn depth(self) -> i32 {
        (self.max.z - self.min.z).max(0)
    }

    #[must_use]
    pub fn cell_count(self) -> usize {
        self.width() as usize * self.height() as usize * self.depth() as usize
    }

    #[must_use]
    pub fn contains(self, position: BlockPosition) -> bool {
        position.x >= self.min.x
            && position.x < self.max.x
            && position.y >= self.min.y
            && position.y < self.max.y
            && position.z >= self.min.z
            && position.z < self.max.z
    }

    /// Clamps `position` to the nearest cell inside these bounds -- used to
    /// keep a segment goal reachable when the goal itself sits just outside
    /// the sampled corridor.
    #[must_use]
    pub fn clamp(self, position: BlockPosition) -> BlockPosition {
        BlockPosition {
            x: position
                .x
                .clamp(self.min.x, (self.max.x - 1).max(self.min.x)),
            y: position
                .y
                .clamp(self.min.y, (self.max.y - 1).max(self.min.y)),
            z: position
                .z
                .clamp(self.min.z, (self.max.z - 1).max(self.min.z)),
        }
    }
}

/// One flat byte per block over [`GridBounds`]. Cloneable and `Send`, which
/// is the whole point -- see this module's doc comment.
#[derive(Clone, Debug)]
pub struct TerrainGrid {
    bounds: GridBounds,
    cells: Vec<u8>,
    /// How many cells were actually filled in from loaded chunks. Zero means
    /// nothing around the bot is known at all (just joined, or a total
    /// desync), which callers report rather than treating as "walled in".
    known_cells: usize,
}

impl TerrainGrid {
    /// An all-`Unknown` grid over `bounds`.
    #[must_use]
    pub fn empty(bounds: GridBounds) -> Self {
        Self {
            cells: vec![TerrainClass::Unknown.as_byte(); bounds.cell_count()],
            bounds,
            known_cells: 0,
        }
    }

    #[must_use]
    pub fn bounds(&self) -> GridBounds {
        self.bounds
    }

    #[must_use]
    pub fn known_cells(&self) -> usize {
        self.known_cells
    }

    fn index(&self, position: BlockPosition) -> Option<usize> {
        if !self.bounds.contains(position) {
            return None;
        }
        let dx = (position.x - self.bounds.min.x) as usize;
        let dy = (position.y - self.bounds.min.y) as usize;
        let dz = (position.z - self.bounds.min.z) as usize;
        let width = self.bounds.width() as usize;
        let depth = self.bounds.depth() as usize;
        // Y-major so a vertical probe (the most common access pattern: feet,
        // head, floor) touches nearby memory.
        Some((dy * width * depth) + (dx * depth) + dz)
    }

    /// The class at `position`, or `Unknown` outside the sampled region.
    #[must_use]
    pub fn get(&self, position: BlockPosition) -> TerrainClass {
        self.index(position)
            .map(|index| TerrainClass::from_byte(self.cells[index]))
            .unwrap_or_default()
    }

    /// Writes one cell. Silently ignores positions outside the bounds --
    /// the sampler walks whole chunks, whose edges routinely fall outside a
    /// non-chunk-aligned region, and clipping there is expected rather than
    /// exceptional.
    pub fn set(&mut self, position: BlockPosition, class: TerrainClass) {
        let Some(index) = self.index(position) else {
            return;
        };
        let previously_known = TerrainClass::from_byte(self.cells[index]).known();
        if class.known() && !previously_known {
            self.known_cells += 1;
        } else if !class.known() && previously_known {
            self.known_cells -= 1;
        }
        self.cells[index] = class.as_byte();
    }

    /// Whether a body of `height` blocks can occupy the column whose feet
    /// are at `feet` -- every cell from the feet up must be passable, and
    /// all of them must be known.
    #[must_use]
    pub fn body_fits(&self, feet: BlockPosition, height: i32) -> bool {
        (0..height).all(|offset| {
            let cell = self.get(BlockPosition {
                x: feet.x,
                y: feet.y + offset,
                z: feet.z,
            });
            cell.known() && cell.passable()
        })
    }

    /// The cell directly under `feet` -- the floor the bot stands on.
    #[must_use]
    pub fn floor_below(&self, feet: BlockPosition) -> TerrainClass {
        self.get(BlockPosition {
            x: feet.x,
            y: feet.y - 1,
            z: feet.z,
        })
    }

    /// Whether `feet` is a legal standing position for a `height`-block body
    /// on solid ground: body fits, and the block below supports standing.
    #[must_use]
    pub fn standable(&self, feet: BlockPosition, height: i32) -> bool {
        self.body_fits(feet, height) && self.floor_below(feet).supports_standing()
    }

    /// Whether `feet` is somewhere the bot can be *without* solid ground --
    /// swimming. Kept separate from [`Self::standable`] so the cost model
    /// can price the two differently.
    #[must_use]
    pub fn swimmable(&self, feet: BlockPosition, height: i32) -> bool {
        self.get(feet) == TerrainClass::Water && self.body_fits(feet, height)
    }

    /// Snaps `from` onto the nearest standable cell, searching down first
    /// and then up -- a waypoint generated by the coarse route can just as
    /// easily be buried inside a hill as floating above a valley.
    #[must_use]
    pub fn nearest_standable(
        &self,
        from: BlockPosition,
        height: i32,
        vertical_search: i32,
    ) -> Option<BlockPosition> {
        if self.standable(from, height) {
            return Some(from);
        }
        (1..=vertical_search).find_map(|offset| {
            let below = BlockPosition {
                x: from.x,
                y: from.y - offset,
                z: from.z,
            };
            if self.standable(below, height) {
                return Some(below);
            }
            let above = BlockPosition {
                x: from.x,
                y: from.y + offset,
                z: from.z,
            };
            self.standable(above, height).then_some(above)
        })
    }
}

/// Straight-line block distance, the unit every cost and heuristic in this
/// layer is expressed in.
#[must_use]
pub fn block_distance(from: BlockPosition, to: BlockPosition) -> f64 {
    let dx = f64::from(to.x - from.x);
    let dy = f64::from(to.y - from.y);
    let dz = f64::from(to.z - from.z);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Horizontal-only distance -- what route/segment logic measures with, since
/// a 40-block climb and a 40-block walk are the same amount of *route*.
#[must_use]
pub fn horizontal_distance(from: BlockPosition, to: BlockPosition) -> f64 {
    f64::from(to.x - from.x).hypot(f64::from(to.z - from.z))
}

/// Center of a block cell, for handing a block-space waypoint back to the
/// position-space movement layer.
#[must_use]
pub fn block_center(position: BlockPosition) -> PositionSnapshot {
    PositionSnapshot {
        x: f64::from(position.x) + 0.5,
        y: f64::from(position.y),
        z: f64::from(position.z) + 0.5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(x: i32, y: i32, z: i32) -> BlockPosition {
        BlockPosition { x, y, z }
    }

    fn flat_grid() -> TerrainGrid {
        let bounds = GridBounds {
            min: position(-8, 60, -8),
            max: position(8, 70, 8),
        };
        let mut grid = TerrainGrid::empty(bounds);
        for x in -8..8 {
            for z in -8..8 {
                grid.set(position(x, 63, z), TerrainClass::Solid);
                for y in 64..70 {
                    grid.set(position(x, y, z), TerrainClass::Air);
                }
            }
        }
        grid
    }

    #[test]
    fn everything_outside_the_bounds_reads_as_unknown() {
        let grid = flat_grid();
        assert_eq!(grid.get(position(0, 64, 0)), TerrainClass::Air);
        assert_eq!(grid.get(position(100, 64, 0)), TerrainClass::Unknown);
        assert_eq!(grid.get(position(0, 5000, 0)), TerrainClass::Unknown);
    }

    #[test]
    fn writing_outside_the_bounds_is_ignored_rather_than_panicking() {
        let mut grid = flat_grid();
        let known_before = grid.known_cells();
        grid.set(position(1000, 64, 1000), TerrainClass::Solid);
        assert_eq!(grid.known_cells(), known_before);
    }

    #[test]
    fn known_cell_accounting_tracks_writes_in_both_directions() {
        let bounds = GridBounds {
            min: position(0, 0, 0),
            max: position(2, 2, 2),
        };
        let mut grid = TerrainGrid::empty(bounds);
        assert_eq!(grid.known_cells(), 0);
        grid.set(position(0, 0, 0), TerrainClass::Solid);
        grid.set(position(1, 1, 1), TerrainClass::Air);
        assert_eq!(grid.known_cells(), 2);
        grid.set(position(0, 0, 0), TerrainClass::Air);
        assert_eq!(
            grid.known_cells(),
            2,
            "overwriting a known cell keeps it known"
        );
        grid.set(position(0, 0, 0), TerrainClass::Unknown);
        assert_eq!(grid.known_cells(), 1);
    }

    #[test]
    fn standing_requires_a_floor_and_a_clear_body() {
        let grid = flat_grid();
        assert!(grid.standable(position(0, 64, 0), 2));
        assert!(
            !grid.standable(position(0, 65, 0), 2),
            "floating one block above the floor is not standing"
        );
        assert!(
            !grid.standable(position(0, 63, 0), 2),
            "inside the floor block is not standing"
        );
    }

    #[test]
    fn a_two_block_body_does_not_fit_under_a_one_block_ceiling() {
        let mut grid = flat_grid();
        grid.set(position(2, 65, 2), TerrainClass::Solid);
        assert!(!grid.standable(position(2, 64, 2), 2));
        assert!(grid.standable(position(2, 64, 2), 1));
    }

    #[test]
    fn unknown_cells_are_never_standable_even_with_a_floor() {
        let mut grid = flat_grid();
        grid.set(position(3, 64, 3), TerrainClass::Unknown);
        assert!(!grid.standable(position(3, 64, 3), 2));
    }

    #[test]
    fn nearest_standable_searches_both_directions() {
        let grid = flat_grid();
        assert_eq!(
            grid.nearest_standable(position(0, 64, 0), 2, 4),
            Some(position(0, 64, 0))
        );
        assert_eq!(
            grid.nearest_standable(position(0, 61, 0), 2, 6),
            Some(position(0, 64, 0)),
            "a waypoint buried below ground snaps up to the surface"
        );
    }

    #[test]
    fn swimming_needs_water_at_the_feet() {
        let mut grid = flat_grid();
        grid.set(position(4, 64, 4), TerrainClass::Water);
        grid.set(position(4, 65, 4), TerrainClass::Water);
        assert!(grid.swimmable(position(4, 64, 4), 2));
        assert!(!grid.swimmable(position(0, 64, 0), 2));
    }

    #[test]
    fn spanning_bounds_cover_both_endpoints_with_margin() {
        let bounds =
            GridBounds::spanning(position(0, 64, 0), position(40, 70, -20), 8, 16, -64, 319);
        assert!(bounds.contains(position(0, 64, 0)));
        assert!(bounds.contains(position(40, 70, -20)));
        assert!(bounds.contains(position(-8, 64, -28)));
        assert!(!bounds.contains(position(-9, 64, 0)));
    }

    #[test]
    fn clamping_pulls_an_outside_position_to_the_edge() {
        let bounds = GridBounds {
            min: position(0, 0, 0),
            max: position(10, 10, 10),
        };
        assert_eq!(bounds.clamp(position(50, 5, -5)), position(9, 5, 0));
        assert_eq!(bounds.clamp(position(5, 5, 5)), position(5, 5, 5));
    }

    #[test]
    fn block_center_lands_in_the_middle_of_the_cell_horizontally() {
        let center = block_center(position(3, 64, -2));
        assert!((center.x - 3.5).abs() < 1e-9);
        assert!((center.y - 64.0).abs() < 1e-9);
        assert!((center.z + 1.5).abs() < 1e-9);
    }

    #[test]
    fn distances_measure_what_they_say() {
        assert!((block_distance(position(0, 0, 0), position(3, 4, 0)) - 5.0).abs() < 1e-9);
        assert!((horizontal_distance(position(0, 0, 0), position(3, 40, 4)) - 5.0).abs() < 1e-9);
    }
}
