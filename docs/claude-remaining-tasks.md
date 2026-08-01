# Baritone-style pathfinding engine — remaining work

All 7 phases are now done (Phases 0-3 were done in an earlier session; Phases
4-7 were completed in this session, resuming from a mid-Phase-4 stop). This
file remains as a record of what was done and the design decisions locked in
along the way. The original approved plan is still at
`C:\Users\elias\.claude\plans\frolicking-toasting-nebula.md` and has the full
architecture rationale/research citations — read that first for *why*, this
file is the *what happened*. The only genuinely open items are the release
build ICE and live-server manual validation — both disclosed in the Phase 7 /
final report section below, and both outside what this session could resolve
(a toolchain issue and a lack of a reachable Minecraft server, respectively).

## Post-Phase-7 bug fixes (found via real usage, same session)

1. **Pillaring/bridging/staircasing stopped after placing exactly 1 block.**
   Root cause: `PathfindingPolicy::refresh_scaffold`
   (`vendor/azalea/azalea/src/pathfinder/policy.rs`) re-applied the
   `minimum_held` threshold (config default 4) as a *live* gate on every
   refresh, and `refresh_pathfinding_policy` runs on every single game tick
   via `check_for_path_obstruction` (not just at goal submission). The moment
   one placement dropped the held count below `minimum_held`, `scaffold_item`
   flipped to `None`, disabling every placement-based move for the rest of the
   route — and since the obstruction check re-verifies the already-planned
   path's edges every tick, this made the remaining pillar/bridge/staircase
   edges look "obstructed" (they could no longer be regenerated), triggering a
   patch that also couldn't succeed (same disabled policy), stranding the bot.
   **Fix**: `refresh_scaffold` now keeps using the already-selected scaffold
   item as long as it's still held at all (`count > 0`) and still
   allowed/not-denied; `minimum_held` only gates *selecting a new* item (i.e.
   starting a route), not continuing an already-started one. Added 3 unit
   tests in `policy.rs` plus an end-to-end simulation regression test,
   `test_pillar_continues_after_dropping_below_minimum_held` (starts with
   exactly `minimum_held` cobblestone climbing 4 blocks) — confirmed this
   test fails on the old code (bot stops at y=72, 1 block up) and passes with
   the fix. Vendor suite is now 42/42.
