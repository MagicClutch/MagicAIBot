# Architecture, ownership, and hardening audit

## Audit note (2026-07-26)

The requested baseline (“Tasks 1–21 complete”) is not present in this Git history. The repository has
implementations through the single-block interaction work, while `skills` and `ai` are placeholders.
Consequently, inventory transactions, crafting, eating, containers, gathering, and the Phase 2/3
end-to-end command paths cannot truthfully be hardened or accepted. Adding mock gameplay would hide
that gap, so this change hardens shared lifecycle primitives and documents the unsupported links.

| Issue / evidence | Risk | Smallest change | Verification |
|---|---|---|---|
| Controllers had no shared declarative lease order. | Future composed tasks can deadlock or overlap ownership. | `ResourceLeases` sorts/deduplicates movement → look → interaction → inventory → container and makes waits cancellable. | Ordering, cancellation, and drop-release unit tests. |
| Async work lacked a cross-session token/correlation ID. | A stale completion could mutate newer work; logs cannot be joined safely. | `OperationContext` validates session generation and carries a random safe correlation ID. | Stale-session and uniqueness test. |
| Menu closure could be inferred during terminal cleanup. | Status could claim a disconnected menu was closed. | `ActionCleanup` stops movement/mining and records unconfirmed closure as `Unknown`. | Terminal-path matrix test. |
| Block-search values are cloned into controllers. | Future caches would diverge if added to this value service. | No refactor today: it is immutable configuration only. Convert it to shared identity before adding mutable caches. | Source audit; no cache/state exists. |
| No inventory action service or raw menu clicks exist. | Adding clicks to tasks/controllers would violate ownership. | Preserve the boundary: future clicks must exist only in one inventory action service. | `rg` audit and supported-container table. |

No adapter or compatibility layer was proven dead, so none was removed. The old task façade remains in
use by `App`; replacing it without the missing Task Runtime baseline would be an unjustified rewrite.

## Dependency direction and owners

`App` is the composition/lifecycle root. Console parsing produces typed commands; `App` orchestrates
existing services. `MinecraftClient` alone translates Azalea ECS/protocol behavior and publishes
`WorldState`. Movement owns navigation input, `BlockNavigationService` owns target/approach retries,
`LookController` owns rotation, and `InteractionController` owns intentional break/place. Tasks must
only sequence those owners. Intentional break never enables pathfinder mining; mining is an explicit
`NavigationMode::AllowMining` choice.

Inventory is currently a read-only `WorldState` snapshot plus selected-hotbar integration. There is no
menu transaction engine. When introduced, exactly one inventory action service must own menu clicks,
cursor recovery, revision confirmation, and close confirmation; tasks may only call that service.

## Lifecycle contract

Composed work declares resources and receives them in this global order:

1. movement
2. look
3. interaction
4. inventory
5. container

Acquisition is cancellation-aware. A dropped lease releases every acquired resource, including a
partially built set. Each operation captures a connection-session generation; a different current
generation produces `StaleSession`, never success. Terminal cleanup stops movement and mining. A menu
is `Closed` only after confirmation; disconnect or an unobservable close is `Unknown`.

Existing runtime cleanup in `App` cancels interaction, block navigation, look, movement, and task
status on disconnect and shutdown. This is not yet a complete session-scoped task runtime: direct
commands do not all acquire the new leases, and Azalea listener generation is not exposed as a typed
readiness lifecycle.

## Test scope and acceptance

The mocked gathering matrix exercises four requested source labels (logs, stone, visible ore, food)
against tool-crafted, chest-deposit, inventory-pressure, changed-world, timeout, cancellation, death,
and disconnect events. It validates only common ownership cleanup; it is deliberately not described as
a production gathering end-to-end test.

**Phase 2 and Phase 3 acceptance criteria are not fully met.** Missing production capabilities include
central inventory/menu actions, crafting execution/knowledge, food/eating, containers/deposit, generic
gathering, and a complete task/session runtime. Until those are implemented and server-confirmed, the
corresponding tool/container tables and command help must continue to say unsupported.
