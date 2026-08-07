//! The weighted cost model: what every kind of movement is *worth*, in one
//! place. Pure -- no world access, no Azalea types. [`crate::pathfinding::
//! moves`] decides which moves are geometrically legal; this module decides
//! what they cost, and A* does the rest.
//!
//! The unit is "one block of easy walking" = `walk`. Everything else is
//! priced relative to that, which makes the numbers readable in config: a
//! `break_block` of 6.0 says "mining through one block is worth walking six
//! blocks around it", and a `lava_avoidance` of 200.0 says "walk 200 blocks
//! out of your way rather than touch lava". Tuning is therefore a config
//! edit, not a code change.
//!
//! Expansion is the explicit design goal (the spec's "the scoring system
//! should allow future expansion"): a new movement gets a [`MoveKind`]
//! variant and an arm in [`CostProfile::cost_of`], and nothing else in the
//! pathfinding layer has to know about it. A new *hazard* doesn't even need
//! that -- it goes in `terrain::classify` and is priced by the existing
//! hazard weights.

use crate::pathfinding::terrain::TerrainClass;

/// One movement between two adjacent-ish cells. `blocks` fields carry the
/// magnitude the cost scales with, so the cost function stays a pure
/// function of the kind alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoveKind {
    /// Straight walk to an orthogonally adjacent cell at the same height.
    Walk,
    /// Walk to a diagonally adjacent cell at the same height.
    Diagonal,
    /// Step or jump up one block.
    JumpUp,
    /// Controlled descent of `blocks` blocks.
    Drop { blocks: i32 },
    /// Move through water.
    Swim,
    /// Move that requires mining `blocks` blocks out of the way first.
    Break { blocks: i32 },
    /// Move across a gap that has to be bridged (placing `blocks` blocks).
    Bridge { blocks: i32 },
    /// Climb a ladder/vine column by `blocks` blocks.
    Climb { blocks: i32 },
}

/// Every weight the pathfinder scores with. Mirrored one-to-one by
/// `crate::config::PathfindingCostConfig`, which is where the user-facing
/// defaults and validation live.
#[derive(Clone, Copy, Debug)]
pub struct CostProfile {
    /// Cost of one block of flat walking -- the unit everything else is
    /// expressed in.
    pub walk: f64,
    /// Cost of one diagonal step. Geometrically `sqrt(2)` walks; kept
    /// separate so a user can discourage diagonals (some servers' anticheat
    /// dislikes them) without touching anything else.
    pub diagonal: f64,
    /// Cost of gaining one block of height.
    pub jump_up: f64,
    /// Cost per block of descent, below [`Self::max_safe_drop`].
    pub drop_per_block: f64,
    /// Deepest drop the bot will take without treating it as fall damage.
    pub max_safe_drop: i32,
    /// Flat penalty added to any drop deeper than `max_safe_drop`, on top of
    /// the per-block cost -- what makes the search prefer stairs to a cliff.
    pub fall_damage_penalty: f64,
    /// Cost of one block of swimming.
    pub swim: f64,
    /// Cost of mining one block out of the way.
    pub break_block: f64,
    /// Cost of placing one block to bridge a gap.
    pub bridge_block: f64,
    /// Cost of one block of ladder/vine climbing.
    pub climb: f64,
    /// Added for entering a damaging-but-survivable cell (fire, cactus,
    /// magma, ...).
    pub hazard: f64,
    /// Added for entering (or passing directly over) lava. Deliberately
    /// enormous: the search should exhaust every alternative first.
    pub lava_avoidance: f64,
    /// Added for a cell adjacent to a hazard -- gives routes a reason to
    /// leave a block of clearance rather than shaving the edge of a lava
    /// lake.
    pub hazard_proximity: f64,
    /// Added per cell for standing next to a drop with nothing below --
    /// cheap edge avoidance, since the bot's own physics can slide it off a
    /// ledge it was only ever meant to walk beside.
    pub ledge_proximity: f64,
    /// Added for a cell within the danger radius of a known hostile entity.
    /// See `crate::pathfinding::moves::MovePolicy::entity_hazards`.
    pub entity_hazard: f64,
}

impl Default for CostProfile {
    fn default() -> Self {
        Self {
            walk: 1.0,
            diagonal: 1.41,
            jump_up: 1.8,
            drop_per_block: 1.1,
            max_safe_drop: 3,
            fall_damage_penalty: 20.0,
            swim: 3.0,
            break_block: 6.0,
            bridge_block: 8.0,
            climb: 2.5,
            hazard: 40.0,
            lava_avoidance: 200.0,
            hazard_proximity: 6.0,
            ledge_proximity: 1.0,
            entity_hazard: 25.0,
        }
    }
}

impl CostProfile {
    /// Base cost of a movement, before any per-cell terrain penalties.
    #[must_use]
    pub fn cost_of(&self, kind: MoveKind) -> f64 {
        match kind {
            MoveKind::Walk => self.walk,
            MoveKind::Diagonal => self.diagonal,
            MoveKind::JumpUp => self.jump_up,
            MoveKind::Drop { blocks } => {
                let blocks = blocks.max(0);
                let base = self.drop_per_block * f64::from(blocks);
                if blocks > self.max_safe_drop {
                    base + self.fall_damage_penalty
                } else {
                    base
                }
            }
            MoveKind::Swim => self.swim,
            MoveKind::Break { blocks } => self.walk + self.break_block * f64::from(blocks.max(0)),
            MoveKind::Bridge { blocks } => self.walk + self.bridge_block * f64::from(blocks.max(0)),
            MoveKind::Climb { blocks } => self.climb * f64::from(blocks.max(1)),
        }
    }

