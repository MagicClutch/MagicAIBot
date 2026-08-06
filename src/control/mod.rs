//! The global emergency-stop system: a hard interrupt for every other
//! subsystem in this crate (movement, pathfinding, combat, interaction,
//! look, container, survival, ...), deliberately kept independent of any of
//! them.
//!
//! [`emergency::EmergencyStop`] is the signal itself -- a re-armable
//! broadcast interrupt every blocking wait in `App` races against
//! (`App::wait_tick`), so firing it wakes every currently-stuck command
//! immediately, regardless of which subsystem it's stuck in or whether that
//! subsystem's own cancellation logic would ever notice on its own.
//! [`stop::execute`] is what actually runs once that signal fires: a
//! direct, synchronous reset of every controller, not a negotiated
//! shutdown.
//!
//! Neither half lives inside `movement`, `mobs::combat`, `interaction`,
//! `navigation`, or any other subsystem it resets -- see this module's own
//! existence for that.

pub mod emergency;
pub mod stop;

pub use emergency::EmergencyStop;
