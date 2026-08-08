//! The consume state machine: the six states a golden apple passes through
//! from "the bot decided to eat" to "the sword is back in its hand". Pure --
//! no Azalea, no I/O, no clocks beyond the `Instant` it is handed.
//!
//! # Why this is a machine and not a pair of flags
//!
//! Vanilla item use is fragile: attacking, sprinting, selecting a hotbar
//! slot, raising a shield or clicking in the inventory all cancel it. A bite
//! therefore has to be an *atomic* action -- once started, nothing but an
//! emergency may touch the hand until it finishes -- and atomicity needs an
//! explicit answer to "am I in the middle of one right now", available to
//! every subsystem, on every tick, including the ticks where nothing
//! happened.
//!
//! The states, and what each is waiting for:
//!
//! ```text
//! Idle              nothing in progress
//!  -> Preparing         food chosen; shield down, slot being selected
//!  -> WaitingForSlotAck slot selected, waiting for the *server* to confirm
//!  -> Using             use-item sent; waiting for the stack to shrink
//!  -> Consumed          the bite landed
//!  -> RestoringWeapon   putting the weapon back
//!  -> Idle
//! ```
//!
//! Every non-terminal state carries a deadline. A machine that can only make
//! progress cannot get stuck holding an apple forever -- which is the exact
//! failure this replaces.

use std::time::{Duration, Instant};

/// How long to wait for the server to acknowledge the hotbar slot before
/// giving up on this attempt. Generous next to the single tick it normally
/// takes: the packet can be delayed by a busy inventory or a slow tick, and
/// abandoning a swap that was about to land just restarts the same race.
pub const SLOT_ACK_TIMEOUT: Duration = Duration::from_millis(500);

/// How long to wait for a started bite to actually consume something.
/// Vanilla food takes 1.6s; the slack covers latency and a dropped packet
/// without letting a silently-cancelled bite hold the hand indefinitely.
pub const USE_TIMEOUT: Duration = Duration::from_millis(2200);

/// Bound on the weapon-restore step, so a failed swap cannot strand the
/// machine outside `Idle` and block all future eating.
pub const RESTORE_TIMEOUT: Duration = Duration::from_millis(600);

/// Where a consume attempt currently is.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ConsumeState {
    #[default]
    Idle,
    /// Food chosen, hand being cleared and the slot selected.
    Preparing,
    /// Slot selected; waiting for `acknowledged_hotbar_slot()` to match.
    /// Sending use-item before this lands makes the server apply the use to
    /// the *previous* item -- the sword -- which is what left the bot
    /// holding an uneaten apple.
    WaitingForSlotAck,
    /// Use-item sent; the bite is in flight.
    Using,
    /// The stack shrank: the bite landed.
    Consumed,
    /// Putting the weapon back before combat resumes.
    RestoringWeapon,
}

impl ConsumeState {
    /// Whether a consume attempt is in progress at all.
    #[must_use]
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Idle)
    }

    /// Whether combat actions must be suppressed right now.
    ///
    /// True from the moment food is chosen until the bite has landed --
    /// every one of attacking, sprint resets, weapon swaps and shield
    /// raising cancels vanilla item use, so all of them wait. Deliberately
    /// *false* during `RestoringWeapon`: the bite is over by then, and the
    /// restore is itself a weapon swap.
    #[must_use]
    pub fn blocks_combat(self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::WaitingForSlotAck | Self::Using | Self::Consumed
        )
    }

    /// Whether the hand is committed to food -- what the inventory guard is
    /// raised for. Covers the restore too, since that is the combat code
    /// putting the weapon back and no other subsystem should race it.
    #[must_use]
    pub fn holds_hand(self) -> bool {
        self.is_active()
    }
}

/// What the driver should do after [`ConsumeMachine::advance`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConsumeAction {
    /// Nothing to do this tick; stay where you are.
    Wait,
    /// Select `hotbar_index` and move to `WaitingForSlotAck`.
    SelectSlot { hotbar_index: u8 },
    /// The slot is confirmed: send use-item once.
    StartUse,
    /// The bite landed; put the weapon back.
    RestoreWeapon,
    /// This attempt is over. `retry_in` is `Some` when another attempt is
    /// due after that delay, `None` when the retry budget is spent.
    Failed {
        reason: ConsumeFailure,
        retry_in: Option<Duration>,
    },
    /// Fully finished -- back to `Idle`, combat resumes.
    Finished,
}

