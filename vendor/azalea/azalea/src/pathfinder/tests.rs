use std::{
    collections::HashSet,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use azalea_block::{BlockState, blocks::CobblestoneSlab, properties::SlabKind};
use azalea_client::{ClientMovementState, interact::StartUseItemEvent};
use azalea_core::{
    direction::Direction,
    position::{BlockPos, ChunkPos},
    tick::GameTick,
};
use azalea_entity::{LookDirection, inventory::Inventory};
use azalea_inventory::ItemStack;
use azalea_registry::builtin::{BlockKind, ItemKind};
use azalea_world::{Chunk, ChunkStorage, PartialChunkStorage};
use bevy_ecs::prelude::*;

use super::{
    GotoEvent, PathfinderSystems,
    astar::PathfinderTimeout,
    custom_state::CustomPathfinderState,
    goals::BlockPosGoal,
    moves,
    policy::PathfindingPolicy,
    simulation::{SimulatedPlayerBundle, Simulation},
    vertical::ScaffoldPolicy,
};
use crate::pathfinder::goto_event::PathfinderOpts;

fn setup_blockposgoal_simulation(
    partial_chunks: &mut PartialChunkStorage,
    start_pos: BlockPos,
    end_pos: BlockPos,
    solid_blocks: &[BlockPos],
) -> Simulation {
    let mut simulation = setup_simulation_world(partial_chunks, start_pos, solid_blocks, &[]);

    // you can uncomment this while debugging tests to get trace logs
    // simulation.app.add_plugins(bevy_log::LogPlugin {
    //     level: bevy_log::Level::TRACE,
    //     filter: "".to_owned(),
    //     ..Default::default()
    // });

    simulation.app.world_mut().write_message(GotoEvent {
        entity: simulation.entity,
        goal: Arc::new(BlockPosGoal(end_pos)),
        opts: PathfinderOpts {
            successors_fn: moves::default_move,
            allow_mining: false,
            retry_on_no_path: true,
            min_timeout: PathfinderTimeout::Nodes(1_000_000),
            max_timeout: PathfinderTimeout::Nodes(5_000_000),
        },
    });
    simulation
}

fn setup_simulation_world(
    partial_chunks: &mut PartialChunkStorage,
    start_pos: BlockPos,
    solid_blocks: &[BlockPos],
    extra_blocks: &[(BlockPos, BlockState)],
) -> Simulation {
    let mut chunk_positions = HashSet::new();
    for block_pos in solid_blocks {
        chunk_positions.insert(ChunkPos::from(block_pos));
    }
    for (block_pos, _) in extra_blocks {
        chunk_positions.insert(ChunkPos::from(block_pos));
    }

    let mut chunks = ChunkStorage::default();
    for chunk_pos in chunk_positions {
        partial_chunks.set(&chunk_pos, Some(Chunk::default()), &mut chunks);
    }
    for block_pos in solid_blocks {
        chunks.set_block_state(*block_pos, BlockKind::Stone.into());
    }
    for (block_pos, block_state) in extra_blocks {
        chunks.set_block_state(*block_pos, *block_state);
    }

    let player = SimulatedPlayerBundle::new(start_pos.center_bottom());
    Simulation::new(chunks, player)
}

pub fn assert_simulation_reaches(simulation: &mut Simulation, ticks: usize, end_pos: BlockPos) {
    wait_until_bot_starts_moving(simulation);
    for _ in 0..ticks {
        simulation.tick();
    }
    assert_eq!(BlockPos::from(simulation.position()), end_pos);
}

pub fn wait_until_bot_starts_moving(simulation: &mut Simulation) {
    let start_pos = simulation.position();
    let start_time = Instant::now();
    while simulation.position() == start_pos
        && !simulation.is_mining()
        && start_time.elapsed() < Duration::from_millis(5000)
    {
        simulation.tick();
        thread::yield_now();
    }
}

#[test]
fn test_simple_forward() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_blockposgoal_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(0, 71, 1),
        &[BlockPos::new(0, 70, 0), BlockPos::new(0, 70, 1)],
    );
    assert_simulation_reaches(&mut simulation, 20, BlockPos::new(0, 71, 1));
}

#[test]
fn test_double_diagonal_with_walls() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_blockposgoal_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(2, 71, 2),
        &[
            BlockPos::new(0, 70, 0),
            BlockPos::new(1, 70, 1),
            BlockPos::new(2, 70, 2),
            BlockPos::new(1, 72, 0),
            BlockPos::new(2, 72, 1),
        ],
    );
    assert_simulation_reaches(&mut simulation, 30, BlockPos::new(2, 71, 2));
}

#[test]
fn test_jump_with_sideways_momentum() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_blockposgoal_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 3),
        BlockPos::new(5, 76, 0),
        &[
            BlockPos::new(0, 70, 3),
            BlockPos::new(0, 70, 2),
            BlockPos::new(0, 70, 1),
            BlockPos::new(0, 70, 0),
            BlockPos::new(1, 71, 0),
            BlockPos::new(2, 72, 0),
            BlockPos::new(3, 73, 0),
            BlockPos::new(4, 74, 0),
            BlockPos::new(5, 75, 0),
        ],
    );
    assert_simulation_reaches(&mut simulation, 120, BlockPos::new(5, 76, 0));
}

#[test]
fn test_parkour_2_block_gap() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_blockposgoal_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(0, 71, 3),
        &[BlockPos::new(0, 70, 0), BlockPos::new(0, 70, 3)],
    );
    assert_simulation_reaches(&mut simulation, 40, BlockPos::new(0, 71, 3));
}

#[test]
fn test_descend_and_parkour_2_block_gap() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_blockposgoal_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(3, 67, 4),
        &[
            BlockPos::new(0, 70, 0),
            BlockPos::new(0, 69, 1),
            BlockPos::new(0, 68, 2),
            BlockPos::new(0, 67, 3),
            BlockPos::new(0, 66, 4),
            BlockPos::new(3, 66, 4),
        ],
    );
    assert_simulation_reaches(&mut simulation, 100, BlockPos::new(3, 67, 4));
}

#[test]
fn test_small_descend_and_parkour_2_block_gap() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_blockposgoal_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(0, 70, 5),
        &[
            BlockPos::new(0, 70, 0),
            BlockPos::new(0, 70, 1),
            BlockPos::new(0, 69, 2),
            BlockPos::new(0, 69, 5),
        ],
    );
    assert_simulation_reaches(&mut simulation, 40, BlockPos::new(0, 70, 5));
}

#[test]
fn test_quickly_descend() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_blockposgoal_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(0, 68, 3),
        &[
            BlockPos::new(0, 70, 0),
            BlockPos::new(0, 69, 1),
            BlockPos::new(0, 68, 2),
            BlockPos::new(0, 67, 3),
        ],
    );
    assert_simulation_reaches(&mut simulation, 60, BlockPos::new(0, 68, 3));
}

#[test]
fn test_2_gap_ascend_thrice() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_blockposgoal_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(3, 74, 0),
        &[
            BlockPos::new(0, 70, 0),
            BlockPos::new(0, 71, 3),
            BlockPos::new(3, 72, 3),
            BlockPos::new(3, 73, 0),
        ],
    );
    assert_simulation_reaches(&mut simulation, 60, BlockPos::new(3, 74, 0));
}

#[test]
fn test_consecutive_3_gap_parkour() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_blockposgoal_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(4, 71, 12),
        &[
            BlockPos::new(0, 70, 0),
            BlockPos::new(0, 70, 4),
            BlockPos::new(0, 70, 8),
            BlockPos::new(0, 70, 12),
            BlockPos::new(4, 70, 12),
        ],
    );
    assert_simulation_reaches(&mut simulation, 80, BlockPos::new(4, 71, 12));
}

