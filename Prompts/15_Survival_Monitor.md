# Survival Monitor

Before implementing anything:

1. Read \`instructions/GOALS.md\` completely.
2. Inspect WorldState, InventoryState, eating, movement, and event systems.
3. Read this prompt completely.
4. Implement only this prompt.
5. Do not implement gathering, crafting, shelter building, combat strategy, AI planning, or long-term survival goals.
6. Reuse existing state and action systems.
7. Do not create duplicate health or environment tracking.
8. Keep every automatic response configurable and bounded.

---

# Goal

Implement a survival-monitoring service that observes immediate dangers and can trigger a limited set of already-existing emergency actions.

This is not a general survival AI.

---

# Monitored Conditions

Monitor at least:

- low health
- critical health
- low hunger
- drowning risk
- lava contact
- fire
- dangerous fall state
- void danger
- suffocation when detectable
- recent damage
- death
- missing ground beneath the bot when known

Use WorldState as the source of truth.

---

# Survival State

Expose a read-only survival snapshot containing:

- current health
- absorption
- hunger
- current danger flags
- recent damage timestamp
- last damage amount when available
- current emergency level
- active emergency action
- whether automatic survival responses are enabled

---

# Emergency Levels

Define clear levels such as:

- safe
- caution
- danger
- critical

Make thresholds configurable.

Do not infer danger from unknown world data without marking uncertainty.

---

# Allowed Automatic Responses

Only use existing low-level actions.

Allowed responses may include:

- stop current movement
- stop active interaction
- eat available food
- jump while in water
- move toward nearby safe ground using existing bounded navigation
- move away from immediately known lava
- disconnect when configured as a final emergency response

Do not implement:

- crafting
- building
- mining an escape route
- combat
- using potions
- placing water
- using ender pearls
- finding shelter
- autonomous resource collection

Those require future prompts.

---

# Priority

Emergency survival actions should be able to preempt lower-priority manual or navigation actions through the existing ownership system.

Requirements:

- explicit priority
- clear cancellation reason
- safe ownership transfer
- no permanently stuck emergency state
- cooldown after repeated triggers
- bounded retry count

---

# Low Hunger

When enabled:

- detect hunger below the configured threshold
- call the existing eating action
- avoid eating repeatedly when an eating action is active
- respect emergency-food configuration
- apply cooldown after failure

Do not gather food.

---

# Water and Drowning

When underwater and air is low, where air state is available:

- attempt upward swimming or jumping
- optionally navigate to nearby known air
- stop when safe
- fail clearly if no loaded safe position is known

Do not search unloaded terrain.

---

# Lava and Fire

When in lava or burning:

- stop unrelated actions
- attempt bounded movement toward known non-lava space
- avoid selecting positions with dangerous drops
- optionally disconnect if configured and escape fails

Do not place water or blocks.

---

# Fall Detection

Detect dangerous downward movement using actual position and velocity where available.

This prompt may:

- stop forward input
- attempt to move toward nearby known support
- log the danger

Do not implement advanced clutch techniques.

---

# Events

Emit reusable events such as:

- danger detected
- emergency level changed
- emergency action started
- emergency action completed
- emergency action failed
- bot returned to safe state

---

# Commands

Add commands such as:

- \`survival status\`
- \`survival on\`
- \`survival off\`
- \`survival thresholds\`
- \`survival stop\`

Minecraft chat usage must respect authorization.

---

# Configuration

Support:

- automatic survival enabled
- low-health threshold
- critical-health threshold
- low-hunger threshold
- drowning threshold
- danger cooldown
- emergency retry limit
- allow emergency disconnect
- movement escape radius
- emergency action timeout

---

# Logging

Log state transitions, not every tick.

Include:

- detected danger
- severity
- selected response
- preempted action
- result
- cooldown
- return to safe state

---

# Testing

Add tests for:

- severity calculation
- threshold boundaries
- low-hunger trigger
- duplicate-action prevention
- emergency preemption
- cooldown
- lava response selection
- drowning response selection
- safe-state recovery
- disabled automatic response
- disconnect/death reset

---

# Acceptance Criteria

- The project compiles.
- Survival conditions are monitored centrally.
- Emergency severity is exposed.
- Existing eating and movement actions can be triggered.
- Emergency actions are bounded and configurable.
- Lower-priority actions can be safely preempted.
- Tests pass.
- No gathering, crafting, building, combat, or AI planning is implemented.

At completion, update implementation status and summarize monitored conditions, automatic responses, configuration, tests, and limitations.
