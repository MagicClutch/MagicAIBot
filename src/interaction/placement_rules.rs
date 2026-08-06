pub fn is_air(id: Option<&str>) -> bool {
    matches!(
        id,
        Some("minecraft:air" | "minecraft:cave_air" | "minecraft:void_air")
    )
}
pub fn is_replaceable(id: Option<&str>) -> bool {
    is_air(id)
        || matches!(
            id,
            Some(
                "minecraft:grass"
                    | "minecraft:tall_grass"
                    | "minecraft:fern"
                    | "minecraft:large_fern"
                    | "minecraft:snow"
            )
        )
}
pub fn has_support(id: Option<&str>) -> bool {
    id.is_some_and(|id| {
        !is_replaceable(Some(id)) && id != "minecraft:water" && id != "minecraft:lava"
    })
}
/// Blocks that must never be selected as a mining target or attempted with
/// a break packet, regardless of how they were found (block search,
/// `/mine`/`#mine`, `/break` on whatever's directly looked at, ...) --
/// checked once here rather than in every caller. Survival-mode players
/// can't break these at all; attempting it just wastes a verification
/// timeout and a retry cycle before failing anyway.
pub fn is_unbreakable(id: Option<&str>) -> bool {
    matches!(id, Some("minecraft:bedrock"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_support_and_replaceable_blocks() {
        assert!(is_replaceable(Some("minecraft:air")));
        assert!(has_support(Some("minecraft:stone")));
        assert!(!has_support(Some("minecraft:water")));
    }

    #[test]
    fn bedrock_is_unbreakable_and_nothing_else_is() {
        assert!(is_unbreakable(Some("minecraft:bedrock")));
        assert!(!is_unbreakable(Some("minecraft:stone")));
        assert!(!is_unbreakable(None));
    }
}