#[test]
fn test_jumps_with_more_sideways_momentum() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_blockposgoal_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(4, 74, 9),
        &[
            BlockPos::new(0, 70, 0),
            BlockPos::new(0, 70, 1),
            BlockPos::new(0, 70, 2),
            BlockPos::new(0, 71, 3),
            BlockPos::new(0, 72, 6),
            BlockPos::new(0, 73, 9),
            // this is the point where the bot might fall if it has too much momentum
            BlockPos::new(2, 73, 9),
            BlockPos::new(4, 73, 9),
        ],
    );
    assert_simulation_reaches(&mut simulation, 80, BlockPos::new(4, 74, 9));
}

#[test]
fn test_mine_through_non_colliding_block() {
    let mut partial_chunks = PartialChunkStorage::default();

    let mut simulation = setup_simulation_world(
        &mut partial_chunks,
        BlockPos::new(0, 72, 1),
        &[BlockPos::new(0, 71, 1)],
        &[
            (BlockPos::new(0, 71, 0), BlockKind::SculkVein.into()),
            (BlockPos::new(0, 70, 0), BlockKind::GrassBlock.into()),
            // this is an extra check to make sure that we don't accidentally break the block
            // below (since tnt will break instantly)
            (BlockPos::new(0, 69, 0), BlockKind::Tnt.into()),
        ],
    );

    simulation.app.world_mut().write_message(GotoEvent {
        entity: simulation.entity,
        goal: Arc::new(BlockPosGoal(BlockPos::new(0, 70, 0))),
        opts: PathfinderOpts::new()
            .min_timeout(PathfinderTimeout::Nodes(1_000_000))
            .max_timeout(PathfinderTimeout::Nodes(5_000_000)),
    });

    assert_simulation_reaches(&mut simulation, 200, BlockPos::new(0, 70, 0));
}

// ---------------------------------------------------------------------
// Build-move tests: bridging, pillaring, staircasing, and the combined
// move set. Placement has no client-side prediction (see
// `moves/build.rs`'s module doc), so every test below installs
// `install_fake_placement_ack`, a test-only system that watches
// `StartUseItemEvent` and writes the resulting block directly into the
// simulated world -- mirroring how `MiningPlugin`'s real
// `handle_finish_mining_block_observer` fakes mining completion (mining
// has genuine client-side prediction; placement does not).
// ---------------------------------------------------------------------

const SCAFFOLD_ITEM_ID: &str = "minecraft:cobblestone";

fn build_capable_policy() -> PathfindingPolicy {
    PathfindingPolicy {
        allow_pillaring: true,
        allow_bridging: true,
        allow_staircase_building: true,
        fast_bridge_enabled: true,
        fast_bridge_edge_threshold: 0.2,
        scaffold: ScaffoldPolicy {
            allowed: vec![SCAFFOLD_ITEM_ID.to_owned()],
            denied: Vec::new(),
            minimum_held: 1,
        },
        ..Default::default()
    }
}

fn classic_bridge_policy() -> PathfindingPolicy {
    PathfindingPolicy {
        fast_bridge_enabled: false,
        ..build_capable_policy()
    }
}

/// Gives the simulated player a full stack of scaffold material in the
/// first hotbar slot and installs `policy` as its `CustomPathfinderState`.
/// `refresh_pathfinding_policy` (run by `goto_listener` when the goal is
/// submitted) fills in `scaffold_item`/`scaffold_available` from this
/// inventory, exactly like the real app flow.
fn equip_for_building_with_count(
    simulation: &mut Simulation,
    policy: PathfindingPolicy,
    count: i32,
) {
    let world = simulation.app.world_mut();
    let mut inventory = world
        .get_mut::<Inventory>(simulation.entity)
        .expect("simulated player has an Inventory component");
    let hotbar_slot = inventory
        .inventory_menu
        .hotbar_slots_range()
        .next()
        .expect("player menu has a hotbar");
    if let Some(slot) = inventory.inventory_menu.slot_mut(hotbar_slot) {
        *slot = ItemStack::new(ItemKind::Cobblestone, count);
    }
    drop(inventory);

    let custom_state = CustomPathfinderState::default();
    custom_state.0.write().insert(policy);
    world.entity_mut(simulation.entity).insert(custom_state);
}

fn equip_for_building(simulation: &mut Simulation, policy: PathfindingPolicy) {
    equip_for_building_with_count(simulation, policy, 64);
}

/// Fakes the server acknowledging a placement: every `StartUseItemEvent`
/// dispatched by `ExecuteCtx::place` becomes a real block in the simulated
/// world at the position it targeted, on the same tick it was requested.
fn install_fake_placement_ack(simulation: &mut Simulation) {
    let world = simulation.world.clone();
    simulation.app.add_systems(
        GameTick,
        (move |mut events: MessageReader<StartUseItemEvent>,
               mut inventories: Query<&mut Inventory>| {
            for event in events.read() {
                let Some(reference) = event.force_block else {
                    continue;
                };
                let face = event.force_direction.unwrap_or(Direction::Up);
                let placed_at = reference.offset_with_direction(face);
                world
                    .write()
                    .chunks
                    .set_block_state(placed_at, BlockKind::Cobblestone.into());

                // Also consume the scaffold item from inventory, same as a
                // real server round-trip would -- needed so tests can verify
                // `PathfindingPolicy::refresh_scaffold` keeps working as the
                // held count drops (see
                // `test_pillar_continues_after_dropping_below_minimum_held`).
                if let Ok(mut inventory) = inventories.get_mut(event.entity) {
                    consume_one_cobblestone(&mut inventory);
                }
            }
        })
        .after(PathfinderSystems),
    );
}

fn consume_one_cobblestone(inventory: &mut Inventory) {
    for menu_slot in inventory.inventory_menu.hotbar_slots_range() {
        let Some(slot) = inventory.inventory_menu.slot_mut(menu_slot) else {
            continue;
        };
        if let ItemStack::Present(data) = slot
            && data.kind == ItemKind::Cobblestone
        {
            data.count -= 1;
            if data.count <= 0 {
                *slot = ItemStack::Empty;
            }
            return;
        }
    }
}

fn setup_build_simulation(
    partial_chunks: &mut PartialChunkStorage,
    start_pos: BlockPos,
    end_pos: BlockPos,
    solid_blocks: &[BlockPos],
) -> Simulation {
    let mut simulation = setup_simulation_world(partial_chunks, start_pos, solid_blocks, &[]);
    equip_for_building(&mut simulation, build_capable_policy());
    install_fake_placement_ack(&mut simulation);

    simulation.app.world_mut().write_message(GotoEvent {
        entity: simulation.entity,
        goal: Arc::new(BlockPosGoal(end_pos)),
        opts: PathfinderOpts {
            successors_fn: moves::combined_move,
            allow_mining: true,
            retry_on_no_path: true,
            min_timeout: PathfinderTimeout::Nodes(1_000_000),
            max_timeout: PathfinderTimeout::Nodes(5_000_000),
        },
    });
    simulation
}

#[test]
fn test_bridge_across_gap() {
    let mut partial_chunks = PartialChunkStorage::default();
    // Floor only under the start block; everything from x=1 onward (down to
    // the bottom of the world) is open air, so there is nothing to walk,
    // jump, or mine onto -- bridging is the only way across.
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(3, 71, 0),
        &[BlockPos::new(0, 70, 0)],
    );
    assert_simulation_reaches(&mut simulation, 200, BlockPos::new(3, 71, 0));
}

