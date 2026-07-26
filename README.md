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
