# Magic AI Bot Commands

This document is the authoritative command reference for the current source code.
It covers three distinct but related command systems:

1. **Local console commands** — typed into the terminal while the bot is running. Prefixed with `/`.
2. **Minecraft chat AI requests** — typed in Minecraft chat with a configurable prefix (default `!`). Routed through Groq (OpenAI-compatible API) for natural-language planning.
3. **Groq internal tool calls** — tool-call JSON returned by the Groq provider. Validated and executed by the bot runtime.

These systems are **not** identical. Console commands offer direct deterministic control. Chat AI requests go through Groq. Groq tool calls are a strict subset validated by the registry.

---

## 1. Quick start

### Local console

```
/status
/players
/inventory
/goto 10 64 20
/follow 5cat
/gather oak_log 10
/stop
```

### Minecraft chat AI

```
!hi
!follow me
!give me 10 oak logs
!ai come to me
!stop
```

---

## 2. Local console commands

All commands are parsed in `src/console/commands.rs::parse_input` and dispatched in `src/app.rs::execute_console_input`. Every command in the table below is confirmed present in both the parser and the dispatcher.

### General

| Command | Aliases | Syntax | Description | Long-running | Cancel command | Status |
|---|---|---|---|---|---|---|
| `/help` | | `/help` | Print command reference | No | — | Implemented |
| `/status` | | `/status` | Show connection, position, inventory, uptime | No | — | Implemented |
| `/chat` | | `/chat <message>` | Send raw text to Minecraft chat | No | — | Implemented |
| `/players` | | `/players` | List nearby known players with distance/position | No | — | Implemented |
| `/inventory` | | `/inventory` | Show full inventory summary and slot details | No | — | Implemented |
| `/entities` | | `/entities [radius]` | List nearby entities (radius 1–256, default 64) | No | — | Implemented |
| `/reconnect` | | `/reconnect` | Disconnect, cancel all tasks, reconnect to server | No | — | Implemented |
| `/quit` | | `/quit` | Shut down the application | No | — | Implemented |

### Movement

| Command | Aliases | Syntax | Description | Long-running | Cancel command | Status |
|---|---|---|---|---|---|---|
| `/goto` | | `/goto <x> <y> <z>` | Walk to coordinates (no mining) | Yes | `/stop` or `/stopall` | Implemented |
| `/goto-mine` | | `/goto-mine <x> <y> <z>` | Walk to coordinates with mining-aware routing | Yes | `/stop` or `/stopall` | Implemented |
| `/follow` | | `/follow <name>` | Follow a player continuously | Yes | `/stop` or `/stopall` | Implemented |
| `/movement` | | `/movement` | Show movement state, destination, distance, elapsed | No | — | Implemented |
| `/path-status` | | `/path-status` | Show Azalea pathfinder status | No | — | Implemented |
| `/stop` | `/stopmovement` | `/stop` | Stop movement only; interaction/look may remain | No | — | Implemented |
| `/stopall` | | `/stopall` | Stop movement, interaction, look, tasks, gather, collector, AI | No | — | Implemented |

**Argument details for `/goto` and `/goto-mine`:**

```
x, y, z: integer coordinates (parsed as i32)
```

Example: `/goto 120 64 -35`

**Argument details for `/follow`:**

```
name: exact Minecraft player name (string, no whitespace)
```

### Block search and navigation

| Command | Aliases | Syntax | Description | Long-running | Cancel command | Status |
|---|---|---|---|---|---|---|
| `/findblock` | | `/findblock <id> [radius] [limit]` | Search loaded blocks by ID | No | — | Implemented |
| `/nearestblock` | | `/nearestblock <id> [radius]` | Find nearest loaded block (limit=1) | No | — | Implemented |
| `/gotoblock` | `/navigate-to-block` | `/gotoblock <id> [radius] [mine]` | Navigate to a matching block | Yes | `/cancelgotoblock` | Implemented |
| `/gotoblockstatus` | | `/gotoblockstatus` | Show block-navigation state, distance, attempts | No | — | Implemented |
| `/cancelgotoblock` | | `/cancelgotoblock` | Cancel block navigation | No | — | Implemented |

**Argument details for `/findblock`:**

```
id:     block identifier (e.g. oak_log, minecraft:stone)
radius: optional u32, default from config (default_radius, typically 32)
limit:  optional usize, default from config (default_result_limit, typically 20)
```

**Argument details for `/gotoblock`:**

```
id:     block identifier
radius: optional u32 search radius (default from block_navigation.default_search_radius)
mine:   optional literal "mine" — enables mining-aware routing
```

Block IDs are normalized by `normalize_block_id`: bare names get `minecraft:` prepended and are lowercased. `oak_log` becomes `minecraft:oak_log`.

### Look controls

| Command | Aliases | Syntax | Description | Long-running | Cancel command | Status |
|---|---|---|---|---|---|---|
| `/look` | `/lookat` | `/look <x> <y> <z>` | Look at a world position | Yes | `/lookstop` | Implemented |
| `/lookblock` | | `/lookblock <id>` | Look at a loaded block by ID | Yes | `/lookstop` | Implemented |
| `/lookplayer` | | `/lookplayer <name>` | Track a player with camera | Yes | `/lookstop` | Implemented |
| `/lookentity` | | `/lookentity <type>` | Look at an entity type (lowercased) | Yes | `/lookstop` | Implemented |
| `/lookstop` | | `/lookstop` | Cancel active look task | No | — | Implemented |
| `/lookstatus` | | `/lookstatus` | Show look state, target, precision, yaw/pitch | No | — | Implemented |

### Block interaction

