# Magic AI Bot: Technical Project Knowledge

> Audit scope: current working tree, inspected 2026-07-26. “Confirmed” means supported by the checked source/configuration; “Inferred” follows directly from control flow; “Unconfirmed” needs a real server/provider run. The working tree was already dirty before this documentation task (including AI/chat changes); only these two documentation files are added.

## 1. Project overview

**Confirmed:** `magic_ai_bot` is a single-package Rust 2024 headless Minecraft Java bot. It uses a vendored Azalea client, stores an application-owned snapshot of the loaded world, and offers typed local-console operations for movement, block interactions, collection, tree chopping, and selected container/crafting diagnostics. The active working tree additionally has opt-in Groq (OpenAI-compatible) REST planning and Minecraft-chat requests.

For a first-time reader: `src/app.rs::App` wires Azalea's low-level client into small feature services. Azalea events refresh `WorldState`; a Tokio event loop ticks controllers; console commands or a Groq plan initiate typed work. It is an evolving bot rather than a finished autonomous agent: APIs and deterministic/mocked state machines are more complete than several live-server integrations.

**Confirmed stack/environment:** Rust nightly (`rust-toolchain.toml`), Tokio current-thread runtime, Azalea path dependencies from `vendor/azalea`, Reqwest REST to Groq API, Serde/TOML configuration, tracing. `Cargo.lock` pins Azalea revision `6249c295d353b9b3ef68f665b311cba39211fd19`; README records Minecraft 26.2/protocol 776. Expected runtime is a machine with a reachable Java server, writable local `auth-cache/` for Microsoft mode, and a working directory containing `config.toml`.

**Status:** partially implemented. Live primitives exist for connection, snapshots, movement/pathfinding, look, intentional interactions, dropped-item collection, tree chopping, and a chest adapter. Recipe/smelting/inventory/crafting modules include substantial pure or mock-tested behavior but have explicitly limited live adapters. AI is newly implemented, allowlisted, and not authorization-hardened.

## 2. Repository map

```text
Cargo.toml / Cargo.lock             Single binary manifest and locked dependencies.
rust-toolchain.toml                 `nightly`, minimal profile.
config.toml.example                 Safe configuration template; local config is ignored.
README.md                           User-facing status; contains some stale statements.
docs/ARCHITECTURE.md                Ownership/lifecycle audit and limitations.
docs/gemini.md                      (Rewritten for Groq setup.)
docs/furnace-execution.md           Furnace executor boundary.
Prompts/                            Historical staged requirements, not runtime code.
vendor/azalea/                      Vendored Minecraft client/protocol implementation.
src/main.rs                         Runtime bootstrap.
src/app.rs                          Composition root, command dispatch, AI and gather glue.
src/config.rs                       TOML config/defaults/validation and logging initialization.
src/minecraft/                      Azalea client/event translation/world snapshots/container adapters.
src/console/                        Terminal line reader and typed slash-command parser.
src/ai/                             Groq provider, tool-call validation, session state, command registry.
src/movement/, navigation/          Azalea goal control and safe block approach selection.
src/blocks/                         Loaded-world search and ore safety/selection.
src/look/, interaction/             Rotation/look and verified break/place/tool selection.
src/collection/, food/              Item/drop and food selection controllers.
src/tree_chopping.rs                Conservative loaded-tree detection/execution.
src/tasks/                          Generic runtime/task lifecycle and mocked gather/tool planner.
src/inventory/, container/, crafting/, smelting/, processing/
                                  Inventory/menu, container, recipe/crafting/furnace abstractions.
src/inventory_cleanup.rs            Policy-driven cleanup state machine.
```

## 3–4. Build, run, and configuration reference

Run from repository root:

```powershell
cargo build
cargo run
cargo check
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features
```

`Config::load(Path::new("config.toml"))` makes root `config.toml` mandatory; `.env` is optionally loaded first by `dotenvy::dotenv()`. Copy the example rather than exposing secrets. Debug logging is `[logging] level = "debug"` and/or `debug = true` (see `src/config.rs::init_logging`). Start with an offline test server using `[minecraft] account_mode="offline"`, then `/help` in local stdin; Microsoft mode invokes Azalea device login and writes `auth-cache/azalea-auth.json`.

### Config fields

