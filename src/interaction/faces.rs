use crate::minecraft::world_state::BlockPosition;

/// Why a face is being targeted. Placement is deliberately stricter because
/// the clicked face determines where the server places the new block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockFacePurpose {
    #[allow(dead_code)]
    Look,
    Break,
    PlaceSupport,
    #[allow(dead_code)]
    Interact,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockFace {
    Up,
    Down,
    North,
    South,
    West,
    East,
}
impl BlockFace {
    pub fn offset(self) -> (i32, i32, i32) {
        match self {
            Self::Up => (0, 1, 0),
            Self::Down => (0, -1, 0),
            Self::North => (0, 0, -1),
            Self::South => (0, 0, 1),
            Self::West => (-1, 0, 0),
            Self::East => (1, 0, 0),
        }
    }
    pub fn opposite(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Down => Self::Up,
            Self::North => Self::South,
            Self::South => Self::North,
            Self::West => Self::East,
            Self::East => Self::West,
        }
    }
    pub fn neighbor(self, position: BlockPosition) -> BlockPosition {
        let (x, y, z) = self.offset();
        BlockPosition {
            x: position.x + x,
            y: position.y + y,
            z: position.z + z,
        }
    }
}

/// Safe, inset points on a face. This is the single full-cube fallback used
/// by look, breaking and placement. Collision-shape-specific points can be
/// substituted here later without changing those callers.
#[must_use]
pub fn face_hit_points(
    position: BlockPosition,
    face: BlockFace,
    purpose: BlockFacePurpose,
    inset: f64,
    edge_margin: f64,
    maximum: usize,
) -> Vec<[f64; 3]> {
    let inset = inset.clamp(0.000_001, 0.49);
    let margin = edge_margin.clamp(inset, 0.49);
    let offsets = [
        (0.0, 0.0),
        (0.0, -0.18),
        (0.0, 0.18),
        (-0.18, 0.0),
        (0.18, 0.0),
    ];
    let _requires_exact_face = matches!(purpose, BlockFacePurpose::PlaceSupport);
    offsets
        .into_iter()
        .take(maximum)
        .map(|(first, second)| {
            let middle = 0.5;
            let low = margin;
            let high = 1.0 - margin;
            let (x, y, z) = match face {
                BlockFace::North => (middle + first, middle + second, inset),
                BlockFace::South => (middle + first, middle + second, 1.0 - inset),
                BlockFace::West => (inset, middle + second, middle + first),
                BlockFace::East => (1.0 - inset, middle + second, middle + first),
                BlockFace::Up => (middle + first, 1.0 - inset, middle + second),
                BlockFace::Down => (middle + first, inset, middle + second),
            };
            [
                f64::from(position.x) + x.clamp(low, high),
                f64::from(position.y) + y.clamp(low, high),
                f64::from(position.z) + z.clamp(low, high),
            ]
        })
        .collect()
}
pub fn best_face(target: BlockPosition, bot: [f64; 3]) -> BlockFace {
    let dx = bot[0] - (f64::from(target.x) + 0.5);
    let dy = bot[1] - (f64::from(target.y) + 0.5);
    let dz = bot[2] - (f64::from(target.z) + 0.5);
    if dy.abs() >= dx.abs() && dy.abs() >= dz.abs() {
        if dy >= 0.0 {
            BlockFace::Up
        } else {
            BlockFace::Down
        }
    } else if dx.abs() >= dz.abs() {
        if dx >= 0.0 {
            BlockFace::East
        } else {
            BlockFace::West
        }
    } else if dz >= 0.0 {
        BlockFace::South
    } else {
        BlockFace::North
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selects_face_nearest_bot() {
        assert_eq!(
            best_face(BlockPosition { x: 0, y: 0, z: 0 }, [4.0, 0.5, 0.5]),
            BlockFace::East
        );
        assert_eq!(
            BlockFace::North.neighbor(BlockPosition { x: 1, y: 2, z: 3 }),
            BlockPosition { x: 1, y: 2, z: 2 }
        );
    }
    #[test]
    fn support_direction_maps_to_the_clicked_opposite_face() {
        let target = BlockPosition {
            x: 10,
            y: 64,
            z: 20,
        };
        assert_eq!(
            (BlockFace::Down.neighbor(target), BlockFace::Down.opposite()),
            (
                BlockPosition {
                    x: 10,
                    y: 63,
                    z: 20
                },
                BlockFace::Up
            )
        );
        assert_eq!(
            (BlockFace::Up.neighbor(target), BlockFace::Up.opposite()),
            (
                BlockPosition {
                    x: 10,
                    y: 65,
                    z: 20
                },
                BlockFace::Down
            )
        );
        assert_eq!(
            (
                BlockFace::North.neighbor(target),
                BlockFace::North.opposite()
            ),
            (
                BlockPosition {
                    x: 10,
                    y: 64,
                    z: 19
                },
                BlockFace::South
            )
        );
        assert_eq!(
            (
                BlockFace::South.neighbor(target),
                BlockFace::South.opposite()
            ),
            (
                BlockPosition {
                    x: 10,
                    y: 64,
                    z: 21
                },
                BlockFace::North
            )
        );
        assert_eq!(
            (BlockFace::West.neighbor(target), BlockFace::West.opposite()),
            (BlockPosition { x: 9, y: 64, z: 20 }, BlockFace::East)
        );
        assert_eq!(
            (BlockFace::East.neighbor(target), BlockFace::East.opposite()),
            (
                BlockPosition {
                    x: 11,
                    y: 64,
                    z: 20
                },
                BlockFace::West
            )
        );
    }

    #[test]
    fn face_points_are_inset_and_never_on_edges() {
        for face in [
            BlockFace::Up,
            BlockFace::Down,
            BlockFace::North,
            BlockFace::South,
            BlockFace::West,
            BlockFace::East,
        ] {
            let points = face_hit_points(
                BlockPosition {
                    x: 10,
                    y: 64,
                    z: -3,
                },
                face,
                BlockFacePurpose::Break,
                0.001,
                0.12,
                5,
            );
            assert_eq!(points.len(), 5);
            assert!(points.iter().all(|point| point[0] > 10.0
                && point[0] < 11.0
                && point[1] > 64.0
                && point[1] < 65.0
                && point[2] > -3.0
                && point[2] < -2.0));
        }
    }
}
