use super::*;
use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};

struct Ground(Arc<AtomicUsize>);
impl voxel::Generator for Ground {
    fn material(&self,_:u32,y:u32,_:u32)->u16 {self.0.fetch_add(1,Ordering::Relaxed);u16::from(y<16)}
    fn uniform_box(&self,min:[u32;3],max:[u32;3])->Option<u16> {if max[1]<=16 {Some(1)}else if min[1]>=16 {Some(0)}else{None}}
}

#[test]
fn motor_floor_and_support_memos_keep_exact_keys_and_drop_phase_state() {
    use std::cell::Cell;
    let calls=Cell::new(0);
    let floor=MotorFloorMemo::default();
    for x in [Fx::from_ratio(-7,8),Fx::from_ratio(-1,8)] {
        assert_eq!(floor.at_cell(x,x,||{calls.set(calls.get()+1);(Fx::from_int(16),true)}),Fx::from_int(16));
    }
    assert_eq!(calls.get(),1,"a proven clean integer column shares fractional probes");
    let exact=MotorFloorMemo::default();
    for x in [Fx::from_ratio(1,4),Fx::from_ratio(3,4)] {
        assert_eq!(exact.at_cell(x,x,||{calls.set(calls.get()+1);(x,false)}),x);
    }
    assert_eq!(calls.get(),3,"occupied columns retain exact sub-cell keys");
    let memo=MotorSupportMemo::default();let reads=Cell::new(0);
    for x in [0,128,0,128] {
        let value=memo.at([Fx::from_int(x),Fx::ZERO],0,||{reads.set(reads.get()+1);[Fx::from_int(x);4]});
        assert_eq!(value,[Fx::from_int(x);4]);
    }
    assert_eq!(reads.get(),2,"both ways survive a colliding support-floor key");
    memo.at([Fx::ZERO;2],1,||{reads.set(reads.get()+1);[Fx::ONE;4]});
    assert_eq!(reads.get(),3,"heading belongs to the exact support key");
    let fresh=MotorFloorMemo::default();
    assert_eq!(fresh.at_cell(Fx::from_ratio(-1,8),Fx::from_ratio(-1,8),||(Fx::from_int(17),true)),Fx::from_int(17));
}

#[test]
fn motor_surface_memo_refreshes_same_height_source_after_a_phase_boundary() {
    use nav::vehicle::support::Surface;
    let mut world=World::new(128,64,128,4096,Box::new(Ground(Arc::new(AtomicUsize::new(0)))));
    let sample=|world:&World| {
        let mut probe=nav::vehicle::Probe::new(world);
        let cell=std::cell::RefCell::new(&mut probe);let memo=MotorSurface::new(&cell);
        let a=memo.sample(Fx::from_ratio(257,4),Fx::from_int(16),Fx::from_ratio(257,4));
        let b=memo.sample(Fx::from_ratio(259,4),Fx::from_int(40),Fx::from_ratio(259,4));
        assert_eq!(a,b,"clean terrain is independent of fractional position and query height");a
    };
    let before=sample(&world);world.queue_edit(voxel::Edit::put(64,15,64,2));world.flush();
    let after=sample(&world);assert_eq!(after.height,before.height);assert_ne!(after.source,before.source);
}
#[test]
fn support_quiet_and_unrelated_events_do_no_sole_queries_and_ready_restores() {
    let reads=Arc::new(AtomicUsize::new(0));let world=World::new(2048,512,2048,65536,Box::new(Ground(reads.clone())));
    let body=voxel::unit::spider();let mut m=Motion::new([1024,16,1024],modding::range::RangeUnitKind::Spider,&body);
    let rubble=nav::vehicle::Rubble::new();let quiet=nav::vehicle::Changed::default();m.enabled=true;
    for tick in 0..8 {m.step(&world,&body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&rubble,&quiet);}
    assert!(m.feet.iter().all(|f|f.ready()));
    reads.store(0,Ordering::Relaxed);
    m.refresh_footing(&world,&rubble,&body,Some(&quiet));assert_eq!(reads.load(Ordering::Relaxed),0);
    let mut far=quiet.clone();far.column(32,32);
    m.refresh_footing(&world,&rubble,&body,Some(&far));assert_eq!(reads.load(Ordering::Relaxed),0);
    let contact=m.feet[0].contact.unwrap();let mut near=quiet.clone();near.column(contact.pos.x.to_int_trunc(),contact.pos.z.to_int_trunc());
    m.refresh_footing(&world,&rubble,&body,Some(&near));
    let queries=reads.load(Ordering::Relaxed);assert!(queries>0 && queries<=m.rigs.iter().map(|r|r.sole.probes().len()).sum::<usize>());
    let mut bytes=Blob::new();m.save(&mut bytes);
    let mut copy=Motion::new([1024,16,1024],modding::range::RangeUnitKind::Spider,&body);
    assert!(copy.load(&mut Scan::new(&bytes.v)));assert_eq!(m.feet,copy.feet);
    for tick in 8..25 {
        for motion in [&mut m,&mut copy] {motion.step(&world,&body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&rubble,&quiet);}
        let mut a=Blob::new();m.save(&mut a);let mut b=Blob::new();copy.save(&mut b);assert_eq!(a.v,b.v);
    }
}