The definitive types/defaults are `src/config.rs`; `config.toml.example` is the operationally complete example. All optional sections below use `#[serde(default)]`; `[minecraft]`, `[reconnect]`, `[logging]` are required. “Used” means a current source reference, not necessarily a complete live capability.

| Section / fields | Type/default | Used by / notes |
|---|---|---|
| `minecraft.server`, `username`, `account_mode` | string, string, `offline|microsoft`; required | `MinecraftClient::connect`; server/account data are sensitive operational data. |
| `reconnect.enabled`, `delay_seconds`, `maximum_attempts` | bool/10/5 | `MinecraftClient` supervisor; validation rejects enabled with zero attempts. |
| `console.enabled`, `send_plain_input_to_chat`, `show_system_messages`, `show_action_bar_messages` | bool defaults; false for plain forwarding/action bars | starts stdin; filters display only. |
| `logging.level`, `debug` | string/bool; required section | tracing filter. |
| `world_state.nearby_entity_radius`, `maximum_tracked_entities`, `stale_entity_seconds` | 64/256/30 | `WorldState` bounds. |
| `movement.follow_distance`, `repath_interval_ms`, `arrival_distance` | 3.0/500/1.5 | `MovementService`. |
| `multitasking.*` | forward 35°, strafe 75°, backward 130°, extreme 160° | camera-relative local-input suggestion. |
| `block_search.*` | radius 32, max 128, limit 20/max 256, vertical 32 | loaded-block search. |
| `block_navigation.*` | radius 32/max 128, candidates 20, reach 4.5, arrival 1.5, attempts 10, stuck 12s, max 120s, repath 1000ms | safe approach/navigation. |
| `look.*`, `look.randomization.*`, `look.motion.*` | see example/default functions | `LookController` timing, prediction, seeded variation and rotation bounds. |
| `interaction.*`, `interaction.face_targeting.*` | reach 4.5; retry 3; verify 1500ms; auto navigation/look/tool true; safety lists empty | `InteractionController`, tool policy, face selection. |
| `vertical_navigation.*` | disabled by default | **Partially implemented/unconfirmed use:** no `config.vertical_navigation` consumer was found in `src/`. |
| `tree_chopping.*` | configured tree list, radii/topology/time limits | `TreeChopService`. |
| `smelting.*` | radius 32, poll 250ms, confirmation 3s, total 300s, reopen 2 | data/executor config; `station_search_radius` produces dead-code warning. |
| `inventory_cleanup.*` | protections true, rules [] | cleanup policy module; no App command dispatch found. |
| `ensure_tool.*` | tiers iron/stone/wood, reserve 10, smelting false | tool-planning request. |

Gemini and chat configuration:

```toml
[gemini]
enabled = true
model = "gemini-2.5-flash"
api_key = "YOUR_GEMINI_API_KEY" # direct key takes priority; do not commit
api_key_env = "GEMINI_API_KEY"  # fallback only if api_key missing/blank
request_timeout_seconds = 30
max_request_retries = 2
max_steps_per_session = 30
max_session_seconds = 600
temperature = 0.1
include_nearby_blocks = true
include_nearby_entities = true
include_inventory = true

[ai.chat]
enabled = true
prefix = "!"
respond_in_chat = true
accept_console_ai_command = true
strip_prefix_whitespace = true
max_request_length = 500
```

`GroqConfig::resolve_api_key` uses environment named by `api_key_env` (fallback `GROQ_API_KEY`), otherwise fails without including a value. `Debug` redacts env var values. `[groq.limits]` defaults: quantities 64, navigation 256, action/session 30, replans 8, session seconds 600, mining/crafting/containers/placement allowed. Validation uses gather/mine/craft/navigation/container/wait/place_block bounds and the Groq command registry; **confirmed unused/partial fields:** `include_nearby_blocks`, `max_replans_per_session`, both session second limits, and `respond_in_chat`/`accept_console_ai_command`.

For exact key coverage, the following is the field inventory from the checked template (numbers/booleans are TOML `integer`/`float`/`boolean`, lists are arrays of strings unless apparent otherwise). Defaults are the values in `config.toml.example`; missing optional keys use the corresponding `default_*` in `config.rs`.

