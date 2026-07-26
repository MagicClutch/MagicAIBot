# Storage Index

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect WorldState, BlockSearch, ContainerInteraction, InventoryState, and item matching systems.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not automatically deposit or withdraw items.
6. Do not create permanent learned knowledge across restarts.
7. Do not implement base planning, resource gathering, or AI planning.
8. Keep all indexed data session-scoped, validated, and explicitly stale when uncertain.

---

# Goal

Implement a session-scoped storage index that records known containers and their observed contents.

The index should help future systems locate likely storage without pretending to know the contents of containers that have not been inspected recently.

---

# Storage Entry

Represent each known storage container with:

- dimension
- block position
- container type
- optional custom name
- last observed contents
- last opened timestamp
- last validated timestamp
- inventory revision when observed
- total slot count
- free-slot estimate
- item-count summary
- observed categories
- stale status
- accessibility status when known
- optional user-defined label

---

# Session Scope

The index must:

- exist only during the current process session
- reset on application restart
- reset or invalidate entries on server change
- invalidate dimension-specific information appropriately
- not create long-term learned storage behavior

Configuration-defined labels may persist, but automatically discovered contents must not.

---

# Discovery

Allow containers to enter the index through:

- successful ContainerInteraction
- explicit console registration
- observed block updates
- block searches when configured

A discovered but unopened container must be marked as having unknown contents.

---

# Validation

Before using an entry:

- confirm the chunk is loaded when possible
- confirm the block is still a compatible container
- detect block removal or replacement
- mark entries stale after configured time
- mark contents unknown after suspected external modification
- never claim exact contents when the container has not been opened recently

---

# Queries

Provide APIs for:

- all known containers
- containers by type
- containers by label
- containers likely containing an item
- containers likely containing an item group
- containers with estimated free space
- nearest known container
- nearest recently validated container
- stale entries
- inaccessible entries

Results must include confidence and staleness information.

---

# Item Summaries

For observed contents, index:

- exact item counts
- item-group counts
- occupied slots
- free slots
- partially filled compatible stacks

Preserve enough metadata to avoid combining unique non-stack-compatible items incorrectly.

---

# Labels

Support user-defined labels such as:

- ores
- tools
- food
- building
- valuables
- dump
- temporary

Labels may be assigned through configuration or commands.

Do not automatically invent semantic labels using AI.

---

# Events

Update or invalidate entries on:

- container opened
- container transfer completed
- container closed
- block changed
- chunk unloaded
- dimension changed
- disconnect
- reconnect
- server changed

---

# Commands

Add commands such as:

- \`storage list\`
- \`storage nearby <radius>\`
- \`storage info <x> <y> <z>\`
- \`storage find minecraft:diamond\`
- \`storage findgroup ores\`
- \`storage label <x> <y> <z> <label>\`
- \`storage unlabel <x> <y> <z>\`
- \`storage stale\`
- \`storage clear\`

These commands must not transfer items.

---

# Configuration

Support:

- stale-after duration
- content-expiry duration
- maximum indexed containers
- discovery radius
- allowed container types
- labels
- whether unopened containers are indexed
- whether chunk unload marks entries stale
- server-change reset behavior

---

# Results and Confidence

Search results should distinguish:

- exact recently observed contents
- stale observed contents
- unknown contents
- container no longer valid
- container currently unloaded
- estimated free space
- confirmed free space

---

# Logging

Log:

- storage discovered
- contents indexed
- entry refreshed
- entry invalidated
- label changed
- stale entry used
- index cleared

---

# Testing

Add tests for:

- session reset
- stale transitions
- content invalidation
- item-count indexing
- group-count indexing
- nearest-container ordering
- confidence classification
- block replacement
- label assignment
- maximum-entry eviction

---

# Acceptance Criteria

- The project compiles.
- Known containers can be indexed during the current session.
- Contents are recorded only after observation.
- Stale and unknown contents are clearly distinguished.
- Queries by item, group, distance, and label work.
- The index does not permanently learn across restarts.
- Tests pass.
- No automatic transfer, storage strategy, gathering, or AI planning is implemented.

At completion, update implementation status and summarize the index model, invalidation behavior, commands, tests, and limitations.
