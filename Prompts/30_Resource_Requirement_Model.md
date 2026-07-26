# Resource Requirement Model

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect RecipeKnowledge, SmeltingKnowledge, InventoryState, item groups, and existing result types.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not execute crafting, smelting, gathering, mining, storage operations, or AI requests.
6. Keep the system read-only.
7. Reuse existing recipe and item models.
8. Ensure recursive calculations are bounded and deterministic.

---

# Goal

Implement a reusable resource-requirement model.

Given a desired item and count, the system should calculate which direct and recursive ingredients would be required to produce it using known recipes.

This is a calculation system, not an autonomous planner.

---

# Requirement Request

Support:

- desired output item
- desired output count
- preferred recipe identifiers
- allowed recipe types
- maximum recursion depth
- whether current inventory should be subtracted
- whether known storage contents should be considered
- whether smelting dependencies are allowed
- deterministic recipe-selection policy

---

# Requirement Node

Represent each requirement with:

- item or ingredient group
- required count
- available count
- missing count
- source recipe
- recipe output count
- number of operations
- dependency children
- station requirement
- whether it is a terminal raw requirement
- whether it is unresolved
- depth
- calculation warnings

---

# Recipe Expansion

Use RecipeKnowledge and SmeltingKnowledge.

Expansion should:

- select one deterministic recipe
- calculate operation counts correctly
- account for recipe output quantity
- include leftover byproducts where modeled
- recurse into craftable ingredients
- stop at configured depth
- detect cycles
- preserve ingredient alternatives

---

# Inventory Subtraction

When enabled:

- use a consistent InventoryState snapshot
- consume available counts virtually
- avoid subtracting the same item twice
- account for intermediate products already present
- keep calculations deterministic

No actual inventory changes are allowed.

---

# Storage Consideration

When enabled:

- use only recently confirmed StorageIndex contents
- report stale storage separately
- do not treat stale counts as guaranteed
- allow configuration to exclude storage entirely

---

# Alternative Ingredients

For recipe ingredients with alternatives:

- preserve the options
- select deterministically when required
- prefer currently available alternatives
- allow configured item preferences
- report when the choice affects total cost

Do not call an AI provider.

---

# Outputs

Provide:

- hierarchical requirement tree
- flattened raw-material totals
- intermediate-item totals
- required crafting operations
- required smelting operations
- required stations
- currently available counts
- missing counts
- unresolved ingredients
- warnings
- estimated processing time where known

---

# Console Commands

Add commands such as:

- \`requirements minecraft:diamond_pickaxe 1\`
- \`requirements minecraft:torch 64\`
- \`requirements minecraft:iron_pickaxe 2 inventory\`
- \`requirements minecraft:bread 32 depth 5\`
- \`requirements flat minecraft:piston 10\`

---

# Error Handling

Handle:

- unknown item
- no recipe
- unsupported recipe
- recursion cycle
- depth limit
- ambiguous recipe
- unresolved tag
- stale storage data
- recipe data unavailable

Raw items with no recipe are not errors; they are terminal requirements.

---

# Testing

Add tests for:

- recipe output multiplication
- nested recipes
- inventory subtraction
- intermediate items already available
- ingredient alternatives
- smelting dependencies
- storage inclusion
- cycle detection
- depth limit
- flattened totals
- deterministic selection

---

# Acceptance Criteria

- The project compiles.
- Resource requirements can be calculated recursively.
- Current inventory can be subtracted without double counting.
- Crafting and smelting dependencies are represented.
- Raw terminal resources are clearly identified.
- Calculations are bounded and deterministic.
- Tests pass.
- No execution, gathering, storage mutation, or AI planning is implemented.

At completion, update implementation status and summarize the calculation model, commands, tests, and limitations.