```text
[minecraft] server, username, account_mode
[reconnect] enabled, delay_seconds, maximum_attempts
[console] enabled, send_plain_input_to_chat, show_system_messages, show_action_bar_messages
[logging] level, debug
[groq] enabled, api_key_env, model, base_url, request_timeout_seconds,
        max_request_retries, temperature, include_nearby_blocks,
        include_nearby_entities, include_inventory
[groq.limits] max_gather_quantity, max_mine_quantity, max_craft_quantity,
              max_navigation_distance, max_actions_per_session,
              max_replans_per_session, max_session_seconds, allow_mining,
              allow_crafting, allow_containers, allow_block_placement
[ai.chat] enabled, prefix, respond_in_chat, accept_console_ai_command,
           strip_prefix_whitespace, max_request_length
[world_state] nearby_entity_radius, maximum_tracked_entities, stale_entity_seconds
[movement] follow_distance, repath_interval_ms, arrival_distance
[vertical_navigation] enabled, allow_pillaring, allow_digging_down,
                      prefer_staircase_descent, max_pillar_height, max_dig_depth,
                      minimum_building_blocks, allowed_building_blocks, denied_building_blocks
[multitasking] normal_forward_angle, strafe_angle, backward_angle, extreme_angle
[block_search] default_radius, maximum_radius, default_result_limit,
               maximum_result_limit, default_vertical_range
[block_navigation] default_search_radius, maximum_search_radius, candidate_limit,
                   interaction_distance, arrival_distance, maximum_target_attempts,
                   stuck_timeout_seconds, maximum_navigation_seconds, repath_interval_ms
[look] update_rate, reaction_delay_min_ms, reaction_delay_max_ms,
       moving_target_prediction, prediction_strength, minimum_target_movement
[look.randomization] enabled, block_randomization, entity_randomization,
                      player_randomization, horizontal_strength, vertical_strength,
                      minimum_hold_time_ms, maximum_hold_time_ms, retarget_chance_per_second
[look.motion] minimum_yaw_speed, maximum_yaw_speed, minimum_pitch_speed,
              maximum_pitch_speed, yaw_acceleration, pitch_acceleration,
              yaw_deceleration, pitch_deceleration, slowdown_angle, arrival_tolerance,
              micro_correction_enabled, micro_correction_strength, speed_variation,
              overshoot_chance, maximum_overshoot_degrees
[interaction] maximum_reach, placement_reach, breaking_reach, retry_limit,
              retry_delay_ms, verification_timeout_ms, auto_navigate,
              auto_precise_look, auto_tool_switch, minimum_tool_durability,
              allow_hand_fallback, held_tool_equivalence, protected_tools, reserved_tools
[interaction.face_targeting] face_inset, edge_margin, maximum_face_attempts,
                             maximum_hit_points_per_face
[smelting] station_search_radius, observation_interval_ms, confirmation_timeout_ms,
           total_timeout_seconds, reopen_limit
[inventory_cleanup] protect_rare_items, protect_tools, rules
[tree_chopping] allowed_tree_types, require_nearby_leaves, maximum_connected_logs,
                 maximum_tree_height, maximum_branch_distance, maximum_horizontal_logs,
                 break_leaves, collect_saplings, allow_hand_chopping, search_radius,
                 maximum_trees, total_timeout_seconds
[ensure_tool] material_tier_preference, durability_reserve, allow_smelting
```

The template values are also the confirmed default examples: movement `3.0/500/1.5`; search `32/128/20/256/32`; block navigation `32/128/20/4.5/1.5/10/12/120/1000`; interaction reach `4.5`, retry `3`, verification `1500`; and tree/gather operational bounds shown directly in the template. Configuration validation checks connection, limits, movement, block search/navigation, look, and related configuration; see configuration unit tests in `src/config.rs` for rejected boundary values.

Security scan: no direct API key is stored in GroqConfig; the key is resolved from the `GROQ_API_KEY` environment variable at provider creation time.

## 5. Startup and runtime lifecycle

```text
main.rs::main
→ new 8 MiB thread
→ Tokio current-thread runtime + LocalSet
→ App::initialize
  → dotenv load, Config::load("config.toml"), init_logging
  → construct MinecraftClient and feature services
→ App::run
  → MinecraftClient::connect / Azalea join/spawn
  → spawn_local(console::read_input) if enabled
  → select Ctrl-C, stdin, movement tick, look tick, interaction tick, collection tick
  → cancel controllers/tasks and MinecraftClient::disconnect on exit
```

