# Tree Chopping

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect BlockResourceGathering, BlockSearch, BlockBreaking, navigation, ItemDropCollection, InventoryState, and item groups.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not replant trees, craft axes, build upward, pillar, explore unloaded terrain, or perform AI planning.
6. Reuse existing generic gathering systems.
7. Implement tree-specific structure recognition without duplicating block breaking.
8. Keep each tree operation bounded and safe.

---

# Goal

Implement specialized tree detection and chopping for loaded trees.

The bot should identify connected trunk structures, chop reachable logs in a safe order, collect drops, and stop after the requested log count or tree count is reached.

---

# Supported Trees

Support common log-based trees through configurable block groups.

Examples:

- oak
- birch
- spruce
- jungle
- acacia
- dark oak
- mangrove
- cherry
- pale oak or future version-specific trees

Do not hardcode only one Minecraft version.

---

# Tree Detection

Starting from a log candidate:

- identify connected trunk logs
- distinguish nearby separate trees when possible
- bound horizontal and vertical traversal
- avoid traversing huge player-built log structures indefinitely
- detect likely roots or mangrove complexity
- record leaves around the structure as supporting evidence
- classify uncertain structures

Use loaded world data only.

---

# Tree Model

Represent:

- tree identifier for the active task
- log positions
- trunk base
- highest known log
- tree type
- estimated log count
- reachable logs
- unreachable logs
- uncertainty
- whether the structure exceeds configured limits

---

# Chopping Order

Prefer an order that:

- starts from reachable lower logs
- does not leave the bot standing inside removed blocks
- reduces unnecessary navigation
- avoids breaking supporting terrain
- handles branching trunks
- stops when remaining logs require unsupported climbing

Do not pillar, scaffold, or place blocks.

---

# Tool Selection

Use ItemSelector and BlockBreaking.

Prefer axes where suitable.

If no suitable axe exists:

- optionally allow hand or alternate tools through configuration
- report reduced suitability
- do not craft an axe

---

# Execution

For each selected tree:

1. Validate the structure.
2. Determine reachable logs.
3. Navigate to safe positions.
4. Break logs using BlockBreaking.
5. Collect drops.
6. Re-scan the local structure after changes.
7. Continue until the tree is exhausted, the requested amount is reached, or limits stop the action.

---

# Leaves and Saplings

This prompt may collect naturally dropped nearby:

- logs
- sticks
- apples
- saplings

Do not deliberately break leaves unless explicitly configured.

Do not replant saplings yet.

---

# Player-Built Structures

Avoid destroying likely player builds.

Support configurable protections such as:

- maximum connected-log count
- require nearby leaves
- reject processed wood blocks
- reject horizontal log structures beyond a threshold
- protected regions
- explicit override

---

# Results

Return:

- requested logs collected
- requested trees chopped
- partially completed
- no trees found
- only uncertain structures found
- no reachable logs
- no suitable tool
- inventory full
- maximum tree size exceeded
- timed out
- cancelled
- disconnected
- died

Include:

- trees inspected
- trees chopped
- logs broken
- logs collected
- unreachable logs
- uncertain structures skipped

---

# Commands

Add commands such as:

- \`choptree nearest\`
- \`choptree oak\`
- \`choptree logs 64\`
- \`choptree count 3\`
- \`choptree stop\`
- \`choptree status\`

---

# Configuration

Support:

- allowed tree types
- require nearby leaves
- maximum connected logs
- maximum tree height
- maximum branch distance
- break leaves
- collect saplings
- allow hand chopping
- search radius
- maximum trees
- total timeout

---

# Logging

Log:

- tree candidate found
- structure classified
- uncertain structure skipped
- tree selected
- chopping started
- reachable logs recalculated
- tree completed
- partial completion
- cancellation
- failure

---

# Testing

Add tests for:

- simple tree detection
- branching tree detection
- adjacent separate trees
- large artificial structure rejection
- leaf-evidence requirement
- chopping order
- unreachable upper logs
- task limits
- inventory progress
- cancellation cleanup

---

# Acceptance Criteria

- The project compiles.
- Loaded trees can be identified and chopped.
- Generic navigation, breaking, collection, and item-selection systems are reused.
- Player-built log structures are protected by configurable heuristics.
- Reachability and task limits are respected.
- Tests pass.
- No replanting, tool crafting, pillaring, exploration, or AI planning is implemented.

At completion, update implementation status and summarize tree detection, supported types, commands, tests, and limitations.
