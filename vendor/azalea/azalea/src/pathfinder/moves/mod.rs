pub mod basic;
pub mod building;
pub mod parkour;
pub mod uncommon;

use std::{
    fmt::{self, Debug},
    sync::Arc,
};

use azalea_block::BlockState;
use azalea_client::{
    ClientMovementState, SprintDirection, StartSprintEvent, StartWalkEvent, WalkDirection,
    interact::StartUseItemEvent, inventory::SetSelectedHotbarSlotEvent,
    mining::StartMiningBlockEvent,
};
use azalea_core::{
    direction::Direction,
    position::{BlockPos, Vec3},
};
use azalea_inventory::{ItemStack, Menu};
use azalea_protocol::packets::game::s_interact::InteractionHand;
use azalea_registry::builtin::BlockKind;
use azalea_world::World;
use bevy_ecs::{
    component::Component, entity::Entity, message::MessageWriter, system::Commands,
    world::EntityWorldMut,
};
use parking_lot::RwLock;
use tracing::trace;

use super::{
    astar,
    custom_state::CustomPathfinderStateRef,
    mining::MiningCache,
    positions::RelBlockPos,
    world::{CachedWorld, is_block_state_passable},
};
use crate::{
    auto_tool::best_tool_in_hotbar_for_block,
    bot::{JumpEvent, LookAtEvent},
    pathfinder::player_pos_to_block_pos,
};

type Edge = astar::Edge<RelBlockPos, MoveData>;

pub type SuccessorsFn = fn(&mut MovesCtx, RelBlockPos);

/// Re-implement certain bugs and quirks that Baritone has, and disable
/// movements that Baritone doesn't have.
///
/// Meant to help with debugging when directly comparing against Baritone.
pub const BARITONE_COMPAT: bool = false;

pub fn default_move(ctx: &mut MovesCtx, node: RelBlockPos) {
    basic::basic_move(ctx, node);
    parkour::parkour_move(ctx, node);
    uncommon::uncommon_move(ctx, node);
}

/// The default move set plus the Baritone-style terrain-modifying
/// primitives in [`building`] (bridging, staircasing, and pillaring
/// straight up). Pass this to [`super::PathfinderOpts::successors_fn`] so a
/// single pathfinding request considers walking, jumping, mining, placing,
/// bridging, and towering up together and lets A* pick whichever
/// combination is cheapest -- see [`building`] for why each of these moves
/// only fires where the default set can't already reach.
pub fn combined_move(ctx: &mut MovesCtx, node: RelBlockPos) {
    default_move(ctx, node);
    building::building_move(ctx, node);
}

#[derive(Clone)]
pub struct MoveData {
    /// Use the context to determine what events should be sent to complete this
    /// movement.
    pub execute: &'static (dyn Fn(ExecuteCtx) + Send + Sync),
    /// Whether we've reached the target.
    pub is_reached: &'static (dyn Fn(IsReachedCtx) -> bool + Send + Sync),
}
impl Debug for MoveData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MoveData")
            // .field("move_kind", &self.move_kind)
            .finish()
    }
}

pub struct ExecuteCtx<'s, 'w1, 'w2, 'w3, 'w4, 'w5, 'w6, 'w7, 'a> {
    pub entity: Entity,
    /// The node that we're trying to reach.
    pub target: BlockPos,
    /// The last node that we reached.
    pub start: BlockPos,
    pub position: Vec3,
    pub physics: &'a azalea_entity::Physics,
    pub is_currently_mining: bool,
    pub can_mine: bool,
    /// Whether block-placement moves are allowed to actually dispatch a
    /// placement this tick. `false` inside the simulation executor's
    /// no-op-on-the-world lookahead (mirrors `can_mine`), since placement has
    /// no client-side prediction to simulate against.
    pub can_place: bool,
    pub world: Arc<RwLock<World>>,
    pub menu: Menu,
    /// Same [`CustomPathfinderStateRef`] used during move generation (see
    /// [`MovesCtx::custom_state`]), so an execute closure can recover
    /// whatever policy generation used to decide to push this edge (e.g.
    /// which scaffold item to place) without it needing to be threaded
    /// through the `'static` [`MoveData::execute`] function pointer itself.
    /// Empty (not `None`) when the entity has no
    /// [`super::custom_state::CustomPathfinderState`] component.
    pub custom_state: Arc<RwLock<CustomPathfinderStateRef>>,
    /// Live snapshot of this entity's [`PillarClimbBaseline`] component, if
    /// any. A plain query field rather than something read through
    /// [`Self::custom_state`] -- see [`PillarClimbBaseline`]'s doc comment
    /// for why.
    pub pillar_climb_baseline: Option<PillarClimbBaseline>,

    pub commands: &'a mut Commands<'w1, 's>,
    pub look_at_events: &'a mut MessageWriter<'w2, LookAtEvent>,
    pub sprint_events: &'a mut MessageWriter<'w3, StartSprintEvent>,
    pub walk_events: &'a mut MessageWriter<'w4, StartWalkEvent>,
    pub jump_events: &'a mut MessageWriter<'w5, JumpEvent>,
    pub start_mining_events: &'a mut MessageWriter<'w6, StartMiningBlockEvent>,
    pub place_item_events: &'a mut MessageWriter<'w7, StartUseItemEvent>,
}

