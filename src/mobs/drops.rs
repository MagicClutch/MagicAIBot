//! Static resource -> mob registry for `#get`'s mob-farming path. This is the
//! single source of truth for "which mob drops this item" -- extending
//! supported mobs/drops means adding or editing a row here; nothing else in
//! the mob-farming path needs to change.
//!
//! Every drop id here must be the actual Minecraft item id, not a guessed
//! "display name -> snake_case" transform -- vanilla drops the "raw_" prefix
//! entirely for meat (`beef`, `porkchop`, `mutton`, `chicken`, `rabbit`, not
//! `raw_beef`/`raw_porkchop`/...) and renamed fish items at the 1.13
//! flattening (`cod`/`salmon`, not `fish`/`raw_cod`/`raw_salmon`). A wrong id
//! here does not fail to compile or even fail a search -- it silently makes
//! `#get` mine or hunt forever, since `InventorySnapshot::count_item` can
//! never find an item the server never reports. `items::audit_registered_items`
//! (called at startup) checks every id in this table against Azalea's item
//! registry specifically to catch this class of mistake.

/// `(mob entity id, drop item ids)`. Declared in this fixed order so a
/// resource listed under more than one mob (`spider_eye`, dropped by both
/// Spider and Witch) resolves to a deterministic mob rather than depending on
/// hash-map iteration order.
const MOB_DROPS: &[(&str, &[&str])] = &[
    ("minecraft:cow", &["minecraft:leather", "minecraft:beef"]),
    ("minecraft:pig", &["minecraft:porkchop"]),
    (
        "minecraft:sheep",
        &[
            "minecraft:mutton",
            // No generic "wool" item/block id exists since the 1.13
            // flattening -- every color is its own id (`white_wool`, ...).
            // `#get wool 10` is correctly rejected as unknown; a caller must
            // name a color.
            "minecraft:white_wool",
            "minecraft:black_wool",
            "minecraft:gray_wool",
            "minecraft:light_gray_wool",
            "minecraft:brown_wool",
        ],
    ),
    (
        "minecraft:chicken",
        &["minecraft:chicken", "minecraft:feather"],
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
    ("minecraft:turtle", &["minecraft:turtle_scute"]),
    (
        "minecraft:polar_bear",
        &["minecraft:cod", "minecraft:salmon"],
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

/// Every item id referenced by [`MOB_DROPS`], for startup registry
/// validation (`items::audit_registered_items`).
pub(crate) fn registered_drop_items() -> impl Iterator<Item = &'static str> {
    MOB_DROPS
        .iter()
        .flat_map(|(_, drops)| drops.iter().copied())
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
        assert_eq!(
            mob_for_resource("minecraft:white_wool"),
            Some("minecraft:sheep")
        );
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
            mob_for_resource("minecraft:turtle_scute"),
            Some("minecraft:turtle")
        );
        assert_eq!(
            mob_for_resource("minecraft:cod"),
            Some("minecraft:polar_bear")
        );
        assert_eq!(
            mob_for_resource("minecraft:salmon"),
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
