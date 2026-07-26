# Smelting Knowledge

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect RecipeKnowledge, item matching, InventoryState, and registry access.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not operate furnaces, move items, gather fuel, gather ingredients, or perform AI planning.
6. Reuse existing recipe and item models.
7. Prefer authoritative version-aware game data.
8. Keep all calculations deterministic and read-only.

---

# Goal

Implement a read-only smelting-knowledge service.

The system should understand furnace-like recipes, valid fuels, processing times, experience values, and resource requirements without executing smelting.

---

# Supported Processing Types

Support where game data is available:

- furnace smelting
- blast furnace
- smoker

Design the model for future support of other processing stations without implementing them now.

---

# Smelting Recipe Model

Represent:

- recipe identifier
- input ingredient
- output item
- output count
- processing station
- cooking time
- experience
- ingredient alternatives
- source and protocol version
- whether the recipe is currently available

---

# Fuel Model

Represent:

- fuel item
- burn duration
- number of standard furnace operations
- compatibility restrictions when applicable
- whether the item leaves a remainder
- configured cost or preference
- whether it is protected or emergency-only

Prefer registry-backed fuel data.

---

# Queries

Provide APIs for:

- recipes producing an item
- recipes accepting an input
- recipe by identifier
- valid stations for a recipe
- valid fuels
- fuel required for a requested count
- minimum fuel items needed
- available smelt count from inventory
- missing input count
- missing fuel amount
- estimated processing time

---

# Fuel Selection

Implement deterministic read-only fuel ranking based on:

- sufficient total burn time
- minimizing wasted burn time
- configured preferences
- protected-fuel status
- stack accessibility
- item value policy
- number of inventory slots required

Do not move or consume fuel.

---

# Craftability Equivalent

Using an InventoryState snapshot, calculate:

- maximum processable output
- limiting resource
- available input
- available fuel
- required operation count
- expected output
- expected experience
- estimated duration

---

# Station Requirements

Report whether:

- a furnace is required
- a blast furnace can be used
- a smoker can be used
- the selected station is incompatible
- no station is currently known

Do not search or navigate to stations in this prompt.

---

# Console Commands

Add read-only commands such as:

- \`smeltinfo minecraft:iron_ingot\`
- \`smeltfrom minecraft:raw_iron\`
- \`smeltcheck minecraft:iron_ingot 32\`
- \`fuelinfo minecraft:coal\`
- \`fuels\`
- \`smelttime minecraft:iron_ingot 64\`

---

# Caching

Build indexes for:

- output item
- input item or tag
- station type
- fuels

Cache by protocol or registry revision.

Invalidate when server recipe data changes.

---

# Error Handling

Handle:

- unknown recipe
- unsupported station
- recipe data unavailable
- unknown fuel
- ingredient alternatives
- malformed registry data
- insufficient fuel
- protected fuel only
- no known station

---

# Testing

Add tests for:

- furnace recipes
- blast-furnace recipes
- smoker recipes
- ingredient alternatives
- output count calculations
- fuel requirement calculations
- burn-time waste
- protected-fuel filtering
- estimated duration
- cache invalidation

---

# Acceptance Criteria

- The project compiles.
- Furnace-like recipes can be queried.
- Fuel requirements and processing time can be calculated.
- Current inventory smelting capacity can be inspected.
- Recipe and fuel data are version-aware.
- Read-only commands work.
- Tests pass.
- No furnace execution, gathering, navigation, or AI planning is implemented.

At completion, update implementation status and summarize data sources, APIs, commands, tests, and limitations.
