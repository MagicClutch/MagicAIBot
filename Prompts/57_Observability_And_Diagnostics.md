# Observability and Diagnostics

Read \`instructions/GOALS.md\`, inspect logging, tasks, goals, AI Router, state, inventory, and errors. Implement only this prompt; expose no secrets, keep normal overhead low, and add no gameplay features.

## Goal

Provide production-grade diagnostics. Track connection uptime/reconnects, active goals/tasks, durations/failures, navigation failures, block actions, gathered items, AI requests/latency/tokens/cost/errors, inventory conflicts, and survival emergencies.

Expose bounded diagnostics containing current goal/task, resource leases, player/connection state, recent errors, provider health, queues, and practical memory data. Correlate command, goal, plan, task, and AI IDs.

Add diagnostics, diagnostics tasks/ai/world/inventory, metrics, and health commands. Optionally provide securely bound, disabled-by-default Prometheus, JSON snapshot, or structured diagnostics file export.

Test increments, bounded history, redaction, correlation, health, disabled exports, and failure diagnostics.