Movement tick runs at `movement.repath_interval_ms`; look tick is `1000/look.update_rate`; interaction/navigation/gather/container/food/mining/tree ticks every 50 ms; active drop collector ticks every 100 ms. `finalize_ai_task()` is the central cleanup path for all AI terminal results (success, failure, timeout, cancellation), clearing gather, collector, interaction (releasing look), block navigation, and movement atomically. The Azalea client supervisor handles reconnect when enabled. Disconnect cleanup occurs once when App observes a state transition; no gameplay action is resumed automatically.

## 6. Minecraft events and chat flow

`MinecraftClient::wait_for_disconnect` consumes `Event::Spawn`, player add/update/remove, remove-entity packet, `Event::Tick`, `Event::Chat`, disconnect/failure. `refresh_ecs_state` queries Azalea ECS for local position/health/inventory/dimension, entities, item entities, and players, then updates `WorldState`; it is a snapshot refresh rather than per-block event processing. Inventory currently reports `revision: 0` in `inventory_from_component`, an integration limitation for stateful confirmation.

`events::handle_chat` classifies player/disguised, system, action-bar. `packet.content()` is raw player body (e.g. `!follow me`) and is saved as `ChatRecord.text`; `packet.message()` is rendered (e.g. `<5cat> !follow me`) and must not be parsed. Sender name and UUID are copied from the packet; formatted message is not stored. Bot echoes are suppressed by UUID when present, otherwise case-insensitive username; `WorldState::record_received_from` suppresses duplicates. System/action-bar display depends on console config and never enters AI.

## 7. Command system

`console::commands::parse_input` is the authoritative console syntax parser; `App::execute_console_input` dispatches it. The console is trusted local operator input by design. Table legend: C=console; G=Gemini registered; L=long/continuous; X=has cancellation. “Tested” means parser/unit/mock coverage, not live server behavior.

| Commands / aliases | C | G | L/X | Handler / limitation |
|---|---:|---:|---|---|
| `status`, `players`, `inventory`, `entities`, `movement`, `path-status` | yes | status/inventory registry only | no | `App::print_*`; Gemini status/inventory are **not runtime-executed**. |
| `goto`, `goto-mine`, `follow`, `stop|stopmovement`, `stopall` | yes | goto/follow/stop/stop_all | L/X | `MovementService`, `TaskService`; follow is continuous. |
| `findblock`, `nearestblock`, `gotoblock|navigate-to-block`, `gotoblockstatus`, `cancelgotoblock` | yes | no | L/X | `BlockSearchService`/`BlockNavigationService`. |
| `look|lookat`, `lookblock`, `lookplayer`, `lookentity`, `lookstop`, `lookstatus` | yes | no | L/X | `LookController`. |
| `breakblock`, `break`, `breaknearest`, `place`, `placeblock`, `select-tool`, `stopinteraction`, `interactionstatus` | yes | no | L/X | `InteractionController`; state transition verification. |
| `gather`, `gatherstatus`, `gathercancel`, `task*`, `tasks` | yes | `gather` | L/X | App gather + TaskService; item scans only loaded world. |
| `collect-item|collectdrop`, `mine-ore|mineore`, `collect-food`, `chop-tree|choptree`, feature `status|stop` forms | yes | no | L/X | respective services; behavior bounded, live coverage varies. |
| `recipe`, `craft-check`, `craft`, `ensure-tool|craft-tool`, `testoaklog` | yes | no | varies | recipe/craft adapters; README documents limits. |
| `open-chest`, `take-item`, `store-item`, `container-status`, `close-container` | yes | no | L/X | `ContainerService`; chest-focused adapter. |
| `chat`, `reconnect`, `quit`, `help` | yes | no | varies | direct client/lifecycle. |
| `ai`, `aicancel`, `aistatus` | yes | n/a | L/X | Groq integration; `accept_console_ai_command` is ignored. |

The Groq registry (`ai::registry::command_registry`) exposes 14 snake_case tool definitions and describes completion. `AiAction` has additional variants (craft/collect_food/find_player/open_container/take_item/store_item/wait); `validate_action` rejects them as unregistered. The registry is the single shared source of truth for both console and AI command definitions. Do not add an enum action without registry, validation, runtime, tests, and docs changes.

## 8–10. Groq architecture, tool calling, and sessions

```text
console / raw prefixed chat → AiRequest(trusted chat sender)
→ App::submit_ai_request snapshot/context → GroqProvider::chat (tool calls)
→ parse tool calls → validate_action allowlist/limits
→ AiSession::begin_action(UUID) → App::execute_ai_action
→ underlying controller + observation → complete_action(UUID) → verify_objective
```

