//! Deterministic, deny-by-default inventory cleanup planning and execution.
//!
//! The policy engine never estimates item value or searches for storage. Callers
//! must provide metadata, reservations, protected slots, and a configured chest
//! destination. Mutation is available only through the narrow service traits.

use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CleanupAction {
    #[default]
    Keep,
    Discard,
    Store,
    Reserve,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct CleanupRule {
    #[serde(default)]
    pub item: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub action: CleanupAction,
    #[serde(default)]
    pub minimum_retained: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CleanupPolicy {
    #[serde(default = "yes")]
    pub protect_rare_items: bool,
    #[serde(default = "yes")]
    pub protect_tools: bool,
    #[serde(default)]
    pub storage: Option<String>,
    #[serde(default)]
    pub rules: Vec<CleanupRule>,
}
fn yes() -> bool {
    true
}
impl Default for CleanupPolicy {
    fn default() -> Self {
        Self {
            protect_rare_items: true,
            protect_tools: true,
            storage: None,
            rules: vec![],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupItem {
    pub slot: usize,
    pub item_id: String,
    pub count: u32,
    /// Registry-supplied tags only. Cleanup does not derive or discover tags.
    pub tags: BTreeSet<String>,
    pub rare: bool,
    pub tool: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupSnapshot {
    pub revision: u64,
    pub items: Vec<CleanupItem>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CleanupSafety {
    pub protected_slots: BTreeSet<usize>,
    /// Item quantities currently owned by active tasks.
    pub reservations: BTreeMap<String, u32>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupStep {
    pub slot: usize,
    pub item_id: String,
    pub amount: u32,
    pub action: CleanupAction,
    pub reason: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupPlan {
    pub revision: u64,
    pub storage: Option<String>,
    pub steps: Vec<CleanupStep>,
}

/// Evaluate policy without side effects. Output is stable by slot and item id.
pub fn plan_cleanup(
    policy: &CleanupPolicy,
    snapshot: &CleanupSnapshot,
    safety: &CleanupSafety,
) -> CleanupPlan {
    let mut items = snapshot.items.clone();
    items.sort_by(|a, b| (a.slot, &a.item_id).cmp(&(b.slot, &b.item_id)));
    let mut retained = BTreeMap::<String, u32>::new();
    let mut reserved_left = safety.reservations.clone();
    let mut steps = Vec::with_capacity(items.len());
    for item in items {
        let (mut action, minimum, mut reason) = selected_rule(policy, &item).map_or(
            (
                CleanupAction::Keep,
                0,
                "no matching rule; default keep".into(),
            ),
            |(r, why)| (r.action, r.minimum_retained, why),
        );
        if safety.protected_slots.contains(&item.slot) {
            action = CleanupAction::Keep;
            reason = "equipped or protected slot".into();
        } else if policy.protect_rare_items && item.rare {
            action = CleanupAction::Keep;
            reason = "rare-item protection".into();
        } else if policy.protect_tools && item.tool {
            action = CleanupAction::Keep;
            reason = "tool protection".into();
        }
        let reservation = reserved_left.entry(item.item_id.clone()).or_default();
        let reserved_here = (*reservation).min(item.count);
        *reservation -= reserved_here;
        if reserved_here == item.count && reserved_here > 0 {
            action = CleanupAction::Reserve;
            reason = "active task reservation".into();
        }
        let already = *retained.get(&item.item_id).unwrap_or(&0);
        let required = minimum.max(*safety.reservations.get(&item.item_id).unwrap_or(&0));
        let keep_for_minimum = required.saturating_sub(already).min(item.count);
        let protected_amount = reserved_here.max(keep_for_minimum);
        let amount = match action {
            CleanupAction::Discard | CleanupAction::Store => {
                item.count.saturating_sub(protected_amount)
            }
            CleanupAction::Keep | CleanupAction::Reserve => 0,
        };
        *retained.entry(item.item_id.clone()).or_default() += item.count - amount;
        if amount == 0 && matches!(action, CleanupAction::Discard | CleanupAction::Store) {
            action = CleanupAction::Keep;
            reason = if reserved_here > 0 {
                "active task reservation"
            } else {
                "minimum retained count"
            }
            .into();
        }
        steps.push(CleanupStep {
            slot: item.slot,
            item_id: item.item_id,
            amount,
            action,
            reason,
        });
    }
    CleanupPlan {
        revision: snapshot.revision,
        storage: policy.storage.clone(),
        steps,
    }
}

fn selected_rule<'a>(
    policy: &'a CleanupPolicy,
    item: &CleanupItem,
) -> Option<(&'a CleanupRule, String)> {
    policy
        .rules
        .iter()
        .enumerate()
        .filter_map(|(n, r)| {
            let specificity = if r.item.as_deref() == Some(&item.item_id) {
                2
            } else if r.item.is_none() && r.tag.as_ref().is_some_and(|t| item.tags.contains(t)) {
                1
            } else {
                return None;
            };
            Some((specificity, std::cmp::Reverse(n), r))
        })
        .max_by_key(|(specificity, n, _)| (*specificity, *n))
        .map(|(_, _, r)| {
            (
                r,
                if r.item.is_some() {
                    "exact item rule"
                } else {
                    "item tag rule"
                }
                .into(),
            )
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupOutcome {
    Completed,
    Partial,
    NoStorage,
    Full,
    Stale,
    Rejected,
    TimedOut,
    Cancelled,
    Died,
    Disconnected,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StepConfirmation {
    pub slot: usize,
    pub action: CleanupAction,
    pub requested: u32,
    pub confirmed: u32,
    pub detail: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupResult {
    pub outcome: CleanupOutcome,
    pub planned_revision: u64,
    pub confirmations: Vec<StepConfirmation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)] // Runtime adapters map these terminal conditions as they are integrated.
pub enum MutationError {
    Full,
    TimedOut,
    Died,
    Disconnected,
    Rejected,
}
pub trait ChestService {
    fn store(&mut self, destination: &str, slot: usize, amount: u32) -> Result<u32, MutationError>;
}
pub trait InventoryActions {
    fn discard(&mut self, slot: usize, amount: u32) -> Result<u32, MutationError>;
}

#[derive(Clone, Default)]
pub struct CleanupCancellation(Arc<AtomicBool>);
impl CleanupCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Execute a previously visible plan. The caller must supply the current
/// revision immediately before mutation; every mutation produces a confirmation.
pub fn execute_cleanup(
    plan: &CleanupPlan,
    current_revision: u64,
    chest: &mut impl ChestService,
    inventory: &mut impl InventoryActions,
    cancel: &CleanupCancellation,
) -> CleanupResult {
    if current_revision != plan.revision {
        return result(plan, CleanupOutcome::Stale, vec![]);
    }
    let mut confirmations = vec![];
    for step in &plan.steps {
        if cancel.is_cancelled() {
            return result(plan, CleanupOutcome::Cancelled, confirmations);
        }
        if step.amount == 0 || matches!(step.action, CleanupAction::Keep | CleanupAction::Reserve) {
            continue;
        }
        let mutation = match step.action {
            CleanupAction::Store => match &plan.storage {
                Some(destination) => chest.store(destination, step.slot, step.amount),
                None => return result(plan, CleanupOutcome::NoStorage, confirmations),
            },
            CleanupAction::Discard => inventory.discard(step.slot, step.amount),
            _ => unreachable!(),
        };
        match mutation {
            Ok(confirmed) => {
                confirmations.push(StepConfirmation {
                    slot: step.slot,
                    action: step.action,
                    requested: step.amount,
                    confirmed,
                    detail: "server-confirmed".into(),
                });
                if confirmed != step.amount {
                    return result(plan, CleanupOutcome::Partial, confirmations);
                }
            }
            Err(error) => {
                return result(
                    plan,
                    match error {
                        MutationError::Full => CleanupOutcome::Full,
                        MutationError::TimedOut => CleanupOutcome::TimedOut,
                        MutationError::Died => CleanupOutcome::Died,
                        MutationError::Disconnected => CleanupOutcome::Disconnected,
                        MutationError::Rejected => CleanupOutcome::Rejected,
                    },
                    confirmations,
                );
            }
        }
    }
    result(plan, CleanupOutcome::Completed, confirmations)
}
fn result(
    plan: &CleanupPlan,
    outcome: CleanupOutcome,
    confirmations: Vec<StepConfirmation>,
) -> CleanupResult {
    CleanupResult {
        outcome,
        planned_revision: plan.revision,
        confirmations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(slot: usize, id: &str, count: u32) -> CleanupItem {
        CleanupItem {
            slot,
            item_id: id.into(),
            count,
            tags: BTreeSet::new(),
            rare: false,
            tool: false,
        }
    }
    fn policy(rules: Vec<CleanupRule>) -> CleanupPolicy {
        CleanupPolicy {
            rules,
            ..Default::default()
        }
    }
    fn rule(item: Option<&str>, tag: Option<&str>, action: CleanupAction, min: u32) -> CleanupRule {
        CleanupRule {
            item: item.map(Into::into),
            tag: tag.map(Into::into),
            action,
            minimum_retained: min,
        }
    }
    #[test]
    fn default_keeps_everything() {
        let p = plan_cleanup(
            &CleanupPolicy::default(),
            &CleanupSnapshot {
                revision: 1,
                items: vec![item(2, "dirt", 64)],
            },
            &Default::default(),
        );
        assert_eq!(p.steps[0].action, CleanupAction::Keep);
        assert_eq!(p.steps[0].amount, 0);
    }
    #[test]
    fn exact_precedes_tag_and_first_tie_wins() {
        let mut i = item(1, "diamond", 8);
        i.tags.insert("loot".into());
        let p = plan_cleanup(
            &policy(vec![
                rule(None, Some("loot"), CleanupAction::Discard, 0),
                rule(Some("diamond"), None, CleanupAction::Store, 2),
                rule(Some("diamond"), None, CleanupAction::Discard, 0),
            ]),
            &CleanupSnapshot {
                revision: 2,
                items: vec![i],
            },
            &Default::default(),
        );
        assert_eq!(
            (p.steps[0].action, p.steps[0].amount),
            (CleanupAction::Store, 6)
        );
    }
    #[test]
    fn protections_and_reservations_override_destructive_rules() {
        let mut rare = item(0, "rare", 1);
        rare.rare = true;
        let mut tool = item(1, "pick", 1);
        tool.tool = true;
        let safety = CleanupSafety {
            protected_slots: [2].into(),
            reservations: BTreeMap::from([("food".into(), 3)]),
        };
        let p = plan_cleanup(
            &policy(vec![
                rule(None, Some("all"), CleanupAction::Discard, 0),
                rule(Some("rare"), None, CleanupAction::Discard, 0),
                rule(Some("pick"), None, CleanupAction::Discard, 0),
                rule(Some("armor"), None, CleanupAction::Discard, 0),
                rule(Some("food"), None, CleanupAction::Discard, 0),
            ]),
            &CleanupSnapshot {
                revision: 1,
                items: vec![rare, tool, item(2, "armor", 1), item(3, "food", 3)],
            },
            &safety,
        );
        assert!(p.steps.iter().all(|s| s.amount == 0));
    }
    #[test]
    fn minimum_is_retained_deterministically() {
        let p = plan_cleanup(
            &policy(vec![rule(Some("dirt"), None, CleanupAction::Discard, 10)]),
            &CleanupSnapshot {
                revision: 1,
                items: vec![item(5, "dirt", 8), item(1, "dirt", 8)],
            },
            &Default::default(),
        );
        assert_eq!(
            p.steps.iter().map(|s| s.amount).collect::<Vec<_>>(),
            vec![0, 6]
        );
    }
    struct Mock {
        calls: Vec<(usize, u32)>,
        error: Option<MutationError>,
    }
    impl ChestService for Mock {
        fn store(&mut self, _: &str, s: usize, a: u32) -> Result<u32, MutationError> {
            self.calls.push((s, a));
            self.error.map_or(Ok(a), Err)
        }
    }
    impl InventoryActions for Mock {
        fn discard(&mut self, s: usize, a: u32) -> Result<u32, MutationError> {
            self.calls.push((s, a));
            self.error.map_or(Ok(a), Err)
        }
    }
    #[test]
    fn execute_uses_only_authorized_boundary_and_confirms() {
        let p = CleanupPlan {
            revision: 7,
            storage: Some("base:chest-a".into()),
            steps: vec![
                CleanupStep {
                    slot: 1,
                    item_id: "dirt".into(),
                    amount: 3,
                    action: CleanupAction::Store,
                    reason: "rule".into(),
                },
                CleanupStep {
                    slot: 2,
                    item_id: "gravel".into(),
                    amount: 2,
                    action: CleanupAction::Discard,
                    reason: "rule".into(),
                },
            ],
        };
        let mut c = Mock {
            calls: vec![],
            error: None,
        };
        let mut i = Mock {
            calls: vec![],
            error: None,
        };
        let r = execute_cleanup(&p, 7, &mut c, &mut i, &Default::default());
        assert_eq!(r.outcome, CleanupOutcome::Completed);
        assert_eq!(c.calls, vec![(1, 3)]);
        assert_eq!(i.calls, vec![(2, 2)]);
        assert_eq!(r.confirmations.len(), 2);
    }
    #[test]
    fn stale_plan_never_mutates() {
        let p = CleanupPlan {
            revision: 1,
            storage: None,
            steps: vec![],
        };
        let mut c = Mock {
            calls: vec![],
            error: None,
        };
        let mut i = Mock {
            calls: vec![],
            error: None,
        };
        assert_eq!(
            execute_cleanup(&p, 2, &mut c, &mut i, &Default::default()).outcome,
            CleanupOutcome::Stale
        );
        assert!(c.calls.is_empty() && i.calls.is_empty());
    }
    #[test]
    fn cancellation_stops_between_confirmed_steps() {
        let p = CleanupPlan {
            revision: 1,
            storage: None,
            steps: vec![CleanupStep {
                slot: 1,
                item_id: "x".into(),
                amount: 1,
                action: CleanupAction::Discard,
                reason: "rule".into(),
            }],
        };
        let token = CleanupCancellation::default();
        token.cancel();
        let mut c = Mock {
            calls: vec![],
            error: None,
        };
        let mut i = Mock {
            calls: vec![],
            error: None,
        };
        assert_eq!(
            execute_cleanup(&p, 1, &mut c, &mut i, &token).outcome,
            CleanupOutcome::Cancelled
        );
        assert!(i.calls.is_empty());
    }
    #[test]
    fn missing_storage_and_full_are_structured() {
        let step = CleanupStep {
            slot: 1,
            item_id: "x".into(),
            amount: 1,
            action: CleanupAction::Store,
            reason: "rule".into(),
        };
        let mut c = Mock {
            calls: vec![],
            error: Some(MutationError::Full),
        };
        let mut i = Mock {
            calls: vec![],
            error: None,
        };
        let mut p = CleanupPlan {
            revision: 1,
            storage: None,
            steps: vec![step],
        };
        assert_eq!(
            execute_cleanup(&p, 1, &mut c, &mut i, &Default::default()).outcome,
            CleanupOutcome::NoStorage
        );
        p.storage = Some("known".into());
        assert_eq!(
            execute_cleanup(&p, 1, &mut c, &mut i, &Default::default()).outcome,
            CleanupOutcome::Full
        );
    }
}
