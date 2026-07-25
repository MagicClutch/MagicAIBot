use std::collections::HashMap;

use crate::minecraft::world_state::BlockPosition;

pub fn approach_positions(target: BlockPosition) -> Vec<BlockPosition> {
    let mut positions = Vec::with_capacity(8);
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

pub fn is_valid_approach(
    target: BlockPosition,
    approach: BlockPosition,
    target_id: &str,
    cells: &HashMap<BlockPosition, Option<String>>,
    interaction_distance: f64,
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
    is_passable(foot) && is_passable(head) && is_safe_floor(floor)
}

pub fn is_passable(block_id: &str) -> bool {
    matches!(
        block_id,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

pub fn is_safe_floor(block_id: &str) -> bool {
    !is_passable(block_id) && !is_hazard(block_id)
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
    fn generates_cardinal_and_diagonal_approaches() {
        assert_eq!(
            approach_positions(BlockPosition { x: 0, y: 64, z: 0 }).len(),
            8
        );
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
            4.5
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
            4.5
        ));
        values.remove(&BlockPosition { x: -1, y: 63, z: 0 });
        assert!(!is_valid_approach(
            target,
            approach,
            "minecraft:stone",
            &values,
            4.5
        ));
    }
}
