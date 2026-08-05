//! Static resource -> mob registry for `#get`'s mob-farming path. This is the
//! single source of truth for "which mob drops this item" -- extending
//! supported mobs/drops means adding or editing a row here; nothing else in
//! the mob-farming path needs to change.

/// `(mob entity id, drop item ids)`. Declared in this fixed order so a
/// resource listed under more than one mob (`spider_eye`, dropped by both
/// Spider and Witch) resolves to a deterministic mob rather than depending on
/// hash-map iteration order.
const MOB_DROPS: &[(&str, &[&str])] = &[
    (
        "minecraft:cow",
        &["minecraft:leather", "minecraft:beef", "minecraft:raw_beef"],
    ),
    (
        "minecraft:pig",
        &["minecraft:porkchop", "minecraft:raw_porkchop"],
    ),
    (
        "minecraft:sheep",
        &[
            "minecraft:mutton",
            "minecraft:raw_mutton",
            "minecraft:wool",
            "minecraft:white_wool",
            "minecraft:black_wool",
            "minecraft:gray_wool",
            "minecraft:light_gray_wool",
            "minecraft:brown_wool",
        ],
    ),
    (
        "minecraft:chicken",
        &[
            "minecraft:chicken",
            "minecraft:raw_chicken",
            "minecraft:feather",
        ],
    ),
    (
        "minecraft:skeleton",
        &["minecraft:bone", "minecraft:bone_meal", "minecraft:arrow"],
    ),
    (
        "minecraft:spider",
        &["minecraft:string", "minecraft:spider_eye"],
    ),
    ("minecraft:creeper", &["minecraft:gunpowder"]),
    ("minecraft:enderman", &["minecraft:ender_pearl"]),
    (
        "minecraft:rabbit",
        &[
            "minecraft:rabbit",
            "minecraft:raw_rabbit",
            "minecraft:rabbit_hide",
            "minecraft:rabbit_foot",
        ],
    ),
    (
        "minecraft:zombie",
        &[
            "minecraft:rotten_flesh",
            "minecraft:potato",
            "minecraft:carrot",
            "minecraft:iron_ingot",
        ],
    ),
    (
        "minecraft:witch",
        &[
            "minecraft:redstone",
            "minecraft:glowstone_dust",
            "minecraft:sugar",
            "minecraft:glass_bottle",
            "minecraft:spider_eye",
        ],
    ),
    ("minecraft:blaze", &["minecraft:blaze_rod"]),
    ("minecraft:magma_cube", &["minecraft:magma_cream"]),
    ("minecraft:slime", &["minecraft:slime_ball"]),
    ("minecraft:turtle", &["minecraft:scute"]),
    (
        "minecraft:polar_bear",
        &[
            "minecraft:fish",
            "minecraft:raw_cod",
            "minecraft:raw_salmon",
        ],
    ),
];

/// Looks up which mob (its entity id, e.g. `minecraft:cow`) drops
/// `resource_id` (already normalized to `minecraft:...`). `None` means this
/// resource is not a known mob drop -- callers fall back to treating it as a
/// block (see `super::resolve_resource`).
pub fn mob_for_resource(resource_id: &str) -> Option<&'static str> {
    MOB_DROPS
        .iter()
        .find(|(_, drops)| drops.contains(&resource_id))
        .map(|(mob, _)| *mob)
}

/// Bare (non-namespaced) mob name for concise console output, e.g. `cow` from
/// `minecraft:cow`.
pub fn mob_label(mob_id: &str) -> &str {
    mob_id.rsplit(':').next().unwrap_or(mob_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_documented_resource_to_mob_mappings() {
        assert_eq!(mob_for_resource("minecraft:leather"), Some("minecraft:cow"));
        assert_eq!(mob_for_resource("minecraft:beef"), Some("minecraft:cow"));
        assert_eq!(
            mob_for_resource("minecraft:porkchop"),
            Some("minecraft:pig")
        );
        assert_eq!(
            mob_for_resource("minecraft:mutton"),
            Some("minecraft:sheep")
        );
        assert_eq!(mob_for_resource("minecraft:wool"), Some("minecraft:sheep"));
        assert_eq!(
            mob_for_resource("minecraft:feather"),
            Some("minecraft:chicken")
        );
        assert_eq!(
            mob_for_resource("minecraft:bone"),
            Some("minecraft:skeleton")
        );
        assert_eq!(
            mob_for_resource("minecraft:arrow"),
            Some("minecraft:skeleton")
        );
        assert_eq!(
            mob_for_resource("minecraft:string"),
            Some("minecraft:spider")
        );
        assert_eq!(
            mob_for_resource("minecraft:gunpowder"),
            Some("minecraft:creeper")
        );
        assert_eq!(
            mob_for_resource("minecraft:ender_pearl"),
            Some("minecraft:enderman")
        );
        assert_eq!(
            mob_for_resource("minecraft:rabbit_foot"),
            Some("minecraft:rabbit")
        );
        assert_eq!(
            mob_for_resource("minecraft:rotten_flesh"),
            Some("minecraft:zombie")
        );
        assert_eq!(
            mob_for_resource("minecraft:blaze_rod"),
            Some("minecraft:blaze")
        );
        assert_eq!(
            mob_for_resource("minecraft:magma_cream"),
            Some("minecraft:magma_cube")
        );
        assert_eq!(
            mob_for_resource("minecraft:slime_ball"),
            Some("minecraft:slime")
        );
        assert_eq!(
            mob_for_resource("minecraft:scute"),
            Some("minecraft:turtle")
        );
        assert_eq!(
            mob_for_resource("minecraft:raw_cod"),
            Some("minecraft:polar_bear")
        );
    }

    #[test]
    fn ambiguous_drop_resolves_to_the_first_declared_mob() {
        // spider_eye is dropped by both Spider and Witch; Spider is declared
        // first so it wins deterministically.
        assert_eq!(
            mob_for_resource("minecraft:spider_eye"),
            Some("minecraft:spider")
        );
    }

    #[test]
    fn unrelated_resource_is_not_a_mob_drop() {
        assert_eq!(mob_for_resource("minecraft:oak_log"), None);
        assert_eq!(mob_for_resource("minecraft:diamond"), None);
    }

    #[test]
    fn mob_label_strips_the_namespace() {
        assert_eq!(mob_label("minecraft:cow"), "cow");
        assert_eq!(mob_label("cow"), "cow");
    }
}
