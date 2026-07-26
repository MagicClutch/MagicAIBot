# Entity Interaction

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect EntitySearch, navigation, look control, inventory selection, and interaction ownership.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement combat, following, trading logic, breeding, taming strategy, or AI planning.
6. Reuse existing search, navigation, rotation, state, and ownership systems.
7. Do not duplicate entity lookup.
8. Confirm interactions through observable state when possible.

---

# Goal

Implement a reusable low-level entity-interaction action.

The bot should be able to interact normally with a selected entity using the main hand or offhand.

This prompt implements the interaction primitive only.

---

# Interaction Types

Support:

- normal interact
- interact at a specific entity-relative point when supported
- main-hand interaction
- offhand interaction
- optional held-item requirement

Do not implement attack interactions in this prompt.

---

# Interaction Request

Create a typed request containing at least:

- exact entity target or EntitySearch query
- interaction hand
- optional required item
- maximum search distance
- interaction range
- whether navigation is allowed
- whether moving-target tracking is allowed
- timeout
- retry limit
- cancellation support

---

# Target Resolution

When using a query:

1. search known loaded entities
2. reject ambiguous player matches
3. rank candidates deterministically
4. choose a valid candidate
5. retain stable identity using entity ID and UUID where available

Do not silently switch to another entity after interaction begins unless configured.

---

# Reach and Look

Use existing navigation and look control.

Requirements:

- navigate to a safe interaction position
- account for entity movement
- remain within protocol interaction range
- face the entity or interaction point
- re-evaluate if the target moves
- stop after bounded retries

Do not implement indefinite chasing.

---

# Item Selection

If a required item is specified:

- use ItemSelector
- confirm the correct hand and item
- restore previous selection when configured
- fail clearly if the item is unavailable

Do not craft or gather missing items.

---

# Interaction Execution

Perform the normal Minecraft entity-interaction packet or Azalea action.

Requirements:

- acquire interaction ownership
- send one interaction attempt
- wait for observable confirmation when available
- retry only within configured limits
- avoid packet spam
- handle entity despawn
- handle dimension change
- handle death and disconnect
- stop on cancellation

---

# Confirmation

Some entity interactions do not provide a direct success event.

Use the strongest available evidence, such as:

- GUI or container opened
- entity metadata changed
- inventory changed
- held item count changed
- server response received
- entity state changed
- configured interaction acknowledgment

When confirmation is impossible, return a distinct result such as \`interaction_sent_unconfirmed\`.

Do not report confirmed success without evidence.

---

# Results

Return outcomes such as:

- confirmed interaction
- interaction sent but unconfirmed
- target not found
- ambiguous target
- target disappeared
- target unreachable
- out of range
- item missing
- interaction busy
- server rejected
- timed out
- cancelled
- disconnected
- died

---

# Commands

Add temporary commands such as:

- \`interact entity <id>\`
- \`interact player <name>\`
- \`interact nearest <entity-type> <radius>\`
- \`interact offhand <entity-id>\`
- \`interact stop\`
- \`interact status\`

---

# Configuration

Support:

- default interaction range
- navigation timeout
- target movement tolerance
- retry limit
- confirmation timeout
- restore selected slot
- permitted entity groups
- forbidden entity groups

---

# Logging

Log:

- target resolved
- navigating
- target moved
- item selected
- interaction sent
- interaction confirmed
- interaction unconfirmed
- retry
- cancellation
- failure

---

# Testing

Add tests for:

- target resolution
- ambiguous player selection
- moving-target handling
- interaction range
- item requirement
- retry limits
- confirmation classification
- target disappearance
- cancellation cleanup
- disconnect/death cleanup

---

# Acceptance Criteria

- The project compiles.
- The bot can interact with a selected nearby entity.
- Search, navigation, look control, and ItemSelector are reused.
- Moving targets are handled within bounded limits.
- Confirmed and unconfirmed outcomes are distinguished.
- Interaction ownership prevents conflicts.
- Tests pass.
- No combat, trading strategy, following, or AI planning is implemented.

At completion, update implementation status and summarize supported interactions, confirmation methods, tests, and limitations.
