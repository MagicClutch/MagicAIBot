//! Pure, unit-testable physics and decision logic for the automatic water
//! bucket MLG (see `crate::survival::SurvivalController` for the async
//! orchestration layer that calls into this module). Nothing here touches
//! Azalea or the network -- every function takes plain values or already-
//! fetched block ids, the same split this codebase uses elsewhere
//! (`interaction::placement_rules`, `interaction::faces`,
//! `interaction::reach`) to keep the interesting logic testable without a
//! live connection.

use crate::{
    interaction::placement_rules::{has_support, is_replaceable},
    minecraft::world_state::BlockPosition,
};

/// Water evaporates in the Nether; MLG is pointless (and wastes the bucket
/// slot mid-fall) there.
pub const NETHER_DIMENSION: &str = "minecraft:the_nether";
pub const WATER_BUCKET_ID: &str = "minecraft:water_bucket";
pub const WATER_BLOCK_ID: &str = "minecraft:water";
pub const LAVA_BLOCK_ID: &str = "minecraft:lava";

/// Gravity applied to falling velocity every tick, in blocks/tick^2 -- mirrors
/// `azalea_physics::travel::get_effective_gravity`'s vanilla value (not
/// callable directly from here: it's private to that crate, and depends on
/// live ECS context this prediction deliberately doesn't need).
pub const GRAVITY_PER_TICK: f64 = 0.08;
/// Vertical drag applied after gravity every tick -- mirrors the `0.98`
/// multiplier `azalea_physics::travel` applies to `velocity.y` each tick
/// outside a fluid.
pub const DRAG_PER_TICK: f64 = 0.98;
pub const TICK_MILLIS: u64 = 50;
/// Bound on the tick-by-tick simulation below so a bad prediction (e.g. no
/// floor found, falling into an overhang loop) can never spin forever --
/// 400 ticks is 20 simulated seconds, far beyond any realistic MLG fall.
const MAX_SIMULATED_TICKS: u32 = 400;

/// A fully-resolved prediction of where and when the bot will land.
#[derive(Clone, Debug, PartialEq)]
pub struct LandingPrediction {
    /// The solid block the bot will land on.
    pub support: BlockPosition,
    /// The empty cell directly above `support` -- where water must be
    /// placed to break the fall.
    pub water_target: BlockPosition,
    pub ticks_to_impact: u32,
    /// Total fall distance (blocks) at the moment of impact, including
    /// whatever `Physics::fall_distance` had already accumulated before this
    /// prediction was made.
    pub predicted_total_fall: f64,
    /// Block id of `support`, kept so the look target can detect the ground
    /// changing out from under an in-progress aim (see
    /// `look::look_controller::resolve_context`'s `LookTarget::BlockFacePoint`
    /// staleness check).
    pub support_id: String,
}

/// Ticks until a body falling from `start_y` with `velocity_y` (blocks/tick,
/// negative = downward) reaches `landing_feet_y`, applying the same gravity
/// and drag Azalea's own physics engine applies every tick outside a fluid.
/// Saturates at `MAX_SIMULATED_TICKS` rather than looping forever if the
/// inputs never converge (e.g. `landing_feet_y` above `start_y`).
pub fn ticks_to_impact(start_y: f64, velocity_y: f64, landing_feet_y: f64) -> u32 {
    let mut y = start_y;
    let mut velocity = velocity_y;
    let mut ticks = 0u32;
    while y > landing_feet_y && ticks < MAX_SIMULATED_TICKS {
        velocity = (velocity - GRAVITY_PER_TICK) * DRAG_PER_TICK;
        y += velocity;
        ticks += 1;
    }
    ticks
}

/// Total predicted fall distance at landing: whatever `Physics` already
/// accumulated before this tick, plus the remaining drop to `landing_feet_y`.
pub fn predicted_total_fall(fall_distance_so_far: f64, start_y: f64, landing_feet_y: f64) -> f64 {
    fall_distance_so_far + (start_y - landing_feet_y).max(0.0)
}

/// Whether a predicted total fall distance is dangerous enough to warrant
/// the clutch.
pub fn is_dangerous(predicted_total_fall: f64, min_fall_distance: f64) -> bool {
    predicted_total_fall >= min_fall_distance
}

/// Remaining vertical distance (blocks) to the surface the bot's feet will
/// land on -- the primary signal placement timing is driven by (see
/// `should_place_now`), recomputed fresh every tick from the bot's live
/// position rather than carried forward from an earlier prediction.
pub fn remaining_drop_blocks(current_y: f64, landing_feet_y: f64) -> f64 {
    (current_y - landing_feet_y).max(0.0)
}

