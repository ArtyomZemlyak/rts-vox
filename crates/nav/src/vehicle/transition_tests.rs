use super::*;
use crate::vehicle::{coarse, support::{Piece, Sources, SupportRef}, Rubble};
use voxel::{World,Generator};
struct Flat;
impl Generator for Flat {
 fn material(&self,_:u32,y:u32,_:u32)->u16 {u16::from(y<16)}
 fn uniform_box(&self,min:[u32;3],max:[u32;3])->Option<u16>{if max[1]<=16{Some(1)}else if min[1]>=16{Some(0)}else{None}}
}
fn shape()->Shape {Shape {continuous:false,detail:0,half:[2,2],support:[1,1],clearance:Fx::from_int(3),step:4,min_up:Fx::from_ratio(7,10),height:8,span:3,reach:0,grade_slack:Fx::ZERO,ramp:Fx::ONE}}
fn pose(height:i32)->Pose {Pose {height:Fx::from_int(height),floors:[Fx::from_int(height);4],blocked:0,tilt:false,steep:false,excess:Fx::ZERO,crush:0}}
#[test]
fn transition_first_refusal_lift_span_and_direction_are_explicit() {
 let s=shape();let a=[Fx::from_int(32);2];let b=[Fx::from_int(33);2];
 for level in [Level::Coarse,Level::Fine,Level::Motor] {
  let policy=Policy {level,..Policy::fine(s,false)};
  let p=TransitionProof::geometry(a,b,pose(16),pose(20),s,policy);
  assert!(p.accepted());assert_eq!(p.kind,Kind::StepUp);assert_eq!(p.required_lift,Fx::from_int(4));assert_eq!(p.span,Fx::from_int(3));
  let mut bad=pose(25);bad.blocked=1;bad.tilt=true;bad.steep=true;
  let mut p=TransitionProof::geometry(a,b,pose(16),bad,s,policy);
  p.refuse(Reason::Moving);p.refuse(Reason::WindowUnready);assert_eq!(p.first_refusal,Some(Reason::Obstacle));
  assert_eq!(p.required_lift,Fx::from_int(9));assert!(!p.sources_checked);assert!(p.sources.is_empty());
 }
 let fine=TransitionProof::geometry(a,b,pose(24),pose(16),s,Policy::fine(s,false));
 assert_eq!(fine.kind,Kind::DropDown);assert_eq!(fine.required_lift,Fx::ZERO);assert_eq!(fine.drop,Fx::from_int(8));assert_eq!(fine.first_refusal,Some(Reason::DropLimit));
 let motor=TransitionProof::geometry(a,b,pose(24),pose(16),s,Policy::motor(s,false,false));
 assert_eq!(motor.first_refusal,fine.first_refusal);assert_eq!(motor.drop_limit,fine.drop_limit);
 // Passive fall is not a planned, source-checked landing or a Ready vote.
 let falling=TransitionProof::geometry(a,b,pose(24),pose(16),s,Policy::passive(s,false,false));
 assert!(falling.accepted());assert_eq!(falling.kind,Kind::DropDown);assert!(!falling.sources_checked);
}
#[test]
fn transition_coarse_fine_and_motor_share_sources_on_flat_ground() {
 let w=World::new(128,64,128,65536,Box::new(Flat));let r=Rubble::new();let s=shape();
 let from=[32,32];let to=[36,32];let a=from.map(Fx::from_int);let b=to.map(Fx::from_int);
 let cfg=coarse::Config {cell:8,nodes:100,yielding:[0,0],local_grade:true,restarts:1};
 let coarse=coarse::transition_proof(&w,&r,s,from,to,cfg);
 let mut ground=Probe::new(&w).with_rubble(&r);
 let fine=ground.transition_proof(a,0,b,0,s,Policy::fine(s,false));
 let motor=ground.transition_proof(a,0,b,0,s,Policy::motor(s,false,false));
 for p in [&coarse,&fine,&motor] {assert!(p.accepted(),"{p:?}");assert!(p.sources_checked);assert!(!p.sources.is_empty() && p.sources.len()<=2048);assert!(p.sources.iter().all(|s|s.source.kind==0 && s.validity==Validity::Supported));}
 assert_eq!(coarse.sources,fine.sources);assert_eq!(fine.sources,motor.sources);
 assert_eq!(fine,Probe::new(&w).with_rubble(&r).transition_proof(a,0,b,0,s,Policy::fine(s,false)),"cold query differs from warm");
}
fn piece()->Piece {
 Piece {source:SupportRef {kind:1,id:77,content:9,pose:2},position:[Fx::from_int(64),Fx::from_ratio(359,10),Fx::from_int(64)],
 basis:[[Fx::ONE,Fx::ZERO,Fx::ZERO],[Fx::ZERO,Fx::ONE,Fx::ZERO],[Fx::ZERO,Fx::ZERO,Fx::ONE]],low:[Fx::from_int(44),Fx::from_int(16),Fx::from_int(44)],high:[Fx::from_int(84),Fx::from_int(56),Fx::from_int(84)],
 shape:u64::MAX,centre:[Fx::from_ratio(3,2);3],side:4,mm:1000,stable:true,moving:false}
}
#[test]
fn transition_sources_do_not_inherit_same_height_replacement_wake_remove_or_reclaim() {
 let w=World::new(128,128,128,65536,Box::new(Flat));let s=shape();let at=[Fx::from_int(64);2];
 let columns=||{let mut c=Vec::new();for x in 60..=68{for z in 60..=68{c.push(([x,z],56));}}c};
 let stable=Rubble::from_sorted(columns()).with_sources(Sources::new(vec![piece()]));
 let old=Probe::new(&w).with_rubble(&stable).transition_proof(at,0,at,0,s,Policy::fine(s,false));
 assert!(old.accepted(),"{old:?}");assert!(old.sources.iter().all(|s|s.source.id==77));
 for event in 0..4 {
  let mut p=piece();
  match event {0=>{p.moving=true;p.stable=false;p.source.pose+=1;},1=>{},2=>{p.source.id+=1;},_=>{p.shape&=!(1u64<<63);p.source.content+=1;}}
  let rubble=if event==1 {Rubble::new().with_sources(Sources::new(vec![]))}else if event==0 {Rubble::new().with_sources(Sources::new(vec![p]))}else{Rubble::from_sorted(columns()).with_sources(Sources::new(vec![p]))};
  let proof=Probe::new(&w).with_rubble(&rubble).transition_proof(at,0,at,0,s,Policy::fine(s,false));
  if event==0 {assert_eq!(proof.first_refusal,Some(Reason::Moving));assert!(proof.sources.iter().any(|s|s.validity==Validity::Moving));}
  else {assert!(proof.accepted(),"event{event}: {proof:?}");assert_ne!(proof.sources,old.sources);}
  let mut bytes=fx::blob::Blob::new();rubble.sources().save_baseline(&mut bytes);
  let sources=Sources::load_baseline(&mut fx::blob::Scan::new(&bytes.v)).unwrap();
  let cold=Rubble::from_sorted(rubble.iter().map(|(p,h)|(*p,*h)).collect()).with_sources(sources);
  assert_eq!(proof,Probe::new(&w).with_rubble(&cold).transition_proof(at,0,at,0,s,Policy::fine(s,false)));
 }
}

