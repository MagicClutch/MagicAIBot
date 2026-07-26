# Goal Manager

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect TaskRuntime, unified commands, logging, events, and existing action APIs.
3. Implement only this prompt; do not integrate external AI providers, generate plans from natural language, or add gameplay capabilities.
4. Reuse TaskManager rather than creating a second execution system.
5. Keep all goal state session-scoped.

## Goal

Implement a central GoalManager representing high-level user objectives. A goal describes what the user wants; tasks describe how work is executed.

## Goal Model and States

Every goal needs a unique ID, type, original request, normalized description, source/requester, timestamps, priority, status, progress, linked task IDs, parent ID, cancellation token, optional deadline, structured result, warnings, and failure reason.

Support: received, validating, ready, planning, executing, paused, blocked, cancelling, cancelled, succeeded, partially succeeded, and failed. Reject invalid transitions.

## Goal Manager

Support creating, validating, starting, pausing, resuming, cancelling, querying, listing active/recent goals, attaching tasks, updating progress, completing/failing goals, and clearing history.

Goals must submit and monitor tasks through TaskManager. One goal may own several tasks; propagate child failures and cancellation, retain partial results, derive progress from measurable task progress, and retain completed task links in session history.

Represent blocked goals explicitly, including missing resources, unsupported capabilities, no known target, unavailable required provider, authorization denial, and unavailable world data. Do not retry blocked goals continuously without a policy.

Support configurable priorities. Higher priorities may advance queued work and preempt compatible lower-priority work, but never bypass critical SurvivalMonitor actions.

## Commands and Events

Add \`goals\`, \`goal status <id>\`, \`goal cancel <id>\`, \`goal pause <id>\`, \`goal resume <id>\`, \`goal history\`, and \`goal clear completed\`. Do not parse arbitrary natural language into plans yet.

Emit created, validated, planning-requested, started, blocked, progress-changed, paused, resumed, cancelled, succeeded, partially-succeeded, and failed events.

## Testing and Acceptance

Test lifecycle transitions, task attachment, cancellation propagation, partial completion, blocking, priority ordering, history bounds, progress aggregation, and shutdown cleanup.

Goals and tasks must stay clearly separated, use TaskManager, remain session-scoped, expose commands/events, and pass tests. Update implementation status with architecture, commands, and tests.
