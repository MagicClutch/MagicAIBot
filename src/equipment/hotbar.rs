//! Automatic hotbar equipment: keeps the best sword/pickaxe/axe/shovel/
//! building block/water bucket (plus any configured utility items) in a
//! designated hotbar slot, swapping in a better one automatically -- works
//! alongside `equipment::manager::EquipmentService` (armor/offhand) rather
//! than replacing it. See `crate::config::HotbarEquipmentConfig`.
//!
//! Reuses the existing scoring/preference systems instead of inventing a
//! new one: `equipment::tools` for sword/pickaxe/axe/shovel (itself built on
//! `interaction::tool_selection`'s material tier/category tables), and
//! `crate::bridging::scaffold_allow_list` (the same preference/blacklist
//! bridging already uses for scaffold material) for blocks. The one new
//! primitive is `equipment::manager::swap_into_slot`, and that's shared
//! with `EquipmentService` too.
//!
//! Re-evaluation is event-driven: `InventorySnapshot::revision` only
//! changes when slot contents actually do (item pickup, a crafted item, a
//! manual inventory move, ...), so a tick that sees the same revision as
//! last time does no work beyond that one integer comparison.
//! `periodic_scan_seconds` is only a fallback safety net, not the primary
//! trigger -- see [`HotbarEquipmentService::tick`].

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::sync::Mutex;
use tracing::debug;

use crate::{
    bridging,
    config::{AutoDropConfig, HotbarEquipmentConfig, ToolEquipmentConfig},
    equipment::{
        autodrop::{drop_displaced_item, should_drop},
        manager::swap_into_slot,
        model::{EquipmentItem, HOTBAR_PROTOCOL_SLOTS},
        tools,
    },
    interaction::tool_selection::ToolCategory,
    logging,
    minecraft::client::MinecraftClient,
};

const WATER_BUCKET_ID: &str = "minecraft:water_bucket";

#[derive(Clone, Copy, Debug)]
enum AutoDropGroup {
    Tools,
    Weapons,
}

#[derive(Default)]
struct Inner {
    last_seen_revision: Option<u64>,
    last_scan_at: Option<Instant>,
}

#[derive(Clone)]
pub struct HotbarEquipmentService {
    hotbar: HotbarEquipmentConfig,
    tools: ToolEquipmentConfig,
    autodrop: AutoDropConfig,
    inner: Arc<Mutex<Inner>>,
}

