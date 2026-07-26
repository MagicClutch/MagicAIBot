# Long-Distance Navigation

Read \`instructions/GOALS.md\`, inspect local navigation, exploration, WorldState, survival, and runtime. Implement only this prompt; no Nether optimization, portals, boats, or Elytra. Reuse Azalea pathfinding and keep travel cancellable/checkpointed.

## Goal

Travel reliably toward distant overworld coordinates beyond local loaded areas.

Requests include destination, tolerance, maximum distance, timeout, sprinting, food threshold, checkpoint interval, unknown-terrain policy, and cancellation.

Use bounded intermediate waypoints, incremental terrain validation, refreshed routes as chunks load, last safe checkpoints, detour/oscillation limits, and total distance limits. Recover from path failure, updates, stuck state, hunger, configured night danger, reconnect/displacement, and unloaded destination chunks.

Expose remaining/travelled distance, waypoint, replans, safe checkpoint, and meaningful estimates. Test segmentation, checkpoints, detour limits, cancellation, reconnect, timeout, and tolerance.