#[test]
fn support_real_carrier_events_keep_world_anchors_and_restore_through_recovery() {
    use crate::gait::{Readiness,SupportReason};
    use nav::vehicle::support::Sources;
    let world=World::new(2048,512,2048,65536,Box::new(Ground(Arc::new(AtomicUsize::new(0)))));
    let body=voxel::unit::commander();let mut debris=crate::burst::Bursts::default();
    let up=[V3::new(Fx::ONE,Fx::ZERO,Fx::ZERO),V3::UP,V3::new(Fx::ZERO,Fx::ZERO,Fx::ONE)];
    for dx in [-40,0,40] {for dz in [-40,0,40] {
        let id=77+debris.fragments.len() as u64;
        debris.fragments.push(crate::burst::Fragment {id,position:V3::new(Fx::from_int(1024+dx),Fx::from_ratio(359,10),Fx::from_int(1024+dz)),
            velocity:V3::ZERO,material:1,paint:None,mm:1000,basis:up,mass_g:1,count:64,shape:u64::MAX,side:4,chunk:None,held:0,resting:true});
    }}
    let gather=|d:&crate::burst::Bursts|d.rubble_with_scratch(&world,&[],&[],&mut crate::burst::RubbleScratch::default(),&|_,_|{});
    let stable=gather(&debris);let quiet=nav::vehicle::Changed::default();
    let mut original=Motion::new([1024,56,1024],modding::range::RangeUnitKind::Commander,&body);original.enabled=true;
    for tick in 0..8 {original.step(&world,&body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&stable,&quiet);}
    assert!(original.feet.iter().all(|f|f.ready() && f.support.source.is_some_and(|s|s.kind==1 && s.id>=77 && s.id<86)));
    let mut bytes=Blob::new();original.save(&mut bytes);let start=bytes.v;
    for event in 0..4 {
        for (i,f) in debris.fragments.iter_mut().enumerate() {
            f.id=77+i as u64;f.shape=u64::MAX;f.count=64;f.resting=true;f.velocity=V3::ZERO;
            match event {0=>{f.resting=false;f.velocity.x=Fx::ONE;},2=>f.id+=100,
                3=>{f.shape&=!(1|(1u64<<63));f.count=62;},_=>{}}
        }
        let expected=match event {0=>SupportReason::Moving,1|2=>SupportReason::Removed,_=>SupportReason::SourceChanged};
        let mut next=if event==1 {nav::vehicle::Rubble::new().with_sources(Sources::new(vec![]))}else{gather(&debris)};
        if event==2 {assert_eq!(stable,next,"same-height replacement");}
        let mut changed=quiet.clone();stable.sources().changes(next.sources(),&mut changed);
        next.set_support_changes(changed.clone());
        let nav_changed=if stable==next {quiet.clone()}else{changed};
        let mut m=Motion::new([1024,56,1024],modding::range::RangeUnitKind::Commander,&body);
        assert!(m.load(&mut Scan::new(&start)));let before=m.feet.iter().map(|f|f.contact).collect::<Vec<_>>();
        m.invalidate_ground(Some(&nav_changed));m.refresh_footing(&world,&next,&body,Some(&nav_changed));
        next.set_support_changes(quiet.clone()); // A delivered source event lasts one phase.
        for (i,foot) in m.feet.iter().enumerate() {
            assert_eq!(foot.contact,before[i],"source event moved ankle {event}/{i}");
            assert_eq!(foot.support.reason,expected,"event {event}/{i}: {:?}",foot.support);
            assert_eq!(foot.support.readiness,if event==3 {Readiness::Ready}else{Readiness::Rejected});
        }
        let mut bytes=Blob::new();m.save(&mut bytes);
        let mut copy=Motion::new([1024,56,1024],modding::range::RangeUnitKind::Commander,&body);
        assert!(copy.load(&mut Scan::new(&bytes.v)));assert_eq!(m.feet,copy.feet);
        // Wake and removal keep their rejected reason through a saved recovery;
        // replacement can subsequently earn a new Ready proof on the new ID.
        for tick in 8..50 {
            for motion in [&mut m,&mut copy] {motion.step(&world,&body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&next,&quiet);}
            let mut a=Blob::new();m.save(&mut a);let mut b=Blob::new();copy.save(&mut b);assert_eq!(a.v,b.v,"event {event}, tick {tick}");
            if event==0 {assert!(!m.feet.iter().any(|f|f.ready() && f.support.source.is_some_and(|s|s.kind==1)),"moving carrier admitted weight");}
        }
        if event>=2 {assert!(m.feet.iter().all(|f|f.ready()),"event {event} never regained Ready");}
        if event==1 {assert!(m.feet.iter().all(|f|f.ready() && f.support.source.is_some_and(|s|s.kind==0)),"removed carrier did not recover onto terrain");}
    }
}