impl HotbarEquipmentService {
    pub fn new(
        hotbar: HotbarEquipmentConfig,
        tools: ToolEquipmentConfig,
        autodrop: AutoDropConfig,
    ) -> Self {
        Self {
            hotbar,
            tools,
            autodrop,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// Cheap on every call while nothing has changed: fetches the live
    /// inventory (needed either way to read its revision), compares that
    /// revision against the last one actually evaluated, and only does the
    /// real per-category work when it changed or the periodic fallback is
    /// due.
    pub async fn tick(&self, minecraft: &MinecraftClient) {
        if !self.hotbar.enabled {
            return;
        }
        let world = minecraft.world_state_snapshot().await;
        if !world.joined_world() || !world.inventory.available {
            return;
        }
        let revision = world.inventory.revision;
        let due = {
            let inner = self.inner.lock().await;
            inner.last_seen_revision != Some(revision)
                || inner.last_scan_at.is_none_or(|at| {
                    at.elapsed() >= Duration::from_secs(self.hotbar.periodic_scan_seconds)
                })
        };
        if !due {
            return;
        }
        {
            let mut inner = self.inner.lock().await;
            inner.last_seen_revision = Some(revision);
            inner.last_scan_at = Some(Instant::now());
        }
        let Ok(snapshot) = minecraft.equipment_snapshot().await else {
            return;
        };
        self.evaluate(minecraft, &snapshot.inventory).await;
    }

    /// Runs every configured category against one consistent inventory
    /// snapshot. A swap partway through makes the snapshot slightly stale
    /// for whatever runs after it in the same pass (e.g. a tool that was
    /// sitting in another category's target slot) -- accepted rather than
    /// re-fetching between every category, since that revision change
    /// self-corrects on the very next tick (see `tick`'s doc comment) and
    /// re-fetching per-category would mean far more network round trips for
    /// an edge case this rare.
    async fn evaluate(&self, minecraft: &MinecraftClient, inventory: &[EquipmentItem]) {
        self.evaluate_tool_category(
            minecraft,
            ToolCategory::Sword,
            AutoDropGroup::Weapons,
            self.hotbar.slots.sword,
            inventory,
        )
        .await;
        self.evaluate_tool_category(
            minecraft,
            ToolCategory::Pickaxe,
            AutoDropGroup::Tools,
            self.hotbar.slots.pickaxe,
            inventory,
        )
        .await;
        self.evaluate_tool_category(
            minecraft,
            ToolCategory::Axe,
            AutoDropGroup::Tools,
            self.hotbar.slots.axe,
            inventory,
        )
        .await;
        self.evaluate_tool_category(
            minecraft,
            ToolCategory::Shovel,
            AutoDropGroup::Tools,
            self.hotbar.slots.shovel,
            inventory,
        )
        .await;
        self.evaluate_blocks(minecraft, self.hotbar.slots.blocks, inventory)
            .await;
        self.evaluate_exact_item(
            minecraft,
            WATER_BUCKET_ID,
            self.hotbar.slots.water_bucket,
            inventory,
        )
        .await;
        for utility in &self.hotbar.utility_items {
            self.evaluate_exact_item(minecraft, &utility.item_id, Some(utility.slot), inventory)
                .await;
        }
    }

    async fn evaluate_tool_category(
        &self,
        minecraft: &MinecraftClient,
        category: ToolCategory,
        group: AutoDropGroup,
        slot_number: Option<u8>,
        inventory: &[EquipmentItem],
    ) {
        let Some(slot_number) = slot_number else {
            return;
        };
        let target = protocol_slot(slot_number);
        let held = inventory.iter().find(|item| item.slot == target);
        let Some(candidate) = tools::best_candidate(self.tools.ranking_mode, category, inventory)
        else {
            return;
        };
        if candidate.item.slot == target {
            return;
        }
        if !tools::should_replace(self.tools.ranking_mode, held, candidate) {
            debug!(
                ?category,
                held_score = held.map(tools::rank_score),
                candidate_score = tools::rank_score(candidate.item),
                "hotbar: best available candidate is not enough of an improvement over the held item"
            );
            return;
        }
        debug!(
            ?category,
            item = %candidate.item.item_id,
            candidate_score = tools::rank_score(candidate.item),
            target_slot = slot_number,
            "hotbar: found a better item"
        );
        let source_slot = candidate.item.slot;
        let displaced = held.map(|item| item.item_id.clone());
        if !swap_into_slot(minecraft, source_slot, target).await {
            return;
        }
        logging::info(format!(
            "Equipped {} in hotbar slot {slot_number}",
            candidate.item.item_id
        ));
        if let Some(displaced_id) = displaced {
            self.maybe_drop(minecraft, group, &displaced_id, source_slot)
                .await;
        }
    }

    /// Building material: reuses `bridging::scaffold_allow_list` directly
    /// (preferred blocks first, then any other non-blacklisted block held)
    /// rather than a score -- blocks have no durability/tier to weigh, just
    /// bridging's existing preference order.
    async fn evaluate_blocks(
        &self,
        minecraft: &MinecraftClient,
        slot_number: Option<u8>,
        inventory: &[EquipmentItem],
    ) {
        let Some(slot_number) = slot_number else {
            return;
        };
        let target = protocol_slot(slot_number);
        let held_ids: Vec<&str> = inventory.iter().map(|item| item.item_id.as_str()).collect();
        let preference = bridging::scaffold_allow_list(held_ids.iter().copied());
        let Some(best_id) = preference.first() else {
            return;
        };
        let currently_equipped = inventory
            .iter()
            .find(|item| item.slot == target)
            .is_some_and(|item| &item.item_id == best_id);
        if currently_equipped {
            return;
        }
        let Some(source) = inventory
            .iter()
            .find(|item| &item.item_id == best_id && item.slot != target)
        else {
            return;
        };
        debug!(block = %best_id, target_slot = slot_number, "hotbar: found a more preferred building block");
        if swap_into_slot(minecraft, source.slot, target).await {
            logging::info(format!("Equipped {best_id} in hotbar slot {slot_number}"));
        }
    }

    /// Water bucket / configured utility items: no ranking, just "keep
    /// exactly this item id stocked here if it's held anywhere else" --
    /// the hotbar equivalent of `EquipmentService`'s offhand stocking.
    async fn evaluate_exact_item(
        &self,
        minecraft: &MinecraftClient,
        item_id: &str,
        slot_number: Option<u8>,
        inventory: &[EquipmentItem],
    ) {
        let Some(slot_number) = slot_number else {
            return;
        };
        let target = protocol_slot(slot_number);
        let already_there = inventory
            .iter()
            .any(|item| item.slot == target && item.item_id == item_id);
        if already_there {
            return;
        }
        let Some(source) = inventory
            .iter()
            .find(|item| item.item_id == item_id && item.slot != target)
        else {
            return;
        };
        debug!(item = %item_id, target_slot = slot_number, "hotbar: stocking configured item");
        if swap_into_slot(minecraft, source.slot, target).await {
            logging::info(format!("Stocked {item_id} in hotbar slot {slot_number}"));
        }
    }

    async fn maybe_drop(
        &self,
        minecraft: &MinecraftClient,
        group: AutoDropGroup,
        item_id: &str,
        slot: usize,
    ) {
        let category = match group {
            AutoDropGroup::Tools => &self.autodrop.tools,
            AutoDropGroup::Weapons => &self.autodrop.weapons,
        };
        if !should_drop(
            self.autodrop.enabled,
            category,
            item_id,
            &self.autodrop.protected_items,
        ) {
            debug!(item = %item_id, ?group, "hotbar: not dropping the displaced item (autodrop disabled or protected)");
            return;
        }
        if drop_displaced_item(minecraft, slot).await {
            logging::info(format!("Dropped {item_id}"));
        } else {
            debug!(item = %item_id, "hotbar: drop attempt failed");
        }
    }
}

/// 1-9 (as typed/shown in-game) to the protocol slot index -- config
/// validation (`Config::validate`) already guarantees every configured slot
/// is in that range.
fn protocol_slot(slot_number: u8) -> usize {
    *HOTBAR_PROTOCOL_SLOTS.start() + usize::from(slot_number.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AccountMode, BridgingConfig, ConsoleConfig, MinecraftConfig, ReconnectConfig,
        VerticalNavigationConfig, WorldStateConfig,
    };

    fn minecraft() -> MinecraftClient {
        MinecraftClient::new(
            MinecraftConfig {
                server: "localhost:25565".to_owned(),
                username: "MagicBot".to_owned(),
                account_mode: AccountMode::Offline,
            },
            ReconnectConfig {
                enabled: false,
                delay_seconds: 10,
                maximum_attempts: 5,
            },
            ConsoleConfig::default(),
            WorldStateConfig::default(),
            VerticalNavigationConfig::default(),
            BridgingConfig::default(),
        )
    }

    fn service() -> HotbarEquipmentService {
        HotbarEquipmentService::new(
            HotbarEquipmentConfig::default(),
            ToolEquipmentConfig::default(),
            AutoDropConfig::default(),
        )
    }

    #[test]
    fn protocol_slot_maps_in_game_slot_one_to_the_first_hotbar_protocol_slot() {
        assert_eq!(protocol_slot(1), 36);
        assert_eq!(protocol_slot(9), 44);
    }

    #[tokio::test]
    async fn ticking_without_a_connection_does_not_panic() {
        service().tick(&minecraft()).await;
    }

    #[tokio::test]
    async fn disabled_config_short_circuits_before_touching_world_state() {
        let disabled = HotbarEquipmentService::new(
            HotbarEquipmentConfig {
                enabled: false,
                ..HotbarEquipmentConfig::default()
            },
            ToolEquipmentConfig::default(),
            AutoDropConfig::default(),
        );
        disabled.tick(&minecraft()).await;
    }
}
