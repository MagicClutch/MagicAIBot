# Block Resource Gathering

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect BlockSearch, NavigateToBlock, BlockBreaking, ItemDropCollection, InventoryState, SurvivalMonitor, and item groups.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not craft tools, build bridges, explore unloaded terrain, use AI planning, or gather from mobs and containers.
6. Reuse existing block search, navigation, breaking, and drop collection.
7. Keep gathering bounded by loaded world data and explicit limits.
8. Do not create specialized mining behavior for ores or trees yet.

---

# Goal

Implement a generic action for gathering resources that come from breaking known loaded blocks.

Example requests include gathering stone, dirt, sand, logs, or visible ores.

This is a reusable bounded gathering loop, not a full mining strategy.

---

# Gathering Request

Support:

- target block identifier
- target block group
- desired resulting item
- desired collected count
- maximum search radius
- maximum travel distance
- maximum blocks to break
- allowed tool groups
- whether drops should be collected
- allow partial completion
- timeout
- cancellation support

---

# Block-to-Drop Mapping

Use authoritative loot or block data when available.

The system should understand:

- expected dropped item
- Silk Touch differences when known
- Fortune differences when known
- tool requirement
- whether no drop is expected with the current tool
- blocks with non-deterministic drops
- blocks requiring special handling

When exact loot data is unavailable, expose uncertainty.

---

# Candidate Selection

Use BlockSearch.

Rank candidates by:

1. known safety
2. reachable interaction position
3. expected useful drop
4. distance
5. tool suitability
6. stable coordinate order

Do not repeatedly target failed blocks without a cooldown or exclusion set.

---

# Gathering Loop

For each selected block:

1. Confirm remaining required count.
2. Confirm inventory capacity.
3. Search for candidates.
4. Select a reachable candidate.
5. Navigate using existing navigation.
6. Break using BlockBreaking.
7. Collect resulting drops using ItemDropCollection.
8. Confirm inventory count increase.
9. Update remaining amount.
10. Continue within configured limits.

---

# Progress Measurement

Measure success from actual inventory counts.

Do not assume one broken block equals one collected item.

Account for:

- multiple drops
- no drop
- lost drops
- Fortune
- Silk Touch
- stack limits
- pickup failure

---

# Failure Memory

Maintain only task-local failure records for:

- unreachable block
- unsafe block
- changed block
- no useful drop
- repeated navigation failure

Do not permanently learn these failures across restarts.

---

# Tool Handling

Use ItemSelector and BlockBreaking tool selection.

If no suitable tool exists:

- return a clear result
- do not craft one
- do not continue breaking blocks that produce no useful drop unless explicitly permitted

---

# Safety

Stop or skip candidates when:

- critical survival emergency occurs
- inventory is full
- target would expose known lava
- target is beneath the bot and forbidden
- target supports dangerous falling blocks when detectable
- target lies beyond loaded safe data
- maximum failures are reached

---

# Results

Return:

- completed
- partially completed
- no matching loaded blocks
- no reachable blocks
- no suitable tool
- inventory full
- unsafe candidates only
- maximum blocks reached
- timed out
- cancelled
- disconnected
- died

Include:

- requested count
- collected count
- blocks broken
- candidates skipped
- failed candidates
- elapsed time

---

# Commands

Add commands such as:

- \`gatherblock minecraft:cobblestone 64\`
- \`gatherblock group logs 32\`
- \`gatherblock minecraft:sand 128 radius 48\`
- \`gatherblock stop\`
- \`gatherblock status\`

---

# Configuration

Support:

- search radius
- travel distance
- maximum blocks
- maximum failures
- failed-target cooldown
- allow uncertain drops
- allow partial completion
- inventory reserve slots
- total timeout
- safety restrictions

---

# Logging

Log:

- gathering requested
- candidate selected
- candidate skipped
- block broken
- drops collected
- progress updated
- inventory full
- partial completion
- cancellation
- failure

---

# Testing

Add tests for:

- candidate ranking
- drop expectation
- inventory-count progress
- multiple drops
- no-drop handling
- failed-target exclusion
- block limits
- partial completion
- inventory-full behavior
- survival preemption
- cancellation cleanup

---

# Acceptance Criteria

- The project compiles.
- The bot can gather a requested amount from matching loaded blocks.
- Existing search, navigation, breaking, and collection systems are reused.
- Progress is measured from actual inventory changes.
- Failed candidates are avoided during the active task.
- Safety and resource limits are respected.
- Tests pass.
- No crafting, exploration, bridge building, specialized mining, or AI planning is implemented.

At completion, update implementation status and summarize the gathering loop, commands, tests, and limitations.