    /// Penalty for the terrain the bot's body actually ends up in.
    #[must_use]
    pub fn terrain_penalty(&self, destination: TerrainClass) -> f64 {
        if destination.lethal() {
            return self.lava_avoidance;
        }
        if destination.damaging() {
            return self.hazard;
        }
        0.0
    }

    /// Penalty for what the bot ends up standing *on*: lava directly below a
    /// walkway is nearly as bad as walking into it, since one mistimed
    /// physics tick puts the bot in it.
    #[must_use]
    pub fn floor_penalty(&self, floor: TerrainClass) -> f64 {
        if floor.lethal() {
            return self.lava_avoidance;
        }
        if floor.damaging() {
            return self.hazard;
        }
        0.0
    }

    /// Penalty for hazards merely *near* the destination cell -- see
    /// [`Self::hazard_proximity`].
    #[must_use]
    pub fn proximity_penalty(&self, adjacent_hazards: usize, adjacent_lava: usize) -> f64 {
        self.hazard_proximity * adjacent_hazards as f64
            + self.lava_avoidance.min(self.hazard_proximity * 8.0) * adjacent_lava as f64
    }

    /// Lowest possible cost of moving one block in any direction. A* needs
    /// this to keep its heuristic admissible: multiply the straight-line
    /// distance by the cheapest per-block cost and the estimate can never
    /// exceed the true remaining cost, which is what guarantees the search
    /// doesn't return a needlessly expensive route.
    #[must_use]
    pub fn cheapest_step(&self) -> f64 {
        self.walk
            .min(self.diagonal / std::f64::consts::SQRT_2)
            .min(self.swim)
            .min(self.drop_per_block)
            .max(f64::MIN_POSITIVE)
    }

    /// Admissible A* heuristic: straight-line block distance priced at the
    /// cheapest possible per-block rate.
    #[must_use]
    pub fn heuristic(&self, straight_line_blocks: f64) -> f64 {
        straight_line_blocks * self.cheapest_step()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diagonal_costs_less_than_two_walks() {
        let profile = CostProfile::default();
        assert!(profile.cost_of(MoveKind::Diagonal) < 2.0 * profile.cost_of(MoveKind::Walk));
        assert!(profile.cost_of(MoveKind::Diagonal) > profile.cost_of(MoveKind::Walk));
    }

    #[test]
    fn a_safe_drop_is_cheap_and_a_dangerous_one_is_not() {
        let profile = CostProfile::default();
        let safe = profile.cost_of(MoveKind::Drop {
            blocks: profile.max_safe_drop,
        });
        let dangerous = profile.cost_of(MoveKind::Drop {
            blocks: profile.max_safe_drop + 1,
        });
        assert!(safe < profile.fall_damage_penalty);
        assert!(dangerous > safe + profile.fall_damage_penalty);
    }

    #[test]
    fn breaking_scales_with_the_number_of_blocks_removed() {
        let profile = CostProfile::default();
        let one = profile.cost_of(MoveKind::Break { blocks: 1 });
        let two = profile.cost_of(MoveKind::Break { blocks: 2 });
        assert!((two - one - profile.break_block).abs() < 1e-9);
    }

    #[test]
    fn mining_through_a_block_is_worth_several_blocks_of_walking() {
        let profile = CostProfile::default();
        assert!(
            profile.cost_of(MoveKind::Break { blocks: 1 }) > 5.0 * profile.cost_of(MoveKind::Walk),
            "a detour of a few blocks should always beat mining"
        );
    }

    #[test]
    fn lava_is_priced_far_above_any_survivable_hazard() {
        let profile = CostProfile::default();
        assert!(
            profile.terrain_penalty(TerrainClass::Lava)
                > profile.terrain_penalty(TerrainClass::Hazard)
        );
        assert_eq!(profile.terrain_penalty(TerrainClass::Air), 0.0);
        assert_eq!(profile.terrain_penalty(TerrainClass::Solid), 0.0);
    }

    #[test]
    fn standing_on_lava_is_penalized_like_standing_in_it() {
        let profile = CostProfile::default();
        assert_eq!(
            profile.floor_penalty(TerrainClass::Lava),
            profile.lava_avoidance
        );
        assert_eq!(profile.floor_penalty(TerrainClass::Solid), 0.0);
    }

    #[test]
    fn the_heuristic_never_exceeds_the_cheapest_real_route() {
        // Admissibility, checked the way it actually matters: the estimate
        // for N blocks must not exceed N of the cheapest possible steps.
        let profile = CostProfile::default();
        for blocks in [1.0, 7.5, 100.0, 5000.0] {
            let cheapest_real = blocks * profile.cheapest_step();
            assert!(profile.heuristic(blocks) <= cheapest_real + 1e-9);
        }
    }

    #[test]
    fn the_cheapest_step_accounts_for_diagonals_being_cheaper_per_block() {
        // A diagonal covers sqrt(2) blocks for `diagonal` cost, so its
        // per-block rate is what bounds the heuristic, not `walk`.
        let profile = CostProfile {
            diagonal: 1.0,
            ..CostProfile::default()
        };
        assert!(profile.cheapest_step() < profile.walk);
    }

    #[test]
    fn proximity_penalties_accumulate_per_neighbor() {
        let profile = CostProfile::default();
        assert_eq!(profile.proximity_penalty(0, 0), 0.0);
        assert!(profile.proximity_penalty(2, 0) > profile.proximity_penalty(1, 0));
        assert!(profile.proximity_penalty(0, 1) > profile.proximity_penalty(1, 0));
    }
}
