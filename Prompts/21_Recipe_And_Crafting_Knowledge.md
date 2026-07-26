# Recipe and Crafting Knowledge

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect InventoryState, item groups, configuration, and Minecraft registry access.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not perform crafting, move items, interact with crafting tables, gather resources, or call an AI provider.
6. Reuse existing item models and identifiers.
7. Do not hardcode recipes when authoritative game data is available.
8. Keep recipe queries deterministic and version-aware.

---

# Goal

Implement a read-only recipe and crafting-knowledge service.

The system should understand what recipes exist, their ingredients, outputs, crafting requirements, and whether the bot currently has the required ingredients.

This prompt does not execute crafting.

---

# Recipe Model

Represent at least:

- recipe identifier
- output item
- output count
- recipe type
- shaped or shapeless
- width and height when shaped
- ingredient alternatives
- ingredient counts
- required station
- whether the recipe is currently known or unlocked
- version or registry source
- special recipe marker where applicable

---

# Supported Recipe Types

Initially support where data is available:

- player 2x2 crafting
- crafting-table 3x3 crafting
- shaped recipes
- shapeless recipes

Design for future support of:

- furnace
- blast furnace
- smoker
- stonecutter
- smithing
- brewing

Do not execute or fully implement those future systems now.

---

# Recipe Source

Prefer:

- Azalea registry data
- Minecraft data bundled for the active protocol version
- server recipe synchronization

Avoid manually encoding vanilla recipes in source code.

If multiple sources exist, define precedence and document it.

---

# Ingredient Matching

Support ingredients specified as:

- exact item
- one of several items
- item tag
- empty slot
- special ingredient predicate when necessary

Reuse existing item matching and group systems where appropriate.

---

# Recipe Queries

Provide APIs for:

- recipes producing an item
- recipe by identifier
- all currently known recipes
- craftable recipes from current inventory
- missing ingredients
- maximum craft count
- whether a crafting table is required
- ingredient alternatives
- choose a preferred recipe deterministically

---

# Craftability

Using an InventoryState snapshot, calculate:

- whether one craft is possible
- maximum number of crafts
- missing items
- missing counts
- whether ingredient alternatives exist
- whether required station is unavailable

Do not mutate inventory.

---

# Dependency Expansion

Implement limited recipe dependency inspection.

For a requested output, expose direct craftable dependencies.

Example:

- stone pickaxe
  - sticks
  - cobblestone

Do not recursively generate a full autonomous gathering plan yet.

Allow bounded recursive inspection for diagnostics, with:

- maximum depth
- cycle detection
- deterministic recipe choice

---

# Console Commands

Add read-only commands such as:

- \`recipe minecraft:crafting_table\`
- \`recipes for minecraft:stick\`
- \`craftable\`
- \`craftcheck minecraft:stone_pickaxe 1\`
- \`ingredients minecraft:stone_pickaxe\`
- \`recipe tree minecraft:stone_pickaxe <depth>\`

Do not craft anything.

---

# Error Handling

Handle:

- unknown item
- unknown recipe
- recipe data unavailable
- unsupported recipe type
- ambiguous preferred recipe
- malformed registry data
- recursion cycle
- depth limit reached

---

# Caching

Recipe data may be cached.

Requirements:

- cache by active protocol or registry version
- invalidate on reconnect when server recipe state changes
- avoid rebuilding recipe indexes for every query
- expose source and revision information

---

# Testing

Add tests for:

- shaped recipes
- shapeless recipes
- ingredient alternatives
- tag ingredients
- maximum craft count
- missing ingredient calculation
- recipe selection
- cycle detection
- recursive depth limits
- cache invalidation

---

# Acceptance Criteria

- The project compiles.
- Recipes can be queried by output and identifier.
- Current inventory craftability can be calculated.
- Missing ingredients are reported accurately.
- Recipe data is version-aware and cached.
- Read-only console commands work.
- Tests pass.
- No crafting execution, gathering, or AI planning is implemented.

At completion, update implementation status and summarize recipe sources, query API, commands, tests, and unsupported recipe types.
