# Crafting Execution

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect RecipeKnowledge, InventoryState, ItemSelector, inventory-operation ownership, navigation, block search, and entity interaction.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not gather missing ingredients, craft recursive dependencies automatically, interact with furnaces, use stonecutters, use smithing tables, or call an AI provider.
6. Reuse existing inventory and recipe systems.
7. Do not create duplicate recipe logic.
8. Confirm crafting through authoritative inventory changes.

---

# Goal

Implement crafting execution for currently available ingredients.

Support player 2x2 crafting and crafting-table 3x3 crafting.

The system must not gather missing resources.

---

# Crafting Request

Create a typed request containing:

- output item or recipe identifier
- desired output count
- preferred recipe
- whether a crafting table may be used
- whether navigation to a known crafting table is allowed
- maximum crafting-table search radius
- timeout
- cancellation support
- whether the previous inventory layout should be restored where practical

---

# Recipe Selection

Use RecipeKnowledge.

Requirements:

- resolve all recipes for the requested output
- filter unsupported recipe types
- determine craftability
- select deterministically
- respect explicit recipe choice
- calculate required craft repetitions
- report missing ingredients

Do not duplicate ingredient calculations.

---

# Crafting Modes

Support:

## Player Crafting

For recipes fitting the 2x2 grid.

## Crafting Table

For recipes requiring 3x3.

If a crafting table is required:

- search known loaded blocks
- navigate to a reachable table if allowed
- interact with it
- confirm the crafting interface opened

Do not craft a crafting table when none exists.

---

# Inventory Transactions

Use serialized inventory operations.

Requirements:

- acquire inventory ownership
- record starting inventory revision
- move ingredients into the crafting grid
- handle shaped placement correctly
- handle shapeless placement correctly
- wait for output slot update
- collect output
- repeat only as needed
- return remaining ingredients appropriately
- clear the crafting grid on cancellation or failure where possible
- verify every important revision change

Do not send rapid uncontrolled click sequences.

---

# Shift Crafting

Do not rely on shift-click crafting unless it can be implemented and confirmed safely.

A simple one-craft-at-a-time implementation is acceptable initially.

Correctness is more important than speed.

---

# Count Semantics

Clearly distinguish:

- requested output item count
- recipe output count per craft
- number of craft operations
- available maximum output
- actual crafted count

Do not overcraft unless explicitly configured.

---

# Confirmation

Success requires InventoryState confirmation that the expected output count increased.

Also verify ingredient consumption where possible.

Return partial success if some crafts completed before failure.

---

# Failure Handling

Handle:

- recipe unknown
- unsupported recipe
- missing ingredients
- crafting table required
- crafting table not found
- crafting table unreachable
- interface did not open
- inventory changed unexpectedly
- output slot did not update
- server rejected click
- no inventory space
- timeout
- cancellation
- death
- disconnect

---

# Results

Return structured data including:

- selected recipe
- requested item count
- crafted item count
- number of completed crafts
- ingredients consumed
- remaining missing amount
- whether player grid or crafting table was used
- final inventory revision
- result status

---

# Commands

Add commands such as:

- \`craft minecraft:stick 4\`
- \`craft recipe <recipe-id> <count>\`
- \`craftcheck minecraft:stone_pickaxe 1\`
- \`craft stop\`
- \`craft status\`

Do not gather missing ingredients.

---

# Configuration

Support:

- allow player crafting
- allow crafting table
- crafting-table search radius
- click delay
- inventory-confirmation timeout
- maximum craft repetitions
- overcrafting allowed
- restore grid after failure
- protected inventory slots

---

# Logging

Log:

- crafting request
- recipe selected
- missing ingredients
- crafting table selected
- interface opened
- grid populated
- output collected
- partial completion
- cancellation
- failure

Avoid logging every inventory packet.

---

# Testing

Add tests for:

- shaped-grid layout
- shapeless-grid layout
- 2x2 eligibility
- crafting-table requirement
- craft-count calculation
- no-overcraft behavior
- missing ingredients
- inventory revision conflicts
- partial completion
- cancellation cleanup
- output confirmation

Use integration tests for real inventory interaction where practical.

---

# Acceptance Criteria

- The project compiles.
- The bot can craft supported recipes using current inventory contents.
- Player and crafting-table recipes work.
- RecipeKnowledge is reused.
- Missing resources are reported rather than gathered.
- Inventory mutations are serialized and confirmed.
- Partial completion is reported accurately.
- Tests pass.
- No smelting, gathering, recursive autonomous crafting, or AI planning is implemented.

At completion, update implementation status and summarize supported recipe types, transaction behavior, commands, tests, and limitations.