#[test]
fn test_bridge_off_axis_target_goes_straight_not_zigzag() {
    let mut partial_chunks = PartialChunkStorage::default();
    // Open air everywhere except the start block, same as
    // `test_bridge_across_gap`, but the target is off-axis (dx=5, dz=1) --
    // the common real case, e.g. a `/goto` whose target is mostly ahead
    // with only a small sideways component. Purely cardinal bridge
    // segments cost exactly the same in total whether they're ordered as
    // one straight run or interleaved between axes, so without a cost bias
    // A* has no reason to prefer the straight-then-turn shape over an
    // interleaved (visually diagonal/staircase) one.
    //
    // This only checks the *shape* of however far the route gets, not that
    // it reaches the target -- see `test_bridge_off_axis_target_reaches_goal`
    // for a completion check over the same route.
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(1, 71, 5),
        &[BlockPos::new(0, 70, 0)],
    );
    wait_until_bot_starts_moving(&mut simulation);

    let mut positions = vec![BlockPos::from(simulation.position())];
    let mut ticks_since_progress = 0;
    let mut total_ticks = 0;
    while ticks_since_progress < 100 {
        simulation.tick();
        total_ticks += 1;
        assert!(
            total_ticks < 5000,
            "route never settled (kept changing block position every tick) after {total_ticks} ticks: {positions:?}"
        );
        let pos = BlockPos::from(simulation.position());
        if positions.last() == Some(&pos) {
            ticks_since_progress += 1;
            continue;
        }
        ticks_since_progress = 0;
        positions.push(pos);
    }
    // Made some real progress on both axes, so the shape check below is
    // meaningful and not just a straight run that never needed to turn.
    assert!(
        positions.len() >= 4,
        "route stalled immediately: {positions:?}"
    );

    // Count how many times the direction of travel switches between the X
    // and Z axis across the whole route. A clean straight-then-turn path
    // switches axis at most once; a zigzag/staircase pattern switches
    // repeatedly.
    let mut axis_switches = 0;
    let mut last_axis: Option<char> = None;
    for window in positions.windows(2) {
        let (a, b) = (window[0], window[1]);
        let axis = if a.x != b.x {
            Some('x')
        } else if a.z != b.z {
            Some('z')
        } else {
            None
        };
        if let Some(axis) = axis {
            if let Some(last) = last_axis
                && last != axis
            {
                axis_switches += 1;
            }
            last_axis = Some(axis);
        }
    }
    assert!(
        axis_switches <= 1,
        "expected a straight-then-turn bridge (at most 1 axis switch), got {axis_switches} switches: {positions:?}"
    );
}

#[test]
fn test_pillar_search_finds_full_multi_block_path_in_one_search() {
    // Regression test: `pillar_up_move` used to re-validate the *current*
    // position's standability against live (unmodified) world state --
    // only ever true for the bot's real starting position, since every
    // hypothetical position further up the same pillar chain only becomes
    // real once an *earlier* edge in that same search actually executes
    // and places its block. That capped every single A* search at exactly
    // one pillar edge no matter how tall the climb needed to be, forcing a
    // full repath (each with its own multi-hundred-ms-to-multi-second
    // minimum search time, see `PathfinderOpts::min_timeout`) after every
    // single block -- which is what made towering look stalled/unresponsive
    // in practice, not a jump or camera bug.
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(0, 76, 0),
        &[BlockPos::new(0, 70, 0)],
    );
    wait_until_bot_starts_moving(&mut simulation);
    let executing_path = simulation
        .get_component::<super::ExecutingPath>()
        .expect("pathfinder should have an active path after starting to move");
    assert!(
        !executing_path.is_path_partial,
        "expected the very first search to find the complete path to a nearby goal"
    );
    assert!(
        executing_path.path.len() >= 4,
        "expected the first search to chain multiple pillar edges together in one search, got {} edge(s)",
        executing_path.path.len()
    );
}

#[test]
fn test_pillar_up_to_tower() {
    let mut partial_chunks = PartialChunkStorage::default();
    // No terrain at all above the start block -- the only edge that can
    // possibly reach 5 blocks straight up with nothing to jump onto is
    // pillar_up_move.
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(0, 76, 0),
        &[BlockPos::new(0, 70, 0)],
    );
    assert_simulation_reaches(&mut simulation, 500, BlockPos::new(0, 76, 0));
}

#[test]
fn test_pillar_climbs_arbitrarily_high_when_uncapped() {
    // Regression test for a bug where the bot stopped pillaring after only
    // 2 blocks: `max_pillar_height` defaults to `0` (unset/uncapped) here
    // via `build_capable_policy()`, so nothing should stop this climb
    // partway -- a much taller tower (15 blocks) than
    // `test_pillar_up_to_tower`'s 5, to make sure it isn't quietly hitting
    // some other small hidden ceiling.
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(0, 86, 0),
        &[BlockPos::new(0, 70, 0)],
    );
    assert_simulation_reaches(&mut simulation, 4000, BlockPos::new(0, 86, 0));
}

#[test]
fn test_pillar_respects_max_pillar_height_cap() {
    let mut partial_chunks = PartialChunkStorage::default();
    // Same "nothing but open air above the start" setup as
    // `test_pillar_up_to_tower`, but the goal (20 blocks up) sits far above
    // a configured `max_pillar_height` of 5, and the bot holds unlimited
    // scaffold material. Without the cap nothing would stop it from
    // pillaring all the way to the goal; with it, `pillar_up_move` must
    // stop offering another step once the search has already climbed 5
    // blocks, regardless of how much material is held or how high the goal
    // actually is.
    let mut simulation = setup_simulation_world(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        &[BlockPos::new(0, 70, 0)],
        &[],
    );
    let mut policy = build_capable_policy();
    policy.max_pillar_height = 5;
    equip_for_building(&mut simulation, policy);
    install_fake_placement_ack(&mut simulation);

    simulation.app.world_mut().write_message(GotoEvent {
        entity: simulation.entity,
        goal: Arc::new(BlockPosGoal(BlockPos::new(0, 91, 0))),
        opts: PathfinderOpts {
            successors_fn: moves::combined_move,
            allow_mining: true,
            retry_on_no_path: true,
            min_timeout: PathfinderTimeout::Nodes(1_000_000),
            max_timeout: PathfinderTimeout::Nodes(5_000_000),
        },
    });

    wait_until_bot_starts_moving(&mut simulation);
    for _ in 0..1000 {
        simulation.tick();
    }

    // Check the cap by reading back how tall the *placed* column actually
    // is, rather than the bot's raw Y position: once pillaring is capped
    // and the (now unreachable) goal is still 15+ blocks further up, the
    // pathfinder can still go on to try other, unrelated moves (e.g. an
    // exploratory jump from `default_move`'s parkour set) that transiently
    // carry the bot's hitbox a little higher than the capped standing
    // height without ever placing a block -- that's expected pathfinding
    // behavior, not a cap violation, so asserting on raw position height
    // would be asserting something this feature was never meant to
    // guarantee. What actually matters is that no more than
    // `max_pillar_height` blocks ever get *built*.
    let placed_height = {
        let world = simulation.world.read();
        let mut height = 0;
        while world.get_block_state(BlockPos::new(0, 71 + height, 0)) == Some(BlockKind::Cobblestone.into())
        {
            height += 1;
        }
        height
    };
    assert_eq!(
        placed_height, 5,
        "expected exactly max_pillar_height (5) blocks to be placed, got {placed_height}"
    );
    assert_ne!(
        BlockPos::from(simulation.position()),
        BlockPos::new(0, 91, 0),
        "the capped route should never actually be able to reach the goal 20 blocks up"
    );
}

