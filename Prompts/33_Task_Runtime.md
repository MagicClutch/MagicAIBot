# Task Runtime

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect every existing action, ownership system, cancellation mechanism, command service, logging system, and event bus.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement an AI planner, Gemini integration, natural-language goal decomposition, persistent learning, or new gameplay capabilities.
6. Refactor existing actions into the shared runtime instead of creating duplicate wrappers.
7. Preserve working behavior.
8. Keep the runtime typed, asynchronous, observable, and production-ready.

---

# Goal

Implement the central runtime for executing, monitoring, cancelling, and composing the bot's existing actions.

The current project has multiple reusable actions. They now need one consistent task lifecycle.

This prompt implements task infrastructure, not intelligent planning.

---

# Core Task Model

Define a reusable task abstraction with lifecycle states such as:

- created
- queued
- waiting for resources
- running
- paused
- cancelling
- cancelled
- succeeded
- failed

Use names appropriate to the project.

---

# Task Identity

Every task should have:

- unique task ID
- task type
- creation timestamp
- start timestamp
- completion timestamp
- source command
- requesting user when available
- priority
- parent task ID when applicable
- optional goal ID for future use
- correlation ID for logging

---

# Task Trait

The task abstraction should support:

- validation
- start
- asynchronous execution
- cancellation
- status reporting
- progress reporting
- structured result
- typed error
- required resource leases
- cleanup

Avoid requiring every task to duplicate lifecycle code.

---

# Task Context

Provide a shared context containing references to existing services, such as:

- WorldState
- InventoryState
- MovementController
- navigation
- BlockSearch
- EntitySearch
- ItemSelector
- chat and console output
- configuration
- logging
- cancellation
- event publishing

Do not create global mutable singletons.

---

# Task Manager

Implement a central manager supporting:

- submit task
- start task
- cancel task
- query task
- list active tasks
- list recently completed tasks
- enforce concurrency rules
- apply priority
- propagate shutdown
- collect results
- remove old history entries

---

# Concurrency

Define clear concurrency policy.

Examples:

- only one movement-owning task at a time
- only one inventory-mutating task at a time
- read-only tasks may run concurrently
- survival emergencies may preempt lower-priority tasks
- chat responses may run independently
- incompatible resource leases prevent simultaneous execution

Use existing ownership systems rather than replacing them unnecessarily.

---

# Task Resources

Represent resources such as:

- movement
- rotation
- interaction
- inventory mutation
- container access
- combat
- exclusive player control

Tasks should declare required resources before execution where practical.

Prevent deadlocks through:

- stable acquisition order
- bounded waits
- cancellation-aware acquisition
- no locks held across unrelated waits

---

# Cancellation

Cancellation must:

- propagate to child tasks
- stop movement
- stop item use
- release inventory operations
- close containers when appropriate
- preserve partial results
- return a structured cancellation reason
- work during navigation, waiting, and inventory operations

---

# Parent and Child Tasks

Support composition without implementing intelligent planning.

A parent task should be able to run child tasks:

- sequentially
- conditionally
- with shared cancellation
- with result propagation
- with bounded retries

Do not implement arbitrary AI-generated task graphs yet.

---

# Progress

Provide typed progress information such as:

- current phase
- completed units
- total units when known
- percentage when meaningful
- current target
- last update timestamp
- current child task
- warning message

Do not invent percentages for tasks without measurable progress.

---

# Task History

Keep bounded in-memory history for the current process session.

Store:

- final state
- duration
- result summary
- error category
- partial progress

Do not persist learned behavior across restarts.

Optional plain operational logs are allowed, but task memory must remain session-scoped.

---

# Existing Action Integration

Integrate existing actions as task implementations or task-compatible operations, including where practical:

- navigation
- look at target
- block breaking
- block placement
- eating
- entity interaction
- following
- melee combat
- crafting
- container transfers
- smelting
- item collection
- block gathering
- tree chopping

Do not rewrite their internal logic unless necessary for lifecycle consistency.

---

# Commands

Add commands such as:

- \`tasks\`
- \`task status <id>\`
- \`task cancel <id>\`
- \`task cancel all\`
- \`task history\`
- \`task resources\`
- \`task active\`

Existing gameplay commands should submit tasks through TaskManager instead of launching unmanaged background actions.

---

# Events

Emit events such as:

- task submitted
- task queued
- task started
- task progress changed
- task paused
- task resumed
- task cancellation requested
- task cancelled
- task succeeded
- task failed
- task preempted

---

# Error Model

Distinguish:

- validation failure
- resource conflict
- execution failure
- timeout
- cancellation
- preemption
- dependency failure
- disconnected
- died
- internal error

Preserve underlying action errors.

---

# Panic Safety

A task panic must not permanently leave resources owned.

Use appropriate task supervision.

Log internal failures without crashing the entire bot where recovery is possible.

Do not hide programming errors.

---

# Configuration

Support:

- maximum queued tasks
- maximum task history
- default timeout
- resource wait timeout
- task priorities
- emergency priority
- completed-task retention
- progress update rate limit
- shutdown grace period

---

# Testing

Add tests for:

- lifecycle transitions
- invalid transitions
- queue ordering
- priority ordering
- resource conflicts
- stable resource acquisition order
- cancellation propagation
- child-task failure
- partial results
- emergency preemption
- task-history bounds
- shutdown cleanup
- panic resource release

---

# Acceptance Criteria

- The project compiles.
- Existing actions run through one central TaskManager.
- Task lifecycle and status are consistent.
- Resource conflicts are prevented.
- Cancellation propagates and cleans up correctly.
- Parent and child task composition works.
- In-memory task history is bounded.
- Commands expose task status and cancellation.
- Tests pass.
- No AI planner, Gemini integration, persistent learning, or new gameplay feature is implemented.

At completion, update implementation status and summarize architecture changes, migrated actions, commands, tests, and remaining migration work.
