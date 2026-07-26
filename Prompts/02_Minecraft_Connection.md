# Minecraft Connection

Before implementing anything:

1. Read instructions/GOALS.md.
2. Read this prompt.
3. Implement ONLY this prompt.

---

# Goal

Implement a reliable Minecraft connection layer using Azalea.

No AI yet.

No tasks yet.

Only networking and client management.

---

# Requirements

Support joining servers.

Support reconnecting after disconnects.

Support graceful shutdown.

Support clean startup.

Handle connection failures.

Handle authentication failures.

Handle kicked events.

Handle server disconnects.

---

# Client Manager

Create a reusable ClientManager.

Responsibilities:

connect()

disconnect()

reconnect()

is_connected()

current_server()

player_uuid()

player_name()

Expose a clean API.

---

# Events

Create an event system for:

Connected

Disconnected

Kicked

Respawn

Dimension Changed

Death

Spawned

These events should be reusable by future modules.

---

# Configuration

Read server information from config.

Do not hardcode anything.

---

# Logging

Log:

Connecting

Connected

Disconnected

Reconnect attempts

Failures

---

# Error Handling

Recover whenever possible.

Avoid crashes.

---

# Acceptance Criteria

✓ Bot joins a server

✓ Clean reconnect support

✓ Event system exists

✓ ClientManager reusable

✓ Fully documented

Provide a short implementation summary.