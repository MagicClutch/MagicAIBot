use crate::{
    look::aim_point::BlockAimMode,
    minecraft::world_state::{BlockPosition, PositionSnapshot},
};

#[derive(Clone, Debug, PartialEq)]
pub enum LookTarget {
    Block {
        position: BlockPosition,
        block_id: Option<String>,
        aim_mode: BlockAimMode,
    },
    BlockFacePoint {
        position: BlockPosition,
        block_id: Option<String>,
        point: [f64; 3],
    },
    World(PositionSnapshot),
    Entity(u32),
    Player(String),
    #[allow(dead_code)]
    MovementDirection,
}

impl LookTarget {
    pub fn label(&self) -> String {
        match self {
            Self::Block {
                position, block_id, ..
            } => block_id
                .clone()
                .unwrap_or_else(|| format!("block {} {} {}", position.x, position.y, position.z)),
            Self::BlockFacePoint {
                position, block_id, ..
            } => block_id.clone().unwrap_or_else(|| {
                format!("block face {} {} {}", position.x, position.y, position.z)
            }),
            Self::World(position) => format!(
                "position ({:.1}, {:.1}, {:.1})",
                position.x, position.y, position.z
            ),
            Self::Entity(entity_id) => format!("entity {entity_id}"),
            Self::Player(name) => format!("player {name}"),
            Self::MovementDirection => "movement direction".into(),
        }
    }
}
