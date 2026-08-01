# Magic AI Bot — AI Context

## Purpose and stack

`magic_ai_bot` is a single Rust 2024 binary that logs an Azalea Minecraft Java client into a server, maintains an owned loaded-world snapshot, and exposes local-console controls.  The current working tree also has opt-in Groq (OpenAI-compatible) planning and `!` Minecraft-chat requests. Azalea is vendored at `vendor/azalea` (lockfile records revision `6249c295d353b9b3ef68f665b311cba39211fd19`; README says Minecraft 26.2/protocol 776). Runtime is a current-thread Tokio runtime on an 8 MiB spawned thread.

Important direct dependencies: Azalea/client/inventory (path dependencies), Tokio, Tokio-util cancellation, Serde/serde_json/TOML, Reqwest 0.12 with rustls, tracing, dotenvy, UUID, anyhow/thiserror. Toolchain: pinned `nightly` in `rust-toolchain.toml`.

## Entry points and map

- `src/main.rs::main`: creates larger-stack runtime then `App::initialize` / `App::run`.
- `src/app.rs::App`: composition root, select-loop, console dispatch, AI integration, application-level gather loop.
- `src/minecraft/client.rs::MinecraftClient`: Azalea connect/auth/reconnect, ECS refresh, navigation/chat packet adapter.
- `src/minecraft/events.rs::handle_chat`: packet -> `WorldState`; uses `ChatPacket::content()` (raw player body), not rendered `message()`.
- `src/minecraft/world_state.rs`: bounded snapshots for bot, inventory, players, entities/items, movement, last sent/received chat.
- `src/console/commands.rs`: local `/` parser and command enum; `src/console/mod.rs` reads stdin.
- `src/ai/mod.rs`: AI action types, session models, validation; `src/ai/provider.rs` is the `AiProvider` trait; `src/ai/groq.rs` is the Groq HTTP implementation; `src/ai/registry.rs` is the Groq tool registry.
- `src/movement`, `src/navigation`, `src/look`, `src/interaction`, `src/blocks`: movement/pathfinding, approach positions, look and confirmed break/place.
- `src/collection`, `src/tree_chopping`, `src/food`, `src/container`, `src/crafting`, `src/inventory`, `src/smelting`, `src/tasks`: feature services; several are mocks/adapters or not live-wired.
- `docs/ARCHITECTURE.md`, `docs/gemini.md` (rewritten for Groq), `docs/furnace-execution.md`, README: pre-existing guidance; this document supersedes their stale capability claims only where current source conflicts.

## Configuration and run

Working directory must contain `config.toml`; load path is literal `Path::new("config.toml")`. Start from `config.toml.example`; dotenvy loads a `.env` if present. Required top-level TOML sections are `[minecraft]`, `[reconnect]`, and `[logging]`; others default in `src/config.rs::Config`.

```powershell
cargo build
cargo run
cargo check
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features
```

Set `[minecraft] server`, `username`, and `account_mode = "offline"|"microsoft"`; configure an offline test server or Microsoft device login. `[logging] level` plus `debug` controls tracing. Groq: `[groq] enabled`, model/base_url/timeout/retries/temperature and limits. API key is read from `GROQ_API_KEY` environment variable (never log or commit keys). The deprecated `[gemini]` section is preserved for backward compatibility.

`[ai] busy_behavior="reject"`. `[ai.chat]`: `enabled=true`, `prefix="!"`, `respond_in_chat=true`, `accept_console_ai_command=true`, `acknowledge_requests=true`, `strip_prefix_whitespace=true`, `max_request_length=500`, `incoming_queue_capacity=64`; access allow/block lists and a per-player rate limit live below it. Response and console flags are enforced.

## Runtime rules

`App::run` connects, spawns local stdin input, then selects Ctrl-C, input, and movement/look/interaction/collection ticks. On disconnect it cancels interaction/block navigation/look/movement and marks tasks disconnected. Shutdown repeats cancellation, disconnects, and awaits console reader.

Azalea events update world state: spawn, player add/update/remove, remove entity packets, tick/ECS entity/inventory/player refresh, chat, disconnect and connection failure. There is no application event queue for individual block updates; state is refreshed on `Event::Tick`.

Player chat `!hi`, `!follow me`, `!ai hi`, and `!ai follow me` are processed on the movement tick. `chat_request` strips exactly the configured prefix then an optional exact `ai ` / `AI ` token; raw `!ai follow me` becomes `follow me`. Sender name/UUID from `ChatPacket` forms trusted `AiRequester`; only the raw body is prefix tested. Bot echo suppression prefers UUID, then name. System/action-bar chat never reaches AI.

Chat AI processes a bounded FIFO player-chat queue, applies case-insensitive allow/block lists and a trusted UUID-or-name rate-limit key before Gemini, and rejects while an AI session is active. `operators_only` conservatively rejects because no trusted Azalea operator adapter exists. `!stop`/`!cancel` invokes cancellation.

## Gemini contract

`GeminiClient::plan` POSTs to `https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent`, header `x-goog-api-key`, `responseMimeType: application/json`, an advisory schema, configured timeout, and `0..=max_request_retries` attempts (no backoff/rate limiter). Context includes request/requester, bot summary, inventory, active task, nearby players/entities and the registry. Nearby blocks config exists but is not included.

