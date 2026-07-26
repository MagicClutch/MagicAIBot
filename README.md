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
