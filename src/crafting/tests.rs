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

struct Mock {
    state: MenuSnapshot,
    table: Result<(), DriverError>,
    fail_take: Option<DriverError>,
    cancel_after_place: Option<CraftCancellation>,
}
impl Mock {
    fn new(items: &[(&str, u32)]) -> Self {
        Self {
            state: MenuSnapshot {
                revision: 1,
                inventory: items.iter().map(|(i, n)| (i.to_string(), *n)).collect(),
                free_inventory_slots: 10,
                alive: true,
                connected: true,
                ..Default::default()
            },
            table: Ok(()),
            fail_take: None,
            cancel_after_place: None,
        }
    }
    fn bump(&mut self) {
        self.state.revision += 1
    }
}
impl CraftingDriver for Mock {
    fn snapshot(&mut self) -> MenuSnapshot {
        self.state.clone()
    }
    fn open_player_menu(&mut self) -> Result<MenuSnapshot, DriverError> {
        self.state.menu = Some(CraftingMenu::Player);
        self.state.grid = vec![None; 4];
        self.bump();
        Ok(self.state.clone())
    }
    fn open_known_table(&mut self, _: u32, navigate: bool) -> Result<MenuSnapshot, DriverError> {
        self.table?;
        if !navigate {
            return Err(DriverError::Rejected);
        }
        self.state.menu = Some(CraftingMenu::Table);
        self.state.grid = vec![None; 9];
        self.bump();
        Ok(self.state.clone())
    }
    fn place_one(&mut self, slot: usize, item: &str, _: u64) -> Result<MenuSnapshot, DriverError> {
        let n = self
            .state
            .inventory
            .get_mut(item)
            .ok_or(DriverError::Rejected)?;
        if *n == 0 {
            return Err(DriverError::Rejected);
        }
        *n -= 1;
        self.state.grid[slot] = Some(item.into());
        self.bump();
        if let Some(c) = &self.cancel_after_place {
            c.cancel()
        }
        Ok(self.state.clone())
    }
    fn take_output(&mut self, item: &str, _: u64) -> Result<MenuSnapshot, DriverError> {
        if let Some(e) = self.fail_take.take() {
            return Err(e);
        };
        for x in &mut self.state.grid {
            *x = None
        }
        *self.state.inventory.entry(item.into()).or_default() += 4;
        self.bump();
        Ok(self.state.clone())
    }
    fn recover(&mut self, _: u64) -> Result<MenuSnapshot, DriverError> {
        for x in &mut self.state.grid {
            if let Some(i) = x.take() {
                *self.state.inventory.entry(i).or_default() += 1
            }
        }
        self.state.cursor = None;
        self.bump();
        Ok(self.state.clone())
    }
}
fn sticks(count: u32) -> ResolvedCraftPlan {
    ResolvedCraftPlan {
        recipe_id: "minecraft:stick".into(),
        output_item: "minecraft:stick".into(),
        output_per_craft: 4,
        requested_output: count,
        shaped: true,
        placements: vec![
            IngredientPlacement {
                item: "minecraft:plank".into(),
                row: 0,
                column: 0,
            },
            IngredientPlacement {
                item: "minecraft:plank".into(),
                row: 1,
                column: 0,
            },
        ],
    }
}
#[test]
fn player_shaped_and_no_overcraft_operations() {
    let mut m = Mock::new(&[("minecraft:plank", 4)]);
    let r = execute(&mut m, &sticks(8), &Default::default(), &Default::default());
    assert_eq!(r.status, CraftStatus::Completed);
    assert_eq!((r.crafted, r.completed_operations), (8, 2));
    assert_eq!(r.menu, Some(CraftingMenu::Player));
}
#[test]
fn missing_materials_never_opens_menu() {
    let mut m = Mock::new(&[("minecraft:plank", 1)]);
    let r = execute(&mut m, &sticks(4), &Default::default(), &Default::default());
    assert_eq!(r.status, CraftStatus::MissingMaterials);
    assert_eq!(r.missing["minecraft:plank"], 1);
    assert_eq!(m.state.menu, None);
}
#[test]
fn shapeless_layout_is_row_major() {
    let p = ResolvedCraftPlan {
        shaped: false,
        placements: vec![
            IngredientPlacement {
                item: "a".into(),
                row: 0,
                column: 0,
            },
            IngredientPlacement {
                item: "b".into(),
                row: 0,
                column: 1,
            },
            IngredientPlacement {
                item: "c".into(),
                row: 1,
                column: 0,
            },
        ],
        recipe_id: "r".into(),
        output_item: "out".into(),
        output_per_craft: 4,
        requested_output: 4,
    };
    let mut m = Mock::new(&[("a", 1), ("b", 1), ("c", 1)]);
    assert_eq!(
        execute(&mut m, &p, &Default::default(), &Default::default()).status,
        CraftStatus::Completed
    );
}
#[test]
fn three_by_three_navigates_to_table() {
    let mut p = sticks(4);
    p.placements.push(IngredientPlacement {
        item: "minecraft:plank".into(),
        row: 2,
        column: 2,
    });
    let mut m = Mock::new(&[("minecraft:plank", 3)]);
    let r = execute(&mut m, &p, &Default::default(), &Default::default());
    assert_eq!(r.menu, Some(CraftingMenu::Table));
}
#[test]
fn table_not_allowed_is_wrong_menu() {
    let mut p = sticks(4);
    p.placements.push(IngredientPlacement {
        item: "minecraft:plank".into(),
        row: 2,
        column: 2,
    });
    let mut m = Mock::new(&[("minecraft:plank", 3)]);
    let o = CraftingOptions {
        allow_table: false,
        ..Default::default()
    };
    assert_eq!(
        execute(&mut m, &p, &o, &Default::default()).status,
        CraftStatus::WrongMenu
    );
}
#[test]
fn partial_result_and_cleanup() {
    let mut m = Mock::new(&[("minecraft:plank", 4)]);
    let mut calls = 0;
    struct Twice<'a> {
        m: &'a mut Mock,
        c: &'a mut u8,
    }
    impl CraftingDriver for Twice<'_> {
        fn snapshot(&mut self) -> MenuSnapshot {
            self.m.snapshot()
        }
        fn open_player_menu(&mut self) -> Result<MenuSnapshot, DriverError> {
            self.m.open_player_menu()
        }
        fn open_known_table(&mut self, r: u32, n: bool) -> Result<MenuSnapshot, DriverError> {
            self.m.open_known_table(r, n)
        }
        fn place_one(&mut self, s: usize, i: &str, r: u64) -> Result<MenuSnapshot, DriverError> {
            self.m.place_one(s, i, r)
        }
        fn take_output(&mut self, i: &str, r: u64) -> Result<MenuSnapshot, DriverError> {
            *self.c += 1;
            if *self.c == 2 {
                Err(DriverError::TimedOut)
            } else {
                self.m.take_output(i, r)
            }
        }
        fn recover(&mut self, r: u64) -> Result<MenuSnapshot, DriverError> {
            self.m.recover(r)
        }
    }
    let r = execute(
        &mut Twice {
            m: &mut m,
            c: &mut calls,
        },
        &sticks(8),
        &Default::default(),
        &Default::default(),
    );
    assert_eq!(r.status, CraftStatus::Partial);
    assert_eq!(r.crafted, 4);
    assert!(r.recovered);
    assert!(r.grid.iter().all(Option::is_none));
}
#[test]
fn cancellation_recovers_grid() {
    let token = CraftCancellation::default();
    let mut m = Mock::new(&[("minecraft:plank", 4)]);
    m.cancel_after_place = Some(token.clone());
    let r = execute(&mut m, &sticks(8), &Default::default(), &token);
    assert_eq!(r.status, CraftStatus::Cancelled);
    assert!(r.recovered);
}
#[test]
fn output_without_revision_is_rejected() {
    let m = Mock::new(&[("minecraft:plank", 2)]);
    struct Bad(Mock);
    impl CraftingDriver for Bad {
        fn snapshot(&mut self) -> MenuSnapshot {
            self.0.snapshot()
        }
        fn open_player_menu(&mut self) -> Result<MenuSnapshot, DriverError> {
            self.0.open_player_menu()
        }
        fn open_known_table(&mut self, r: u32, n: bool) -> Result<MenuSnapshot, DriverError> {
            self.0.open_known_table(r, n)
        }
        fn place_one(&mut self, s: usize, i: &str, r: u64) -> Result<MenuSnapshot, DriverError> {
            self.0.place_one(s, i, r)
        }
        fn take_output(&mut self, _: &str, _: u64) -> Result<MenuSnapshot, DriverError> {
            Ok(self.0.state.clone())
        }
        fn recover(&mut self, r: u64) -> Result<MenuSnapshot, DriverError> {
            self.0.recover(r)
        }
    }
    assert_eq!(
        execute(
            &mut Bad(m),
            &sticks(4),
            &Default::default(),
            &Default::default()
        )
        .status,
        CraftStatus::Rejected
    );
}
#[test]
fn terminal_driver_results_are_structured() {
    for (e, s) in [
        (DriverError::TimedOut, CraftStatus::TimedOut),
        (DriverError::Died, CraftStatus::Died),
        (DriverError::Disconnected, CraftStatus::Disconnected),
        (DriverError::WrongMenu, CraftStatus::WrongMenu),
        (DriverError::Cancelled, CraftStatus::Cancelled),
        (DriverError::NoSpace, CraftStatus::NoSpace),
    ] {
        let mut m = Mock::new(&[("minecraft:plank", 2)]);
        m.table = Err(e);
        let mut p = sticks(4);
        p.placements.push(IngredientPlacement {
            item: "minecraft:plank".into(),
            row: 2,
            column: 0,
        });
        m.state.inventory.insert("minecraft:plank".into(), 3);
        assert_eq!(
            execute(&mut m, &p, &Default::default(), &Default::default()).status,
            s
        )
    }
}
#[test]
fn resolve_grid_places_shaped_ingredients_by_recipe_shape() {
    let book = RecipeBook::fallback().unwrap();
    let inv = inventory(&[("minecraft:oak_planks", 4)]);
    let plan = book.resolve_grid("stick", 4, &inv).unwrap();
    assert_eq!(plan.recipe_id, "minecraft:stick");
    assert_eq!(plan.output_item, "minecraft:stick");
    assert_eq!(plan.requested_output, 4);
    assert_eq!(plan.grid_size(), 2);
    assert_eq!(plan.placements.len(), 2);
    assert!(
        plan.placements
            .iter()
            .all(|p| p.item == "minecraft:oak_planks" && p.column == 0)
    );
    let rows: Vec<u8> = plan.placements.iter().map(|p| p.row).collect();
    assert!(rows.contains(&0) && rows.contains(&1));
}

