# Furnace Execution

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect SmeltingKnowledge, ContainerInteraction, StorageIndex, InventoryState, navigation, BlockSearch, and inventory-operation ownership.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not gather missing inputs or fuel.
6. Do not build furnaces.
7. Do not implement long-term furnace arrays, automatic factory management, or AI planning.
8. Reuse existing container and transaction systems.

---

# Goal

Implement execution of furnace, blast-furnace, and smoker recipes using resources already present in the bot's inventory.

The bot should locate a known reachable station, insert input and fuel, wait for processing, collect output, and confirm results.

---

# Smelting Request

Create a typed request containing:

- output item or recipe identifier
- desired output count
- preferred station type
- whether alternative compatible stations are allowed
- whether navigation is allowed
- station search radius
- whether partial completion is allowed
- whether existing station contents may be used
- whether the bot should wait for all output
- timeout
- cancellation support

---

# Recipe and Requirement Resolution

Use SmeltingKnowledge.

Determine:

- selected recipe
- required input count
- required fuel burn time
- selected fuel
- number of operations
- expected output
- estimated time
- compatible stations

Do not duplicate smelting calculations.

---

# Station Selection

Search loaded known blocks for compatible stations.

Rank by:

1. compatibility
2. known empty or compatible state
3. reachability
4. distance
5. stable coordinate order

Do not use a station containing incompatible items unless explicitly allowed and safely recoverable.

---

# Opening and Inspecting

Use ContainerInteraction to:

- navigate to the station
- open it
- confirm station type
- inspect input, fuel, and output slots
- detect active processing
- identify incompatible contents

---

# Loading the Furnace

Use serialized inventory operations.

Requirements:

- place correct input
- place sufficient fuel
- avoid excessive fuel waste where practical
- respect protected fuel
- confirm slot changes
- stop on unexpected revision
- avoid stealing existing unrelated output unless configured

---

# Existing Contents

Handle:

- matching input already present
- compatible fuel already present
- completed matching output
- unrelated input
- unrelated output
- active incompatible recipe
- another player changing slots

Return a clear conflict when the station cannot be used safely.

---

# Waiting

Monitor authoritative furnace state.

Requirements:

- track progress when available
- calculate bounded expected completion time
- avoid busy polling
- allow cancellation
- detect station unload
- detect interface closure
- reopen only within a small retry limit
- allow emergency survival preemption
- do not hold movement input while waiting safely

---

# Collecting Output

Collect only output belonging to the requested compatible recipe when it can be determined safely.

Confirm:

- output count increased in player inventory
- furnace output decreased
- actual completed operation count

Report partial completion accurately.

---

# Cancellation Policy

Configuration should define whether cancellation:

- leaves input and fuel in the furnace
- attempts to recover unprocessed input
- attempts to recover fuel
- collects completed output
- closes the interface

Default to the safest non-destructive behavior.

---

# Results

Return:

- completed
- partially completed
- missing input
- missing fuel
- no compatible station
- station unreachable
- station occupied
- incompatible contents
- insufficient inventory space
- processing interrupted
- timed out
- cancelled
- disconnected
- died

Include:

- actual output collected
- input consumed
- fuel consumed
- station used
- elapsed time

---

# Commands

Add commands such as:

- \`smelt minecraft:iron_ingot 32\`
- \`smelt recipe <recipe-id> <count>\`
- \`smelt station furnace\`
- \`smelt stop\`
- \`smelt status\`

---

# Configuration

Support:

- station search radius
- station preference
- alternative stations
- allow partial completion
- use existing fuel
- use existing input
- cancellation recovery
- polling interval
- confirmation timeout
- total timeout
- retry limit

---

# Logging

Log:

- smelting requested
- recipe selected
- station selected
- station opened
- contents inspected
- input loaded
- fuel loaded
- processing started
- output collected
- partial completion
- cancellation
- failure

---

# Testing

Add tests for:

- station compatibility
- station ranking
- existing compatible contents
- incompatible contents
- fuel loading plan
- partial completion
- output confirmation
- timeout
- cancellation policy
- inventory revision conflicts
- ownership cleanup

---

# Acceptance Criteria

- The project compiles.
- The bot can execute supported furnace-like recipes using current inventory.
- SmeltingKnowledge and ContainerInteraction are reused.
- Inputs, fuel, and outputs are server-confirmed.
- Existing station contents are handled safely.
- Partial completion and cancellation are accurate.
- Tests pass.
- No resource gathering, furnace construction, factory management, or AI planning is implemented.

At completion, update implementation status and summarize supported stations, execution flow, commands, tests, and limitations.
