//! Application-supplied policy for the build-capable move set
//! ([`super::moves::building`]), carried through [`super::custom_state`] so
//! both move generation (`successors_fn`, background A* thread) and move
//! execution (`ExecuteCtx`, `GameTick` systems) see the same snapshot.
//!
//! The caller (the downstream bot crate) is responsible for inserting a
//! [`super::custom_state::CustomPathfinderState`] component with an initial
//! [`PathfindingPolicy`] (the `allow_*` flags and [`vertical::ScaffoldPolicy`]
//! preferences) before submitting a [`super::GotoEvent`]. From then on,
//! [`refresh_pathfinding_policy`] recomputes the *live* half of the policy
//! (which scaffold item is currently selected and how many are held) from a
//! fresh [`Menu`] at every one of this crate's own replanning call sites --
//! goal submission, path-merge, obstruction detection, and timeout-triggered
//! patching -- exactly mirroring how [`super::mining::MiningCache`] is
//! rebuilt at each of those same sites.

use std::collections::HashMap;

use azalea_inventory::Menu;

use super::{custom_state::CustomPathfinderState, vertical};

/// Which build-capable primitives are currently allowed, and what scaffold
/// material (if any) is available to place with. `Default` is "everything
/// off" so a route with no policy inserted behaves exactly like the
/// unmodified `default_move` set.
#[derive(Clone, Debug, Default)]
pub struct PathfindingPolicy {
    pub allow_pillaring: bool,
    pub allow_bridging: bool,
    pub allow_staircase_building: bool,
    /// Hard ceiling (in blocks) on how tall a single continuous tower-up
    /// climb may get -- see
    /// `moves::building::tower_up::execute::execute_pillar_up_move`'s doc
    /// comment for exactly how "continuous climb" is tracked (it survives
    /// replans, but resets if the bot genuinely descends). `0` (the
    /// `Default`) means "unset": pillaring is uncapped, matching this
    /// field's absence before it existed -- the same "0 means unset"
    /// convention [`Self::fast_bridge_edge_threshold`] uses.
    pub max_pillar_height: u32,
    /// Baritone-style "speed bridging": walk normally (not sneaking) across
    /// each block, only sneaking for the brief moment needed to place the
    /// next one near the edge, instead of sneaking for the whole approach.
    /// `false` (the `Default`) keeps the original permanent-sneak technique
    /// -- see `moves::building::bridging`'s bridge-move doc comment for the
    /// tradeoffs.
    pub fast_bridge_enabled: bool,
    /// How close to a block's edge (in blocks, e.g. `0.2`) the bot must be
    /// before it starts sneaking to place the next bridge block, when
    /// [`Self::fast_bridge_enabled`] is set. Only meaningful together with
    /// that flag. `0.0` (the `Default`) is treated as "unset" and falls
    /// back to a conservative built-in default (see
    /// `moves::building::bridging::FAST_BRIDGE_EDGE_THRESHOLD_FALLBACK`)
    /// rather than actually meaning zero, since a zero threshold would
    /// never trigger and the bot would just walk off the edge.
    pub fast_bridge_edge_threshold: f64,
    /// Static scaffold preferences (allow/deny lists, minimum held count),
    /// set once by the caller. Never mutated by [`Self::refresh_scaffold`].
    pub scaffold: vertical::ScaffoldPolicy,
    /// Item id of the scaffold block to place, recomputed from live
    /// inventory by [`Self::refresh_scaffold`]. `None` disables every
    /// placement-based move regardless of the flags above.
    pub scaffold_item: Option<String>,
    /// How many of `scaffold_item` are currently held (hotbar only -- see
    /// [`super::tool_policy::find_hotbar_item`], which is what actually
    /// selects it at execution time and only searches the hotbar). Used only
    /// as an availability gate (no placement edges at all when zero); this
    /// does not track remaining count along a specific route.
    pub scaffold_available: u32,
}

impl PathfindingPolicy {
    pub fn can_place(&self) -> bool {
        self.scaffold_item.is_some() && self.scaffold_available > 0
    }

