use crate::minecraft::world_state::{BlockPosition, PositionSnapshot};
pub fn distance_to_block_center(position: PositionSnapshot, block: BlockPosition) -> f64 {
    ((position.x - (f64::from(block.x) + 0.5)).powi(2)
        + (position.y - (f64::from(block.y) + 0.5)).powi(2)
        + (position.z - (f64::from(block.z) + 0.5)).powi(2))
    .sqrt()
}
pub fn within_reach(position: Option<PositionSnapshot>, block: BlockPosition, reach: f64) -> bool {
    position.is_some_and(|position| distance_to_block_center(position, block) <= reach)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checks_reach() {
        assert!(within_reach(
            Some(PositionSnapshot {
                x: 0.5,
                y: 0.5,
                z: 0.5
            }),
            BlockPosition { x: 0, y: 0, z: 0 },
            0.1
        ));
        assert!(!within_reach(
            Some(PositionSnapshot {
                x: 10.0,
                y: 0.0,
                z: 0.0
            }),
            BlockPosition { x: 0, y: 0, z: 0 },
            4.5
        ));
    }
}