struct Wall;
impl Generator for Wall {fn material(&self,x:u32,y:u32,_:u32)->u16 {u16::from(y<16 || x==40 && y<40)}}
#[test]
fn transition_sweep_cannot_accept_clear_endpoints_through_a_wall_or_an_unchecked_suffix() {
 let w=World::new(512,64,128,65536,Box::new(Wall));let s=shape();let a=[Fx::from_int(32);2];
 let p=Probe::new(&w).transition_proof(a,0,[Fx::from_int(48),a[1]],0,s,Policy::fine(s,false));
 assert!(!p.accepted());assert!(p.failure_at.unwrap()[0]<Fx::from_int(48));assert!(p.checked_samples>1);
 let p=Probe::new(&w).transition_proof([Fx::from_int(100),a[1]],0,[Fx::from_int(400),a[1]],0,s,Policy::fine(s,false));
 assert_eq!(p.first_refusal,Some(Reason::Budget));assert_eq!(p.checked_samples,80);assert!(p.sources.len()<=2048);
}
struct Ditch;
impl Generator for Ditch {fn material(&self,x:u32,y:u32,_:u32)->u16 {u16::from(y<if(40..45).contains(&x){8}else{16})}}
#[test]
fn transition_cross_gap_names_real_span_witnesses_without_raising_the_low_source() {
 let w=World::new(128,64,128,65536,Box::new(Ditch));
 let mut s=shape();s.span=18;s.ramp=Fx::ZERO;
 let a=[Fx::from_int(32);2];let b=[Fx::from_int(52),a[1]];
 let p=Probe::new(&w).transition_proof(a,0,b,0,s,Policy::fine(s,false));
 assert!(p.accepted(),"{p:?}");assert_eq!(p.kind,Kind::CrossGap);assert_eq!(p.span,Fx::from_int(18));
 assert!(p.sources.iter().any(|s|s.height==Fx::from_int(8)));
 assert!(p.sources.iter().any(|s|s.height==Fx::from_int(16)));
 s.span=2;
 assert!(!Probe::new(&w).transition_proof(a,0,b,0,s,Policy::fine(s,false)).accepted(),"short span granted the same bridge");
}

