# Inventory State

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect WorldState, events, configuration, and existing item-selection code.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement crafting, chest interaction, automatic sorting, resource gathering, equipment strategies, or AI planning.
6. Reuse Azalea inventory data and existing state systems.
7. Do not create conflicting inventory representations.
8. Keep inventory reads consistent, typed, and observable.

---

# Goal

Implement a centralized inventory-state service that exposes an accurate read-only model of the bot's inventory and equipment.

This system will support future crafting, gathering, combat, building, and planning.

---

# Inventory Model

Track at least:

- hotbar slots
- main inventory slots
- armor slots
- offhand slot
- selected hotbar slot
- cursor stack when available
- currently open container type
- currently open container slots when available
- item identifier
- stack count
- durability or damage
- maximum durability when known
- enchantments when available
- custom name when available
- relevant item components or metadata
- empty slots

Do not discard unknown metadata required to identify unique item stacks.

---

# Central Service

Create an \`InventoryState\` or equivalent service integrated with WorldState or clearly adjacent to it.

Expose immutable snapshots.

Useful queries should include:

- item in slot
- selected item
- offhand item
- armor contents
- total count of an item
- find all matching stacks
- find first matching slot
- available empty slots
- available hotbar slots
- whether an item exists
- durability percentage
- currently open container information

---

# Slot Types

Use strongly typed slot identifiers.

Avoid passing ambiguous raw integers throughout the project.

Represent distinctions such as:

- hotbar
- main inventory
- armor
- offhand
- container
- cursor

Provide explicit conversion to protocol indices only at the Azalea integration boundary.

---

# Item Matching

Implement reusable structured matching based on:

- exact item identifier
- one of multiple identifiers
- configured item group
- minimum count
- maximum damage
- minimum durability
- optional metadata-sensitive matching
- optional custom predicate for internal use

Do not add natural-language item parsing.

---

# Updates

Update inventory state from authoritative Azalea events or state changes.

Handle:

- login inventory synchronization
- slot updates
- held-item changes
- item consumption
- tool durability changes
- death
- respawn
- container open
- container update
- container close
- disconnect
- reconnect

Avoid polling when event-driven information is available.

---

# Consistency

A snapshot must represent a coherent point in time.

Track revision numbers or update ticks.

Future actions should be able to check whether inventory changed between selection and execution.

---

# Console Commands

Add read-only commands such as:

- \`inventory\`
- \`inventory hotbar\`
- \`inventory armor\`
- \`inventory count minecraft:cobblestone\`
- \`inventory find minecraft:diamond_pickaxe\`
- \`inventory selected\`

Output should be concise and readable.

Do not allow moving items yet unless already required by previous code.

---

# Configuration

Add configurable item groups, such as:

- pickaxes
- axes
- shovels
- weapons
- food
- building blocks
- logs
- ores

Groups should use item identifiers or tags and remain extensible.

---

# Error Handling

Handle:

- inventory unavailable during login
- unknown items
- unsupported metadata
- container closing during a read
- stale snapshots
- malformed item-group configuration
- disconnect resets

Unknown data must remain explicit.

---

# Testing

Add tests for:

- slot typing and conversion
- total item counts
- exact item matching
- group matching
- durability filtering
- empty-slot counting
- revision changes
- container open and close
- disconnect reset
- snapshot consistency

---

# Acceptance Criteria

- The project compiles.
- Inventory and equipment are represented centrally.
- Immutable consistent snapshots are available.
- Item queries and configured groups work.
- Slot types prevent ambiguous indexing.
- Console inspection commands work.
- Disconnect and reconnect reset state correctly.
- Tests pass.
- No crafting, sorting, container automation, or AI planning is implemented.

At completion, update implementation status and summarize the state model, query API, commands, tests, and metadata limitations.
