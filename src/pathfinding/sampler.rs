//! The bridge between Azalea's live world and the pathfinder's owned
//! snapshots: samples a region into a [`TerrainGrid`] and derives the
//! per-chunk summaries the navigation cache keeps.
//!
//! Only one function here touches the client
//! ([`MinecraftClient::sample_terrain`]); everything else is pure analysis
//! of the resulting grid and is unit-tested against hand-built terrain.
//!
//! [`MinecraftClient::sample_terrain`]: crate::minecraft::client::MinecraftClient

use crate::{
    error::AppError,
    minecraft::{client::MinecraftClient, world_state::BlockPosition},
    pathfinding::{
        grid::{GridBounds, TerrainGrid},
        moves::BODY_HEIGHT,
        route::ChunkKey,
        world_cache::{ChunkSummary, NavigationCache},
    },
};

/// A finished sample: the grid the search will run on, plus what was learned
/// about each chunk it covered.
pub struct TerrainSample {
    pub grid: TerrainGrid,
    pub summaries: Vec<(ChunkKey, ChunkSummary)>,
}

impl TerrainSample {
    /// Whether the sample found any loaded terrain at all. `false` means the
    /// bot is somewhere with no chunks (just connected, mid-dimension
    /// change, or a desync) -- a condition to wait out, not to fail on.
    #[must_use]
    pub fn has_terrain(&self) -> bool {
        self.grid.known_cells() > 0
    }
}

/// Samples the corridor between `from` and `to` and summarizes it.
///
/// The corridor -- rather than a disc around the bot -- is what a segment
/// search actually needs: `margin` blocks of slack on each side is enough
/// room to route around a hill without paying for terrain far behind the bot
/// that no route would ever use.
pub async fn sample_corridor(
    minecraft: &MinecraftClient,
    from: BlockPosition,
    to: BlockPosition,
    margin: i32,
    vertical_window: i32,
) -> Result<TerrainSample, AppError> {
    let bounds = GridBounds::spanning(
        from,
        to,
        margin,
        vertical_window,
        // The client clamps to the world's real limits; these are the
        // technical extremes, so nothing is clipped here by accident.
        -2048,
        2048,
    );
    let grid = minecraft.sample_terrain(bounds).await?;
    let summaries = summarize(&grid);
    Ok(TerrainSample { grid, summaries })
}

/// Records a sample's chunk summaries in the navigation cache.
pub fn record(cache: &mut NavigationCache, sample: &TerrainSample) {
    for (key, summary) in &sample.summaries {
        cache.record(*key, *summary);
    }
}

/// Derives one [`ChunkSummary`] per chunk the grid covers.
///
/// A chunk with no known cells at all isn't summarized: it wasn't loaded, so
/// there is nothing to record, and writing a "nothing here" summary would
/// poison the route planner into believing an unexplored chunk was surveyed
/// and found unroutable.
#[must_use]
pub fn summarize(grid: &TerrainGrid) -> Vec<(ChunkKey, ChunkSummary)> {
    let bounds = grid.bounds();
    if bounds.cell_count() == 0 {
        return Vec::new();
    }
    let min_chunk = ChunkKey::of(bounds.min);
    let max_chunk = ChunkKey::of(BlockPosition {
        x: bounds.max.x - 1,
        y: bounds.min.y,
        z: bounds.max.z - 1,
    });
    let mut summaries = Vec::new();
    for chunk_x in min_chunk.x..=max_chunk.x {
        for chunk_z in min_chunk.z..=max_chunk.z {
            let key = ChunkKey {
                x: chunk_x,
                z: chunk_z,
            };
            if let Some(summary) = summarize_chunk(grid, key) {
                summaries.push((key, summary));
            }
        }
    }
    summaries
}

