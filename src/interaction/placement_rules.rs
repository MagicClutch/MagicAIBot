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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_support_and_replaceable_blocks() {
        assert!(is_replaceable(Some("minecraft:air")));
        assert!(has_support(Some("minecraft:stone")));
        assert!(!has_support(Some("minecraft:water")));
    }
}
