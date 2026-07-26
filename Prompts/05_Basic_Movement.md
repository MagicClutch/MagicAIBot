# Basic Movement

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect all existing code, especially WorldState and client management.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement pathfinding, obstacle avoidance, block breaking, block placement, combat, AI planning, or navigation to distant goals.
6. Reuse existing systems and Azalea APIs.
7. Never duplicate movement or state logic.
8. Keep the implementation cancellable, asynchronous, and production-ready.

---

# Goal

Implement a low-level movement-control layer for direct player input.

This layer should expose safe reusable primitives that future navigation and combat systems can use.

It is not a pathfinder.

---

# Movement Controller

Create a reusable \`MovementController\` or equivalent abstraction.

It should support:

- move forward
- move backward
- strafe left
- strafe right
- jump
- sneak
- sprint
- stop individual inputs
- stop all movement
- set yaw
- set pitch
- set yaw and pitch together
- select a hotbar slot if Azalea exposes this cleanly

Movement should represent held player inputs where appropriate, not repeated chat commands or teleports.

---

# Movement State

Track currently active inputs.

Expose read-only status such as:

- active movement directions
- jumping
- sneaking
- sprinting
- current requested rotation
- whether movement is enabled

Do not treat requested movement as proof that the bot actually moved. Actual position remains owned by WorldState.

---

# Direct Movement Commands

Add temporary console commands for manual testing, for example:

- \`move forward\`
- \`move back\`
- \`move left\`
- \`move right\`
- \`jump\`
- \`sneak on\`
- \`sneak off\`
- \`sprint on\`
- \`sprint off\`
- \`look <yaw> <pitch>\`
- \`movement stop\`

Use the existing unified command system.

Minecraft chat control should follow existing authorization rules. Do not create a separate parser if a parser already exists.

---

# Cancellation

All movement must stop when:

- the movement command is cancelled
- the bot disconnects
- the bot dies
- shutdown begins
- a higher-level owner releases control

Implement a clear cancellation or control-ownership mechanism suitable for future tasks.

Avoid leaving movement keys held after errors.

---

# Control Ownership

Prepare movement for multiple future consumers.

Examples include:

- navigation
- combat
- emergency recovery
- manual console control

Implement a simple ownership or lease model so two systems cannot issue conflicting input simultaneously.

Do not implement those future consumers now.

Requirements:

- one active owner at a time
- explicit acquire and release
- safe cleanup when an owner disappears
- ability to force-stop during shutdown or disconnect
- meaningful error if control is already owned

---

# Rotation

Normalize yaw and clamp pitch correctly.

Avoid instant invalid values.

Provide:

- immediate rotation for low-level use
- an interface that can later support smooth rotation

Do not implement target-looking logic yet.

---

# Safety

Prevent invalid input such as:

- NaN rotation
- infinite rotation
- invalid hotbar slot
- movement while disconnected when it would cause errors

Expected failures should return typed errors.

---

# Logging

Log meaningful transitions:

- movement ownership acquired
- movement ownership released
- movement started
- movement stopped
- forced reset
- invalid command
- movement failure

Avoid logging every tick.

---

# Testing

Add unit tests for:

- yaw normalization
- pitch clamping
- ownership conflicts
- ownership release
- stop-all behavior
- invalid rotations
- invalid hotbar slots

---

# Acceptance Criteria

- The project compiles.
- Direct movement inputs work on a test server.
- Movement can be stopped reliably.
- Rotation works.
- Movement ownership prevents conflicting controllers.
- Disconnect, death, and shutdown clear inputs.
- Commands use the existing command architecture.
- Tests pass.
- No pathfinding or autonomous navigation is implemented.

At completion, update implementation status and summarize changed files, tests, and manual verification steps.