2. **Bot stopped responding to ALL commands after the first task, and
   "resumed the old task" after being killed and reconnecting.** Root cause:
   `await_movement_terminal` (`src/tasks/mod.rs`, used by `/goto`/`/goto-mine`
   via `TaskService::tracked`) had no timeout — it loops until
   `MovementStatus` reaches `Completed`/`Failed`/`Cancelled`. Azalea's
   pathfinder is submitted with `retry_on_no_path(true)`
   (`MinecraftClient::start_navigation_to`), so a genuinely unreachable goal
   (not enough scaffold material -- exactly what fix #1 above was about --
   or a destination behind terrain the policy can't cross) retries forever
   *inside Azalea* without ever surfacing as an explicit failure;
   `MovementStatus` just stays `MovingToPosition` indefinitely. Since
   `await_movement_terminal` blocks `TaskService::tracked`, which blocks the
   single `tokio::select!` loop that also handles every console command
   (`App::run`), this froze the *entire app* — not just movement — until the
   process was killed. On kill+reconnect, the old goal was never cleared
   (the in-flight `/goto` never finished, `current_task` was never cleared,
   `Pathfinder.goal` was never reset), so it looked like the bot "went back
   to the old task."
   **Fix**: added `MovementConfig::maximum_navigation_seconds` (default 120,
   same default/bounds as the pre-existing `BlockNavigationConfig` field of
   the same name, which already had this exact protection —
   `BlockNavigationService::tick` calls `timed_out(...)` internally, so
   `/goto-block` was never vulnerable to this). `await_movement_terminal` now
   tracks elapsed time against this deadline and, if exceeded, calls
   `movement.stop()` (which sends Azalea's `force_stop_pathfinding`, clearing
   `Pathfinder.goal` so the retry loop actually stops, not just pauses) and
   returns `Err(AppError::PathfindingFailure(...))`, letting `tracked()`
   complete normally and the console loop become responsive again. Added
   `MovementConfig::maximum_navigation_seconds` to `config.toml` (120s) with
   an explanatory comment. **Not independently unit-tested** — reproducing
   "stuck forever" requires a live pathfinder retry loop, which needs a real
   Minecraft connection; verified instead by full compile + the existing 354
   app tests (including `goto_position_fails_promptly_when_disconnected`,
   which still passes and exercises the disconnected fast-fail path, a
   different branch from the new timeout).
   Also found (pre-existing, unrelated, not touched): `movement_service.rs`
   has dead code, `PATHFINDER_STARTUP_GRACE`/`pathfinder_startup_grace_elapsed`
   (flagged by clippy as unused) — looks like an abandoned earlier attempt at
   *minimum*-grace-period detection (don't call a freshly-submitted goto
   "stuck" within its first 2 seconds), a different concern from this fix's
   *maximum*-wait deadline. Worth revisiting if further movement-reliability
   work happens, but out of scope here.

   Also noticed while validating: the vendor `pathfinder::` test suite is
   occasionally flaky when run in parallel (`cargo test -p azalea --lib
   pathfinder::`) — roughly 1 in 6-7 full runs on this machine fails a
   single simulation test (seen: `test_follow_style_goal_change_reroutes_through_build_moves`),
   but every failure passes when re-run in isolation or when the whole suite
   is run with `-- --test-threads=1` (42/42 reliably). Root cause is
   `wait_until_bot_starts_moving`'s use of a real wall-clock 5-second
   `Instant`/`Duration` timeout (`vendor/azalea/azalea/src/pathfinder/tests.rs`)
   racing against CPU contention from many parallel Bevy `App`s under test —
   pre-existing test-harness behavior, not a logic regression introduced this
   session (confirmed: the same code passes deterministically single-threaded).
   Not fixed — would need the harness's timing to be simulated/tick-based
   rather than wall-clock, which is a larger change than this session's scope.
   If CI or a future session sees intermittent vendor-test failures, rerun
   with `--test-threads=1` before assuming a real regression.

3. **`/stop` always printed "Movement stopped" even when nothing was
   running.** `src/app.rs`'s `ConsoleCommand::Stop` handler now reads
   `active_stop_description()` (checks `MovementService::snapshot()` for
   `FollowingPlayer`/`MovingToPosition`, then `BlockNavigationService::snapshot()`
   for an active search/move state) *before* cancelling, and prints `"Bot
   stopped (<description>)"` (e.g. "following Steve", "moving to position",
   "navigating to minecraft:oak_log") when something was actually active, or
   `"Bot has no task to stop"` otherwise. Note: `/goto`/`/goto-mine`/
   `/goto-block` run through `TaskService::tracked`, which blocks the console
   loop until they finish (single-threaded `tokio::select!` loop in
   `App::run`), so a `/stop` typed during one of those is queued and only
   processed after the goto naturally resolves — `/stop` can only actually
   catch something "in the act" for `/follow` (fire-and-forget, ticked
   separately) or an in-progress `/goto-block` search. This blocking behavior
   of `/goto` itself is a pre-existing architectural characteristic, not
   something changed or fixed here — flagging it since it's the reason
   `/stop` can't interrupt an in-flight `/goto`, only report on it afterward.

## Status summary

| Phase | Status |
|---|---|
| 0 — Shared policy in vendor (tool_policy, vertical.rs scaffold policy) | **Done** |
| 1 — Placement execution primitive (ExecuteCtx/IsReachedCtx, StartUseItemEvent, ordering fix, Placing timeout carve-out) | **Done** |
| 2 — New movement primitives (`moves/build.rs`: pillar-up, bridge, staircase-up) | **Done** |
| 3 — Policy plumbing (`PathfindingPolicy` via `CustomPathfinderStateRef`, refreshed at all 4 replan sites) | **Done** |
| 4 — App-side integration | **Done** |
| 5 — Logging | **Done** |
| 6 — Tests (new Simulation-harness tests for the new moves) | **Done** |
| 7 — Validation (fmt/check/test/clippy/build --release) + final report | **Done** (release build blocked by unrelated toolchain issue, see below) |

Everything compiles and passes `fmt`/`check`/`test`/`clippy` in both crates.
Vendor crate: 38/38 `pathfinder::` tests pass (30 pre-existing + 8 new).
App crate: 354/354 tests pass. `cargo build --release` is blocked by a
pre-existing, unrelated nightly-toolchain ICE compiling `tokio` itself — see
Phase 7 below.

## What changed so far (all in the vendored `vendor/azalea` checkout unless noted)

- `vendor/azalea/azalea/src/pathfinder/tool_policy.rs` (new) — canonical tool-selection
  algorithm (ported from `src/interaction/tool_selection.rs`, which is now a thin
  `pub(crate) use azalea::pathfinder::tool_policy::{...}` re-export). Also has
  `candidates_for_block`/`find_hotbar_item` sync helpers.
- `vendor/azalea/azalea/src/pathfinder/vertical.rs` — extended with `ScaffoldPolicy`,
  `select_scaffold_block`, `dominant_step_toward` (ported from `src/navigation/vertical.rs`,
  which still exists unchanged — **not yet deleted**, see below).
- `vendor/azalea/azalea/src/pathfinder/policy.rs` (new) — `PathfindingPolicy` struct
  (allow_pillaring/allow_bridging/allow_staircase_building/scaffold policy/live
  scaffold item+count) + `refresh_pathfinding_policy()`, wired into `CustomPathfinderStateRef`.
  Refreshed at all 4 replan call sites: `pathfinder/mod.rs::goto_listener`,
  `pathfinder/mod.rs::path_found_listener`, `pathfinder/execute/patching.rs::check_for_path_obstruction`,
  `pathfinder/execute/mod.rs::patch_path_from_timeout`.
- `vendor/azalea/azalea/src/pathfinder/moves/mod.rs` — `ExecuteCtx` gained
  `can_place: bool`, `place_item_events: MessageWriter<StartUseItemEvent>`,
  `custom_state: Arc<RwLock<CustomPathfinderStateRef>>`, and methods `place()`,
  `clear_placing()`, `custom::<T>()`. `IsReachedCtx` gained `world: Arc<RwLock<World>>`.
  New `Placing` marker component. New `pub fn combined_move` = `default_move` + `build::build_move`.
- `vendor/azalea/azalea/src/pathfinder/moves/build.rs` (new) — `pillar_up_move`,
  `bridge_move`, `staircase_up_move` + their execute/is_reached functions. Each only
  fires where the existing walk/mine moves can't already reach (checked via
  `cost_for_standing`/`cost_for_breaking_block` guards), so they never compete with
  or duplicate `default_move`'s edges. **Design note**: staircase-up over fully open
  air (no solid reference anywhere) is deliberately NOT handled as its own move —
  it's expected to emerge from A* composing `pillar_up_move` with ordinary walk/mine
  moves instead, since a single diagonal placement move would need an unverified
  placement technique. Dig-down/staircase-down needed no new move: the existing
  `moves/basic.rs::downward_move` already refuses to mine unless the landing block
  two below is already solid, which is already maximally safe.
