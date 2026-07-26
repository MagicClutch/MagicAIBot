# Natural Language Command Classifier

Before implementing anything:

1. Read \`instructions/GOALS.md\`.
2. Inspect unified commands, GoalManager, AI Router, and authorization.
3. Implement only this prompt.
4. Do not create execution plans or execute gameplay actions.
5. Prefer deterministic local parsing for obvious commands before AI.

## Goal

Classify incoming natural-language messages into structured command intents.

Support conversation, status, stop/cancel, follow, move, gather, craft, smelt, build, combat, protect, explore, inventory, storage, and unknown complex-goal categories without requiring every future intent to exist.

Pipeline: normalize input, apply exact parser, apply deterministic patterns, invoke AI Router only for remaining ambiguity, validate structured output, and return intent/confidence.

Results include intent, entities/items/quantities/names/coordinates/constraints, urgency, confidence, parser source, ambiguities, and original text.

Classification never bypasses authorization; mark sensitive attack-player, discard, destructive building, disconnect, chat, and server-command intents. Control cost by avoiding AI for clear commands such as stop, status, follow Steve, eat, or gather 32 cobblestone; retain no personal learning across restarts.

Add \`classify <text>\`, \`classify debug <text>\`, and \`classifier stats\`, with no secret exposure.

Test commands, quantities, identifiers, names, coordinates, ambiguity, AI fallback, malformed AI, confidence thresholds, and cancellation priority. No plan/action is executed.
