//! The Wind Charge self-launch state machine: throw a Wind Charge straight
//! down at the bot's own feet, wait for the resulting knockback to actually
//! launch it into the air, then hand off to the Mace's existing opportunistic
//! smash-attack handling (`combat::mace`, driven from `combat::executor`'s
//! `apply_attack`) for the fall. Pure -- no Azalea, no I/O, no clocks beyond
//! the `Instant` it is handed.
//!
//! ```text
//! Idle              nothing in progress
//!  -> Preparing         Wind Charge chosen; slot being selected
//!  -> WaitingForSlotAck slot selected, waiting for the *server* to confirm
//!  -> AimingDown        aiming straight down, waiting for the camera to
//!                       actually get there
//!  -> WaitingForLaunch  use-item sent; waiting for the knockback to
//!                       actually lift the bot off the ground
//!  -> Idle
//! ```
//!
//! # Reactive, not simulated
//!
//! Nothing here assumes a travel time, a blast radius, or a resulting
//! launch velocity -- every step waits for something the bot can actually
//! *observe* (a hotbar ack, the camera reaching a steep pitch, `velocity_y`
//! actually spiking upward) rather than a fixed delay, and every wait is
//! bounded so a throw that fizzles (no line of sight to the ground, a
//! desync, a cancelled interaction) can't strand the bot standing still
//! forever. This mirrors `combat::consume`'s exact reasoning for the same
//! kind of problem -- see that module's doc comment.
//!
//! # What this deliberately does not do
//!
//! No aim/positioning during the fall itself, and no attempt to land
//! exactly on the target -- the bot self-launches from wherever it already
//! is (in melee range, by construction -- see `combat::executor`'s trigger
//! check) and whatever height results is whatever height results. A target
//! that moves away mid-launch can still cause a whiff; there is no aerial
//! steering to correct for it.

use std::time::{Duration, Instant};

pub const WIND_CHARGE_ITEM_ID: &str = "minecraft:wind_charge";

/// How steep the camera pitch must be (degrees, 90 is straight down) before
/// the throw is sent. Short of exactly 90 on purpose: the interpolated
/// `Precise` look motion approaches its target asymptotically in practice,
/// and demanding an exact match would mean never quite firing.
pub const AIM_DOWN_PITCH_THRESHOLD: f32 = 80.0;

/// How long to wait for the server to acknowledge the Wind Charge's hotbar
/// slot before giving up on this attempt. See `consume::SLOT_ACK_TIMEOUT`'s
/// doc comment for the same reasoning -- this is the identical mechanism,
/// kept as its own constant so the two modules stay decoupled.
pub const SLOT_ACK_TIMEOUT: Duration = Duration::from_millis(500);

/// How long to wait for the camera to actually reach [`AIM_DOWN_PITCH_THRESHOLD`].
/// Generous relative to a `Precise` look's normal settle time: this is a
/// large (typically 90+ degree) pitch swing, not the small corrections most
/// precise interactions make.
pub const AIM_TIMEOUT: Duration = Duration::from_millis(700);

/// How long to wait, after the throw is sent, for the resulting knockback to
/// actually lift the bot -- see [`LAUNCH_VELOCITY_THRESHOLD`]. The
/// projectile only has to cross the gap from eye height to the ground right
/// underneath the bot, so this is generous slack over what should normally
/// be well under a second.
pub const LAUNCH_TIMEOUT: Duration = Duration::from_millis(1000);

/// Minimum upward `velocity_y` that counts as "the Wind Charge actually
/// launched the bot" rather than an incidental small hop -- comfortably
/// above a normal jump's peak (~0.42) so a stray jump elsewhere in the tick
/// can't be mistaken for a successful launch.
pub const LAUNCH_VELOCITY_THRESHOLD: f64 = 0.6;

/// How long after a finished attempt (successful or not) before another may
/// begin -- so a bot that keeps re-entering melee range doesn't burn through
/// its Wind Charges throwing one every single tick, and gets a few ordinary
/// grounded hits in between combos.
pub const ATTEMPT_COOLDOWN: Duration = Duration::from_secs(3);