fn summarize_chunk(grid: &TerrainGrid, key: ChunkKey) -> Option<ChunkSummary> {
    let bounds = grid.bounds();
    let start_x = (key.x * 16).max(bounds.min.x);
    let end_x = (key.x * 16 + 16).min(bounds.max.x);
    let start_z = (key.z * 16).max(bounds.min.z);
    let end_z = (key.z * 16 + 16).min(bounds.max.z);
    if start_x >= end_x || start_z >= end_z {
        return None;
    }
    let mut known = 0usize;
    let mut total = 0usize;
    let mut surface_y: Option<i32> = None;
    for x in start_x..end_x {
        for z in start_z..end_z {
            for y in bounds.min.y..bounds.max.y {
                let position = BlockPosition { x, y, z };
                total += 1;
                if !grid.get(position).known() {
                    continue;
                }
                known += 1;
                if grid.standable(position, BODY_HEIGHT) {
                    surface_y = Some(surface_y.map_or(y, |current: i32| current.max(y)));
                }
            }
        }
    }
    if known == 0 {
        return None;
    }
    let coverage = if total == 0 {
        0.0
    } else {
        known as f64 / total as f64
    };
    // Sampled and found to contain nowhere at all to stand: genuinely
    // unroutable (solid rock, open void, a lava lake), and the route planner
    // is allowed to avoid it. That conclusion is only trustworthy because the
    // chunk *was* loaded -- see this function's doc comment.
    let Some(surface_y) = surface_y else {
        return Some(ChunkSummary {
            coverage,
            ..ChunkSummary::unroutable(0)
        });
    };
    Some(ChunkSummary {
        surface_y: Some(surface_y),
        routable: true,
        coverage,
        sampled_at: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathfinding::terrain::TerrainClass;

    fn position(x: i32, y: i32, z: i32) -> BlockPosition {
        BlockPosition { x, y, z }
    }

    /// Two chunks side by side (x 0..32), surface at y=63.
    fn two_chunks() -> TerrainGrid {
        let bounds = GridBounds {
            min: position(0, 60, 0),
            max: position(32, 70, 16),
        };
        let mut grid = TerrainGrid::empty(bounds);
        for x in 0..32 {
            for z in 0..16 {
                for y in 60..=63 {
                    grid.set(position(x, y, z), TerrainClass::Solid);
                }
                for y in 64..70 {
                    grid.set(position(x, y, z), TerrainClass::Air);
                }
            }
        }
        grid
    }

    #[test]
    fn summarizing_reports_one_entry_per_covered_chunk() {
        let summaries = summarize(&two_chunks());
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].0, ChunkKey { x: 0, z: 0 });
        assert_eq!(summaries[1].0, ChunkKey { x: 1, z: 0 });
    }

    #[test]
    fn the_summary_reports_the_highest_standable_surface() {
        let summaries = summarize(&two_chunks());
        assert_eq!(summaries[0].1.surface_y, Some(64));
        assert!(summaries[0].1.routable);
        assert!((summaries[0].1.coverage - 1.0).abs() < 1e-9);
    }

    #[test]
    fn an_unloaded_chunk_is_not_summarized_at_all() {
        // Only the left chunk is filled in; the right one stays Unknown, as
        // an unloaded chunk would be.
        let bounds = GridBounds {
            min: position(0, 60, 0),
            max: position(32, 70, 16),
        };
        let mut grid = TerrainGrid::empty(bounds);
        for x in 0..16 {
            for z in 0..16 {
                grid.set(position(x, 63, z), TerrainClass::Solid);
                grid.set(position(x, 64, z), TerrainClass::Air);
                grid.set(position(x, 65, z), TerrainClass::Air);
            }
        }
        let summaries = summarize(&grid);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].0, ChunkKey { x: 0, z: 0 });
    }

    #[test]
    fn a_solid_chunk_with_nowhere_to_stand_is_recorded_as_unroutable() {
        let bounds = GridBounds {
            min: position(0, 60, 0),
            max: position(16, 70, 16),
        };
        let mut grid = TerrainGrid::empty(bounds);
        for x in 0..16 {
            for z in 0..16 {
                for y in 60..70 {
                    grid.set(position(x, y, z), TerrainClass::Solid);
                }
            }
        }
        let summaries = summarize(&grid);
        assert_eq!(summaries.len(), 1);
        assert!(!summaries[0].1.routable);
        assert_eq!(summaries[0].1.surface_y, None);
    }

    #[test]
    fn a_partially_loaded_chunk_reports_reduced_coverage() {
        let bounds = GridBounds {
            min: position(0, 60, 0),
            max: position(16, 70, 16),
        };
        let mut grid = TerrainGrid::empty(bounds);
        for x in 0..8 {
            for z in 0..16 {
                for y in 60..70 {
                    grid.set(position(x, y, z), TerrainClass::Air);
                }
            }
        }
        let summaries = summarize(&grid);
        assert!((summaries[0].1.coverage - 0.5).abs() < 1e-6);
    }

    #[test]
    fn recorded_summaries_become_route_knowledge() {
        use crate::pathfinding::route::ChunkKnowledge;
        let sample = TerrainSample {
            grid: two_chunks(),
            summaries: summarize(&two_chunks()),
        };
        assert!(sample.has_terrain());
        let mut cache = NavigationCache::new(64, 1000);
        record(&mut cache, &sample);
        assert!(cache.is_known(ChunkKey { x: 0, z: 0 }));
        assert!(!cache.is_blocked(ChunkKey { x: 0, z: 0 }));
        assert_eq!(cache.surface_y(ChunkKey { x: 1, z: 0 }), Some(64));
    }

    #[test]
    fn an_empty_sample_reports_no_terrain() {
        let sample = TerrainSample {
            grid: TerrainGrid::empty(GridBounds {
                min: position(0, 0, 0),
                max: position(4, 4, 4),
            }),
            summaries: Vec::new(),
        };
        assert!(!sample.has_terrain());
    }
}