`GroqProvider` uses Reqwest directly, endpoint `/chat/completions`, `Authorization: Bearer` header, OpenAI-compatible tool calling format, timeout, and retry count inclusive. It logs tool calls and text responses; it does retry with backoff on HTTP failures. API key never enters context/error, but arbitrary chat request/world names do.

The AI request loop:
1. Build context (system prompt with registry-generated tools + conversation history)
2. Call Groq chat completions API
3. Parse tool_calls from response
4. Convert each tool call to an `AiAction` via `tool_call_to_action()`
5. Validate against registry and limits
6. Execute action via `execute_ai_action()`
7. Repeat until `finish` tool is returned or limits reached

**Invalid/handled:** unknown tool name; absent action fields; unregistered action; invalid IDs/zero/excess quantities; out-of-range/nonfinite goto; disabled mining/craft/container.

`AiSessionStatus`: `Interpreting`, `Planning` (unused), `Executing`, `WaitingForTask`, `Replanning`, `Completed`, `Failed`, `Cancelled`. `AiActionStatus`: planned/validated/starting/running/waiting/verifying/succeeded/failed/cancelled/timed out. `begin_action` rejects an already active action; execution UUID comparison prevents a stale `complete_action`. Cancellation clears active/pending and increments session generation. **Confirmed gaps:** `App` holds one `Option<AiSession>` but console submissions can overwrite it; no queue; session duration/replan count/generation checks are not used in the App loop; only gather completions are wired to terminal action completion. AI actions do not implement a full replanning cycle.

## 11–12. Minecraft chat requests and responses

`App::tick_ai_chat` runs only while connected on movement ticks. It reads the sole latest `WorldStateSnapshot.last_received_chat`, dedupes its timestamp, requires Player kind and configured raw prefix, then creates `AiRequester { name, uuid, source: MinecraftChat }`. `chat_request` trims after one prefix if configured and strips optional `ai ` / `AI `:

```text
!hi → hi
!follow me → follow me
!ai hi → hi
!ai follow me → follow me
```

`me`, `my`, and `I` are **not programmatically resolved** in current code. The trusted requester is supplied to Gemini, but `plan.requester` can overwrite `session.requester`: confirmed security/identity bug. `!stop`/`!cancel` cancels current work. Bot messages are filtered before saving. No access control, requester UUID binding, per-player/provider rate limit, or queue exists.

`MinecraftClient::send_chat` validates nonempty and protocol maximum length, sends through Azalea, and records sent chat for duplicate tracking. `submit_ai_request` prints Groq response text, strips `/`, then unconditionally attempts public `[AI] {message}`; it ignores send errors, does not split long messages, and has no private mode/duplicate response guard. There is no accepted-request acknowledgement; Groq output is sent only if the response contains a message.

```text
Player: !hi          → Bot: [AI] Hi 5cat! What should I do?
Player: !follow me   → Bot: [AI] I’ll follow you.
```

## 13–16. Navigation, interaction, gathering, inventory/crafting, following

Azalea is the pathfinder. `MovementService::goto` submits a goal and tracks arrival by Euclidean `arrival_distance`; it refreshes goals at repath interval and has a two-second startup grace. `BlockNavigationService` generates cardinal/diagonal/overhead standing cells (`approach_positions`), requiring loaded/passable feet/head, solid support, existing target and interaction distance. It tries bounded candidates/approaches, records failures per navigation generation, checks target changes and 120s timeout. “no safe reachable interaction position” / `NoValidApproachPosition` means no candidate standing cell passes this preflight (or all paths/approaches fail), not necessarily that the target is globally unreachable.

`InteractionController` owns precise look, tool selection, break/place retries and observed block-state confirmation. `TreeChopService` is separate conservative tree topology detection. `App::tick_gather` is a glue collector, not `tasks::gather::gather`: scan matching loaded blocks → retain nonignored → current-tree/nearest sort → `break_at` → wait interaction completion → wait up to 3s for item entity → `DropCollector` → inventory delta. It has 300s overall timeout. A `MAX_FAILED_TARGETS` (16) guard aborts gather when all nearby blocks are unreachable. Unreachable/failed target positions enter `ignored_targets`, but any successful pickup clears that entire set. When the AI session completes, fails, or times out, `finalize_ai_task()` clears gather state, collector, interaction, navigation, and movement atomically.

