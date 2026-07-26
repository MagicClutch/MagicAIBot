use super::*;

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
fn no_space_is_reported_before_mutation() {
    let mut m = Mock::new(&[("minecraft:plank", 2)]);
    m.state.free_inventory_slots = 0;
    assert_eq!(
        execute(&mut m, &sticks(4), &Default::default(), &Default::default()).status,
        CraftStatus::NoSpace
    )
}
