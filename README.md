# Magic AI Bot

Headless Minecraft AI bot foundation written in Rust, with Azalea planned as the
Minecraft client framework. This stage provides configuration, logging, graceful
shutdown, and module boundaries only; it does not connect to a server or perform
gameplay actions.

## Prerequisites

- Rust toolchain (the current Azalea repository may require Rust nightly; follow
  its repository guidance if stable compilation is not available)
- A Minecraft Java server is only required once Minecraft integration is enabled

## Configuration

Copy `config.toml.example` to `config.toml` and adjust it for the local server.
Do not commit credentials or other secrets.

Set `account_mode = "offline"` for an offline server. For Microsoft
authentication, set it to `"microsoft"` and use the Microsoft account identifier
as `username`; Azalea's device-code flow stores its token cache in the ignored
`auth-cache/` directory. Reconnect behavior is controlled by the `[reconnect]`
section.

## Build and run

```bash
cargo build
cargo run
```

The application waits for Ctrl+C and then shuts down gracefully.

## Console commands

With the console enabled, use `/help`, `/status`, `/chat <message>`, `/players`,
`/reconnect`, and `/quit`. Plain terminal input is sent as Minecraft chat when
`send_plain_input_to_chat` is enabled.

Manual smoke-test sequence:

```text
/help
/status
/chat Hello from Magic AI Bot
hello from plain console input
/reconnect
/quit
```

## Project structure

```text
src/
├── main.rs       # Tokio entry point
├── app.rs        # Application composition and lifecycle
├── config.rs     # TOML configuration and logging setup
├── error.rs      # Unified application errors
├── minecraft/    # Azalea connection and lifecycle integration
├── console/      # Future operator commands
├── movement/     # Future movement service
├── skills/       # Future skill services
├── tasks/        # Future task orchestration
└── ai/           # Future AI decision-making
```

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
