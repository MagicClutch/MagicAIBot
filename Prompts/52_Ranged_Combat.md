# Ranged Combat

Read \`instructions/GOALS.md\`, inspect melee combat, entity search, look, selection, equipment, and movement. Implement only this prompt; no autonomous targeting or advanced ballistic ML.

## Goal

Implement bounded basic ranged combat against an explicitly selected target using reliable bow, crossbow, or throwable support.

Select weapon/ammunition, approach or retreat into range, track movement, use bounded simple lead, charge/release normally, confirm ammo/durability changes, and obey shot/time limits.

Never fire at invalid targets, protected entities in line, unavailable ammo, unreasonable range, critical health, or insufficient configured line-of-sight confidence. Report attempts, confirmations, ammo, target outcome, and abort reason.

Test selection, ammo, charge timing, range, lead bounds, protected targets, and cancellation.
