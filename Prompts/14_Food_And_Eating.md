# Food and Eating

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect InventoryState, ItemSelector, WorldState, MovementController, and existing interaction code.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement food gathering, farming, hunting, crafting, AI planning, or general survival strategy.
6. Reuse existing inventory, hotbar, movement-control, logging, configuration, and error systems.
7. Do not duplicate item selection or use-item logic.
8. Keep eating cancellable, bounded, and server-confirmed.

---

# Goal

Implement a reusable low-level eating action.

The bot should be able to choose suitable food from its current inventory, eat it using normal Minecraft behavior, and confirm the resulting state change.

This prompt implements eating execution only.

---

# Food Model

Create a reusable food-information abstraction.

For each supported food item, expose where available:

- item identifier
- hunger restored
- saturation restored
- whether it is always edible
- whether it has harmful effects
- whether it has special behavior
- whether it is configured as forbidden
- whether it is configured as emergency-only

Use Minecraft registry or Azalea data where practical.

Avoid maintaining a fragile hardcoded list if reliable game data is available.

---

# Eating Request

Create a typed request containing at least:

- target hunger level
- minimum hunger threshold for eating
- whether always-edible foods may be consumed
- whether harmful foods may be consumed
- whether emergency foods may be consumed
- preferred food group
- timeout
- cancellation support
- whether the previous hotbar selection should be restored

---

# Food Selection

Use ItemSelector.

Do not manually scan slots in the eating system.

Default food ranking should consider:

1. safe food
2. already selected food
3. food already in the hotbar
4. amount of hunger missing
5. avoiding excessive waste
6. saturation value
7. configured preferences
8. stack count
9. emergency-only status

The ranking must be deterministic.

Do not always consume the highest-value food when a smaller food is sufficient.

---

# Eating Execution

Perform normal use-item behavior.

Requirements:

- select the chosen food
- start using the item
- hold use for the required duration
- monitor hunger and inventory state
- stop use when finished
- confirm food count decreased or hunger increased
- handle server interruption
- handle player damage interrupting use
- handle movement or another system taking interaction control
- stop safely on cancellation, death, disconnect, or timeout

Do not send repeated use packets unnecessarily.

---

# State Confirmation

Success should require at least one authoritative indication:

- hunger increased
- food stack count decreased
- server-confirmed use completion

Prefer confirming both hunger and inventory changes where available.

Do not report success only because the expected duration elapsed.

---

# Interaction Ownership

Eating must coordinate with:

- movement ownership
- item-selection ownership
- use-item ownership
- combat or future emergency systems

Implement or reuse a general interaction lease.

Two systems must not simultaneously hold right-click use.

---

# Results

Return structured outcomes such as:

- ate successfully
- hunger already sufficient
- no food available
- no safe food available
- food not accessible
- interaction busy
- interrupted
- item changed
- timed out
- cancelled
- disconnected
- died
- state change not confirmed

Include selected food and before/after hunger values where available.

---

# Commands

Add temporary commands such as:

- \`eat\`
- \`eat to <hunger>\`
- \`eat item <item-id>\`
- \`eat emergency\`
- \`eat stop\`
- \`eat status\`

Use the unified command system.

---

# Configuration

Support:

- preferred foods
- forbidden foods
- harmful foods
- emergency foods
- minimum hunger threshold
- target hunger
- maximum acceptable hunger waste
- restore previous slot
- eating timeout

---

# Logging

Log:

- eating requested
- food selected
- item use started
- eating interrupted
- state change confirmed
- no food available
- cancellation
- failure

Avoid logging every use tick.

---

# Testing

Add tests for:

- deterministic food ranking
- avoiding hunger waste
- forbidden food filtering
- harmful-food filtering
- emergency-food handling
- already-satisfied behavior
- cancellation cleanup
- timeout
- inventory change confirmation
- hunger change confirmation
- previous-slot restoration

---

# Acceptance Criteria

- The project compiles.
- The bot can select and eat available food.
- Food ranking is configurable and deterministic.
- Normal Minecraft use-item behavior is used.
- Eating is confirmed from state changes.
- Cancellation releases all ownership.
- Existing inventory and item-selection systems are reused.
- Tests pass.
- No food gathering, farming, crafting, or planning is implemented.

At completion, update implementation status and summarize files, API, configuration, tests, and known limitations.
