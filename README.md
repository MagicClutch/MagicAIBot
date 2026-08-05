# Magic AI Bot

Headless Minecraft AI bot foundation written in Rust, with Azalea planned as the
Minecraft client framework. This stage provides configuration, logging, graceful
shutdown, and module boundaries only; it does not connect to a server or perform
gameplay actions.
Headless Minecraft Java bot written in Rust on Azalea. The current branch implements connection,
loaded-world snapshots, deterministic block search, movement/pathfinding, look control, and confirmed
single-block break/place actions. Two Baritone-style commands add bounded, universal gathering built
entirely from those same primitives (see below): `/get <item> <amount>` (also `#get` from chat) asks
for an *item* and resolves how to obtain it (ore/conversion source blocks, or a mob to hunt) on its
own, while `/mine <block> [block...] <amount>` (also `#mine`) mines the *exact* block(s) named and
counts blocks destroyed, not items received.

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
| Gather an item by amount (ore/conversion or mob-drop, auto-resolved) | Yes | `/get <item> <amount>` (`#get` from chat); repeats nearest-reachable search + break/kill until the inventory count is satisfied; `/stop` cancels. |
| Mine an exact block (or any of several) by amount | Yes | `/mine <block> [block...] <amount>` (`#mine` from chat); repeats nearest-reachable search + break until the requested number of blocks is destroyed; counts blocks, not items; `/stop` cancels. |

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

`/get` and `/mine` follow the same rule: both are bounded loops over existing services
(`src/app.rs`'s `run_get_item`/`run_get_mob`/`run_mine`), not a new gathering system, and both reuse
`BlockNavigationService::start_multi` (search + nearest-*reachable*-of-several-block-ids + approach,
falling back across candidates the same way `/gotoblock` does -- generalized from a single block id
to a set so `#get diamond 10` can search `diamond_ore` *and* `deepslate_diamond_ore` in one pass) and
`InteractionController::break_at` (tool selection, precise look, break, verified removal, same as
`/break`).

They differ only in what the argument means and what gets counted:

- **`/get <item> <amount>`** (`#get` from chat) takes an *item* -- never a block to mine directly.
  `mobs::resolve_resource` (`src/mobs/mod.rs`) decides how it's obtained, in order: the
  ore/conversion table (`blocks::drop_blocks_for_item`, `src/blocks/drops.rs` -- e.g. `diamond` ->
  mine `diamond_ore` or `deepslate_diamond_ore`, whichever is nearer; an item that's *also*
  independently a valid block, like `cobblestone` or `dirt`, searches its own block form too, not
  just its conversion sources, or existing cobblestone/dirt would never be picked up), then the
  mob-drop table (`mobs::drops::MOB_DROPS` -- `leather` -> hunt `cow`), then finally "mine a block
  with this exact id" for anything that drops itself (`oak_log`, `sand`, ...). A handful of items
  are both a rare mob drop *and* an ore/crop product (`redstone`, `carrot`, `potato`,
  `glowstone_dust`); mining/farming wins there since it's the practical source. There is no
  loot-table data bundled in Azalea to derive any of this from (loot tables are server-side
  data-pack content, not part of the client protocol/registry), so `BLOCK_DROPS` is a hand-maintained
  table covering every vanilla block whose deterministic primary drop differs from the block itself;
  anything not listed defaults to "drops itself", correct for the overwhelming majority of blocks.
  After breaking, the block path walks onto the broken position to trigger vanilla's proximity item
  pickup (mining reach is well beyond the pickup radius, so without this the drop is often left on
  the ground and the count never advances) before checking inventory again -- inventory is always
  counted against the resolved item, never the mined block. The mob path
  (`src/mobs/combat.rs`'s `CombatController`) mirrors the same "fresh search every iteration, never
  retry a proven-unreachable candidate" shape over entities instead of blocks: walks to melee range
  with `MovementService::goto` re-aimed at the mob's live position, looks at it with the existing
  `LookTarget::Entity` support `/lookentity` already uses, attacks with the one genuinely new
  primitive this added (`MinecraftClient::attack_entity`), and walks onto the drop location
  afterward for the same proximity-pickup reason.
- **`/mine <block> [block...] <amount>`** (`#mine` from chat) takes one or more *blocks* literally --
  no resolution of any kind. `#mine diamond_ore deepslate_diamond_ore 10` mines whichever of the two
  is nearer each iteration and stops once 10 blocks (either kind) have been destroyed; `#mine stone
  100` only ever searches for `stone`. Progress is a plain count of blocks broken, never inventory,
  since what a block drops (or whether it drops anything at all -- silk touch, fortune, "drops
  nothing") is deliberately none of `/mine`'s concern.

Both stop and report `[ERROR] Block not found: <block>` / `[ERROR] Mob not found: <mob>` if no
candidate exists in the loaded world, and abort after 5 consecutive failures rather than retrying
forever. `/stop` cancels an in-progress run like any other movement/navigation.

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