/// Where a self-launch attempt currently is.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WindLaunchState {
    #[default]
    Idle,
    /// Wind Charge chosen; hand being cleared and the slot selected.
    Preparing,
    /// Slot selected; waiting for `acknowledged_hotbar_slot()` to match --
    /// see `consume::ConsumeState::WaitingForSlotAck`'s doc comment for why
    /// sending anything before this lands acts on the *previous* item.
    WaitingForSlotAck,
    /// Aiming the camera straight down; waiting for it to actually get
    /// there before the throw is sent -- for the same reason as above:
    /// sending use-item before the rotation packet lands throws in
    /// whatever direction the bot was previously facing.
    AimingDown,
    /// Use-item sent; waiting for the resulting knockback to lift the bot.
    WaitingForLaunch,
}

impl WindLaunchState {
    /// Whether a self-launch attempt is in progress at all -- the caller's
    /// signal to suppress the ordinary weapon-selection/attack/movement
    /// this tick, mirroring `ConsumeState::is_active`.
    #[must_use]
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Idle)
    }
}

/// What the driver should do after [`WindLaunchMachine::advance`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WindLaunchAction {
    /// Nothing to do this tick; stay where you are.
    Wait,
    /// Select the reserved Wind Charge hotbar slot and move to
    /// `WaitingForSlotAck`.
    SelectSlot,
    /// The slot is confirmed: command the camera straight down.
    AimDown,
    /// The camera is confirmed pointed down: send use-item once.
    Throw,
    /// `velocity_y` just crossed [`LAUNCH_VELOCITY_THRESHOLD`] -- the bot is
    /// airborne. Equip the Mace and restore the normal aim; the fall itself
    /// is `combat::mace`'s job from here.
    Launched,
    /// This attempt is over without a launch. `retry_in` is how long until
    /// [`WindLaunchMachine::ready_to_start`] allows another.
    Failed {
        reason: WindLaunchFailure,
        retry_in: Duration,
    },
}

/// Why an attempt was abandoned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindLaunchFailure {
    /// The server never confirmed the Wind Charge's hotbar slot.
    SlotNotAcknowledged,
    /// The camera never reached [`AIM_DOWN_PITCH_THRESHOLD`] in time.
    AimNeverSettled,
    /// Use-item was sent but `velocity_y` never crossed
    /// [`LAUNCH_VELOCITY_THRESHOLD`] -- the throw missed, was blocked, or
    /// otherwise fizzled.
    NeverLaunched,
}

impl WindLaunchFailure {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::SlotNotAcknowledged => "hotbar slot never acknowledged",
            Self::AimNeverSettled => "camera never settled facing down",
            Self::NeverLaunched => "no launch detected",
        }
    }
}

/// What the world says right now, as far as the machine cares.
#[derive(Clone, Copy, Debug)]
pub struct WindLaunchObservation {
    /// The hotbar slot the *server* has been told is held, if known.
    pub acknowledged_slot: Option<u8>,
    /// Whether the camera is currently pointed down past
    /// [`AIM_DOWN_PITCH_THRESHOLD`].
    pub aim_settled: bool,
    pub on_ground: bool,
    /// Minecraft's raw vertical velocity (positive while rising).
    pub velocity_y: f64,
    pub now: Instant,
}

/// One self-launch attempt, start to finish.
#[derive(Clone, Debug)]
pub struct WindLaunchMachine {
    state: WindLaunchState,
    /// When the current state was entered, for its deadline.
    since: Instant,
    /// When the next attempt may begin, after this one finished (either way).
    next_attempt_after: Option<Instant>,
}

