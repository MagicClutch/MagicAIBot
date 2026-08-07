//! The navigation cache: what the bot knows about the world, keyed by
//! chunk, outliving the chunks themselves. Pure -- the [`sampler`] fills it,
//! this module only stores and reasons about what it was given.
//!
//! [`sampler`]: crate::pathfinding::sampler
//!
//! # Why cache at all
//!
//! Minecraft unloads chunks the moment the bot walks out of render distance.
//! Without a cache, a bot that walks 200 blocks and turns around knows
//! nothing about where it just came from, so a replan has to treat its own
//! backtrail as unexplored. Baritone's answer is a persistent world cache;
//! this is the same idea at a coarser grain -- per chunk, keep the cheap
//! summary the high-level route planner actually consults (is there ground
//! here, roughly how high, is this chunk hopeless) rather than every block.
//!
//! The expensive full-resolution terrain is *not* cached: block-level A*
//! only ever runs on terrain that is loaded right now, so caching blocks
//! would mostly serve to let the bot path confidently through a house
//! someone demolished an hour ago.
//!
//! # Freshness
//!
//! Every summary carries the tick it was sampled at, and callers get to
//! decide what "too old" means ([`NavigationCache::prune`]). Chunk changes
//! invalidate directly ([`NavigationCache::invalidate`]), which is what the
//! spec's "a chunk changes -> discard the affected segments" is built on.

use std::collections::HashMap;

use crate::{
    minecraft::world_state::BlockPosition,
    pathfinding::route::{ChunkKey, ChunkKnowledge},
};

/// The cheap per-chunk summary the route planner consults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChunkSummary {
    /// Highest standable Y found while sampling, if any.
    pub surface_y: Option<i32>,
    /// Whether any standable cell was found at all. A chunk sampled with
    /// none is genuinely unroutable (solid rock, void, a lava lake) and the
    /// route planner refuses it.
    pub routable: bool,
    /// Fraction of the sampled column volume that was known, 0.0-1.0 -- a
    /// chunk sampled at the edge of render distance may be only partly
    /// filled in, and a later, fuller sample should win.
    pub coverage: f64,
    /// Monotonic counter (not a clock) recording when this was written --
    /// see [`NavigationCache::tick`]. A counter rather than an `Instant`
    /// keeps this module pure and its tests deterministic.
    pub sampled_at: u64,
}

impl ChunkSummary {
    #[must_use]
    pub fn unroutable(sampled_at: u64) -> Self {
        Self {
            surface_y: None,
            routable: false,
            coverage: 1.0,
            sampled_at,
        }
    }
}

/// Chunk-keyed terrain knowledge with a capacity bound and age-based
/// pruning.
#[derive(Clone, Debug)]
pub struct NavigationCache {
    summaries: HashMap<ChunkKey, ChunkSummary>,
    capacity: usize,
    /// Monotonic write counter; also serves as the age clock.
    tick: u64,
    /// How many ticks a summary stays trustworthy. Terrain does change, and
    /// a summary from ten thousand writes ago is a guess.
    max_age: u64,
}

impl NavigationCache {
    #[must_use]
    pub fn new(capacity: usize, max_age: u64) -> Self {
        Self {
            summaries: HashMap::new(),
            capacity: capacity.max(1),
            tick: 0,
            max_age: max_age.max(1),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.summaries.len()
    }

    /// Records what a sample found for one chunk. A *fuller* sample always
    /// wins over a sparser one taken at the same moment, so a chunk glimpsed
    /// at the edge of render distance isn't allowed to overwrite a proper
    /// look at it.
    pub fn record(&mut self, key: ChunkKey, summary: ChunkSummary) {
        self.tick += 1;
        let summary = ChunkSummary {
            sampled_at: self.tick,
            ..summary
        };
        match self.summaries.get(&key) {
            Some(existing)
                if existing.coverage > summary.coverage
                    && self.tick.saturating_sub(existing.sampled_at) < self.max_age / 4 =>
            {
                // Keep the better-covered recent sample.
            }
            _ => {
                self.summaries.insert(key, summary);
            }
        }
        if self.summaries.len() > self.capacity {
            self.evict_oldest();
        }
    }

    /// Forgets a chunk entirely -- what a block change or chunk reload
    /// triggers. Returns whether anything was actually cached for it, so
    /// callers can skip the replan when a change lands somewhere the bot
    /// never knew about.
    pub fn invalidate(&mut self, key: ChunkKey) -> bool {
        self.summaries.remove(&key).is_some()
    }

    /// Forgets the chunk containing `position`, plus its immediate
    /// neighbors: a block change on a chunk border affects routing through
    /// both sides, and one extra chunk of forgetting is far cheaper than a
    /// route that walks into a wall.
    pub fn invalidate_around(&mut self, position: BlockPosition) -> usize {
        let center = ChunkKey::of(position);
        (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (dx, dz)))
            .filter(|(dx, dz)| {
                self.invalidate(ChunkKey {
                    x: center.x + dx,
                    z: center.z + dz,
                })
            })
            .count()
    }

    /// Drops everything older than `max_age`.
    pub fn prune(&mut self) -> usize {
        let tick = self.tick;
        let max_age = self.max_age;
        let before = self.summaries.len();
        self.summaries
            .retain(|_, summary| tick.saturating_sub(summary.sampled_at) <= max_age);
        before - self.summaries.len()
    }

