# Item Drop Collection

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect EntitySearch, WorldState, navigation, InventoryState, SurvivalMonitor, and movement ownership.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not break blocks, fight mobs, gather from containers, craft, or perform AI planning.
6. Reuse existing entity search and navigation systems.
7. Do not chase items indefinitely.
8. Keep collection bounded, safe, and inventory-aware.

---

# Goal

Implement a reusable action that collects known dropped-item entities from the loaded world.

The system should target specified item drops, navigate close enough to pick them up normally, and confirm inventory changes.

---

# Collection Request

Support:

- exact item
- one of several items
- item group
- exact entity ID
- minimum stack count
- requested total count
- maximum search radius
- maximum chase distance
- maximum item entities
- timeout
- allow partial completion
- cancellation support

---

# Candidate Search

Use EntitySearch.

Only consider:

- loaded dropped-item entities
- matching item stacks
- entities within configured bounds
- entities in the current dimension
- entities with sufficiently fresh state
- entities not known to be unreachable or dangerous

---

# Candidate Ranking

Rank deterministically by:

1. safety
2. distance
3. requested-item relevance
4. stack count
5. age when available
6. stable entity ID

Do not always prioritize the largest stack if it requires unsafe travel.

---

# Safety

Reject or penalize items:

- in lava
- over known dangerous drops
- outside loaded safe terrain
- behind unreachable geometry
- too close to the void
- inside forbidden areas when such data exists
- beyond configured chase distance

Unknown safety should follow configuration.

---

# Inventory Capacity

Before chasing an item:

- inspect available slots
- inspect compatible partial stacks
- estimate whether the stack can be collected
- stop when inventory is full
- optionally collect a partial stack when normal Minecraft behavior permits it

Do not discard items to create space.

---

# Collection Execution

For each candidate:

1. Retain stable entity identity.
2. Navigate toward its current position.
3. Update destination only when movement exceeds a threshold.
4. Enter pickup range.
5. Wait for entity disappearance or inventory increase.
6. Confirm actual collected count.
7. Continue until request is satisfied or limits are reached.

---

# Ownership

Collection must:

- acquire navigation ownership
- yield to critical survival actions
- stop movement on every exit
- release ownership after cancellation or failure
- avoid conflicting with FollowPlayer or combat

---

# Results

Return:

- completed
- partially completed
- no matching drops
- inventory full
- all candidates unsafe
- all candidates unreachable
- item despawned
- timed out
- cancelled
- disconnected
- died

Include:

- requested count
- actual collected count
- entities targeted
- entities collected
- entities lost
- remaining count

---

# Commands

Add commands such as:

- \`collectdrop minecraft:diamond 10\`
- \`collectdrop group ores all\`
- \`collectdrop nearest\`
- \`collectdrop stop\`
- \`collectdrop status\`

---

# Configuration

Support:

- search radius
- chase distance
- pickup confirmation timeout
- maximum entities
- destination update threshold
- maximum replan frequency
- unknown-terrain policy
- allow partial completion
- total timeout

---

# Logging

Log:

- collection requested
- candidate selected
- candidate moved
- candidate unsafe
- pickup confirmed
- item despawned
- inventory full
- partial completion
- cancellation
- failure

---

# Testing

Add tests for:

- item matching
- candidate ranking
- unsafe-item rejection
- inventory-capacity estimation
- moving item updates
- entity disappearance
- inventory confirmation
- partial completion
- cancellation cleanup
- survival preemption

---

# Acceptance Criteria

- The project compiles.
- The bot can collect matching loaded dropped items.
- Candidate selection is deterministic and safety-aware.
- Collection is confirmed through entity and inventory changes.
- Inventory capacity is respected.
- Chasing is bounded.
- Tests pass.
- No block breaking, mob combat, container use, crafting, or AI planning is implemented.

At completion, update implementation status and summarize behavior, commands, tests, and limitations.
