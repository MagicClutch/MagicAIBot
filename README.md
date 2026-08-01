# Magic AI Bot

## Dropped-item collection status

Dropped-item collection is implemented for already loaded Azalea item entities. Use
`/collect-item minecraft:diamond 10`, a comma-separated item set, `group
ores|logs|food COUNT`, or `nearest`; `/collect-item status` and `/collect-item stop`
provide bounded debug controls. The collector ranks safe/near/relevant candidates
deterministically, checks inventory capacity, remembers failed entities, bounds
moving-target replans, and confirms pickup using inventory revisions and matching
count deltas alongside entity presence.

The action only walks with the existing movement service. It does not explore,
break blocks, open containers, or fight. Terrain hazards that Azalea does not expose
in the current application snapshot remain `Unknown` (allowed by the current
collector policy), stack capacity is conservatively modeled as 64, and a despawn
cannot be distinguished from an entity merge unless a matching inventory delta is
also observed. Task 9 integration is ready at the typed request/result boundary;
shared survival preemption and richer terrain annotations remain follow-up inputs.

Headless Minecraft AI bot foundation written in Rust, with Azalea planned as the
Minecraft client framework. This stage provides configuration, logging, graceful
shutdown, and module boundaries only; it does not connect to a server or perform
gameplay actions.
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
| Minecraft chat commands | Yes | `#`-prefixed messages run the same parser as the local console; access control, rate limiting. |

| Tool/action | Supported | Confirmation / cancellation |
|---|---:|---|
| Loaded block search | Yes | Loaded chunks only; `/stopall` cancels active controllers. |
| Move/follow | Yes | Azalea path status plus observed position; `/stop`. |
| Pathfinder mining | Opt-in | Only `/goto-mine` or the `mine` option; distinct from intentional `/break`. |
| Look | Yes | Rotation/visibility checks; `/lookstop`. |
| Break/place one block | Yes | Exact block-state change; `/stopinteraction`. |
| Inventory summary/tool hotbar selection | Read / limited | No menu clicks or inventory transaction service exists. |
| Craft/eat/container/deposit | No | Commands are intentionally absent. |
| Gather logs/stone/visible ore/food | No | Removed. |

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

## Read-only crafting knowledge (Task 11)

`/recipe <recipe-id>` inspects a recipe and `/craft-check <item> [count]
[depth]` builds a deterministic plan against a cloned inventory snapshot. The
service never clicks a menu, moves items, navigates, places a station, or changes
inventory.

The pinned Azalea revision targets Minecraft 26.2 / protocol 776. Azalea decodes
recipe-book displays and item/tag holders, but its client packet plugin currently
does not retain recipe-book packets, and the protocol's numeric display IDs are
not resource-location recipe IDs. Therefore the service labels and caches a
small, incomplete `fallback-1` diagnostic dataset for that exact version. The
fallback contains only recipes needed to exercise the knowledge API; unsupported
or absent recipes return structured failures rather than being guessed. A future
server-recipe adapter takes precedence over and replaces this fallback.

