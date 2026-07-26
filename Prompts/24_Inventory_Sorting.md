# Inventory Sorting

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect InventoryState, ItemSelector, ContainerInteraction, item groups, and inventory-operation ownership.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement storage-location memory, automatic chest selection, resource gathering, crafting, smelting, or AI planning.
6. Reuse existing inventory transaction systems.
7. Do not send raw inventory clicks outside the established integration boundary.
8. Keep sorting deterministic, configurable, cancellable, and server-confirmed.

---

# Goal

Implement reusable inventory-sorting functionality for the bot's personal inventory and an already-open container.

The system should organize items according to configurable rules without deciding which world container should store them.

---

# Sorting Scope

Support:

- player hotbar
- player main inventory
- an already-open container
- selected slot ranges
- optional inclusion of armor and offhand for inspection only

Do not automatically move equipped armor or offhand items unless explicitly configured.

---

# Sorting Rules

Create configurable rules based on:

- exact item identifier
- item group
- item tag
- item category
- durability
- custom name
- enchantment presence
- stack count
- protected status
- preferred slot
- preferred slot range
- priority

Rules must be processed deterministically.

---

# Default Layout Support

Allow configurable layout concepts such as:

- weapons
- tools
- food
- blocks
- utility items
- empty reserved slots
- temporary task slots

Do not force one layout.

Provide useful example configuration only.

---

# Sorting Plan

Before mutating inventory:

1. Capture a consistent InventoryState snapshot.
2. Determine protected slots.
3. Classify all movable stacks.
4. Merge compatible partial stacks where allowed.
5. assign stacks to preferred slots.
6. Preserve unmatched items safely.
7. Produce an ordered transaction plan.
8. Validate that the plan does not lose or overwrite items.

Expose the plan for debugging before execution.

---

# Stack Merging

Support:

- merging compatible stacks
- respecting maximum stack size
- preserving metadata differences
- avoiding merges between non-identical metadata-sensitive items
- deterministic source and destination selection

Never assume items with the same base identifier are stack-compatible.

---

# Protected Items and Slots

Respect:

- protected hotbar slots
- active ItemSelector leases
- currently selected task item
- armor
- offhand
- named protected items
- configured valuable-item groups
- temporary reservation handles

Sorting must fail or skip safely when protected state conflicts with the requested layout.

---

# Execution

Use the existing serialized inventory-operation system.

Requirements:

- execute one confirmed step at a time
- wait for revision updates
- detect unexpected inventory changes
- stop on stale state
- support cancellation
- release ownership on every exit
- report partial completion accurately
- avoid excessive click speed

---

# Dry Run

Support a dry-run mode that:

- creates the sorting plan
- shows proposed moves
- does not mutate inventory
- reports conflicts
- reports unmatched items
- estimates number of operations

---

# Container Sorting

When a container is already open:

- allow sorting only container slots
- allow sorting only player slots
- allow sorting both independently
- do not automatically transfer categories between player and container
- preserve server-provided slot mapping

Cross-storage classification belongs to later prompts.

---

# Results

Return structured outcomes such as:

- already sorted
- sorted successfully
- partially sorted
- dry-run completed
- protected-slot conflict
- no valid layout
- inventory changed unexpectedly
- operation rejected
- timed out
- cancelled
- disconnected
- container closed
- died

Include:

- moves planned
- moves completed
- stacks merged
- unmatched items
- conflicts

---

# Commands

Add commands such as:

- \`sort inventory\`
- \`sort hotbar\`
- \`sort container\`
- \`sort inventory dry\`
- \`sort layout <name>\`
- \`sort stop\`
- \`sort status\`

---

# Configuration

Support:

- named layouts
- slot ranges
- protected slots
- protected item groups
- merge partial stacks
- move unknown items
- operation delay
- confirmation timeout
- maximum operations
- restore selected slot

---

# Logging

Log:

- sorting requested
- layout selected
- plan created
- conflicts detected
- execution started
- operation rejected
- partial completion
- sorting completed
- cancellation

Avoid logging every inventory packet.

---

# Testing

Add tests for:

- deterministic classification
- preferred-slot assignment
- stack compatibility
- partial-stack merging
- protected slots
- protected items
- unmatched-item handling
- dry-run behavior
- stale revision rejection
- partial execution
- cancellation cleanup

---

# Acceptance Criteria

- The project compiles.
- Player inventories and open containers can be sorted.
- Sorting plans are deterministic and inspectable.
- Metadata-sensitive stacks are preserved.
- Protected slots and items are respected.
- Every mutation is server-confirmed.
- Dry-run mode works.
- Tests pass.
- No storage-location selection, gathering, crafting, smelting, or AI planning is implemented.

At completion, update implementation status and summarize layouts, planning behavior, commands, tests, and limitations.
