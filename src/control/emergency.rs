//! The emergency-stop signal: a dedicated, re-armable broadcast interrupt,
//! independent of any single controller's own `cancel()` logic.
//!
//! Built on `tokio_util::sync::CancellationToken` (the same primitive
//! `App::shutdown` already uses for graceful process shutdown) but wrapped
//! so it can fire more than once: a plain `CancellationToken` is one-shot --
//! once cancelled it stays cancelled forever, which would make a *second*
//! `/stop` a no-op (every future `.cancelled()` wait would resolve
//! instantly without the caller ever having been blocked on anything, and
//! there would be no way to tell "an old stop" apart from "a new one").
//! [`EmergencyStop::trigger`] cancels the current token to wake every
//! current waiter, then atomically swaps in a fresh one so the *next* stop
//! can wake a *new* set of waiters the same way.

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct EmergencyStop {
    inner: Arc<Mutex<CancellationToken>>,
}

impl Default for EmergencyStop {
    fn default() -> Self {
        Self::new()
    }
}

impl EmergencyStop {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(CancellationToken::new())),
        }
    }

    /// A token bound to the current stop "generation". `.cancelled()` on
    /// this resolves the instant [`Self::trigger`] is next called -- race
    /// it inside a `tokio::select!` anywhere a wait must be hard-interruptible.
    /// Fetch a fresh one (call this again) after each wait completes;
    /// holding on to one across a stop and reusing it will not see a
    /// *later* trigger, since that one already fired and was replaced.
    pub fn token(&self) -> CancellationToken {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Fires the signal, instantly waking every task currently waiting on
    /// a token from [`Self::token`], then re-arms a fresh token so a later
    /// stop can fire again. Synchronous and infallible -- never blocks,
    /// never awaits, safe to call from any context (console thread, a chat
    /// event handler, anywhere `EmergencyStop` has been cloned to).
    pub fn trigger(&self) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.cancel();
        *guard = CancellationToken::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_signal_has_not_fired() {
        let stop = EmergencyStop::new();
        assert!(!stop.token().is_cancelled());
    }

    #[test]
    fn trigger_cancels_every_previously_fetched_token() {
        let stop = EmergencyStop::new();
        let before_a = stop.token();
        let before_b = stop.token();
        assert!(!before_a.is_cancelled());
        stop.trigger();
        assert!(before_a.is_cancelled());
        assert!(before_b.is_cancelled());
    }

    #[test]
    fn trigger_re_arms_so_a_later_stop_still_wakes_new_waiters() {
        let stop = EmergencyStop::new();
        stop.trigger();
        let after = stop.token();
        assert!(
            !after.is_cancelled(),
            "a stale trigger must not leak into the next generation"
        );
        stop.trigger();
        assert!(after.is_cancelled());
    }

    #[test]
    fn clones_share_the_same_signal() {
        let stop = EmergencyStop::new();
        let clone = stop.clone();
        let token = clone.token();
        stop.trigger();
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn a_waiter_blocked_on_the_token_wakes_immediately_on_trigger() {
        let stop = EmergencyStop::new();
        let token = stop.token();
        let waiter = tokio::spawn(async move {
            token.cancelled().await;
        });
        // Give the spawned task a chance to actually start waiting before
        // triggering, so this genuinely exercises "wakes a blocked waiter"
        // rather than "the token was already cancelled before anyone awaited it".
        tokio::task::yield_now().await;
        stop.trigger();
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("waiter must wake within the timeout, not hang forever")
            .expect("waiter task must not panic");
    }

    /// Every blocking wait loop in `App` (movement, pathfinding/block
    /// navigation, combat, interaction, look) shares the exact same
    /// `App::wait_tick` choke point, which races this token -- so a stuck
    /// subsystem is, structurally, always "some task awaiting
    /// `token.cancelled()`". This simulates several such subsystems frozen
    /// at once (a stuck gather loop, a stuck pathfind, a stuck combat
    /// chase, a stuck break/place, a stuck look, ...) and proves a single
    /// `trigger()` wakes all of them together, not just whichever one
    /// happened to be listening first.
    #[tokio::test]
    async fn one_trigger_wakes_every_frozen_subsystem_simultaneously() {
        let stop = EmergencyStop::new();
        let subsystems = ["gather", "mine", "pathfind", "combat", "interact", "look"];
        let mut waiters = Vec::new();
        for name in subsystems {
            let token = stop.token();
            waiters.push(tokio::spawn(async move {
                token.cancelled().await;
                name
            }));
        }
        tokio::task::yield_now().await;
        stop.trigger();
        for (waiter, expected_name) in waiters.into_iter().zip(subsystems) {
            let name = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
                .await
                .unwrap_or_else(|_| panic!("{expected_name} must not stay frozen after a stop"))
                .expect("waiter task must not panic");
            assert_eq!(name, expected_name);
        }
    }
}
