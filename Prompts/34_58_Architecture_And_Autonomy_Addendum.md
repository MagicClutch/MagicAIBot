# Prompts 34–58: Architecture and Autonomy Addendum

This addendum is mandatory for prompts 34–58 in addition to \`Goals.md\`, \`00_Prompt_Execution_Standard.md\`, and the named prompt. It turns the late-stage feature prompts into implementation-grade contracts without duplicating their entire feature descriptions.

## Cross-Cutting Contract

For every prompt in this range, define and test:

- typed request, result, progress, error, configuration, and immutable status/snapshot types;
- exact input normalization and validation at the public boundary;
- explicit owner for state, side effects, retries, and final user-facing result;
- session/operation generation checks for all async completion paths;
- resource leases and priority/preemption behavior through the existing runtime;
- authoritative success confirmation and stale/unavailable-state handling;
- bounded retry, timeout, scan, queue, memory, and cost policies;
- cancellation, disconnect, death, shutdown, and replacement cleanup;
- deterministic tests with mock adapters for external/server/provider dependencies;
- one concise manual test matrix with normal, failure, cancellation, and recovery cases.

## 34–35: Goals and Provider Foundation

### Goal Manager

Goals are intent-level state, not another task scheduler. A goal may own task IDs but may not directly own movement, inventory, interaction, or world mutations. State transitions are compare-and-set/idempotent; a terminal goal cannot restart. Progress is aggregated only from measurable child-task progress and must preserve unknown totals.

Blocked goals must include a machine-readable blocker category, recommended user action, and a retry policy. Cancelling a goal cancels only its active descendants, waits for bounded cleanup, preserves partial results, and records a terminal cancellation reason.

### Provider Interfaces

Provider requests must have privacy classification and stable request/correlation IDs. Provider adapters must never receive raw task handles, ECS handles, secrets outside the authentication boundary, or authority to execute actions. Structured output is valid only after schema validation. Usage/cost fields are optional and never fabricated.

## 36–38: Provider Adapters and Routing

### Gemini and OpenAI-Compatible Adapters

Implement adapters behind testable HTTP transport traits. Apply request/response byte limits before parsing, classify errors without matching fragile text when structured status is available, and make retries idempotency-aware. Cancellation must abort in-flight requests; deadlines include retries and backoff. Health checks must be lightweight, bounded, and never leak credentials.

### Router

Routing produces an immutable route decision: selected provider/model, rejected candidates with reasons, estimated upper cost, and fallback chain. Health is session-scoped with cooldowns. A provider may be selected only if it meets capability, privacy, context, budget, and authorization requirements. Never silently fall back to a provider with weaker privacy policy.

## 39–43: Intent, Plans, Execution, and Recovery

### Classifier

The classifier returns data only. It does not create a task, goal, plan, or side effect. Deterministic parsing takes precedence over AI. Low confidence or ambiguity is represented explicitly; dangerous intent always requires execution-time authorization even at high confidence.

### Planner Schema and AI Planner

Plan actions are enum-like registered capability references with typed parameters. Every plan has schema version, assumptions, preconditions, explicit completion criteria, bounded retries, and fallback policy. AI plans must be parsed as untrusted data, validated twice (schema and capability semantics), and be discarded rather than partially executed when invalid.

### Executor and Replanning

Executor converts only validated steps through registered factories. It records immutable step attempts, does not mutate plan content during execution, and treats already-satisfied steps as explicit outcomes. Replanning receives typed failure facts and completed outputs; it cannot erase completed work or exceed remaining time/cost/attempt budgets. Deterministic recovery must run before an AI call.

## 44–50: Autonomous Resource, Travel, and Building Work

### Resource Goals

Use actual inventory/storage counts as progress. Separate deterministic requirement expansion from optional AI planning. Every candidate source has freshness/confidence; stale storage is reopened/validated before transfer. Never generate a gather/craft/smelt step for an unsupported or unloaded capability.

### Exploration, Travel, Mining, and Caves

All navigation uses checkpointed task-local memory. Unknown terrain is not assumed safe. Do not issue concurrent paths; refresh destination only after threshold movement or meaningful world change. Return paths, danger budgets, food thresholds, and no-progress recovery are explicit. Mining and cave actions must preserve a safe standing position and return capability, never mine straight down by default, and keep failure memory task-local.

### Building and Schematics

Build plans are deterministic block-operation graphs. Validate materials, volume, protected regions, target state, support, collisions, and placement face before execution. A schematic parser has strict file/decompression/palette/volume limits and maps only known block states. Preview is side-effect free and must show unsupported blocks and estimated operations. Execution revalidates each changed block and reports partial completion.

## 51–56: Equipment, Combat, Assistance, and Authorization

### Equipment and Combat

Equipment ranking is deterministic, metadata-aware, and cannot replace protected/better equipment without explicit policy. Combat always uses stable entity identity, exact permissions, normal reach/cooldown/use rules, bounded prediction, and a survival-abort threshold. Crystal/anchor actions require server-confirmed placement/entity/block state and conservative self-risk limits; uncertainty reduces or blocks aggression according to configuration.

### Assistance and Permissions

Assistance binds to a requester UUID and expires. Guarding is limited to configured hostile targets; PvP is denied by default. Authorization is evaluated before goal creation, before expensive AI routing, and again before destructive execution. Rate/cost limits have explicit rejection behavior and audit records. No classification, plan, or task may bypass these checks.

## 57–58: Operations and Test Environment

### Observability

Metrics use bounded labels/cardinality, bounded error histories, and correlation IDs. Diagnostics must be safe under partial startup/disconnect and redact secrets by construction. Exporters are opt-in, access-controlled, and do not block action threads.

### Integration Harness

Separate deterministic unit/simulation tests from optional local-server tests. Server fixtures must fix version, seed, coordinates, players, permissions, and world reset procedure. Paid provider calls are never required. Tests must have per-test deadlines, captured structured logs, cleanup assertions, and clear environment prerequisites.
