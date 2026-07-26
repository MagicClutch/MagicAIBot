# Planner Data Model

Before implementing anything:

1. Read \`instructions/GOALS.md\`.
2. Inspect GoalManager, TaskRuntime, ResourceRequirementModel, classifier, and actions.
3. Implement only this prompt.
4. Do not call AI providers or execute plans.
5. Keep the representation provider-neutral.

## Goal

Define the typed, versioned plan model used to turn goals into executable task graphs.

A plan includes ID, goal ID, version, summary, assumptions, constraints, required capabilities, ordered/dependent steps, expected outputs, completion conditions, failure policies, cost estimate, risk warnings, creation source, and validation state.

A step includes ID, action type, typed parameters, dependencies, preconditions, expected result, bounded retry policy, timeout, priority, optional/required state, resource needs, and fallback IDs.

Map only registered actions such as navigation, look, block interaction, eating, entity interaction, follow, combat, craft, smelt, collection, gathering, tree chopping, deposit, and withdraw. Unknown actions fail validation.

Validate dependencies, cycles, duplicate/unreachable steps, unsupported actions, invalid parameters, unbounded retries, impossible timeouts, conflicting resources, and missing completion conditions.

Support stable JSON (or equivalent) serialization, schema versioning, and never deserialize directly into executable closures.

Test valid sequential/dependency graphs, cycles, unknown actions, invalid parameters, optional/fallback steps, schema versioning, and stable serialization. No AI or execution is implemented.
