# Chat and Console Interface

Before implementing anything:

1. Read instructions/GOALS.md.
2. Read this prompt.
3. Implement ONLY this prompt.

---

# Goal

Create the communication layer between the user and the AI.

No planning.

No execution.

Only communication.

---

# Minecraft Chat

Listen for all incoming chat.

Capture:

player

message

timestamp

chat type

Store them as structured events.

---

# Console

Create an interactive console.

Allow entering commands while the bot is running.

Examples:

Follow me

Get diamonds

Stop

Status

Inventory

Hello

Nothing should actually execute yet.

Only produce command events.

---

# Unified Command System

Both console and Minecraft chat should produce the same internal command type.

Example:

CommandReceived

source

sender

message

timestamp

Future systems should not care where the command originated.

---

# Chat Output

Create a reusable ChatService.

Support:

send_chat()

reply()

system_message()

Future modules should use this service instead of calling Azalea directly.

---

# Console Output

Create a ConsoleService.

Support formatted logging.

Readable status output.

Future command registration.

---

# Events

Emit events whenever a command is received.

No AI yet.

---

# Logging

Log:

incoming messages

outgoing messages

console commands

errors

---

# Acceptance Criteria

✓ Minecraft chat received

✓ Console input works

✓ Unified command model

✓ ChatService exists

✓ ConsoleService exists

✓ Event-driven architecture

Provide a summary of everything implemented.