/// Why an attempt was abandoned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsumeFailure {
    /// The server never confirmed the hotbar slot.
    SlotNotAcknowledged,
    /// Use-item was sent but nothing was ever consumed -- something
    /// cancelled the bite.
    Interrupted,
    /// The weapon restore didn't complete in time.
    RestoreStalled,
}

impl ConsumeFailure {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::SlotNotAcknowledged => "hotbar slot never acknowledged",
            Self::Interrupted => "use interrupted",
            Self::RestoreStalled => "weapon restore stalled",
        }
    }
}

/// Limits on how hard the machine tries.
#[derive(Clone, Copy, Debug)]
pub struct ConsumeLimits {
    /// Attempts allowed for one decision to eat, including the first.
    pub retry_limit: u32,
    /// Pause between a failed attempt and the next, so a consume that keeps
    /// being cancelled doesn't retry every tick.
    pub retry_delay: Duration,
}

impl Default for ConsumeLimits {
    fn default() -> Self {
        Self {
            retry_limit: 3,
            retry_delay: Duration::from_millis(150),
        }
    }
}

/// What the world says right now, as far as the machine cares.
#[derive(Clone, Copy, Debug)]
pub struct ConsumeObservation {
    /// The hotbar slot the *server* has been told is held, if known.
    pub acknowledged_slot: Option<u8>,
    /// How many of the food item the bot is holding.
    pub held_count: u32,
    /// Whether the weapon is back in hand (ends `RestoringWeapon`).
    pub weapon_restored: bool,
    pub now: Instant,
}

/// One consume attempt, start to finish.
#[derive(Clone, Debug)]
pub struct ConsumeMachine {
    state: ConsumeState,
    /// The food this attempt is eating, and the slot it lives in.
    item_id: Option<String>,
    hotbar_index: u8,
    /// How many were held when the bite started -- the bite has landed when
    /// the count drops below it.
    count_at_start: u32,
    /// When the current state was entered, for its deadline.
    since: Instant,
    /// Attempts made for the current decision to eat.
    attempts: u32,
    /// When the next attempt may begin, after a failure.
    retry_after: Option<Instant>,
}