- `vendor/azalea/azalea-client/src/plugins/interact/mod.rs` — `StartUseItemEvent`/
  `StartUseItemQueued` gained `force_direction: Option<Direction>` (defaults to `Up`,
  preserving old behavior) so placement can click a *side* face, not just top —
  required for bridging (you place against the side of the block you're standing on,
  not the top of something that may not exist below a gap).
- `vendor/azalea/azalea-client/src/plugins/inventory/mod.rs` — fixed a latent system-
  ordering ambiguity: `ensure_has_sent_carried_item` now also runs
  `.after(interact::handle_start_use_item_queued)` (previously only ordered after
  mining's equivalent), so a same-tick hotbar-switch-then-place can't race.
- `src/interaction/tool_selection.rs` — rewritten as a thin re-export of
  `azalea::pathfinder::tool_policy` (see above).
- `src/app.rs` — added SIGTERM handling (`wait_for_terminate_signal`, Unix-only,
  no-op on Windows) alongside the existing Ctrl+C handler in `App::run`'s select
  loop, so Docker/Pelican Panel's `docker stop` (SIGTERM) triggers the same graceful
  shutdown path. **This part is done and unrelated to the pathfinding work** — it
  was a mid-turn user request ("make sure i can use the bot in pelican panel").
  Verified: config/auth-cache paths are already relative (Pelican-friendly), no
  Windows-only APIs found elsewhere.
- `src/config.rs` — `VerticalNavigationConfig` gained `allow_bridging: bool`
  (default `true`), doc comment updated to describe it as pathfinder-engine policy
  rather than command-layer terrain-assist policy. **Struct name kept as-is**
  (not renamed to `PathfindingConfig`) to minimize diff — it already had the right
  shape.

## Design decisions locked in (don't re-litigate these)

1. **`NavigationMode` (`src/movement/mod.rs`) is being KEPT, not removed.** It's used
   in 10+ files (`food/mod.rs`, `tasks/mod.rs`, `navigation/block_navigation.rs`,
   `navigation/navigation_state.rs`, `movement/movement_service.rs`, `app.rs`).
   Removing it was in scope per the plan but only "if nothing else needs it" — it's
   too widely used to justify the removal risk. Instead, **its meaning is being
   expanded**: `NavigationMode::AllowMining` now means "mining AND building both
   enabled, cost-based" (previously just mining). `MovementOnly` still means
   walk/jump only. This is a strict capability upgrade for every existing
   `AllowMining` call site (gathering, mining tasks, block navigation, etc.) — no
   call site needs to change *which* mode it passes, they just automatically gain
   build capability. This matches the user's requirement that gathering/mining/
   exploration tasks inherit the new capabilities without command-layer changes.
   `food/mod.rs`'s `MovementOnly` uses were deliberately left alone (narrower,
   already-intentional use case, out of scope to change).

