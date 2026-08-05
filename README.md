# Magic AI Bot

Headless Minecraft AI bot foundation written in Rust, with Azalea planned as the
Minecraft client framework. This stage provides configuration, logging, graceful
shutdown, and module boundaries only; it does not connect to a server or perform
gameplay actions.
Headless Minecraft Java bot written in Rust on Azalea. The current branch implements connection,
loaded-world snapshots, deterministic block search, movement/pathfinding, look control, and confirmed
single-block break/place actions. `/get <resource> <amount>` (Baritone-style, also usable from chat
as `#get <resource> <amount>`) adds bounded, universal resource gathering -- of blocks *and* mob
drops -- built entirely from those same primitives -- see below.

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
| Minecraft chat commands | Yes | `#`-prefixed messages run the same parser as the local console; access control, rate limiting. |

| Tool/action | Supported | Confirmation / cancellation |
|---|---:|---|
| Loaded block search | Yes | Loaded chunks only; `/stopall` cancels active controllers. |
| Move/follow | Yes | Azalea path status plus observed position; `/stop`. |
| Pathfinder mining | Opt-in | Only `/goto-mine` or the `mine` option; distinct from intentional `/break`. |
| Look | Yes | Rotation/visibility checks; `/lookstop`. |
| Break/place one block | Yes | Exact block-state change; `/stopinteraction`. |
| Inventory summary/tool hotbar selection | Read / limited | No menu clicks or inventory transaction service exists. |
| Craft/eat/deposit | No | Commands are intentionally absent. |
| Gather any loaded block by id/amount | Yes | `/get <resource> <amount>` (`#get` from chat); repeats nearest-reachable search + break until the inventory count is satisfied; `/stop` cancels. |
| Farm a mob drop by item/amount | Yes | Same `/get <resource> <amount>`, auto-detected via a resource->mob table (leather->cow, string->spider, ...); repeats nearest-reachable search + kill + collect; `/stop` cancels. |

| Container | Inspect | Transfer/click | Notes |
|---|---:|---:|---|
| Player inventory | Yes | Selected hotbar slot only | Snapshot may be unavailable while disconnected. |
| Chest/barrel/shulker/furnace/hopper | No | No | Never reported as open or closed without observation. |

## Commands and safety

Run `/help` for the complete list. Commands are local-console-only and therefore require access to
the bot process. World-changing commands have explicit names; `/goto` never mines, while
`/goto-mine` opts into Azalea pathfinder mining. `/break` is intentional interaction and is not a
pathfinder mode. Use `/stopinteraction` or `/stop` to cancel at the documented scope.
Success means an authoritative observed state transition, not merely a packet send.

Search radii, retry counts, timeouts, and look variation are bounded in `config.toml.example`.
Disconnect/reconnect and shutdown stop movement, navigation, look, and interaction; work is not
automatically resumed. Logs are local and are never forwarded to Minecraft chat. There is no task
orchestration layer -- console/chat commands call the owning service (movement, block navigation,
look, interaction) directly and wait for it to finish.

`/get <resource> <amount>` follows the same rule: it is a bounded loop over existing services
(`src/app.rs`'s `resolve_and_run_get_resource`, dispatching to `run_get_block` or `run_get_mob`),
not a new gathering system. `mobs::resolve_resource` decides which: it tries the mob-drop table
(`src/mobs/drops.rs`) first, falling back to treating `resource` as a block id -- a resource id that
is also an incidentally valid block (the wool colors) still resolves to farming the mob, since that
is how it is actually obtained.

The block path re-scans loaded chunks each iteration for the nearest *reachable* matching block
(falling back across candidates the same way `/gotoblock` does), navigates to it with mining
allowed, breaks it with the same tool-selection/verification pipeline as `/break`, walks onto the
broken block's position to trigger vanilla's proximity item pickup (mining reach is well beyond the
pickup radius, so without this the drop is often left on the ground and the count never advances),
and checks inventory again. Inventory is counted against the block's actual drop item, resolved once
per run
via `blocks::drop_item_for_block` (`src/blocks/drops.rs`) -- e.g. `#get diamond_ore 10` mines
`diamond_ore` but counts `diamond`, and `#get iron_ore 10` mines `iron_ore` but counts `raw_iron`.
There is no loot-table data bundled in Azalea to derive this from (loot tables are server-side
data-pack content, not part of the client protocol/registry), so it's a hand-maintained table
covering every vanilla block whose deterministic primary drop differs from the block itself (ores,
`stone`→`cobblestone`, `grass_block`→`dirt`, mismatched crop names, ...); anything not listed
defaults to "drops itself", which is correct for the overwhelming majority of blocks. Console output
always names the drop item, not the mined block, once they differ (e.g. `Collected 10 raw_iron from
iron_ore`).

The mob path (`src/mobs/combat.rs`'s `CombatController`) mirrors the same shape over entities
instead of blocks: it re-scans currently loaded entities each iteration for the nearest reachable
live mob (falling back across candidates the same way, and never re-attempting one it already
proved unreachable within that search), walks to melee range with `MovementService::goto` re-aimed
at the mob's live position, looks at it with the existing `LookTarget::Entity` support `/lookentity`
already uses, attacks with the one genuinely new primitive this added
(`MinecraftClient::attack_entity`), and walks onto the drop location afterward to trigger vanilla's
proximity pickup. The mob-drop table is intentionally small and hand-maintained
(`mobs::drops::MOB_DROPS`); extending it to more mobs/drops means adding a row there.

Either path stops and reports `[ERROR] Block not found: <block>` / `[ERROR] Mob not found: <mob>` if
no candidate exists in the loaded world, and aborts after 5 consecutive failures rather than
retrying forever. `/stop` cancels an in-progress run like any other movement/navigation.

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
- Container menu confirmation and clicks are implemented for chests only; crafting, eating,
  gathering, and death recovery remain unsupported.
- Pathfinder results and server acceptance remain environment-dependent; loaded-world search does
  not explore or reveal unloaded blocks (no X-ray).

## Project structure

```text
src/
├── main.rs       # Tokio entry point
├── app.rs        # Application composition and lifecycle
├── config.rs     # TOML configuration and logging setup
├── error.rs      # Unified application errors
├── logging.rs    # Logging helpers
├── minecraft/    # Azalea connection and lifecycle integration
├── console/      # Operator commands and parser
├── movement/     # Movement service and controls
├── navigation/   # Safe block approach and navigation
├── look/         # Rotation and look controllers
├── interaction/  # Block break/place, tool selection
├── blocks/       # Loaded-block search
├── tasks/        # Shared action-failure/identity types used by interaction
└── container/    # Chest interaction
```

Out of scope: AI planning, persistence, autonomous survival/exploration/combat, advanced building,
X-ray, reinforcement learning, and unrelated dependency upgrades.
