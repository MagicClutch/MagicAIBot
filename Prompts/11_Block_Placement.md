# Block Placement

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect WorldState, MovementController, navigation, look control, and inventory code.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement building plans, schematics, crafting, resource gathering, scaffolding algorithms, or AI planning.
6. Reuse existing systems.
7. Never duplicate inventory selection, navigation, or rotation logic.
8. Keep placement cancellable, validated, and server-confirmed.

---

# Goal

Implement a reusable low-level block-placement action for placing one block at an explicitly selected world position.

This is an execution primitive for future building systems.

---

# Placement Request

Create a typed request containing at least:

- target position
- desired block item or accepted item group
- interaction range
- whether navigation is permitted
- whether item substitution is permitted
- timeout
- preferred face or placement orientation when relevant
- cancellation support

---

# Validation

Before placement, verify:

- bot is connected and alive
- target dimension matches
- relevant chunks are loaded
- target location is replaceable
- target is not already occupied by an incompatible block
- a valid neighboring support block exists
- the bot has the requested block item
- the position is within world height limits
- the placement is within configured range or can be reached
- the placement would not intersect the bot when avoidable
- the target is not forbidden by configuration

---

# Placement Geometry

Determine a valid placement interaction:

- support-block position
- support face
- hit vector
- target block position
- required player standing position
- required look target

Use Minecraft placement rules where available.

Do not assume every block can be placed by clicking its center.

Support ordinary full blocks first.

Design the API for future orientation-sensitive blocks, but do not attempt to perfectly support every special block in this prompt.

---

# Reach and Look

Use existing navigation to reach a valid interaction position.

Use existing look control to face the correct hit point.

Do not duplicate those systems.

If no valid interaction position exists, return a typed failure.

---

# Item Selection

Use existing inventory and hotbar selection logic.

Requirements:

- locate the requested item
- select it if already in the hotbar
- move it to an available hotbar slot only if an existing inventory-operation system supports this
- otherwise return a clear \`item_not_accessible\` result
- confirm the selected item before placement

Do not implement full inventory rearrangement if it does not exist yet.

---

# Placement Execution

Perform normal Minecraft use-item/block interaction.

After sending the interaction:

- monitor WorldState
- confirm the target block changed as expected
- detect server rejection
- detect item count changes when available
- retry only within a small configured limit
- do not spam placement packets
- stop on cancellation, death, disconnect, or timeout

Success requires world-state confirmation.

---

# Placement Results

Return structured outcomes such as:

- placed
- already satisfied
- target occupied
- target not replaceable
- no support face
- item missing
- item inaccessible
- unreachable
- out of range
- forbidden position
- server rejected
- result not confirmed
- timed out
- cancelled
- disconnected
- died

---

# Replaceability

Create a reusable method for deciding whether a target block is replaceable.

It should handle common cases such as:

- air
- cave air
- void air
- grass-like replaceable vegetation
- fluids only when explicitly allowed
- snow layers or similar blocks where supported

Use registry or Azalea data rather than a fragile hardcoded list wherever possible.

---

# Safety Configuration

Support configurable controls for:

- allowed placement blocks
- forbidden placement blocks
- fluid replacement
- placement inside entities
- placement at the bot's feet
- placement above the bot
- maximum retries
- interaction distance
- action timeout

---

# Commands

Add temporary commands such as:

- \`placeblock <x> <y> <z> minecraft:cobblestone\`
- \`placebelow minecraft:cobblestone\`
- \`place stop\`
- \`place status\`

Do not add large building commands.

---

# Logging

Log:

- placement requested
- item selected
- interaction position selected
- navigation started
- placement sent
- placement confirmed
- retry
- cancellation
- failure

Avoid packet-level spam at normal log levels.

---

# Testing

Add tests for:

- replaceable-block detection
- support-face selection
- target occupied
- item missing
- out-of-range behavior
- orientation data structures
- server-confirmed completion
- retry limits
- cancellation cleanup
- disconnect/death cleanup

---

# Acceptance Criteria

- The project compiles.
- The bot can place an ordinary full block at a valid position.
- Placement uses normal Minecraft interaction.
- Navigation, looking, WorldState, and inventory systems are reused.
- Success is confirmed from server-updated world state.
- Failures are typed and useful.
- Cancellation and cleanup work.
- Tests pass.
- No schematic, crafting, gathering, or AI planning system is implemented.

At completion, update implementation status and summarize implementation, tests, supported block types, and limitations.