/// Extra distance (blocks) to place water earlier by, compensating for
/// round-trip network latency and the server's own tick-processing delay:
/// at `velocity_y` blocks/tick, `latency_ms` of round-trip time covers this
/// much additional fall before a placement packet sent *now* could possibly
/// take effect on the server. Scales with current fall speed rather than
/// being a fixed distance, since the same latency covers far more ground
/// near terminal velocity than early in a fall.
pub fn latency_compensation_blocks(velocity_y: f64, latency_ms: u64) -> f64 {
    let ticks = latency_ms as f64 / TICK_MILLIS as f64;
    velocity_y.abs() * ticks
}

/// The actual distance-to-impact threshold placement fires at this tick:
/// the configured base offset (e.g. "2-3 blocks before impact"), widened by
/// `latency_compensation_blocks` and, when `uncertain` (the predicted
/// landing block just changed -- still-resolving horizontal drift, a
/// knockback still being absorbed, ...), an extra fixed safety margin so an
/// unstable trajectory places slightly earlier rather than risking a total
/// miss.
pub fn effective_placement_offset(
    base_offset_blocks: f64,
    velocity_y: f64,
    latency_ms: u64,
    uncertain: bool,
) -> f64 {
    const UNCERTAINTY_BONUS_BLOCKS: f64 = 1.0;
    base_offset_blocks
        + latency_compensation_blocks(velocity_y, latency_ms)
        + if uncertain {
            UNCERTAINTY_BONUS_BLOCKS
        } else {
            0.0
        }
}

/// Whether it's time to place: either the remaining drop has shrunk inside
/// `effective_offset_blocks`, or impact is predicted within the very next
/// tick regardless of distance (a last-chance safety valve for an extreme
/// fall speed where a single tick's movement can cover more ground than the
/// offset window is wide, which would otherwise step clean over it without
/// ever satisfying the distance check). Recomputed fresh every survival
/// tick from the live prediction -- never a fixed sleep.
pub fn should_place_now(
    remaining_drop_blocks: f64,
    effective_offset_blocks: f64,
    ticks_to_impact: u32,
) -> bool {
    remaining_drop_blocks <= effective_offset_blocks || ticks_to_impact <= 1
}

/// The column of block positions to inspect for a landing surface, ordered
/// nearest-to-farthest starting one block below `bot_feet`.
pub fn landing_column(bot_feet: BlockPosition, depth: i32) -> Vec<BlockPosition> {
    (1..=depth)
        .map(|offset| BlockPosition {
            x: bot_feet.x,
            y: bot_feet.y - offset,
            z: bot_feet.z,
        })
        .collect()
}

/// Scans an already-fetched column (ordered nearest-to-farthest, as
/// `landing_column` produces) for the first non-replaceable block -- the
/// surface the bot will land on. Mirrors `is_replaceable`'s air/vegetation
/// rules so grass, snow layers, tall grass etc. don't get mistaken for solid
/// ground. Returns `None` if the column runs into an unloaded chunk (a `None`
/// entry) before a surface is found, or if the column is exhausted without
/// one -- both mean "can't predict yet", not "no danger".
pub fn find_landing_support(
    column: &[(BlockPosition, Option<String>)],
) -> Option<(BlockPosition, String)> {
    for (position, id) in column {
        match id {
            Some(id) if !is_replaceable(Some(id)) => return Some((*position, id.clone())),
            Some(_) => continue,
            None => return None,
        }
    }
    None
}

/// Whether `support` (already confirmed non-replaceable by
/// `find_landing_support`) is a landing surface MLG can actually help with:
/// solid ground (not a fluid -- landing in water already takes no damage,
/// and lava is a different hazard this feature doesn't address), with clear
/// air directly above it to place water into.
pub fn is_valid_landing_surface(support_id: &str, above_id: Option<&str>) -> bool {
    has_support(Some(support_id)) && is_replaceable(above_id)
}

/// Whether `support_id` names a liquid the bot would land in directly,
/// making the clutch unnecessary (water) or unable to help (lava).
pub fn lands_in_liquid(support_id: &str) -> bool {
    support_id == WATER_BLOCK_ID || support_id == LAVA_BLOCK_ID
}

/// Nether restriction gate: `disable_in_nether` is a config escape hatch
/// (see `crate::config::SurvivalConfig`), but defaults to enforcing the rule.
pub fn nether_disables_mlg(dimension: Option<&str>, disable_in_nether: bool) -> bool {
    disable_in_nether && dimension == Some(NETHER_DIMENSION)
}

