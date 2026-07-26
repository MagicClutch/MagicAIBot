# Look at Target

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect WorldState, MovementController, and navigation code.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not break blocks, place blocks, attack entities, or implement AI planning.
6. Reuse the existing rotation controls and coordinate types.
7. Do not create a second movement controller.
8. Keep the system precise, cancellable, and reusable.

---

# Goal

Implement a reusable target-looking system that rotates the bot toward blocks, positions, entities, or players.

Future mining, placement, interaction, and combat systems will depend on this feature.

---

# Target Types

Support looking at:

- exact world position
- center of a block
- configurable point on a block
- entity position
- entity eye position when known
- player eye position
- fixed yaw and pitch

Use a typed target enum or equivalent.

---

# Rotation Calculation

Correctly calculate yaw and pitch from the bot's current eye position to the target point.

Requirements:

- account for player eye height
- normalize yaw
- clamp pitch
- handle targets directly above or below
- reject NaN and infinite coordinates
- use Minecraft-compatible orientation conventions
- avoid unnecessary jitter near the target angle

---

# Look Modes

Provide at least:

## Immediate

Set the required yaw and pitch immediately.

## Smooth

Rotate toward the target over time.

Smooth mode should support:

- maximum yaw speed
- maximum pitch speed
- update interval or tick-based updates
- angular tolerance
- timeout
- cancellation

Do not make movement depend on frame rate.

---

# Dynamic Targets

For entities and players, optionally track the moving target until:

- alignment is reached
- timeout occurs
- the target disappears
- cancellation occurs

Do not continue tracking indefinitely without limits.

---

# Visibility

Provide geometric alignment only.

Do not implement full ray tracing or line-of-sight validation unless a minimal existing Azalea API makes it trivial and reusable.

The result should clearly distinguish:

- rotation aligned
- target missing
- timed out
- cancelled
- disconnected
- invalid target
- movement control unavailable

---

# Control Ownership

Coordinate with the existing movement/rotation ownership system.

Looking should not silently override another controller's rotation.

Support explicit acquisition and cleanup.

Future combat may need a higher-priority owner, but do not implement combat now.

---

# Public API

Provide reusable operations equivalent to:

- look at position
- look at block
- look at entity
- look at player
- check whether currently aligned
- cancel active look action

Use names consistent with the codebase.

---

# Commands

Add test commands such as:

- \`look pos <x> <y> <z>\`
- \`look block <x> <y> <z>\`
- \`look player <name>\`
- \`look entity <id>\`
- \`look yawpitch <yaw> <pitch>\`
- \`look stop\`

Allow an optional \`smooth\` or \`immediate\` mode.

---

# Logging

Log:

- look action started
- target resolved
- alignment completed
- target lost
- timeout
- cancellation
- invalid target

Avoid logging every rotation tick.

---

# Testing

Add tests for:

- cardinal directions
- targets above and below
- yaw wrapping around ±180 degrees
- pitch limits
- block-center calculation
- eye-position calculation
- angular tolerance
- smooth step limits
- moving-target loss
- cancellation
- invalid coordinates

---

# Acceptance Criteria

- The project compiles.
- The bot can face positions, blocks, entities, and players.
- Yaw and pitch calculations match Minecraft orientation.
- Immediate and smooth modes work.
- Dynamic targets can be tracked within bounded limits.
- Cancellation and ownership cleanup work.
- Tests pass.
- No mining, placement, attacking, or AI planning is implemented.

At completion, update implementation status and summarize the API, commands, tests, and any Azalea limitations discovered.