#[test]
fn support_terrain_content_revision_changes_at_the_same_height() {
    let mut world=World::new(2048,512,2048,65536,Box::new(Ground(Arc::new(AtomicUsize::new(0)))));
    let body=voxel::unit::commander();let mut m=Motion::new([1024,16,1024],modding::range::RangeUnitKind::Commander,&body);
    let rubble=nav::vehicle::Rubble::new();let quiet=nav::vehicle::Changed::default();m.enabled=true;
    for tick in 0..8 {m.step(&world,&body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&rubble,&quiet);}
    let old=m.feet[0].clone();assert!(old.ready());let c=old.contact.unwrap();let axes=c.axes();let mut dirty=quiet.clone();
    let mut saved=Blob::new();m.save(&mut saved);
    for p in m.rigs[0].sole.probes() {
        let at=c.pos.add(axes[0].scale(p.x)).add(axes[1].scale(p.y)).add(axes[2].scale(p.z));
        let (x,z)=(at.x.to_int_trunc(),at.z.to_int_trunc());
        world.queue_edit(voxel::Edit{x:x as u32,y:15,z:z as u32,mat:3,by:voxel::NOBODY});dirty.column(x,z);
    }
    world.flush();m.invalidate_ground(Some(&dirty));m.refresh_footing(&world,&rubble,&body,Some(&dirty));
    assert!(m.feet[0].ready());assert_eq!(m.feet[0].contact,old.contact);
    assert_ne!(m.feet[0].support.signature,old.support.signature);
    assert_eq!(m.feet[0].support.reason,crate::gait::SupportReason::SourceChanged);
    // A caller restoring a Motion may have no old dirty pages. Saved Ready
    // is still evidence from the old world, and must be checked before XY.
    let mut restored=Motion::new([1024,16,1024],modding::range::RangeUnitKind::Commander,&body);
    assert!(restored.load(&mut Scan::new(&saved.v)));
    assert_eq!(restored.feet[0].support,old.support);
    restored.step(&world,&body,true,true,crate::mobility::Mobility::intact(true),&[],8,98,&rubble,&quiet);
    assert!(restored.feet[0].ready());assert_eq!(restored.feet[0].contact,old.contact);
    assert_eq!(restored.feet[0].support,m.feet[0].support,"restore inherited the old same-height material proof");
}

