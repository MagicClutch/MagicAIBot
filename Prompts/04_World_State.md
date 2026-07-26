# World State

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect the existing project and all code from previous prompts.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement movement, pathfinding, mining, placement, inventory automation, AI planning, or future gameplay features.
6. Reuse existing configuration, logging, event, client, chat, console, and error systems.
7. Never create duplicate systems.
8. Keep the implementation asynchronous, modular, testable, and production-ready.

---

# Goal

Implement a centralized, read-only WorldState system that maintains the bot's current understanding of itself and the loaded Minecraft world.

This system will become the shared source of truth for future navigation, mining, combat, inventory, planning, and task modules.

Do not implement autonomous behavior yet.

---

# Core Requirements

Create a reusable \`WorldState\` service that tracks at least:

- connection state
- bot UUID
- bot username
- bot entity ID
- current position
- previous position
- velocity when available
- yaw
- pitch
- current dimension
- health
- absorption
- hunger
- saturation when available
- experience level
- game mode
- whether the bot is alive
- whether the bot is on the ground
- whether the bot is in water
- whether the bot is in lava
- whether the bot is sneaking
- whether the bot is sprinting
- currently selected hotbar slot
- loaded entities
- loaded players
- loaded chunks
- known block information from loaded chunks

Use Azalea's existing state and event APIs where practical. Do not duplicate Azalea's entire internal ECS or world representation.

---

# State Model

Separate state into clear structures, such as:

- \`PlayerState\`
- \`EnvironmentState\`
- \`EntityState\`
- \`ConnectionState\`
- \`WorldSnapshot\`

Use strongly typed coordinates and identifiers where practical.

Avoid loosely structured maps when a typed model is more appropriate.

---

# Snapshots

Provide an immutable snapshot API.

Future modules should be able to request a consistent snapshot without directly mutating state.

Example responsibilities:

- \`snapshot()\`
- \`player_state()\`
- \`current_position()\`
- \`current_dimension()\`
- \`nearby_entities(radius)\`
- \`nearby_players(radius)\`
- \`block_at(position)\`
- \`is_chunk_loaded(position)\`

Use names appropriate to the existing architecture.

---

# Synchronization

The bot will be asynchronous.

Use safe synchronization primitives.

Requirements:

- no blocking mutex held across \`.await\`
- no unnecessary copying of large world data
- no global mutable state
- concurrent readers should be supported efficiently
- state updates must not create race conditions

---

# Events

Update WorldState from the existing connection and Minecraft event systems.

Handle at least:

- spawn
- respawn
- movement updates
- rotation updates
- health updates
- dimension changes
- death
- entity spawn
- entity movement
- entity removal
- chunk load
- chunk unload
- block update
- disconnect

Do not create a second unrelated event bus.

---

# Staleness

Track update timestamps or ticks where useful.

Future systems must be able to detect stale information.

Do not invent world information when chunks are not loaded.

Unknown state must remain explicitly unknown.

---

# Console Status

Extend the existing status output so it can display a concise snapshot containing:

- connection state
- position
- dimension
- health
- hunger
- loaded entity count
- loaded chunk count

Do not add autonomous commands.

---

# Error Handling

The state system must tolerate:

- missing player state during login
- incomplete chunk data
- unloaded chunks
- entities disappearing during reads
- reconnects
- dimension transitions
- temporary absence of optional values

Expected temporary conditions must not crash the bot.

---

# Testing

Add unit tests for:

- snapshot consistency
- entity filtering by radius
- chunk-loaded checks
- block lookup behavior
- state reset after disconnect
- dimension transition handling

Use mocks or isolated state structures where an actual server is unnecessary.

---

# Documentation

Document:

- which module owns each kind of state
- whether values are live, cached, optional, or derived
- how future modules should read WorldState
- why callers must not mutate snapshots

---

# Acceptance Criteria

- The project compiles.
- WorldState is centralized and reusable.
- Player and world information update from Azalea events.
- Immutable snapshots are available.
- Loaded and unknown world data are distinguished.
- Disconnect and reconnect correctly reset relevant state.
- Existing systems are reused.
- Tests pass.
- No movement or task execution is implemented.

At completion, update the project documentation or implementation-status file and provide a concise summary of changed files, tests, and remaining limitations.
