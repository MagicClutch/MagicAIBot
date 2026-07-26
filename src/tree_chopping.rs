//! Conservative, loaded-world-only tree recognition and chopping orchestration.
//! Gameplay primitives remain owned by block search, interaction, movement and look.
use std::{collections::{HashMap, HashSet, VecDeque}, time::{Duration, Instant}};

use crate::{blocks::{block_query::BlockSearchQuery, BlockSearchService}, config::TreeChoppingConfig, interaction::{InteractionController, interaction_controller::InteractionState}, logging, look::LookController, minecraft::{client::{ConnectionState, MinecraftClient}, world_state::{BlockPosition, InventorySnapshot}}, movement::MovementService};

const LOG_SUFFIXES: &[&str] = &["oak_log", "birch_log", "spruce_log", "jungle_log", "acacia_log", "dark_oak_log", "mangrove_log", "cherry_log", "pale_oak_log"];
const LEAF_SUFFIXES: &[&str] = &["oak_leaves", "birch_leaves", "spruce_leaves", "jungle_leaves", "acacia_leaves", "dark_oak_leaves", "mangrove_leaves", "cherry_leaves", "pale_oak_leaves", "azalea_leaves", "flowering_azalea_leaves"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructureUncertainty { None, MissingLeaves, HorizontalStructure, TraversalBoundary, MixedWood }
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeModel { pub id: u64, pub logs: Vec<BlockPosition>, pub trunk_base: BlockPosition, pub highest_known_log: BlockPosition, pub tree_type: String, pub estimated_log_count: usize, pub reachable_logs: Vec<BlockPosition>, pub unreachable_logs: Vec<BlockPosition>, pub uncertainty: StructureUncertainty, pub exceeds_limits: bool }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChopOutcome { Running, Completed, Partial, NoTreesFound, OnlyUncertainStructures, NoReachableLogs, NoSuitableTool, InventoryFull, MaximumTreeSizeExceeded, ChangedWorld, TimedOut, Cancelled, Disconnected, Died }
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChopResult { pub outcome: ChopOutcome, pub requested_logs: u32, pub logs_collected: u32, pub trees_inspected: u32, pub trees_chopped: u32, pub logs_broken: u32, pub unreachable_logs: u32, pub uncertain_structures_skipped: u32, pub detail: Option<String> }
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChopRequest { Nearest, TreeType(String), Logs(u32), Count(u32) }
#[derive(Clone, Debug)]
pub struct TreeObservation { pub position: BlockPosition, pub block_id: String }

fn tree_type(id: &str) -> Option<&str> { LOG_SUFFIXES.iter().find_map(|suffix| id.strip_prefix("minecraft:").and_then(|v| (v == *suffix).then(|| suffix.trim_end_matches("_log")))) }
fn adjacent(a: BlockPosition, b: BlockPosition, base_y: i32) -> bool {
    let dx=(a.x-b.x).unsigned_abs(); let dy=(a.y-b.y).unsigned_abs(); let dz=(a.z-b.z).unsigned_abs();
    // Do not fuse neighboring ground-level trunks; horizontal links are treated as branches only above the base.
    dx <= 1 && dy <= 1 && dz <= 1 && dx+dy+dz > 0 && (dx+dz == 0 || a.y.min(b.y) > base_y)
}
pub fn detect_tree(seed: BlockPosition, observations: &[TreeObservation], config: &TreeChoppingConfig) -> Option<TreeModel> {
    let blocks: HashMap<_,_> = observations.iter().map(|b|(b.position,b.block_id.as_str())).collect();
    let seed_id=*blocks.get(&seed)?; let kind=tree_type(seed_id)?; if !config.allowed_tree_types.iter().any(|v|v==kind) { return None; }
    let base_y=observations.iter().filter(|b|tree_type(&b.block_id)==Some(kind) && b.position.x==seed.x && b.position.z==seed.z).map(|b|b.position.y).min().unwrap_or(seed.y);
    let mut queue=VecDeque::from([seed]); let mut seen=HashSet::from([seed]); let mut exceeded=false;
    while let Some(current)=queue.pop_front() {
        if seen.len() >= config.maximum_connected_logs { exceeded=true; break; }
        for candidate in observations.iter().filter(|b| tree_type(&b.block_id).is_some()) {
            if seen.contains(&candidate.position) || !adjacent(current,candidate.position,base_y) { continue; }
            let horizontal=(candidate.position.x-seed.x).unsigned_abs().max((candidate.position.z-seed.z).unsigned_abs());
            let vertical=(candidate.position.y-base_y).unsigned_abs();
            if horizontal>config.maximum_branch_distance || vertical>config.maximum_tree_height { exceeded=true; continue; }
            // Mixed species touching a canopy is evidence of ambiguity, not permission to demolish it.
            if tree_type(&candidate.block_id)==Some(kind) { seen.insert(candidate.position); queue.push_back(candidate.position); }
        }
    }
    let mut logs:Vec<_>=seen.into_iter().collect(); logs.sort_by_key(|p|(p.y,(p.x-seed.x).abs()+(p.z-seed.z).abs(),p.x,p.z));
    let base=*logs.iter().min_by_key(|p|(p.y,(p.x-seed.x).abs()+(p.z-seed.z).abs())).unwrap();
    let high=*logs.iter().max_by_key(|p|(p.y,-((p.x-seed.x).abs()+(p.z-seed.z).abs()),p.x,p.z)).unwrap();
    let leaves=observations.iter().any(|b| LEAF_SUFFIXES.iter().any(|s|b.block_id.ends_with(s)) && logs.iter().any(|p| (p.x-b.position.x).unsigned_abs()<=2 && (p.y-b.position.y).unsigned_abs()<=2 && (p.z-b.position.z).unsigned_abs()<=2));
    let horizontal=logs.iter().filter(|p|p.x!=base.x||p.z!=base.z).count();
    let mixed=observations.iter().any(|b| tree_type(&b.block_id).is_some_and(|t|t!=kind) && logs.iter().any(|p|adjacent(*p,b.position,base_y)));
    let uncertainty=if exceeded {StructureUncertainty::TraversalBoundary} else if mixed {StructureUncertainty::MixedWood} else if horizontal>config.maximum_horizontal_logs {StructureUncertainty::HorizontalStructure} else if config.require_nearby_leaves&&!leaves {StructureUncertainty::MissingLeaves} else {StructureUncertainty::None};
    // Eye-level reach is conservative. Interaction/navigation revalidates actual reach for every block.
    let reachable_logs=logs.iter().copied().filter(|p|p.y<=base.y+5).collect(); let unreachable_logs=logs.iter().copied().filter(|p|p.y>base.y+5).collect();
    Some(TreeModel{id:position_id(base),estimated_log_count:logs.len(),logs,trunk_base:base,highest_known_log:high,tree_type:kind.into(),reachable_logs,unreachable_logs,uncertainty,exceeds_limits:exceeded})
}
fn position_id(p:BlockPosition)->u64 { ((p.x as u64).wrapping_mul(73856093))^((p.y as u64).wrapping_mul(19349663))^((p.z as u64).wrapping_mul(83492791)) }
pub fn chopping_order(tree:&TreeModel)->Vec<BlockPosition>{ let mut v=tree.reachable_logs.clone(); v.sort_by_key(|p|(p.y,(p.x-tree.trunk_base.x).abs()+(p.z-tree.trunk_base.z).abs(),p.x,p.z)); v }
fn log_count(inv:&InventorySnapshot)->u32 { inv.total_counts.iter().filter(|(id,_)| LOG_SUFFIXES.iter().any(|s|id.ends_with(s))).map(|(_,n)|*n).sum() }
fn has_axe(inv:&InventorySnapshot)->bool { inv.slots.iter().any(|s|s.item_id.as_deref().is_some_and(|id|id.ends_with("_axe"))) }

#[derive(Clone, Debug)] struct Active { request:ChopRequest, result:ChopResult, queue:VecDeque<(BlockPosition,String)>, initial_logs:u32, started:Instant, waiting:bool }
#[derive(Clone, Debug)] pub struct TreeChopService { config:TreeChoppingConfig, active:Option<Active>, last:Option<ChopResult> }
impl TreeChopService {
 pub fn new(config:TreeChoppingConfig)->Self{Self{config,active:None,last:None}}
 pub fn status(&self)->Option<&ChopResult>{self.active.as_ref().map(|a|&a.result).or(self.last.as_ref())}
 pub async fn start(&mut self, request:ChopRequest, minecraft:&MinecraftClient, search:&BlockSearchService)->ChopResult {
  self.active=None;
  // Leaves are excluded from the break queue; saplings remain passive drop collection.
  let _passive_drop_policy=(self.config.break_leaves,self.config.collect_saplings);
  let world=minecraft.world_state_snapshot().await; let requested=match request{ChopRequest::Logs(n)=>n,_=>u32::MAX};
  let mut base=ChopResult{outcome:ChopOutcome::Running,requested_logs:requested,logs_collected:0,trees_inspected:0,trees_chopped:0,logs_broken:0,unreachable_logs:0,uncertain_structures_skipped:0,detail:None};
  if world.bot.alive==Some(false){base.outcome=ChopOutcome::Died;return self.finish(base)}
  if !world.inventory.available {base.outcome=ChopOutcome::InventoryFull;base.detail=Some("inventory state unavailable or has no confirmed capacity".into());return self.finish(base)}
  if !has_axe(&world.inventory)&&!self.config.allow_hand_chopping {base.outcome=ChopOutcome::NoSuitableTool;return self.finish(base)}
  let mut obs=Vec::new();
  for suffix in LOG_SUFFIXES.iter().chain(LEAF_SUFFIXES) { if let Ok(found)=search.search_raw(minecraft,BlockSearchQuery{block_id:format!("minecraft:{suffix}"),radius:self.config.search_radius,maximum_results:self.config.maximum_connected_logs.min(256),vertical_range:self.config.maximum_tree_height}).await { obs.extend(found.into_iter().map(|b|TreeObservation{position:b.position,block_id:b.block_id})); } }
  let mut seeds:Vec<_>=obs.iter().filter(|b|tree_type(&b.block_id).is_some()).map(|b|b.position).collect(); seeds.sort_by_key(|p|(p.y,p.x,p.z)); let mut claimed=HashSet::new(); let mut trees=Vec::new();
  for seed in seeds {if claimed.contains(&seed){continue} if let Some(t)=detect_tree(seed,&obs,&self.config){base.trees_inspected+=1; claimed.extend(t.logs.iter().copied()); if t.exceeds_limits {base.outcome=ChopOutcome::MaximumTreeSizeExceeded;base.detail=Some("connected log traversal reached a configured bound".into());return self.finish(base)} if t.uncertainty!=StructureUncertainty::None {base.uncertain_structures_skipped+=1;continue} if matches!(&request,ChopRequest::TreeType(k) if k!=&t.tree_type){continue} trees.push(t)} }
  if trees.is_empty(){base.outcome=if base.uncertain_structures_skipped>0{ChopOutcome::OnlyUncertainStructures}else{ChopOutcome::NoTreesFound};return self.finish(base)}
  trees.sort_by_key(|t|t.id); let tree_limit=match request{ChopRequest::Count(n)=>n,_=>1}.min(self.config.maximum_trees); let mut queue=VecDeque::new();
  for tree in trees.into_iter().take(tree_limit as usize){base.unreachable_logs+=tree.unreachable_logs.len() as u32; for p in chopping_order(&tree){queue.push_back((p,format!("minecraft:{}_log",tree.tree_type)));} base.trees_chopped+=1;}
  if queue.is_empty(){base.outcome=ChopOutcome::NoReachableLogs;return self.finish(base)}
  logging::info("Tree chopping started"); self.active=Some(Active{request,result:base.clone(),queue,initial_logs:log_count(&world.inventory),started:Instant::now(),waiting:false}); base
 }
 fn finish(&mut self,r:ChopResult)->ChopResult{self.last=Some(r.clone());r}
 pub async fn stop(&mut self,m:&MinecraftClient,movement:&MovementService,look:&LookController,interaction:&InteractionController)->ChopResult {interaction.cancel(m,movement,look).await; let mut r=self.active.take().map(|a|a.result).unwrap_or(ChopResult{outcome:ChopOutcome::Cancelled,requested_logs:0,logs_collected:0,trees_inspected:0,trees_chopped:0,logs_broken:0,unreachable_logs:0,uncertain_structures_skipped:0,detail:None});r.outcome=ChopOutcome::Cancelled;self.finish(r)}
 pub async fn tick(&mut self,m:&MinecraftClient,movement:&MovementService,look:&LookController,interaction:&InteractionController){
  let Some(mut a)=self.active.take() else{return}; let world=m.world_state_snapshot().await;
  let end=if m.connection_state()!=ConnectionState::Connected{Some(ChopOutcome::Disconnected)}else if world.bot.alive==Some(false){Some(ChopOutcome::Died)}else if a.started.elapsed()>Duration::from_secs(self.config.total_timeout_seconds){Some(ChopOutcome::TimedOut)}else{None};
  if let Some(outcome)=end{interaction.cancel(m,movement,look).await;a.result.outcome=outcome;self.finish(a.result);return}
  a.result.logs_collected=log_count(&world.inventory).saturating_sub(a.initial_logs); let goal=match a.request{ChopRequest::Logs(n)=>Some(n),_=>None}; if goal.is_some_and(|n|a.result.logs_collected>=n){a.result.outcome=ChopOutcome::Completed;self.finish(a.result);return}
  if a.waiting {match interaction.snapshot().await.state{InteractionState::Completed=>{a.result.logs_broken+=1;a.waiting=false},InteractionState::Failed=>{a.result.outcome=ChopOutcome::Partial;a.result.detail=Some("a log was unreachable or changed before server confirmation".into());self.finish(a.result);return},_=>{self.active=Some(a);return}}}
  if let Some((p,expected))=a.queue.pop_front(){match m.block_id_at(p).await{Ok(Some(id)) if id==expected=>match interaction.break_at(m,movement,look,p).await{Ok(())=>a.waiting=true,Err(e)=>{a.result.outcome=ChopOutcome::Partial;a.result.detail=Some(e.to_string());self.finish(a.result);return}},Ok(_)=>{a.result.outcome=ChopOutcome::ChangedWorld;a.result.detail=Some("planned log changed or its chunk unloaded".into());self.finish(a.result);return},Err(e)=>{a.result.outcome=ChopOutcome::Partial;a.result.detail=Some(e.to_string());self.finish(a.result);return}}}else{a.result.outcome=if a.result.unreachable_logs>0{ChopOutcome::Partial}else{ChopOutcome::Completed};self.finish(a.result);return} self.active=Some(a)
 }
}

#[cfg(test)] mod tests {
 use super::*;
 fn p(x:i32,y:i32,z:i32)->BlockPosition{BlockPosition{x,y,z}} fn b(x:i32,y:i32,z:i32,id:&str)->TreeObservation{TreeObservation{position:p(x,y,z),block_id:format!("minecraft:{id}")}}
 fn cfg()->TreeChoppingConfig{TreeChoppingConfig::default()}
 #[test] fn simple_and_branching_topology(){let o=vec![b(0,0,0,"oak_log"),b(0,1,0,"oak_log"),b(0,2,0,"oak_log"),b(1,3,0,"oak_log"),b(1,3,1,"oak_leaves")];let t=detect_tree(p(0,0,0),&o,&cfg()).unwrap();assert_eq!(t.logs.len(),4);assert_eq!(t.uncertainty,StructureUncertainty::None);assert_eq!(chopping_order(&t)[0],p(0,0,0));}
 #[test] fn separates_adjacent_trunks(){let o=vec![b(0,0,0,"oak_log"),b(0,1,0,"oak_log"),b(1,0,0,"oak_log"),b(1,1,0,"oak_log"),b(0,2,0,"oak_leaves"),b(1,2,0,"oak_leaves")];assert_eq!(detect_tree(p(0,0,0),&o,&cfg()).unwrap().logs.len(),2);}
 #[test] fn rejects_missing_leaves_and_oversized_artificial_structure(){let o=vec![b(0,0,0,"oak_log"),b(0,1,0,"oak_log")];assert_eq!(detect_tree(p(0,0,0),&o,&cfg()).unwrap().uncertainty,StructureUncertainty::MissingLeaves);let mut c=cfg();c.maximum_connected_logs=2;let mut big=o;big.push(b(0,2,0,"oak_log"));big.push(b(0,3,0,"oak_leaves"));assert!(detect_tree(p(0,0,0),&big,&c).unwrap().exceeds_limits);}
 #[test] fn tall_logs_are_reported_unreachable(){let mut o=(0..8).map(|y|b(0,y,0,"spruce_log")).collect::<Vec<_>>();o.push(b(0,8,0,"spruce_leaves"));let t=detect_tree(p(0,0,0),&o,&cfg()).unwrap();assert_eq!(t.unreachable_logs.len(),2);}
}
