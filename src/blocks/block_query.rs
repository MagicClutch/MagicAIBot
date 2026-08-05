use crate::error::AppError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockSearchQuery {
    pub block_id: String,
    pub radius: u32,
    pub maximum_results: usize,
    pub vertical_range: u32,
}

pub fn normalize_block_id(input: &str) -> Result<String, AppError> {
    let input = input.trim().to_ascii_lowercase();
    if input.is_empty() {
        return Err(AppError::InvalidBlockIdentifier(
            "block identifier must not be empty".into(),
        ));
    }
    if input.chars().any(char::is_whitespace) {
        return Err(AppError::InvalidBlockIdentifier(
            "block identifier must not contain spaces".into(),
        ));
    }

    let normalized = if input.contains(':') {
        input
    } else {
        format!("minecraft:{input}")
    };
    let Some((namespace, path)) = normalized.split_once(':') else {
        return Err(AppError::InvalidBlockIdentifier(
            "block identifier must contain a valid namespace and path".into(),
        ));
    };
    if namespace.is_empty()
        || path.is_empty()
        || normalized.matches(':').count() != 1
        || !valid_identifier_part(namespace, true)
        || !valid_identifier_part(path, false)
    {
        return Err(AppError::InvalidBlockIdentifier(format!(
            "malformed block identifier: {normalized}"
        )));
    }
    if namespace != "minecraft" {
        return Err(AppError::UnknownBlockIdentifier(normalized));
    }

    normalized
        .parse::<azalea::registry::builtin::BlockKind>()
        .map_err(|_| AppError::UnknownBlockIdentifier(normalized.clone()))?;
    Ok(normalized)
}

pub(crate) fn valid_identifier_part(value: &str, namespace: bool) -> bool {
    value.chars().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || matches!(character, '_' | '-' | '.' | '/')
            || (!namespace && character == ':')
    })
}

pub fn validate_search_values(
    radius: u32,
    maximum_results: usize,
    vertical_range: u32,
    maximum_radius: u32,
    maximum_result_limit: usize,
) -> Result<(), AppError> {
    if radius == 0 || radius > maximum_radius {
        return Err(AppError::InvalidBlockSearchRadius {
            radius,
            maximum: maximum_radius,
        });
    }
    if maximum_results == 0 || maximum_results > maximum_result_limit {
        return Err(AppError::InvalidBlockSearchResultLimit {
            limit: maximum_results,
            maximum: maximum_result_limit,
        });
    }
    if vertical_range == 0 || vertical_range > 384 {
        return Err(AppError::InvalidBlockSearchVerticalRange(vertical_range));
    }
    Ok(())
}

#[must_use]
pub fn chunk_coordinate(block_coordinate: i32) -> i32 {
    block_coordinate.div_euclid(16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_validates_identifiers() {
        assert_eq!(
            normalize_block_id(" Oak_Log ").unwrap(),
            "minecraft:oak_log"
        );
        assert_eq!(
            normalize_block_id("minecraft:STONE").unwrap(),
            "minecraft:stone"
        );
    }

    #[test]
    fn rejects_malformed_and_unknown_identifiers() {
        for input in [
            "",
            "minecraft:",
            "minecraft:oak log",
            "minecraft:not_a_real_block",
        ] {
            assert!(normalize_block_id(input).is_err(), "{input}");
        }
    }

    #[test]
    fn validates_search_values() {
        assert!(validate_search_values(32, 20, 32, 128, 256).is_ok());
        assert!(validate_search_values(0, 20, 32, 128, 256).is_err());
        assert!(validate_search_values(32, 0, 32, 128, 256).is_err());
        assert!(validate_search_values(32, 20, 385, 128, 256).is_err());
    }

    #[test]
    fn handles_negative_chunk_boundaries() {
        assert_eq!(chunk_coordinate(0), 0);
        assert_eq!(chunk_coordinate(15), 0);
        assert_eq!(chunk_coordinate(16), 1);
        assert_eq!(chunk_coordinate(-1), -1);
        assert_eq!(chunk_coordinate(-16), -1);
        assert_eq!(chunk_coordinate(-17), -2);
    }
}
