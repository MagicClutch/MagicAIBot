# GOALS.md

# Minecraft Autonomous AI

Version: 1.0

---

# Vision

The goal of this project is to build a fully autonomous Minecraft AI player in Rust using Azalea.

The AI should behave like a real experienced player instead of a scripted bot.

It should be capable of understanding natural language, planning complex goals, solving problems on its own, and interacting with the Minecraft world without requiring step-by-step instructions.

The long-term objective is for the AI to perform almost any Minecraft task that a skilled player could perform.

---

# Core Philosophy

The user gives goals.

The AI figures out how to accomplish them.

Example:

```
Get me 32 diamonds.
```

The AI should automatically decide to:

* gather food
* craft tools
* upgrade equipment
* search caves
* strip mine
* avoid dangers
* return home
* give the diamonds

without asking for every individual step.

---

# Main Objectives

The AI should eventually be capable of:

* Mining
* Building
* PvP
* Crystal PvP
* Resource gathering
* Exploration
* Farming
* Crafting
* Inventory management
* Chest organization
* Trading
* Villager interaction
* Navigation
* Parkour
* Boat travel
* Elytra flight
* Nether travel
* End travel
* Base construction
* Redstone interaction
* Multiplayer cooperation
* Following players
* Guarding players
* Assisting players
* Completing large projects
* Completing long-term goals

The architecture should never assume a limited feature set.

Everything should be expandable.

---

# AI Philosophy

The AI is autonomous.

The AI makes decisions.

The AI solves problems.

The AI creates plans.

The AI executes plans.

The AI adapts while executing.

The AI should require as little user interaction as possible.

---

# Natural Language

Commands can come from

* Minecraft chat
* Console
* Future APIs
* Future Discord integration
* Future Web UI

The user should be able to write things like:

```
Bring me food.

Build a house.

Find a village.

Protect me.

Follow me.

Mine diamonds.

Get me a stack of logs.

Build this schematic.

Collect 10 shulker boxes.

Go to spawn.

Fight that player.
```

The AI determines how to complete the request.

---

# Planning

Every complex task should be broken into smaller tasks.

Example:

```
Get diamonds
```

↓

Need pickaxe

↓

Need iron

↓

Mine iron

↓

Craft pickaxe

↓

Need food

↓

Collect food

↓

Mine diamonds

↓

Return

Planning should always be hierarchical.

Large goals become small executable tasks.

---

# AI Router

The project must never depend on a single AI provider.

Every request should pass through an AI Router.

Example:

```
User Request

↓

AI Router

↓

Classify Request

↓

Choose Provider

↓

Execute
```

Routing should be configurable.

Example routing:

* Chat model
* Gemini
* OpenAI
* Claude
* Local models
* Future providers

Providers should be interchangeable.

No provider should be hardcoded.

The router should prioritize the lowest-cost capable provider unless configured otherwise.

---

# Short-Term Context

The AI may remember information during the current task.

Example:

* discovered cave
* nearby village
* current objective
* current inventory
* active enemies

This information only exists while the task is active.

---

# Long-Term Learning

The AI should **not** permanently learn new behaviors or modify its own decision-making across restarts.

Improvements come from code updates, not autonomous self-learning.

This keeps the bot deterministic, predictable, and easier to debug.

---

# Architecture

The project should be heavily modular.

Example structure:

```
Brain
    Planner
    Goal Manager
    AI Router
    Memory
    Executor

World
    World State
    Inventory
    Navigation
    Combat
    Building

Tasks
    Move
    Mine
    Place
    Craft
    Attack
    Gather
    Build

Interfaces
    Chat
    Console
    Future API
    Future Web UI
```

Modules should communicate through well-defined interfaces.

Avoid tight coupling.

---

# Code Quality

The codebase should always prioritize:

* readability
* maintainability
* modularity
* simplicity
* performance
* reliability

Never sacrifice architecture for short-term convenience.

---

# Reusability

Every system should be reusable.

Avoid duplicate code.

If functionality already exists,

reuse it.

Do not create parallel implementations.

---

# Extensibility

Every feature should support future expansion.

Avoid hardcoded assumptions.

Everything should be configurable.

---

# Configuration

Behavior should be configurable whenever practical.

Examples:

* AI providers
* API keys
* movement settings
* combat settings
* mining settings
* building settings
* planner settings
* logging
* debugging

---

# Error Handling

The bot should never crash from expected failures.

Recover whenever possible.

Examples:

* path blocked
* block missing
* player moved
* inventory full
* tool broke
* lava found
* chunk unloaded

Every failure should have a recovery strategy.

---

# Logging

Every important action should be logged.

Examples:

```
Planning goal

Searching for iron

Mining block

Crafting pickaxe

Changing task

Combat started

Combat ended

Goal completed

Goal failed
```

Logs should be useful for debugging.

---

# Performance

Avoid unnecessary:

* allocations
* cloning
* API requests
* world scans
* path recalculations

Reuse cached information whenever possible.

---

# Task System

Every action should be represented as a task.

Examples:

MoveTask

MineTask

CraftTask

BuildTask

AttackTask

CollectTask

FollowTask

Tasks should:

* start
* update
* finish
* fail
* cancel

Tasks should be composable.

---

# Planner

The planner should:

* understand goals
* estimate requirements
* discover dependencies
* schedule tasks
* react to failures
* replan when necessary

The planner is responsible for strategy.

Tasks are responsible for execution.

---

# Combat

Combat should eventually support:

* sword PvP
* axe PvP
* crystal PvP
* anchor PvP
* ranged combat
* shield usage

Combat should appear human.

---

# Navigation

Navigation should:

avoid lava

avoid cliffs

avoid dangerous falls

use bridges

use boats

use doors

use ladders

use water

navigate caves

recover when stuck

---

# Inventory

Inventory management should support:

sorting

crafting

equipping armor

tool selection

food selection

totems

resource storage

automatic organization

---

# Building

Building should support:

placing blocks

breaking blocks

schematics

blueprints

repairing

terraforming

large-scale construction

---

# World Understanding

The AI should maintain an accurate internal representation of:

* blocks
* entities
* players
* mobs
* inventories
* structures
* dimensions

The world model should continuously update.

---

# Development Rules

Every implementation prompt must follow these rules:

1. Read GOALS.md completely.
2. Read \`Prompts/00_Prompt_Execution_Standard.md\` completely.
3. Read the requested prompt.
4. Only implement the requested feature.
5. Never implement future prompts.
6. Reuse existing systems.
7. Do not duplicate code.
8. Maintain architecture consistency.
9. Keep code production-ready.
10. Write clean Rust.
11. Add documentation where useful.
12. Use typed errors, bounded retries, explicit ownership, cancellation-safe cleanup, and server-confirmed state changes as defined by the shared execution standard.
13. Prompts 34–58 must also read and follow \`Prompts/34_58_Architecture_And_Autonomy_Addendum.md\`.

---

# Definition of Done

A feature is complete only when:

* it compiles
* it is tested
* it integrates with existing systems
* no duplicate code exists
* logging is implemented
* errors are handled
* configuration is respected
* documentation is updated

---

# Final Goal

The finished project should feel like an intelligent Minecraft player—not a collection of scripts.

The AI should receive a high-level objective, independently create a plan, execute it, adapt to changing conditions, recover from failures, and complete the objective using clean, reusable systems that can continue to grow as the project evolves.