    /// Recomputes [`Self::scaffold_item`]/[`Self::scaffold_available`] from
    /// a live hotbar snapshot, using [`Self::scaffold`]'s static preferences.
    ///
    /// `minimum_held` only gates *starting* to rely on a scaffold item, so a
    /// route isn't begun with too little material to plausibly finish it.
    /// Once an item is selected, it stays selected down to the last one held
    /// -- this runs on every replan (including the per-tick obstruction
    /// check), so re-applying the `minimum_held` threshold on every refresh
    /// would flip [`Self::can_place`] to `false` the moment a single
    /// placement dropped the held count below `minimum_held`, stranding an
    /// in-progress pillar/bridge/staircase route after just one block even
    /// though material was still available.
    pub fn refresh_scaffold(&mut self, menu: &Menu) {
        let mut counts: HashMap<String, u32> = HashMap::new();
        let slots = menu.slots();
        for menu_slot in menu.hotbar_slots_range() {
            let Some(item) = slots.get(menu_slot) else {
                continue;
            };
            if item.is_empty() {
                continue;
            }
            *counts.entry(item.kind().to_string()).or_insert(0) += item.count().max(0) as u32;
        }

        if let Some(current) = self.scaffold_item.as_deref() {
            let held = counts.get(current).copied().unwrap_or(0);
            let still_allowed = self.scaffold.allowed.iter().any(|id| id == current)
                && !self.scaffold.denied.iter().any(|id| id == current);
            if held > 0 && still_allowed {
                self.scaffold_available = held;
                return;
            }
        }

        self.scaffold_item = vertical::select_scaffold_block(&self.scaffold, &counts);
        self.scaffold_available = self
            .scaffold_item
            .as_deref()
            .map_or(0, |id| counts.get(id).copied().unwrap_or(0));
    }
}

/// Refreshes the live half of whatever [`PathfindingPolicy`] is already
/// stored in `state` (a no-op if none was ever inserted -- there is nothing
/// to keep in sync). Uses [`parking_lot::RwLock::try_write`] rather than
/// `write` per [`CustomPathfinderState`]'s own documented caution: a read
/// lock may be held for the duration of an in-flight A* search, so this must
/// never block the caller (a stale policy for one replanning pass is
/// harmless; a stalled game tick is not).
pub fn refresh_pathfinding_policy(state: &CustomPathfinderState, menu: &Menu) {
    let Some(mut inner) = state.0.try_write() else {
        return;
    };
    let Some(mut policy) = inner.get::<PathfindingPolicy>().cloned() else {
        return;
    };
    policy.refresh_scaffold(menu);
    inner.insert(policy);
}

#[cfg(test)]
mod tests {
    use azalea_inventory::{ItemStack, Player};
    use azalea_registry::builtin::ItemKind;

    use super::*;

    fn menu_with_hotbar_item(item: ItemKind, count: i32) -> Menu {
        let mut player = Player::default();
        let hotbar_start = *Menu::Player(Player::default()).hotbar_slots_range().start();
        let mut menu = Menu::Player(std::mem::take(&mut player));
        if let Some(slot) = menu.slot_mut(hotbar_start) {
            *slot = ItemStack::new(item, count);
        }
        menu
    }

    fn policy_with_scaffold(allowed: &[&str], minimum_held: u32) -> PathfindingPolicy {
        PathfindingPolicy {
            scaffold: vertical::ScaffoldPolicy {
                allowed: allowed.iter().map(|s| s.to_string()).collect(),
                denied: Vec::new(),
                minimum_held,
            },
            ..Default::default()
        }
    }

    #[test]
    fn refresh_scaffold_keeps_selected_item_below_minimum_held() {
        let mut policy = policy_with_scaffold(&["minecraft:cobblestone"], 4);
        let full_menu = menu_with_hotbar_item(ItemKind::Cobblestone, 4);
        policy.refresh_scaffold(&full_menu);
        assert_eq!(
            policy.scaffold_item.as_deref(),
            Some("minecraft:cobblestone")
        );
        assert!(policy.can_place());

        // Simulate having placed 1 of the 4 -- held count drops below
        // `minimum_held` but the item is still selected and still available.
        let depleted_menu = menu_with_hotbar_item(ItemKind::Cobblestone, 3);
        policy.refresh_scaffold(&depleted_menu);
        assert_eq!(
            policy.scaffold_item.as_deref(),
            Some("minecraft:cobblestone")
        );
        assert_eq!(policy.scaffold_available, 3);
        assert!(policy.can_place());
    }

    #[test]
    fn refresh_scaffold_drops_selection_once_fully_depleted() {
        let mut policy = policy_with_scaffold(&["minecraft:cobblestone"], 1);
        let menu = menu_with_hotbar_item(ItemKind::Cobblestone, 1);
        policy.refresh_scaffold(&menu);
        assert!(policy.can_place());

        let empty_menu = menu_with_hotbar_item(ItemKind::Cobblestone, 0);
        policy.refresh_scaffold(&empty_menu);
        assert_eq!(policy.scaffold_item, None);
        assert!(!policy.can_place());
    }

    #[test]
    fn refresh_scaffold_requires_minimum_held_to_select_a_new_item() {
        let mut policy = policy_with_scaffold(&["minecraft:cobblestone"], 4);
        let low_menu = menu_with_hotbar_item(ItemKind::Cobblestone, 2);
        policy.refresh_scaffold(&low_menu);
        assert_eq!(
            policy.scaffold_item, None,
            "starting a route with fewer than minimum_held should not select the item"
        );
    }
}
