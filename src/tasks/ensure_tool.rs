//! Policy-only orchestration for obtaining a harvesting tool.
//!
//! This module deliberately knows nothing about windows or packet clicks.  It
//! composes inventory, recipe, crafting and smelting adapters and is therefore
//! usable by both the live client and deterministic simulations.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolRequest {
    pub block: String,
    pub category: ToolCategory,
    pub minimum_tier: MaterialTier,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ToolCategory {
    Pickaxe,
    Axe,
    Shovel,
    Hoe,
    Shears,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MaterialTier {
    Wood,
    Stone,
    Gold,
    Iron,
    Diamond,
    Netherite,
}

impl MaterialTier {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Wood => "wooden",
            Self::Stone => "stone",
            Self::Gold => "golden",
            Self::Iron => "iron",
            Self::Diamond => "diamond",
            Self::Netherite => "netherite",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolPolicy {
    /// Most desirable first. Tiers absent from this list are rejected.
    pub tier_preference: Vec<MaterialTier>,
    pub durability_reserve: u32,
    pub allow_smelting: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemStack {
    pub slot: u16,
    pub item: String,
    pub count: u32,
    pub durability_left: Option<u32>,
    pub protected: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryView {
    pub revision: u64,
    pub empty_slots: usize,
    pub stacks: Vec<ItemStack>,
}

impl InventoryView {
    fn count(&self, item: &str) -> u32 {
        self.stacks
            .iter()
            .filter(|s| s.item == item && !s.protected)
            .map(|s| s.count)
            .sum()
    }
    fn usable_tool(&self, item: &str, reserve: u32) -> Option<u16> {
        self.stacks
            .iter()
            .find(|s| {
                s.item == item
                    && !s.protected
                    && s.count > 0
                    && s.durability_left.is_none_or(|d| d > reserve)
            })
            .map(|s| s.slot)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProductionKind {
    Craft { station: Option<String> },
    Smelt { station: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductionStep {
    pub output: String,
    pub output_count: u32,
    pub times: u32,
    pub inputs: BTreeMap<String, u32>,
    pub kind: ProductionKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequirementNode {
    pub item: String,
    pub required: u32,
    pub available: u32,
    pub operation: Option<ProductionKind>,
    pub children: Vec<RequirementNode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductionPlan {
    pub target: RequirementNode,
    pub steps: Vec<ProductionStep>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanFailure {
    MissingMaterials {
        requirements: RequirementNode,
        missing: BTreeMap<String, u32>,
    },
    MissingStation {
        requirements: RequirementNode,
        station: String,
    },
    Rejected(String),
}

pub trait RecipePlanner {
    /// Plans only from the supplied inventory. It must return dependency-first
    /// steps and the exact requirement tree used to derive them.
    fn plan(
        &self,
        item: &str,
        inventory: &InventoryView,
        allow_smelting: bool,
    ) -> Result<ProductionPlan, PlanFailure>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionFailure {
    MissingStation(String),
    NoSpace,
    Rejected(String),
    Partial { produced: u32, expected: u32 },
    Timeout,
    Cancelled,
    Died,
    Disconnected,
}

pub trait ToolExecution {
    fn inventory(&mut self) -> Result<InventoryView, ExecutionFailure>;
    fn craft(
        &mut self,
        step: &ProductionStep,
        expected_revision: u64,
    ) -> Result<(), ExecutionFailure>;
    fn smelt(
        &mut self,
        step: &ProductionStep,
        expected_revision: u64,
    ) -> Result<(), ExecutionFailure>;
    fn reserve_and_select(
        &mut self,
        slot: u16,
        expected_revision: u64,
    ) -> Result<(), ExecutionFailure>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnsureToolOutcome {
    Ready {
        item: String,
        slot: u16,
        crafted: bool,
        requirements: RequirementNode,
    },
    MissingMaterials {
        requirements: RequirementNode,
        missing: BTreeMap<String, u32>,
    },
    MissingStation {
        requirements: Option<RequirementNode>,
        station: String,
    },
    NoSpace {
        requirements: RequirementNode,
    },
    Rejected {
        requirements: Option<RequirementNode>,
        reason: String,
    },
    Partial {
        requirements: RequirementNode,
        produced: u32,
        expected: u32,
    },
    Timeout {
        requirements: Option<RequirementNode>,
    },
    Cancelled {
        requirements: Option<RequirementNode>,
    },
    Died {
        requirements: Option<RequirementNode>,
    },
    Disconnected {
        requirements: Option<RequirementNode>,
    },
}

pub struct EnsureTool<'a, P, E> {
    pub planner: &'a P,
    pub execution: &'a mut E,
    pub policy: &'a ToolPolicy,
}

impl<P: RecipePlanner, E: ToolExecution> EnsureTool<'_, P, E> {
    pub fn run(&mut self, request: &ToolRequest) -> EnsureToolOutcome {
        let initial = match self.execution.inventory() {
            Ok(i) => i,
            Err(e) => return map_execution(e, None),
        };
        let candidates = candidates(request, self.policy);
        if candidates.is_empty() {
            return EnsureToolOutcome::Rejected {
                requirements: None,
                reason: "no configured tier satisfies the request".into(),
            };
        }

        // Existing usable tools always win; never consume protected equipment.
        for item in &candidates {
            if let Some(slot) = initial.usable_tool(item, self.policy.durability_reserve) {
                let root = RequirementNode {
                    item: item.clone(),
                    required: 1,
                    available: 1,
                    operation: None,
                    children: vec![],
                };
                return match self.execution.reserve_and_select(slot, initial.revision) {
                    Ok(()) => EnsureToolOutcome::Ready {
                        item: item.clone(),
                        slot,
                        crafted: false,
                        requirements: root,
                    },
                    Err(e) => map_execution(e, Some(root)),
                };
            }
        }

        let mut best_failure = None;
        for item in candidates {
            match self
                .planner
                .plan(&item, &initial, self.policy.allow_smelting)
            {
                Ok(plan) => return self.execute_plan(item, plan),
                Err(PlanFailure::MissingMaterials {
                    requirements,
                    missing,
                }) => {
                    best_failure = Some(EnsureToolOutcome::MissingMaterials {
                        requirements,
                        missing,
                    })
                }
                Err(PlanFailure::MissingStation {
                    requirements,
                    station,
                }) => {
                    best_failure = Some(EnsureToolOutcome::MissingStation {
                        requirements: Some(requirements),
                        station,
                    })
                }
                Err(PlanFailure::Rejected(reason)) => {
                    best_failure = Some(EnsureToolOutcome::Rejected {
                        requirements: None,
                        reason,
                    })
                }
            }
        }
        best_failure.unwrap()
    }

    fn execute_plan(&mut self, target: String, plan: ProductionPlan) -> EnsureToolOutcome {
        let mut crafted = false;
        for step in &plan.steps {
            let before = match self.execution.inventory() {
                Ok(i) => i,
                Err(e) => return map_execution(e, Some(plan.target.clone())),
            };
            // Another task may have completed this exact step since planning.
            let needed = step.output_count.saturating_mul(step.times);
            if before.count(&step.output) >= needed {
                continue;
            }
            if before.empty_slots == 0 && before.count(&step.output) == 0 {
                return EnsureToolOutcome::NoSpace {
                    requirements: plan.target,
                };
            }
            if let Some((_, _)) = step
                .inputs
                .iter()
                .find(|(id, count)| before.count(id) < **count * step.times)
            {
                let missing = step
                    .inputs
                    .iter()
                    .filter_map(|(id, count)| {
                        let n = count * step.times;
                        (before.count(id) < n).then(|| (id.clone(), n - before.count(id)))
                    })
                    .collect();
                return EnsureToolOutcome::MissingMaterials {
                    requirements: plan.target,
                    missing,
                };
            }
            let result = match &step.kind {
                ProductionKind::Craft { .. } => self.execution.craft(step, before.revision),
                ProductionKind::Smelt { .. } if self.policy.allow_smelting => {
                    self.execution.smelt(step, before.revision)
                }
                ProductionKind::Smelt { .. } => {
                    Err(ExecutionFailure::Rejected("smelting is disabled".into()))
                }
            };
            if let Err(e) = result {
                return map_execution(e, Some(plan.target));
            }
            crafted = true;
        }
        // Confirmation and reservation use one final atomic revision contract.
        let final_view = match self.execution.inventory() {
            Ok(i) => i,
            Err(e) => return map_execution(e, Some(plan.target.clone())),
        };
        let Some(slot) = final_view.usable_tool(&target, self.policy.durability_reserve) else {
            return EnsureToolOutcome::Partial {
                requirements: plan.target,
                produced: final_view.count(&target),
                expected: 1,
            };
        };
        match self.execution.reserve_and_select(slot, final_view.revision) {
            Ok(()) => EnsureToolOutcome::Ready {
                item: target,
                slot,
                crafted,
                requirements: plan.target,
            },
            Err(e) => map_execution(e, Some(plan.target)),
        }
    }
}

fn candidates(request: &ToolRequest, policy: &ToolPolicy) -> Vec<String> {
    let mut seen = BTreeSet::new();
    policy
        .tier_preference
        .iter()
        .copied()
        .filter(|t| *t >= request.minimum_tier)
        .filter(|t| seen.insert(*t))
        .filter_map(|tier| {
            let suffix = match request.category {
                ToolCategory::Pickaxe => "pickaxe",
                ToolCategory::Axe => "axe",
                ToolCategory::Shovel => "shovel",
                ToolCategory::Hoe => "hoe",
                ToolCategory::Shears if tier == MaterialTier::Iron => "shears",
                ToolCategory::Shears => return None,
            };
            Some(if suffix == "shears" {
                "minecraft:shears".into()
            } else {
                format!("minecraft:{}_{suffix}", tier.id())
            })
        })
        .collect()
}

fn map_execution(e: ExecutionFailure, requirements: Option<RequirementNode>) -> EnsureToolOutcome {
    match e {
        ExecutionFailure::MissingStation(station) => EnsureToolOutcome::MissingStation {
            requirements,
            station,
        },
        ExecutionFailure::NoSpace => requirements.map_or(
            EnsureToolOutcome::Rejected {
                requirements: None,
                reason: "no inventory space".into(),
            },
            |requirements| EnsureToolOutcome::NoSpace { requirements },
        ),
        ExecutionFailure::Rejected(reason) => EnsureToolOutcome::Rejected {
            requirements,
            reason,
        },
        ExecutionFailure::Partial { produced, expected } => requirements.map_or(
            EnsureToolOutcome::Rejected {
                requirements: None,
                reason: "partial execution without plan".into(),
            },
            |requirements| EnsureToolOutcome::Partial {
                requirements,
                produced,
                expected,
            },
        ),
        ExecutionFailure::Timeout => EnsureToolOutcome::Timeout { requirements },
        ExecutionFailure::Cancelled => EnsureToolOutcome::Cancelled { requirements },
        ExecutionFailure::Died => EnsureToolOutcome::Died { requirements },
        ExecutionFailure::Disconnected => EnsureToolOutcome::Disconnected { requirements },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Planner {
        plan: Result<ProductionPlan, PlanFailure>,
    }
    impl RecipePlanner for Planner {
        fn plan(&self, _: &str, _: &InventoryView, _: bool) -> Result<ProductionPlan, PlanFailure> {
            self.plan.clone()
        }
    }
    struct Exec {
        views: Vec<InventoryView>,
        reads: usize,
        actions: Vec<String>,
        fail: Option<ExecutionFailure>,
    }
    impl ToolExecution for Exec {
        fn inventory(&mut self) -> Result<InventoryView, ExecutionFailure> {
            let v = self.views[self.reads.min(self.views.len() - 1)].clone();
            self.reads += 1;
            Ok(v)
        }
        fn craft(&mut self, s: &ProductionStep, revision: u64) -> Result<(), ExecutionFailure> {
            self.actions.push(format!("craft:{}@{revision}", s.output));
            self.fail.clone().map_or(Ok(()), Err)
        }
        fn smelt(&mut self, s: &ProductionStep, revision: u64) -> Result<(), ExecutionFailure> {
            self.actions.push(format!("smelt:{}@{revision}", s.output));
            self.fail.clone().map_or(Ok(()), Err)
        }
        fn reserve_and_select(&mut self, slot: u16, revision: u64) -> Result<(), ExecutionFailure> {
            self.actions.push(format!("select:{slot}@{revision}"));
            self.fail.clone().map_or(Ok(()), Err)
        }
    }
    fn view(revision: u64, stacks: Vec<ItemStack>) -> InventoryView {
        InventoryView {
            revision,
            empty_slots: 10,
            stacks,
        }
    }
    fn stack(slot: u16, item: &str, count: u32) -> ItemStack {
        ItemStack {
            slot,
            item: item.into(),
            count,
            durability_left: None,
            protected: false,
        }
    }
    fn policy() -> ToolPolicy {
        ToolPolicy {
            tier_preference: vec![MaterialTier::Iron, MaterialTier::Stone],
            durability_reserve: 10,
            allow_smelting: true,
        }
    }
    fn request() -> ToolRequest {
        ToolRequest {
            block: "minecraft:stone".into(),
            category: ToolCategory::Pickaxe,
            minimum_tier: MaterialTier::Wood,
        }
    }
    fn root() -> RequirementNode {
        RequirementNode {
            item: "minecraft:iron_pickaxe".into(),
            required: 1,
            available: 0,
            operation: None,
            children: vec![],
        }
    }

    #[test]
    fn selects_existing_usable_tool_without_planning_or_crafting() {
        let mut tool = stack(3, "minecraft:iron_pickaxe", 1);
        tool.durability_left = Some(50);
        let mut exec = Exec {
            views: vec![view(7, vec![tool])],
            reads: 0,
            actions: vec![],
            fail: None,
        };
        let planner = Planner {
            plan: Err(PlanFailure::Rejected("must not plan".into())),
        };
        let outcome = EnsureTool {
            planner: &planner,
            execution: &mut exec,
            policy: &policy(),
        }
        .run(&request());
        assert!(matches!(
            outcome,
            EnsureToolOutcome::Ready {
                crafted: false,
                slot: 3,
                ..
            }
        ));
        assert_eq!(exec.actions, ["select:3@7"]);
    }

    #[test]
    fn executes_intermediates_and_smelting_dependency_in_order() {
        let stick = ProductionStep {
            output: "minecraft:stick".into(),
            output_count: 4,
            times: 1,
            inputs: BTreeMap::from([("minecraft:oak_planks".into(), 2)]),
            kind: ProductionKind::Craft { station: None },
        };
        let iron = ProductionStep {
            output: "minecraft:iron_ingot".into(),
            output_count: 1,
            times: 3,
            inputs: BTreeMap::from([("minecraft:raw_iron".into(), 1)]),
            kind: ProductionKind::Smelt {
                station: "minecraft:furnace".into(),
            },
        };
        let pick = ProductionStep {
            output: "minecraft:iron_pickaxe".into(),
            output_count: 1,
            times: 1,
            inputs: BTreeMap::from([
                ("minecraft:stick".into(), 2),
                ("minecraft:iron_ingot".into(), 3),
            ]),
            kind: ProductionKind::Craft {
                station: Some("minecraft:crafting_table".into()),
            },
        };
        let initial = view(
            1,
            vec![
                stack(0, "minecraft:oak_planks", 2),
                stack(1, "minecraft:raw_iron", 3),
            ],
        );
        let after_stick = view(
            2,
            vec![
                stack(2, "minecraft:stick", 4),
                stack(1, "minecraft:raw_iron", 3),
            ],
        );
        let after_iron = view(
            3,
            vec![
                stack(2, "minecraft:stick", 4),
                stack(3, "minecraft:iron_ingot", 3),
            ],
        );
        let done = view(4, vec![stack(5, "minecraft:iron_pickaxe", 1)]);
        let planner = Planner {
            plan: Ok(ProductionPlan {
                target: root(),
                steps: vec![stick, iron, pick],
            }),
        };
        let mut exec = Exec {
            views: vec![initial.clone(), initial, after_stick, after_iron, done],
            reads: 0,
            actions: vec![],
            fail: None,
        };
        let outcome = EnsureTool {
            planner: &planner,
            execution: &mut exec,
            policy: &policy(),
        }
        .run(&request());
        assert!(matches!(
            outcome,
            EnsureToolOutcome::Ready { crafted: true, .. }
        ));
        assert_eq!(
            exec.actions,
            [
                "craft:minecraft:stick@1",
                "smelt:minecraft:iron_ingot@2",
                "craft:minecraft:iron_pickaxe@3",
                "select:5@4"
            ]
        );
    }

    #[test]
    fn revalidation_prevents_duplicate_crafting() {
        let step = ProductionStep {
            output: "minecraft:iron_pickaxe".into(),
            output_count: 1,
            times: 1,
            inputs: BTreeMap::new(),
            kind: ProductionKind::Craft { station: None },
        };
        let raced = view(9, vec![stack(6, "minecraft:iron_pickaxe", 1)]);
        let planner = Planner {
            plan: Ok(ProductionPlan {
                target: root(),
                steps: vec![step],
            }),
        };
        let mut exec = Exec {
            views: vec![view(8, vec![]), raced.clone(), raced],
            reads: 0,
            actions: vec![],
            fail: None,
        };
        let outcome = EnsureTool {
            planner: &planner,
            execution: &mut exec,
            policy: &policy(),
        }
        .run(&request());
        assert!(matches!(
            outcome,
            EnsureToolOutcome::Ready { crafted: false, .. }
        ));
        assert_eq!(exec.actions, ["select:6@9"]);
    }

    #[test]
    fn tier_preference_skips_unplannable_higher_tier() {
        struct TierPlanner;
        impl RecipePlanner for TierPlanner {
            fn plan(
                &self,
                item: &str,
                _: &InventoryView,
                _: bool,
            ) -> Result<ProductionPlan, PlanFailure> {
                if item.contains("iron") {
                    Err(PlanFailure::MissingMaterials {
                        requirements: root(),
                        missing: BTreeMap::from([("minecraft:iron_ingot".into(), 3)]),
                    })
                } else {
                    Ok(ProductionPlan {
                        target: RequirementNode {
                            item: item.into(),
                            required: 1,
                            available: 0,
                            operation: None,
                            children: vec![],
                        },
                        steps: vec![],
                    })
                }
            }
        }
        let stone = view(2, vec![stack(4, "minecraft:stone_pickaxe", 1)]);
        let mut exec = Exec {
            views: vec![view(1, vec![]), stone],
            reads: 0,
            actions: vec![],
            fail: None,
        };
        let outcome = EnsureTool {
            planner: &TierPlanner,
            execution: &mut exec,
            policy: &policy(),
        }
        .run(&request());
        assert!(
            matches!(outcome, EnsureToolOutcome::Ready { item, .. } if item == "minecraft:stone_pickaxe")
        );
    }

    #[test]
    fn propagates_structured_failure() {
        let step = ProductionStep {
            output: "minecraft:iron_pickaxe".into(),
            output_count: 1,
            times: 1,
            inputs: BTreeMap::new(),
            kind: ProductionKind::Craft { station: None },
        };
        let planner = Planner {
            plan: Ok(ProductionPlan {
                target: root(),
                steps: vec![step],
            }),
        };
        let mut exec = Exec {
            views: vec![view(1, vec![]), view(2, vec![])],
            reads: 0,
            actions: vec![],
            fail: Some(ExecutionFailure::Timeout),
        };
        assert!(matches!(
            EnsureTool {
                planner: &planner,
                execution: &mut exec,
                policy: &policy()
            }
            .run(&request()),
            EnsureToolOutcome::Timeout {
                requirements: Some(_)
            }
        ));
    }
}
