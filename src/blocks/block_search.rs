use crate::{
    blocks::{block_query::BlockSearchQuery, block_snapshot::BlockSnapshot},
    error::AppError,
    logging,
    minecraft::client::MinecraftClient,
};

#[derive(Clone, Debug)]
pub struct BlockSearchService {
    maximum_radius: u32,
    maximum_result_limit: usize,
    default_vertical_range: u32,
}

impl BlockSearchService {
    pub fn new(
        maximum_radius: u32,
        maximum_result_limit: usize,
        default_vertical_range: u32,
    ) -> Self {
        Self {
            maximum_radius,
            maximum_result_limit,
            default_vertical_range,
        }
    }

    pub async fn search_nearby(
        &self,
        minecraft: &MinecraftClient,
        mut query: BlockSearchQuery,
    ) -> Result<Vec<BlockSnapshot>, AppError> {
        if query.vertical_range == 0 {
            query.vertical_range = self.default_vertical_range;
        }
        crate::blocks::block_query::validate_search_values(
            query.radius,
            query.maximum_results,
            query.vertical_range,
            self.maximum_radius,
            self.maximum_result_limit,
        )?;
        logging::info(format!(
            "Searching for {} within {} blocks",
            query.block_id, query.radius
        ));
        let results = self.search_raw(minecraft, query.clone()).await?;
        let results = sort_deduplicate_limit(results, query.maximum_results);
        if results.is_empty() {
            logging::info(format!("No {} blocks found", query.block_id));
        } else {
            logging::success(format!("Found {} {} blocks", results.len(), query.block_id));
        }
        Ok(results)
    }

    pub(crate) async fn search_raw(
        &self,
        minecraft: &MinecraftClient,
        mut query: BlockSearchQuery,
    ) -> Result<Vec<BlockSnapshot>, AppError> {
        if query.vertical_range == 0 {
            query.vertical_range = self.default_vertical_range;
        }
        crate::blocks::block_query::validate_search_values(
            query.radius,
            query.maximum_results,
            query.vertical_range,
            self.maximum_radius,
            self.maximum_result_limit,
        )?;
        let results = minecraft
            .scan_loaded_blocks(&query)
            .await?
            .into_iter()
            .map(|candidate| candidate.into_snapshot())
            .collect::<Vec<_>>();
        Ok(sort_deduplicate_limit(results, query.maximum_results))
    }
}

pub fn format_find_results(block_id: &str, radius: u32, results: &[BlockSnapshot]) -> String {
    if results.is_empty() {
        return format!("No loaded {block_id} blocks found within {radius} blocks.");
    }
    let mut output = format!("Found {} {block_id} blocks:\n", results.len());
    for (index, result) in results.iter().enumerate() {
        output.push_str(&format!(
            "\n{}. ({}, {}, {}) | {:.1} blocks",
            index + 1,
            result.position.x,
            result.position.y,
            result.position.z,
            result.distance
        ));
    }
    output
}

pub fn format_nearest_result(
    block_id: &str,
    radius: u32,
    result: Option<&BlockSnapshot>,
) -> String {
    let Some(result) = result else {
        return format!("No loaded {block_id} block found within {radius} blocks.");
    };
    format!(
        "Nearest {block_id}:\nPosition: {} {} {}\nDistance: {:.1} blocks",
        result.position.x, result.position.y, result.position.z, result.distance
    )
}

pub fn sort_deduplicate_limit(
    mut results: Vec<BlockSnapshot>,
    maximum_results: usize,
) -> Vec<BlockSnapshot> {
    // Distance alone is not a total ordering.  A stable coordinate tie-break is
    // important to callers which retry a search (notably bounded mining): the
    // same loaded-world snapshot must produce the same target sequence.
    results.sort_by(|left, right| {
        left.distance.total_cmp(&right.distance).then_with(|| {
            (left.position.x, left.position.y, left.position.z).cmp(&(
                right.position.x,
                right.position.y,
                right.position.z,
            ))
        })
    });
    results.dedup_by_key(|result| result.position);
    results.truncate(maximum_results);
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::world_state::BlockPosition;

    fn block(x: i32, distance: f64) -> BlockSnapshot {
        BlockSnapshot {
            position: BlockPosition { x, y: 64, z: 0 },
            block_id: "minecraft:stone".into(),
            distance,
            dimension: Some("minecraft:overworld".into()),
            chunk_loaded: true,
        }
    }

    #[test]
    fn formats_results_and_empty_results() {
        let results = vec![block(-1, 5.44)];
        assert!(format_find_results("minecraft:stone", 32, &results).contains("5.4 blocks"));
        assert_eq!(
            format_nearest_result("minecraft:stone", 32, None),
            "No loaded minecraft:stone block found within 32 blocks."
        );
    }

    #[test]
    fn sorts_deduplicates_and_limits_results() {
        let results = sort_deduplicate_limit(vec![block(2, 8.0), block(1, 2.0), block(1, 2.0)], 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].position.x, 1);
    }
}
