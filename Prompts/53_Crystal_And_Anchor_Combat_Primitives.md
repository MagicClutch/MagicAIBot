# Crystal and Anchor Combat Primitives

Read \`instructions/GOALS.md\`, inspect placement, interaction, melee, look, inventory, WorldState, and safety. Implement reusable primitives only, respecting rules/target restrictions and normal mechanics.

## Goal

Provide crystal placement/spawn selection/attack/removal/explosion confirmation plus conservative local explosion-risk estimates. Provide anchor placement, glowstone charging, dimension validation, permitted explosion interaction, and state confirmation; avoid safe-dimension misuse unless explicit.

Coordinate movement, rotation, placement, interaction, inventory, and combat; always clean up. Risk estimates use known distance, armor, health, obstruction, and positions and clearly mark uncertainty.

Test crystal validation/spawn/attack, anchor charging/dimensions, self-damage limits, restrictions, and cancellation. No complete PvP strategy.
