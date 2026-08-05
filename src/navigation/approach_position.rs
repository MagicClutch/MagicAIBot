use std::collections::HashMap;

use crate::minecraft::world_state::BlockPosition;

pub fn approach_positions(target: BlockPosition) -> Vec<BlockPosition> {
    let mut positions = Vec::with_capacity(11);
    for (x, z) in [
        (target.x - 1, target.z),
        (target.x + 1, target.z),
        (target.x, target.z - 1),
        (target.x, target.z + 1),
        (target.x - 1, target.z - 1),
        (target.x - 1, target.z + 1),
        (target.x + 1, target.z - 1),
        (target.x + 1, target.z + 1),
    ] {
        positions.push(BlockPosition { x, y: target.y, z });
    }
    // Standing on top can be the only valid interaction position for a
    // partially enclosed block. Below is included only when the normal floor,
    // foot and head validation accepts it.
    positions.push(BlockPosition {
        x: target.x,
        y: target.y + 1,
        z: target.z,
    });
    positions.push(BlockPosition {
        x: target.x,
        y: target.y - 1,
        z: target.z,
    });
    // A player whose feet are two blocks below the target has clear headroom
    // and can strike the underside of an overhead log at normal reach.
    positions.push(BlockPosition {
        x: target.x,
        y: target.y - 2,
        z: target.z,
    });
    positions
}

pub fn required_cells(approach: BlockPosition) -> [BlockPosition; 3] {
    [
        approach,
        BlockPosition {
            y: approach.y + 1,
            ..approach
        },
        BlockPosition {
            y: approach.y - 1,
            ..approach
        },
    ]
}

/// `allow_mining` must mirror whatever `NavigationMode` the caller is
/// actually pathfinding with. When true, a solid-but-minable foot/head cell
/// is accepted even though it isn't open air yet -- Azalea's pathfinder (see
/// `PathfinderOpts::allow_mining`) digs it out en route and already owns the
/// real feasibility check (hardness/tool cost, unbreakable blocks, liquids
/// above/adjacent -- see `CachedWorld::uncached_cost_for_breaking_block`).
/// Requiring the cell to *already* be air here -- as this function used to,
/// unconditionally -- rejected every fully buried ore candidate before
/// Azalea ever got a chance to tunnel to it, since none of its neighboring
/// cells are exposed.
pub fn is_valid_approach(
    target: BlockPosition,
    approach: BlockPosition,
    target_id: &str,
    cells: &HashMap<BlockPosition, Option<String>>,
    interaction_distance: f64,
    allow_mining: bool,
) -> bool {
    if approach == target || interaction_distance <= 0.0 {
        return false;
    }
    let dx = f64::from(target.x - approach.x);
    let dy = f64::from(target.y - approach.y);
    let dz = f64::from(target.z - approach.z);
    if (dx * dx + dy * dy + dz * dz).sqrt() > interaction_distance {
        return false;
    }
    let Some(Some(actual_target)) = cells.get(&target) else {
        return false;
    };
    if actual_target != target_id || is_hazard(actual_target) {
        return false;
    }
    let required = required_cells(approach);
    let Some(Some(foot)) = cells.get(&required[0]) else {
        return false;
    };
    let Some(Some(head)) = cells.get(&required[1]) else {
        return false;
    };
    let Some(Some(floor)) = cells.get(&required[2]) else {
        return false;
    };
    is_traversable(foot, allow_mining) && is_traversable(head, allow_mining) && is_safe_floor(floor)
}

