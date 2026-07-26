# Advanced Inventory and Equipment

Read \`instructions/GOALS.md\`, inspect inventory, selection, sorting, survival, and combat. Implement only this prompt; do not craft/gather equipment and preserve protected items.

## Goal

Implement configurable central selection/equipping for armor, offhand, tools, weapons, food, utilities, and emergency items.

Policies include best armor, durability/enchantment limits, preferred weapon/tool, shield/totem/food/empty offhand, block, and emergency item. Armor ranking considers armor, toughness, protection, durability, binding curse, and preference; never downgrade armor.

All mutations acquire inventory ownership, confirm revisions, handle cursor/previous items, cancel safely, and report partial changes. Add \`equip best\`, \`equip armor\`, \`equip offhand totem\`, \`equip weapon\`, and \`equipment status\`.

Test ranking, durability/enchantments/curses, offhand policy, protected items, stale revisions, and cancellation.
