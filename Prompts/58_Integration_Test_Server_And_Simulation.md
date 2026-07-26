# Integration Test Server and Simulation

Read \`instructions/GOALS.md\`, inspect tests, mocks, runtime, and configuration. Implement only this prompt; add no gameplay behavior, require reproducibility, and never require paid AI by default.

## Goal

Create integration testing for unit, simulated-state, mocked Azalea events, optional local Minecraft server, and mocked providers.

Document local tooling to start/reset/stop a compatible deterministic server/world, create fixtures, connect the bot, and capture logs; containers are optional.

Cover connection/reconnect, chat, movement/search/navigation, breaking/placement/eating, inventory/crafting/container/smelting/gathering, following/combat test mobs, mock-AI goals, cancellation, and survival preemption.

Control seeds, timeouts, coordinates, fixtures, responses, server version, and config. CI runs formatting/linting/unit/non-server integration/security checks where practical; server tests can be optional. Document test commands.