/// Whether a water bucket is available and usable the same way every other
/// automatic item-selection path in this codebase requires (see
/// `InteractionController::place_at`): present, and in the hotbar
/// specifically, since nothing here can move an item into the hotbar.
pub fn water_bucket_available(
    inventory_available: bool,
    has_water_bucket: bool,
    water_bucket_in_hotbar: bool,
) -> bool {
    inventory_available && has_water_bucket && water_bucket_in_hotbar
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(x: i32, y: i32, z: i32) -> BlockPosition {
        BlockPosition { x, y, z }
    }

    // -- ticks_to_impact / predicted_total_fall: cliff, tower, and pillar
    // falls are all the same physics (a straight drop with a fixed starting
    // point), just different narratives for how the bot got there.

    #[test]
    fn cliff_fall_from_height_is_dangerous() {
        // 16-block drop, starting at rest (walked off an edge).
        let total = predicted_total_fall(0.0, 80.0, 64.0);
        assert_eq!(total, 16.0);
        assert!(is_dangerous(total, 4.0));
        assert!(ticks_to_impact(80.0, 0.0, 64.0) > 0);
    }

    #[test]
    fn short_step_off_a_ledge_is_not_dangerous() {
        let total = predicted_total_fall(0.0, 65.0, 64.0);
        assert_eq!(total, 1.0);
        assert!(!is_dangerous(total, 4.0));
    }

    #[test]
    fn tower_fall_accounts_for_distance_already_accumulated() {
        // A pillar partially gave way: 2 blocks already fallen before this
        // prediction, then another 6 blocks to the ground.
        let total = predicted_total_fall(2.0, 70.0, 64.0);
        assert_eq!(total, 8.0);
        assert!(is_dangerous(total, 4.0));
    }

    #[test]
    fn pillar_fall_from_a_tall_column_is_dangerous() {
        let total = predicted_total_fall(0.0, 96.0, 64.0);
        assert!(is_dangerous(total, 4.0));
        assert!(ticks_to_impact(96.0, 0.0, 64.0) > ticks_to_impact(80.0, 0.0, 64.0));
    }

    #[test]
    fn bridge_fall_mid_gap_is_dangerous() {
        // Fell through a gap while bridging, 10 blocks above the ground.
        let total = predicted_total_fall(0.0, 74.0, 64.0);
        assert!(is_dangerous(total, 4.0));
    }

    #[test]
    fn knockback_fall_with_upward_velocity_still_converges_and_takes_longer() {
        // Punched off a structure: initial velocity is upward before gravity
        // takes over, so impact should take longer than falling from the
        // same height at rest.
        let baseline = ticks_to_impact(80.0, 0.0, 64.0);
        let knocked_back = ticks_to_impact(80.0, 0.4, 64.0);
        assert!(knocked_back > baseline);
    }

    #[test]
    fn ticks_to_impact_is_zero_when_already_at_or_below_the_target() {
        // Nothing to fall -- the loop guard must not even run once.
        assert_eq!(ticks_to_impact(10.0, 0.0, 100.0), 0);
        assert_eq!(ticks_to_impact(64.0, -1.0, 64.0), 0);
    }

    #[test]
    fn ticks_to_impact_saturates_on_an_extreme_drop_instead_of_looping_forever() {
        // Terminal velocity is a few blocks/tick, so a 100,000-block drop
        // cannot possibly land within the simulation cap -- must saturate
        // rather than spin indefinitely.
        assert_eq!(ticks_to_impact(100_000.0, 0.0, 0.0), MAX_SIMULATED_TICKS);
    }

    #[test]
    fn ticks_to_impact_converges_well_within_the_cap_for_a_realistic_drop() {
        // A full Overworld build-height fall (384 blocks) must resolve to an
        // exact tick count, not a saturated one.
        let ticks = ticks_to_impact(384.0, 0.0, 0.0);
        assert!(ticks > 0 && ticks < MAX_SIMULATED_TICKS);
    }

    #[test]
    fn should_place_now_triggers_within_the_offset_distance() {
        assert!(should_place_now(2.0, 2.5, 10)); // 2 blocks left, 2.5-block offset
        assert!(!should_place_now(5.0, 2.5, 10)); // still 5 blocks out
        assert!(should_place_now(0.0, 2.5, 10));
    }

    #[test]
    fn should_place_now_has_a_last_tick_safety_valve() {
        // Predicted impact next tick, even though the distance check alone
        // wouldn't yet trigger -- an extreme-velocity edge case where one
        // tick's movement could otherwise jump clean over the offset window.
        assert!(should_place_now(50.0, 2.5, 1));
        assert!(should_place_now(50.0, 2.5, 0));
    }

    #[test]
    fn latency_compensation_scales_with_fall_speed() {
        assert_eq!(latency_compensation_blocks(0.0, 100), 0.0);
        // 2 blocks/tick * (100ms / 50ms per tick) = 4 blocks.
        assert_eq!(latency_compensation_blocks(-2.0, 100), 4.0);
        let slow = latency_compensation_blocks(-1.0, 100);
        let fast = latency_compensation_blocks(-3.0, 100);
        assert!(fast > slow);
    }

    #[test]
    fn effective_offset_widens_for_latency_and_uncertainty() {
        let baseline = effective_placement_offset(2.5, -1.0, 0, false);
        assert_eq!(baseline, 2.5);
        let with_latency = effective_placement_offset(2.5, -1.0, 100, false);
        assert!(with_latency > baseline);
        let uncertain = effective_placement_offset(2.5, -1.0, 0, true);
        assert!(uncertain > baseline);
    }

    #[test]
    fn remaining_drop_never_goes_negative() {
        assert_eq!(remaining_drop_blocks(70.0, 64.0), 6.0);
        assert_eq!(remaining_drop_blocks(60.0, 64.0), 0.0);
    }

    // -- landing-surface scanning, including "moving landing target".

    #[test]
    fn finds_the_first_solid_block_scanning_down() {
        let column = vec![
            (pos(0, 79, 0), Some("minecraft:air".into())),
            (pos(0, 78, 0), Some("minecraft:air".into())),
            (pos(0, 77, 0), Some("minecraft:tall_grass".into())),
            (pos(0, 76, 0), Some("minecraft:stone".into())),
            (pos(0, 75, 0), Some("minecraft:stone".into())),
        ];
        assert_eq!(
            find_landing_support(&column),
            Some((pos(0, 76, 0), "minecraft:stone".into()))
        );
    }

    #[test]
    fn moving_landing_target_tracks_horizontal_drift() {
        // Two columns from different (x, z) positions -- as the bot drifts
        // sideways while falling, the predicted landing block changes.
        let column_a = vec![(pos(10, 63, 5), Some("minecraft:stone".into()))];
        let column_b = vec![(pos(12, 60, 7), Some("minecraft:dirt".into()))];
        let a = find_landing_support(&column_a).unwrap();
        let b = find_landing_support(&column_b).unwrap();
        assert_ne!(a.0, b.0);
    }

    #[test]
    fn unloaded_chunk_in_the_column_reports_unknown_rather_than_a_surface() {
        let column = vec![
            (pos(0, 79, 0), Some("minecraft:air".into())),
            (pos(0, 78, 0), None),
            (pos(0, 77, 0), Some("minecraft:stone".into())),
        ];
        assert_eq!(find_landing_support(&column), None);
    }

    #[test]
    fn exhausted_column_with_no_surface_reports_unknown() {
        let column = vec![(pos(0, 79, 0), Some("minecraft:air".into())); 5];
        assert_eq!(find_landing_support(&column), None);
    }

    // -- placement-surface validity.

    #[test]
    fn solid_ground_with_clear_air_above_is_a_valid_surface() {
        assert!(is_valid_landing_surface(
            "minecraft:stone",
            Some("minecraft:air")
        ));
    }

    #[test]
    fn invalid_placement_surface_when_the_space_above_is_occupied() {
        // The bot would land on stone, but something already sits in the
        // cell water would need to go into.
        assert!(!is_valid_landing_surface(
            "minecraft:stone",
            Some("minecraft:torch")
        ));
    }

    #[test]
    fn liquids_are_not_treated_as_placeable_support() {
        assert!(!is_valid_landing_surface(
            WATER_BLOCK_ID,
            Some("minecraft:air")
        ));
        assert!(!is_valid_landing_surface(
            LAVA_BLOCK_ID,
            Some("minecraft:air")
        ));
    }

    #[test]
    fn landing_in_water_or_lava_is_detected_directly() {
        assert!(lands_in_liquid(WATER_BLOCK_ID));
        assert!(lands_in_liquid(LAVA_BLOCK_ID));
        assert!(!lands_in_liquid("minecraft:stone"));
    }

    // -- dimension and inventory gating.

    #[test]
    fn nether_dimension_disables_mlg_by_default() {
        assert!(nether_disables_mlg(Some(NETHER_DIMENSION), true));
    }

    #[test]
    fn overworld_dimension_never_disables_mlg() {
        assert!(!nether_disables_mlg(Some("minecraft:overworld"), true));
        assert!(!nether_disables_mlg(Some("minecraft:overworld"), false));
    }

    #[test]
    fn dimension_switching_from_overworld_to_nether_flips_the_gate() {
        let overworld = Some("minecraft:overworld");
        let nether = Some(NETHER_DIMENSION);
        assert!(!nether_disables_mlg(overworld, true));
        assert!(nether_disables_mlg(nether, true));
    }

    #[test]
    fn disable_in_nether_config_flag_can_be_turned_off() {
        assert!(!nether_disables_mlg(Some(NETHER_DIMENSION), false));
    }

    #[test]
    fn no_bucket_available_blocks_mlg() {
        assert!(!water_bucket_available(true, false, false));
        assert!(!water_bucket_available(true, true, false)); // held, but not in hotbar
        assert!(!water_bucket_available(false, true, true)); // inventory stale
        assert!(water_bucket_available(true, true, true));
    }
}