Inventory is `WorldState::InventorySnapshot`; hotbar selection exists. `crafting`, `container`, `smelting`, `processing`, `inventory`, and `tasks` contain typed/mocked state machines, recipe fallback data, and many tests. Treat a feature as live only if dispatched in `App` and backed by client adapter. README explicitly limits recipe knowledge and menu protocol capture; the current client's inventory snapshot revision is hard-coded 0. Chests support observed Generic9x3/9x6 flow; generic arbitrary containers and complete live crafting/furnace integration require server testing.

Follow finds `WorldState::find_player_by_name`, requires a loaded position, follows with configured distance, and updates each tick. A missing player clears following and goes idle. Dimension is stored in snapshots but `MovementService::follow` does not compare bot/player dimensions before goal submission. AI follow is continuous and has no natural terminal completion; objective verification only checks current `movement.target_player` name.

## 17–18. Diagnostics and recovery

Logging helpers in `src/logging.rs` yield normal levels and prefixes including `[AI]`, `[CHAT]`, `[SYSTEM]`, `[ACTIONBAR]`; most feature diagnostics use `logging::{info,warning,success}`. No formal `[ERROR]` taxonomy exists.

| Message | Likely cause / code | Check |
|---|---|---|
| `Groq API key is missing or unavailable` | config disabled or missing `GROQ_API_KEY` env var; `GroqProvider::from_config` | set `GROQ_API_KEY` environment variable. |
| `Target unreachable, ignoring` | `break_at`/interaction failure in `App::tick_gather` | block approach, loaded chunks, route, tool, target changes. |
| `no reachable approach position` | `BlockNavigationService` preflight exhausted | inspect feet/head/floor and target geometry. |
| `Dropped item did not appear` | no matching item entity within 3s | verify server drops/entity tracking/item mapping. |
| `Current tree exhausted...` | gather changes tree component | inspect loaded candidates; note ignored target reset. |
| `A previous action is still running` | session active action invariant | cancel/status; console overwrite remains a bug. |

Missing config/invalid values return `AppError`; connection failure ends `App::run`; reconnect uses configured finite attempts. Chat send rejects empty/too-long/disconnected. Controller failure is usually held in snapshot/status and logged. Many `let _ =` cancellation/send calls intentionally swallow cleanup errors; sending Gemini response specifically ignores errors. There is no panic recovery beyond runtime-thread join reporting `runtime thread panicked`; source `expect`/`unwrap` occurrences are mainly tests plus poisoned standard mutex expectations in task lifecycle services.

## 19. Prioritized bugs and suspicious behavior

### Critical

None confirmed as an unconditional corruption/crash in the inspected code. A direct local Gemini key exists; exposure risk is security-critical if its value left the machine.

### High

1. **AI registry/runtime match (fixed).** `get_status`, `get_inventory`, `chop_tree`, `collect_item`, `mine_ore`, `look_at_player`, and `goto_block` are now registered and executable in `execute_ai_action`. The registry and runtime are aligned; `craft`, `collect_food`, `find_player`, `open_container`, `take_item`, `store_item`, and `wait` remain unregistered.
2. **Untrusted response can replace trusted requester.** `App::submit_ai_request` overwrites `session.requester` from response context. Security-sensitive, low code risk.
3. **No authorization or rate limit for chat AI.** `tick_ai_chat` accepts every non-bot player with prefix and can call a paid external API / move/break blocks through gather. Fix policy/UUID allowlist, budgets, cooldown, audit tests before production use.
4. **Controls advertised by config do not control behavior.** `respond_in_chat`, `accept_console_ai_command`, timing/replan fields and `include_nearby_blocks` are partly/unused. `allow_block_placement` defaults to `true` and gates `place_block` Gemini validation. Fix or remove/document remaining unused controls; test config behavior.

### Medium