#[test]
fn test_pillar_continues_after_dropping_below_minimum_held() {
    // Regression test for a real bug: `minimum_held` (config default 4) was
    // being re-applied as a live gate on every replan (including the
    // per-tick obstruction check), not just when first selecting a scaffold
    // item. Starting with exactly `minimum_held` cobblestone means the very
    // first placement drops the held count below the threshold -- matching
    // what users hit with small stacks of building material -- so if the bug
    // regresses, the bot places 1 block and then refuses to select a
    // scaffold item ever again for the rest of the route.
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_simulation_world(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        &[BlockPos::new(0, 70, 0)],
        &[],
    );
    let mut policy = build_capable_policy();
    policy.scaffold.minimum_held = 4;
    equip_for_building_with_count(&mut simulation, policy, 4);
    install_fake_placement_ack(&mut simulation);

    simulation.app.world_mut().write_message(GotoEvent {
        entity: simulation.entity,
        goal: Arc::new(BlockPosGoal(BlockPos::new(0, 75, 0))),
        opts: PathfinderOpts {
            successors_fn: moves::combined_move,
            allow_mining: true,
            retry_on_no_path: true,
            min_timeout: PathfinderTimeout::Nodes(1_000_000),
            max_timeout: PathfinderTimeout::Nodes(5_000_000),
        },
    });

    // 4 blocks up, needing all 4 held cobblestone -- if the bug were still
    // present, the bot would place 1 block, the held count would drop to 3
    // (below `minimum_held: 4`), and every subsequent replan would refuse to
    // select a scaffold item at all, stranding the bot after the first step.
    assert_simulation_reaches(&mut simulation, 500, BlockPos::new(0, 75, 0));
}

#[test]
fn test_staircase_up_ledge() {
    let mut partial_chunks = PartialChunkStorage::default();
    // A single-block-too-short ledge: solid ground continues at y=70 up to
    // x=2, but the landing at (3, 71, 0) has no block under it (3, 70, 0),
    // so `ascend_move` can't reach it and staircasing has to extend the
    // floor by one block before stepping up.
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(3, 71, 0),
        &[
            BlockPos::new(0, 70, 0),
            BlockPos::new(1, 70, 0),
            BlockPos::new(2, 70, 0),
        ],
    );
    assert_simulation_reaches(&mut simulation, 200, BlockPos::new(3, 71, 0));
}

#[test]
fn test_combined_bridge_and_pillar_route() {
    let mut partial_chunks = PartialChunkStorage::default();
    // Bridge across a gap, then pillar up onto a tower -- forces A* to
    // compose two different build moves into a single route, per the
    // module doc's note that staircase-over-open-air is expected to emerge
    // from exactly this kind of composition.
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(3, 75, 0),
        &[BlockPos::new(0, 70, 0)],
    );
    assert_simulation_reaches(&mut simulation, 800, BlockPos::new(3, 75, 0));
}

#[test]
fn test_build_move_path_completion_reports_goal_reached() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(0, 76, 0),
        &[BlockPos::new(0, 70, 0)],
    );
    wait_until_bot_starts_moving(&mut simulation);
    for _ in 0..2400 {
        simulation.tick();
    }
    assert_eq!(
        BlockPos::from(simulation.position()),
        BlockPos::new(0, 76, 0)
    );
    let pathfinder = simulation.component::<super::Pathfinder>();
    assert!(
        pathfinder.goal.is_none(),
        "goal should be cleared once a pillar-up route reaches its target"
    );
}

#[test]
fn test_build_move_path_cancellation_stops_pillaring() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(0, 90, 0),
        &[BlockPos::new(0, 70, 0)],
    );
    wait_until_bot_starts_moving(&mut simulation);
    for _ in 0..10 {
        simulation.tick();
    }
    let position_at_cancel = simulation.position();
    assert!(
        position_at_cancel.y > 71.0,
        "expected the bot to have started pillaring up before cancellation"
    );

    simulation
        .app
        .world_mut()
        .entity_mut(simulation.entity)
        .remove::<super::ExecutingPath>();
    simulation
        .app
        .world_mut()
        .get_mut::<super::Pathfinder>(simulation.entity)
        .expect("simulated player has a Pathfinder component")
        .goal = None;

    for _ in 0..20 {
        simulation.tick();
    }
    let position_after_cancel = simulation.position();
    assert!(
        position_after_cancel.y < 90.0,
        "cancelled path should not have continued climbing to the original goal"
    );
}

#[test]
fn test_build_move_dynamic_replanning_around_new_obstruction() {
    let mut partial_chunks = PartialChunkStorage::default();
    // A bridge route across a 3-block gap; once the bot is committed to
    // crossing it, a block is placed directly in its path (mutating the
    // `PartialChunkStorage`-backed world mid-route) and the obstruction
    // detector (`execute/patching.rs::check_for_path_obstruction`) must
    // patch around it rather than the bot getting stuck walking into it.
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(4, 71, 0),
        &[BlockPos::new(0, 70, 0), BlockPos::new(4, 70, 0)],
    );
    wait_until_bot_starts_moving(&mut simulation);
    for _ in 0..10 {
        simulation.tick();
    }
    // Obstruct the air directly ahead of the bridge route with a solid
    // block it didn't place itself, simulating another player/mob altering
    // the world mid-route.
    simulation
        .world
        .write()
        .chunks
        .set_block_state(BlockPos::new(2, 71, 0), BlockKind::Stone.into());

    assert_simulation_reaches(&mut simulation, 300, BlockPos::new(4, 71, 0));
}

#[test]
fn test_follow_style_goal_change_reroutes_through_build_moves() {
    let mut partial_chunks = PartialChunkStorage::default();
    // Mirrors what `/follow` does in the app crate
    // (`MovementService::refresh_navigation_goal`, called periodically from
    // `tick_follow`): submit a walkable ground-level goal, let the bot
    // start moving toward it, then resubmit a new goal directly above it
    // with no floor of its own -- as if the followed player had climbed
    // onto a tower. The bot must re-route through pillar_up_move rather
    // than stopping once the original ground-level goal is reached.
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(2, 71, 0),
        &[
            BlockPos::new(0, 70, 0),
            BlockPos::new(1, 70, 0),
            BlockPos::new(2, 70, 0),
        ],
    );
    wait_until_bot_starts_moving(&mut simulation);
    for _ in 0..10 {
        simulation.tick();
    }

    simulation.app.world_mut().write_message(GotoEvent {
        entity: simulation.entity,
        goal: Arc::new(BlockPosGoal(BlockPos::new(2, 74, 0))),
        opts: PathfinderOpts {
            successors_fn: moves::combined_move,
            allow_mining: true,
            retry_on_no_path: true,
            min_timeout: PathfinderTimeout::Nodes(1_000_000),
            max_timeout: PathfinderTimeout::Nodes(5_000_000),
        },
    });

    assert_simulation_reaches(&mut simulation, 500, BlockPos::new(2, 74, 0));
}