impl WindLaunchMachine {
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            state: WindLaunchState::Idle,
            since: now,
            next_attempt_after: None,
        }
    }

    #[must_use]
    pub fn state(&self) -> WindLaunchState {
        self.state
    }

    /// Whether a new attempt is allowed to start right now, given the
    /// cooldown after the last one.
    #[must_use]
    pub fn ready_to_start(&self, now: Instant) -> bool {
        self.state == WindLaunchState::Idle
            && self.next_attempt_after.is_none_or(|after| now >= after)
    }

    /// Forgets everything, including the cooldown. For a new fight.
    pub fn reset(&mut self, now: Instant) {
        self.state = WindLaunchState::Idle;
        self.since = now;
        self.next_attempt_after = None;
    }

    /// Begins a new attempt -- the caller has already swapped the Wind
    /// Charge into its reserved slot; this only starts the state machine
    /// that selects it.
    pub fn begin(&mut self, now: Instant) {
        self.state = WindLaunchState::Preparing;
        self.since = now;
    }

    /// Records that the caller has sent the slot selection.
    pub fn slot_selected(&mut self, now: Instant) {
        if self.state == WindLaunchState::Preparing {
            self.state = WindLaunchState::WaitingForSlotAck;
            self.since = now;
        }
    }

    /// Records that the caller has commanded the aim-down look.
    pub fn aim_started(&mut self, now: Instant) {
        if self.state == WindLaunchState::WaitingForSlotAck {
            self.state = WindLaunchState::AimingDown;
            self.since = now;
        }
    }

    /// Records that the caller has sent use-item.
    pub fn thrown(&mut self, now: Instant) {
        if self.state == WindLaunchState::AimingDown {
            self.state = WindLaunchState::WaitingForLaunch;
            self.since = now;
        }
    }

    /// Advances the machine against what the world currently reports, and
    /// says what the driver should do next.
    pub fn advance(&mut self, observation: WindLaunchObservation) -> WindLaunchAction {
        let elapsed = observation.now.saturating_duration_since(self.since);
        match self.state {
            WindLaunchState::Idle => WindLaunchAction::Wait,
            WindLaunchState::Preparing => WindLaunchAction::SelectSlot,
            WindLaunchState::WaitingForSlotAck => {
                if observation.acknowledged_slot.is_some() {
                    return WindLaunchAction::AimDown;
                }
                if elapsed >= SLOT_ACK_TIMEOUT {
                    return self.fail(WindLaunchFailure::SlotNotAcknowledged, observation.now);
                }
                WindLaunchAction::Wait
            }
            WindLaunchState::AimingDown => {
                if observation.aim_settled {
                    return WindLaunchAction::Throw;
                }
                if elapsed >= AIM_TIMEOUT {
                    return self.fail(WindLaunchFailure::AimNeverSettled, observation.now);
                }
                WindLaunchAction::Wait
            }
            WindLaunchState::WaitingForLaunch => {
                if !observation.on_ground && observation.velocity_y >= LAUNCH_VELOCITY_THRESHOLD {
                    self.finish(observation.now);
                    return WindLaunchAction::Launched;
                }
                if elapsed >= LAUNCH_TIMEOUT {
                    return self.fail(WindLaunchFailure::NeverLaunched, observation.now);
                }
                WindLaunchAction::Wait
            }
        }
    }

    fn fail(&mut self, reason: WindLaunchFailure, now: Instant) -> WindLaunchAction {
        self.state = WindLaunchState::Idle;
        self.since = now;
        self.next_attempt_after = Some(now + ATTEMPT_COOLDOWN);
        WindLaunchAction::Failed {
            reason,
            retry_in: ATTEMPT_COOLDOWN,
        }
    }

    fn finish(&mut self, now: Instant) {
        self.state = WindLaunchState::Idle;
        self.since = now;
        self.next_attempt_after = Some(now + ATTEMPT_COOLDOWN);
    }
}

