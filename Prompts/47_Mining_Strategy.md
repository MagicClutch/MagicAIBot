# Mining Strategy

Read \`instructions/GOALS.md\`, inspect gathering, search, breaking, placement, inventory, survival, and planning. Implement only this prompt; no branch mining/cave mapping or reckless breaking.

## Goal

Provide safe targeted mining of visible or locally reachable ore blocks/resources.

Requests include target/group, count, radius, depth change, block limit, tunnel/support permissions, lava policy, timeout, and cancellation.

When access is needed, identify minimal validated blocking blocks, avoid known lava/falling hazards, preserve standing space, optionally use configured support blocks, and never mine straight down by default. After one ore, scan bounded connected veins, choose safe order, update per break, collect drops, and stop at count.

Protect against lava, falling blocks, suffocation, drops, tool breakage, full inventory, and lost return paths. Test veins, access blocks, lava/falling rejection, limits, partial completion, tools, and cancellation.