#[test]
fn test_bridge_across_long_gap() {
    // Regression test for a real bug: bridging over more than ~4 blocks in a
    // row would reliably freeze the bot in place with no floor under its
    // feet and no way to recover.
    //
    // Root cause was in `vertical_is_reached`: it only checked that the
    // player's block-position matched the target column and that
    // `physics.on_ground()` was true. But the player hitbox is wider than
    // one block, so while sneak-walking the last stretch toward a column's
    // edge, a single tick's movement could carry the *center* point past
    // the target column's boundary while the hitbox still had just enough
    // overlap with the block behind it to read as grounded -- a
    // false-positive "reached" that fired before `at_edge_toward` in
    // `execute_bridge_move` ever got a tick to actually place the block
    // underneath. The path would then advance onto a column with no floor,
    // permanently: it had no support to walk closer on for the *next*
    // edge's own approach check, so it just sat there.
    //
    // Whether any given edge hits this depends on how the fixed per-tick
    // walk step happens to land relative to the edge-detection window, so
    // it doesn't reproduce on every single block -- a long, purely straight
    // bridge (unlike `test_bridge_across_gap`'s short 3-block one) reliably
    // hits the misaligned case within the first several blocks.
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(10, 71, 0),
        &[BlockPos::new(0, 70, 0)],
    );
    assert_simulation_reaches(&mut simulation, 2000, BlockPos::new(10, 71, 0));
}

#[test]
fn test_bridge_off_axis_target_reaches_goal() {
    // Completion counterpart to
    // `test_bridge_off_axis_target_goes_straight_not_zigzag` (which only
    // checks the shape of the route). Also a regression test for the
    // `vertical_is_reached` false-positive fixed alongside
    // `test_bridge_across_long_gap`.
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(0, 71, 0),
        BlockPos::new(1, 71, 5),
        &[BlockPos::new(0, 70, 0)],
    );
    assert_simulation_reaches(&mut simulation, 2000, BlockPos::new(1, 71, 5));
}

// ---------------------------------------------------------------------
// Fast bridging ("speed bridging"): walk normally between edges, sneaking
// only for the brief moment needed to place the next block, rather than
// sneaking for the whole approach. `build_capable_policy` already enables
// this by default (`fast_bridge_enabled: true`), matching the real app --
// see `execute_fast_bridge_move` in `moves/build.rs`.
//
// All start positions here sit well inside a single loaded 16x16 chunk
// (`setup_build_simulation` only loads the chunk containing the start
// block) so a few blocks of travel in any cardinal direction never crosses
// into unloaded terrain.
// ---------------------------------------------------------------------

/// Fakes the server rejecting the first `fail_first_n` placement attempts
/// (no block appears, no material is consumed) before acknowledging every
/// attempt after that normally -- for testing that a rejected placement
/// gets retried rather than silently stalling or skipping ahead.
fn install_flaky_placement_ack(simulation: &mut Simulation, fail_first_n: usize) {
    let world = simulation.world.clone();
    let remaining_failures = Arc::new(std::sync::atomic::AtomicUsize::new(fail_first_n));
    simulation.app.add_systems(
        GameTick,
        (move |mut events: MessageReader<StartUseItemEvent>,
               mut inventories: Query<&mut Inventory>| {
            for event in events.read() {
                let Some(reference) = event.force_block else {
                    continue;
                };
                if remaining_failures
                    .try_update(
                        std::sync::atomic::Ordering::Relaxed,
                        std::sync::atomic::Ordering::Relaxed,
                        |remaining| remaining.checked_sub(1),
                    )
                    .is_ok()
                {
                    // Simulated rejection: no block placed, no material
                    // consumed, exactly like a real placement that never
                    // landed server-side.
                    continue;
                }
                let face = event.force_direction.unwrap_or(Direction::Up);
                let placed_at = reference.offset_with_direction(face);
                world
                    .write()
                    .chunks
                    .set_block_state(placed_at, BlockKind::Cobblestone.into());
                if let Ok(mut inventory) = inventories.get_mut(event.entity) {
                    consume_one_cobblestone(&mut inventory);
                }
            }
        })
        .after(PathfinderSystems),
    );
}

/// Fakes the server acknowledging a placement the same way
/// `install_fake_placement_ack` does, but consumes one of whichever item
/// [`PathfindingPolicy::scaffold_item`] currently names (read live from the
/// entity's own `CustomPathfinderState`) instead of always assuming
/// cobblestone -- needed to verify material actually gets consumed from
/// whichever scaffold item the policy switched to.
fn install_fake_placement_ack_tracking_scaffold_item(simulation: &mut Simulation) {
    let world = simulation.world.clone();
    simulation.app.add_systems(
        GameTick,
        (move |mut events: MessageReader<StartUseItemEvent>,
               mut inventories: Query<&mut Inventory>,
               custom_states: Query<&CustomPathfinderState>| {
            for event in events.read() {
                let Some(reference) = event.force_block else {
                    continue;
                };
                let face = event.force_direction.unwrap_or(Direction::Up);
                let placed_at = reference.offset_with_direction(face);
                world
                    .write()
                    .chunks
                    .set_block_state(placed_at, BlockKind::Cobblestone.into());

                let Some(item_id) = custom_states.get(event.entity).ok().and_then(|state| {
                    state
                        .0
                        .read()
                        .get::<PathfindingPolicy>()
                        .and_then(|policy| policy.scaffold_item.clone())
                }) else {
                    continue;
                };
                if let Ok(mut inventory) = inventories.get_mut(event.entity) {
                    consume_one_matching(&mut inventory, &item_id);
                }
            }
        })
        .after(PathfinderSystems),
    );
}

fn consume_one_matching(inventory: &mut Inventory, item_id: &str) {
    for menu_slot in inventory.inventory_menu.hotbar_slots_range() {
        let Some(slot) = inventory.inventory_menu.slot_mut(menu_slot) else {
            continue;
        };
        if let ItemStack::Present(data) = slot
            && data.kind.to_string() == item_id
        {
            data.count -= 1;
            if data.count <= 0 {
                *slot = ItemStack::Empty;
            }
            return;
        }
    }
}

/// Equips multiple distinct scaffold materials across successive hotbar
/// slots (first item in slot 0, second in slot 1, ...) instead of a single
/// stack, for testing that the bot switches materials once the first one
/// it's using runs out.
fn equip_for_building_with_items(
    simulation: &mut Simulation,
    policy: PathfindingPolicy,
    items: &[(ItemKind, i32)],
) {
    let world = simulation.app.world_mut();
    let mut inventory = world
        .get_mut::<Inventory>(simulation.entity)
        .expect("simulated player has an Inventory component");
    let hotbar_slots: Vec<usize> = inventory.inventory_menu.hotbar_slots_range().collect();
    for (slot_index, (kind, count)) in hotbar_slots.iter().zip(items) {
        if let Some(slot) = inventory.inventory_menu.slot_mut(*slot_index) {
            *slot = ItemStack::new(*kind, *count);
        }
    }
    drop(inventory);

    let custom_state = CustomPathfinderState::default();
    custom_state.0.write().insert(policy);
    world.entity_mut(simulation.entity).insert(custom_state);
}

fn is_sneaking(simulation: &Simulation) -> bool {
    simulation
        .get_component::<ClientMovementState>()
        .is_some_and(|state| state.trying_to_crouch)
}

#[test]
fn test_fast_bridge_east() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(13, 71, 8),
        &[BlockPos::new(8, 70, 8)],
    );
    assert_simulation_reaches(&mut simulation, 500, BlockPos::new(13, 71, 8));
}

#[test]
fn test_fast_bridge_west() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(3, 71, 8),
        &[BlockPos::new(8, 70, 8)],
    );
    assert_simulation_reaches(&mut simulation, 500, BlockPos::new(3, 71, 8));
}

#[test]
fn test_fast_bridge_south() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(8, 71, 13),
        &[BlockPos::new(8, 70, 8)],
    );
    assert_simulation_reaches(&mut simulation, 500, BlockPos::new(8, 71, 13));
}