5. **Gather retry memory is cleared after progress.** `App::tick_gather` `ignored_targets.clear()` after an inventory delta; a prior unreachable target can be selected again. Reproduce with one reachable and one permanently blocked matching block. Retain per-task failed positions/tree generation or use bounded failure count; add integration/mock selection test.
6. **Session sequencing is incomplete.** Session/replan limits and timeout are not enforced, non-action plans do not replan, only gather completion feeds action completion. `follow` has no terminal lifecycle. Implement explicit action watchers/state transitions and tests.
7. **Console AI can overwrite session.** `start_ai` does not honor `accept_console_ai_command` or active session. Reproduce two `/ai` inputs while gather/follow runs. Reject/queue/cancel explicitly and preserve task ownership.
8. **Chat transport behavior is incomplete.** `send_chat` error ignored for AI response; no length split/ack/private modes. Respect config, report failed delivery locally, chunk messages safely.

### Low / unconfirmed

9. **Burst chat loss (confirmed design risk):** one `last_received_chat` slot plus movement-tick polling can drop intermediate messages. Use an owned bounded request queue if requirements demand it.
10. **Cross-dimension follow (unconfirmed live failure):** dimension is not checked by `MovementService::follow`; Azalea may reject or path indefinitely. Add a same-dimension guard and live test.
11. **README/documentation drift (confirmed):** README says chat/AI unsupported in an older table, while current source wires both. Keep this audit and README aligned when merging current changes.

## 20–21. Security and concurrency

Secrets: direct config wins, env fallback is supported, debug formatting redacts API key. Never commit `.env`, ignored `config.toml`, `auth-cache/`, or response logs containing sensitive player text. Prompt injection: Minecraft chat, player names, entities, and inventory strings are untrusted text sent in Gemini context. The typed registry/limits reduce command injection, but planner-visible context can manipulate messaging and `requester` field currently has an identity flaw. Console commands are arbitrary trusted-local operator control; Minecraft chat has no authorization. Retry loops are finite (Gemini request retry, navigation candidate limits, gather 300s), but chat requests are unthrottled and API retries lack backoff.

Tokio is single-thread current-thread + LocalSet for App/Azalea; `MinecraftClient` has async mutexes around client/world state; services use async mutex state. `TaskService`/lifecycle uses standard `Mutex` and UUID ownership. `ResourceLeases` (`tasks/runtime.rs`) asynchronously acquires canonical Movement→Look→Interaction→Inventory→Container locks; `ResourceManager` (`tasks/lifecycle.rs`) atomically leases a separate resource enum. **Confirmed duplication:** these two independent ownership systems are not globally integrated into all direct App controllers, so their single-action invariants are not a process-wide guarantee. Event/task generation and `AiExecutionId` guard some stale completions, but not all controller interactions.

## 22. Test coverage

`cargo test` runs 263 tests: config parsing/validation/key precedence; console parser; AI provider/tool-call/session/validation/chat prefix; world snapshots/event self-filter; movement/navigation approach/arrival; block search/ore selection; look/interactions/tool policy; collector/tree/food; task/lifecycle leases; recipe/crafting/container/inventory/smelting/processing mocks.

| Area | Coverage | Missing important coverage |
|---|---|---|
| configuration | strong unit | unused field behavior / real config file env integration |
| chat parsing | AI function + bot filter | `App::tick_ai_chat`, bursts, authorization, response switches |
| Groq parsing/validation | strong unit | HTTP error/backoff/schema-provider integration |
| AI session | basic UUID/validation | App sequencing, overwrites, timeout/replan/follow watchers |
| movement/navigation | unit algorithms | live Azalea path server |
| gather | App tree component + generic mock gather | actual App unreachable-memory regression/live drops |
| inventory/craft/container/smelt | extensive mocks | real menu packet/revision clicks |
| following | snapshot helper | loss/dimension/live follow |
| error handling | many unit paths | disconnect/reconnect integration |

Recommended tests: `app::tests::chat_ai_respects_respond_and_console_flags`; `app::tests::chat_requester_cannot_be_overwritten_by_plan`; `app::tests::gather_keeps_unreachable_target_ignored_after_progress`; `app::tests::second_console_ai_is_rejected`; AI action registry/runtime completeness test; local-server smoke tests for chat, follow, break/gather, chest and reconnect.

## 23–24. Dependency and data-structure reference

Azalea (`path vendor/azalea`, default features off; serde/packet-event/online-mode) provides protocol, client ECS, navigation, auth and chat. Its pinned source is a compatibility surface. Tokio drives I/O/timers/local tasks; Reqwest JSON+rustls is Gemini REST; Serde/serde_json/TOML parse config/provider payloads; Tokio-util tokens cancel work; UUID gives session/action/correlation identifiers. No official Gemini SDK is used. There is no separate pathfinding crate: Azalea owns live paths; app computes candidate geometry.