impl Default for WindLaunchMachine {
    fn default() -> Self {
        Self::new(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> Instant {
        use std::sync::OnceLock;
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        *ORIGIN.get_or_init(Instant::now)
    }

    fn at(millis: u64) -> Instant {
        origin() + Duration::from_millis(millis)
    }

    fn observation(millis: u64) -> WindLaunchObservation {
        WindLaunchObservation {
            acknowledged_slot: None,
            aim_settled: false,
            on_ground: true,
            velocity_y: 0.0,
            now: at(millis),
        }
    }

    /// Drives a clean, fully successful launch and returns the machine.
    fn launch_successfully() -> WindLaunchMachine {
        let mut machine = WindLaunchMachine::new(at(0));
        machine.begin(at(0));
        assert_eq!(
            machine.advance(observation(0)),
            WindLaunchAction::SelectSlot
        );
        machine.slot_selected(at(0));

        let mut acked = observation(50);
        acked.acknowledged_slot = Some(6);
        assert_eq!(machine.advance(acked), WindLaunchAction::AimDown);
        machine.aim_started(at(50));

        // Still swinging the camera down.
        assert_eq!(machine.advance(observation(150)), WindLaunchAction::Wait);

        let mut settled = observation(300);
        settled.aim_settled = true;
        assert_eq!(machine.advance(settled), WindLaunchAction::Throw);
        machine.thrown(at(300));

        // Thrown, not airborne yet.
        let mut still_grounded = observation(350);
        still_grounded.on_ground = true;
        assert_eq!(machine.advance(still_grounded), WindLaunchAction::Wait);

        let mut launched = observation(500);
        launched.on_ground = false;
        launched.velocity_y = 1.2;
        assert_eq!(machine.advance(launched), WindLaunchAction::Launched);
        machine
    }

    #[test]
    fn a_new_machine_is_idle_and_ready() {
        let machine = WindLaunchMachine::new(at(0));
        assert_eq!(machine.state(), WindLaunchState::Idle);
        assert!(machine.ready_to_start(at(0)));
        assert!(!machine.state().is_active());
    }

    #[test]
    fn a_full_launch_succeeds_end_to_end() {
        let machine = launch_successfully();
        assert_eq!(machine.state(), WindLaunchState::Idle);
        assert!(
            !machine.ready_to_start(at(500)),
            "the post-attempt cooldown applies even on success"
        );
        assert!(machine.ready_to_start(at(500 + ATTEMPT_COOLDOWN.as_millis() as u64)));
    }

    #[test]
    fn use_is_not_sent_until_the_slot_is_acknowledged() {
        let mut machine = WindLaunchMachine::new(at(0));
        machine.begin(at(0));
        machine.slot_selected(at(0));
        assert_eq!(machine.advance(observation(100)), WindLaunchAction::Wait);
        let mut acked = observation(200);
        acked.acknowledged_slot = Some(6);
        assert_eq!(machine.advance(acked), WindLaunchAction::AimDown);
    }

    #[test]
    fn the_throw_is_not_sent_until_the_camera_settles() {
        let mut machine = WindLaunchMachine::new(at(0));
        machine.begin(at(0));
        machine.slot_selected(at(0));
        let mut acked = observation(0);
        acked.acknowledged_slot = Some(6);
        machine.advance(acked);
        machine.aim_started(at(0));

        assert_eq!(machine.advance(observation(100)), WindLaunchAction::Wait);
        let mut settled = observation(200);
        settled.aim_settled = true;
        assert_eq!(machine.advance(settled), WindLaunchAction::Throw);
    }

    #[test]
    fn an_unacknowledged_slot_fails_and_schedules_a_retry() {
        let mut machine = WindLaunchMachine::new(at(0));
        machine.begin(at(0));
        machine.slot_selected(at(0));
        let action = machine.advance(observation(SLOT_ACK_TIMEOUT.as_millis() as u64));
        assert_eq!(
            action,
            WindLaunchAction::Failed {
                reason: WindLaunchFailure::SlotNotAcknowledged,
                retry_in: ATTEMPT_COOLDOWN,
            }
        );
        assert_eq!(machine.state(), WindLaunchState::Idle);
        assert!(!machine.ready_to_start(at(SLOT_ACK_TIMEOUT.as_millis() as u64)));
    }

    #[test]
    fn a_camera_that_never_settles_fails_and_schedules_a_retry() {
        let mut machine = WindLaunchMachine::new(at(0));
        machine.begin(at(0));
        machine.slot_selected(at(0));
        let mut acked = observation(0);
        acked.acknowledged_slot = Some(6);
        machine.advance(acked);
        machine.aim_started(at(0));

        let action = machine.advance(observation(AIM_TIMEOUT.as_millis() as u64));
        assert_eq!(
            action,
            WindLaunchAction::Failed {
                reason: WindLaunchFailure::AimNeverSettled,
                retry_in: ATTEMPT_COOLDOWN,
            }
        );
        assert_eq!(machine.state(), WindLaunchState::Idle);
    }

    #[test]
    fn a_throw_that_never_launches_fails_and_schedules_a_retry() {
        let mut machine = WindLaunchMachine::new(at(0));
        machine.begin(at(0));
        machine.slot_selected(at(0));
        let mut acked = observation(0);
        acked.acknowledged_slot = Some(6);
        machine.advance(acked);
        machine.aim_started(at(0));
        let mut settled = observation(0);
        settled.aim_settled = true;
        machine.advance(settled);
        machine.thrown(at(0));

        // Still on the ground the whole time -- never launched.
        let action = machine.advance(observation(LAUNCH_TIMEOUT.as_millis() as u64));
        assert_eq!(
            action,
            WindLaunchAction::Failed {
                reason: WindLaunchFailure::NeverLaunched,
                retry_in: ATTEMPT_COOLDOWN,
            }
        );
        assert_eq!(machine.state(), WindLaunchState::Idle);
    }

    #[test]
    fn a_small_incidental_hop_does_not_count_as_a_launch() {
        let mut machine = WindLaunchMachine::new(at(0));
        machine.begin(at(0));
        machine.slot_selected(at(0));
        let mut acked = observation(0);
        acked.acknowledged_slot = Some(6);
        machine.advance(acked);
        machine.aim_started(at(0));
        let mut settled = observation(0);
        settled.aim_settled = true;
        machine.advance(settled);
        machine.thrown(at(0));

        // A normal jump peaks around 0.42 -- well under the threshold.
        let mut hop = observation(100);
        hop.on_ground = false;
        hop.velocity_y = 0.42;
        assert_eq!(machine.advance(hop), WindLaunchAction::Wait);
    }

    #[test]
    fn no_state_can_persist_past_its_deadline() {
        for setup in 0..3 {
            let mut machine = WindLaunchMachine::new(at(0));
            machine.begin(at(0));
            if setup >= 1 {
                machine.slot_selected(at(0));
            }
            if setup >= 2 {
                let mut acked = observation(0);
                acked.acknowledged_slot = Some(6);
                machine.advance(acked);
                machine.aim_started(at(0));
            }
            let action = machine.advance(observation(60_000));
            if setup == 0 {
                assert_eq!(action, WindLaunchAction::SelectSlot);
            } else {
                assert!(
                    matches!(action, WindLaunchAction::Failed { .. }),
                    "setup {setup} hung in {:?}",
                    machine.state()
                );
                assert_eq!(machine.state(), WindLaunchState::Idle);
            }
        }
    }

    #[test]
    fn out_of_order_transitions_are_ignored_rather_than_corrupting_the_state() {
        let mut machine = WindLaunchMachine::new(at(0));
        machine.slot_selected(at(0));
        machine.aim_started(at(0));
        machine.thrown(at(0));
        assert_eq!(machine.state(), WindLaunchState::Idle);
    }

    #[test]
    fn resetting_clears_the_cooldown_too() {
        let mut machine = launch_successfully();
        assert!(!machine.ready_to_start(at(500)));
        machine.reset(at(500));
        assert!(
            machine.ready_to_start(at(500)),
            "a fight reset clears everything"
        );
    }

    #[test]
    fn failures_describe_themselves_for_logging() {
        assert_eq!(
            WindLaunchFailure::NeverLaunched.label(),
            "no launch detected"
        );
        assert_eq!(
            WindLaunchFailure::SlotNotAcknowledged.label(),
            "hotbar slot never acknowledged"
        );
        assert_eq!(
            WindLaunchFailure::AimNeverSettled.label(),
            "camera never settled facing down"
        );
    }
}
