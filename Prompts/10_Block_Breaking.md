# Block Breaking

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect WorldState, BlockSearchService, navigation, look control, and inventory state already present.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement block placement, crafting, resource-goal planning, chest storage, or AI providers.
6. Reuse existing systems.
7. Do not create duplicate navigation, rotation, or block-search logic.
8. Make every action cancellable and recoverable.

---

# Goal

Implement a reusable low-level block-breaking action.

The bot should be able to break one explicitly selected block or one block selected through BlockSearchService.

This prompt is about the execution primitive, not autonomous resource gathering.

---

# Breaking Request

Create a typed request containing at least:

- exact block position or structured search query
- maximum search distance
- navigation allowed or required
- interaction range
- timeout
- preferred tool behavior
- whether drops should be collected
- whether dangerous blocks may be broken
- cancellation support

Defaults must come from configuration.

---

# Validation

Before breaking, verify:

- the bot is connected and alive
- the target dimension matches
- the target chunk is loaded
- the target block exists
- the target is not air
- the target is within configured allowed distance
- the block is not forbidden by safety configuration
- the bot has movement and interaction control
- the target can be reached or is already in range

Do not pretend success if the block changed before execution.

---

# Reach Target

Use existing navigation to reach a safe interaction position.

Use existing look control to face an appropriate point on the target block.

Do not duplicate those calculations.

If navigation is disabled and the block is out of range, return a clear failure.

---

# Tool Selection

Implement a minimal reusable tool-selection policy using currently available inventory information.

Choose the best available tool based on Minecraft suitability and expected break speed where Azalea exposes the required data.

At minimum:

- prefer pickaxes for appropriate blocks
- prefer axes for appropriate blocks
- prefer shovels for appropriate blocks
- prefer hoes where appropriate
- otherwise use a safe fallback

Do not craft missing tools.

Do not switch away from items protected by future policy unless configuration permits it.

Return the selected slot or explain why no suitable tool exists.

---

# Breaking Execution

Use normal Minecraft digging behavior.

Requirements:

- start digging
- continue for the correct duration or use Azalea's supported mechanism
- stop or abort correctly
- monitor the actual world state
- confirm the target block changed
- handle server rejection
- handle target replacement
- handle movement out of range
- handle tool breakage
- handle death and disconnect
- stop safely on cancellation

Do not instantly delete blocks or use server commands.

---

# Completion

A break action succeeds only after WorldState confirms the original target block is no longer present in its original state.

Possible outcomes should include:

- broken
- target already absent
- target changed
- target not found
- unreachable
- out of range
- forbidden block
- no suitable tool
- timed out
- cancelled
- disconnected
- died
- server rejected or state not confirmed

---

# Drop Collection

If \`collect_drops\` is enabled:

- perform only a small bounded movement toward nearby drops resulting from the block
- use loaded entity information
- stop after a configured timeout
- do not implement general item farming
- do not chase unrelated items indefinitely

If reliable drop ownership cannot be determined, document the limitation.

---

# Safety Configuration

Support configurable restrictions including:

- forbidden blocks
- protected block groups
- avoid breaking blocks with containers
- avoid breaking blocks below the bot
- avoid breaking blocks above the bot
- avoid opening lava or water when detectable
- maximum breaking distance
- maximum action timeout

The current project policy may default to acting freely, but technical safety controls must remain configurable.

---

# Commands

Add temporary commands such as:

- \`breakblock <x> <y> <z>\`
- \`breaknearest minecraft:stone 16\`
- \`breaknearest logs 32\`
- \`break stop\`
- \`break status\`

Commands should produce concise status and failure reasons.

---

# Logging

Log important phases:

- target selected
- navigating
- tool selected
- looking at target
- digging started
- target changed
- break confirmed
- cancelled
- failed

Avoid per-tick spam.

---

# Testing

Add tests for:

- validation
- target already absent
- forbidden blocks
- tool ranking
- out-of-range behavior
- target change during execution
- cancellation cleanup
- timeout
- disconnect/death cleanup
- completion confirmation
- bounded drop collection

Add integration tests where practical.

---

# Acceptance Criteria

- The project compiles.
- The bot can break a specified reachable block normally.
- Search, navigation, look control, and inventory information are reused.
- A suitable existing tool is selected.
- Success is confirmed from world state.
- Cancellation stops digging and movement.
- Failures are typed and logged.
- Tests pass.
- No placement, crafting, general gathering, or AI planning is implemented.

At completion, update implementation status and summarize files, behavior, tests, and server-specific limitations.
