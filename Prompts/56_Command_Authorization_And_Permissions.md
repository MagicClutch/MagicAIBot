# Command Authorization and Permissions

Read \`instructions/GOALS.md\`, inspect chat, console, classifier, goals, combat, building, and config. Implement only this prompt; prefer UUIDs, deny safely, and keep console policy explicit.

## Goal

Centralize authorization for every command source.

Support Minecraft UUID, display name, console/future API identities, roles, and permissions for status, movement, follow, inspection, gathering, crafting, storage, building, destructive breaks, mob/player combat, AI, cancellation, configuration, and shutdown.

Authorize before goal creation, prevent natural-language bypasses, require explicit player-combat permission, and configure console behavior. Add per-identity rate limits for commands, active goals, AI budget, combat, and chat. Audit requester/category/decision/source/denial/correlation without secrets.

Add permissions list/check/reload commands. Test UUID/name, denials, classifier bypass, limits, inheritance, console, AI budget, and player combat.
