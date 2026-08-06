//! Plain state/snapshot data for the PvP kill task -- no Azalea types, no
//! I/O, mirroring the shape every other controller's snapshot in this
//! codebase already uses (`mobs::combat::CombatSnapshot`,
//! `InteractionSnapshot`, `LookSnapshot`, ...).

use std::time::SystemTime;

use uuid::Uuid;

use crate::{combat::health::CombatMode, minecraft::world_state::PositionSnapshot};

/// Lifecycle of a `#kill <player>` task, per the spec's required state set.
/// `Created` is the default, pre-`start()` state (equivalent to every other
/// controller's `Idle`); `Running` covers the whole active fight -- there is
/// no separate "acquiring" state because resolving a *named* player is a
/// single, synchronous world-state lookup (see `kill::KillController::start`),
/// not a search loop the way `mobs::combat::CombatController::kill_nearest`'s
/// nearest-candidate search over an unnamed mob type is. See
/// [`CombatPhase`] for the finer-grained state machine *within* `Running`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum KillState {
    #[default]
    Created,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// The fine-grained combat state machine within [`KillState::Running`],
/// per the spec's required phase set. Recomputed fresh every tick from
/// current conditions (distance, health mode, shield/cooldown state, ...)
/// rather than enforced through a strict transition table -- several of
/// these are simultaneously true in a real fight (the bot is always
/// strafing *and* often also preparing a crit, for instance), so this
/// reports whichever phase best describes what `crate::combat::executor`
/// is actually doing this tick, in the priority order laid out in
/// `executor::compute_phase`. Exists primarily for status/log transparency
/// -- like `KillState`, nothing besides logging reads it back today.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CombatPhase {
    #[default]
    Idle,
    AcquireTarget,
    Engage,
    Strafe,
    CritPrep,
    Attack,
    ShieldBreak,
    Defensive,
    Heal,
    Chase,
    Reposition,
    Finish,
    Abort,
}

#[derive(Clone, Debug, Default)]
pub struct KillSnapshot {
    pub state: KillState,
    pub phase: CombatPhase,
    /// The bot's own health-based aggression tier this tick -- see
    /// `crate::combat::health`.
    pub mode: CombatMode,
    pub target_name: Option<String>,
    /// Kept for parity with every other controller's snapshot in this
    /// codebase (`mobs::combat::CombatSnapshot`, `InteractionSnapshot`,
    /// `LookSnapshot`, ...); unlike those, nothing currently reads it back
    /// since `#kill` has no dedicated status command.
    #[allow(dead_code)]
    pub target_uuid: Option<Uuid>,
    pub target_position: Option<PositionSnapshot>,
    #[allow(dead_code)]
    pub started_at: Option<SystemTime>,
    pub failure_reason: Option<String>,
    /// Whether the target was seen actively blocking on the most recent
    /// tick that could observe it -- see `crate::combat::shield_break`.
    pub shield_detected: bool,
    pub hits_landed: u32,
    pub crits_landed: u32,
}
