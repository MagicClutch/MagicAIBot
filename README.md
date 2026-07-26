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
├── minecraft/    # Azalea connection and lifecycle integration
├── console/      # Future operator commands
├── movement/     # Future movement service
├── skills/       # Future skill services
├── tasks/        # Future task orchestration
└── ai/           # Future AI decision-making
```
