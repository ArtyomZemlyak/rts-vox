use super::*;
use phys::V3;
use super::battle::Order;
use std::collections::BTreeSet;
use nav::vehicle::support::{Surface,Validity};

fn arena()->BlastRange {
 let mut l=RangeLayout::from_ron(include_str!("../../../data/range-motion-yard.ron")).unwrap();
 l.size=[2048,512,2048];l.clear_max=[2048,2048];l.blocks.clear();
 let c=l.course.as_mut().unwrap();c.max=[2008,2008];c.patrols.clear();c.ramps.clear();
 l.units=[(RangeUnitKind::Spider,[512,56,512]),(RangeUnitKind::Walker04,[1024,56,512]),
  (RangeUnitKind::Commander,[512,56,1024]),(RangeUnitKind::Strider,[1024,56,1024])].into_iter()
  .map(|(kind,at)|modding::range::RangeUnitDef{kind,at,allied:true,weapon:None}).collect();
 let config=ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
 let mut r=BlastRange::with_layout(config,l).unwrap();r.set_navigation_workers(1);r.set_physics_workers(1);r.start_free_control().unwrap();
 for [x,z] in [[512,512],[1024,512],[512,1024],[1024,1024]] {
  for dx in [-80,-40,0,40,80] {for dz in [-80,-40,0,40,80] {super::tests::drop_heap(&mut r,[x+dx,16,z+dz],1000,64);}}
 }
 for _ in 0..8 {r.step();}
 for u in &r.units {assert!(u.motion.feet.iter().all(|f|f.ready() && f.support.source.is_some_and(|s|s.kind==1)),"{:?} must actually start on stable fragments",u.kind);}
 r
}
fn conditions(r:&mut BlastRange) {
 // Actual voxel cut, connectivity and matter accounting, not a healthy mask.
 let shin=r.units[0].body.legs[0].shin as usize;
 let b=&r.units[0].body.segments[shin].body;let (cut,ny,nz)=(b.vol.nx as usize/2,b.vol.ny as usize,b.vol.nz as usize);
 let mut removed=0;
 for z in 0..nz {for y in 0..ny {if r.units[0].body.segments[shin].body.remove(cut,y,z) {
  r.units[0].seg_removed.push((shin as u8,[cut as u8,y as u8,z as u8]));removed+=1;
 }}}
 assert!(removed>0);r.matter.annihilate_body(0,removed);r.units[0].refresh_damage(0);assert!(r.units[0].motion.legs_lost[0]);assert!(r.units[0].status.alive);
 // Installed geometry determines Disabled; no artificial engine-response flag.
 let mut cells=Vec::new();let u=&r.units[2];
 for z in 0..u.body.vol.nz as usize {for y in 0..u.body.vol.ny as usize {for x in 0..u.body.vol.nx as usize {
  if u.body.vol.get(x,y,z) && u.body.part_at(x,y,z)==voxel::Part::Engine {cells.push([x,y,z]);}
 }}}
 assert!(!cells.is_empty());
 for c in &cells {assert!(r.units[2].body.remove(c[0],c[1],c[2]));r.units[2].removed.push(c.map(|v|v as u8));}
 r.matter.annihilate_body(2,cells.len());r.units[2].refresh_damage(2);assert!(r.units[2].status.alive && !r.units[2].status.engine);assert_eq!(r.units[2].mobility.engine,Fx::ZERO);
 let u=&mut r.units[1];let burst=[u.motion.pos[0].sub(Fx::from_int(20)),u.motion.hull.y,u.motion.pos[2]].map(Fx::to_int_trunc);
 u.motion.impulse(&u.body,burst,100_000);assert_ne!(u.motion.shove,V3::ZERO);
 for id in 0..4 {assert!(r.battle_order(id,Order::Pilot{throttle:1,turn:if id%2==0{1}else{-1},aim:[1400,80,1400],fire:false},None));}
}
fn event(r:&mut BlastRange,event:usize,ids:&BTreeSet<u64>) {
 if event==0 {for f in r.matter.debris.fragments.iter_mut().filter(|f|ids.contains(&f.id)){f.resting=false;f.velocity.x=Fx::ONE;}return;}
 let held:BTreeSet<_>=r.matter.debris.fragments.iter().filter(|f|ids.contains(&f.id)).map(|f|f.held).collect();
 for &id in &held {let n=r.matter.economy.ledger.held(id);let ground=r.matter.economy.ground;assert_eq!(r.matter.economy.ledger.transfer(id,ground,n),n);}
 if event==1 {r.matter.debris.sweep(&r.matter.economy);}
 else {r.matter.debris.retire_reclaimed(&mut r.matter.economy,&held);}
 assert!(!r.matter.debris.fragments.iter().any(|f|ids.contains(&f.id)));
}
#[test]
fn transition_damaged_impulsed_disabled_cold_restore_source_events_workers_1_2_4_8() {
 let mut original=arena();let ids:BTreeSet<_>=original.matter.debris.fragments.iter().map(|f|f.id).collect();conditions(&mut original);
 let snapshot=original.snapshot();
 for source_event in 0..3 {
  let workers:Vec<usize>=std::env::var("RTS_TRANSITION_WORKERS").ok().map(|v|vec![v.parse().unwrap()]).unwrap_or_else(||vec![1,2,4,8]);
  assert!(workers.iter().all(|w|[1,2,4,8].contains(w)));
  let mut branches:Vec<_>=workers.iter().copied().map(|workers| {
   let mut r=BlastRange::restore(original.config,&snapshot).unwrap();r.set_physics_workers(workers);r.set_navigation_workers(workers);
   assert_eq!(r.state_hash(),original.state_hash(),"cold initial workers{workers}");r
  }).collect();
  // Preserve a genuinely warm continuing branch. All worker branches are cold.
  let mut warm=arena();conditions(&mut warm);assert_eq!(warm.snapshot(),snapshot);
  let mut hashes=String::from("tick,hash\n");
  let mut moving=[false;4];let mut moved=[false;4];let mut proofs=[0usize;4];let start:Vec<_>=branches[0].units.iter().map(|u|u.motion.pos).collect();
  for tick in 0..64 {
   if tick==if source_event==0 {0}else{4} {event(&mut warm,source_event,&ids);for r in &mut branches {event(r,source_event,&ids);}}
   warm.step();
   for r in &mut branches {
    let previous:Vec<_>=r.units.iter().map(|u|u.motion.feet.clone()).collect();
    for u in &mut r.units {u.motion.trace_anchors(true);u.motion.clear_anchor_trace();}
    r.step();assert!(r.matter.economy.ledger.balanced());
    assert!(!r.units[0].motion.feet[0].ready(),"severed limb retained Ready");
    assert!(!r.units[2].status.engine);assert_eq!(r.units[2].motion.drive.vel,Fx::ZERO,"Disabled powered itself");
    let live:BTreeSet<_>=r.matter.debris.fragments.iter().filter(|f|!f.resting || f.velocity!=V3::ZERO).map(|f|f.id).collect();
    let surface=std::cell::RefCell::new(nav::vehicle::Probe::new(&r.world).with_rubble(&r.rubble));
    for (id,u) in r.units.iter().enumerate() {
     let trace=u.motion.anchor_trace().unwrap();assert!(trace.events().len()<=crate::gait::AnchorTrace::LIMIT);
     if let Some(proof)=&trace.transition {
      assert_eq!(proof.level,nav::vehicle::transition::Level::Motor);assert!(proof.sources_checked);
      assert!(!proof.sources.is_empty() && proof.sources.len()<=24);
      assert!(!proof.soles.is_empty());
      for (sample,sole) in proof.sources.iter().zip(&proof.soles) {
       assert!(!u.motion.legs_lost[sole.leg] && !u.motion.legs_buckled[sole.leg]);
       assert!(!sole.ready || sole.planted && sole.validity==Validity::Supported,"proof fabricated Ready");
       if sole.ready {
        let gap=Fx::from_ratio(if u.kind==RangeUnitKind::Strider {75}else{50},i32::from(voxel::unit::WORLD_MM));
        assert!(sole.probe_at[1].sub(sample.height).abs()<=gap,"proof used ankle height instead of the actual surface");
        assert_eq!(sample.validity,Validity::Supported);assert_eq!(sample.source,sole.source);
       }
      }
      proofs[id]+=1;
     }
     let root=u.motion.frame(&u.body);
     moved[id]|=u.motion.pos[0]!=start[id][0] || u.motion.pos[2]!=start[id][2];
     for (leg,foot) in u.motion.feet.iter().enumerate() {
      if foot.ready() {
       let c=foot.contact.unwrap();let rig=&u.motion.rigs()[leg];let d=c.pos.sub(root.base);
       let local=V3::new(d.dot(root.x),d.dot(root.y),d.dot(root.z));
       let error=rig.solve_contact(local).ankle.sub(local).len_sq().sqrt();
       assert!(error<=Fx::from_ratio(1,20),"Ready IK event{source_event}/unit{id}/leg{leg}/tick{tick}: {error:?}");
       assert_eq!(surface.validity(foot.support.source.unwrap()),Validity::Supported,"stale Ready source");
       let old=&previous[id][leg];
       if old.ready() {let before=old.contact.unwrap();assert_eq!([c.pos.x,c.pos.z],[before.pos.x,before.pos.z],"Ready anchor slid under damage/impulse/Disabled");}
      }
      assert!(!(foot.ready() && foot.support.source.is_some_and(|s|s.kind==1 && live.contains(&s.id))),"moving debris received Ready event{source_event}/unit{id}/tick{tick}");
      moving[id]|=foot.support.reason==crate::gait::SupportReason::Moving;
     }
    }
   }
   let hash=warm.state_hash();for r in &branches {assert_eq!(hash,r.state_hash(),"warm/cold event{source_event} tick{tick}");}
   hashes.push_str(&format!("{tick},{hash}\n"));
   if tick==6 || tick==24 {
    for (i,r) in branches.iter_mut().enumerate() {let state=r.snapshot();let h=r.state_hash();*r=BlastRange::restore(r.config,&state).unwrap();r.set_physics_workers(workers[i]);r.set_navigation_workers(workers[i]);assert_eq!(r.state_hash(),h,"cold restore during recovery");}
   }
  }
  if let Some(dir)=std::env::var_os("RTS_TRANSITION_CAPTURE_DIR") {
   let dir=std::path::PathBuf::from(dir);std::fs::create_dir_all(&dir).unwrap();
   let tag=std::env::var("RTS_TRANSITION_WORKERS").unwrap_or_else(|_|"all".into());
   std::fs::write(dir.join(format!("workers{tag}-event{source_event}.csv")),hashes).unwrap();
  }
  assert!(proofs[0]>0 && proofs[1]>0 && proofs[2]>0,"missing actual motor proof: {proofs:?}");
  assert!(moved[1],"actual impulse had no physical displacement");
  if source_event==0 {assert!(moving[0] && moving[1] && moving[2],"wake did not reach damaged/impulsed/Disabled: {moving:?}");}
  else {for (id,u) in branches[0].units.iter().enumerate() {assert!(u.motion.feet.iter().any(|f|f.ready() && f.support.source.is_some_and(|s|s.kind==0)),"event{source_event}/unit{id} never admitted a terrain landing: pos={:?} hull={:?} feet={:?}",u.motion.pos,u.motion.hull,u.motion.feet.iter().map(|f|(f.ready(),f.support.reason,f.support.source,f.contact.map(|c|c.pos),f.placement().map(|c|c.pos))).collect::<Vec<_>>());}}
 }
}
