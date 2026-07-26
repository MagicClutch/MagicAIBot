# Entity Search

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect WorldState and existing entity representations.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not move, attack, follow, interact with, or pathfind to entities.
6. Search only currently known loaded entities.
7. Reuse existing coordinate, state, logging, configuration, and error systems.
8. Keep searches deterministic and efficient.

---

# Goal

Implement a reusable entity-search service.

Future combat, following, farming, trading, assistance, and interaction systems will use it.

This prompt only searches and ranks known entities.

---

# Entity Query

Create a typed query supporting:

- exact entity ID
- exact UUID
- entity type
- one of several entity types
- player name
- player UUID
- entity category
- distance from origin
- horizontal radius
- vertical radius
- full radius
- dimension
- maximum result count
- alive-only
- player-only
- hostile-only
- passive-only
- item entities
- visible metadata conditions
- optional custom predicate

---

# Entity Categories

Support configurable or registry-backed categories such as:

- players
- hostile mobs
- passive mobs
- animals
- monsters
- projectiles
- dropped items
- vehicles
- villagers
- bosses
- decorative entities

Avoid hardcoding behavior for every entity type.

---

# Search Results

Each result should include:

- entity identifier
- UUID when available
- entity type
- position
- distance
- dimension
- alive state when known
- velocity when known
- display name when known
- player name when applicable
- last update timestamp or tick
- relevant metadata
- staleness status

Do not return direct mutable entity references.

---

# Result States

Differentiate:

- found
- no match in known loaded entities
- entity state unavailable
- stale result
- invalid query
- cancelled

Do not claim an entity does not exist outside loaded chunks.

---

# Ordering

Support deterministic ordering by:

- nearest
- farthest
- entity ID
- name
- type

Tie-breaking must be stable.

---

# Performance

Requirements:

- avoid locking WorldState repeatedly
- use a consistent snapshot
- allow early exit for exact ID or UUID
- avoid large unnecessary allocations
- enforce maximum result limits
- support cancellation for large searches
- do not scan raw chunk blocks

---

# Player Lookup

Player lookup should support:

- exact case-sensitive match
- exact case-insensitive match
- optional prefix match
- UUID match

Avoid ambiguous selection unless explicitly allowed.

Return an ambiguity error when multiple candidates match.

---

# Commands

Add read-only commands such as:

- \`findentity <entity-type> <radius>\`
- \`findplayer <name>\`
- \`nearby entities <radius>\`
- \`nearby players <radius>\`
- \`nearby hostiles <radius>\`
- \`entity info <id>\`

Do not navigate or interact.

---

# Configuration

Support named entity groups such as:

- hostile
- passive
- farm_animals
- dangerous_projectiles
- villagers
- item_drops

---

# Testing

Add tests for:

- exact ID search
- UUID search
- type matching
- category matching
- player-name matching
- ambiguous player names
- radius boundaries
- deterministic ordering
- stale-result handling
- result limits
- cancellation

---

# Acceptance Criteria

- The project compiles.
- Known entities can be searched through structured queries.
- Player lookup is safe and deterministic.
- Entity groups are configurable.
- Results expose staleness and update information.
- Console inspection commands work.
- Tests pass.
- No movement, following, combat, interaction, or AI planning is implemented.

At completion, update implementation status and summarize the query model, commands, tests, and entity-data limitations.