struct PartialGround {lo:[u32;2],hi:[u32;2],keep:[u32;2]}
impl voxel::Generator for PartialGround {
    fn material(&self,x:u32,y:u32,z:u32)->u16 {
        let in_hole=(self.lo[0]..=self.hi[0]).contains(&x) && (self.lo[1]..=self.hi[1]).contains(&z) && [x,z]!=self.keep;
        u16::from(y<16 && !in_hole)
    }
}
#[test]
fn support_biped_does_not_lift_the_last_ready_foot_for_a_pending_partial_sole() {
    let world=World::new(2048,512,2048,65536,Box::new(Ground(Arc::new(AtomicUsize::new(0)))));
    let body=voxel::unit::commander();let mut m=Motion::new([1024,16,1024],modding::range::RangeUnitKind::Commander,&body);
    let rubble=nav::vehicle::Rubble::new();let quiet=nav::vehicle::Changed::default();m.enabled=true;
    for tick in 0..8 {m.step(&world,&body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&rubble,&quiet);}
    assert!(m.feet.iter().all(|f|f.ready()));let root=m.frame(&body);let profile=m.terrain_capability();
    let gait=crate::gait::Gait::of(m.chassis.gait.as_ref().unwrap());
    let probe=std::cell::RefCell::new(nav::vehicle::Probe::new(&world));
    let floor=|x,z|probe.borrow_mut().foot_floor(x,z);
    let mut chosen=None;
    for phase in 0..8 {
        let stance=crate::gait::Stance{grounded:true,root:&root,head:m.drive.head,phase:phase*angle::FULL/8,
            lift_scale:Fx::ONE,linear:V3::new(Fx::ONE,Fx::ZERO,Fx::ZERO),angular:0,cycles:Fx::from_ratio(1,10),gait:&gait,floor:&floor};
        let mut feet=m.feet.clone();
        Motion::advance_walking_feet(&m.rigs,&mut feet,&m.legs_lost,&m.legs_buckled,&stance,true,profile.min_support_pct,true,true,
            Motion::walking_spread(&root),m.fold_milli,m.guided_biped(&body),None,&probe,profile);
        if let Some(outgoing)=feet.iter().position(|f|!f.planted()) {chosen=Some((stance,outgoing));break;}
    }
    let (stance,outgoing)=chosen.expect("real rig must request a lift in the control case");let pending=1-outgoing;
    let c=m.feet[pending].contact.unwrap();let axes=c.axes();
    let points=m.rigs[pending].sole.probes().iter().map(|p| {
        let at=c.pos.add(axes[0].scale(p.x)).add(axes[1].scale(p.y)).add(axes[2].scale(p.z));
        [at.x.to_int_trunc() as u32,at.z.to_int_trunc() as u32]
    }).collect::<Vec<_>>();
    let lo=std::array::from_fn(|a|points.iter().map(|p|p[a]).min().unwrap());let hi=std::array::from_fn(|a|points.iter().map(|p|p[a]).max().unwrap());
    let partial=World::new(2048,512,2048,65536,Box::new(PartialGround{lo,hi,keep:points[0]}));
    let partial=std::cell::RefCell::new(nav::vehicle::Probe::new(&partial));
    let mut feet=m.feet.clone();feet[pending].support.readiness=crate::gait::Readiness::Pending;
    let old=feet[outgoing].contact;
    Motion::advance_walking_feet(&m.rigs,&mut feet,&m.legs_lost,&m.legs_buckled,&stance,true,profile.min_support_pct,true,true,
        Motion::walking_spread(&root),m.fold_milli,m.guided_biped(&body),None,&partial,profile);
    assert!(feet[outgoing].ready());assert_eq!(feet[outgoing].contact,old);
    assert!(!feet[pending].ready());assert_eq!(feet[pending].support.reason,crate::gait::SupportReason::PartialSole);
}