impl ExecuteCtx<'_, '_, '_, '_, '_, '_, '_, '_, '_> {
    pub fn on_tick_start(&mut self) {
        self.set_sneaking(false);
    }

    pub fn look_at(&mut self, position: Vec3) {
        self.look_at_events.write(LookAtEvent {
            entity: self.entity,
            position: Vec3 {
                x: position.x,
                // look forward
                y: self.position.up(1.53).y,
                z: position.z,
            },
        });
    }

    pub fn look_at_exact(&mut self, position: Vec3) {
        self.look_at_events.write(LookAtEvent {
            entity: self.entity,
            position,
        });
    }

    pub fn sprint(&mut self, direction: SprintDirection) {
        self.sprint_events.write(StartSprintEvent {
            entity: self.entity,
            direction,
        });
    }

    pub fn walk(&mut self, direction: WalkDirection) {
        self.walk_events.write(StartWalkEvent {
            entity: self.entity,
            direction,
        });
    }

    pub fn jump(&mut self) {
        self.jump_events.write(JumpEvent {
            entity: self.entity,
        });
    }

    fn set_sneaking(&mut self, sneaking: bool) {
        self.commands
            .entity(self.entity)
            .queue(move |mut entity: EntityWorldMut<'_>| {
                if let Some(mut physics_state) = entity.get_mut::<ClientMovementState>() {
                    physics_state.trying_to_crouch = sneaking;
                }
            });
    }
    pub fn sneak(&mut self) {
        self.set_sneaking(true);
    }

    pub fn jump_if_in_water(&mut self) {
        if self.physics.is_in_water() {
            self.jump();
        }
    }

    /// Returns whether this block could be mined.
    pub fn should_mine(&mut self, block: BlockPos) -> bool {
        let block_state = self.world.read().get_block_state(block).unwrap_or_default();
        should_mine_block_state(block_state)
    }

    /// Mine the block at the given position.
    ///
    /// Returns whether the block is being mined.
    pub fn mine(&mut self, block: BlockPos) -> bool {
        if !self.can_mine {
            return false;
        }

        let block_state = self.world.read().get_block_state(block).unwrap_or_default();
        if is_block_state_passable(block_state) {
            // block is already passable, no need to mine it
            return false;
        }

        let best_tool_result = best_tool_in_hotbar_for_block(block_state, &self.menu);
        trace!("best tool for {block_state:?}: {best_tool_result:?}");

        self.commands.trigger(SetSelectedHotbarSlotEvent {
            entity: self.entity,
            slot: best_tool_result.index as u8,
        });

        self.is_currently_mining = true;

        self.walk(WalkDirection::None);
        self.look_at_exact(block.center());
        self.start_mining_events.write(StartMiningBlockEvent {
            entity: self.entity,
            position: block,
            force: true,
        });

        true
    }

    /// Mine the given block, but make sure the player is standing at the start
    /// of the current node first.
    pub fn mine_while_at_start(&mut self, block: BlockPos) -> bool {
        let horizontal_distance_from_start = (self.start.center() - self.position)
            .horizontal_distance_squared()
            .sqrt();
        let at_start_position = player_pos_to_block_pos(self.position) == self.start
            && horizontal_distance_from_start < 0.25;

        if self.should_mine(block) {
            if at_start_position {
                self.look_at(block.center());
                self.mine(block);
            } else {
                self.look_at(self.start.center());
                self.walk(WalkDirection::Forward);
            }
            true
        } else {
            false
        }
    }

    pub fn get_block_state(&self, block: BlockPos) -> BlockState {
        self.world.read().get_block_state(block).unwrap_or_default()
    }

    /// Places a block by right-clicking `face` of `reference` (the new block
    /// ends up in the cell adjacent to `reference` on that side), selecting
    /// the first hotbar slot whose item satisfies `item_predicate`. Returns
    /// whether a placement was actually dispatched this tick.
    ///
    /// `reference` must already be a solid block the bot can see/reach; this
    /// never raycasts to find one. Unlike [`Self::mine`], placement has **no
    /// client-side prediction**: the world will not show the placed block
    /// until the server round-trips a block-update packet, arbitrarily many
    /// ticks later (see
    /// `azalea_client::interact::handle_start_use_item_queued`'s doc note).
    /// Callers must re-check world state on later ticks rather
    /// than assuming success, and should call [`Self::clear_placing`] once
    /// they observe it landed (or give up).
    pub fn place(
        &mut self,
        reference: BlockPos,
        face: Direction,
        item_predicate: impl Fn(&ItemStack) -> bool,
    ) -> bool {
        if !self.can_place {
            return false;
        }
        let Some(slot) = super::tool_policy::find_hotbar_item(&self.menu, item_predicate) else {
            return false;
        };
        self.commands.trigger(SetSelectedHotbarSlotEvent {
            entity: self.entity,
            slot,
        });
        self.walk(WalkDirection::None);
        self.look_at_exact(reference.center());
        self.place_item_events.write(StartUseItemEvent {
            entity: self.entity,
            hand: InteractionHand::MainHand,
            force_block: Some(reference),
            force_direction: Some(face),
        });
        let placed_at = reference.offset_with_direction(face);
        let entity = self.entity;
        self.commands
            .entity(entity)
            .insert(Placing { pos: placed_at });
        true
    }

    /// Removes the [`Placing`] timeout carve-out marker. Call this once a
    /// dispatched placement has been confirmed (or abandoned), otherwise
    /// `timeout_movement` will keep treating this entity as mid-placement.
    pub fn clear_placing(&mut self) {
        let entity = self.entity;
        self.commands.entity(entity).remove::<Placing>();
    }

    /// Marks that a build move (bridge/pillar/staircase) is stalled at the
    /// point where it would place a block, but has no scaffold item
    /// available to place with. Call every tick this is true -- like
    /// [`Self::place`]/[`Self::clear_placing`], this is re-evaluated fresh
    /// each tick rather than latched, so recovering material (a switch to a
    /// different allowed item, or picking more up) clears it automatically
    /// on whichever tick the move next has something to place with.
    ///
    /// `timeout_movement` uses the presence of this component (once its
    /// normal timeout elapses) to recognize the route as unrecoverable and
    /// cancel the goal outright, rather than repeatedly patching/retrying a
    /// route that ran out of material and has no way to get more on its
    /// own.
    pub fn mark_blocked_on_scaffold(&mut self) {
        let entity = self.entity;
        self.commands
            .entity(entity)
            .insert(BlockedOnMissingScaffold);
    }

    /// Clears [`Self::mark_blocked_on_scaffold`]'s marker. Call at the start
    /// of every build move's execute function, before any early return,
    /// same as [`ExecuteCtx::on_tick_start`] does for sneaking -- so a tick
    /// that doesn't hit the "no scaffold" branch never leaves a stale
    /// marker behind.
    pub fn clear_blocked_on_scaffold(&mut self) {
        let entity = self.entity;
        self.commands
            .entity(entity)
            .remove::<BlockedOnMissingScaffold>();
    }

    /// Persists a new [`PillarClimbBaseline`] for this entity. See
    /// [`Self::pillar_climb_baseline`] to read it back (on a later tick --
    /// like every other `Commands`-based write in this file, this doesn't
    /// take effect until the command is applied).
    pub fn set_pillar_climb_baseline(&mut self, baseline_y: i32) {
        let entity = self.entity;
        self.commands
            .entity(entity)
            .insert(PillarClimbBaseline { baseline_y });
    }

    /// Reads whatever custom state generation stored of type `T` (see
    /// [`MovesCtx::custom_state`]), or `T::default()` if none was inserted.
    pub fn custom<T: Clone + Default + Send + Sync + 'static>(&self) -> T {
        self.custom_state
            .read()
            .get::<T>()
            .cloned()
            .unwrap_or_default()
    }
}

