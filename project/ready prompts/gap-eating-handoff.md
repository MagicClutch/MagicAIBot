# Golden apple eating rework — state as of 2026-08-08

Everything below is **committed to the working tree, compiling, and green**:
`cargo test` 644 passed, `cargo clippy --all-targets` clean (one pre-existing
warning in `equipment/armor.rs`), `cargo build --release` succeeds.

Nothing is half-finished. It is safe to stop here.

---

## The bug that was fixed

The bot selected a golden apple, held it, entered the eating state, and
often never actually ate it.

**Root cause: the combat loop was not the thing cancelling the bite.**
`combat/executor.rs` already suppressed attacks, sprint, weapon swaps and
shield while eating — but two *other* subsystems don't go through it:

- `HotbarEquipmentService::tick`
- `EquipmentService::tick` (armor / offhand)

Both run every ~50ms and re-evaluate whenever the inventory **revision**
changes. Eating changes the revision. So a bite reliably woke them, they
fired a hotbar select or a container click, and the server cancelled the
item use. The `if !eating` checks in the combat tick could never see it.

---

## What was added

### 1. `src/combat/consume.rs` (new, pure, 15 tests)

The six-state machine the spec asked for:

```
Idle -> Preparing -> WaitingForSlotAck -> Using -> Consumed -> RestoringWeapon -> Idle
```

- Every non-Idle state carries a deadline (`SLOT_ACK_TIMEOUT` 500ms,
  `USE_TIMEOUT` 2200ms, `RESTORE_TIMEOUT` 600ms) — it cannot get stuck.
- `WaitingForSlotAck -> Using` only when
  `acknowledged_hotbar_slot() == desired_slot`.
- Retries with budget + delay; a success clears the budget.
- `blocks_combat()` vs `holds_hand()` are deliberately different: during
  `RestoringWeapon` the bite is over (sprint/shield free) but the hand is
  still the consume's (no attacking/weapon swap).

### 2. The consume guard (`src/minecraft/client.rs`)

`begin_consume_guard()` / `end_consume_guard()` / `consume_guard_active()`.
While raised, these return `AppError::InventoryBusy`:

- `select_hotbar_slot`
- `select_item_in_hotbar`
- `container_click`  ← this is the one that was letting the services in

Every caller already treats `InventoryBusy` as "try again next tick".
The consume path uses bypass variants: `select_hotbar_slot_during_consume`,
`container_click_during_consume`, `manager::swap_into_slot_during_consume`.

The guard is **released the moment the bite lands** (on `RestoreWeapon`),
because the restore itself goes through the guarded path and would
otherwise refuse itself and stall.

### 3. Retreat while eating

`steering::retreat_band()` reuses the existing steering by turning fight
distance into crowding distance — the bot backs off and keeps circling,
camera still locked on the target. Wired via `MovementSnapshot.retreat`.

### 4. Config (`[killbot]`, in both config.toml and config.toml.example)

```toml
eat_retry_delay_ms = 150
eat_retry_limit = 3
allow_retreat_while_eating = true
```

---

## Two corrections to the spec, already applied

1. **"sprint away while eating" is impossible in vanilla** — sprinting
   cancels item use. The retreat therefore *walks*. There is a test
   asserting the bot never sprints while retreating.
2. **Retreating contradicts the earlier "no run away, just fight" rule.**
   It is scoped to the bite only and gated behind
   `allow_retreat_while_eating` (set `false` to restore pure aggression).

---

## What to do tomorrow

### First: verify on a live server (the only thing not done)

The spec's manual list, in order:

1. Full health, start `#kill <player>`.
2. Take damage to ~7 hearts (below `heal_threshold = 8.0`).
3. Watch for this exact log sequence:
   ```
   [INFO] Health low: eating mid-fight
   [INFO] Preparing golden_apple
   [INFO] Waiting for hotbar acknowledgement
   [INFO] Started eating golden_apple
   [SUCCESS] golden_apple consumed
   [INFO] Restoring weapon
   [INFO] Re-engaging target
   ```
4. Confirm absorption + regeneration actually applied.
5. Confirm the sword is back and attacks resume.
6. Repeat while sprinting, strafing, being hit, and mid-combo.

**If it still fails,** the log tells you which state it died in:
- stuck after "Preparing" → the swap/select is failing; check
  `swap_into_slot_during_consume`.
- "Waiting for hotbar acknowledgement" then a retry → the server never
  acked the slot; raise `SLOT_ACK_TIMEOUT` in `consume.rs`.
- "Started eating" then `[WARNING] ... use interrupted` → something is
  *still* cancelling it. Add whatever it is to the consume guard in
  `client.rs` (that is the single place to extend).

### Then, if time allows

- **Suspect remaining interrupter:** `crate::survival`'s water-bucket MLG
  calls `select_hotbar_slot` (survival/mod.rs:659,683). It is deliberately
  exempt (emergency), but if a false-positive fall detection fires during a
  bite it will cancel it. Consider routing it through the guard with an
  explicit override.
- **Status output goes to the console, not chat.** `#inventory` and friends
  use `println!`; only `logging::*` reaches chat. Routing the status
  printers through `logging` would make in-game queries actually answerable
  in-game (I flagged this when adding concurrent status requests).
- **`#goto` typed during a task still queues and runs afterward** rather
  than being rejected with "busy". Change if that surprises you in practice.

### Known-good baseline

If anything regresses, `git diff` against the last commit shows only:
`src/combat/consume.rs` (new), `src/combat/executor.rs`,
`src/minecraft/client.rs`, `src/equipment/manager.rs`,
`src/combat/movement/{steering,controller}.rs`, `src/config.rs`,
`config.toml`, `config.toml.example`.

### Build commands

```
cargo fmt
cargo clippy --all-targets
cargo test
cargo build --release
```

Note: `cargo build --release` needs the `[profile.release.package.tokio]
opt-level = 1` workaround in `Cargo.toml` — that is for a rustc ICE in the
pinned nightly compiling tokio, unrelated to this project. Remove it when
the toolchain updates.
