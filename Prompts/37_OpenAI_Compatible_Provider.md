# OpenAI-Compatible Provider

Before implementing anything:

1. Read \`instructions/GOALS.md\`.
2. Inspect AI provider interfaces and the Gemini adapter.
3. Implement only this prompt.
4. Do not implement routing or planning.
5. Support configurable OpenAI-style endpoints and never hardcode one model.

## Goal

Implement an OpenAI-compatible adapter for hosted or local compatible chat/responses endpoints.

Configure key, base URL, model, optional organization/project, timeout, retries, output limit, temperature, structured output, tool calls, pricing, and enabled state.

Detect/configure plain chat, JSON-schema output, tool calls, streaming, reasoning options, and context size. Return clear errors for unsupported features.

Map shared requests/responses without leaking provider structures. Handle text, structured output, tool proposals, usage, finish reasons, refusal, incomplete output, and multiple content parts.

Allow official OpenAI, local compatible servers, self-hosted gateways, and other compatible vendors through isolated configuration.

Implement retry classification, bounded backoff, cancellation, timeout, size limits, secret redaction, and optional prompt logging only when explicitly enabled.

Mock successful text/JSON/tool calls, malformed output, rate limits, authentication, missing local usage, timeout, cancellation, and vendor error bodies.

The adapter must work through AIProvider with configurable endpoint/model, structured output/tool proposals, and no planner-facing vendor leakage. No router or planner is implemented.