/// Marks that a placement move dispatched a [`StartUseItemEvent`] and is
/// waiting for the server to acknowledge it landing. Placement has no
/// client-side prediction (unlike mining), so confirmation can take several
/// ticks; `timeout_movement` uses the presence of this component to avoid
/// cancelling a placement mid-flight, mirroring the existing carve-out for
/// `azalea_client::mining::Mining`.
#[derive(Clone, Copy, Component, Debug)]
pub struct Placing {
    pub pos: BlockPos,
}

/// See [`ExecuteCtx::mark_blocked_on_scaffold`].
#[derive(Clone, Copy, Component, Debug)]
pub struct BlockedOnMissingScaffold;

/// See [`tower::pillar_climb_height`]. A real ECS component (written via
/// [`ExecuteCtx::commands`], read back through [`ExecuteCtx::pillar_climb_baseline`])
/// rather than an entry in [`CustomPathfinderStateRef`]'s shared map: that
/// map is guarded by a single `RwLock` that a background A* search can hold
/// a read lock on for seconds at a time (see [`MovesCtx::custom_state`]'s
/// doc comment), so a `try_write` against it -- fine for the purely
/// cosmetic dedup logging elsewhere in this module, where a missed attempt
/// just means one repeated log line -- silently no-ops far too often to
/// safely gate a safety-critical height cap on. A plain component write
/// through `Commands` never contends with that lock at all.
#[derive(Clone, Copy, Component, Debug)]
pub struct PillarClimbBaseline {
    pub baseline_y: i32,
}

