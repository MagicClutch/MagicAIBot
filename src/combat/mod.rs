//! Standalone PvP combat: `#kill <player>`.
//!
//! The fight is **full aggression** end to end: the bot closes the gap,
//! stays in the target's face, eats mid-fight without ever backing off, and
//! keeps swinging until the target is dead. Nothing in here retreats,
//! flees, or ends a fight because the bot is hurt -- see [`executor`]'s
//! module doc comment for the one thing that briefly pauses attacking (a
//! bite in progress) and why.
//!
//! Deliberately independent of `crate::movement`/`crate::navigation` (the
//! pathfinder-backed systems every other command -- `/goto`, `/follow`,
//! `#get`, `#mine` -- shares) and of `crate::mobs::combat` (the
//! pathfinder-backed mob-hunting extension of `#get`). Both of those reuse
//! Azalea's A* pathfinder to walk to a destination and simply stop there;
//! PvP against a player needs continuous positioning relative to a
//! constantly-moving target instead -- strafing, distance maintenance,
//! sprint resets, crit timing -- which is a different problem with a
//! different, much simpler movement model (see [`movement`]), not a
//! bigger version of the same one. Keeping it in its own module tree with
//! its own state machine means neither system can regress the other.
//!
//! What *is* reused, deliberately, rather than reimplemented: the look
//! controller (`crate::look`, via `look::LookTarget::PredictedPlayer`) for
//! aiming, `crate::minecraft::client::MinecraftClient` for every actual
//! network action (attack packets, raw walk/sprint/jump input, hotbar
//! selection), and `crate::equipment::tools`/`crate::equipment::manager`
//! for weapon ranking and equipping. See each submodule's own doc comment
//! for specifics.
//!
//! - [`state`] -- plain state/snapshot data, including the
//!   [`state::CombatPhase`] state machine.
//! - [`targeting`] -- pure distance/prediction/staleness math.
//! - [`movement`] -- the combat-only continuous steering controller (see
//!   its own module doc comment for why `#kill` does not share the
//!   project's navigation stack).
//! - [`terrain_probe`] -- lightweight local obstacle/hazard reading for
//!   that controller.
//! - [`crits`] -- pure attack-cooldown and critical-hit-timing decisions.
//! - [`shield_break`] -- pure axe-switch policy for breaking the *target's*
//!   shield.
//! - [`defense`] -- pure policy for the bot's *own* shield use.
//! - [`health`] -- pure health-based behavior mode
//!   ([`health::CombatMode`]).
//! - [`heal`] -- pure food-selection policy.
//! - [`executor`] -- per-tick async orchestration: turns the pure
//!   decisions above into actual client calls.
//! - [`kill`] -- [`kill::KillController`], the public controller `App`
//!   drives for `#kill`/`/kill`.

pub mod crits;
pub mod defense;
pub mod executor;
pub mod heal;
pub mod health;
pub mod kill;
pub mod movement;
pub mod shield_break;
pub mod state;
pub mod targeting;
pub mod terrain_probe;

pub use kill::KillController;
