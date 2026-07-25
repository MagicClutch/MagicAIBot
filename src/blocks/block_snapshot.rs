use crate::minecraft::world_state::BlockPosition;

#[derive(Clone, Debug, PartialEq)]
pub struct BlockSnapshot {
    pub position: BlockPosition,
    pub block_id: String,
    pub distance: f64,
    pub dimension: Option<String>,
    pub chunk_loaded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LoadedBlockCandidate {
    pub position: BlockPosition,
    pub block_id: String,
    pub distance_squared: f64,
    pub dimension: Option<String>,
    pub chunk_loaded: bool,
}

impl LoadedBlockCandidate {
    pub fn into_snapshot(self) -> BlockSnapshot {
        BlockSnapshot {
            position: self.position,
            block_id: self.block_id,
            distance: self.distance_squared.sqrt(),
            dimension: self.dimension,
            chunk_loaded: self.chunk_loaded,
        }
    }
}
