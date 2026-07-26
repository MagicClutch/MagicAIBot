# Gemini Provider

Before implementing anything:

1. Read \`instructions/GOALS.md\`.
2. Inspect provider interfaces, configuration, redaction, logging, and HTTP infrastructure.
3. Implement only this prompt.
4. Integrate Gemini behind the provider-neutral interface.
5. Do not implement routing or planning; do not expose Gemini types outside the adapter.

## Goal

Implement a production-ready Gemini provider adapter for structured Minecraft planning requests.

Configure secure API-key lookup, model, endpoint, timeout, retries, output limit, temperature, structured-output support, safety settings, cost metadata, and enabled state. Never commit keys.

Translate shared requests for system instructions, user content, structured JSON output, optional conversation context, cancellation, and generation settings. Convert normal text, JSON, malformed JSON, blocked/truncated/empty output, usage, and finish reasons into the shared response model.

Implement bounded exponential backoff, rate-limit handling, retryable-error classification, timeout, cancellation, health checks, and response-size limits; never endlessly retry authentication or invalid requests. Make pricing configurable.

Log provider, model, request ID, latency, retries, usage, and error category without keys, full private prompts, or raw responses at normal log levels.

Test mocked HTTP text, structured JSON, malformed JSON, authentication, rate limits, retries, timeout, cancellation, blocks, usage conversion, and redaction.

Gemini must work through AIProvider with typed errors/retries and protected secrets. No router, planner, or gameplay integration is implemented. Document required configuration.