#[test]
fn test_fast_bridge_north() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(8, 71, 3),
        &[BlockPos::new(8, 70, 8)],
    );
    assert_simulation_reaches(&mut simulation, 500, BlockPos::new(8, 71, 3));
}

#[test]
fn test_fast_bridge_single_corner() {
    // East 3, then a 90-degree turn south for 3 more -- cardinal-only, no
    // diagonal bridging.
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(11, 71, 11),
        &[BlockPos::new(8, 70, 8)],
    );
    assert_simulation_reaches(&mut simulation, 800, BlockPos::new(11, 71, 11));
}

#[test]
fn test_fast_bridge_multiple_corners() {
    // An off-axis target far enough in both dimensions that reaching it
    // needs several turns' worth of cardinal-only segments, not just one.
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(4, 71, 3),
        &[BlockPos::new(8, 70, 8)],
    );
    assert_simulation_reaches(&mut simulation, 1200, BlockPos::new(4, 71, 3));
}

#[test]
fn test_fast_bridge_placement_failure_recovery() {
    // The very first placement attempt is rejected (simulating a missed or
    // anticheat-blocked placement); the bot must keep sneaking and retry
    // rather than fall in or advance without a real block underneath it.
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_simulation_world(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        &[BlockPos::new(8, 70, 8)],
        &[],
    );
    equip_for_building(&mut simulation, build_capable_policy());
    install_flaky_placement_ack(&mut simulation, 3);

    simulation.app.world_mut().write_message(GotoEvent {
        entity: simulation.entity,
        goal: Arc::new(BlockPosGoal(BlockPos::new(11, 71, 8))),
        opts: PathfinderOpts {
            successors_fn: moves::combined_move,
            allow_mining: true,
            retry_on_no_path: true,
            min_timeout: PathfinderTimeout::Nodes(1_000_000),
            max_timeout: PathfinderTimeout::Nodes(5_000_000),
        },
    });

    assert_simulation_reaches(&mut simulation, 800, BlockPos::new(11, 71, 8));
}

#[test]
fn test_fast_bridge_edge_threshold_gates_sneaking() {
    // The bot must not sneak until it's genuinely within the configured
    // edge threshold of the current block's boundary -- not earlier, and
    // (via `test_fast_bridge_edge_threshold_survives_full_speed_approach`)
    // not so late that a fast-moving tick skips the window entirely.
    const THRESHOLD: f64 = 0.2;
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(11, 71, 8),
        &[BlockPos::new(8, 70, 8)],
    );
    wait_until_bot_starts_moving(&mut simulation);

    let mut saw_sneaking = false;
    for _ in 0..400 {
        simulation.tick();
        let pos = simulation.position();
        if is_sneaking(&simulation) {
            saw_sneaking = true;
            let boundary = pos.x.floor() + 1.0;
            assert!(
                pos.x >= boundary - THRESHOLD - 0.05,
                "sneaked at x={} but the edge threshold ({THRESHOLD}) toward the boundary at {boundary} wasn't reached yet",
                pos.x
            );
        }
        if BlockPos::from(pos) == BlockPos::new(11, 71, 8) {
            break;
        }
    }
    assert!(
        saw_sneaking,
        "expected at least one sneak pulse while bridging"
    );
    assert_eq!(
        BlockPos::from(simulation.position()),
        BlockPos::new(11, 71, 8)
    );
}

#[test]
fn test_fast_bridge_edge_threshold_survives_full_speed_approach() {
    // A long straight bridge, checked tick-by-tick: sneaking must engage
    // only briefly near each edge (not for most of the route, the way
    // classic/permanent-sneak bridging does), and must never engage while
    // the bot is still well inside a block -- i.e. the configured
    // threshold is wide enough that a normal-speed walking tick can't skip
    // clean over the detection window and walk off the edge unsneaked.
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(15, 71, 8),
        &[BlockPos::new(8, 70, 8)],
    );
    wait_until_bot_starts_moving(&mut simulation);

    let mut total_ticks = 0;
    let mut sneaking_ticks = 0;
    for _ in 0..2000 {
        simulation.tick();
        let pos = simulation.position();
        if BlockPos::from(pos) == BlockPos::new(15, 71, 8) {
            break;
        }
        total_ticks += 1;
        let sneaking = is_sneaking(&simulation);
        if sneaking {
            sneaking_ticks += 1;
        }
        let fractional_x = pos.x - pos.x.floor();
        assert!(
            !(sneaking && (0.3..0.7).contains(&fractional_x)),
            "sneaked while well inside a block (fractional x={fractional_x}) -- the edge threshold is too narrow for this approach speed"
        );
    }
    assert_eq!(
        BlockPos::from(simulation.position()),
        BlockPos::new(15, 71, 8)
    );
    assert!(total_ticks > 0);
    assert!(
        (sneaking_ticks as f64) < (total_ticks as f64) * 0.5,
        "fast bridging should sneak only briefly near each edge, not for most of the route (sneaked {sneaking_ticks}/{total_ticks} ticks)"
    );
}

#[test]
fn test_fast_bridge_scaffold_switches_when_material_runs_out() {
    // Only 2 cobblestone -- not enough for the whole 5-block bridge -- but
    // a full stack of dirt in the next hotbar slot. The bridge must still
    // complete by switching to dirt once cobblestone is exhausted, the
    // same "keep using whatever's next in preference order" behavior
    // `PathfindingPolicy::refresh_scaffold` already has for pillaring
    // (`test_pillar_continues_after_dropping_below_minimum_held`), applied
    // here to a genuine material switch instead of just a low-count one.
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_simulation_world(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        &[BlockPos::new(8, 70, 8)],
        &[],
    );
    let policy = PathfindingPolicy {
        scaffold: ScaffoldPolicy {
            allowed: vec![
                "minecraft:cobblestone".to_owned(),
                "minecraft:dirt".to_owned(),
            ],
            denied: Vec::new(),
            minimum_held: 1,
        },
        ..build_capable_policy()
    };
    equip_for_building_with_items(
        &mut simulation,
        policy,
        &[(ItemKind::Cobblestone, 2), (ItemKind::Dirt, 64)],
    );
    install_fake_placement_ack_tracking_scaffold_item(&mut simulation);

    simulation.app.world_mut().write_message(GotoEvent {
        entity: simulation.entity,
        goal: Arc::new(BlockPosGoal(BlockPos::new(13, 71, 8))),
        opts: PathfinderOpts {
            successors_fn: moves::combined_move,
            allow_mining: true,
            retry_on_no_path: true,
            min_timeout: PathfinderTimeout::Nodes(1_000_000),
            max_timeout: PathfinderTimeout::Nodes(5_000_000),
        },
    });

    assert_simulation_reaches(&mut simulation, 800, BlockPos::new(13, 71, 8));

    let inventory = simulation
        .get_component::<Inventory>()
        .expect("simulated player has an Inventory component");
    let cobblestone_left = inventory
        .inventory_menu
        .slots()
        .iter()
        .filter(|slot| slot.kind() == ItemKind::Cobblestone)
        .map(|slot| slot.count())
        .sum::<i32>();
    let dirt_left = inventory
        .inventory_menu
        .slots()
        .iter()
        .filter(|slot| slot.kind() == ItemKind::Dirt)
        .map(|slot| slot.count())
        .sum::<i32>();
    assert_eq!(
        cobblestone_left, 0,
        "expected cobblestone to be fully used up"
    );
    assert!(
        dirt_left < 64,
        "expected the bot to have switched to dirt for the rest of the bridge, dirt_left={dirt_left}"
    );
}

