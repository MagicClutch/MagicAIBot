# Player Assistance and Guarding

Read \`instructions/GOALS.md\`, inspect follow, combat, resources, entity search, and authorization. Implement only this prompt; never attack unapproved players or create social learning.

## Goal

Implement session-scoped assistance for one selected player: follow, guard, gather requests, remain nearby, wait, and status.

Guarding maintains distance, detects allowed hostiles near the protected player, prioritizes immediate threats, avoids protected entities, returns after combat, and observes health/time limits. PvP assistance is disabled by default and separately authorized.

Bind UUID, display name, authorization, start, and duration; never switch ambiguous players. Add assist/guard/gather/wait/stop/status commands. Test identity, threats, protections, return, loss, cancellation, authorization, and timeout.
