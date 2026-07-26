# Navigate to Block

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect WorldState, MovementController, and BlockSearchService.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not break or place blocks.
6. Do not implement a general AI planner, resource gathering, crafting, combat, or long-distance exploration.
7. Reuse existing movement, state, search, event, logging, and error systems.
8. Keep navigation cancellable and recoverable.

---

# Goal

Implement the first bounded autonomous navigation behavior: locate a block in loaded world data and move the bot to a safe reachable interaction position near it.

This is not yet a complete universal pathfinding system.

---

# Navigation Request

Create a typed request containing at least:

- block search query or exact target position
- desired interaction range
- maximum navigation distance
- timeout
- whether sprinting is allowed
- whether jumping is allowed
- acceptable final distance
- cancellation token or equivalent

---

# Navigation Result

Return a structured result such as:

- reached
- already in range
- target not found
- target disappeared
- no reachable position
- path not found
- timed out
- cancelled
- disconnected
- died
- movement control unavailable
- world data unavailable

Include useful context:

- selected target
- final position
- distance travelled
- elapsed time
- number of replans

---

# Target Selection

When given a block query:

1. Search using BlockSearchService.
2. Consider candidates in deterministic nearest-first order.
3. Determine whether each candidate has a safe interaction position.
4. Attempt navigation to a reachable candidate.
5. If a candidate cannot be reached, try another candidate within configured limits.

Do not assume that the nearest block is reachable.

---

# Interaction Position

Do not navigate into the target block.

Find a nearby standing position that:

- has sufficient player space
- has a safe supporting block
- is within configured interaction range
- is not lava
- does not require standing inside a solid block
- avoids obviously dangerous drops when detectable
- belongs to loaded world data

Keep this logic reusable for future breaking and placement.

---

# Pathfinding

Use Azalea's supported pathfinding or movement facilities where appropriate.

Wrap them behind the project's navigation interface.

Do not expose Azalea-specific implementation details throughout the codebase.

Requirements:

- start navigation
- monitor progress
- detect completion
- detect failure
- support cancellation
- stop movement on all exits
- handle target changes
- handle disconnect and death
- avoid blocking the runtime

---

# Replanning

Re-evaluate when:

- the target block changes or disappears
- the bot becomes stuck
- the path becomes invalid
- the bot is displaced
- relevant chunks unload
- movement makes no progress for a configured interval

Limit retries to avoid infinite loops.

---

# Stuck Detection

Track actual WorldState positions over time.

Detect lack of meaningful progress.

Do not infer progress only from movement inputs.

Log concise diagnostics when stuck.

---

# Navigation Ownership

Acquire the existing movement-control lease.

Release it on:

- success
- failure
- cancellation
- timeout
- disconnect
- death
- panic-safe cleanup where possible

Never leave movement active.

---

# Commands

Add temporary commands such as:

- \`goto block minecraft:stone 16\`
- \`goto block minecraft:diamond_ore 64\`
- \`goto pos <x> <y> <z>\`
- \`nav stop\`
- \`nav status\`

Use the unified command architecture.

Only one navigation request should control the bot at a time unless the architecture already supports task scheduling.

---

# Configuration

Add settings for:

- default search radius
- maximum search radius
- navigation timeout
- stuck timeout
- maximum replans
- sprinting
- jumping
- interaction range
- dangerous-block list

---

# Testing

Add unit or integration tests for:

- interaction-position selection
- unsafe floor rejection
- insufficient headroom rejection
- deterministic candidate selection
- target disappearance
- cancellation
- timeout
- stuck detection
- cleanup and movement release
- already-in-range behavior

Use a real test server only where necessary.

---

# Acceptance Criteria

- The project compiles.
- The bot can find a loaded block and move near it.
- It stops within a usable interaction range.
- Unsafe standing positions are rejected.
- Navigation is cancellable.
- Stuck detection and bounded replanning work.
- Movement is stopped and ownership released on every exit.
- Tests pass.
- No breaking, placing, general AI planning, or resource gathering is implemented.

At completion, update implementation status and summarize files, navigation behavior, tests, and known limitations.