#[test]
fn test_fast_bridge_path_cancellation() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(20, 71, 8),
        &[BlockPos::new(8, 70, 8)],
    );
    wait_until_bot_starts_moving(&mut simulation);
    for _ in 0..10 {
        simulation.tick();
    }
    let position_at_cancel = simulation.position();
    assert!(
        position_at_cancel.x > 8.5,
        "expected the bot to have started bridging before cancellation"
    );

    simulation
        .app
        .world_mut()
        .entity_mut(simulation.entity)
        .remove::<super::ExecutingPath>();
    simulation
        .app
        .world_mut()
        .get_mut::<super::Pathfinder>(simulation.entity)
        .expect("simulated player has a Pathfinder component")
        .goal = None;

    for _ in 0..20 {
        simulation.tick();
    }
    let position_after_cancel = simulation.position();
    assert!(
        position_after_cancel.x < 20.0,
        "cancelled path should not have continued bridging to the original goal"
    );
}

#[test]
fn test_fast_bridge_unsneaks_after_reaching_destination() {
    // Reaching the destination must fully release sneak, not just stop
    // walking -- a build move can still be mid-sneak-pulse (placing the
    // final block) on the very tick the goal is satisfied, and nothing else
    // resets `trying_to_crouch` once `ExecutingPath` is removed.
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(11, 71, 8),
        &[BlockPos::new(8, 70, 8)],
    );
    assert_simulation_reaches(&mut simulation, 400, BlockPos::new(11, 71, 8));

    assert!(
        !is_sneaking(&simulation),
        "bot should have released sneak after reaching the destination"
    );
    assert!(
        simulation
            .app
            .world_mut()
            .get::<super::ExecutingPath>(simulation.entity)
            .is_none(),
        "ExecutingPath should be cleared once the goal is reached"
    );
    assert!(
        simulation
            .app
            .world_mut()
            .get::<super::Pathfinder>(simulation.entity)
            .expect("simulated player has a Pathfinder component")
            .goal
            .is_none(),
        "goal should be cleared once reached"
    );
}

#[test]
fn test_fast_bridge_cancels_when_no_scaffold_available() {
    // Only 1 scaffold block -- enough for `minimum_held` to let the route
    // start, but nowhere near enough to finish a 3-block bridge. Once it
    // runs out mid-route the bot cannot possibly complete this goal, and
    // must cancel it via `timeout_movement`'s `BlockedOnMissingScaffold`
    // handling instead of hanging (sneaking in place at the edge) forever.
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_simulation_world(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        &[BlockPos::new(8, 70, 8)],
        &[],
    );
    equip_for_building_with_count(&mut simulation, build_capable_policy(), 1);
    install_fake_placement_ack(&mut simulation);

    simulation.app.world_mut().write_message(GotoEvent {
        entity: simulation.entity,
        goal: Arc::new(BlockPosGoal(BlockPos::new(11, 71, 8))),
        opts: PathfinderOpts {
            successors_fn: moves::combined_move,
            allow_mining: true,
            retry_on_no_path: true,
            min_timeout: PathfinderTimeout::Nodes(1_000_000),
            max_timeout: PathfinderTimeout::Nodes(5_000_000),
        },
    });

    let mut goal_cleared = false;
    let start_time = Instant::now();
    while start_time.elapsed() < Duration::from_millis(10_000) {
        simulation.tick();
        thread::yield_now();
        if simulation
            .app
            .world_mut()
            .get::<super::Pathfinder>(simulation.entity)
            .expect("simulated player has a Pathfinder component")
            .goal
            .is_none()
        {
            goal_cleared = true;
            break;
        }
    }

    assert!(
        goal_cleared,
        "expected the goal to be cancelled once no scaffold material was available, instead of hanging forever"
    );
    assert!(
        !is_sneaking(&simulation),
        "bot should not be left sneaking after the route was cancelled"
    );
    assert_ne!(
        BlockPos::from(simulation.position()),
        BlockPos::new(11, 71, 8),
        "bot should not have somehow reached the goal with zero scaffold material"
    );
}

#[test]
fn test_classic_bridge_still_works_when_fast_bridge_disabled() {
    // `PathfindingPolicy::fast_bridge_enabled: false` must keep the
    // original permanent-sneak technique fully working, not just fast
    // bridging.
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_simulation_world(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        &[BlockPos::new(8, 70, 8)],
        &[],
    );
    equip_for_building(&mut simulation, classic_bridge_policy());
    install_fake_placement_ack(&mut simulation);

    simulation.app.world_mut().write_message(GotoEvent {
        entity: simulation.entity,
        goal: Arc::new(BlockPosGoal(BlockPos::new(13, 71, 8))),
        opts: PathfinderOpts {
            successors_fn: moves::combined_move,
            allow_mining: true,
            retry_on_no_path: true,
            min_timeout: PathfinderTimeout::Nodes(1_000_000),
            max_timeout: PathfinderTimeout::Nodes(5_000_000),
        },
    });

    assert_simulation_reaches(&mut simulation, 800, BlockPos::new(13, 71, 8));
}

#[test]
fn test_fast_bridge_is_faster_than_classic_bridge() {
    let route_ticks = |policy_fn: fn() -> PathfindingPolicy| -> usize {
        let mut partial_chunks = PartialChunkStorage::default();
        let mut simulation = setup_simulation_world(
            &mut partial_chunks,
            BlockPos::new(8, 71, 8),
            &[BlockPos::new(8, 70, 8)],
            &[],
        );
        equip_for_building(&mut simulation, policy_fn());
        install_fake_placement_ack(&mut simulation);

        simulation.app.world_mut().write_message(GotoEvent {
            entity: simulation.entity,
            goal: Arc::new(BlockPosGoal(BlockPos::new(15, 71, 8))),
            opts: PathfinderOpts {
                successors_fn: moves::combined_move,
                allow_mining: true,
                retry_on_no_path: true,
                min_timeout: PathfinderTimeout::Nodes(1_000_000),
                max_timeout: PathfinderTimeout::Nodes(5_000_000),
            },
        });

        wait_until_bot_starts_moving(&mut simulation);
        let mut ticks = 0;
        while BlockPos::from(simulation.position()) != BlockPos::new(15, 71, 8) {
            simulation.tick();
            ticks += 1;
            assert!(ticks < 2000, "route never completed");
        }
        ticks
    };

    let fast_ticks = route_ticks(build_capable_policy);
    let classic_ticks = route_ticks(classic_bridge_policy);
    assert!(
        fast_ticks < classic_ticks,
        "expected fast bridging ({fast_ticks} ticks) to complete a 7-block bridge faster than classic permanent-sneak bridging ({classic_ticks} ticks)"
    );
}

// ---------------------------------------------------------------------
// Slab fast bridging: the block supporting the bridge is a top slab
// rather than a full block, which only has solid collision in its upper
// half -- see `slab_click_height` in `moves/build.rs` for why that needs a
// different aim point than `look_behind_point`'s full-block center. These
// tests mirror the full-block fast-bridge tests above, but the floor (and,
// for the "pure slab" tests, every subsequent placement) is a top slab.
// ---------------------------------------------------------------------

const SLAB_SCAFFOLD_ITEM_ID: &str = "minecraft:cobblestone_slab";

fn top_slab_state() -> BlockState {
    CobblestoneSlab {
        kind: SlabKind::Top,
        waterlogged: false,
    }
    .into()
}

fn slab_capable_policy() -> PathfindingPolicy {
    PathfindingPolicy {
        scaffold: ScaffoldPolicy {
            allowed: vec![SLAB_SCAFFOLD_ITEM_ID.to_owned()],
            denied: Vec::new(),
            minimum_held: 1,
        },
        ..build_capable_policy()
    }
}