| Command | Aliases | Syntax | Description | Long-running | Cancel command | Status |
|---|---|---|---|---|---|---|
| `/breakblock` | | `/breakblock` | Break the block in the crosshair | Yes | `/stopinteraction` | Implemented |
| `/break` | | `/break <x> <y> <z>` | Break a specific block | Yes | `/stopinteraction` | Implemented |
| `/breaknearest` | | `/breaknearest <id>` | Find, navigate to, and break nearest block | Yes | `/stopinteraction` | Implemented |
| `/select-tool` | | `/select-tool <id>` | Debug: score hotbar for a block type | No | — | Implemented |
| `/place` | | `/place <id>` | Place held block beside crosshair target | Yes | `/stopinteraction` | Implemented |
| `/place` | | `/place <x> <y> <z> <id>` | Place a block at specific coordinates | Yes | `/stopinteraction` | Implemented |
| `/placeblock` | | `/placeblock <id>` | Place block (block-first syntax) | Yes | `/stopinteraction` | Implemented |
| `/placeblock` | | `/placeblock <id> <x> <y> <z>` | Place block at coordinates (block-first syntax) | Yes | `/stopinteraction` | Implemented |
| `/stopinteraction` | | `/stopinteraction` | Cancel block interaction | No | — | Implemented |
| `/interactionstatus` | | `/interactionstatus` | Show interaction state, target, progress, distance | No | — | Implemented |

Note: `/place` argument order is `x y z id`, while `/placeblock` argument order is `id x y z`. Both produce the same `PlaceAt` variant when coordinates are given.

### Gathering and tasks

| Command | Aliases | Syntax | Description | Long-running | Cancel command | Status |
|---|---|---|---|---|---|---|
| `/gather` | | `/gather <target> <quantity>` | Gather a resource type or item | Yes | `/gathercancel` | Implemented |
| `/gatherstatus` | | `/gatherstatus` | Show gather/task progress | No | — | Implemented |
| `/gathercancel` | | `/gathercancel` | Cancel gather, preserve partial progress | No | — | Implemented |
| `/taskstatus` | `/tasks` | `/taskstatus` | Show movement, look, and workflow status | No | — | Implemented |
| `/task status` | | `/task status <id>` | Show details for a specific task | No | — | Implemented |
| `/task cancel` | | `/task cancel <id>` | Cancel a specific task | No | — | Implemented |
| `/task cancel all` | | `/task cancel all` | Cancel all active tasks | No | — | Implemented |
| `/task recent` | `/task history` | `/task recent` | Show recently completed tasks | No | — | Implemented |

**Argument details for `/gather`:**

```
target: one of:
        "logs" / "log"    → tree logs
        "stone"           → stone blocks
        "ores" / "ore"    → visible ores
        "food"            → food items
        any item ID       → specific item (e.g. "oak_log", "minecraft:diamond")
quantity: positive integer (> 0)
```

Example: `/gather logs 16`

### Collection, mining, food, and trees

| Command | Aliases | Syntax | Description | Long-running | Cancel command | Status |
|---|---|---|---|---|---|---|
| `/collect-item` | `/collectdrop` | `/collect-item <items> <count>` | Collect dropped items | Yes | `/collect-item stop` | Implemented |
| `/collect-item` | `/collectdrop` | `/collect-item nearest` | Collect nearest dropped item | Yes | `/collect-item stop` | Implemented |
| `/collect-item` | `/collectdrop` | `/collect-item group <group> <count>` | Collect by group (ores, logs, food) | Yes | `/collect-item stop` | Implemented |
| `/collect-item` | `/collectdrop` | `/collect-item status` | Show collection status | No | — | Implemented |
| `/collect-item` | `/collectdrop` | `/collect-item stop` | Cancel collection | No | — | Implemented |
| `/mine-ore` | `/mineore` | `/mine-ore <ore\|group> <count> [radius]` | Mine ore blocks | Yes | `/mine-ore stop` | Implemented |
| `/mine-ore` | `/mineore` | `/mine-ore status` | Show mining status | No | — | Implemented |
| `/mine-ore` | `/mineore` | `/mine-ore stop` | Cancel ore mining | No | — | Implemented |
| `/collect-food` | | `/collect-food` | Collect 1 food item | Yes | `/collect-food stop` | Implemented |
| `/collect-food` | | `/collect-food <item> <count>` | Collect N of a specific food item | Yes | `/collect-food stop` | Implemented |
| `/collect-food` | | `/collect-food value <points>` | Collect food worth N food points | Yes | `/collect-food stop` | Implemented |
| `/collect-food-status` | | `/collect-food-status` | Show food collector status | No | — | Implemented |
| `/collect-food-stop` | | `/collect-food-stop` | Cancel food collection | No | — | Implemented |
| `/chop-tree` | `/choptree` | `/chop-tree nearest` | Chop the nearest tree | Yes | `/chop-tree stop` | Implemented |
| `/chop-tree` | `/choptree` | `/chop-tree <type>` | Chop a tree of a specific type | Yes | `/chop-tree stop` | Implemented |
| `/chop-tree` | `/choptree` | `/chop-tree logs <n>` | Chop until N logs collected | Yes | `/chop-tree stop` | Implemented |
| `/chop-tree` | `/choptree` | `/chop-tree count <n>` | Chop N trees | Yes | `/chop-tree stop` | Implemented |
| `/chop-tree` | `/choptree` | `/chop-tree status` | Show chopping status | No | — | Implemented |
| `/chop-tree` | `/choptree` | `/chop-tree stop` | Cancel tree chopping | No | — | Implemented |

**Argument details for `/collect-item`:**

```
items: comma-separated item IDs (e.g. "diamond", "minecraft:diamond,minecraft:emerald")
count: positive integer
group: "ores", "logs", or "food"
```

Comma-separated items create an `AnyOf` filter. Single items use exact matching.

**Argument details for `/mine-ore`:**

```
ore:   ore name (lowercased, e.g. "diamond", "iron_ore", "minecraft:gold_ore")
       Values ending in "_ore" or starting with "minecraft:" are treated as exact IDs.
       Others are treated as group selectors.
count: positive integer
radius: optional u32, default 32
```