This read-only model and planner are ready to supply Task 12 inventory/crafting
execution work. Crafting-table presence, synchronized recipe unlock state, the
complete vanilla recipe corpus, special predicates, and non-crafting recipe
types remain explicit integration work; no execution behavior is included here.

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
├── blocks/       # Loaded-block search and ore safety
├── collection/   # Dropped-item collector
├── food/         # Food collection
├── tree_chopping.rs  # Tree detection and chopping
├── tasks/        # Shared action-failure/identity types used by interaction
├── inventory/    # Inventory service
├── container/    # Chest interaction
├── crafting/     # Recipe and crafting execution
├── smelting/     # Furnace/smelting state machines
├── processing/   # Processing-station knowledge
└── inventory_cleanup.rs  # Policy-driven cleanup
```

## Processing-station knowledge (Task 13 handoff)

The processing module is observational and pure: it accepts immutable inventory,
menu-slot, and menu-property snapshots and never opens, loads, waits on, collects
from, places, or obtains a station. Its extensible station identity covers furnace,
blast furnace, and smoker, while the bundled recipe catalog intentionally enables
standard-furnace calculations first.

**Pinned data/API decisions:** the lockfile pins Azalea commit
`6249c295d353b9b3ef68f665b311cba39211fd19`. At that revision,
`azalea-inventory::Menu` defines furnace, blast-furnace, and smoker as ingredient,
fuel, and result slots; `ClientboundContainerSetData` exposes property id/value;
the cooking-recipe display exposes ingredient, fuel, result, station, duration,
and experience; and `ClientboundUpdateRecipes` does not expose the complete
server cooking recipe set. Therefore third-party types remain at an adapter
boundary, menu property values may be absent, and the versioned standard-furnace
fallback catalog is deliberately small rather than pretending to be complete.
Fuel burn values are explicit pinned vanilla fallback data, not inferred from item
tags. Server/datapack recipe and fuel adapters can replace the catalog by revision.

Debug commands are `/furnace-status`, `/smelt-check <output> <count>`, and
`/fuel-info <item>`. The status command reports only an already-observed snapshot
and deliberately performs no station discovery or interaction.

**Limitations and Task 14 readiness:** live container identity/revision/property
capture is not yet wired because the pinned Azalea client leaves container-set-data
handling as a TODO. Blast-furnace and smoker types are modeled but their catalogs
are deferred. Task 14 may consume `ProcessingStationSnapshot`, `CookingRecipe`,
`Fuel`, and `ProcessingRequirements`; execution must separately add authoritative
snapshot capture and serialized inventory ownership, without adding side effects to
this knowledge module.
## Tree chopping (Task 32)

`/chop-tree nearest`, `/chop-tree <tree_type>`, `/chop-tree logs <n>`, and
`/chop-tree count <n>` inspect only loaded vanilla logs/leaves. Use
`/chop-tree status` or `/chop-tree stop` for lifecycle control. Detection is
bounded by the `[tree_chopping]` limits and skips structures without leaf
support or with ambiguous/mixed topology. Execution delegates navigation,
looking, hotbar tool selection, and server-confirmed breaking to the existing
services. It does not craft tools, break leaves, climb/build, replant, or
explore. Upper logs outside conservative ground reach are returned as partial.

Manual server matrix: test each configured vanilla tree, a neighboring pair,
a branched/tall tree, missing axe, full inventory, block changed mid-operation,
and stop/disconnect/death while chopping. Mock tests do not exercise live
Azalea networking, pathfinding, item-drop pickup, or server registry variants.
Task 17 (entity interaction) remains independent and ready; this feature adds
no entity-control ownership.
## Crafting execution status

The crafting module accepts a resolved shaped or shapeless plan and executes a
bounded, one-output-at-a-time transaction through a serialized menu driver. It
selects the player 2x2 grid when possible and otherwise requests navigation to a
known loaded crafting table. Every driver mutation must return a newer
server-confirmed menu revision; output collection additionally requires the
expected inventory-count delta. Failures attempt to return cursor/grid items and
always report the remaining contents.

`/craft <item> <count>`, `/craft status`, and `/craft stop` are debugging
surfaces. Recipe lookup is intentionally not guessed by the command: until the
read-only RecipeKnowledge task supplies a resolved plan and the runtime Azalea
driver is wired, direct item requests are rejected. The executor never gathers
or recursively crafts ingredients, creates or places a table, smelts, or chooses
tools.

Task 13 integration is **not ready for a live-server claim**: the transactional
state machine and mocked player/table menus are ready, but this branch does not
contain the prerequisite RecipeKnowledge/InventoryState services or a live
Azalea menu-driver adapter. Those prerequisites should provide authoritative
inventory revisions and own the Azalea click/navigation bridge before the next
task consumes crafting execution.
Out of scope: AI planning, persistence, autonomous survival/exploration/combat, advanced building,
X-ray, reinforcement learning, and unrelated dependency upgrades.
