//! Shared action-failure/identity vocabulary used directly by app-level
//! services (currently `InteractionController`). There is no task
//! orchestration layer here -- the application calls `MovementService`,
//! `BlockNavigationService`, `LookController`, and `InteractionController`
//! directly and waits for each to reach a terminal state itself (see the
//! `await_*_terminal` pollers and `*_and_wait` helpers in `src/app.rs`).

pub mod lifecycle;
pub use lifecycle::{ActionFailure, Invalidation};