**Argument details for `/chop-tree`:**

```
type: tree type name (lowercased, e.g. "oak", "birch")
      Allowed types (default config): oak, birch, spruce, jungle, acacia,
      dark_oak, mangrove, cherry, pale_oak
n:    positive integer count
```

### Crafting and tools

| Command | Aliases | Syntax | Description | Long-running | Cancel command | Status |
|---|---|---|---|---|---|---|
| `/recipe` | | `/recipe <id>` | Show a read-only recipe from the versioned recipe book | No | — | Implemented |
| `/craft-check` | | `/craft-check <item> [count] [depth]` | Plan crafting with virtual inventory (read-only) | No | — | Implemented |
| `/craft` | | `/craft <item> <count>` | Submit crafting debug request | No | — | **Partial** (rejected at runtime) |
| `/craft` | | `/craft status` | Show craft status | No | — | Implemented |
| `/craft` | | `/craft stop` | Stop crafting | No | — | Implemented |
| `/ensure-tool` | `/craft-tool` | `/ensure-tool <id>` | Debug ensure-tool request | No | — | **Partial** (prints stub message) |
| `/testoaklog` | | `/testoaklog` | Break and restore nearest oak log (debug) | Yes | `/stopinteraction` | Implemented |

**Note on `/craft`:** The parser accepts the command, but the dispatcher prints a rejection message: `"this debug surface requires RecipeKnowledge to submit a resolved plan; it will not guess recipes or gather materials."` The `/craft status` and `/craft stop` subcommands work normally.

**Note on `/ensure-tool`:** The parser accepts the command, but the dispatcher prints: `"Ensure-tool planning is available, but no live crafting adapter is wired for {block_id}."` No action is taken.

**Argument details for `/craft-check`:**

```
item:  item identifier (e.g. "torch", "minecraft:stick")
count: positive integer (default 1)
depth: recipe recursion depth 1–64 (default 8)
```

**Argument details for `/recipe`:**

```
id: item identifier (e.g. "stick", "minecraft:torch")
```

### Containers

| Command | Aliases | Syntax | Description | Long-running | Cancel command | Status |
|---|---|---|---|---|---|---|
| `/open-chest` | | `/open-chest <x> <y> <z>` | Navigate to and open a chest | Yes | `/close-container` | Implemented |
| `/take-item` | | `/take-item <id> <count>` | Take items from open container | Yes | `/close-container` | Implemented |
| `/store-item` | | `/store-item <id> <count>` | Store items into open container | Yes | `/close-container` | Implemented |
| `/container-status` | | `/container-status` | Show ContainerService interaction status | No | — | Implemented |
| `/containerstatus` | | `/containerstatus` | Show read-only container observer state | No | — | Implemented |
| `/close-container` | | `/close-container` | Close the open container | No | — | Implemented |

**Important:** `/container-status` and `/containerstatus` are **different commands**. The former shows the interaction controller state (phase, target, transfers). The latter shows the read-only world-state container observer (open state, slots, revision, identity).

### AI

| Command | Aliases | Syntax | Description | Long-running | Cancel command | Status |
|---|---|---|---|---|---|---|
| `/ai` | | `/ai <request>` | Send a natural-language request to Groq | Yes | `/aicancel` | Implemented |
| `/aicancel` | | `/aicancel` | Cancel active AI session and its task | No | — | Implemented |
| `/aistatus` | | `/aistatus` | Show AI session status, requester, step count | No | — | Implemented |

---

## 3. Exact syntax reference

### Coordinates

All coordinate commands (`/goto`, `/goto-mine`, `/break`, `/look`, `/place`, `/open-chest`) accept three space-separated integers:

```
<x> <y> <z>
```

Values are parsed as `i32`. Example: `/goto 120 64 -35`

The AI `goto` action uses `f64` coordinates: `{"type":"goto","x":10.0,"y":64.0,"z":20.0}`

### Block and item identifiers

All commands that accept block/item IDs normalize through `normalize_block_id` or `normalize_item_id`:

- Bare names: `oak_log` → `minecraft:oak_log`
- Namespaced: `minecraft:oak_log` → `minecraft:oak_log` (lowercased)

No whitespace, no more than one `:` character.

### Quantities and counts

| Command | Field | Type | Bounds |
|---|---|---|---|
| `/gather` | quantity | u32 | > 0 |
| `/collect-item` | count | u32 | > 0 |
| `/mine-ore` | count | u32 | > 0 |
| `/mine-ore` | radius | Option<u32> | None or > 0 |
| `/chop-tree logs` | n | u32 | > 0 |
| `/chop-tree count` | n | u32 | > 0 |
| `/craft` | count | u32 | > 0 |
| `/craft-check` | count | u32 | > 0 |
| `/craft-check` | depth | usize | 1–64 |
| `/take-item` | count | u32 | > 0 |
| `/store-item` | count | u32 | > 0 |
| `/collect-food` | count | u32 | > 0 |
| `/collect-food value` | points | u32 | > 0 |
| `/entities` | radius | Option<u32> | None or 1–256 |
| `/findblock` | radius | Option<u32> | None or > 0 |
| `/findblock` | limit | Option<usize> | None or > 0 |
| `/nearestblock` | radius | Option<u32> | None or > 0 |
| `/gotoblock` | radius | Option<u32> | None or > 0 |

### Player names

Commands accepting `<name>` (e.g. `/follow`, `/lookplayer`) parse through `parse_follow_name`, which returns the argument as-is (trimmed). No validation against known players occurs at parse time.

### Entity types

`/lookentity <type>` lowercases the argument and searches for a matching entity in the current world snapshot.

---

## 4. Minecraft chat AI commands

### How the prefix works

The default prefix is `!`, configured in `config.toml`:

```toml
[ai.chat]
prefix = "!"
```

### Supported forms

```
!<request>
!ai <request>
!AI <request>
```

The processing in `src/ai/mod.rs::chat_request`:

1. Strip exactly the configured prefix (`!`).
2. Strip one optional `ai ` or `AI ` token from the remainder.
3. Trim whitespace if `strip_prefix_whitespace = true` (default).
4. Reject if the result is empty or exceeds `max_request_length` (default 500).

Examples:

| Chat message | After prefix removal | After `ai` removal | Final request |
|---|---|---|---|
| `!hi` | `hi` | `hi` | `hi` |
| `!follow me` | `follow me` | `follow me` | `follow me` |
| `!ai follow me` | `ai follow me` | `follow me` | `follow me` |
| `!AI gather oak logs` | `AI gather oak logs` | `gather oak logs` | `gather oak logs` |

### Special commands

| Request text | Behavior |
|---|---|
| `stop` | Cancels the active AI session (after prefix removal) |
| `cancel` | Cancels the active AI session (after prefix removal) |

These are checked after prefix removal but **before** Groq is called. They trigger `cancel_ai()` and respond with "Current task cancelled."

### Trusted requester

The sender's name and UUID from `ChatPacket` form the `AiRequester`. Words `me`, `my`, `I`, `myself`, `requester` in the follow action are resolved to the trusted sender name — **never** to a value from Gemini's output.

### Behavior details

- **Responses are public**: all `message_to_player` text is sent as a `[AI]`-prefixed Minecraft chat message when `respond_in_chat = true` (default).
- **Acknowledgement**: if `acknowledge_requests = true` (default), the bot sends "Request received." immediately.
- **Busy behavior**: if `ai.busy_behavior = "reject"` (the only current mode), a second request while one is active gets "I am already completing another task."
- **Rate limiting**: enabled by default. 3 requests per 30-second window per player (UUID or name key).
- **Access control**: `operators_only` conservatively rejects all (no trusted operator adapter exists). `allowed_players` and `blocked_players` lists are case-insensitive name matches.
- **Plain chat ignored**: non-prefixed Minecraft chat messages are never routed to AI.
- **`accept_console_ai_command`**: when `false`, `/ai` from the local console is rejected. **Note:** chat AI requests bypass this flag.

---

## 5. Groq tool registry

### Registry definition

Defined in `src/ai/registry.rs::command_registry()`. This is the authoritative allowlist sent to Groq as tool definitions and used by `validate_action`.

| Action type | Tool name | Arguments | Enabled | Execution type | Notes |
|---|---|---|---|---|---|
| `FollowPlayer` | `follow_player` | `player` (string, required) | Yes | Continuous | Follows until cancelled |
| `Goto` | `goto` | `x` (f64), `y` (f64), `z` (f64) | Yes | LongRunning | Navigate to position |
| `Gather` | `gather` | `item` (string), `quantity` (u32) | Yes | LongRunning | Gather items until inventory delta met |
| `Stop` | `stop` | *(none)* | Yes | Immediate | Stop movement and tasks |
| `StopAll` | `stop_all` | *(none)* | Yes | Immediate | Cancel all active bot tasks |
| `Finish` | `finish` | `summary` (string, **optional**) | Yes | Immediate | Finish verified objective |
| `PlaceBlock` | `place_block` | `x` (i32), `y` (i32), `z` (i32), `block_id` (string) | Yes | LongRunning | Place a block at coordinates |
| `GetStatus` | `get_status` | *(none)* | Yes | Immediate | Print bot status |
| `GetInventory` | `get_inventory` | *(none)* | Yes | Immediate | Print bot inventory |
| `ChopTree` | `chop_tree` | `tree_type` (string, optional) | Yes | LongRunning | Chop down a nearby tree |
| `CollectItem` | `collect_item` | `item` (string), `quantity` (u32) | Yes | LongRunning | Collect dropped items |
| `MineOre` | `mine_ore` | `ore` (string), `quantity` (u32) | Yes | LongRunning | Mine a specific ore type |
| `LookAtPlayer` | `look_at_player` | `player` (string) | Yes | LongRunning | Look at a specific player |
| `GotoBlock` | `goto_block` | `block` (string), `search_radius` (u32, optional) | Yes | LongRunning | Navigate to a specific block type |

### AiAction enum variants — full classification

The `AiAction` enum in `src/ai/mod.rs` has 21+ variants. 14 are registered and executable.

**Registered and executed in runtime:**

| Variant | Tool name | Runtime handler in `app.rs` |
|---|---|---|
| `FollowPlayer { player }` | `follow_player` | Calls `movement.follow()` |
| `Goto { x, y, z }` | `goto` | Calls `tasks.goto_position()` |
| `Gather { item, quantity }` | `gather` | Calls `start_gather()` |
| `Stop` | `stop` | Cancels tasks and stops movement |
| `StopAll` | `stop_all` | Same as Stop (cancels tasks + movement) |
| `Finish { summary }` | `finish` | Prints summary, verifies objective |
| `PlaceBlock { x, y, z, block_id }` | `place_block` | Calls `tasks.place_block()` |
| `GetStatus` | `get_status` | Prints status to console |
| `GetInventory` | `get_inventory` | Prints inventory to console |
| `ChopTree { log_type }` | `chop_tree` | Delegates to tree chopping service |
| `CollectItem { item, quantity }` | `collect_item` | Delegates to drop collector |
| `MineOre { ore, quantity }` | `mine_ore` | Delegates to mining service |
| `LookAtPlayer { player }` | `look_at_player` | Delegates to look controller |
| `GotoBlock { block, search_radius }` | `goto_block` | Delegates to block navigation |

**Defined in enum but NOT registered — rejected by `validate_action`:**

