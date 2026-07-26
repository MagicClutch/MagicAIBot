use super::*;

fn inventory(items: &[(&str, u32)]) -> InventorySnapshot {
    InventorySnapshot {
        available: true,
        total_counts: items.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        ..Default::default()
    }
}

#[test]
fn loads_shaped_shapeless_and_version_source() {
    let book = RecipeBook::fallback().unwrap();
    assert_eq!(book.source().protocol, 776);
    assert!(!book.source().complete);
    assert!(matches!(
        book.recipe("stick").unwrap().layout,
        RecipeLayout::Shaped {
            width: 1,
            height: 2,
            ..
        }
    ));
    assert!(matches!(
        book.recipe("oak_planks").unwrap().layout,
        RecipeLayout::Shapeless { .. }
    ));
}

#[test]
fn availability_uses_alternatives_and_does_not_mutate_inventory() {
    let book = RecipeBook::fallback().unwrap();
    let inv = inventory(&[("minecraft:bamboo_planks", 4)]);
    let before = inv.total_counts.clone();
    let got = book.availability(book.recipe("stick").unwrap(), &inv, true, 2);
    assert_eq!(got.maximum_crafts, 2);
    assert!(got.missing.is_empty());
    assert_eq!(inv.total_counts, before);
}

#[test]
fn tag_and_missing_counts_are_reported() {
    let book = RecipeBook::fallback().unwrap();
    let inv = inventory(&[]);
    let got = book.availability(book.recipe("oak_planks").unwrap(), &inv, true, 2);
    assert_eq!(got.maximum_crafts, 0);
    assert_eq!(got.missing[0].count, 2);
    assert_eq!(got.missing[0].alternatives.len(), 8);
}

#[test]
fn grid_station_is_enforced() {
    let book = RecipeBook::fallback().unwrap();
    let inv = inventory(&[("minecraft:cobblestone", 3), ("minecraft:stick", 2)]);
    assert_eq!(
        book.availability(book.recipe("stone_pickaxe").unwrap(), &inv, false, 1)
            .failure,
        Some(Failure::StationUnavailable(Station::CraftingTable))
    );
}

#[test]
fn rounds_operations_and_consumes_intermediates_virtually() {
    let book = RecipeBook::fallback().unwrap();
    let inv = inventory(&[("minecraft:oak_log", 1), ("minecraft:coal", 2)]);
    let plan = book.plan("torch", 5, &inv, true, 8);
    assert_eq!(plan.failure, None);
    assert_eq!(plan.steps.last().unwrap().operations, 2);
    assert_eq!(plan.steps.last().unwrap().produced, 8);
    assert!(
        plan.steps
            .iter()
            .any(|s| s.output == "minecraft:oak_planks")
    );
    assert!(plan.steps.iter().any(|s| s.output == "minecraft:stick"));
}

fn custom(recipes: Vec<Recipe>) -> RecipeBook {
    let mut map = BTreeMap::new();
    let mut outputs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in recipes {
        outputs
            .entry(r.output.clone())
            .or_default()
            .push(r.id.clone());
        map.insert(r.id.clone(), r);
    }
    RecipeBook {
        source: SourceInfo {
            version: "test".into(),
            protocol: 0,
            revision: "test".into(),
            complete: true,
        },
        recipes: map,
        by_output: outputs,
        tags: BTreeMap::new(),
    }
}
fn unary(id: &str, output: &str, input: &str) -> Recipe {
    Recipe {
        id: id.into(),
        output: output.into(),
        output_count: 1,
        station: Station::Player,
        layout: RecipeLayout::Shapeless {
            ingredients: vec![IngredientSlot {
                ingredient: Ingredient::Item(input.into()),
                count: 1,
            }],
        },
        known: true,
        special: false,
    }
}

#[test]
fn detects_cycles_and_depth_limits() {
    let cycle = custom(vec![
        unary("minecraft:a", "minecraft:a", "minecraft:b"),
        unary("minecraft:b", "minecraft:b", "minecraft:a"),
    ]);
    assert!(matches!(
        cycle.plan("a", 1, &inventory(&[]), true, 8).failure,
        Some(Failure::Cycle { .. })
    ));
    let chain = custom(vec![
        unary("minecraft:a", "minecraft:a", "minecraft:b"),
        unary("minecraft:b", "minecraft:b", "minecraft:c"),
    ]);
    assert_eq!(
        chain.plan("a", 1, &inventory(&[]), true, 1).failure,
        Some(Failure::DepthLimit {
            item: "minecraft:b".into(),
            limit: 1
        })
    );
}

#[test]
fn failures_are_structured() {
    let book = RecipeBook::fallback().unwrap();
    assert_eq!(
        book.preferred("not_real"),
        Err(Failure::UnknownItem("minecraft:not_real".into()))
    );
}
