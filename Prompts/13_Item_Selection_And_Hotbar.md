# Item Selection and Hotbar Management

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect InventoryState, MovementController, block breaking, and block placement.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement full inventory sorting, crafting, chest interaction, equipment strategy, resource gathering, or AI planning.
6. Reuse existing inventory and selected-slot systems.
7. Consolidate any temporary item-selection logic created by earlier prompts.
8. Keep every mutation server-confirmed, serialized, and cancellable.

---

# Goal

Implement a reusable item-selection and minimal hotbar-management service.

Future tasks should be able to request an item without manually searching slots or sending raw inventory packets.

---

# Item Selector

Create a central \`ItemSelector\` or equivalent.

Support requests based on:

- exact item
- one of several items
- configured item group
- preferred item
- minimum count
- minimum durability
- maximum damage
- metadata-sensitive or metadata-insensitive matching
- preferred current selection
- preference for hotbar items
- optional ranking policy

---

# Selection Policy

Use deterministic ranking.

A sensible default order is:

1. already selected matching item
2. matching item already in the hotbar
3. best matching item in main inventory if hotbar movement is allowed
4. otherwise fail clearly

For tools, allow ranking by:

- suitability
- mining speed when known
- enchantments when known
- remaining durability
- configured preference

Do not implement crafting when no item exists.

---

# Selected Hotbar Slot

Provide safe operations to:

- select a hotbar slot
- select a matching item
- read currently selected item
- confirm server/client state update
- restore a previous selected slot
- reject invalid slot numbers

Use zero-based or strongly typed internal slots consistently. Convert user-facing slots clearly.

---

# Minimal Inventory-to-Hotbar Movement

When a requested item exists only in the main inventory:

- identify an appropriate hotbar destination
- prefer an empty slot
- otherwise use a configurable replaceable slot
- never overwrite a protected slot
- perform the normal inventory click sequence
- confirm the resulting inventory state revision
- rollback or report failure when the operation is rejected

Only implement the minimum required transfer between main inventory and hotbar.

Do not implement general sorting.

---

# Protected Slots

Support configuration for:

- permanently protected hotbar slots
- preferred tool slots
- preferred weapon slot
- preferred food slot
- preferred block slot
- slots temporary tasks may replace
- whether previous contents should be restored after a task

Do not force a fixed layout.

---

# Leases and Concurrency

Inventory mutations must be serialized.

Implement an inventory-operation lease or queue so that:

- block breaking cannot switch tools while placement switches blocks
- two tasks cannot click inventory slots simultaneously
- cancellation releases control
- disconnect and death clear pending operations
- stale inventory revisions abort unsafe actions

Reuse an existing general resource-ownership system if appropriate.

---

# Selection Handle

Consider returning a temporary selection handle that records:

- selected item
- selected hotbar slot
- previous selected slot
- inventory revision
- whether an item was moved
- optional restoration action

When dropped or explicitly released, it should safely restore previous selection when configured and still valid.

Avoid fragile implicit behavior if Rust lifecycle constraints make explicit cleanup safer.

---

# Results and Errors

Return typed outcomes such as:

- selected
- already selected
- moved to hotbar and selected
- item missing
- no acceptable item
- insufficient durability
- no usable hotbar slot
- protected-slot conflict
- inventory busy
- stale inventory
- server rejected mutation
- timed out
- cancelled
- disconnected
- died

---

# Commands

Add temporary commands such as:

- \`selectitem minecraft:diamond_pickaxe\`
- \`selectgroup pickaxes\`
- \`selectslot 1\`
- \`hotbar\`
- \`hotbar move <inventory-slot> <hotbar-slot>\`
- \`itemselect stop\`

Make user-facing hotbar numbering explicit, preferably 1–9.

---

# Integration

Refactor previous temporary selection code so:

- block breaking uses ItemSelector
- block placement uses ItemSelector
- no duplicate tool ranking remains
- no feature sends raw selected-slot changes outside the central service unless required at the integration boundary

Do not expand those features beyond integration.

---

# Logging

Log:

- selection request
- matching candidates
- selected result
- item moved to hotbar
- slot restored
- protected-slot refusal
- stale revision
- server rejection
- cancellation

Do not print full sensitive item metadata unnecessarily.

---

# Testing

Add tests for:

- deterministic ranking
- already-selected preference
- hotbar preference
- durability filtering
- protected slots
- empty-slot selection
- replacement policy
- stale revision rejection
- concurrent-operation rejection
- cancellation cleanup
- previous-slot restoration
- invalid slot numbers

---

# Acceptance Criteria

- The project compiles.
- Items can be selected through one centralized API.
- Main-inventory items can be moved into a safe hotbar slot.
- Protected slots are respected.
- Inventory mutations are serialized.
- Changes are confirmed from InventoryState.
- Block breaking and placement use the central selector.
- Cancellation and restoration work.
- Tests pass.
- No general sorting, crafting, chest automation, or AI planning is implemented.

At completion, update implementation status and summarize the API, integrations, configuration, tests, and known protocol limitations.
