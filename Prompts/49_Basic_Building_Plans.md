# Basic Building Plans

Read \`instructions/GOALS.md\`, inspect placement, breaking, inventory, gathering, planning, and WorldState. Implement only this prompt; no schematic import, automatic resource creation, or unbounded structures.

## Goal

Implement deterministic plans for parameterized walls, floors, platforms, pillars, boxes, simple rooms, staircases, and bridge segments.

Generate ordered operations with target/expected/desired block, support/access requirements, dependencies, optional breaks, and placements. Before execution validate dimensions/materials, protected areas, occupied/unsupported targets, entity intersections, and max volume.

Use TaskRuntime and existing block actions; support pause/resume/cancel/progress, already-correct skips, bounded retries, and partial completion.

Add build wall/floor/box/preview/stop/status commands with documented syntax. Test geometry, materials, dependency order, correct blocks, occupied targets, volume limits, cancellation, and partial completion.
