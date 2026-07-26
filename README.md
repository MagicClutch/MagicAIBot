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

## Chest interaction primitive

The container state machine currently supports normal and trapped chests at exact loaded block positions. It reuses block navigation and precise look control, sends an explicit Azalea block interaction, accepts only authoritative `Generic9x3` (single) and `Generic9x6` (server-supported double chest) menus, and serializes pickup clicks while checking the menu window and server state revision. Exact and allowed-partial take/store requests are available through `/take-item` and `/store-item`; `/open-chest`, `/container-status`, and `/close-container` provide manual diagnostics.

This is intentionally a low-level chest adapter. It does not index or sort storage and does not support crafting, furnace processing, shulker boxes, or other exotic menus. Live server compatibility still depends on the pinned Azalea menu state ID updates and should be exercised against the Task 11 integration server before higher-level storage behavior is built.