#[test]
fn resolve_grid_reports_table_sized_recipes() {
    let book = RecipeBook::fallback().unwrap();
    let inv = inventory(&[("minecraft:cobblestone", 3), ("minecraft:stick", 2)]);
    let plan = book.resolve_grid("stone_pickaxe", 1, &inv).unwrap();
    assert_eq!(plan.grid_size(), 3);
}

#[test]
fn resolve_grid_prefers_ingredients_already_in_stock() {
    let book = RecipeBook::fallback().unwrap();
    // oak_planks is shapeless over a log-family tag; give the bot spruce
    // logs specifically and confirm the resolver picks that concrete item
    // instead of an arbitrary alternative it doesn't actually have.
    let inv = inventory(&[("minecraft:spruce_log", 4)]);
    let plan = book.resolve_grid("oak_planks", 4, &inv).unwrap();
    assert!(
        plan.placements
            .iter()
            .all(|p| p.item == "minecraft:spruce_log")
    );
}

#[test]
fn resolve_grid_fails_for_unknown_item() {
    let book = RecipeBook::fallback().unwrap();
    assert_eq!(
        book.resolve_grid("not_real", 1, &inventory(&[]))
            .unwrap_err(),
        Failure::UnknownItem("minecraft:not_real".into())
    );
}

