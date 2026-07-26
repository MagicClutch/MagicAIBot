# Plan Executor

Read \`instructions/GOALS.md\`, inspect GoalManager, TaskRuntime, PlannerDataModel, and task factories, and implement only this prompt. Do not add gameplay actions or arbitrary code execution.

## Goal

Execute validated plans by mapping approved action names to typed registered task factories.

Reject unknown actions, validate parameters again, expose capability metadata, and never evaluate dynamic code, shell commands, or arbitrary server commands.

Support sequential/dependency execution, optional/fallback steps, bounded retries, per-step and goal timeouts, cancellation, and partial completion. Check deterministic preconditions, refresh relevant WorldState, detect already-satisfied outcomes, mark blocked steps, and do not silently alter plans.

Support failure policies: fail, skip optional, retry, fallback, partial completion, and explicit future replan request. Map current step, completion, blocks, retries, outputs, and warnings into GoalManager progress.

Add \`plan execute <goal-id>\`, \`plan stop <goal-id>\`, and \`plan execution status <goal-id>\`. Test dependencies, optional/fallback behavior, retries, cancellation, failure propagation, already-satisfied steps, partial completion, and unknown action rejection.
