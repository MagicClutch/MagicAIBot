# Logging and Chat Cleanup

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect the current logging, console, chat, and command code.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement AI responses, planning, task execution, or gameplay features.
6. Refactor existing systems instead of creating parallel replacements.
7. Preserve existing behavior unless this prompt explicitly improves it.
8. Keep output readable and suitable for long-running production use.

---

# Goal

Clean up logging, Minecraft chat handling, and console output so the bot produces structured, readable, non-duplicated information.

This prompt should improve existing communication infrastructure before more gameplay features are added.

---

# Structured Logging

Standardize logging across the project.

Use the existing Rust logging stack if one is already present. Otherwise use an appropriate structured solution such as \`tracing\`.

Support:

- error
- warning
- info
- debug
- trace when useful

Every log entry should include relevant structured context where available, such as:

- component
- server
- username
- task ID
- command source
- sender
- dimension
- error category

Do not put secrets into logs.

---

# Logging Configuration

Add configuration for:

- global log level
- optional per-module levels
- human-readable console output
- optional file logging
- optional structured JSON logging
- timestamps
- optional ANSI color
- dependency log filtering

Defaults should be useful and not excessively noisy.

---

# Secret Redaction

Ensure that logs never expose:

- API keys
- access tokens
- passwords
- session tokens
- authentication responses
- full sensitive configuration values

Implement reusable redaction for configuration diagnostics.

---

# Chat Normalization

Normalize incoming Minecraft chat into a clean internal model.

Correctly distinguish when available:

- player chat
- system messages
- server announcements
- commands
- whispers
- action-bar messages
- unknown chat types

Preserve original structured content where useful, but also provide readable plain text.

---

# Duplicate Prevention

Ensure incoming and outgoing messages are not logged multiple times by overlapping systems.

There should be a clear owner for:

- raw protocol diagnostics
- normalized chat logs
- command logs
- outgoing chat logs
- console output

---

# Noise Filtering

Support configurable suppression or lower log levels for noisy events, such as:

- keepalive traffic
- repeated position updates
- chunk packet details
- dependency internals
- duplicate server messages

Do not silently discard errors.

---

# Console Presentation

Improve console readability.

Requirements:

- user-entered commands remain visually distinct
- Minecraft chat remains readable
- logs do not corrupt the input line
- shutdown messages are clear
- multiline errors are formatted sensibly
- status output is concise
- ANSI formatting can be disabled

Use a suitable input library only if necessary and compatible with async operation.

---

# Chat Safety

Before sending outgoing Minecraft chat:

- enforce protocol length limits
- split messages only when safe and configured
- prevent accidental blank messages
- handle disconnected state
- return typed errors
- rate-limit outgoing messages to prevent spam or kicks

Make limits configurable.

---

# Correlation

Introduce lightweight correlation identifiers for future goals and tasks.

Do not implement goals or tasks now.

The logging API should merely allow optional fields such as:

- request ID
- goal ID
- task ID

---

# Testing

Add tests for:

- secret redaction
- message normalization
- duplicate filtering where applicable
- length validation
- outgoing rate limiting
- blank-message rejection
- configuration parsing

---

# Acceptance Criteria

- Logging is centralized and structured.
- Console and Minecraft chat output are readable.
- Duplicate logging is removed.
- API keys and tokens are redacted.
- Outgoing chat is validated and rate-limited.
- Log behavior is configurable.
- Existing systems continue working.
- Tests pass.
- No AI or gameplay feature is implemented.

At completion, update implementation status and summarize the cleanup, configuration changes, and any migration notes.
