# Cave Exploration

Read \`instructions/GOALS.md\`, inspect local exploration, mining, navigation, placement, survival, and WorldState. Implement only this prompt; no persistent cave maps and keep work bounded.

## Goal

Explore loaded cave networks session-locally while maintaining a return route.

Build a temporary graph of junctions, passages, vertical changes, dead ends, hazards, resource observations, return edges, and safe points. Prefer unvisited reachable passages, mark dead ends, avoid hazards, maintain return paths, and stop at depth/distance/duration/resource limits or inventory/food thresholds.

When configured and torches are available, place lights at bounded intervals, avoid duplicates, reserve a minimum count, and never auto-craft torches.

Report resources found/collected, passages, hazards, deepest point, and return success. Test traversal, junctions, dead ends, return, lighting, hazards, bounds, and cancellation.