`GeminiPlan` fields are all optional/default except the schema asks Gemini for `session_complete`: `message_to_player`, `requester`, `objective`, `next_action`, `session_complete`. Parser accepts raw JSON or one outer ```json fence; rejects an `actions` array. `AiAction::Finish { summary: Option<String> }` means missing `summary` is accepted; the old `missing field summary` issue is fixed in current source.

Valid minimal replies:

```json
{"message_to_player":"I’ll follow you.","next_action":{"type":"follow_player","player":"5cat"},"session_complete":false}
```

```json
{"message_to_player":"Hi 5cat!","next_action":null,"session_complete":true}
```

```json
{"message_to_player":"Done.","next_action":{"type":"finish"},"session_complete":true}
```

Only `get_status`, `get_inventory`, `follow_player`, `goto`, `gather`, `stop`, `stop_all`, `finish` are registered/valid for Gemini. Runtime actually implements follow/goto/gather/stop/stop_all/finish; get-status/get-inventory pass validation but fall into “not available”. The broader `AiAction` enum is not a capability list.

Sessions (`AiSession`) have `Interpreting`, `Planning` (currently unused), `Executing`, `WaitingForTask`, `Replanning`, terminal statuses. One `active_action` is enforced per `AiSession`; `AiExecutionId` protects `complete_action` from stale IDs. `App` stores only one optional session, but console `start_ai` overwrites it without a busy check: unconfirmed concurrency/stale-work risk. `max_session_seconds`, replanning limit, `generation`, and `pending_action` are model fields/config but are not enforced by `App`.

## Commands and capabilities

All `src/console/commands.rs::ConsoleCommand` variants are local-console only. Key supported/currently dispatched commands: `/status`, `/players`, `/inventory`, `/entities`, `/chat`; `/goto`, `/goto-mine`, `/follow`, `/movement`, `/path-status`, `/stop`; block search/navigation; look; `/break`, `/breaknearest`, `/place`, `/select-tool`, interaction status/stop; task/gather; `/collect-item`; `/mine-ore`; `/chop-tree`; chest open/take/store/status/close; recipe/craft-check and limited craft controls; food and ensure-tool controls; `/reconnect`, `/quit`; `/ai`, `/aicancel`, `/aistatus`.

Cancellation is scoped: `/stop` movement, `/stopinteraction` interaction, `/lookstop` look, `/stopall` broad controllers/tasks/AI, and feature-specific `stop` forms. Console and Gemini registries are separate sources of truth.

Gather implementation is `App::{start_gather,tick_gather}`: scan loaded matching blocks, exclude `ignored_targets`, prefer current tree/nearest/low, call `InteractionController::break_at`, wait for block completion and visible drop, then `DropCollector`; inventory delta completes. 300-second total limit; `MAX_FAILED_TARGETS` (16) aborts when all nearby blocks are unreachable. `finalize_ai_task()` is the central cleanup path called from all terminal results (success, failure, timeout, cancellation), clearing gather state, collector, interaction (releasing look), block navigation, and movement atomically.

Block navigation’s safe interaction position is from `navigation::approach_position::{approach_positions,is_valid_approach}` and needs loaded valid foot/head/floor cells plus interaction distance. `BlockNavigationService` remembers failed blocks and `(target, approach)` only for its current generation; no global memory.

Movement follows known loaded `PlayerSnapshot` positions, refreshes its goal on `movement.repath_interval_ms`, and uses `follow_distance`. Losing player makes movement idle. AI follow never terminally completes unless a later explicit completion/verification occurs; `verify_objective(Follow)` accepts a current matching movement target, not distance/duration.

## Known issues / safety

1. **Medium, fixed:** enabled Groq registry now contains 14 runtime-executable actions; 7 `AiAction` variants remain intentionally unregistered (craft, collect_food, find_player, open_container, take_item, store_item, wait).
2. **Medium, confirmed:** planner requester output is ignored; trusted chat requester and local `follow me` binding are authoritative.
3. **Medium, confirmed:** allow/block lists and rate limits exist, but UUID allowlist configuration and reliable operator-state integration are not implemented.
4. **Medium, confirmed:** `include_nearby_blocks`, replan limit, and some session-limit fields remain partially implemented; do not assume them as safety controls. `allow_block_placement` is now `true` by default and a `place_block` action is registered in the Groq registry.
5. **Medium, confirmed:** message-to-player is always public when nonempty; sent chat failures are discarded; no split/max-length handling beyond client rejection; no acknowledgement on accepted chat request.
6. **Low, fixed:** gather now has a `MAX_FAILED_TARGETS` (16) guard to abort when all nearby blocks are unreachable, and `finalize_ai_task()` properly clears all state on terminal results.
7. **Medium, confirmed:** the AI request loop sends sequential requests to Groq until `finish` tool is returned or limits are reached.
8. **Medium, confirmed:** direct console `/ai` is accepted even if `accept_console_ai_command=false`; it can replace existing `ai_session` while physical work continues.
9. **Low:** current `cargo clippy --all-targets --all-features` succeeds with warnings (dead-code/unwired adapter warnings in inventory/processing/smelting and style warnings in pre-existing code). Code in our modified files passes cleanly.

## Test/validation snapshot (2026-07-27)

`cargo fmt --check` pending, `cargo check` passed, `cargo test` passed (263 passed, 0 failed), `cargo clippy --all-targets --all-features` pending. Tests are deterministic unit/mock tests; no live Minecraft or Groq integration server fixture exists.

## Information that must be re-verified

- Live Azalea compatibility, server/protocol version behavior, chat sender fields, menu revision/click behavior, navigation and interaction success on the target server.
- Whether local `GROQ_API_KEY` environment variable and endpoint/model access are valid; never paste its key into prompts/logs.
- The uncommitted working-tree AI changes (`src/ai/*`, `app.rs`, config, etc.) are not necessarily present on another branch/clone.
- README capability claims conflict with source in places; consult symbols above and `docs/PROJECT_KNOWLEDGE.md` for the audited snapshot.
