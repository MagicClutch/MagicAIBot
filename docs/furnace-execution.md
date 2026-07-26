# Furnace execution status

The executor is an event-driven coordinator over two existing-system boundaries:
`SmeltingKnowledge` resolves the immutable recipe/fuel plan and `FurnacePort`
serializes navigation, container opening, revision-bound clicks, observations, and
closing. It never crafts or places a station and never gathers inventory.

The pinned Azalea revision models furnace menus as three leading slots
(ingredient, fuel, result); blast furnaces have the same layout. Station block
identity remains separate from menu identity so a furnace-looking menu cannot
silently authorize the wrong loaded block. Every mutation carries the observed
menu revision and success is reported only after both player output gain and
furnace output decrease are observed.

`/smelt`, `/smelt recipe`, `/smelt status`, and `/smelt stop` are debug command
forms. The current application has no container-interaction transaction adapter,
recipe registry, or inventory-operation owner (the prerequisite Tasks 4, 10,
and 13 are not present on this branch), so the live command reports that limit
rather than issuing unsafe raw Azalea clicks. The state machine and mocked port
are ready to bind once those prerequisite interfaces land. Task 15 is therefore
not ready for live integration, but can consume `SmeltResult` without changing
the executor.
