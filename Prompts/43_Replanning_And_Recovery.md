# Replanning and Recovery

Read \`instructions/GOALS.md\`, inspect the planner, executor, goals, and task errors; implement only this prompt and add no gameplay actions. Replanning must be bounded, cost-aware, and preserve completed work.

## Goal

Revise failed or blocked plans using current state and typed failures.

Support triggers such as vanished targets, missing resources/tools/stations, unreachable paths, full inventory, world changes, timeout, invalid assumptions, and explicit requests. Do not auto-replan programming errors.

Provide original goal/plan, completed/failed steps, error, world/inventory summaries, partial results, unavailable capabilities, retry history, and remaining cost/time. Revised plans must preserve completed outcomes, avoid known failures unless justified, use registered capabilities/bounded retries, respect budgets, and pass validation.

Configure maximum replans, minimum delay, AI cost, repeated failure category limits, and total goal timeout. Prefer deterministic recovery first: refresh stale state, retry transient disconnect once, choose another candidate, clear obsolete targets, or inspect stale storage.

Test preservation, repeated failures, deterministic recovery, valid/invalid revisions, exhausted cost/time, cancellation, and provider failure.
