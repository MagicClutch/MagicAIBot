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
    /// Like [`Self::Player`], but leads the aim slightly ahead of the
    /// player's current position based on their live velocity, so the
    /// crosshair anticipates a moving target instead of always trailing
    /// slightly behind it. Used by `crate::combat` for PvP; falls back to
    /// exactly [`Self::Player`]'s behavior when velocity is unavailable.
    PredictedPlayer(String),
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
            Self::Player(name) | Self::PredictedPlayer(name) => format!("player {name}"),
            Self::MovementDirection => "movement direction".into(),
        }
    }
}
