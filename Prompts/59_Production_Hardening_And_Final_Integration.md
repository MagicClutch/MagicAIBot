# Production Hardening and Final Integration

Before implementing anything:

1. Read \`Prompts/Goals.md\` and \`Prompts/00_Prompt_Execution_Standard.md\` completely.
2. Read every completed prompt, the implementation-status document, the configuration example, and the current command reference.
3. Inspect the whole repository, including all public APIs, task/resource ownership, connection lifecycle, background tasks, test utilities, and feature flags.
4. Write an audit note before refactoring: identify the exact issue, affected modules, evidence, risk, intended smallest change, and verification method.
5. Implement only production hardening, integration cleanup, documentation, and missing tests. Do not add unrelated gameplay capabilities.
6. Preserve working interfaces unless a documented migration is essential. Remove duplication instead of placing another wrapper around it.
7. Keep every changed behavior configurable, observable, bounded, cancellable, and authorization-aware.

---

# Goal

Prepare the Minecraft AI for reliable long-running operation after the earlier systems have been implemented.

This prompt is an integration and correctness gate. It does not mean that every future Minecraft mechanic must be implemented. It means the capabilities that do exist must have one coherent lifecycle from command receipt through verified completion, cleanup, and reporting.

---

# Required End-to-End Lifecycle Audit

Trace and test each supported user-facing operation through this exact lifecycle:

\`\`\`text
input source
→ identity and authorization
→ command parser or classifier
→ validated request or goal
→ deterministic plan or validated AI plan
→ TaskRuntime submission
→ resource/lease acquisition
→ world and inventory preconditions
→ action execution
→ authoritative confirmation
→ progress/result reporting
→ resource release and bounded history
\`\`\`

For every link, verify:

- there is one authoritative owner;
- errors remain typed and preserve root causes;
- cancellation is possible before and during the action;
- stale operation/session completions cannot update newer work;
- local logs are never rebroadcast as Minecraft chat;
- success is not inferred merely from a sent packet, elapsed delay, or local prediction;
- reconnect, death, dimension change, and shutdown produce a safe terminal state.

Document any unsupported link as a limitation rather than silently bypassing it.

---

# Architecture and Dependency Review

Audit module boundaries and dependency direction. Resolve only justified issues, including:

- cyclic dependencies between application services;
- duplicate representations of player, inventory, world, task, or connection state;
- direct ECS/protocol types leaking through application-facing snapshots;
- duplicate command parsers or registries;
- duplicate navigation, look, interaction, selection, or task controllers;
- global mutable state;
- background tasks with no owner, cancellation handle, generation guard, or join/supervision path;
- blocking file/network/CPU work on async executor threads;
- locks, ECS borrows, or interior-mutable guards held across \`.await\`;
- unbounded vectors, maps, channels, caches, task histories, retry loops, block/entity scans, or diagnostic buffers;
- dependency internals treated as stable public APIs.

For each refactor, specify the previous owner, new owner, public migration path, compatibility impact, and regression tests. Do not rewrite a stable subsystem simply to make it look uniform.

---

# Lifecycle, Resource, and Cleanup Guarantees

Verify that the TaskRuntime, GoalManager, and all direct command entry points enforce one shared lifecycle:

- every task has identity, parent/goal linkage where applicable, cancellation, timeout, progress, result, and structured error;
- movement, rotation, interaction, inventory mutation, container access, combat, and exclusive control use declared leases;
- resource acquisition has a stable order and bounded, cancellation-aware waiting;
- preemption leaves the preempted task paused or terminal according to its contract, never silently running;
- cancellation propagates to children and releases all inputs, item use, inventory cursors/leases, containers, path goals, and look ownership;
- terminal tasks cannot re-enter, emit duplicate completion, or retain active child handles;
- completed history and diagnostics history are bounded by configuration;
- panics are logged/supervised and cannot permanently strand ownership.

Run targeted race tests for command replacement, cancellation during waits, disconnect during interaction, reconnect after cancellation, and shutdown while a task owns each resource category.

---

# Connection, Reconnect, and World Readiness

Audit the complete connection lifecycle:

\`\`\`text
Disconnected
→ Connecting
→ Authenticating
→ Joining
→ WaitingForLocalPlayer
→ WaitingForDimensionAndChunks
→ Ready
→ Disconnecting
\`\`\`

Requirements:

- listener registration is session-scoped and exactly once per live connection;
- every reconnect increments a session generation and invalidates old ECS references, task handles, cancellation tokens, and event handlers;
- no world-dependent command starts before bot entity, position, dimension, and minimum local chunk readiness are available;
- disconnect cancels active workflows, clears movement/path/look/interactions, invalidates stale targets, and marks snapshots unavailable rather than fabricating values;
- reconnect does not resume previous actions unless a future explicit resume policy exists;
- readiness and connection failures are reported once with a useful cause;
- chat/console commands remain registered across reconnects without duplicated dispatch.

Add reproducible tests for multiple reconnects, early command rejection, stale completion suppression, duplicate-listener prevention, and post-reconnect movement/navigation/interaction readiness.

---

# Data Integrity and Confirmation Review

For every world-changing feature that exists, inspect its final success condition:

| Capability | Required confirmation |
| --- | --- |
| Movement/navigation | Current WorldState position is inside realistic arrival tolerance and target constraints still hold. |
| Look/interaction | Final raycast/target state matches the required target policy. |
| Break | The selected exact block position changed from the expected state; never infer success from digging start. |
| Place | The requested exact target position contains the expected placed block; support face remains strict. |
| Inventory mutation | Authoritative inventory revision and source/destination stack changes match the operation. |
| Container transfer | Container and player inventory state both confirm the move. |
| Eat | Hunger and/or item count confirms consumption according to the action contract. |
| Craft/smelt | Expected output count increased and relevant inputs changed. |
| Entity interaction/combat | Use the strongest available observable state and explicitly distinguish unconfirmed send. |

Remove any generic success fallback that reports completion before confirmation. Normalize unknown, stale, unloaded, missing, and server-rejected state into typed outcomes.

---

# Configuration Validation and Safe Defaults

Create or consolidate startup validation for every enabled feature. Validate:

- required server/account/provider fields;
- numeric ranges, minimums, maximums, and cross-field relationships;
- retries, timeouts, queue sizes, cache/history limits, and scan radii;
- item, block, entity, recipe, and permission identifiers where validation data is available;
- provider capability/model/pricing configuration;
- command permissions and rate/cost limits;
- feature dependencies and incompatible feature combinations;
- log file, export file, and network binding paths;
- diagnostics/metrics endpoints, which must be disabled and loopback-bound by default.

Provide documented defaults. Fail fast for unsafe, impossible, or security-relevant configuration; report all independent validation errors together where practical. Redact every secret, token, password, authorization header, session value, and private prompt in errors and diagnostics.

---

# Security Review

Threat-model all supported inputs:

- console and Minecraft chat command sources;
- player identity spoofing and mutable display names;
- natural-language classifier ambiguity and prompt injection;
- AI provider requests/responses;
- local blueprint/config/import files;
- diagnostic/export endpoints;
- logs and task history;
- environment variables and secrets.

Requirements:

- UUID-based authorization when available; names are display-only fallback;
- authorization before goal/task creation and again before sensitive side effects;
- AI output may reference only registered actions and typed validated parameters;
- no arbitrary shell/server command execution, dynamic code evaluation, unsafe deserialization, or path traversal;
- bounded request/context/response/file/decompression sizes;
- rate, concurrency, cost, and retry limits per command source/identity;
- audit records contain decision, category, correlation IDs, and reason without secrets or unnecessary private chat;
- destructive, player-combat, high-cost, and shutdown actions require explicit configured permission.

Add negative tests for spoofed chat, permission bypass attempts, malformed plans/provider output, unsafe path input, oversized input, and secret redaction.

---

# Performance and Operational Health

Measure before optimizing. Profile and document representative workloads for:

- immutable WorldState and InventoryState snapshots;
- block/entity searches at configured maxima;
- navigation replans and movement updates;
- task submission, progress updates, and history eviction;
- event fan-out and reconnect cycles;
- inventory/container transactions;
- recipe/smelting indexes;
- AI context construction, routing, retries, and logging.

Correct material regressions by reducing duplicated scans, allocations, or lock duration. Do not add permanent indexes, caches, or background loops unless they have a bounded lifecycle, invalidation strategy, metrics, and a demonstrated need.

Expose low-overhead operational health: connection/session state, active goal/task counts, queue/resource contention, bounded recent error categories, provider health, and memory/task-growth indicators where practical.

---

# Documentation Completion

Update or create:

- README and quick start;
- architecture and ownership guide;
- module/API guide;
- complete configuration reference with safe examples;
- provider setup and secret-handling guide;
- command and permission reference;
- task/goal/plan lifecycle guide;
- testing and local server guide;
- diagnostics, troubleshooting, and log interpretation guide;
- security model and threat boundaries;
- known limitations and unsupported mechanics;
- contribution and release-checklist guide.

Each documented command must identify authorization requirements, side effects, cancellation command, success/failure semantics, and manual test example.

---

# Final Test Matrix

Run and record results for:

1. formatter, compiler, linter, and all deterministic unit tests;
2. simulated WorldState, inventory, navigation, and interaction confirmation tests;
3. task/goal lifecycle, ownership conflict, cancellation, preemption, and stale-generation tests;
4. command authorization, chat self-message filtering, rate-limit, and no-log-rebroadcast tests;
5. provider mock, routing, cost limit, timeout, fallback, schema validation, and malformed-output tests;
6. disconnect/death/shutdown/reconnect tests, including repeated reconnects;
7. optional local-server integration scenarios using a documented deterministic fixture;
8. a bounded soak test that watches memory growth, active task count, retry count, and listener count;
9. configuration-validation and redaction tests.

Do not suppress flaky tests to make the build green. Diagnose whether flakiness comes from timing, shared state, external infrastructure, or an actual race; then make it deterministic or mark the environment-dependent test explicitly optional with documented prerequisites.

---

# Final Acceptance Criteria

- The project compiles, formats, and lints cleanly within the supported toolchain.
- Unit and supported integration tests pass with no known reproducible failure.
- Commands, authorization, goals, plans, tasks, and actions have one coherent lifecycle.
- No supported action bypasses typed validation, ownership, cancellation, or authoritative confirmation.
- AI routing is provider-neutral, cost-aware, permission-checked, and incapable of executing unregistered actions.
- Shutdown, cancellation, disconnect, death, and reconnect release all resources and reject stale work.
- Configuration, logs, diagnostics, task history, caches, and queues are bounded and secrets are redacted.
- Documentation accurately distinguishes implemented behavior, optional infrastructure, limitations, and unsupported mechanics.
- No duplicate major controller, parser, cache, task runtime, or world-state service remains.
- The bot remains deterministic except where explicitly configured, bounded, and seedable variation is required.

At completion:

1. update implementation status;
2. list every major subsystem and its maturity;
3. list open defects, limitations, and unsupported Minecraft mechanics;
4. list exact validation and manual-test commands with outcomes;
5. provide an honest production-readiness assessment, including blockers. Do not claim production readiness while unresolved critical safety, security, data-integrity, or resource-leak issues remain.