impl ConsumeMachine {
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            state: ConsumeState::Idle,
            item_id: None,
            hotbar_index: 0,
            count_at_start: 0,
            since: now,
            attempts: 0,
            retry_after: None,
        }
    }

    #[must_use]
    pub fn state(&self) -> ConsumeState {
        self.state
    }

    #[must_use]
    pub fn item_id(&self) -> Option<&str> {
        self.item_id.as_deref()
    }

    #[must_use]
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Whether another attempt is allowed to start right now, given the
    /// retry pause.
    #[must_use]
    pub fn ready_to_start(&self, now: Instant) -> bool {
        self.state == ConsumeState::Idle && self.retry_after.is_none_or(|after| now >= after)
    }

    /// Forgets everything, including the retry budget. For a new fight, or
    /// when the decision to eat is withdrawn (the target dropped into
    /// finisher range).
    pub fn reset(&mut self, now: Instant) {
        self.state = ConsumeState::Idle;
        self.item_id = None;
        self.count_at_start = 0;
        self.since = now;
        self.attempts = 0;
        self.retry_after = None;
    }

    /// Begins an attempt on `item_id`, held in `hotbar_index`.
    pub fn begin(&mut self, item_id: String, hotbar_index: u8, held_count: u32, now: Instant) {
        self.state = ConsumeState::Preparing;
        self.item_id = Some(item_id);
        self.hotbar_index = hotbar_index;
        self.count_at_start = held_count;
        self.since = now;
        self.attempts += 1;
        self.retry_after = None;
    }

    /// Records that the caller has sent the slot selection.
    pub fn slot_selected(&mut self, now: Instant) {
        if self.state == ConsumeState::Preparing {
            self.state = ConsumeState::WaitingForSlotAck;
            self.since = now;
        }
    }

    /// Records that the caller has sent use-item.
    pub fn use_started(&mut self, held_count: u32, now: Instant) {
        if self.state == ConsumeState::WaitingForSlotAck {
            self.state = ConsumeState::Using;
            self.count_at_start = held_count;
            self.since = now;
        }
    }

    /// Records that the caller has asked for the weapon back.
    pub fn restore_started(&mut self, now: Instant) {
        if self.state == ConsumeState::Consumed {
            self.state = ConsumeState::RestoringWeapon;
            self.since = now;
        }
    }

    /// Advances the machine against what the world currently reports, and
    /// says what the driver should do next.
    pub fn advance(
        &mut self,
        observation: ConsumeObservation,
        limits: &ConsumeLimits,
    ) -> ConsumeAction {
        let elapsed = observation.now.saturating_duration_since(self.since);
        match self.state {
            ConsumeState::Idle => ConsumeAction::Wait,
            ConsumeState::Preparing => ConsumeAction::SelectSlot {
                hotbar_index: self.hotbar_index,
            },
            ConsumeState::WaitingForSlotAck => {
                if observation.acknowledged_slot == Some(self.hotbar_index) {
                    return ConsumeAction::StartUse;
                }
                if elapsed >= SLOT_ACK_TIMEOUT {
                    return self.fail(ConsumeFailure::SlotNotAcknowledged, observation.now, limits);
                }
                ConsumeAction::Wait
            }
            ConsumeState::Using => {
                if observation.held_count < self.count_at_start {
                    self.state = ConsumeState::Consumed;
                    self.since = observation.now;
                    return ConsumeAction::RestoreWeapon;
                }
                if elapsed >= USE_TIMEOUT {
                    return self.fail(ConsumeFailure::Interrupted, observation.now, limits);
                }
                ConsumeAction::Wait
            }
            ConsumeState::Consumed => ConsumeAction::RestoreWeapon,
            ConsumeState::RestoringWeapon => {
                if observation.weapon_restored {
                    self.finish(observation.now);
                    return ConsumeAction::Finished;
                }
                if elapsed >= RESTORE_TIMEOUT {
                    // The weapon didn't come back, but the bite *did* land
                    // and the hand must not stay locked: report it and let
                    // the ordinary per-tick weapon selection sort it out.
                    self.finish(observation.now);
                    return ConsumeAction::Failed {
                        reason: ConsumeFailure::RestoreStalled,
                        retry_in: None,
                    };
                }
                ConsumeAction::Wait
            }
        }
    }

    /// Abandons the current attempt, scheduling a retry if the budget
    /// allows.
    pub fn fail(
        &mut self,
        reason: ConsumeFailure,
        now: Instant,
        limits: &ConsumeLimits,
    ) -> ConsumeAction {
        let exhausted = self.attempts >= limits.retry_limit;
        self.state = ConsumeState::Idle;
        self.item_id = None;
        self.since = now;
        self.retry_after = (!exhausted).then(|| now + limits.retry_delay);
        ConsumeAction::Failed {
            reason,
            retry_in: (!exhausted).then_some(limits.retry_delay),
        }
    }

    fn finish(&mut self, now: Instant) {
        self.state = ConsumeState::Idle;
        self.item_id = None;
        self.since = now;
        self.attempts = 0;
        self.retry_after = None;
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

    fn observation(millis: u64) -> ConsumeObservation {
        ConsumeObservation {
            acknowledged_slot: None,
            held_count: 3,
            weapon_restored: false,
            now: at(millis),
        }
    }

    fn limits() -> ConsumeLimits {
        ConsumeLimits::default()
    }

    /// Drives a clean, fully successful consume and returns the machine.
    fn consume_successfully() -> ConsumeMachine {
        let mut machine = ConsumeMachine::new(at(0));
        machine.begin("minecraft:golden_apple".into(), 7, 3, at(0));
        assert_eq!(
            machine.advance(observation(0), &limits()),
            ConsumeAction::SelectSlot { hotbar_index: 7 }
        );
        machine.slot_selected(at(50));

        let mut acked = observation(100);
        acked.acknowledged_slot = Some(7);
        assert_eq!(machine.advance(acked, &limits()), ConsumeAction::StartUse);
        machine.use_started(3, at(100));

        // Still chewing.
        assert_eq!(
            machine.advance(observation(800), &limits()),
            ConsumeAction::Wait
        );

        // The stack shrank.
        let mut eaten = observation(1700);
        eaten.held_count = 2;
        assert_eq!(
            machine.advance(eaten, &limits()),
            ConsumeAction::RestoreWeapon
        );
        machine.restore_started(at(1700));

        let mut restored = observation(1750);
        restored.held_count = 2;
        restored.weapon_restored = true;
        assert_eq!(
            machine.advance(restored, &limits()),
            ConsumeAction::Finished
        );
        machine
    }

    #[test]
    fn a_new_machine_is_idle_and_ready() {
        let machine = ConsumeMachine::new(at(0));
        assert_eq!(machine.state(), ConsumeState::Idle);
        assert!(machine.ready_to_start(at(0)));
        assert!(!machine.state().is_active());
        assert!(!machine.state().blocks_combat());
    }

    #[test]
    fn a_golden_apple_is_consumed_end_to_end() {
        let machine = consume_successfully();
        assert_eq!(machine.state(), ConsumeState::Idle);
        assert_eq!(machine.attempts(), 0, "a success clears the retry budget");
        assert!(machine.ready_to_start(at(1750)));
    }

    #[test]
    fn combat_is_blocked_for_the_whole_bite_and_released_for_the_restore() {
        let mut machine = ConsumeMachine::new(at(0));
        machine.begin("minecraft:golden_apple".into(), 7, 3, at(0));
        assert!(machine.state().blocks_combat(), "preparing");
        machine.slot_selected(at(50));
        assert!(machine.state().blocks_combat(), "waiting for ack");
        machine.use_started(3, at(100));
        assert!(machine.state().blocks_combat(), "using");

        let mut eaten = observation(1700);
        eaten.held_count = 2;
        machine.advance(eaten, &limits());
        assert!(machine.state().blocks_combat(), "consumed");

        machine.restore_started(at(1700));
        assert!(
            !machine.state().blocks_combat(),
            "the bite is over; restoring is itself a weapon swap"
        );
        assert!(
            machine.state().holds_hand(),
            "but the hand is still the consume's until it finishes"
        );
    }

    #[test]
    fn use_is_not_sent_until_the_server_acknowledges_the_slot() {
        let mut machine = ConsumeMachine::new(at(0));
        machine.begin("minecraft:golden_apple".into(), 7, 3, at(0));
        machine.slot_selected(at(0));

        // Nothing acknowledged yet.
        assert_eq!(
            machine.advance(observation(100), &limits()),
            ConsumeAction::Wait
        );
        // The *wrong* slot acknowledged is not good enough either.
        let mut wrong = observation(200);
        wrong.acknowledged_slot = Some(2);
        assert_eq!(machine.advance(wrong, &limits()), ConsumeAction::Wait);

        let mut right = observation(300);
        right.acknowledged_slot = Some(7);
        assert_eq!(machine.advance(right, &limits()), ConsumeAction::StartUse);
    }

    #[test]
    fn a_slow_acknowledgement_still_lands_within_the_timeout() {
        let mut machine = ConsumeMachine::new(at(0));
        machine.begin("minecraft:golden_apple".into(), 7, 3, at(0));
        machine.slot_selected(at(0));
        let mut late = observation(SLOT_ACK_TIMEOUT.as_millis() as u64 - 1);
        late.acknowledged_slot = Some(7);
        assert_eq!(machine.advance(late, &limits()), ConsumeAction::StartUse);
    }

    #[test]
    fn an_unacknowledged_slot_fails_and_schedules_a_retry() {
        let mut machine = ConsumeMachine::new(at(0));
        machine.begin("minecraft:golden_apple".into(), 7, 3, at(0));
        machine.slot_selected(at(0));
        let action = machine.advance(observation(SLOT_ACK_TIMEOUT.as_millis() as u64), &limits());
        assert_eq!(
            action,
            ConsumeAction::Failed {
                reason: ConsumeFailure::SlotNotAcknowledged,
                retry_in: Some(limits().retry_delay),
            }
        );
        assert_eq!(machine.state(), ConsumeState::Idle);
    }

    #[test]
    fn an_interrupted_bite_is_detected_and_retried() {
        // This is the failure every combat action causes: use-item was sent,
        // something cancelled it, and nothing is ever consumed.
        let mut machine = ConsumeMachine::new(at(0));
        machine.begin("minecraft:golden_apple".into(), 7, 3, at(0));
        machine.slot_selected(at(0));
        machine.use_started(3, at(0));

        assert_eq!(
            machine.advance(observation(1000), &limits()),
            ConsumeAction::Wait
        );
        let action = machine.advance(observation(USE_TIMEOUT.as_millis() as u64), &limits());
        assert_eq!(
            action,
            ConsumeAction::Failed {
                reason: ConsumeFailure::Interrupted,
                retry_in: Some(limits().retry_delay),
            }
        );
        assert_eq!(machine.state(), ConsumeState::Idle);
    }

    #[test]
    fn a_retry_waits_for_the_delay_rather_than_firing_every_tick() {
        let mut machine = ConsumeMachine::new(at(0));
        machine.begin("minecraft:golden_apple".into(), 7, 3, at(0));
        machine.slot_selected(at(0));
        machine.use_started(3, at(0));
        machine.advance(observation(USE_TIMEOUT.as_millis() as u64), &limits());

        let failed_at = USE_TIMEOUT.as_millis() as u64;
        assert!(!machine.ready_to_start(at(failed_at + 50)), "too soon");
        assert!(machine.ready_to_start(at(failed_at + 150)));
    }

    #[test]
    fn the_retry_budget_is_finite() {
        let mut machine = ConsumeMachine::new(at(0));
        let limits = ConsumeLimits {
            retry_limit: 3,
            retry_delay: Duration::from_millis(150),
        };
        let mut failures = 0;
        for attempt in 0..3 {
            let start = attempt * 3000;
            machine.begin("minecraft:golden_apple".into(), 7, 3, at(start));
            machine.slot_selected(at(start));
            machine.use_started(3, at(start));
            let action = machine.advance(observation(start + 2200), &limits);
            let ConsumeAction::Failed { retry_in, .. } = action else {
                panic!("expected a failure on attempt {attempt}");
            };
            failures += 1;
            if attempt < 2 {
                assert!(retry_in.is_some(), "attempt {attempt} should retry");
            } else {
                assert!(retry_in.is_none(), "the budget must run out");
            }
        }
        assert_eq!(failures, 3);
        assert!(
            machine.ready_to_start(at(999_999)),
            "exhausted means no more retries for this decision, not permanently stuck"
        );
    }

    #[test]
    fn a_success_resets_the_budget_for_the_next_time_the_bot_is_low() {
        let mut machine = consume_successfully();
        assert_eq!(machine.attempts(), 0);
        machine.begin("minecraft:golden_apple".into(), 7, 2, at(9000));
        assert_eq!(machine.attempts(), 1, "counting starts again from scratch");
    }

    #[test]
    fn a_stalled_weapon_restore_still_releases_the_hand() {
        let mut machine = ConsumeMachine::new(at(0));
        machine.begin("minecraft:golden_apple".into(), 7, 3, at(0));
        machine.slot_selected(at(0));
        machine.use_started(3, at(0));
        let mut eaten = observation(1700);
        eaten.held_count = 2;
        machine.advance(eaten, &limits());
        machine.restore_started(at(1700));

        let action = machine.advance(
            observation(1700 + RESTORE_TIMEOUT.as_millis() as u64),
            &limits(),
        );
        assert_eq!(
            action,
            ConsumeAction::Failed {
                reason: ConsumeFailure::RestoreStalled,
                retry_in: None,
            }
        );
        assert_eq!(
            machine.state(),
            ConsumeState::Idle,
            "the hand must never stay locked by a failed restore"
        );
    }

    #[test]
    fn withdrawing_the_decision_clears_everything() {
        let mut machine = ConsumeMachine::new(at(0));
        machine.begin("minecraft:golden_apple".into(), 7, 3, at(0));
        machine.slot_selected(at(0));
        machine.use_started(3, at(0));
        // The target dropped into finisher range: stop eating immediately.
        machine.reset(at(500));
        assert_eq!(machine.state(), ConsumeState::Idle);
        assert!(!machine.state().blocks_combat());
        assert!(
            machine.ready_to_start(at(500)),
            "no retry penalty for a withdrawal"
        );
        assert_eq!(machine.attempts(), 0);
    }

    #[test]
    fn no_state_can_persist_past_its_deadline() {
        // Every non-idle state either progresses or fails; none of them can
        // hold the hand forever, which is the property the old two-flag
        // version lacked.
        for setup in 0..3 {
            let mut machine = ConsumeMachine::new(at(0));
            machine.begin("minecraft:golden_apple".into(), 7, 3, at(0));
            if setup >= 1 {
                machine.slot_selected(at(0));
            }
            if setup >= 2 {
                machine.use_started(3, at(0));
            }
            // Far past every timeout in the module.
            let action = machine.advance(observation(60_000), &limits());
            if setup == 0 {
                // Preparing always emits its select immediately.
                assert_eq!(action, ConsumeAction::SelectSlot { hotbar_index: 7 });
            } else {
                assert!(
                    matches!(action, ConsumeAction::Failed { .. }),
                    "setup {setup} hung in {:?}",
                    machine.state()
                );
                assert_eq!(machine.state(), ConsumeState::Idle);
            }
        }
    }

    #[test]
    fn out_of_order_transitions_are_ignored_rather_than_corrupting_the_state() {
        let mut machine = ConsumeMachine::new(at(0));
        // None of these apply in Idle.
        machine.slot_selected(at(0));
        machine.use_started(3, at(0));
        machine.restore_started(at(0));
        assert_eq!(machine.state(), ConsumeState::Idle);
    }

    #[test]
    fn failures_describe_themselves_for_logging() {
        assert_eq!(ConsumeFailure::Interrupted.label(), "use interrupted");
        assert_eq!(
            ConsumeFailure::SlotNotAcknowledged.label(),
            "hotbar slot never acknowledged"
        );
    }
}