| Variant | Notes |
|---|---|
| `Craft { item, quantity }` | Not in registry |
| `CollectFood { minimum_food_points }` | Not in registry |
| `FindPlayer { player }` | Not in registry |
| `OpenContainer` | Not in registry |
| `TakeItem { item, quantity }` | Not in registry |
| `StoreItem { item, quantity }` | Not in registry |
| `Wait { milliseconds }` | Not in registry |

### Example JSON objects

```json
{
  "type": "follow_player",
  "player": "5cat"
}
```

```json
{
  "type": "goto",
  "x": 10.0,
  "y": 64.0,
  "z": 20.0,
  "mining": false
}
```

```json
{
  "type": "gather",
  "item": "minecraft:oak_log",
  "quantity": 8
}
```

```json
{
  "type": "stop"
}
```

```json
{
  "type": "stop_all"
}
```

```json
{
  "type": "finish",
  "summary": "Gathered 10 oak logs."
}
```

```json
{
  "type": "finish"
}
```

```json
{
  "type": "place_block",
  "x": 10,
  "y": 64,
  "z": 20,
  "block_id": "minecraft:oak_planks"
}
```

`finish.summary` is optional (`#[serde(default)]`). When absent, the runtime prints "Task completed."

---

## 6. Cancellation matrix

| Command | Movement | Follow | Block navigation | Interaction | Look | Gather | Collection | Mining | Tree chopping | Food collection | Container | AI session | Tasks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `/stop` | **Yes** | **Yes** | **Yes** | No | No | No | No | No | No | No | No | No | No |
| `/stopmovement` | **Yes** (alias) | **Yes** | **Yes** | No | No | No | No | No | No | No | No | No | No |
| `/stopall` | **Yes** | **Yes** | **Yes** | **Yes** | **Yes** | **Yes** | **Yes** | No | No | No | No | **Yes** | **Yes** |
| `/gathercancel` | **Yes** | — | **Yes** | **Yes** | — | **Yes** | **Yes** | No | No | No | No | No | **Yes** |
| `/stopinteraction` | No | No | No | **Yes** | No | No | No | No | No | No | No | No | No |
| `/lookstop` | No | No | No | No | **Yes** | No | No | No | No | No | No | No | No |
| `/cancelgotoblock` | No | No | **Yes** | No | No | No | No | No | No | No | No | No | No |
| `/aicancel` | **Yes** | — | **Yes** | — | — | **Yes** | — | — | — | — | — | **Yes** | **Yes** |
| `/mine-ore stop` | No | No | No | No | No | No | No | **Yes** | No | No | No | No | No |
| `/chop-tree stop` | No | No | No | No | No | No | No | No | **Yes** | No | No | No | No |
| `/collect-food stop` | No | No | No | No | No | No | No | No | No | **Yes** | No | No | No |
| `/collect-item stop` | No | No | No | No | No | No | **Yes** | No | No | No | No | No | No |
| `/close-container` | No | No | No | No | No | No | No | No | No | No | **Yes** | No | No |
| `/task cancel <id>` | No | No | No | No | No | No | No | No | No | No | No | No | **Yes** |
| `/task cancel all` | No | No | No | No | No | No | No | No | No | No | No | No | **Yes** |
| `!stop` / `!cancel` | **Yes** | — | **Yes** | — | — | **Yes** | — | — | — | — | — | **Yes** | **Yes** |

### Ownership restrictions for Minecraft chat

`!stop` and `!cancel` are **not** restricted to the session owner. Any player with chat access who sends `!stop` (after prefix removal) triggers cancellation of the current AI session regardless of who started it. There is no per-requester cancellation scope.

---

## 7. Status commands

| Command | Output fields | Source | Works while disconnected | Common states |
|---|---|---|---|---|
| `/status` | Connection state, username, server, position, block position, yaw/pitch, dimension, health, food, hotbar slot, inventory count, nearby players/entities, current task, uptime, reconnect settings | Live `MinecraftClient` + `WorldState` | No | Connected/Disconnected |
| `/movement` | Movement state, destination, current position, remaining distance, target player, elapsed time, input adaptation, failure reason | Live `MovementService` snapshot | No | Idle/MovingToPosition/FollowingPlayer/Completed/Failed |
| `/path-status` | Pathfinder status | Live `MinecraftClient` | No | calculating/executing/completed/idle |
| `/gotoblockstatus` | State, block ID, search radius, target/approach positions, distance, candidates, attempts, elapsed, failure reason | Live `BlockNavigationService` snapshot | No | Idle/Moving/Searching/Reached/Failed |
| `/lookstatus` | State, target, precision, priority, yaw/pitch, speeds, elapsed, failure reason | Live `LookController` snapshot | No | Idle/Looking/Completed/Failed |
| `/interactionstatus` | State, target, progress%, distance, elapsed, retries, failure reason | Live `InteractionController` snapshot | No | Idle/Breaking/Placing/Completed/Failed |
| `/gatherstatus` | Same as `/taskstatus` (calls `print_task_status`) | Live `TaskService` + `MovementService` + `LookController` | No | Idle/Moving/Completed |
| `/taskstatus` | Movement state, look state, workflow (ID, type, state), progress phase, active/recent task counts | Live `TaskService` + `MovementService` + `LookController` | No | Various |
| `/task status <id>` | Task type, state, source, correlation, progress, failure details | `TaskService` query | No | Per-task |
| `/task recent` | List of recent tasks with ID, type, state, progress | `TaskService` recent list | No | — |
| `/container-status` | Phase, target, window ID, transferred/total, outcome, detail | Live `ContainerService` | No | Idle/Opening/Transferring/Closing |
| `/containerstatus` | Session generation, open/synced state, window identity, revision, slot counts, cursor, timestamps | Live `WorldState` container snapshot | No | Various |
| `/collect-food-status` | Active/idle, phase, last result (outcome, counts, sources) | `FoodCollector` status | No | Active/Idle |
| `/collect-item status` | Running/idle, collected/target, target entity, lost | `DropCollector` status | No | Running/Idle |
| `/mine-ore status` | State, collected/requested, broken, skipped, failures, reason | Live `MiningService` snapshot | No | Various |
| `/chop-tree status` | Outcome, logs collected/requested, broken, unreachable, uncertain skipped | `TreeChopService` last result | No | Per-operation |
| `/craft status` | Active, recipe ID, operations, crafted, last result | `CraftService` status | No | Active/Idle |
| `/aistatus` | Session status, requester, objective, step/max, elapsed | `App.ai_session` | No | Various |
| `/interactionstatus` | State, target, progress%, distance, elapsed, retries, failure | Live `InteractionController` snapshot | No | Idle/Breaking/Placing |