/// Gives the simulated player a full stack of slab scaffold material and
/// installs `policy`, mirroring `equip_for_building` but for slab bridging
/// tests.
fn equip_for_slab_building(simulation: &mut Simulation, policy: PathfindingPolicy) {
    let world = simulation.app.world_mut();
    let mut inventory = world
        .get_mut::<Inventory>(simulation.entity)
        .expect("simulated player has an Inventory component");
    let hotbar_slot = inventory
        .inventory_menu
        .hotbar_slots_range()
        .next()
        .expect("player menu has a hotbar");
    if let Some(slot) = inventory.inventory_menu.slot_mut(hotbar_slot) {
        *slot = ItemStack::new(ItemKind::CobblestoneSlab, 64);
    }
    drop(inventory);

    let custom_state = CustomPathfinderState::default();
    custom_state.0.write().insert(policy);
    world.entity_mut(simulation.entity).insert(custom_state);
}

/// Fakes the server acknowledging a slab placement: every
/// `StartUseItemEvent` becomes a top slab in the simulated world (mirroring
/// how the real bridge move always aims for the upper half of the target
/// cell, see `slab_click_height`), so a whole multi-block bridge stays
/// slab-supported end to end.
fn install_fake_slab_placement_ack(simulation: &mut Simulation) {
    let world = simulation.world.clone();
    simulation.app.add_systems(
        GameTick,
        (move |mut events: MessageReader<StartUseItemEvent>,
               mut inventories: Query<&mut Inventory>| {
            for event in events.read() {
                let Some(reference) = event.force_block else {
                    continue;
                };
                let face = event.force_direction.unwrap_or(Direction::Up);
                let placed_at = reference.offset_with_direction(face);
                world
                    .write()
                    .chunks
                    .set_block_state(placed_at, top_slab_state());
                if let Ok(mut inventory) = inventories.get_mut(event.entity) {
                    consume_one_matching(&mut inventory, SLAB_SCAFFOLD_ITEM_ID);
                }
            }
        })
        .after(PathfinderSystems),
    );
}

/// Like `setup_build_simulation`, but `slab_floor_blocks` are top slabs
/// instead of full stone.
fn setup_slab_build_simulation(
    partial_chunks: &mut PartialChunkStorage,
    start_pos: BlockPos,
    end_pos: BlockPos,
    slab_floor_blocks: &[BlockPos],
) -> Simulation {
    let extra_blocks: Vec<(BlockPos, BlockState)> = slab_floor_blocks
        .iter()
        .map(|pos| (*pos, top_slab_state()))
        .collect();
    let mut simulation = setup_simulation_world(partial_chunks, start_pos, &[], &extra_blocks);
    equip_for_slab_building(&mut simulation, slab_capable_policy());
    install_fake_slab_placement_ack(&mut simulation);

    simulation.app.world_mut().write_message(GotoEvent {
        entity: simulation.entity,
        goal: Arc::new(BlockPosGoal(end_pos)),
        opts: PathfinderOpts {
            successors_fn: moves::combined_move,
            allow_mining: true,
            retry_on_no_path: true,
            min_timeout: PathfinderTimeout::Nodes(1_000_000),
            max_timeout: PathfinderTimeout::Nodes(5_000_000),
        },
    });
    simulation
}

#[test]
fn test_slab_fast_bridge_east() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_slab_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(13, 71, 8),
        &[BlockPos::new(8, 70, 8)],
    );
    assert_simulation_reaches(&mut simulation, 500, BlockPos::new(13, 71, 8));
}

#[test]
fn test_slab_fast_bridge_west() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_slab_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(3, 71, 8),
        &[BlockPos::new(8, 70, 8)],
    );
    assert_simulation_reaches(&mut simulation, 500, BlockPos::new(3, 71, 8));
}

#[test]
fn test_slab_fast_bridge_south() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_slab_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(8, 71, 13),
        &[BlockPos::new(8, 70, 8)],
    );
    assert_simulation_reaches(&mut simulation, 500, BlockPos::new(8, 71, 13));
}

#[test]
fn test_slab_fast_bridge_north() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_slab_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(8, 71, 3),
        &[BlockPos::new(8, 70, 8)],
    );
    assert_simulation_reaches(&mut simulation, 500, BlockPos::new(8, 71, 3));
}

/// Camera requirement: while crossing a slab bridge, pitch has to stay
/// steeply downward (close to the ~78 degrees slab bridging asks for) and
/// never relax toward the horizon -- a shallow pitch would mean the camera
/// snapped forward toward the direction of travel instead of staying aimed
/// at the top slab edge, which is exactly the "no forward camera snapping"
/// requirement this move exists to satisfy.
#[test]
fn test_slab_bridge_targets_top_edge_with_steep_pitch() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_slab_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(14, 71, 8),
        &[BlockPos::new(8, 70, 8)],
    );
    wait_until_bot_starts_moving(&mut simulation);

    let mut sampled_any = false;
    for _ in 0..400 {
        simulation.tick();
        if BlockPos::from(simulation.position()) == BlockPos::new(14, 71, 8) {
            break;
        }
        // Only sample once actual progress has been made past the start --
        // the very first tick or two can still be settling into the
        // approach.
        if simulation.position().x > 9.0 {
            let pitch = simulation.component::<LookDirection>().x_rot();
            assert!(
                (60.0..=90.0).contains(&pitch),
                "pitch {pitch} should stay steeply downward while slab bridging, not snap forward"
            );
            sampled_any = true;
        }
    }
    assert!(
        sampled_any,
        "never made enough progress across the slab bridge to sample pitch"
    );
}

#[test]
fn test_slab_bridge_completion_reports_goal_reached() {
    let mut partial_chunks = PartialChunkStorage::default();
    let mut simulation = setup_slab_build_simulation(
        &mut partial_chunks,
        BlockPos::new(8, 71, 8),
        BlockPos::new(12, 71, 8),
        &[BlockPos::new(8, 70, 8)],
    );
    assert_simulation_reaches(&mut simulation, 500, BlockPos::new(12, 71, 8));
    let pathfinder = simulation.component::<super::Pathfinder>();
    assert!(
        pathfinder.goal.is_none(),
        "goal should be cleared once a slab bridge reaches its target"
    );
}

/// Mixed slab/full-block bridging: the bot starts on a top slab (so the
/// very first segment must use the slab-aware aim point) but every
/// placement after that lands as an ordinary full block
/// (`install_fake_placement_ack`, not the slab-specific ack) -- exercising
/// the fallback back to `look_behind_point` once the support is no longer
/// a slab.
#[test]
fn test_mixed_slab_and_full_block_bridge() {
    let mut partial_chunks = PartialChunkStorage::default();
    let extra_blocks = [(BlockPos::new(8, 70, 8), top_slab_state())];
    let mut simulation =
        setup_simulation_world(&mut partial_chunks, BlockPos::new(8, 71, 8), &[], &extra_blocks);
    equip_for_slab_building(&mut simulation, slab_capable_policy());
    install_fake_placement_ack(&mut simulation);

    simulation.app.world_mut().write_message(GotoEvent {
        entity: simulation.entity,
        goal: Arc::new(BlockPosGoal(BlockPos::new(13, 71, 8))),
        opts: PathfinderOpts {
            successors_fn: moves::combined_move,
            allow_mining: true,
            retry_on_no_path: true,
            min_timeout: PathfinderTimeout::Nodes(1_000_000),
            max_timeout: PathfinderTimeout::Nodes(5_000_000),
        },
    });
    assert_simulation_reaches(&mut simulation, 500, BlockPos::new(13, 71, 8));
}
