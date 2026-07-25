use std::collections::HashSet;

use crate::{blocks::block_snapshot::BlockSnapshot, minecraft::world_state::BlockPosition};

pub fn next_candidate<'a>(
    candidates: &'a [BlockSnapshot],
    failed_blocks: &HashSet<BlockPosition>,
) -> Option<&'a BlockSnapshot> {
    candidates
        .iter()
        .find(|candidate| !failed_blocks.contains(&candidate.position))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_nearest_non_failed_candidate() {
        let candidates = vec![
            BlockSnapshot {
                position: BlockPosition { x: 1, y: 64, z: 0 },
                block_id: "minecraft:stone".into(),
                distance: 1.0,
                dimension: None,
                chunk_loaded: true,
            },
            BlockSnapshot {
                position: BlockPosition { x: 2, y: 64, z: 0 },
                block_id: "minecraft:stone".into(),
                distance: 2.0,
                dimension: None,
                chunk_loaded: true,
            },
        ];
        let mut failed = HashSet::new();
        failed.insert(candidates[0].position);
        assert_eq!(next_candidate(&candidates, &failed).unwrap().position.x, 2);
    }
}
