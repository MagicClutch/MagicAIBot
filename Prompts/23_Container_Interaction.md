# Container Interaction

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect BlockSearch, navigation, look control, InventoryState, ItemSelector, EntityInteraction, and inventory-operation ownership.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement automatic sorting, base storage strategy, resource gathering, shulker management, ender-chest planning, or AI planning.
6. Reuse existing interaction and inventory systems.
7. Do not create a second inventory transaction engine.
8. Keep all container operations server-confirmed and cancellable.

---

# Goal

Implement low-level interaction with block containers.

The bot should be able to open a selected container, inspect its contents, transfer specified items, and close it safely.

This prompt implements container primitives only.

---

# Supported Containers

Initially support common inventory containers where protocol behavior is available:

- chest
- trapped chest
- barrel
- shulker box
- furnace-like inventory inspection
- hopper
- dispenser
- dropper

Do not implement furnace processing or fuel strategy yet.

Handle double chests through the server-provided container layout rather than assuming slot counts.

---

# Container Target

Support:

- exact block position
- nearest block by type
- nearest block in a configured container group

Use BlockSearch and navigation.

Do not search unloaded chunks.

---

# Open Container

To open a container:

- validate target block
- find a safe interaction position
- navigate when allowed
- look at the correct block face
- interact normally
- wait for the authoritative container-open event
- verify container type and position where available
- acquire inventory-operation ownership

Do not assume success because the interaction packet was sent.

---

# Container Snapshot

Expose a read-only snapshot containing:

- container type
- block position when known
- window or menu identifier
- container slot count
- container slots
- player inventory slots
- revision or state ID
- open timestamp
- whether the snapshot is stale
- whether the container is still open

---

# Transfer Operations

Implement reusable operations for:

- deposit an exact item
- deposit an item group
- withdraw an exact item
- withdraw an item group
- transfer a requested count
- transfer all matching items
- move between explicit slots
- close container

Use normal inventory click behavior.

---

# Transfer Planning

Before clicking:

- calculate matching source stacks
- calculate destination capacity
- respect stack-size limits
- respect protected player slots
- avoid overwriting incompatible stacks
- determine whether the full request fits
- optionally allow partial transfer
- produce a deterministic transfer plan

Do not implement general sorting.

---

# Confirmation

After each transfer step:

- wait for InventoryState revision change
- confirm source and destination stack changes
- detect server rejection
- stop on unexpected state
- replan only within a bounded retry limit

Never assume a click succeeded.

---

# Concurrency

Container operations must exclusively own inventory mutation.

Requirements:

- block crafting and ItemSelector mutations while active
- release ownership after close or failure
- reject simultaneous container actions
- clean up after disconnect, death, or forced close
- detect unexpected container replacement

---

# Results

Return structured outcomes such as:

- container opened
- transfer completed
- transfer partially completed
- item not found
- destination full
- target not found
- target unreachable
- wrong container opened
- container closed unexpectedly
- inventory conflict
- server rejected click
- timed out
- cancelled
- disconnected
- died

Include actual transferred count.

---

# Commands

Add commands such as:

- \`container open <x> <y> <z>\`
- \`container opennearest chest <radius>\`
- \`container list\`
- \`container deposit minecraft:cobblestone 64\`
- \`container withdraw minecraft:diamond 10\`
- \`container transfer <from-slot> <to-slot>\`
- \`container close\`
- \`container status\`

Use explicit slot descriptions in output.

---

# Configuration

Support:

- allowed container types
- container search radius
- interaction timeout
- click delay
- confirmation timeout
- retry limit
- partial transfers
- protected inventory slots
- close after transfer
- maximum operations per request

---

# Logging

Log:

- container target selected
- container opened
- snapshot received
- transfer planned
- transfer completed
- partial transfer
- state conflict
- container closed
- cancellation
- failure

Avoid logging every slot packet at normal levels.

---

# Testing

Add tests for:

- container-type validation
- slot mapping
- stack capacity calculation
- deterministic transfer planning
- protected slots
- partial transfers
- destination full
- item not found
- revision conflict
- unexpected close
- cancellation cleanup
- ownership release

Use integration tests for real container interactions where practical.

---

# Acceptance Criteria

- The project compiles.
- The bot can open supported block containers.
- Container contents are exposed through typed snapshots.
- Items can be deposited and withdrawn by count.
- Transfers are planned deterministically and server-confirmed.
- Inventory ownership prevents concurrent mutations.
- Cancellation and unexpected closure are handled safely.
- Tests pass.
- No automatic sorting, storage strategy, smelting, gathering, or AI planning is implemented.

At completion, update implementation status and summarize supported containers, transfer API, commands, tests, and known protocol limitations.
