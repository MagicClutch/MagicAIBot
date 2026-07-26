# Project Foundation

Before implementing anything:

1. Read instructions/GOALS.md completely.
2. Follow every rule in GOALS.md.
3. Read this prompt completely.
4. Only implement this prompt.
5. Do NOT implement future prompts.
6. Reuse existing systems.
7. Do not duplicate code.
8. Keep the architecture production-ready.

---

# Goal

Create the complete project foundation for a long-term autonomous Minecraft AI using Rust and Azalea.

The goal is NOT to implement gameplay yet.

Only build the architecture.

---

# Requirements

Create a clean Cargo workspace.

Use modern Rust practices.

Separate the project into reusable modules.

Example:

src/

brain/
planner/
router/
memory/
executor/

world/
inventory/
navigation/
entities/
blocks/

tasks/

chat/
console/

config/

logging/

util/

main.rs

The exact structure may differ if you believe a better architecture exists.

---

# Configuration

Implement a configuration system.

Use TOML.

Support:

API keys

Minecraft account

Server IP

Logging

Debug mode

AI providers

Future expansion

---

# Logging

Create a centralized logging system.

Support

INFO

WARN

ERROR

DEBUG

Every future module must use this logger.

---

# Error Handling

Create common error types.

Avoid unwrap() except where absolutely impossible to fail.

Use Result consistently.

---

# Traits

Define the core traits used throughout the project.

Examples:

Task

Planner

AIProvider

Memory

Goal

Executor

Do NOT implement their behavior yet.

Only define interfaces.

---

# Dependencies

Choose modern crates where appropriate.

Only include dependencies that are actually useful.

Avoid unnecessary dependencies.

---

# Documentation

Document every public module.

Explain its responsibility.

---

# Acceptance Criteria

✓ Compiles successfully

✓ Clean architecture

✓ Config system exists

✓ Logging system exists

✓ Error handling exists

✓ Traits exist

✓ Ready for future prompts

At the end provide a short summary of everything created.