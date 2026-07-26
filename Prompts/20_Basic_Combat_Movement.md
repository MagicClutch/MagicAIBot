# Basic Combat Movement

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect BasicMeleeCombat, MovementController, navigation, look control, WorldState, and ownership systems.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement crystal PvP, anchor PvP, advanced prediction, machine learning, aim assist beyond existing look control, or autonomous target selection.
6. Reuse existing combat and movement systems.
7. Do not create a parallel combat controller.
8. Keep movement deterministic, configurable, and human-like.

---

# Goal

Improve basic melee combat movement with simple strafing, spacing, approach, retreat, and bounded sprint-reset behavior.

This prompt enhances movement around an already-selected combat target.

---

# Combat Movement Controller

Create a reusable component used by BasicMeleeCombat.

It should decide between:

- approach
- hold range
- strafe left
- strafe right
- retreat
- jump when appropriate
- stop
- recover from collision
- re-approach after knockback

The combat action remains responsible for target lifecycle.

---

# Spacing

Maintain configurable distance bands:

- too far: approach
- ideal range: strafe or hold
- too close: retreat or reposition
- outside chase range: abort

Do not hold a fixed maximum reach distance at all times.

The bot should naturally move closer than maximum attack range when appropriate.

---

# Strafing

Implement simple bounded strafing.

Requirements:

- choose left or right deterministically using seeded randomness or state
- change direction at human-like intervals
- avoid switching every tick
- avoid strafing into known lava, cliffs, or blocked space
- stop strafing when pathing is required
- permit configuration of strafe duration and cooldown

Randomness must be reproducible in tests.

---

# Target Facing

Continue using LookAtTarget.

Movement decisions must not bypass rotation ownership.

Allow limited yaw error before attacks.

Do not implement unnatural instant perfect tracking unless immediate mode is explicitly configured.

---

# Sprint Reset

Add an optional minimal sprint-reset mechanic.

Requirements:

- configurable
- bounded
- tied to successful or attempted attacks
- never toggled every tick
- does not interfere with survival or navigation cleanup
- falls back gracefully when unsupported

Do not claim perfect PvP combo behavior.

---

# Knockback Recovery

Use actual WorldState movement to detect displacement.

When knocked away:

- reassess distance
- re-approach
- avoid immediate repeated path recalculation
- stop when target becomes unreachable
- preserve target identity

---

# Collision and Stuck Recovery

When movement makes no progress:

- stop conflicting inputs
- attempt a small alternate strafe
- optionally jump if safe
- hand control back to navigation when a path is needed
- limit retries

Do not break or place blocks.

---

# Human-Like Constraints

Support configuration for:

- reaction delay
- strafe interval range
- strafe duration range
- look speed
- attack-distance variation
- sprint-reset probability
- jump probability
- maximum input changes per second

Do not add artificial delays that make cancellation unreliable.

---

# Safety

Combat movement must avoid known:

- lava
- dangerous drops
- unloaded edges
- suffocation spaces
- void edges

Unknown space must be treated according to configuration.

---

# Commands

Extend combat commands with test settings such as:

- \`combat movement on\`
- \`combat movement off\`
- \`combat spacing <min> <ideal> <max>\`
- \`combat strafe on\`
- \`combat strafe off\`
- \`combat movement status\`

Do not create a second combat command hierarchy if one already exists.

---

# Logging

Log movement-state changes at debug level:

- approach
- ideal range
- retreat
- strafe direction
- knockback recovery
- stuck recovery
- safety rejection

Avoid logging each tick.

---

# Testing

Add tests for:

- distance-band decisions
- strafe timing
- deterministic seeded decisions
- unsafe-direction rejection
- knockback recovery
- stuck recovery limits
- sprint-reset cooldown
- cancellation
- emergency preemption
- input cleanup

---

# Acceptance Criteria

- The project compiles.
- Basic melee combat uses improved spacing and strafing.
- Movement does not remain fixed at maximum reach.
- Strafing avoids known dangerous terrain.
- Input changes are bounded and human-like.
- Knockback and collision recovery work.
- Existing combat, navigation, look, and movement systems are reused.
- Tests pass.
- No advanced PvP, crystals, anchors, ranged combat, or AI target selection is implemented.

At completion, update implementation status and summarize behavior, configuration, tests, and limitations.
