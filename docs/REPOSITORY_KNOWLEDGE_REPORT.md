# Magic AI Bot — Repository Knowledge Report

Scope: full read of the current working tree (dirty, uncommitted) on 2026-07-31.
This supersedes `docs/PROJECT_KNOWLEDGE.md`, `docs/AI_CONTEXT.md`, and `docs/COMMANDS.md`
where they conflict with source — those three files were written before the most recent
uncommitted edits and describe an **older command/registry shape** (task-specific console
commands, a task-specific Groq registry with `gather`/`mine_ore`/`chop_tree`/etc.). The
actual current source has already moved to a **primitive-capability architecture**. This
report is based on direct reading of `src/`, `Cargo.toml`, `config.toml.example`, and a
live `cargo check`, not on the older docs. No code was changed to produce this report.

---

## 0. Headline finding: a primitive-capability refactor is already in progress

Before any new refactor work starts, understand that the repo is **mid-migration**:

- `src/ai/registry.rs::command_registry()` now exposes only ~20 low-level primitives
  (`walk_to`, `follow_entity`, `look_at`, `stop_movement`, `break_block`, `place_block`,
  `use_block`, `equip_item`, `move_inventory_item`, `drop_item`, `use_item`, `craft_item`,
  `open_container`, `take_item`, `store_item`, `close_container`, `attack_entity`,
  `interact_with_entity`, `wait`, `cancel_action`, `finish`, plus read-only tools). There
  is **no** `gather`, `mine_ore`, `chop_tree`, `collect_item`, `collect_food`, or
  `goto_block` tool anymore. The system prompt built by `build_system_prompt()` explicitly
  tells the model: *"You do NOT have task-specific commands such as collect_stone,
  build_house, or craft_furnace... you must solve every request by planning a sequence of
  primitive capabilities."*
- Correspondingly, in `src/app.rs::execute_console_input`, the console commands
  `/gather`, `/gathercancel`, `/stopall`, `/mine-ore*`, `/collect-item*`, `/collect-food*`,
  and `/chop-tree*` are **still parsed** (they exist in `ConsoleCommand`) but their handlers
  now just print `"This command is no longer available; use AI capabilities instead."`
  They do nothing.
- The modules that used to implement those high-level behaviors — `src/tasks/gather.rs`,
  `src/tree_chopping.rs`, `src/blocks/mining.rs`, `src/collection/`, `src/food/`,
  `src/tasks/ensure_tool.rs` — are still compiled (declared in `main.rs`) but are **no
  longer called from `app.rs`, `ai/mod.rs`, or `tasks/mod.rs`'s used surface**. `cargo check`
  confirms this: dead-code warnings cluster almost exactly on these files (`smelting/mod.rs`
  24, `blocks/mining.rs` 20, `food/mod.rs` 19, `processing/mod.rs` 18, `tree_chopping.rs` 17,
  `inventory_cleanup.rs` 17, `crafting/mod.rs` 15, `inventory/service.rs` 12,
  `collection/mod.rs` 12).