pub fn should_mine_block_state(block_state: BlockState) -> bool {
    if is_block_state_passable(block_state) || BlockKind::from(block_state) == BlockKind::Water {
        // block is already passable, no need to mine it
        return false;
    }

    true
}

pub struct IsReachedCtx<'a> {
    /// The node that we're trying to reach.
    pub target: BlockPos,
    /// The last node that we reached.
    pub start: BlockPos,
    pub position: Vec3,
    pub physics: &'a azalea_entity::Physics,
    /// Live (uncached) world handle, so a placement move's `is_reached` can
    /// confirm the placed block actually landed server-side rather than
    /// relying on position/physics alone (placement has no client-side
    /// prediction, unlike mining).
    pub world: Arc<RwLock<World>>,
}

/// Returns whether the entity is at the node and should start going to the
/// next node.
#[must_use]
pub fn default_is_reached(
    IsReachedCtx {
        position,
        target,
        physics,
        ..
    }: IsReachedCtx,
) -> bool {
    let block_pos = player_pos_to_block_pos(position);
    if block_pos == target {
        return true;
    }
    // it's fine if we go over the target while swimming
    if physics.is_in_water() && block_pos.down(1) == target {
        return true;
    }

    false
}

/// Stricter `is_reached` used by every terrain-modifying move that places
/// its own support block underfoot ([`build::bridge_move`],
/// [`build::staircase_up_move`], [`tower::pillar_up_move`]) instead of
/// [`default_is_reached`].
///
/// Without this, a path could advance past a placement step just because a
/// jump's apex briefly matched `target`'s position, even though the placed
/// block hasn't actually landed server-side yet and the bot is about to
/// fall back down.
///
/// `on_ground()` alone is not enough, either -- a player hitbox is wider
/// than one block (0.6 wide, centered on `position`), so while still
/// sneak-walking toward the edge of the block it's standing on, a single
/// tick's movement can carry the *center* point past the target column's
/// boundary while the hitbox still has just enough overlap with the block
/// behind it to read as grounded. That combination (`block_pos == target`
/// and `on_ground()`) used to satisfy this check before the build move's
/// own approach-then-place sequence ever got a tick where its edge
/// condition was true, so the bridge/staircase/pillar step it was building
/// toward got marked "reached" without its supporting block ever having
/// been placed -- stranding the bot one column short, with no floor under
/// it and no way to satisfy the (now unreachable, since it has nothing to
/// stand on to get closer) approach condition for the *next* edge.
/// Confirming the block actually below `target` is solid closes that race:
/// it can only pass once the placement has genuinely landed, no matter how
/// the approaching footwork happened to line up against the tick boundary.
#[must_use]
pub(crate) fn vertical_is_reached(
    IsReachedCtx {
        position,
        target,
        physics,
        world,
        ..
    }: IsReachedCtx,
) -> bool {
    let support_confirmed = !is_block_state_passable(
        world
            .read()
            .get_block_state(target.down(1))
            .unwrap_or_default(),
    );
    let block_pos_matches = player_pos_to_block_pos(position) == target;
    let grounded = physics.on_ground() || physics.is_in_water();
    block_pos_matches && grounded && support_confirmed
}

pub struct MovesCtx<'a> {
    pub edges: &'a mut Vec<Edge>,
    pub world: &'a CachedWorld,
    pub mining_cache: &'a MiningCache,
    pub custom_state: &'a CustomPathfinderStateRef,
}
