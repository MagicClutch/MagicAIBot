# Storage Deposit and Withdraw

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect StorageIndex, ContainerInteraction, InventorySorting, InventoryState, navigation, and item matching.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement global storage optimization, automatic chest construction, shulker-box logistics, resource gathering, or AI planning.
6. Reuse existing storage and container primitives.
7. Never assume stale storage contents are accurate.
8. Keep operations bounded, cancellable, and server-confirmed.

---

# Goal

Implement high-level deposit and withdrawal actions using known storage containers.

The system should select a suitable known container, navigate to it, open it, perform a requested transfer, update StorageIndex, and close it safely.

---

# Deposit Request

Support:

- exact item
- item group
- requested count
- all matching items
- excluded slots
- protected items
- preferred storage label
- maximum search distance
- allow partial completion
- timeout
- cancellation

---

# Withdrawal Request

Support:

- exact item
- item group
- requested count
- preferred storage label
- maximum search distance
- allow partial completion
- minimum item durability when applicable
- metadata-sensitive matching
- timeout
- cancellation

---

# Container Selection

Use StorageIndex.

Rank candidates deterministically by:

1. recently confirmed matching contents or capacity
2. preferred label
3. freshness
4. accessibility
5. distance
6. stable coordinate ordering

Do not rely on stale exact counts without reopening and validating the container.

---

# Deposit Candidate Rules

For deposits, prefer containers with:

- confirmed compatible partial stacks
- confirmed free slots
- preferred label
- recent observation
- reachable location

If capacity is stale or unknown, the system may inspect the container before deciding.

---

# Withdrawal Candidate Rules

For withdrawals, prefer containers with:

- recently confirmed matching items
- sufficient confirmed count
- preferred label
- recent observation
- reachable location

If no confirmed candidate exists, stale candidates may be inspected within a bounded limit.

---

# Multi-Container Operations

Allow bounded multi-container completion.

Requirements:

- configurable maximum containers per request
- update remaining count after every transfer
- avoid reopening the same failed container repeatedly
- stop when the request is satisfied
- return partial completion when limits are reached

---

# Execution

For each candidate:

1. Validate the storage block.
2. Navigate to it.
3. Open it with ContainerInteraction.
4. Refresh StorageIndex.
5. Recalculate the transfer.
6. Execute the server-confirmed transfer.
7. Refresh StorageIndex again.
8. Close the container.
9. Continue only if needed.

---

# Unexpected Changes

Handle:

- another player changing contents
- full container
- missing expected item
- container removed
- container inaccessible
- inventory becoming full
- container closing unexpectedly
- stale index
- dimension mismatch

Replan only within configured bounds.

---

# Protected Inventory

Never deposit:

- active task items
- protected hotbar items
- equipped armor
- protected item groups
- user-marked valuables
- items reserved through a lease

unless explicitly requested.

---

# Results

Return:

- completed
- partially completed
- no suitable storage
- item unavailable
- insufficient storage capacity
- candidate data stale
- all candidates unreachable
- inventory full
- timed out
- cancelled
- disconnected
- died

Include:

- actual transferred count
- containers inspected
- containers used
- remaining count
- failures by container

---

# Commands

Add commands such as:

- \`deposit minecraft:cobblestone 64\`
- \`deposit group ores all\`
- \`deposit label building minecraft:stone all\`
- \`withdraw minecraft:diamond 10\`
- \`withdraw group food 32\`
- \`storagetask stop\`
- \`storagetask status\`

---

# Configuration

Support:

- maximum search distance
- maximum containers inspected
- maximum containers used
- allow stale candidates
- allow partial completion
- close after each transfer
- preferred labels by item group
- protected items
- protected groups
- total timeout

---

# Logging

Log:

- storage request
- candidate ranking
- stale candidate inspection
- container opened
- transfer result
- index refreshed
- partial completion
- next candidate
- cancellation
- failure

---

# Testing

Add tests for:

- candidate ranking
- preferred labels
- stale candidate handling
- multi-container completion
- protected items
- capacity calculation
- partial deposit
- partial withdrawal
- candidate failure fallback
- cancellation cleanup
- index refresh

---

# Acceptance Criteria

- The project compiles.
- Items can be deposited into and withdrawn from indexed storage.
- Containers are selected deterministically.
- Stale information is validated before use.
- Multiple containers can be used within bounded limits.
- Transfers and index updates are server-confirmed.
- Protected items are respected.
- Tests pass.
- No chest construction, global optimization, gathering, or AI planning is implemented.

At completion, update implementation status and summarize selection policy, commands, tests, and limitations.
