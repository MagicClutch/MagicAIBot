//! Baritone-style hierarchical long-distance navigation.
//!
//! # Why this exists next to `crate::movement`
//!
//! `crate::movement::MovementService` submits a single goal to Azalea's own
//! pathfinder and watches it execute. That is exactly right for a 30-block
//! hop and structurally wrong for a 5000-block trip: one flat A* over
//! terrain that mostly isn't loaded yet, inside a fixed search timeout, with
//! nothing to fall back on when a chunk changes halfway through. This module
//! is the layer above it -- it decides *where to go next*, in slices small
//! enough to be computed against terrain that genuinely exists, and hands
//! each slice down to the existing movement layer to execute.
//!
//! # The split, and why it is drawn here
//!
//! - **This module plans.** Route, segments, waypoints, costs, hazards,
//!   replanning, chunk knowledge.
//! - **Azalea executes.** Each waypoint hop is handed to
//!   `MovementService`/Azalea's pathfinder, which already does jump timing,
//!   mining, bridging and stuck recovery well (see
//!   `MinecraftClient::start_navigation_to`'s `PathfindingPolicy`).
//!   Reimplementing that would trade a tuned, working executor for a new
//!   one, which is a regression however good the planner is.
//!
//! Every hop handed down is short and inside terrain this module has already
//! verified is loaded and walkable, which is precisely the regime Azalea's
//! pathfinder is reliable in.
//!
//! # Data flow
//!
//! ```text
//! destination
//!     |
//!     v
//! route::plan          coarse chunk-level corridor to the destination
//!     |                (cheap; runs over 16-block cells, not blocks)
//!     v
//! segment::SegmentPlan long path sliced into ~48-block segments
//!     |                future segments stay *planned*, not calculated
//!     v
//! planner::Planner     async, cancellable block-level A* for the *next*
//!     |                segment only, on a sampled snapshot of the world
//!     v
//! executor             walks the calculated waypoints via MovementService
//!     |
//!     v
//! controller           the state machine tying it together, and the only
//!                      thing `App` talks to
//! ```
//!
//! # Module map
//!
//! - [`terrain`] -- block id -> navigation class. Pure.
//! - [`grid`] -- the owned cuboid snapshot every search runs on. Pure.
//! - [`cost`] -- the weighted cost model. Pure.
//! - [`moves`] -- successor generation (what the bot can physically do).
//!   Pure.
//! - [`astar`] -- budgeted, cancellable A*. Pure, CPU-bound.
//! - [`route`] -- high-level chunk-level route planning. Pure.
//! - [`segment`] -- path slices and the segment manager. Pure.
//! - [`world_cache`] -- the navigation cache: chunk-keyed terrain knowledge
//!   with TTL and invalidation. Pure.
//! - [`sampler`] -- the only module that reads Azalea's world; turns loaded
//!   chunks into [`grid::TerrainGrid`]s. Async.
//! - [`planner`] -- runs searches off the runtime thread, cancellably.
//!   Async.
//! - [`executor`] -- drives the bot along a calculated segment. Async.
//! - [`state`] -- the navigation state machine and its snapshot. Pure.
//! - [`controller`] -- [`controller::PathfindingController`], the public API
//!   `App` drives. Async.
//! - [`debug`] -- `debug_pathfinding` output. Pure formatting.

pub mod astar;
pub mod controller;
pub mod cost;
pub mod debug;
pub mod executor;
pub mod grid;
pub mod moves;
pub mod planner;
pub mod route;
pub mod sampler;
pub mod segment;
pub mod state;
pub mod terrain;
pub mod world_cache;

pub use controller::PathfindingController;
pub use state::NavigationState;
