# Local Exploration

Read \`instructions/GOALS.md\`, inspect navigation, WorldState, SurvivalMonitor, and TaskRuntime. Implement only this prompt; no long-distance biome/structure search or teleports. Keep exploration bounded and reversible.

## Goal

Reveal nearby chunks and search for resources/entities through deterministic local exploration.

Requests include origin, radius, duration, target, waypoint spacing, return policy, sprinting, stop-on-found, and cancellation. Implement expanding square, bounded spiral, radial spokes, or known-safe paths rather than random wandering.

Avoid known lava, void risks, excessive drops, unsafe water, and unloaded targets without incremental validation; SurvivalMonitor may preempt. Keep task-local visited waypoints, failed routes, observed chunks, found targets, and safe position. Support returns to start, last safe location, or requester.

Add local/findblock/findentity/stop/status commands. Test waypoints, bounds, target-found stop, failed routes, return, cancellation, survival preemption, and timeout.