#[test]
fn support_changed_columns_are_invalidated_before_warm_or_prepared_sole_assessment() {
    for (kind,body) in [(modding::range::RangeUnitKind::Strider,voxel::unit::strider()),
        (modding::range::RangeUnitKind::Spider,voxel::unit::spider())] {
    let mut world=World::new(256,128,256,65536,Box::new(Ground(Arc::new(AtomicUsize::new(0)))));
    let mut warm=Motion::new([128,16,128],kind,&body);
    warm.enabled=true;
    let rubble=nav::vehicle::Rubble::new();let quiet=nav::vehicle::Changed::default();
    for tick in 0..8 {warm.step(&world,&body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,&rubble,&quiet);}
    assert!(warm.feet.iter().all(|f|f.ready()));
    let contact=warm.feet[0].contact.unwrap();let axes=contact.axes();
    let mut probe=nav::vehicle::Probe::for_search(&world,&warm.yielding,std::mem::take(&mut warm.terrain_columns)).with_rubble(&rubble);
    let points:Vec<_>=warm.rigs[0].sole.probes().iter().map(|p|contact.pos.add(axes[0].scale(p.x)).add(axes[1].scale(p.y)).add(axes[2].scale(p.z))).collect();
    for p in &points {assert_eq!(probe.foot_floor(p.x,p.z),Fx::from_int(16));}
    warm.terrain_columns=probe.take_columns();drop(probe);
    let mut saved=Blob::new();warm.save(&mut saved);
    let make=|| {let mut m=Motion::new([128,16,128],kind,&body);assert!(m.load(&mut Scan::new(&saved.v)));m.support_fresh=warm.support_fresh;m};
    let mut fresh=make();let mut prepared=make();prepared.terrain_columns=warm.terrain_columns.clone();
    let mut changed=quiet.clone();
    for p in points {let [x,z]=[p.x.to_int_trunc(),p.z.to_int_trunc()];world.queue_transfer(voxel::Edit::put(x as u32,16,z as u32,3));changed.column(x,z);}
    world.flush_transfers();
    let ground=prepared.prepare_ground(&world,&body,&rubble,&changed);
    for m in [&mut warm,&mut fresh,&mut prepared] {m.refresh_footing(&world,&rubble,&body,Some(&changed));}
    drop(ground);
    assert_eq!(fresh.feet[0].support.reason,crate::gait::SupportReason::Obstructed,"{kind:?}");
    assert_eq!(warm.feet[0],fresh.feet[0],"warm cached columns hid a real surface edit");
    assert_eq!(prepared.feet[0],fresh.feet[0],"prepared and direct support proofs disagree");
    assert_eq!(warm.feet[0].contact.unwrap().pos,contact.pos,"support refresh moved the anchor");
    }
}

#[test]
fn overhead_falling_canopy_does_not_pull_spider_feet_above_knees() {
    use nav::vehicle::support::{Piece, Sources, SupportRef};
    let world = World::new(512, 256, 512, 65536, Box::new(Ground(Arc::new(AtomicUsize::new(0)))));
    let body = voxel::unit::spider();
    let initial = Motion::new([256, 16, 256], modding::range::RangeUnitKind::Spider, &body);
    let top = initial.frame(&body).world([Fx::from_ratio(body.vol.nx as i32, 2),
        Fx::from_int(body.vol.ny as i32), Fx::from_ratio(body.vol.nz as i32, 2)]).y;
    let canopy_y = top.add(Fx::from_int(100));
    let canopy = Piece {
        source: SupportRef { kind: 1, id: 42, content: 1, pose: 1 },
        position: [Fx::from_int(256), canopy_y, Fx::from_int(256)],
        basis: [[Fx::ONE, Fx::ZERO, Fx::ZERO], [Fx::ZERO, Fx::ONE, Fx::ZERO], [Fx::ZERO, Fx::ZERO, Fx::ONE]],
        low: [Fx::from_int(57), canopy_y.sub(Fx::from_int(49)), Fx::from_int(57)],
        high: [Fx::from_int(455), canopy_y.add(Fx::from_int(49)), Fx::from_int(455)],
        centre: [Fx::from_ratio(3, 2), Fx::ZERO, Fx::from_ratio(3, 2)],
        shape: 0x000f_000f_000f_000f,
        side: 4,
        mm: 10000,
        stable: false,
        moving: true,
    };
    let rubble = nav::vehicle::Rubble::new().with_sources(Sources::new(vec![canopy]));
    let mut unbounded = nav::vehicle::Probe::new(&world).with_rubble(&rubble);
    let root = initial.frame(&body);
    let hits = initial.rigs.iter().filter(|rig| {
        let at = root.base.add(root.x.scale(rig.ankle.x)).add(root.y.scale(rig.ankle.y)).add(root.z.scale(rig.ankle.z));
        unbounded.foot_floor(at.x, at.z) > top
    }).count();
    assert!(hits >= initial.rigs.len()/2, "canopy geometry must intercept actual foot rays: {hits}");
    let quiet = nav::vehicle::Changed::default();
    for pilot in [[1, 0], [1, 1]] {
        let mut m = Motion::new([256, 16, 256], modding::range::RangeUnitKind::Spider, &body);
        m.enabled = true;
        m.pilot = Some(pilot);
        let mut travel = Fx::ZERO;
        for tick in 0..80 {
            let was = m.pos;
            m.step(&world, &body, true, true, crate::mobility::Mobility::intact(true), &[], tick, 98, &rubble, &quiet);
            let dx = m.pos[0].sub(was[0]);
            let dz = m.pos[2].sub(was[2]);
            travel = travel.add(dx.mul(dx).add(dz.mul(dz)).sqrt());
            let root = m.frame(&body);
            for (i, (rig, foot)) in m.rigs.iter().zip(&m.feet).enumerate() {
                let Some(c) = foot.contact else { continue; };
                assert!(!foot.ready() || foot.support.source.is_none_or(|s| s.kind != 1),
                    "moving canopy was admitted as a Ready support at tick {tick}");
                let d = c.pos.sub(root.base);
                let local = V3::new(d.dot(root.x), d.dot(root.y), d.dot(root.z));
                let pose = rig.solve_contact(local);
                assert!(pose.ankle.y < pose.knee.y,
                    "overhead canopy pulled leg {i} ankle {:?} above knee {:?} at tick {tick}", pose.ankle.y, pose.knee.y);
            }
        }
        assert!(travel > Fx::from_int(40), "spider stalled under overhead falling canopy: {travel:?}");
    }
}