fn disconnected_client() -> crate::minecraft::client::MinecraftClient {
    crate::minecraft::client::MinecraftClient::new(
        crate::config::MinecraftConfig {
            server: "localhost:25565".to_owned(),
            username: "MagicBot".to_owned(),
            account_mode: crate::config::AccountMode::Offline,
        },
        crate::config::ReconnectConfig {
            enabled: false,
            delay_seconds: 10,
            maximum_attempts: 5,
        },
        crate::config::ConsoleConfig::default(),
        crate::config::WorldStateConfig::default(),
        crate::config::VerticalNavigationConfig::default(),
        crate::config::BridgingConfig::default(),
    )
}

#[tokio::test]
async fn craft_reports_missing_materials_without_a_live_connection() {
    let service = CraftService::default();
    let recipes = RecipeBook::fallback().unwrap();
    let client = disconnected_client();
    let error = service
        .craft(&client, &recipes, "minecraft:stick", 4)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("missing"));
    // The failed attempt must leave the service idle for the next command.
    assert!(!service.status().active);
}

#[tokio::test]
async fn craft_rejects_recipes_that_need_a_crafting_table() {
    let service = CraftService::default();
    let recipes = RecipeBook::fallback().unwrap();
    let client = disconnected_client();
    let error = service
        .craft(&client, &recipes, "minecraft:stone_pickaxe", 1)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("crafting table"));
}

#[test]
fn stop_on_an_idle_craft_service_is_a_no_op() {
    let service = CraftService::default();
    assert!(!service.stop());
}

#[test]
fn no_space_is_reported_before_mutation() {
    let mut m = Mock::new(&[("minecraft:plank", 2)]);
    m.state.free_inventory_slots = 0;
    assert_eq!(
        execute(&mut m, &sticks(4), &Default::default(), &Default::default()).status,
        CraftStatus::NoSpace
    )
}
