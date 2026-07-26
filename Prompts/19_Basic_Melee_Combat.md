# Basic Melee Combat

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect EntitySearch, navigation, look control, ItemSelector, WorldState, SurvivalMonitor, and ownership systems.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement advanced PvP, crystal PvP, axe-and-shield strategy, ranged combat, target selection AI, or autonomous hunting.
6. Reuse existing systems.
7. Do not duplicate movement, looking, or entity lookup.
8. Keep combat configurable, cancellable, and server-compliant.

---

# Goal

Implement a low-level basic melee combat action against one explicitly selected entity.

This prompt provides a reusable attack primitive, not full PvP intelligence.

---

# Combat Request

Create a typed request containing:

- exact target or EntitySearch query
- allowed target entity groups
- preferred weapon group
- maximum engagement distance
- attack range
- chase timeout
- total combat timeout
- sprinting allowed
- critical-hit attempts allowed
- stop when target dies
- stop when bot health falls below threshold
- cancellation support

---

# Target Safety

Before attacking:

- resolve the target deterministically
- reject ambiguous player targets
- verify the target is alive
- verify target type is allowed
- verify the target is not the bot itself
- respect configured protected players and entities
- respect server or user-defined attack restrictions

The project policy may allow free actions, but attack restrictions must remain configurable.

---

# Weapon Selection

Use ItemSelector.

Default ranking should consider:

- configured preferred weapons
- attack damage when known
- attack speed
- durability
- enchantments when known
- already-selected weapon
- hotbar accessibility

Do not craft or gather weapons.

---

# Engagement

The combat action should:

- approach the target using existing moving-target navigation
- stop inside attack range
- face the target using LookAtTarget
- attack using normal Minecraft behavior
- respect attack cooldown
- continue tracking a moving target
- avoid standing unnecessarily far away
- stop chasing after configured limits
- stop if the target becomes invalid

Do not spam attack packets.

---

# Attack Timing

Use Minecraft attack-cooldown information where available.

Requirements:

- prefer full or configurable cooldown strength
- do not attack faster than allowed
- support a configurable minimum cooldown percentage
- account for weapon attack speed
- track last attack time
- avoid frame-rate-dependent timing

---

# Critical Hits

When enabled and safe:

- attempt a normal jump critical using existing movement controls
- only attack during the valid falling phase
- do not jump continuously
- do not attempt critical hits in water, lava, on ladders, or when unsafe
- fall back to normal attacks

Keep this basic and deterministic.

---

# Movement

Implement only simple combat movement:

- close distance
- remain in attack range
- face target
- optional sprint reset where supported
- stop moving after combat

Do not implement:

- strafing strategy
- combo logic
- W-tapping strategy beyond a minimal configurable sprint reset
- advanced knockback control
- prediction
- crystal placement
- shield disabling
- projectile dodging

---

# Survival Integration

Combat must stop or yield when:

- critical-health threshold is reached
- SurvivalMonitor starts a critical emergency
- bot dies
- bot disconnects
- target enters a forbidden area when such information exists
- cancellation occurs

---

# Combat Results

Return outcomes such as:

- target defeated
- target escaped
- target disappeared
- target invalid
- target protected
- no weapon
- target unreachable
- combat timed out
- low-health abort
- cancelled
- disconnected
- bot died
- movement unavailable
- interaction unavailable

---

# Commands

Add commands such as:

- \`attack entity <id>\`
- \`attack player <name>\`
- \`attack nearest hostile <radius>\`
- \`attack stop\`
- \`attack status\`

Player attacks must require appropriate authorization.

---

# Configuration

Support:

- allowed entity groups
- protected players
- preferred weapons
- attack range
- chase distance
- cooldown threshold
- critical hits
- sprint reset
- health abort threshold
- chase timeout
- combat timeout

---

# Logging

Log:

- target selected
- weapon selected
- engagement started
- attack executed
- target lost
- health abort
- target defeated
- cancellation
- combat ended

Avoid per-tick logging.

---

# Testing

Add tests for:

- target restrictions
- protected-player handling
- weapon ranking
- cooldown timing
- attack-range decisions
- critical-hit eligibility
- health abort
- target death
- target disappearance
- cancellation and ownership cleanup

---

# Acceptance Criteria

- The project compiles.
- The bot can attack one selected entity using normal melee behavior.
- Attack cooldown is respected.
- Moving targets can be chased within bounded limits.
- Existing navigation, look, item-selection, and survival systems are reused.
- Protected targets are respected.
- Combat stops safely on all exit paths.
- Tests pass.
- No advanced PvP, ranged combat, crystal combat, or target-selection AI is implemented.

At completion, update implementation status and summarize behavior, configuration, tests, and known combat limitations.