#[test]
fn foot_obstruction_ceiling_tracks_the_actual_rolled_hip_frame() {
    let body = voxel::unit::spider();
    let mut m = Motion::new([256, 16, 256], modding::range::RangeUnitKind::Spider, &body);
    m.hull.up = V3::new(Fx::from_ratio(3, 5), Fx::from_ratio(4, 5), Fx::ZERO);
    let root = m.frame(&body);
    let expected = m.rigs.iter().map(|r| m.pos[1]
        .sub(r.hip.x.mul(Fx::from_ratio(3, 5))).add(r.hip.y.mul(Fx::from_ratio(4, 5))))
        .max().unwrap();
    let ceiling = m.foot_ceiling(&root);
    assert!(ceiling.sub(expected).abs() < Fx::from_ratio(1, 1000));
    let upright = m.pos[1].add(m.rigs.iter().map(|r| r.hip.y).max().unwrap());
    assert!(ceiling.sub(upright).abs() > Fx::ONE, "fixture must exercise the tilted hip height");
}


#[test]
fn spider_inner_fold_contact_chooses_nearest_reachable_body_height() {
    let world = World::new(512, 256, 512, 65536, Box::new(Ground(Arc::new(AtomicUsize::new(0)))));
    let spider_body = voxel::unit::spider();
    let rubble = nav::vehicle::Rubble::new();
    let quiet = nav::vehicle::Changed::default();
    let mut m = Motion::new([256, 16, 256], modding::range::RangeUnitKind::Spider, &spider_body);
    m.enabled = true;
    m.step(&world, &spider_body, true, true, crate::mobility::Mobility::intact(true), &[], 0, 98, &rubble, &quiet);
    let mut root = m.frame(&spider_body);
    let nominal_y = root.base.y;
    let rig0 = &m.rigs[0];
    let near = rig0.l1.sub(rig0.l2).abs().max(Fx::from_ratio(1, 1000));
    let mut c = m.feet[0].contact.unwrap();
    let hip_world = root.base.add(root.x.scale(rig0.hip.x)).add(root.y.scale(rig0.hip.y)).add(root.z.scale(rig0.hip.z));
    c.pos.x = hip_world.x;
    c.pos.z = hip_world.z;
    c.pos.y = hip_world.y.sub(near.sub(Fx::from_ratio(30, 100)));
    m.feet[0].contact = Some(c);
    let mut hull = m.hull;
    assert!(m.constrain_walking_pose(&spider_body, m.drive.head, m.hull, &m.feet, &mut hull, &mut root));
    let slam = nominal_y.sub(root.base.y).abs();
    assert!(slam <= Fx::from_int(1),
        "inner-sphere contact caused full-bound chassis slam: {:?} world units", slam);
}