---

## 8. Command availability matrix

| Capability | Console command(s) | Minecraft chat AI | Gemini registry | Runtime executed | Notes |
|---|---|---|---|---|---|
| Status | `/status` | No | `get_status` (registered) | Yes | |
| Inventory | `/inventory` | No | `get_inventory` (registered) | Yes | |
| Follow player | `/follow <name>` | `!follow <name>` | `follow_player` (registered) | Yes | |
| Goto position | `/goto <x> <y> <z>` | `!goto <x> <y> <z>` | `goto` (registered) | Yes | |
| Gather items | `/gather <item> <qty>` | `!gather <item> <qty>` | `gather` (registered) | Yes | |
| Stop movement | `/stop` | `!stop` | `stop` (registered) | Yes | |
| Stop all | `/stopall` | `!stop` or `!cancel` | `stop_all` (registered) | Yes | |
| Finish | *(none — AI only)* | *(none)* | `finish` (registered) | Yes | |
| Find/Navigate to block | `/findblock`, `/gotoblock` | No | `goto_block` (registered) | Yes | |
| Look at | `/look`, `/lookblock`, `/lookplayer`, `/lookentity` | No | `look_at_player` (registered) | Yes | |
| Break block | `/break`, `/breakblock`, `/breaknearest` | No | *(none)* | No | |
| Place block | `/place`, `/placeblock` | No | `place_block` (registered) | Yes | |
| Mine ore | `/mine-ore` | No | `mine_ore` (registered) | Yes | |
| Collect item | `/collect-item` | No | `collect_item` (registered) | Yes | |
| Chop tree | `/chop-tree` | No | `chop_tree` (registered) | Yes | |
| Craft | `/craft` (stub), `/craft-check` | No | `craft` (enum, **not registered**) | No | `/craft` prints rejection at runtime |
| Container | `/open-chest`, `/take-item`, `/store-item` | No | `take_item`, `store_item` (enum, **not registered**) | No | |
| Collect food | `/collect-food` | No | `collect_food` (enum, **not registered**) | No | |

---

## 9. Common errors

| Error or output | Meaning | Likely cause | Fix |
|---|---|---|---|
| `Unknown command: <name>` | Parser does not recognize the token | Typo or deprecated command name | Run `/help` for valid commands |
| `Bot is not connected` | Command requires active connection | Bot disconnected or not yet connected | Check server; use `/reconnect` |
| `Player not found` | Follow target does not match a known player | Player offline or out of tracking radius | Verify player name and proximity |
| `Target unreachable` | Block navigation cannot find a valid approach | No loaded cells with valid foot/head/floor positions near the target | Move closer or wait for chunks to load |
| `No reachable approach position` | InteractionController cannot reach the target block | Bot too far or path blocked | Move closer to the block |
| `Another AI task is already active` | `busy_behavior = "reject"` and a session or gather is active | Previous request still running | Wait or send `!stop` |
| `Groq API key is missing or unavailable` | `GroqProvider::from_config` failed | No `GROQ_API_KEY` env var set | Configure `GROQ_API_KEY` environment variable |
| `Groq support is disabled` | `groq.enabled = false` | Configuration | Enable in `config.toml` |
| `[AI] Console AI requests are disabled by configuration.` | `accept_console_ai_command = false` | Configuration | Enable in config or use chat |
| `No remaining reachable gather candidates` | All candidate positions are ignored or unreachable | Targets exhausted within search radius | Wait for chunks to load; cancel and retry |
| `Gather already running` | Second gather while one is active | Concurrent gather requests | Cancel current gather first |
| `Gather timed out` | Gather exceeded 300-second limit | Too many unreachable targets or slow movement | Move closer; reduce quantity |
| `Action limit reached` | AI session exceeded `max_actions_per_session` | Too many steps | Start a new request |
| `Session timed out` | Session exceeded `max_session_seconds` (default 600s) | Long-running task | Start a new request |
| `/<command> does not accept arguments` | Extra arguments provided to a no-arg command | Syntax error | Remove arguments |
| `/<command> accepts one argument` | Too many arguments | Syntax error | Provide exactly one argument |
| `Unknown entity: <type>` | `/lookentity` type not found in current entities | Entity not nearby or wrong name | Check entity type name |
| `Recipe unavailable` | Recipe ID not found in versioned recipe book | Unknown item | Check recipe exists |
| `Craft request ... rejected` | `/craft` debug surface requires a resolved plan | Incomplete implementation | Use `/craft-check` for planning |

---

## 10. Configuration affecting commands

### `[console]`

| Field | Default | Affects |
|---|---|---|
| `enabled` | `true` | Whether stdin input is read at all |
| `send_plain_input_to_chat` | `false` | Whether non-`/` console input is sent to Minecraft chat |
| `show_system_messages` | `true` | System chat display (not command-gated) |
| `show_action_bar_messages` | `false` | Action bar chat display (not command-gated) |

### `[ai]`

| Field | Default | Affects |
|---|---|---|
| `busy_behavior` | `"reject"` | Rejects new AI requests while one is active |

### `[ai.chat]`

