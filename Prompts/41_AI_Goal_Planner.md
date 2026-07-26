# AI Goal Planner

Read \`instructions/GOALS.md\`, inspect GoalManager, AIRouter, PlannerDataModel, ResourceRequirementModel, and capability registry, and implement only this prompt. Do not execute generated plans; validate all model output and keep prompts provider-neutral.

## Goal

Convert complex goals into validated structured plans. Gemini may be preferred only through configuration.

Provide relevant, bounded context: normalized goal, capabilities, player/inventory/world/storage summaries, recipe requirements, restrictions, risk policy, and task limits. Instruct the model to return only the current schema, registered actions, dependencies, completion conditions, assumptions, and bounded retries.

Pipeline: generate, parse structured output, validate schema/actions/bounds, request at most one repair, then reject invalid results. Never execute invalid plans.

Use AIRouter with structured-output capability, cost/timeout/fallback limits, and configurable planning preference. Default to low randomness and record provider/model/schema/request ID, validation, and usage without chain-of-thought.

Add \`plan <goal text>\`, \`plan show <goal-id>\`, \`plan validate <goal-id>\`, and \`plan discard <goal-id>\`. Test valid plans, malformed JSON, unsupported actions, repair success/failure, timeout, fallback, cost limits, context trimming, and schema mismatch. No plan execution.