pub fn is_passable(block_id: &str) -> bool {
    matches!(
        block_id,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

/// A cell a mining-enabled route can stand in: already open air, or a solid
/// block that isn't a liquid/hazard the bot would need to dig through first.
/// Whether it's actually *breakable* (bedrock, etc.) is left to Azalea's own
/// mining-cost check rather than duplicated here.
fn is_traversable(block_id: &str, allow_mining: bool) -> bool {
    is_passable(block_id) || (allow_mining && !is_hazard(block_id) && !is_liquid(block_id))
}

pub fn is_safe_floor(block_id: &str) -> bool {
    !is_passable(block_id) && !is_hazard(block_id)
}

pub fn is_liquid(block_id: &str) -> bool {
    matches!(
        block_id,
        "minecraft:water" | "minecraft:lava" | "minecraft:flowing_water" | "minecraft:flowing_lava"
    )
}

pub fn is_hazard(block_id: &str) -> bool {
    matches!(
        block_id,
        "minecraft:lava"
            | "minecraft:flowing_lava"
            | "minecraft:fire"
            | "minecraft:soul_fire"
            | "minecraft:cactus"
            | "minecraft:magma_block"
            | "minecraft:campfire"
            | "minecraft:soul_campfire"
            | "minecraft:powder_snow"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(
        target: BlockPosition,
        approach: BlockPosition,
    ) -> HashMap<BlockPosition, Option<String>> {
        let mut result = HashMap::new();
        result.insert(target, Some("minecraft:stone".into()));
        let required = required_cells(approach);
        result.insert(required[0], Some("minecraft:air".into()));
        result.insert(required[1], Some("minecraft:air".into()));
        result.insert(required[2], Some("minecraft:dirt".into()));
        result
    }

    #[test]
    fn generates_cardinal_diagonal_and_overhead_approaches() {
        let target = BlockPosition { x: 0, y: 64, z: 0 };
        assert_eq!(approach_positions(target).len(), 11);
        assert!(approach_positions(target).contains(&BlockPosition { x: 0, y: 62, z: 0 }));
    }

    #[test]
    fn validates_passable_foot_head_and_solid_floor() {
        let target = BlockPosition { x: 0, y: 64, z: 0 };
        let approach = BlockPosition { x: -1, y: 64, z: 0 };
        assert!(is_valid_approach(
            target,
            approach,
            "minecraft:stone",
            &cells(target, approach),
            4.5,
            false
        ));
    }

    #[test]
    fn rejects_hazards_and_unloaded_cells() {
        let target = BlockPosition { x: 0, y: 64, z: 0 };
        let approach = BlockPosition { x: -1, y: 64, z: 0 };
        let mut values = cells(target, approach);
        values.insert(
            BlockPosition { x: -1, y: 63, z: 0 },
            Some("minecraft:lava".into()),
        );
        assert!(!is_valid_approach(
            target,
            approach,
            "minecraft:stone",
            &values,
            4.5,
            false
        ));
        values.remove(&BlockPosition { x: -1, y: 63, z: 0 });
        assert!(!is_valid_approach(
            target,
            approach,
            "minecraft:stone",
            &values,
            4.5,
            false
        ));
    }

    /// The bug this fixes: a fully buried ore (every approach cell solid
    /// stone, not air) must be reachable once mining is allowed, since
    /// Azalea's pathfinder can dig the approach cell out itself.
    #[test]
    fn allows_solid_foot_and_head_when_mining_is_enabled() {
        let target = BlockPosition { x: 0, y: 64, z: 0 };
        let approach = BlockPosition { x: -1, y: 64, z: 0 };
        let mut values = HashMap::new();
        values.insert(target, Some("minecraft:diamond_ore".into()));
        let required = required_cells(approach);
        values.insert(required[0], Some("minecraft:stone".into()));
        values.insert(required[1], Some("minecraft:stone".into()));
        values.insert(required[2], Some("minecraft:stone".into()));

        assert!(!is_valid_approach(
            target,
            approach,
            "minecraft:diamond_ore",
            &values,
            4.5,
            false
        ));
        assert!(is_valid_approach(
            target,
            approach,
            "minecraft:diamond_ore",
            &values,
            4.5,
            true
        ));
    }

    /// Mining mode must not turn a liquid- or hazard-filled cell into a
    /// stand-on-able node just because it's "solid enough to dig" -- Azalea
    /// itself refuses to mine adjacent to liquids, and standing in lava
    /// isn't something digging it out first would fix.
    #[test]
    fn mining_mode_still_rejects_liquid_and_hazard_cells() {
        let target = BlockPosition { x: 0, y: 64, z: 0 };
        let approach = BlockPosition { x: -1, y: 64, z: 0 };
        let mut values = cells(target, approach);
        values.insert(required_cells(approach)[0], Some("minecraft:lava".into()));
        assert!(!is_valid_approach(
            target,
            approach,
            "minecraft:stone",
            &values,
            4.5,
            true
        ));

        let mut values = cells(target, approach);
        values.insert(required_cells(approach)[0], Some("minecraft:water".into()));
        assert!(!is_valid_approach(
            target,
            approach,
            "minecraft:stone",
            &values,
            4.5,
            true
        ));
    }
}