- `docs/PROJECT_KNOWLEDGE.md`, `docs/AI_CONTEXT.md`, and `docs/COMMANDS.md` (all untracked,
  written earlier in this same session) still describe the **old** registry and the old
  `App::{start_gather, tick_gather}` gather glue. That glue code (`tick_gather`,
  `finalize_ai_task`'s old gather-specific cleanup, `ActiveGather`) is gone from `app.rs`
  as currently written. Treat those three docs as historical/aspirational, not current.

This matters directly for the task described in the prompt ("primitive-capability
refactor"): **a first pass of exactly that refactor already happened** in the AI/registry
layer, but it was not carried through to the console layer (which still references the old
command surface, now dead-ended) or to the orphaned task-specific modules (which still
exist, still compile, still have their own tests, but are reachable from nothing). The
remaining work is cleanup/completion, not a from-scratch redesign.

---

## 1. Directory overview

| Directory | Purpose | Live? |
|---|---|---|
| `src/` (root files) | `main.rs` bootstrap, `app.rs` composition root/dispatch, `config.rs` TOML config, `error.rs`, `logging.rs` | Live |
| `src/minecraft/` | Azalea connection lifecycle (`client.rs`), event→state translation (`events.rs`), bounded world snapshot (`world_state.rs`), container menu adapter (`container.rs`), inventory action lease (`inventory_actions.rs`), dropped-item search (`dropped_items.rs`), server recipe packet stub (`recipe_manager.rs`, new/incomplete) | Live |
| `src/console/` | `/`-prefixed local command parser (`commands.rs`) and stdin reader (`mod.rs`) | Live (parser); dispatcher partially dead-ended, see §0 |
| `src/ai/` | `mod.rs` (session, `AiAction` enum, validation, chat parsing), `provider.rs` (`AiProvider` trait, OpenAI-style message/tool types), `groq.rs` (Groq HTTP client), `registry.rs` (primitive tool registry, system prompt) | Live, actively evolving |
| `src/movement/` | `MovementService`: Azalea goal submission, follow, arrival tracking, camera-relative local-input suggestion (`multitasking.rs`) | Live |
| `src/navigation/` | `BlockNavigationService`: candidate approach-position generation/selection for reaching a target block safely | Live |
| `src/look/` | `LookController`: rotation, aim-point selection, human-like randomization/interpolation, visibility checks | Live |
| `src/interaction/` | `InteractionController`: break/place with face selection, tool selection, reach checks, retries, server-confirmed state transitions | Live |
| `src/blocks/` | Loaded-world block search (`block_search.rs`, `block_query.rs`), snapshot type, ore mining logic (`mining.rs`) | Search live; `mining.rs` orphaned |
| `src/tasks/` | `TaskService` (lease/history/cancellation runtime + thin wrapper functions used by `app.rs` and the AI dispatch: `goto_position`, `break_block`, `place_block`, `look_at`, `find_block`, `goto_block`, `gather_visible_blocks`); `lifecycle.rs`/`runtime.rs` (two alternate resource-ownership models); `gather.rs`, `ensure_tool.rs` (task-specific, orphaned) | Mixed — see §5 |
| `src/collection/` | Dropped-item collector (`DropCollector`) | Orphaned (compiles, has tests, unreachable from `app.rs`) |
| `src/food/` | Food-item selection/collection controller | Orphaned |
| `src/tree_chopping.rs` | Conservative tree-topology detection + chop execution | Orphaned |
| `src/container/` | Chest/container state machine (model/planner/service/state_machine) | Live — this is the one used by AI `open_container`/`take_item`/`store_item`/`close_container` |
| `src/crafting/` | `RecipeBook` (read-only recipe knowledge, versioned fallback data) + `CraftService` (transactional menu-click state machine) | `RecipeBook` live (read-only lookups); `CraftService` execution path effectively unused — the AI `craft_item` action never reaches it (see §6) |
| `src/inventory/` | `InventoryService`/planner — generic inventory transaction model | Orphaned |
| `src/smelting/`, `src/processing/` | Furnace/smelting and generic processing-station knowledge/state machines | Orphaned |
| `src/inventory_cleanup.rs` | Policy-driven inventory cleanup state machine | Orphaned |
| `src/skills/` | `mod.rs` is a single comment line — placeholder for future capability grouping | Empty stub |
| `vendor/azalea/` | Vendored Azalea Minecraft client/protocol crates (path dependency, pinned commit) | External |
| `Prompts/` | 59 historical staged-requirement prompt files (`00_...` to `59_...`) that were used to build the project incrementally; not runtime code | Historical spec, not source of truth for current state |
| `docs/` | `ARCHITECTURE.md` (ownership/lease audit, slightly stale), `AI_CONTEXT.md`/`COMMANDS.md`/`PROJECT_KNOWLEDGE.md` (stale re: registry, see §0), `gemini.md`, `furnace-execution.md` | Mixed freshness — this report is the current source of truth |

## 2. Module overview (responsibilities)

- **`App` (`app.rs`, 2877 lines)** — composition root. Owns `MinecraftClient`,
  `MovementService`, `BlockNavigationService`, `LookController`, `InteractionController`,
  `TaskService`, `RecipeBook`, `CraftService`, `ContainerService`, and the optional
  `AiSession`. Runs the single `tokio::select!` event loop (movement tick, look tick,
  interaction tick, console input, Ctrl-C). Dispatches console commands and AI actions by
  calling into the owned services directly — it is a thin glue layer, not a pathfinder or
  protocol implementation itself.
- **`MinecraftClient` (`minecraft/client.rs`, 1827 lines)** — the only place that touches
  Azalea's ECS/protocol. Owns connect/auth/reconnect, chat send, world-state snapshot
  publishing, and a set of `pub(crate)` low-level action primitives: `interact_block`,
  `container_click`, `use_item_at_look_target`, `attack_entity`, `interact_with_entity`,
  `drop_item`, `select_item_in_hotbar`, `select_tool_for_block`, `set_current_task`/
  `clear_current_task`, navigation start/stop/status. These are the actual "hands" the rest
  of the app calls.
- **`WorldState`/`WorldStateSnapshot` (`minecraft/world_state.rs`, 942 lines)** — bounded,
  owned snapshot of bot position/health/food, inventory, nearby players/entities, dropped
  items, container observation, chat history, current task label. Refreshed from Azalea
  events (`events.rs`), not per-tick polling of raw ECS. Inventory `revision` is hard-coded
  0 (no real menu-revision tracking for the player's own inventory).
- **`ConsoleCommand`/`parse_input` (`console/commands.rs`, 1184 lines)** — the local `/`
  syntax parser. ~68 enum variants. Authoritative for console syntax; **not** authoritative
  for what actually executes (several variants are dispatch-dead, see §0).
- **`ai/mod.rs` (1415 lines)** — `AiAction` enum (21 primitive-shaped variants), `AiSession`
  state machine, chat-prefix parsing (`chat_request`), requester binding
  (`bind_requester_intent`/`resolve_requester_references`), `validate_action` (registry +
  per-action safety bounds), `tool_call_to_action` (JSON tool-call → typed action).
  `verify_objective` is a stub that always returns `false` (its parameters are unused —
  confirmed by compiler warnings); objective completion is currently driven entirely by the
  model calling `finish`, not by any independent verification.
- **`ai/provider.rs` (460 lines)** — `AiProvider` trait, provider-agnostic
  `AiRequest`/`AiResponse`/`ToolCall`/`ToolDefinition`/`ChatMessage` types (OpenAI-style
  tool-calling shape).
- **`ai/groq.rs` (787 lines)** — `GroqProvider`, the only concrete `AiProvider`
  implementation. Talks to `{base_url}/chat/completions` (default
  `https://api.groq.com/openai/v1`) with `Authorization: Bearer`, handles rate-limit /
  auth / unknown-model errors distinctly, supports a `fallback_models` list and a
  `service_tier` field not documented in the older docs.
- **`ai/registry.rs` (694 lines)** — single source of truth for what the AI can call:
  `command_registry()` (primitive definitions), `generate_readonly_definitions()`
  (13 read-only query tools: `query_inventory`, `get_recipe`, `get_item_information`,
  `get_block_information`, `get_available_capabilities`, `get_active_action`,
  `get_bot_position`, `get_bot_health`, `get_food_level`, `get_equipment`,
  `get_nearby_entities`, `get_block`, `find_blocks`), and `build_system_prompt()`.
- **`movement/movement_service.rs` (419 lines)** — Azalea path-goal submission/tracking
  with a 2s pathfinder-startup grace period, follow-player logic, repath interval, arrival
  distance. Azalea itself is the pathfinder; this service only submits goals and observes
  status/position.
- **`navigation/block_navigation.rs` (543 lines)** + **`navigation/approach_position.rs`**
  — generates cardinal/diagonal/overhead standing-cell candidates around a target block,
  validates loaded/passable feet+head, solid support, interaction distance, then hands the
  chosen approach position to `MovementService`. Tracks failed blocks/approaches per
  navigation "generation" (reset on each new `start()` call), 120s timeout, bounded attempts.
- **`interaction/interaction_controller.rs` (1125 lines)** — owns intentional break/place:
  precise look, tool selection (`tool_selection.rs`), face selection (`faces.rs`),
  reach check (`reach.rs`), placement legality (`placement_rules.rs`), progress estimation,
  and retries until a server-confirmed block-state change is observed. This is distinct
  from and independent of Azalea's own pathfinder-mining mode (`NavigationMode::AllowMining`,
  used only by `/goto-mine` and `goto_block(..., allow_mining)`).
- **`blocks/block_search.rs`/`block_query.rs`** — bounded loaded-world block search
  (radius/vertical-range/result-limit clamps); `blocks/mining.rs` (735 lines, ore
  targeting/safety logic) is orphaned.
- **`container/` (model/planner/service/state_machine)** — chest interaction state machine.
  Confirmed live: backs `open_container`/`take_item`/`store_item`/`close_container` AI
  actions and the `/open-chest`, `/take-item`, `/store-item`, `/close-container` console
  commands. Supports `Generic9x3`/`Generic9x6` chest menus only (README: no furnace/shulker/
  crafting-table menu support here).
- **`crafting/mod.rs` (881 lines)** — `RecipeBook` (versioned fallback recipe data,
  read-only planning against a cloned inventory — used live by `/recipe`, `/craft-check`,
  and the AI `craft_item` handler's plan check) and `CraftService` (a real transactional
  menu-click driver with revision confirmation, cursor recovery, etc. — built and tested,
  but **never invoked** by `app.rs` except for its inert `status()`/`stop()`).
- **`tasks/mod.rs` (1275 lines)** — `TaskService`: a lease/history/cancellation runtime
  (`TaskId`, `TaskState`, `TaskResource`, `CancellationReason`, `TaskFailure`) plus a second
  half (`impl TaskService` block starting ~line 830) of thin async wrapper functions
  (`goto_position`, `goto_block`, `look_at`, `look_at_block`, `break_block`,
  `break_looked_block`, `break_nearest_block`, `place_block`, `place_looked_block`,
  `gather_visible_blocks`, `find_block`) that each call `tracked(...)` to register a task,
  invoke the underlying controller/service **and fully await its completion**, then record
  success/failure. This is the layer both `app.rs` console dispatch and
  `execute_ai_action` call into for movement/look/break/place. `gather_visible_blocks` is
  built but currently unused by both console and AI paths.
- **`tasks/lifecycle.rs` (363 lines)** and **`tasks/runtime.rs` (265 lines)** — two
  *additional*, independent resource-ownership/lease abstractions (`ResourceManager`/
  `OperationContext`/`ActionFailure`/`Invalidation` in `lifecycle.rs`; `ResourceLeases` in
  `runtime.rs`, an ordered movement→look→interaction→inventory→container lock). Neither is
  referenced from anywhere outside its own file — confirmed by `grep` and by `cargo check`
  having no live call sites. This is the duplication the old `ARCHITECTURE.md` audit flagged
  and it is still true today: three parallel notions of "task/resource ownership" exist
  (`TaskService`'s own lease system, `tasks::lifecycle`, `tasks::runtime`), only the first is
  wired up.
- **`config.rs` (1831 lines)** — single `Config` struct, TOML deserialization with
  `#[serde(default)]` almost everywhere except `[minecraft]`/`[reconnect]`/`[logging]`,
  bounds validation, `init_logging`. Very large but mostly repetitive per-section structs
  and default-value functions.

## 3. Control flow — from a player command to a Minecraft action

Two independent front doors converge on the same primitive dispatch:

**Local console:** `console::read_input` (stdin, spawned local task) → `parse_input` builds
a `ConsoleCommand` → `App::execute_console_input` matches on the variant → for movement/look/
break/place, calls `TaskService::{goto_position,break_block,place_block,look_at,...}`, which
registers the call as a tracked task, invokes the owning controller/service, and awaits full
completion before returning. Many high-level variants (`Gather`, `MineOre*`, `CollectItem*`,
`CollectFood*`, `ChopTree*`, `StopAll`, `GatherCancel`) are parsed but print a "no longer
available" message and do nothing (§0).

**AI (chat or `/ai`):** `tick_ai_chat` (movement-tick cadence) pops one queued player chat
message → prefix/allow-list/rate-limit checks → `submit_ai_request` builds a context string
(position, health, food, inventory count, nearby players, previous-action result, step
count) → calls `GroqProvider::chat` with the full primitive+read-only tool list and the
generated system prompt → on a `tool_calls` response, read-only tools are executed inline
in a loop (results fed back as `tool_result` messages, loop continues without returning
control to the app's event loop) and the **first physical tool call** is converted via
`tool_call_to_action`, validated (`ai::validate_action`), and handed to
`execute_ai_action`, which calls the same `TaskService`/`MinecraftClient`/`ContainerService`
primitives the console uses, fully awaiting each one, then calls `complete_ai_action`
(records the result on the session, sets session status to `Replanning`) and **returns**
back up through `submit_ai_request` to the caller.

**Known gap (see §9):** nothing currently re-enters `submit_ai_request` when the session is
in `Replanning`. `tick_ai_provider` (the only periodic re-entry point) only resumes sessions
in `WaitingForProvider` (a Groq rate-limit backoff state), not `Replanning`. In the current
source, after the AI executes exactly one physical primitive it stalls: the session stays
"active" (blocking new requests as busy) until `/aicancel`, `!stop`, or the session-timeout
watchdog (`enforce_ai_limits`, default 600s) ends it. Multi-step autonomous composition —
the entire point of the primitive-capability system prompt — does not currently continue on
its own after step 1. This should be treated as the top-priority functional bug for the next
implementation pass, not an architecture question.

## 4. AI architecture

- **Provider layer:** `AiProvider` trait (`ai/provider.rs`) with one implementation,
  `GroqProvider` (`ai/groq.rs`), talking to Groq's OpenAI-compatible `/chat/completions`.
  No Gemini client remains in source (the `[gemini]` config section is preserved only for
  backward-compatible field names feeding into `GeminiLimits`/`GroqConfig`; there is no
  `GeminiClient` type in `src/`).
- **Tool/capability registry:** `ai::registry::command_registry()` is the single allowlist
  used both to generate Groq tool-call schemas (`generate_tool_definitions`,
  `generate_all_tool_definitions`) and to gate execution (`action_is_registered`, called
  from `validate_action`). Read-only tools are a separate, always-enabled list
  (`generate_readonly_definitions`) executed inline without going through `AiAction`/
  `TaskService` at all (see `execute_readonly_tool` in `app.rs`, not fully read line-by-line
  in this pass but confirmed to exist at `app.rs:1280`).
- **Session handling:** one `Option<AiSession>` lives on `App`. `AiSession` tracks a single
  `active_action` at a time (`begin_action`/`complete_action` with a UUID
  `AiExecutionId` guard against stale completions), a linear `action_history`, and a
  `generation` counter bumped on cancellation. There is no queue — a second request while a
  session is "active" (non-terminal) is rejected unless it looks like a state/conversation
  query (`submit_ai_request`'s `is_conversation_or_query` heuristic: starts with "what"/
  "how"/"do you"/"are you", or contains "inventory"/"health"/"position"/"status"/"doing"/
  "nearby").
- **Planning:** there is no separate planning phase/data model — the "plan" is implicit in
  the model's tool-call sequence, mediated one physical action at a time by the
  system-prompt rules ("Execute no more than one physical capability at a time... When the
  objective is complete, call finish").
- **Execution:** `execute_ai_action` in `app.rs` (~460 lines, one match arm per `AiAction`
  variant) is the sole dispatcher. Movement/look/break/place go through `TaskService`
  wrappers (tracked, awaited). Equip/drop/use-item/attack/interact/container-click go
  directly to `MinecraftClient`/`ContainerService` methods, generally as one-shot calls
  without the retry/verification depth that `InteractionController` gives break/place — an
  inconsistency worth flagging for later hardening (equip/attack/use currently trust a
  single client call's `Result`, not an observed-state confirmation loop).
- **Error handling:** provider errors are categorized (`RateLimited` → backoff state with
  retry-at and model, `UnknownModel`, `Permission`/`AuthenticationError`, generic) each with
  a distinct session-terminal or backoff outcome. Physical-action failures produce a
  `CapabilityResult::failed(status, message)` with a `retryable` flag derived from the
  `CapabilityStatus` (Unreachable/OutOfRange/TimedOut/TargetNotFound/MissingItem are
  retryable) — but nothing currently *acts* on `retryable` automatically; it's informational
  data returned to the model, which decides what to do next (when the loop resumes at all,
  see §3's gap).

## 5. Task architecture

- **Lifecycle:** `TaskService` (`tasks/mod.rs`) assigns a `TaskId`, tracks `TaskState`
  (Created→Queued→WaitingForResources→Running→...→terminal), records `TaskProgress` and a
  bounded history (`DEFAULT_HISTORY_LIMIT = 64`). `submit`/`run`/`run_child` support parent/
  child task relationships and per-task resource declarations (`TaskResource`: Movement,
  Rotation, Interaction, InventoryMutation, ContainerAccess, Combat,
  ExclusivePlayerControl).
- **Ownership:** the `tracked()` helper (used by all the `goto_position`/`break_block`/etc.
  wrappers) submits a task, sets `MinecraftClient`'s "current task" label for status display,
  awaits the closure, and reports success/failure back into task history. This is real and
  used. The two other ownership abstractions (`tasks::lifecycle::ResourceManager`,
  `tasks::runtime::ResourceLeases`) are unused dead code (§2).
- **Cancellation:** `TaskService::cancel_task`/`cancel_all` plus typed
  `CancellationReason` (User, ParentCancelled, Shutdown, Disconnected, PlayerDied,
  Preempted, Replaced). `App::finalize_ai_task` is the central cleanup entry point called
  on AI `Finish`, `CancelAction`, session timeout, and `/aicancel`/`!stop`/`!cancel`: it
  cancels `InteractionController`, `BlockNavigationService`, stops `MovementService`, and
  calls `self.tasks.cancel(&self.minecraft)`.
- **Completion/synchronization:** because each primitive wrapper `.await`s its underlying
  controller to full completion before returning, there is no separate "poll until task
  reaches terminal state" step for these primitives — the task's terminal state is known the
  instant the wrapping async call returns. The main event loop's `interaction_tick` (50ms)
  still separately ticks `BlockNavigationService`, `InteractionController`, and
  `ContainerService` for their own internal retry/observation state machines (those run
  concurrently as background controllers even though the specific *task* awaiting them
  blocks on their result).

## 6. Pathfinding architecture

- **Goals:** Azalea's own pathfinder is the actual path-execution engine; this codebase
  never reimplements A*/pathing. `MovementService::goto` submits an Azalea goal via
  `MinecraftClient::start_navigation_to` and tracks arrival by Euclidean distance
  (`arrival_distance`, default 1.5) with a periodic re-path (`repath_interval_ms`, default
  500) and a 2-second startup grace period before trusting Azalea's reported
  calculating/executing status.
- **Movement controller:** `MovementService` also exposes `follow(player)` (repeatedly
  re-goals toward a tracked player's last known loaded position; goes idle if the player is
  lost) and a camera-relative "local input suggestion" (`multitasking.rs`) that does not
  drive movement itself but is available for future human-like-input work.
- **Navigation loop (block targeting):** `BlockNavigationService` is the layer above raw
  goto for "reach this block": `approach_position::approach_positions` generates
  cardinal/diagonal/overhead standing-cell candidates, `is_valid_approach`/`required_cells`
  check loaded/passable feet+head and solid support, `target_selector::next_candidate`
  picks the next one to try. Bounded by `maximum_target_attempts` (10), `stuck_timeout_seconds`
  (12s), `maximum_navigation_seconds` (120s). Failed blocks/approaches are remembered only
  for the current navigation "generation" (reset each `start()` call) — no cross-task memory.
- **Mining integration:** `NavigationMode::AllowMining` opts a goto into Azalea's own
  pathfinder-mining (used by `/goto-mine` and mining-aware `goto_block`); this is completely
  separate from `InteractionController`'s intentional single-block break, which never enables
  pathfinder mining.
- **Current limitations (confirmed, not inferred):** "no valid approach position" means no
  candidate standing cell passed the preflight checks in currently *loaded* chunks — it does
  not prove the target is globally unreachable. There is no exploration/chunk-loading
  behavior anywhere in the movement/navigation stack — everything operates only on the
  bot's current loaded-world snapshot.

## 7. Existing primitive capabilities (reusable, low-level)

Confirmed live and reachable from both console and AI:

| Capability | Backing code | Notes |
|---|---|---|
| Walk to position | `TaskService::goto_position` → `MovementService::goto` | No mining unless `NavigationMode::AllowMining` |
| Walk to block by ID | `TaskService::goto_block` → `BlockNavigationService::start` | Search + approach-position selection |
| Follow player | `MovementService::follow` | Continuous; no terminal completion signal beyond a snapshot check |
| Look at position/block/player/entity | `LookController` via `TaskService::look_at`/`look_at_block` | Human-like randomization/interpolation |
| Break one block (by coordinate, by "looked-at", by nearest-of-ID) | `InteractionController::{break_at,break_looked,break_nearest}` via `TaskService` | Tool selection, face selection, retries, server-confirmed |
| Place one block | `InteractionController::{place_at,place_looked}` via `TaskService` | Placement legality (`placement_rules.rs`), support checks |
| Right-click a block | `MinecraftClient::interact_block` | One-shot, no retry loop |
| Equip item to hotbar | `MinecraftClient::select_item_in_hotbar` | One-shot |
| Select best tool for a block (debug/inspection) | `MinecraftClient::select_tool_for_block` | Used by `/select-tool`; also feeds `InteractionController`'s auto tool switch |
| Move inventory item between slots | `MinecraftClient::container_click` (two clicks) | Simple pickup/place, not a full validated transaction |
| Drop item | `MinecraftClient::drop_item` | One-shot |
| Use held item | `MinecraftClient::use_item_at_look_target` | One-shot |
| Open/close chest, take/store item | `ContainerService` (`container/`) | Generic9x3/9x6 only |
| Attack entity / interact with entity | `MinecraftClient::{attack_entity,interact_with_entity}` | One-shot, no combat loop |
| Read-only queries (inventory, recipe, item/block info, position, health, food, equipment, nearby entities, block-at-position, find-blocks, active-action) | `ai/registry.rs::generate_readonly_definitions` + handlers in `app.rs::execute_readonly_tool` | Never mutate state |
| Loaded-block search | `BlockSearchService` (`blocks/block_search.rs`) | Radius/vertical-range/result-limit bounded |
| Recipe lookup / crafting plan (read-only) | `RecipeBook` (`crafting/mod.rs`) | Small pinned fallback dataset, not the full vanilla recipe corpus |

Built but currently unreachable from the primitive dispatch (candidates to either wire in or
formally retire): `gather_visible_blocks` (`tasks/mod.rs`), `TreeChopService`
(`tree_chopping.rs`), `MiningService`/ore logic (`blocks/mining.rs`), `DropCollector`
(`collection/`), `FoodCollector` (`food/`), `EnsureTool` planner (`tasks/ensure_tool.rs`),
`CraftService`'s actual menu-transaction execution (`crafting/mod.rs`), `InventoryService`
(`inventory/`), smelting/processing state machines, `inventory_cleanup.rs`.

## 8. Existing high-level/task-specific commands

**Currently functional, console-only** (not exposed to the AI registry): `/recipe`,
`/craft-check` (read-only planning), `/testoaklog` (debug break/restore), `/select-tool`
(debug tool scoring).

**Parsed but dispatch-dead** (print "no longer available; use AI capabilities instead" or
an equivalent stub, per §0): `/gather`, `/gathercancel`, `/stopall`, `/mine-ore` (+status/
stop), `/collect-item` (+status/stop), `/collect-food` (+status/stop), `/chop-tree`
(+status/stop), `/craft <item> <count>` (rejected with an explicit message — recipe
execution is intentionally not guessed), `/ensure-tool` (prints a stub message, takes no
action).

**No task-specific tool exists in the AI registry at all** — by design. The AI is expected
to *compose* primitives (e.g., `find_blocks` → `walk_to` → `equip_item` → `break_block`,
repeated) rather than call a single `gather`/`mine_ore`/`chop_tree` action. This is a
deliberate architectural stance stated directly in `build_system_prompt()`, not an oversight
— but it is currently undermined by the Replanning-stall bug in §3/§9, which prevents that
composition from actually running past one step autonomously.

## 9. What should be preserved (strong foundation)

- **`MinecraftClient`/`WorldState`/`events.rs`** — the Azalea integration boundary is clean:
  one place owns ECS access, one bounded snapshot is the sole read model for everything
  else. Good invariant to keep as the project grows.
- **`MovementService` + `BlockNavigationService` + `LookController` +
  `InteractionController`** — each is a focused, independently testable state machine with
  its own snapshot/status type, config, and cancellation entry point. This is the most
  mature, best-tested layer of the codebase and should remain the foundation any new
  high-level behavior is built on top of, not around.
- **`ai/registry.rs` as single source of truth** — generating both the Groq tool schema and
  the execution allowlist from one `command_registry()` function is the right pattern; keep
  extending it there rather than letting console and AI command surfaces diverge again.
- **`TaskService`'s `tracked()` wrapper pattern** — task registration + await + history
  recording in one place, used consistently by both console and AI dispatch. This is the
  one resource-ownership abstraction that's actually wired up; it should be the one that
  survives (see §10).
- **`ContainerService`** — the only "menu interaction" system that is both fully built and
  actually reachable end-to-end from both console and AI. A good template for what
  `CraftService` and `InventoryService` would need to become to earn the same status.
- **Config validation and bounded defaults throughout** (`config.rs`) — search radii, retry
  counts, timeouts, navigation distance, action/session limits are all clamped, not
  unbounded. Worth preserving as new capabilities are added; don't introduce an unbounded
  primitive.

## 10. What should eventually be simplified

- **Resolve the `Replanning` stall (§3)** before anything else — it silently caps every AI
  task at one physical action today. This is a functional bug, not a design debate.
- **Three parallel task/resource-ownership systems** (`TaskService`'s own lease/tracking,
  `tasks::lifecycle::ResourceManager`, `tasks::runtime::ResourceLeases`) where only the
  first is used. Delete or finish integrating the other two — carrying dead ownership
  abstractions invites someone building new code against the wrong one.
- **Orphaned task-specific modules**: `tasks/gather.rs`, `tasks/ensure_tool.rs`,
  `tree_chopping.rs`, `blocks/mining.rs`, `collection/`, `food/`, `inventory/`,
  `smelting/`, `processing/`, `inventory_cleanup.rs`. All still compile and (mostly) have
  their own unit tests, but none are reachable from `main`'s actual control flow anymore.
  Each is a candidate to either (a) be re-wired as an internal implementation detail behind
  a primitive the AI calls (e.g., `craft_item` eventually driving `CraftService`), or (b) be
  deleted once it's confirmed nothing needs its logic — that decision should be made
  module-by-module, not in bulk, and is explicitly out of scope for this analysis pass.
- **Console command surface vs. AI registry drift** — `ConsoleCommand` still has ~20 dead-
  dispatch variants (§0/§8) left over from before the primitive-capability cutover. Either
  restore console execution through the same primitive path the AI now uses, or remove the
  parser entries — leaving them as silent no-ops is the worst of both options for an
  operator typing commands at the console.
- **`AiAction`'s physical-action robustness is inconsistent** — break/place get full
  retry+confirmation via `InteractionController`; equip/drop/use-item/attack/interact/move-
  inventory-item are one-shot `MinecraftClient` calls with no verification loop. If the AI is
  going to compose many small primitives per task, the failure modes of the "thin" primitives
  need the same server-confirmed-state discipline the "thick" ones already have, or partial
  failures will silently corrupt multi-step plans.
- **`verify_objective` is a no-op stub** (`ai/mod.rs`) — `AiObjective` only has a `General`
  variant and the function always returns `false`; objective completion is entirely
  self-reported by the model via `finish`. If any automatic-verification safety net is
  wanted before shipping autonomous behavior, this is where it plugs in.
- **`docs/PROJECT_KNOWLEDGE.md`, `docs/AI_CONTEXT.md`, `docs/COMMANDS.md`** are now stale
  relative to source (§0) and should be regenerated or retired once the console/registry
  drift above is resolved, to avoid future sessions inheriting the outdated command tables.

---

## Appendix: verification method

Claims above were checked directly against source (not copied from the pre-existing docs):
full or partial reads of `app.rs`, `ai/mod.rs`, `ai/registry.rs`, `ai/provider.rs`,
`ai/groq.rs`, `tasks/mod.rs`, `movement/movement_service.rs`, `navigation/block_navigation.rs`,
`interaction/interaction_controller.rs`, `minecraft/client.rs`, `minecraft/recipe_manager.rs`,
`console/commands.rs`, `config.rs` (structural), `Cargo.toml`; `grep`/cross-reference of
every task-specific module's call sites across `src/`; and a live `cargo check` whose 202
dead-code warnings were cross-tabulated by file to confirm which modules are actually
orphaned versus merely large. No `cargo test` run was performed in this pass (existing docs
report 263 passing tests as of 2026-07-27; re-verify before relying on that count given the
registry/dispatch changes since then).