    #[must_use]
    pub fn get(&self, key: ChunkKey) -> Option<&ChunkSummary> {
        self.summaries
            .get(&key)
            .filter(|summary| self.tick.saturating_sub(summary.sampled_at) <= self.max_age)
    }

    fn evict_oldest(&mut self) {
        let Some(oldest) = self
            .summaries
            .iter()
            .min_by_key(|(_, summary)| summary.sampled_at)
            .map(|(key, _)| *key)
        else {
            return;
        };
        self.summaries.remove(&oldest);
    }
}

impl ChunkKnowledge for NavigationCache {
    fn is_known(&self, key: ChunkKey) -> bool {
        self.get(key).is_some()
    }

    fn is_blocked(&self, key: ChunkKey) -> bool {
        self.get(key).is_some_and(|summary| !summary.routable)
    }

    fn surface_y(&self, key: ChunkKey) -> Option<i32> {
        self.get(key).and_then(|summary| summary.surface_y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(x: i32, z: i32) -> ChunkKey {
        ChunkKey { x, z }
    }

    fn summary(surface_y: i32) -> ChunkSummary {
        ChunkSummary {
            surface_y: Some(surface_y),
            routable: true,
            coverage: 1.0,
            sampled_at: 0,
        }
    }

    #[test]
    fn an_empty_cache_knows_nothing_and_blocks_nothing() {
        let cache = NavigationCache::new(64, 1000);
        assert_eq!(cache.len(), 0);
        assert!(!cache.is_known(key(0, 0)));
        assert!(
            !cache.is_blocked(key(0, 0)),
            "not knowing about a chunk is not the same as knowing it's bad"
        );
        assert_eq!(cache.surface_y(key(0, 0)), None);
    }

    #[test]
    fn recording_makes_a_chunk_known_with_its_surface() {
        let mut cache = NavigationCache::new(64, 1000);
        cache.record(key(1, 2), summary(72));
        assert!(cache.is_known(key(1, 2)));
        assert!(!cache.is_blocked(key(1, 2)));
        assert_eq!(cache.surface_y(key(1, 2)), Some(72));
    }

    #[test]
    fn a_chunk_with_no_standable_ground_is_reported_as_blocked() {
        let mut cache = NavigationCache::new(64, 1000);
        cache.record(key(3, 3), ChunkSummary::unroutable(0));
        assert!(cache.is_known(key(3, 3)));
        assert!(cache.is_blocked(key(3, 3)));
    }

    #[test]
    fn invalidating_forgets_a_chunk_and_reports_whether_it_knew_anything() {
        let mut cache = NavigationCache::new(64, 1000);
        cache.record(key(0, 0), summary(64));
        assert!(cache.invalidate(key(0, 0)));
        assert!(!cache.is_known(key(0, 0)));
        assert!(
            !cache.invalidate(key(0, 0)),
            "a change in a chunk we never knew about is not news"
        );
    }

    #[test]
    fn invalidating_around_a_position_also_clears_the_neighbors() {
        let mut cache = NavigationCache::new(64, 1000);
        for x in -1..=1 {
            for z in -1..=1 {
                cache.record(key(x, z), summary(64));
            }
        }
        cache.record(key(5, 5), summary(64));
        let cleared = cache.invalidate_around(BlockPosition { x: 8, y: 64, z: 8 });
        assert_eq!(cleared, 9);
        assert!(cache.is_known(key(5, 5)), "a distant chunk is untouched");
    }

    #[test]
    fn stale_entries_stop_counting_as_known_and_are_pruned() {
        let mut cache = NavigationCache::new(64, 4);
        cache.record(key(0, 0), summary(64));
        for index in 1..=6 {
            cache.record(key(index, 9), summary(64));
        }
        assert!(
            !cache.is_known(key(0, 0)),
            "an entry older than max_age must not be trusted"
        );
        let pruned = cache.prune();
        assert!(pruned >= 1);
    }

    #[test]
    fn capacity_is_enforced_by_evicting_the_oldest_entry() {
        let mut cache = NavigationCache::new(3, 10_000);
        for index in 0..5 {
            cache.record(key(index, 0), summary(64));
        }
        assert_eq!(cache.len(), 3);
        assert!(!cache.is_known(key(0, 0)), "the oldest is gone");
        assert!(cache.is_known(key(4, 0)), "the newest survives");
    }

    #[test]
    fn a_sparser_sample_does_not_immediately_overwrite_a_fuller_one() {
        let mut cache = NavigationCache::new(64, 1000);
        cache.record(
            key(0, 0),
            ChunkSummary {
                coverage: 0.9,
                ..summary(70)
            },
        );
        cache.record(
            key(0, 0),
            ChunkSummary {
                coverage: 0.1,
                surface_y: Some(20),
                ..summary(20)
            },
        );
        assert_eq!(
            cache.surface_y(key(0, 0)),
            Some(70),
            "a glimpse at render-distance edge must not overwrite a good look"
        );
    }

    #[test]
    fn a_sparse_sample_does_win_once_the_full_one_is_old() {
        let mut cache = NavigationCache::new(64, 8);
        cache.record(
            key(0, 0),
            ChunkSummary {
                coverage: 0.9,
                ..summary(70)
            },
        );
        for index in 1..=4 {
            cache.record(key(index, 5), summary(64));
        }
        cache.record(
            key(0, 0),
            ChunkSummary {
                coverage: 0.2,
                surface_y: Some(30),
                routable: true,
                sampled_at: 0,
            },
        );
        assert_eq!(cache.surface_y(key(0, 0)), Some(30));
    }
}
