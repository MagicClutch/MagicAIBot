# Follow Player

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect EntitySearch, navigation, movement control, WorldState, and command authorization.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement combat assistance, guarding, inventory sharing, teleport requests, AI planning, or social behavior.
6. Reuse existing entity lookup and navigation systems.
7. Do not build a separate pathfinder.
8. Keep following cancellable, bounded, and resilient.

---

# Goal

Implement a reusable player-following behavior.

The bot should follow one selected player while maintaining a configurable distance.

---

# Follow Request

Create a typed request containing:

- player name or UUID
- preferred follow distance
- minimum distance
- maximum distance
- sprinting allowed
- update interval
- lost-target timeout
- maximum follow duration
- whether dimension changes are allowed
- cancellation support

---

# Target Resolution

Use EntitySearch.

Requirements:

- exact player resolution
- reject ambiguous names
- use UUID as stable identity after selection
- handle entity ID changes after respawn
- do not switch to another player with a similar name

---

# Follow Behavior

The bot should:

- remain idle while inside the preferred distance band
- move toward the player when too far away
- stop or move back when too close if configured
- update the target as the player moves
- avoid excessive path recalculation
- use sprinting only when necessary and permitted
- stop movement when the target stops nearby
- replan when the path becomes invalid
- use actual WorldState progress for stuck detection

---

# Distance Bands

Use hysteresis to prevent movement jitter.

Example:

- below minimum distance: stop or back away
- within preferred range: remain idle
- above maximum distance: navigate closer

Make all thresholds configurable.

---

# Moving Target Navigation

Navigation should target a moving region rather than a single outdated block.

Requirements:

- update destination only when movement exceeds a threshold
- cancel obsolete path requests
- limit replanning frequency
- preserve movement ownership safely
- avoid rapid start-stop loops

Extend existing navigation abstractions instead of bypassing them.

---

# Lost Target

When the target is no longer visible:

- wait for a configurable grace period
- retain the last known position
- optionally navigate to the last known position
- stop after the lost-target timeout
- report the reason

Do not search unloaded chunks or use AI.

---

# Dimension Changes

Default behavior should stop following if the player changes dimension.

Add configuration for future dimension-follow support, but do not implement portals or cross-dimension travel in this prompt.

---

# Ownership and Preemption

Following is a long-running action.

Requirements:

- acquire navigation ownership
- allow emergency survival actions to preempt it
- allow explicit stop commands
- cleanly resume only when safe and configured
- release ownership on every exit
- stop all movement after termination

---

# Results

Return or expose states such as:

- starting
- following
- waiting nearby
- target too close
- target lost
- navigating to last known position
- paused by emergency
- stopped
- timed out
- cancelled
- disconnected
- died
- target changed dimension
- path failed

---

# Commands

Add commands such as:

- \`follow <player>\`
- \`follow <player> <distance>\`
- \`follow stop\`
- \`follow status\`
- \`follow pause\`
- \`follow resume\`

Minecraft chat commands must follow existing sender authorization.

---

# Configuration

Support:

- default follow distance
- minimum distance
- maximum distance
- sprint threshold
- target update distance
- minimum replan interval
- lost-target grace period
- lost-target timeout
- maximum duration
- navigate to last known position
- resume after emergency

---

# Logging

Log meaningful transitions:

- follow started
- target resolved
- following
- waiting nearby
- path updated
- target lost
- target found again
- emergency preemption
- stopped
- failure

Avoid per-tick logs.

---

# Testing

Add tests for:

- distance-band decisions
- hysteresis
- target identity retention
- target movement threshold
- replan rate limiting
- lost-target behavior
- dimension-change handling
- cancellation
- emergency preemption
- ownership cleanup

---

# Acceptance Criteria

- The project compiles.
- The bot can follow a selected player.
- Follow distance remains stable without excessive jitter.
- Moving-target navigation is bounded.
- Lost targets and dimension changes are handled.
- Follow can be stopped reliably.
- Emergency preemption works.
- Tests pass.
- No combat, guarding, teleporting, or AI planning is implemented.

At completion, update implementation status and summarize behavior, commands, configuration, tests, and limitations.
