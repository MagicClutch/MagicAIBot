# AI Provider Interfaces

Before implementing anything:

1. Read \`instructions/GOALS.md\`.
2. Inspect configuration, errors, logging, GoalManager, and command models.
3. Implement only this prompt.
4. Do not integrate a real provider, plan, or execute model output.
5. Never hardcode one provider into core systems.

## Goal

Define provider-neutral async interfaces for OpenAI, Gemini, Claude, local models, and future providers.

Represent capabilities including chat completion, structured output, tool calling, task decomposition, planning, classification, vision, long context, streaming, local execution, and context size.

## Provider API

Define an async provider interface for provider/model identity, capabilities, health checks, ordinary and structured requests, usage estimation, cancellation, timeout, retry classification, and provider error conversion.

Provider-neutral requests must contain system instructions, user content, structured context, expected schema, determinism controls, output limit, request ID, timeout, metadata, and privacy classification. Do not leak API-specific JSON through the project.

Responses must support text, structured output, tool-call proposals, usage, estimated cost, finish reason, model/provider, latency, request ID, warnings, and raw provider metadata only at the adapter boundary.

Use typed error classes: authentication, quota, rate limit, timeout, network, invalid request, unsupported capability, malformed response, safety refusal, outage, cancellation, and adapter failure.

## Safety, Tests, and Acceptance

Implement a deterministic mock provider with canned responses and injected failures. API keys come from configuration/environment, are redacted, excluded from status/history, and never logged.

Test capability matching, validation, structured parsing, timeout, cancellation, error conversion, usage, redaction, and mock behavior.

Provider-neutral traits/models and a deterministic mock must exist; core systems must contain no vendor assumptions. No real provider or planner is implemented. Update implementation status with future adapter guidance.
