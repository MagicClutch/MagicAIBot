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
# Tool-selection debug command

`/select-tool <block_id>` runs the same deterministic policy used by
intentional block breaking, prints its explanation, and selects the winning
hotbar slot. It is a debug command and does not break the block. The selector
never crafts, repairs, or enchants tools. Azalea pathfinder mining remains
independent and unchanged.

## Known Azalea limitations

## Project structure

Out of scope: AI planning, persistence, autonomous survival/exploration/combat, advanced building,
X-ray, reinforcement learning, and unrelated dependency upgrades.