2. **`/goto` and `/goto-mine` become identical** — both should submit with
   `NavigationMode::AllowMining` once app.rs is updated (not done yet, see below).
   Since it's a real cost-based A* now, defaulting `/goto` to full capability is
   strictly better than the old behavior (cheap walk edges still win when
   available; mining/building are only used when actually cheaper than failing).
   `GotoMine` command parsing should be kept (harmless alias) rather than removed,
   for backward compatibility.

3. **`/follow` should also default to `AllowMining`** (currently hardcoded to
   `MovementOnly` in `movement_service.rs::follow()` and its internal
   `refresh_navigation_goal` call in `tick_follow`) — not yet changed, see task list
   below.

4. **Mining tool selection continues to use Azalea's own `best_tool_in_hotbar_for_block`**
   (unchanged), NOT routed through the app's richer protected/reserved/durability
   policy. Forking `MiningCache` to inject that policy was flagged by the
   feasibility review as "genuinely new code, not a relocation, moderate blast
   radius" — deliberately scoped out given the size of everything else already
   done. The new `tool_policy.rs`/`ScaffoldPolicy` machinery IS used for scaffold
   *placement* item selection (the genuinely new capability), just not mining.
   **Disclose this as a known limitation in the final report.**

5. **Scaffold/tool selection only searches the hotbar**, not the full inventory —
   matches the app's existing `select_item_in_hotbar`/`select_tool_for_block`
   behavior exactly (not a new limitation introduced by this work).

6. **Multi-block scaffold budgeting across a route is not tracked.** `PathfindingPolicy`
   only gates "is placement possible at all" (item + count > 0), not "will we run
   out N blocks into this specific route." Matches Baritone's own real-world
   imperfection here; disclose as a known limitation.

## Remaining tasks, in order

### Phase 4 — App-side integration — DONE

All steps below were completed and verified (`cargo check --workspace --all-targets`
clean; `cargo test --workspace` 354 passed; vendor crate's 30 `pathfinder::` tests
still pass).