Important models:

| Type | File | Responsibility |
|---|---|---|
| `Config`, `GroqConfig`, `AiChatConfig`, feature configs | `config.rs` | deserialize/default/validate policy. |
| `App`, `ActiveGather` | `app.rs` | composition/root and application gather state. |
| `WorldState`, `WorldStateSnapshot`, `ChatRecord` | `minecraft/world_state.rs` | bounded authoritative application observations. |
| `AiRequest`, `AiRequester`, `AiAction`, `AiObjective` | `ai/mod.rs` | request types and typed action enum. |
| `AiProvider` (trait), `AiRequest`, `AiResponse`, `ToolCall`, `ToolDefinition` | `ai/provider.rs` | abstract AI provider interface. |
| `GroqProvider` | `ai/groq.rs` | Groq HTTP client implementing `AiProvider`. |
| `AiSession`, `ActiveAiAction`, statuses | `ai/mod.rs` | one session/action history and UUID completion guard. |
| `ConsoleCommand` | `console/commands.rs` | local syntax AST. |
| `MovementSnapshot`, `BlockNavigationSnapshot`, interaction/look states | `world_state.rs`, navigation/interaction/look | controller state. |
| `TaskService`, `TaskStatus`, `TaskId` | `tasks/mod.rs` | task leases/history/cancellation. |
| `OperationContext`, `ResourceLeases`; `OperationGuard`, `ResourceManager` | `tasks/runtime.rs`, `tasks/lifecycle.rs` | two ownership/cancellation systems. |

## 25. End-to-end examples

1. **Console `/ai give me some wood`:** parser → `start_ai` (if Groq enabled) → context → Groq returns tool call such as gather oak log → registry/limit validation → `start_gather` → break/drop/collector/inventory confirmation → `finish_ai_if_verified`. **Not implemented:** automatic repeated Groq next-action replanning.
2. **Chat `!give me some wood`:** Event Chat raw content → `WorldState.last_received_chat` → next movement tick `tick_ai_chat` → trusted sender request → same flow. It sends player-facing provider message if present. **Not implemented:** access control/rate limit/ack switch.
3. **Chat `!hi`:** same input → Groq may return text-only response (no tool calls, finish_reason=stop); App treats as conversation.
4. **Chat `!follow me`:** sender reaches Groq, but "me" is not deterministically substituted; planner must choose trusted name. `follow_player` starts movement and AI waits; it has no terminal follow completion. **Not implemented:** deterministic pronoun resolution and completion policy.
5. **Chat `!stop`:** parse before provider call → `cancel_ai` clears AI/gather/task/navigation/movement then public cancellation response. `finalize_ai_task()` is the central cleanup path that also handles successful gather completion and timeout.

## 26. Recommended repair order

1. `src/app.rs`, `src/ai/mod.rs`: preserve trusted chat requester; enforce chat authorization/rate/budget and console flag. Validate with AI/App tests. Security-sensitive.
2. `src/ai/registry.rs`, `src/app.rs`: **(done)** registry expanded to match executable actions; 14 tools registered, 7 unregistered remain.
3. `src/app.rs`: introduce explicit AI state/action completion watchers, request queue/busy policy, session/replan/timeout enforcement; cover gather/goto/follow/cancel races. Moderate behavior risk.
4. `src/app.rs`: honor `respond_in_chat`, acknowledge option if added, delivery failures and chunk limits; test chat output. Low risk.
5. `src/app.rs` gather + navigation: retain per-task unreachable target memory and finite failed-target policy; test reachable/unreachable mixed tree. Moderate gameplay impact.
6. Merge ownership: choose/integrate one resource/session lifecycle model across direct controllers, then exercise disconnect/reconnect live. High integration risk.
7. Add local-server integration fixtures before claiming live crafting/container/furnace or protocol reliability. Validate `cargo test` plus server matrix.

## 27. Validation results

Executed against the current dirty working tree before docs were added:

| Command | Result |
|---|---|
| `cargo fmt --check` | passed |
| `cargo check` | passed; warnings reported |
| `cargo test` | passed: 263 passed, 0 failed |
| `cargo clippy --all-targets --all-features` | passed; warnings (dead-code/unwired modules and style suggestions in pre-existing code) |

No live Minecraft server or Groq request was run, so those integrations remain unconfirmed.