#[test]
fn geometry_and_corridor_accepted_match_full_transition_proof() {
 let a=[Fx::from_int(8);2];let b=[Fx::from_int(12);2];
 for step in [-2,0,1,4,16] {
  let mut s=shape();s.step=step;
  for low in [0,16,32,65535] {
   for high in [0,16,20,32,65535] {
    assert_eq!(
     TransitionProof::corridor_accepted(s,low,high),
     TransitionProof::corridor(a,b,s,low,high).accepted(),
     "corridor mismatch step={step} low={low} high={high}"
    );
   }
  }
  for h0 in [-5,0,16,24] {
   for h1 in [-5,0,16,20,25] {
    for blocked in [0,1] {
     for tilt in [false,true] {
      for steep in [false,true] {
       let old=pose(h0);
       let mut next=pose(h1);next.blocked=blocked;next.tilt=tilt;next.steep=steep;
       for grade_exit in [false,true] {
        for overlap_exit in [false,true] {
         for passive in [false,true] {
          let pol=if passive {Policy::passive(s,grade_exit,overlap_exit)}
           else {Policy::motor(s,grade_exit,overlap_exit)};
          assert_eq!(
           TransitionProof::geometry_accepted(old,next,pol),
           TransitionProof::geometry(a,b,old,next,s,pol).accepted(),
           "geometry mismatch step={step} h0={h0} h1={h1}"
          );
         }
        }
       }
      }
     }
    }
   }
  }
 }
}

#[test]
fn geometry_and_corridor_accepted_keep_fractional_extremes_and_custom_drop_policy() {
 let a=[Fx::ZERO;2];let b=[Fx::ONE;2];let s=shape();
 let heights=[Fx::MIN,Fx(-1),Fx::ZERO,Fx(1),Fx::ONE,Fx::MAX];
 for h0 in heights {for h1 in heights {
  for lift in [Fx::from_int(-1),Fx::ZERO,Fx::ONE,Fx::MAX] {
   for drop in [None,Some(Fx::from_int(-1)),Some(Fx::ZERO),Some(Fx::ONE),Some(Fx::MAX)] {
    for flags in 0..32 {
     let old=Pose {height:h0,..pose(0)};
     let next=Pose {height:h1,blocked:if flags&1!=0 {usize::MAX}else{0},tilt:flags&2!=0,steep:flags&4!=0,..pose(0)};
     let p=Policy {level:Level::Motor,lift,drop,grade_exit:flags&8!=0,grade_slack:Some(Fx::ZERO),overlap_exit:flags&16!=0};
     assert_eq!(TransitionProof::geometry_accepted(old,next,p),TransitionProof::geometry(a,b,old,next,s,p).accepted());
    }
   }
  }
 }}
 for step in [i32::MIN,-1,0,1,i32::MAX] {for low in [i32::MIN,0,i32::MAX] {for high in [i32::MIN,0,i32::MAX] {
  let s=Shape {step,..s};assert_eq!(TransitionProof::corridor_accepted(s,low,high),TransitionProof::corridor(a,b,s,low,high).accepted());
 }}}
}

#[test]
fn grade_exit_excess_enforces_monotonicity_and_slack() {
    let mut s = shape();
    s.grade_slack = Fx::from_ratio(1, 2);
    let a = [Fx::ZERO; 2];
    let b = [Fx::ONE; 2];
    let old = Pose {
        steep: true,
        excess: Fx::from_int(2),
        ..pose(10)
    };
    let within_slack = Pose {
        steep: true,
        excess: Fx::from_ratio(5, 2),
        ..pose(10)
    };
    let steeper = Pose {
        steep: true,
        excess: Fx::from_int(3),
        ..pose(10)
    };
    let policy = Policy::fine(s, true);
    assert!(TransitionProof::geometry_accepted(old, within_slack, policy));
    assert!(TransitionProof::geometry(a, b, old, within_slack, s, policy).accepted());
    assert!(!TransitionProof::geometry_accepted(old, steeper, policy));
    assert_eq!(
        TransitionProof::geometry(a, b, old, steeper, s, policy).first_refusal,
        Some(Reason::Normal)
    );
    let escape_policy = Policy::motor(s, true, false).unbounded_grade_exit();
    assert!(TransitionProof::geometry_accepted(old, steeper, escape_policy));
}