| Field | Default | Affects |
|---|---|---|
| `enabled` | `true` | Whether chat AI processing runs |
| `prefix` | `"!"` | Chat prefix for AI requests |
| `respond_in_chat` | `true` | Whether AI responses are sent as Minecraft chat |
| `acknowledge_requests` | `true` | Whether "Request received." is sent on accepted request |
| `accept_console_ai_command` | `true` | Whether `/ai` from local console is accepted |
| `strip_prefix_whitespace` | `true` | Whether whitespace after prefix is trimmed |
| `max_request_length` | `500` | Maximum characters in an AI request after prefix removal |
| `incoming_queue_capacity` | `64` | FIFO queue capacity for incoming player chat |
| `access.operators_only` | `false` | Rejects all requests (no operator adapter) |
| `access.allowed_players` | `[]` | Whitelist (empty = allow all not blocked) |
| `access.blocked_players` | `[]` | Blacklist |
| `rate_limit.enabled` | `true` | Enable per-player rate limiting |
| `rate_limit.requests` | `3` | Max requests per window |
| `rate_limit.window_seconds` | `30` | Rate limit window |

### `[groq]`

| Field | Default | Affects |
|---|---|---|
| `enabled` | `false` | Whether `/ai` and chat AI requests are processed |
| `model` | `"deepseek-v4-flash-free"` | Model used for planning |
| `base_url` | `"https://api.groq.com/openai/v1"` | API base URL |
| `request_timeout_seconds` | `30` | HTTP timeout per Groq request |
| `max_request_retries` | `2` | Retries on Groq failure |
| `temperature` | `0.1` | Groq temperature |
| `include_nearby_blocks` | `true` | Include block context in prompt |
| `include_nearby_entities` | `true` | Include entity context in prompt |
| `include_inventory` | `true` | Include inventory context in prompt |

### `[groq.limits]` (same as `[gemini.limits]`)

| Field | Default | Affects |
|---|---|---|
| `max_gather_quantity` | `64` | Max quantity for `gather` action |
| `max_mine_quantity` | `64` | Max quantity for `mine_ore` action |
| `max_craft_quantity` | `64` | Max quantity for `craft` action |
| `max_navigation_distance` | `256.0` | Max Euclidean distance for `goto` |
| `max_actions_per_session` | `30` | Hard cap on action count |
| `max_replans_per_session` | `8` | Max replanning attempts (not enforced by App) |
| `allow_mining` | `true` | Gate for `mine_ore` validation |
| `allow_crafting` | `true` | Gate for `craft` validation |
| `allow_containers` | `true` | Gate for `take_item`/`store_item` validation |
| `allow_block_placement` | `true` | Gate for `place_block` validation |

### `[movement]`

| Field | Default | Affects |
|---|---|---|
| `follow_distance` | `3.0` | Distance maintained by `/follow` and AI `follow_player` |
| `repath_interval_ms` | `500` | Movement tick rate (ms) |
| `arrival_distance` | `1.5` | Proximity threshold for arrival |

### `[block_search]`

| Field | Default | Affects |
|---|---|---|
| `default_radius` | `32` | Default search radius for `/findblock`, `/nearestblock`, gather |
| `maximum_radius` | `128` | Maximum allowed search radius |
| `default_result_limit` | `20` | Default result limit for `/findblock` |
| `maximum_result_limit` | `256` | Maximum result limit |
| `default_vertical_range` | `32` | Vertical search range above/below bot |

### `[block_navigation]`

| Field | Default | Affects |
|---|---|---|
| `default_search_radius` | `32` | Default radius for `/gotoblock` |
| `maximum_search_radius` | `128` | Maximum navigation search radius |
| `maximum_target_attempts` | `10` | Max approach candidates tried |
| `maximum_navigation_seconds` | `120` | Timeout for block navigation |

### `[interaction]`

| Field | Default | Affects |
|---|---|---|
| `maximum_reach` | `4.5` | Max interaction distance |
| `auto_tool_switch` | `true` | Automatic tool selection before breaking |
| `minimum_tool_durability` | `2` | Minimum remaining durability for tool use |
| `allow_hand_fallback` | `true` | Allow bare hand when no tool needed |

### `[tree_chopping]`

| Field | Default | Affects |
|---|---|---|
| `allowed_tree_types` | 9 types | Valid tree types for `/chop-tree <type>` |
| `search_radius` | `32` | Search radius for tree candidates |
| `maximum_trees` | `16` | Max trees inspected per request |
| `total_timeout_seconds` | `300` | Timeout for chop operation |

---

## 11. Current limitations

1. **`/craft` is a stub.** The parser accepts `/craft <item> <count>` but the dispatcher rejects it at runtime. Only `/craft status` and `/craft stop` are functional.

2. **`/ensure-tool` / `/craft-tool` is a stub.** Prints a diagnostic message but takes no action. No live crafting adapter is wired.

3. **`/drops` is documented in help but not implemented.** The `print_help()` function mentions `/drops [RADIUS]`, but no parser entry, enum variant, or handler exists. It will return `Unknown command`.

4. **Previously-unregistered actions are now registered.** `get_status`, `get_inventory`, `chop_tree`, `collect_item`, `mine_ore`, `look_at_player`, and `goto_block` are now in the registry and executable via Groq.

5. **7 `AiAction` enum variants remain unregistered.** `craft`, `collect_food`, `find_player`, `open_container`, `take_item`, `store_item`, and `wait` exist in the enum but are never available to Groq.

6. **Console and Groq registries are separate.** Console commands have 69+ enum variants; Groq has 14 registered tools. The Groq tools are generated from the shared command registry in `src/ai/registry.rs`.

7. **AI session can be overwritten.** `/ai` from the console accepts even when a session is active (does not check `can_accept_ai_request`), potentially replacing the session while physical work continues.

