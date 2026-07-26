# Prompt Execution Standard

Every numbered implementation prompt in this directory is governed by this standard in addition to its feature-specific requirements.

## Required Reading and Scope Control

Before changing code:

1. Read \`Prompts/Goals.md\` completely.
2. Read this standard completely.
3. Read the requested numbered prompt completely.
4. Inspect the existing modules, configuration, tests, commands, and public APIs that the prompt names.
5. Inspect the exact dependency versions and official API surface before using an external API.
6. Implement only the requested capability and only the smallest prerequisite extensions needed to integrate it safely.

Do not rebuild working architecture, introduce parallel controllers, duplicate state caches, duplicate command parsers, or bypass established ownership, task, event, logging, authorization, and configuration systems.

## Implementation Invariants

Every implementation must:

- use typed public request, result, error, snapshot, and status models;
- keep third-party/ECS/protocol types behind adapter boundaries;
- avoid mutable global state;
- avoid holding locks or ECS/world borrows across \`.await\`;
- use bounded queues, scans, retries, timers, histories, and caches;
- treat missing, unloaded, stale, optional, and server-unconfirmed information as explicitly unknown;
- use a cancellation path that releases all acquired resources;
- ensure replacement commands and stale asynchronous completions cannot mutate newer operations;
- preserve session boundaries across disconnect, reconnect, death, dimension changes, and shutdown;
- use exact normalized Minecraft identifiers and stable UUID/entity identity where applicable;
- retain existing user changes outside the requested scope.

## State, Ownership, and Concurrency

The source of truth is the existing WorldState/InventoryState and their immutable snapshots. Actions must not invent state from requested inputs or packet sends.

Use existing resource leases and task ownership. At minimum, define/document whether a feature reads world state, controls movement, controls rotation, mutates inventory, uses an interaction hand, opens a container, or controls combat. Incompatible ownership must fail or queue through the established runtime; it must never race.

Every asynchronous operation must carry operation/session identity or an equivalent cancellation-generation check. On cancellation, disconnect, death, or replacement, stop inputs/use actions, release leases, clear temporary targets, and ignore stale results.

## Validation and Server Confirmation

Validate inputs at the command/API boundary, then revalidate mutable world assumptions immediately before side effects.

For a world-changing action, success requires authoritative observable confirmation—normally WorldState, InventoryState, a verified container state, or a supported server event. A packet/API call returning successfully, elapsed time, animation, or local prediction alone is never proof of success.

## Errors and Logging

Expected operational failures must use structured, user-actionable errors. Preserve the underlying cause for debugging and make exactly one parent operation responsible for final user-facing failure output.

Log only meaningful lifecycle transitions at normal level. Per-tick, packet, scan, retry, and calculation diagnostics belong behind debug logging. Logs must never be implicitly sent to Minecraft chat and must redact secrets, tokens, private prompts, and sensitive configuration.

## Configuration

Every behavior affecting safety, cost, retrying, limits, timing, permissions, or data retention must have validated configuration and sensible defaults. Validate upper and lower bounds at startup. A missing optional section must use documented defaults; an unsafe or impossible value must fail configuration validation clearly.

## Tests and Validation

Each prompt must add focused deterministic tests using mocks/adapters where a live server is not required. Cover:

- valid primary flow;
- input/configuration validation;
- state transition and idempotence behavior;
- cancellation and cleanup;
- stale operation/session protection;
- timeout/retry bounds;
- disconnect/death/reconnect behavior when relevant;
- server-confirmation success and failure paths;
- resource ownership conflict or preemption when relevant.

Do not claim mocked tests exercise real Minecraft networking, pathfinding, or provider APIs.

Run the project’s applicable formatter, compiler checks, linter, and tests. Add a concise manual test matrix for server-dependent behavior.

## Documentation and Handoff

Update implementation status and document:

- public API and command changes;
- configuration defaults and safety limits;
- data ownership and lifecycle;
- tests run and results;
- known limitations and deliberately deferred behavior;
- exact manual verification steps.
