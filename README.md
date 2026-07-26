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
