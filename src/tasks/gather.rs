//! Bounded, policy-driven resource gathering orchestration.
//!
//! This module deliberately owns no Minecraft mechanics.  It composes the
//! inventory, storage, tool, search, navigation, breaking, pickup, crafting,
//! and smelting boundaries through [`GatherBackend`].  Consequently the same
//! state machine is usable by the Azalea adapter and deterministic simulations.

use std::{collections::BTreeSet, fmt, time::Duration};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceFamily {
    Logs,
    Stone,
    VisibleOre,
    Food,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatherResource {
    pub item: String,
    pub family: ResourceFamily,
    pub sources: &'static [&'static str],
    pub tool: Option<&'static str>,
    pub craftable: bool,
    pub smeltable: bool,
}

/// The intentionally small allow-list. Ores are searched only as ordinary,
/// loaded blocks; a backend must never discover them from unloaded/hidden data.
pub fn supported_resource(input: &str) -> Option<GatherResource> {
    let id = input
        .strip_prefix("minecraft:")
        .unwrap_or(input)
        .to_ascii_lowercase();
    let (item, family, sources, tool, craftable, smeltable) = match id.as_str() {
        "log" | "logs" | "oak_log" => (
            "minecraft:oak_log",
            ResourceFamily::Logs,
            &[
                "minecraft:oak_log",
                "minecraft:birch_log",
                "minecraft:spruce_log",
            ][..],
            Some("axe"),
            false,
            false,
        ),
        "stone" | "cobblestone" => (
            "minecraft:cobblestone",
            ResourceFamily::Stone,
            &["minecraft:stone"][..],
            Some("pickaxe"),
            false,
            false,
        ),
        "coal" | "coal_ore" => (
            "minecraft:coal",
            ResourceFamily::VisibleOre,
            &["minecraft:coal_ore", "minecraft:deepslate_coal_ore"][..],
            Some("pickaxe"),
            false,
            false,
        ),
        "raw_iron" | "iron_ore" => (
            "minecraft:raw_iron",
            ResourceFamily::VisibleOre,
            &["minecraft:iron_ore", "minecraft:deepslate_iron_ore"][..],
            Some("stone_pickaxe"),
            false,
            false,
        ),
        "iron_ingot" => (
            "minecraft:iron_ingot",
            ResourceFamily::VisibleOre,
            &["minecraft:iron_ore", "minecraft:deepslate_iron_ore"][..],
            Some("stone_pickaxe"),
            false,
            true,
        ),
        "diamond" | "diamond_ore" => (
            "minecraft:diamond",
            ResourceFamily::VisibleOre,
            &["minecraft:diamond_ore", "minecraft:deepslate_diamond_ore"][..],
            Some("iron_pickaxe"),
            false,
            false,
        ),
        "apple" => (
            "minecraft:apple",
            ResourceFamily::Food,
            &["minecraft:oak_leaves", "minecraft:dark_oak_leaves"][..],
            None,
            false,
            false,
        ),
        "carrot" => (
            "minecraft:carrot",
            ResourceFamily::Food,
            &["minecraft:carrots"][..],
            None,
            false,
            false,
        ),
        "potato" => (
            "minecraft:potato",
            ResourceFamily::Food,
            &["minecraft:potatoes"][..],
            None,
            false,
            false,
        ),
        "wheat" => (
            "minecraft:wheat",
            ResourceFamily::Food,
            &["minecraft:wheat"][..],
            None,
            false,
            false,
        ),
        "bread" => (
            "minecraft:bread",
            ResourceFamily::Food,
            &["minecraft:wheat"][..],
            None,
            true,
            false,
        ),
        "baked_potato" => (
            "minecraft:baked_potato",
            ResourceFamily::Food,
            &["minecraft:potatoes"][..],
            None,
            false,
            true,
        ),
        _ => return None,
    };
    Some(GatherResource {
        item: item.into(),
        family,
        sources,
        tool,
        craftable,
        smeltable,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatherRequest {
    pub resource: GatherResource,
    pub quantity: u32,
    pub deposit_to_chest: bool,
    pub limits: GatherLimits,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatherLimits {
    pub candidates: u32,
    pub failures: u32,
    pub travel_blocks: u32,
    pub operations: u32,
    pub timeout: Duration,
}
impl Default for GatherLimits {
    fn default() -> Self {
        Self {
            candidates: 32,
            failures: 8,
            travel_blocks: 128,
            operations: 64,
            timeout: Duration::from_secs(180),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TargetId(pub i64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopReason {
    Complete,
    Absent,
    Unreachable,
    MissingPrerequisite,
    InventoryFull,
    WorldChanged,
    Cancelled,
    Died,
    Disconnected,
    TimedOut,
    LimitReached,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatherResult {
    pub requested: u32,
    pub collected: u32,
    pub remaining: u32,
    pub reason: StopReason,
    pub candidates_tried: u32,
    pub operations: u32,
    pub travelled: u32,
    pub failed_targets: Vec<TargetId>,
}
impl fmt::Display for GatherResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "gathered {}/{} ({} remaining): {:?}; {} candidates, {} operations, {} blocks travelled",
            self.collected,
            self.requested,
            self.remaining,
            self.reason,
            self.candidates_tried,
            self.operations,
            self.travelled
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendFailure {
    Absent,
    Unreachable,
    MissingPrerequisite,
    InventoryFull,
    WorldChanged,
    Cancelled,
    Died,
    Disconnected,
}

/// Adapter over Phase 2/3 services. Every mutating method must return only
/// after its underlying service has authoritatively confirmed the operation.
pub trait GatherBackend {
    fn inventory_count(&mut self, item: &str) -> u32;
    fn elapsed(&mut self) -> Duration;
    fn check_session(&mut self) -> Result<(), BackendFailure>;
    fn ensure_capacity(&mut self, item: &str, amount: u32) -> Result<(), BackendFailure>;
    fn cleanup_or_deposit(&mut self, deposit: bool) -> Result<(), BackendFailure>;
    fn ensure_tool(&mut self, tool: Option<&str>) -> Result<(), BackendFailure>;
    fn find_loaded_visible_source(
        &mut self,
        blocks: &[&str],
        excluded: &BTreeSet<TargetId>,
    ) -> Result<TargetId, BackendFailure>;
    fn navigate(&mut self, target: TargetId, travel_left: u32) -> Result<u32, BackendFailure>;
    fn harvest_intentionally(&mut self, target: TargetId) -> Result<(), BackendFailure>;
    fn pickup_requested_drop(&mut self, item: &str) -> Result<(), BackendFailure>;
    fn craft_requested(&mut self, item: &str, maximum: u32) -> Result<(), BackendFailure>;
    fn smelt_requested(&mut self, item: &str, maximum: u32) -> Result<(), BackendFailure>;
}

fn reason(f: BackendFailure) -> StopReason {
    match f {
        BackendFailure::Absent => StopReason::Absent,
        BackendFailure::Unreachable => StopReason::Unreachable,
        BackendFailure::MissingPrerequisite => StopReason::MissingPrerequisite,
        BackendFailure::InventoryFull => StopReason::InventoryFull,
        BackendFailure::WorldChanged => StopReason::WorldChanged,
        BackendFailure::Cancelled => StopReason::Cancelled,
        BackendFailure::Died => StopReason::Died,
        BackendFailure::Disconnected => StopReason::Disconnected,
    }
}

/// Runs the finite gather state machine. Progress is derived exclusively from
/// inventory deltas; successful service calls never imply an item was gained.
pub fn gather<B: GatherBackend>(backend: &mut B, request: &GatherRequest) -> GatherResult {
    let start = backend.inventory_count(&request.resource.item);
    let mut failed = BTreeSet::new();
    let mut candidates = 0;
    let mut operations = 0;
    let mut travelled = 0;
    let finish = |backend: &mut B, why, failed: &BTreeSet<_>, candidates, operations, travelled| {
        let collected = backend
            .inventory_count(&request.resource.item)
            .saturating_sub(start)
            .min(request.quantity);
        GatherResult {
            requested: request.quantity,
            collected,
            remaining: request.quantity - collected,
            reason: why,
            candidates_tried: candidates,
            operations,
            travelled,
            failed_targets: failed.iter().copied().collect(),
        }
    };
    if request.quantity == 0 {
        return finish(backend, StopReason::Complete, &failed, 0, 0, 0);
    }
    loop {
        let gained = backend
            .inventory_count(&request.resource.item)
            .saturating_sub(start);
        if gained >= request.quantity {
            return finish(
                backend,
                StopReason::Complete,
                &failed,
                candidates,
                operations,
                travelled,
            );
        }
        if backend.elapsed() >= request.limits.timeout {
            return finish(
                backend,
                StopReason::TimedOut,
                &failed,
                candidates,
                operations,
                travelled,
            );
        }
        if operations >= request.limits.operations
            || candidates >= request.limits.candidates
            || failed.len() as u32 >= request.limits.failures
            || travelled >= request.limits.travel_blocks
        {
            return finish(
                backend,
                StopReason::LimitReached,
                &failed,
                candidates,
                operations,
                travelled,
            );
        }
        for action in [
            backend.check_session(),
            backend.ensure_capacity(&request.resource.item, request.quantity - gained),
        ] {
            if let Err(e) = action {
                if e == BackendFailure::InventoryFull
                    && backend.cleanup_or_deposit(request.deposit_to_chest).is_ok()
                {
                    continue;
                }
                return finish(
                    backend,
                    reason(e),
                    &failed,
                    candidates,
                    operations,
                    travelled,
                );
            }
        }
        if let Err(e) = backend.ensure_tool(request.resource.tool) {
            return finish(
                backend,
                reason(e),
                &failed,
                candidates,
                operations,
                travelled,
            );
        }
        let target = match backend.find_loaded_visible_source(request.resource.sources, &failed) {
            Ok(t) => t,
            Err(e) => {
                // Recipes/smelting are prerequisites, not alternate autonomous plans.
                let remaining = request.quantity - gained;
                let converted = if request.resource.craftable {
                    backend.craft_requested(&request.resource.item, remaining)
                } else if request.resource.smeltable {
                    backend.smelt_requested(&request.resource.item, remaining)
                } else {
                    Err(e)
                };
                if converted.is_ok() {
                    operations += 1;
                    continue;
                }
                return finish(
                    backend,
                    reason(converted.err().unwrap_or(e)),
                    &failed,
                    candidates,
                    operations,
                    travelled,
                );
            }
        };
        candidates += 1;
        match backend.navigate(target, request.limits.travel_blocks - travelled) {
            Ok(distance) => travelled += distance,
            Err(BackendFailure::Unreachable) => {
                failed.insert(target);
                continue;
            }
            Err(e) => {
                return finish(
                    backend,
                    reason(e),
                    &failed,
                    candidates,
                    operations,
                    travelled,
                );
            }
        }
        operations += 1;
        if let Err(e) = backend.harvest_intentionally(target) {
            failed.insert(target);
            if !matches!(
                e,
                BackendFailure::WorldChanged | BackendFailure::Unreachable
            ) {
                return finish(
                    backend,
                    reason(e),
                    &failed,
                    candidates,
                    operations,
                    travelled,
                );
            }
            continue;
        }
        if let Err(e) = backend.pickup_requested_drop(&request.resource.item) {
            if matches!(
                e,
                BackendFailure::Cancelled
                    | BackendFailure::Died
                    | BackendFailure::Disconnected
                    | BackendFailure::InventoryFull
            ) {
                return finish(
                    backend,
                    reason(e),
                    &failed,
                    candidates,
                    operations,
                    travelled,
                );
            }
            failed.insert(target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Mock {
        count: u32,
        elapsed: Duration,
        targets: VecDeque<TargetId>,
        nav: VecDeque<Result<u32, BackendFailure>>,
        harvest: VecDeque<Result<(), BackendFailure>>,
        pickup: VecDeque<Result<u32, BackendFailure>>,
        terminal: Option<BackendFailure>,
        capacity: Result<(), BackendFailure>,
        cleanup: Result<(), BackendFailure>,
        prereq: Result<(), BackendFailure>,
        convert: Result<u32, BackendFailure>,
    }
    impl Default for Mock {
        fn default() -> Self {
            Self {
                count: 0,
                elapsed: Duration::ZERO,
                targets: VecDeque::new(),
                nav: VecDeque::new(),
                harvest: VecDeque::new(),
                pickup: VecDeque::new(),
                terminal: None,
                capacity: Ok(()),
                cleanup: Err(BackendFailure::InventoryFull),
                prereq: Ok(()),
                convert: Err(BackendFailure::MissingPrerequisite),
            }
        }
    }
    impl GatherBackend for Mock {
        fn inventory_count(&mut self, _: &str) -> u32 {
            self.count
        }
        fn elapsed(&mut self) -> Duration {
            self.elapsed
        }
        fn check_session(&mut self) -> Result<(), BackendFailure> {
            self.terminal.map_or(Ok(()), Err)
        }
        fn ensure_capacity(&mut self, _: &str, _: u32) -> Result<(), BackendFailure> {
            self.capacity
        }
        fn cleanup_or_deposit(&mut self, _: bool) -> Result<(), BackendFailure> {
            let r = self.cleanup;
            if r.is_ok() {
                self.capacity = Ok(())
            }
            r
        }
        fn ensure_tool(&mut self, _: Option<&str>) -> Result<(), BackendFailure> {
            self.prereq
        }
        fn find_loaded_visible_source(
            &mut self,
            _: &[&str],
            excluded: &BTreeSet<TargetId>,
        ) -> Result<TargetId, BackendFailure> {
            while let Some(t) = self.targets.pop_front() {
                if !excluded.contains(&t) {
                    return Ok(t);
                }
            }
            Err(BackendFailure::Absent)
        }
        fn navigate(&mut self, _: TargetId, _: u32) -> Result<u32, BackendFailure> {
            self.nav.pop_front().unwrap_or(Ok(1))
        }
        fn harvest_intentionally(&mut self, _: TargetId) -> Result<(), BackendFailure> {
            self.harvest.pop_front().unwrap_or(Ok(()))
        }
        fn pickup_requested_drop(&mut self, _: &str) -> Result<(), BackendFailure> {
            match self.pickup.pop_front().unwrap_or(Ok(1)) {
                Ok(n) => {
                    self.count += n;
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        fn craft_requested(&mut self, _: &str, _: u32) -> Result<(), BackendFailure> {
            match self.convert {
                Ok(n) => {
                    self.count += n;
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        fn smelt_requested(&mut self, i: &str, n: u32) -> Result<(), BackendFailure> {
            self.craft_requested(i, n)
        }
    }
    fn run(name: &str, n: u32, m: &mut Mock) -> GatherResult {
        gather(
            m,
            &GatherRequest {
                resource: supported_resource(name).unwrap(),
                quantity: n,
                deposit_to_chest: true,
                limits: GatherLimits::default(),
            },
        )
    }
    #[test]
    fn all_resource_families_confirm_inventory() {
        for name in ["logs", "stone", "diamond", "carrot"] {
            let mut m = Mock {
                targets: [TargetId(1), TargetId(2)].into(),
                pickup: [Ok(1), Ok(2)].into(),
                ..Default::default()
            };
            assert_eq!(run(name, 2, &mut m).reason, StopReason::Complete)
        }
    }
    #[test]
    fn crafting_and_smelting_prerequisites() {
        for name in ["bread", "iron_ingot", "baked_potato"] {
            let mut m = Mock {
                convert: Ok(3),
                ..Default::default()
            };
            let r = run(name, 3, &mut m);
            assert_eq!((r.reason, r.collected), (StopReason::Complete, 3))
        }
    }
    #[test]
    fn unreachable_and_changed_targets_are_remembered() {
        let mut m = Mock {
            targets: [TargetId(1), TargetId(2), TargetId(3)].into(),
            nav: [Err(BackendFailure::Unreachable), Ok(1), Ok(1)].into(),
            harvest: [Err(BackendFailure::WorldChanged), Ok(())].into(),
            pickup: [Ok(1)].into(),
            ..Default::default()
        };
        let r = run("stone", 1, &mut m);
        assert_eq!(r.failed_targets, vec![TargetId(1), TargetId(2)]);
        assert_eq!(r.reason, StopReason::Complete)
    }
    #[test]
    fn capacity_cleanup_and_deposit_recovers() {
        let mut m = Mock {
            capacity: Err(BackendFailure::InventoryFull),
            cleanup: Ok(()),
            targets: [TargetId(1)].into(),
            pickup: [Ok(1)].into(),
            ..Default::default()
        };
        assert_eq!(run("logs", 1, &mut m).reason, StopReason::Complete)
    }
    #[test]
    fn precise_partial_terminal_paths() {
        for (failure, reason) in [
            (BackendFailure::Cancelled, StopReason::Cancelled),
            (BackendFailure::Died, StopReason::Died),
            (BackendFailure::Disconnected, StopReason::Disconnected),
        ] {
            let mut m = Mock {
                count: 1,
                terminal: Some(failure),
                ..Default::default()
            };
            let r = run("stone", 3, &mut m);
            assert_eq!((r.reason, r.collected, r.remaining), (reason, 0, 3))
        }
        let mut m = Mock {
            targets: [TargetId(1)].into(),
            pickup: [Ok(1)].into(),
            prereq: Err(BackendFailure::MissingPrerequisite),
            ..Default::default()
        };
        assert_eq!(
            run("diamond", 2, &mut m).reason,
            StopReason::MissingPrerequisite
        )
    }
    #[test]
    fn absent_full_timeout_and_limits() {
        let mut m = Mock::default();
        assert_eq!(run("stone", 1, &mut m).reason, StopReason::Absent);
        let mut m = Mock {
            capacity: Err(BackendFailure::InventoryFull),
            ..Default::default()
        };
        assert_eq!(run("stone", 1, &mut m).reason, StopReason::InventoryFull);
        let mut m = Mock {
            elapsed: Duration::from_secs(999),
            ..Default::default()
        };
        assert_eq!(run("stone", 1, &mut m).reason, StopReason::TimedOut)
    }
    #[test]
    fn zero_drop_does_not_count_as_progress() {
        let mut m = Mock {
            targets: [TargetId(1)].into(),
            pickup: [Ok(0)].into(),
            ..Default::default()
        };
        let r = run("stone", 1, &mut m);
        assert_eq!((r.collected, r.reason), (0, StopReason::Absent))
    }
}