1. **`src/minecraft/client.rs`**: added `vertical_navigation: VerticalNavigationConfig`
   field + 5th constructor parameter (all 4 call sites updated: `src/app.rs`,
   `src/crafting/tests.rs`, `src/tasks/mod.rs`, `client.rs`'s own test helper).
   `start_navigation_to` now builds a `PathfindingPolicy` from
   `self.vertical_navigation` and `mode.allows_mining()` (one `let allow_build =
   self.vertical_navigation.enabled && mode.allows_mining();` gate applied to all
   three `allow_*` flags — a minor simplification over the originally sketched
   per-flag `&&`, same result), inserts it into a fresh `CustomPathfinderState`
   component on the bot entity via `client.ecs.write().entity_mut(client.entity)`,
   and adds `.successors_fn(combined_move)` to `PathfinderOpts`. `scaffold_item`/
   `scaffold_available` are left at `Default` upfront, populated by
   `refresh_pathfinding_policy` on the first replan tick as designed.

2. **`src/movement/movement_service.rs`**: `follow()` and `tick_follow`'s
   `refresh_navigation_goal` call now pass `NavigationMode::AllowMining` instead of
   `MovementOnly`, so `/follow` gets build capability (design decision #3).

3. **`src/app.rs`**: removed the `vertical_construction: VerticalConstructionService`
   field, its construction, all `cancel()`/`assist_toward(...)` call sites, the
   status print block, and the `goto_with_terrain_assist`/`close_initial_gap_to_player`
   methods entirely. `ConsoleCommand::Goto{x,y,z}` now calls
   `self.tasks.goto_position(..., NavigationMode::AllowMining)` directly.
   `ConsoleCommand::Follow{player}` no longer calls the deleted gap-closing method.
   `GotoMine`/`GotoBlock` were untouched (already used `AllowMining` directly).

4. **Deleted** `src/navigation/vertical_construction.rs` and
   `src/navigation/vertical.rs` entirely; removed their declarations/re-export from
   `src/navigation/mod.rs`. No other file referenced them (`grep` confirmed before
   deleting).

No unexpected `cargo check` fallout beyond what the plan anticipated — the `App`
struct field removal was the only structural break, and it was self-contained to
`app.rs`.

### Phase 5 — Logging — DONE

Investigated first: `src/logging.rs` is `println!`-based and does **not**
subscribe to `tracing` — it's a fully separate pipeline from
`crate::config::init_logging` (`src/config.rs:1609`), which sets up a
`tracing_subscriber::registry()` with a `Targets` filter defaulting everything to
`OFF` except the app crate's own target, plus `azalea`/`azalea_client`/etc at
`DEBUG` **only when `logging.debug = true`** in config. Decision: kept
pathfinder-engine logs `tracing`-only rather than bridging them into the app's
chat/console stream — this matches the existing precedent (patching.rs's
`warn!("path obstructed...")` and the rest of the vendored pathfinder's `info!`/
`debug!`/`warn!` calls were never bridged either), and bridging would require a
new cross-crate channel for no clear benefit. **Known limitation**: with
`logging.debug = false` (the default), none of this is visible — set
`logging.debug = true` to see pathfinder-engine activity via the console.

Audited the 6 requested state-transition events against what already existed in
`vendor/azalea/azalea/src/pathfinder/`:
- **path planned** — already logged (`info!("got goto {:?}, starting from
  {start:?}", ...)` and `info!("Pathfinder took {duration:?}")` in
  `pathfinder/mod.rs`).
- **destination reached** — already logged (`info!("goal was reached!")` in
  `execute/mod.rs::check_node_reached`).
- **replanning path** — already logged (`debug!("Recalculating path because...")`
  in `recalculate_near_end_of_path`/`recalculate_if_has_goal_but_no_path`,
  `warn!("pathfinder timeout, trying to patch path")` and
  `warn!("pathfinder went too far from path...")` in `timeout_movement`, all in
  `execute/mod.rs`).
- **mining obstruction** — already logged (`warn!("path obstructed at index
  {obstructed_index}...")` in `execute/patching.rs::check_for_path_obstruction`;
  this is actually generic obstruction detection, not mining-specific, so it now
  also covers a build-move obstruction, e.g. a placed scaffold block
  disappearing).
- **bridging gap** / **pillaring upward** — these were genuinely missing (the new
  moves in `moves/build.rs` had no logging at all) — **added**.

Implementation (`vendor/azalea/azalea/src/pathfinder/moves/build.rs`): each of
`execute_pillar_up_move`/`execute_bridge_move` is re-invoked every `GameTick`
while its edge is the front of the path, so a naive `info!` at the top would spam
once per tick. Added two small dedup marker types
(`LoggedPillarTarget`/`LoggedBridgeTarget`, each `Option<BlockPos>`) stored in
`ExecuteCtx::custom_state` (the same `CustomPathfinderStateRef` already used for
`PathfindingPolicy`) and two `log_pillar_started`/`log_bridge_started` helpers
that log once per distinct target and no-op on repeat calls for the same target.
Both use `custom_state.try_write()` (not `write()`) for the same reason
`refresh_pathfinding_policy` does — a read lock on that `RwLock` may be held for
the duration of an in-flight background A* search (up to several seconds), and
this runs on the game-tick thread; a missed log for one tick is harmless since
the next tick retries, but a blocked game tick is not. Staircase-up was
deliberately left unlogged — it wasn't one of the 6 requested events, and adding
it would be scope creep beyond what was asked (easy to add later with the same
pattern if wanted).

Verified: `cargo check -p azalea` (vendor crate) and `cargo check --workspace
--all-targets` (app crate) both clean; vendor's 30 `pathfinder::` tests still
pass unchanged (no test coverage added for the new logging itself — that's
inherently exercised by Phase 6's simulation tests once those exist, not
independently testable without one).

### Phase 6 — Tests — DONE

All 8 new tests added to `vendor/azalea/azalea/src/pathfinder/tests.rs`; full
`pathfinder::` suite is now 38 tests, 0 failed (30 pre-existing + 8 new).

Confirmed the research finding: placement has no client-side prediction, so a
naive test would see the bot never make progress past the first
pillar/bridge/staircase step. Added a test-only fake-ack system,
`install_fake_placement_ack`, that watches `StartUseItemEvent` (the same event
`ExecuteCtx::place` writes) and calls `world.chunks.set_block_state(...)`
directly at the position `force_block.offset_with_direction(force_direction)`
resolves to — added via `simulation.app.add_systems(GameTick, ...).after(PathfinderSystems)`
in the test file itself (not in `simulation.rs` — kept fully test-local, no
production code touched), ordered after `PathfinderSystems` so it observes the
event on the same tick it's written rather than needing to wait for it to
survive an event-buffer swap into the next tick. Mining needed no equivalent
because `MiningPlugin`'s real `handle_finish_mining_block_observer` already
fakes completion client-side (genuine prediction, not test-only).

Also added `equip_for_building` (seeds a hotbar slot with 64 cobblestone via
`Inventory`/`Menu::slot_mut`, then inserts a `CustomPathfinderState` wrapping a
`PathfindingPolicy` with all three `allow_*` flags on) and
`setup_build_simulation` (wraps the existing `setup_simulation_world` +
the two helpers above + dispatches the `GotoEvent` with
`successors_fn: moves::combined_move`) as the shared harness for every new test.
`scaffold_item`/`scaffold_available` are deliberately left unset on the
inserted policy — `goto_listener` calls `refresh_pathfinding_policy` on goal
submission exactly like the real app flow, so the test exercises the same
inventory-driven scaffold-selection path production code does, not a shortcut.

New tests, one per item on the original checklist:
- `test_bridge_across_gap` — floor only at the start block, open air (down to
  the world bottom) everywhere else, forcing `bridge_move` as the only option.
- `test_pillar_up_to_tower` — 5 blocks straight up with nothing to jump onto.
- `test_staircase_up_ledge` — floor that's exactly one block short of the
  landing, so `ascend_move` can't reach it but `staircase_up_move` can.
- `test_combined_bridge_and_pillar_route` — bridge then pillar in the same
  route, exercising A* composing two different build moves together (this is
  also the mechanism the "known limitations" section says staircase-over-open-
  air relies on instead of being its own move).
- `test_build_move_path_completion_reports_goal_reached` — asserts
  `Pathfinder::goal` is cleared once a pillar-up route finishes, not just that
  the position matches.
- `test_build_move_path_cancellation_stops_pillaring` — removes
  `ExecutingPath` and clears the goal mid-climb, asserts the bot doesn't keep
  climbing toward the original target.
- `test_build_move_dynamic_replanning_around_new_obstruction` — mutates the
  live `ChunkStorage` mid-route (places a block directly in the bridge path,
  simulating another player/mob) and asserts `check_for_path_obstruction`
  patches around it rather than the bot getting stuck.
- `test_follow_style_goal_change_reroutes_through_build_moves` — submits a
  second `GotoEvent` mid-route to a goal directly above the first (mirroring
  `MovementService::refresh_navigation_goal`'s periodic resubmission during
  `/follow`), asserting the bot re-routes through `pillar_up_move` instead of
  stopping at the original ground-level goal.

Tick budgets needed tuning empirically (pillaring is slower per block than
walking/bridging — each step is place-then-wait-for-landing, not a single
tick): pillar-only tests need ~500 ticks for 5 blocks, the combined
bridge+pillar route needs ~800; bridging alone was fine at 200–300. If Phase 7
or future changes make timing-sensitive tests newly flaky, bump the tick
budget first before suspecting a logic regression.

Verified: `cargo test -p azalea --lib pathfinder::` — 38 passed, 0 failed.

### Phase 7 — Validation — DONE (except live-server manual testing)

Ran in order, fixing forward, across both the app crate and the vendor `azalea`
crate (separately, since `vendor/azalea` is a nested git repo, not a Cargo
workspace member — see known limitations):

1. **`cargo fmt --all`** — found pre-existing unformatted hunks in both crates
   (some from this session's new code, some from earlier Phase 0-3 work that
   had never been through fmt). Applied; `cargo fmt --all -- --check` is now
   clean in both crates. All reformatted files were already part of this
   feature's diff (confirmed via `git status` inside `vendor/azalea` before
   running) — no untouched upstream files were reformatted.
2. **`cargo check --workspace --all-targets`** (app) / **`cargo check -p
   azalea`** (vendor) — both clean, no errors, only pre-existing dead-code
   warnings in the app crate (unrelated to this feature — things like
   `src/food/mod.rs`'s never-constructed structs).
3. **`cargo test --workspace`** (app) — 354 passed, 0 failed.
   **`cargo test -p azalea --lib pathfinder::`** (vendor) — 38 passed, 0
   failed (30 pre-existing + 8 new from Phase 6).
4. **`cargo clippy --all-targets --all-features`** (app) / **`cargo clippy -p
   azalea --all-targets`** (vendor) — both exit 0. App crate has ~206
   pre-existing warnings, none in any file touched this session (verified by
   filtering clippy output to just `client.rs`/`app.rs`/`movement_service.rs`/
   `config.rs`/`navigation/mod.rs`/`tasks/mod.rs`/`crafting/tests.rs` — zero
   hits). Vendor crate: zero warnings in any pathfinder file touched by this
   feature (`build.rs`, `tests.rs`, `policy.rs`, `vertical.rs`, `tool_policy.rs`,
   `mod.rs`, `moves/mod.rs`, `execute/*`).
5. **`cargo build --release`** — **blocked**, not by this feature. It's a
   reproducible internal compiler error (ICE) while the pinned nightly
   (`rustc 1.99.0-nightly (da86f4d07 2026-07-24)`, from `rust-toolchain.toml`)
   codegens the **`tokio`** crate itself at `-C opt-level=3` — i.e. it fails
   compiling a third-party dependency, not `magic_ai_bot` or `azalea` code, and
   fails identically on a clean retry (not a flaky/transient ICE). `cargo
   check`/`cargo test` (dev profile, opt-level=0) both work fine, so this is
   specifically a release-optimization codegen bug in this nightly snapshot,
   pre-existing and unrelated to anything changed in this session. **Not
   fixed** — changing the pinned toolchain version is an environment decision
   for the user to make, not something to do unilaterally. Flagging for the
   user: either pin a different nightly in `rust-toolchain.toml`, or file/check
   upstream `rust-lang/rust` for this ICE (the failure dumped a repro file to
   `C:\Users\elias\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\tokio-1.53.1\rustc-ice-2026-08-01T18_16_34-7916.txt`).

**Manual validation scenarios** (`/goto` through a wall, across a ravine, onto
a tower, up a cliff, down a cave; `/follow` a climbing/descending player and
across changing terrain) — **not run**, no live/dev Minecraft server was
reachable in this session. Disclosed as unverified; the Phase 6 simulation
tests are the closest available substitute and all pass.

## Final report

**1. Files inspected/changed** — see "What changed so far" above for the
full list; summary: `vendor/azalea/azalea/src/pathfinder/{tool_policy,vertical,
policy,custom_state,moves/mod,moves/build,mod,execute/mod,execute/patching,
tests}.rs` (new/extended engine code + tests), `vendor/azalea/azalea-client/src/
plugins/{interact,inventory}/mod.rs` (placement side-face support, ordering
fix), `src/{interaction/tool_selection,config,minecraft/client,movement/
movement_service,app,navigation/mod,crafting/tests,tasks/mod}.rs` (app
integration), `src/navigation/{vertical,vertical_construction}.rs` (deleted,
superseded).

**2. Graph changes** — `PathfinderOpts::successors_fn` now defaults to
`combined_move` (`default_move` + `build_move`) app-side, so every `/goto`,
`/goto-mine`, `/follow`, `/goto-block`, and task-driven navigation call
considers walking/jumping/mining/placing together in one A* search, letting
cost naturally prefer cheap walk edges and only reach for build moves when
nothing cheaper exists.

**3. New primitives** — `pillar_up_move` (place-and-climb straight up),
`bridge_move` (place ahead to cross a gap, clicking the side face of the
current standing block), `staircase_up_move` (extend a one-block-short ledge
then step up). Staircase-over-open-air is not a dedicated move; it emerges
from A* composing `pillar_up_move` with existing walk/mine moves (see
`test_combined_bridge_and_pillar_route`).

**4. Executor changes** — `ExecuteCtx` gained `can_place`/`place_item_events`/
`custom_state`/`place()`/`clear_placing()`/`custom::<T>()`; `IsReachedCtx`
gained `world`; new `Placing` marker component with a dedicated timeout
carve-out (placement has no client-side prediction, confirmation can take
several ticks — mirrors the existing `Mining` carve-out).

**5. Mining/tool-selection integration** — unchanged; still Azalea's own
`best_tool_in_hotbar_for_block`, not routed through the app's richer
protected/reserved/durability policy (known limitation, see #4 in "Design
decisions locked in"). The new `tool_policy.rs`/scaffold-selection machinery
*is* used for placement item selection, the genuinely new capability.

**6. Bridge/pillar/staircase integration** — gated end-to-end by
`PathfindingPolicy` (`allow_pillaring`/`allow_bridging`/`allow_staircase_building`
+ `ScaffoldPolicy`), built from `VerticalNavigationConfig` in
`MinecraftClient::start_navigation_to` and inserted as a `CustomPathfinderState`
component before every goal submission; refreshed live from hotbar inventory
at all 4 replan call sites.

**7. Replanning changes** — none needed structurally; `check_for_path_obstruction`
already recomputes edges generically (now including build-move edges) and
`refresh_pathfinding_policy` is called at every existing replan site.

**8. Logging** — 4 of the 6 requested state-transition logs already existed in
the vendored engine (path planned, destination reached, replanning, obstruction);
added the 2 that didn't (bridging gap, pillaring upward), one-shot per edge via
`try_write`-guarded dedup markers in `custom_state`. Kept `tracing`-only
(visible when `logging.debug = true`), not bridged into the app's chat/console
`println!` pipeline — see Phase 5 above for the full rationale.

**9. Tests added** — 8 new vendor-crate simulation tests (bridging, pillaring,
staircasing, combined route, completion, cancellation, dynamic replanning,
follow-style goal change) plus the Phase 0/3 unit tests for the ported pure
functions. Vendor suite: 38/38 passing. App suite: 354/354 passing, unaffected.

**10. `NavigationMode` handling** — kept and expanded per design decision #1;
`AllowMining` now means mining+building, cost-gated, for every existing call
site with zero call-site changes needed. `/follow` was switched from
`MovementOnly` to `AllowMining` (design decision #3).

**11. Known limitations** — see the dedicated section below (accumulated
throughout); plus the newly-found release-build ICE (Phase 7, unrelated to
this feature) and the lack of live-server manual validation.

**12. Validation status** — fmt/check/test/clippy all clean across both
crates; release build blocked by an unrelated pre-existing toolchain ICE;
manual in-game scenarios unverified (no reachable server this session).

**13. Baritone-style autonomy, per command** — **yes** for `/goto` (now
`AllowMining` unconditionally, cost-based walk/mine/build), **yes** for
`/goto-mine` (identical to `/goto` now, kept as alias), **yes** for `/follow`
(now `AllowMining`, re-routes through build moves on goal changes — see
`test_follow_style_goal_change_reroutes_through_build_moves`), **yes** for
`/goto-block` when its `allow_mining` argument is true (unchanged, already
routed through `AllowMining`), **conditionally** for anything using
`NavigationMode::MovementOnly` explicitly (only `food/mod.rs`'s narrower use
case, deliberately left alone, out of scope) — build capability is bounded by
`config.toml`'s `[vertical_navigation]` (`enabled`, per-primitive `allow_*`
flags, allow/deny block lists, minimum-held threshold) so it can be disabled
or restricted without a code change.

## Known limitations to disclose in the final report (accumulate here as you go)

- Placement has no client-side prediction — multi-tick latency is inherent to this
  Azalea fork, not a bug in this implementation.
- `vendor/azalea` is tracked as a nested git repository (gitlink) in the parent
  repo — `git status`/`git diff` at the top level shows it as a single modified
  path (`m vendor/azalea`), not line-level diffs, unless changes are also
  committed inside vendor's own `.git`. Flagged for the user to decide how they
  want that diff reviewed/committed.
- Mining tool selection still uses Azalea's built-in auto-tool (not the app's
  richer durability/protected/reserved policy) — see design decision #4 above.
- Multi-block scaffold budgeting across a route isn't tracked — see design
  decision #6 above.
- Staircase-up over fully open air (no solid reference block anywhere nearby) is
  not a dedicated move; it emerges from A* composing pillar-up with walk/mine
  moves instead. Confirmed working via `test_combined_bridge_and_pillar_route`
  (Phase 6), but is a genuinely different code path than a single diagonal
  "staircase" move would be.
- Pathfinder-engine logging (path planned/reached/replanned/obstructed, plus
  the two new bridging/pillaring logs) is `tracing`-only and invisible unless
  `logging.debug = true` in config — it is not bridged into the app's
  chat/console `println!`-based log stream. See Phase 5 above for the
  rationale (matches pre-existing precedent for the rest of the vendored
  pathfinder's logging).
- `cargo build --release` currently fails with a reproducible rustc internal
  compiler error (ICE) while compiling the `tokio` dependency at
  `-C opt-level=3`, using the nightly pinned in `rust-toolchain.toml` (`rustc
  1.99.0-nightly (da86f4d07 2026-07-24)`). This is a toolchain bug unrelated to
  this feature — `cargo check`/`cargo test` (dev profile) both work fine, and
  the failure is in `tokio`'s own code, not `magic_ai_bot` or `azalea`. Not
  fixed in this session (changing the toolchain pin is the user's call); a
  repro file was dumped to
  `C:\Users\elias\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\tokio-1.53.1\rustc-ice-2026-08-01T18_16_34-7916.txt`.
- Manual in-game validation (`/goto` through a wall/ravine/tower/cliff/cave,
  `/follow` across changing terrain) was not run — no live/dev Minecraft
  server was reachable this session. The Phase 6 simulation tests are the
  closest available substitute and all pass, but they are not a replacement
  for real-server verification (real server tick timing, real block-update
  packet latency, and real terrain haven't been exercised).