8. **`!stop`/`!cancel` have no ownership scoping.** Any player with chat access can cancel any session.

9. **Chat responses are public only.** There is no private-message option.

10. **Gather clears `ignored_targets` after any confirmed collection**, potentially retrying previously unreachable targets.

11. **`max_replans_per_session` and `max_session_seconds` model fields are not enforced by `App`.** Session timeout is enforced by `enforce_ai_limits()` using the top-level `gemini.max_session_seconds`, but replan counting is not wired.

12. **`allow_block_placement` config defaults to `true` and gates `place_block` Gemini validation.** The `place_block` action is registered and executed via the task runtime.

13. **Some features (inventory cleanup, smelting) have config and modules but are not wired into the command dispatcher.**

14. **Live-server behavior is unverified.** All tests are unit/mock tests. No integration tests exist against a real Minecraft server or Groq API.

---

## 12. Developer reference

### Parser and dispatch

| Symbol | Path | Purpose |
|---|---|---|
| `ConsoleCommand` | `src/console/commands.rs:11` | Enum of all local console commands |
| `ConsoleInput` | `src/console/commands.rs:174` | Wrapper: `Command`, `ChatMessage`, or `Empty` |
| `parse_input` | `src/console/commands.rs:189` | Parse string into `ConsoleInput` |
| `plain_chat_message` | `src/console/commands.rs:182` | Extract chat message if forwarding enabled |
| `read_input` | `src/console/mod.rs:16` | Async stdin reader, sends to channel |
| `execute_console_input` | `src/app.rs:296` | Dispatch `ConsoleInput` to handlers |
| `print_help` | `src/app.rs:2489` | Print help text |

### AI session and planning

| Symbol | Path | Purpose |
|---|---|---|
| `AiAction` | `src/ai/mod.rs:141` | Enum of all Groq action types |
| `AiSession` | `src/ai/mod.rs:219` | Session state machine |
| `AiSessionStatus` | `src/ai/mod.rs:99` | Session lifecycle states |
| `AiActionStatus` | `src/ai/mod.rs:111` | Per-action lifecycle states |
| `AiObjective` | `src/ai/mod.rs:133` | High-level session objective |
| `GroqProvider` | `src/ai/groq.rs` | HTTP client for Groq API (OpenAI-compatible) |
| `AiProvider` (trait) | `src/ai/provider.rs` | Abstract AI provider trait |
| `validate_action` | `src/ai/mod.rs:352` | Validate action against registry and limits |
| `verify_objective` | `src/ai/mod.rs` | Check if objective is satisfied |
| `chat_request` | `src/ai/mod.rs` | Parse chat message into `AiRequest` |
| `bind_requester_intent` | `src/ai/mod.rs` | Resolve "follow me" to trusted sender |
| `resolve_requester_references` | `src/ai/mod.rs` | Resolve me/myself/I to requester name |
| `command_registry` | `src/ai/registry.rs:54` | Groq action allowlist |
| `generate_tool_definitions` | `src/ai/registry.rs:213` | Generate Groq tool defs from registry |
| `build_system_prompt` | `src/ai/registry.rs:260` | Build system prompt from registry |
| `action_is_registered` | `src/ai/registry.rs:326` | Check if action is in enabled registry |
| `action_name` | `src/ai/registry.rs:307` | Map action to registry name |
| `execute_ai_action` | `src/app.rs` | Dispatch Groq action to runtime |
| `submit_ai_request` | `src/app.rs` | Full AI request pipeline |
| `start_ai` | `src/app.rs` | Console `/ai` entry point |
| `cancel_ai` | `src/app.rs` | Cancel active AI session |
| `tick_ai_chat` | `src/app.rs` | Process incoming chat AI requests |
| `enforce_ai_limits` | `src/app.rs` | Session timeout enforcement |
| `finish_ai_if_verified` | `src/app.rs` | Attempt verified completion |
| `finalize_ai_task` | `src/app.rs` | Central cleanup for all AI terminal results (success/failure/timeout/cancellation) |

### Configuration

| Symbol | Path | Purpose |
|---|---|---|
| `Config` | `src/config.rs:18` | Root configuration |
| `GeminiConfig` | `src/config.rs:161` | Gemini planner configuration |
| `GeminiLimits` | `src/config.rs:210` | Gemini action safety limits |
| `AiConfig` | `src/config.rs:56` | AI behavior configuration |
| `AiChatConfig` | `src/config.rs:69` | Chat AI processing configuration |
| `MovementConfig` | `src/config.rs:497` | Movement parameters |
| `BlockSearchConfig` | `src/config.rs:615` | Block search parameters |
| `BlockNavigationConfig` | `src/config.rs:657` | Block navigation parameters |
| `InteractionConfig` | `src/config.rs:917` | Interaction/break/place parameters |
| `TreeChoppingConfig` | `src/config.rs:337` | Tree chopping parameters |
| `ConsoleConfig` | `src/config.rs:461` | Console I/O configuration |

---

## 13. Auto-generated accuracy rules

This document was generated from source code analysis. The following rules governed its creation:

- **Parser code** (`src/console/commands.rs`) is authoritative for console command syntax.
- **Dispatcher code** (`src/app.rs::execute_console_input`) is authoritative for actual execution.
- **Registry code** (`src/ai/registry.rs::command_registry`) is authoritative for Gemini action availability.
- **Enum definitions** (`AiAction` in `src/ai/mod.rs`) are classified but not assumed usable.
- **Tests** support behavior claims but do not override runtime code.
- **`print_help()` output** is not authoritative when source differs (e.g. `/drops`).
- **No command is documented as usable merely because an enum variant exists.**
- **No API keys, tokens, usernames, or server addresses are exposed.**
- **Uncertain live-server behavior is marked as unverified.**

---

*Generated from source snapshot of `magic_ai_bot` on 2026-07-26. Re-verify after any source change.*
