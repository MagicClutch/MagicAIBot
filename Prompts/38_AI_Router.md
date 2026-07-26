# AI Router

Before implementing anything:

1. Read \`instructions/GOALS.md\`.
2. Inspect AIProvider interfaces and adapters.
3. Implement only this prompt.
4. Do not implement Minecraft planning or execute model-proposed tools.
5. Prefer the lowest-cost healthy capable provider by default.

## Goal

Implement a configurable AI Router selecting the cheapest healthy capable provider.

Consider required capabilities, structured output, context/output lengths, privacy, latency preference, max cost, allow/deny lists, model override, and fallback.

Default routing filters disabled, unhealthy, incapable, context-incompatible, privacy-incompatible, and over-cost providers; then ranks by estimated cost with configured quality/latency tie-breakers and deterministic ordering.

On retryable failure, use bounded configured fallbacks, preserve request/correlation IDs, avoid repeating failed providers, enforce total timeout and attempt limits, and record attempts. Do not fallback after safety refusal unless configured.

Estimate token counts and expected/actual cost with timestamped configurable pricing. Maintain session-scoped health states: healthy, degraded, rate-limited, unavailable, authentication failure, and disabled; use cooldowns and bounded checks.

Add \`ai providers\`, \`ai health\`, \`ai route test <capability>\`, \`ai costs\`, and provider enable/disable commands without secrets.

Test cheapest capable selection, capability/context filtering, override, fallback, limits, refusal behavior, cooldown, costs, and deterministic ties. No planner or tool execution is implemented.
