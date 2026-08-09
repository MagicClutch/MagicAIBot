//! Tower building / pillar climbing: place a block beneath the bot's own
//! feet while airborne and climb straight up, column by column. This is the
//! pathfinder's only "ascend in place" primitive -- Minecraft has no move
//! that goes straight up without either a block to jump onto or one to
//! place, so [`crate::pathfinder::moves::default_move`] has no edge for it
//! at all and this never competes with an existing move.
//!
//! Split into three pieces, mirroring "plan it, do it, configure it":
//! - [`planner`]: decides *whether* a pillar-up edge can be offered from a
//!   given position ([`pillar_up_move`], the actual `SuccessorsFn` entry
//!   point A* calls -- re-exported here as this module's public surface).
//! - [`execute`]: decides *what to do this tick* once a pillar-up edge is
//!   the active step of a route -- the sneak/jump/place cycle below.
//! - [`config`]: the one tower-up-specific tuning constant that isn't
//!   already part of the shared [`crate::pathfinder::policy::PathfindingPolicy`]
//!   (see that module's doc comment for why the rest of tower-up's
//!   configuration deliberately lives there instead of being duplicated
//!   here).
//!
//! # Minecraft tick timing
//!
//! Everything here runs on the 20-ticks-per-second `GameTick` schedule, the
//! same clock the vanilla client and server share. A single pillar step is:
//!
//! 1. Stand centered on the column, sneaking (see
//!    [`execute::execute_pillar_up_move`]'s first few lines).
//! 2. Jump. A player's jump starts at ~0.42 blocks/tick of upward velocity
//!    and loses ~0.08 blocks/tick to gravity (plus a small amount of extra
//!    air drag) every tick after, so `velocity.y` counts down and crosses
//!    from positive (still rising) to zero-or-negative (falling) around 4-5
//!    ticks in -- that crossing point is the jump's apex, the instant a real
//!    player would time a click to place a block underfoot with the least
//!    margin for error.
//! 3. Once airborne *and* past that apex (`velocity.y <= 0.0`, see
//!    [`execute::execute_pillar_up_move`]), place a block directly below the
//!    column.
//! 4. Repeat once the bot has landed on the new block.
//!
//! Waiting for the apex specifically isn't needed for *correctness* here --
//! [`crate::pathfinder::moves::ExecuteCtx::place`] dispatches a direct
//! force-placement packet (`force_block`/`force_direction`) rather than
//! simulating a real client raycast/click, so it can't be rejected merely
//! by mistiming when the request goes out the way a human player's click
//! could be. It's done anyway because it's what the requirements this
//! module was built against ask for, it costs only a handful of ticks per
//! block, and it keeps the bot's placement cadence matching genuine
//! Minecraft jump physics instead of firing on literally the first airborne
//! tick.
//!
//! Sneaking is held for the entire jump-and-place sequence (not just the
//! placement instant): it stops the bot from being pushed off the column's
//! edge while airborne and waiting for the placement to land, and lets the
//! server accept a placement that would otherwise intersect the bot's own
//! hitbox (the same "ninja pillaring" technique a player uses).
//!
//! Like every other move in [`crate::pathfinder::moves::building::bridging`],
//! placement here has no client-side prediction, so
//! [`execute::execute_pillar_up_move`] re-checks live world state every
//! tick rather than assuming a placement dispatched on an earlier tick has
//! landed, and reuses [`crate::pathfinder::moves::vertical_is_reached`] so
//! a path never advances past a pillar step whose support block hasn't
//! actually been confirmed by the server yet.

mod config;
mod execute;
mod planner;

pub use planner::pillar_up_move;
