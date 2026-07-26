# Magic AI Bot

Headless Minecraft Java bot written in Rust on Azalea. The current branch implements connection,
loaded-world snapshots, deterministic block search, movement/pathfinding, look control, and confirmed
single-block break/place actions. It is **not** an autonomous resource-gathering bot.

## Quick start

1. Install the pinned nightly from `rust-toolchain.toml`.
2. Copy `config.toml.example` to `config.toml` and review every limit.
3. Use an offline test server with `account_mode = "offline"`, or configure Azalea's Microsoft
   device-code login. Never commit `config.toml` or `auth-cache/`.
4. Run `cargo run`; use `/help` and `/quit` from the local console.

The lockfile pins all Azalea crates to revision `6249c295d353b9b3ef68f665b311cba39211fd19`.
Dependency upgrades are deliberate compatibility work, not routine updates.

## Supported operations

| Source | Supported | Constraints |
|---|---:|---|
| Local console | Yes | Trusted local operator; bounded parser; plain text chat forwarding is off by default. |
| Minecraft chat commands | No | Incoming chat is display/state input only. |
| AI/provider plans | No | No provider, classifier, planner, or autonomous goals exist. |

| Tool/action | Supported | Confirmation / cancellation |
|---|---:|---|
| Loaded block search | Yes | Loaded chunks only; `/stopall` cancels active controllers. |
| Move/follow | Yes | Azalea path status plus observed position; `/stop`. |
| Pathfinder mining | Opt-in | Only `/goto-mine` or the `mine` option; distinct from intentional `/break`. |
| Look | Yes | Rotation/visibility checks; `/lookstop`. |
| Break/place one block | Yes | Exact block-state change; `/stopinteraction`. |
| Inventory summary/tool hotbar selection | Read / limited | No menu clicks or inventory transaction service exists. |
| Craft/eat/container/deposit | No | Commands are intentionally absent. |
| Gather logs/stone/visible ore/food | No | Lifecycle mocks exist, but no production gathering action exists. |

| Container | Inspect | Transfer/click | Notes |
|---|---:|---:|---|
| Player inventory | Yes | Selected hotbar slot only | Snapshot may be unavailable while disconnected. |
| Chest/barrel/shulker/furnace/hopper | No | No | Never reported as open or closed without observation. |

## Commands and safety

Run `/help` for the complete list. Commands are local-console-only and therefore require access to
the bot process. World-changing commands have explicit names; `/goto` never mines, while
`/goto-mine` opts into Azalea pathfinder mining. `/break` is intentional interaction and is not a
pathfinder mode. Use `/stopinteraction`, `/stop`, or `/stopall` to cancel at the documented scope.
Success means an authoritative observed state transition, not merely a packet send.

Search radii, retry counts, timeouts, and look variation are bounded in `config.toml.example`.
Disconnect/reconnect and shutdown stop movement, navigation, look, and interaction; work is not
automatically resumed. Logs are local and are never forwarded to Minecraft chat. Correlation IDs in
the task ownership layer are safe random identifiers and contain no account/session secret.

## Architecture and tests

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for ownership, lease order, cleanup, and the
hardening audit. Tests are deterministic unit/lifecycle mocks; no local-server fixture is bundled.

```bash
cargo fmt --check
cargo check
cargo test
```
# Tool-selection debug command

`/select-tool <block_id>` runs the same deterministic policy used by
intentional block breaking, prints its explanation, and selects the winning
hotbar slot. It is a debug command and does not break the block. The selector
never crafts, repairs, or enchants tools. Azalea pathfinder mining remains
independent and unchanged.

## Chest interaction primitive

The container state machine currently supports normal and trapped chests at exact loaded block positions. It reuses block navigation and precise look control, sends an explicit Azalea block interaction, accepts only authoritative `Generic9x3` (single) and `Generic9x6` (server-supported double chest) menus, and serializes pickup clicks while checking the menu window and server state revision. Exact and allowed-partial take/store requests are available through `/take-item` and `/store-item`; `/open-chest`, `/container-status`, and `/close-container` provide manual diagnostics.

This is intentionally a low-level chest adapter. It does not index or sort storage and does not support crafting, furnace processing, shulker boxes, or other exotic menus. Live server compatibility still depends on the pinned Azalea menu state ID updates and should be exercised against the Task 11 integration server before higher-level storage behavior is built.
## Known Azalea limitations

- The pinned Git revision is an internal compatibility surface and may require the pinned nightly.
- Readiness is currently represented by connection plus available snapshots, not a complete typed
  joining/chunk-readiness state machine.
- Container menu confirmation and clicks are not implemented; therefore crafting, eating, storage,
  gathering, death recovery, and full reconnect-generation integration tests are unsupported.
- Pathfinder results and server acceptance remain environment-dependent; loaded-world search does
  not explore or reveal unloaded blocks (no X-ray).

Out of scope: AI planning, persistence, autonomous survival/exploration/combat, advanced building,
X-ray, reinforcement learning, and unrelated dependency upgrades.
