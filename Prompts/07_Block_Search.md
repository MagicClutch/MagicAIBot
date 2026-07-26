# Block Search

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect WorldState and existing block/chunk abstractions.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not move the bot, break blocks, place blocks, pathfind, or call an AI provider.
6. Search only known loaded world data.
7. Reuse existing coordinate, world-state, logging, configuration, and error systems.
8. Keep searches bounded, cancellable, and efficient.

---

# Goal

Implement a reusable block-search service that finds blocks matching structured criteria within currently loaded world data.

This service will later support mining, building, navigation, farming, and planning.

It must not control movement.

---

# Block Search Service

Create a reusable \`BlockSearchService\` or equivalent.

Support queries based on:

- exact block type
- one of several block types
- block tag or category when available
- distance from an origin
- horizontal radius
- vertical radius
- full three-dimensional radius
- maximum result count
- nearest result
- sorted result list
- dimension
- loaded chunks only
- optional custom predicate

Use typed query and result structures.

---

# Query Examples

The API should be capable of representing requests such as:

- nearest stone block within 16 blocks
- nearest diamond ore within 64 blocks
- all logs within 20 blocks, maximum 100 results
- all air blocks above solid ground in a bounded area
- nearest crafting table
- blocks matching any block in a configured resource group

Do not create natural-language parsing yet.

---

# Search Results

Each result should include at least:

- block position
- block state or identifier
- distance from origin
- dimension
- whether the information came from currently loaded data
- optional metadata useful to later systems

Never claim that a block does not exist outside loaded chunks.

Differentiate:

- found
- not found in searched loaded area
- search area contains unloaded sections
- cancelled
- invalid query

---

# Search Ordering

Support deterministic ordering.

For equal distances, use stable coordinate ordering so tests and behavior are reproducible.

Allow:

- nearest first
- farthest first when useful
- unsorted for performance-sensitive internal use

---

# Performance

Block searches can be expensive.

Requirements:

- enforce configurable search limits
- avoid repeatedly locking WorldState for each block
- operate from a consistent snapshot or efficient read view
- avoid unnecessary allocations
- support early exit for nearest-block searches
- support cancellation
- avoid blocking the async runtime for large searches
- consider a worker thread or incremental scanning if necessary

Do not prematurely build a complex permanent spatial index unless the current architecture clearly needs one.

---

# Block Groups

Add configurable named block groups, for example:

- logs
- ores
- food crops
- replaceable blocks
- solid building blocks

Use Minecraft block identifiers or tags.

Do not hardcode every future category into Rust code.

---

# Console Testing Commands

Add commands such as:

- \`findblock minecraft:stone 16\`
- \`findblock minecraft:diamond_ore 64\`
- \`findblocks logs 32 20\`

Return concise results:

- nearest position
- distance
- number of matches
- warning if parts of the area were unloaded

Do not navigate to the result.

---

# Validation

Reject or safely clamp:

- negative radii
- unreasonably large searches
- zero result limits when invalid
- unknown block identifiers
- unknown block groups
- origins in a different unloaded dimension

---

# Testing

Add unit tests for:

- exact block matching
- multiple block matching
- named groups
- nearest-result ordering
- stable tie-breaking
- radius boundaries
- unloaded-area reporting
- result limits
- cancellation
- invalid queries

---

# Acceptance Criteria

- The project compiles.
- Loaded blocks can be searched by structured criteria.
- Nearest-block searches are deterministic.
- Unknown and unloaded areas are reported honestly.
- Searches are bounded and cancellable.
- Search commands work without moving the bot.
- Existing WorldState is reused.
- Tests pass.
- No navigation, mining, or AI integration is implemented.

At completion, update implementation status and summarize the API, commands, performance protections, and tests.
