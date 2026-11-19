use super::*;
use modding::range::RangeUnitKind;

struct Flat;
impl voxel::Generator for Flat {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 { u16::from(y < 16) }
    fn batches_columns(&self)->bool {true}
    fn uniform_box(&self,min:[u32;3],max:[u32;3])->Option<u16> {
        if max[1]<=16 {Some(1)} else if min[1]>=16 {Some(0)} else {None}
    }
    fn column(&self,_:u32,_:u32,y0:u32,out:&mut[u16]) {
        out.fill(0);
        let end=(16u32.saturating_sub(y0)as usize).min(out.len());
        out[..end].fill(1);
    }
}

fn bytes(m: &Motion) -> Vec<u8> {
    let mut b = Blob::new(); m.save(&mut b); b.v
}

#[test]
fn real_walking_rigs_keep_anchors_and_bounded_control_in_the_sharp_turn_window() {
    let world=World::new(2048,512,2048,65536,Box::new(Flat));
    let rubble=nav::vehicle::Rubble::new();let changed=nav::vehicle::Changed::default();
    for (kind,body) in [(RangeUnitKind::Commander,voxel::unit::commander()),
        (RangeUnitKind::Walker04,voxel::unit::walker04()),(RangeUnitKind::Strider,voxel::unit::strider()),
        (RangeUnitKind::Spider,voxel::unit::spider())] {
        let mut m=Motion::new([1024,16,1024],kind,&body);m.enabled=true;m.trace_anchors(true);
        let mut copy=None;let mut first=None;let mut turns=0;let mut limited=0;let mut outside_lane=false;
        for tick in 0..300 {
            let input=if (200..260).contains(&tick)&&(tick-200)%6==0 {
                Some(if (tick-200)/6%2==0 {(1,-1)}else{(-1,1)})
            }else{match tick {0=>Some((1,0)),80=>Some((0,0)),110=>Some((0,1)),
                160=>Some((-1,0)),260=>Some((0,0)),_=>None}};
            let old=m.drive;let at=m.pos;let previous=m.feet.clone();
            let factor=m.drive_scale.mul(m.hull.load);
            let response=crate::mobility::Mobility::intact(true);
            let limits=response.limits(&Limits::of(&m.chassis,TICKS),false);
            m.clear_anchor_trace();
            for motion in std::iter::once(&mut m).chain(copy.iter_mut()) {
                if let Some((throttle,turn))=input {motion.pilot=Some([throttle,turn]);}
                motion.step(&world,&body,true,true,response,&[],tick,98,&rubble,&changed);
            }
            if let Some(other)=&copy {
                let (a,b)=(bytes(&m),bytes(other));
                assert!(a==b,"{kind:?} turn restore {tick}, first byte difference {:?}",
                    a.iter().zip(&b).position(|(a,b)|a!=b));
            }
            else if tick==201 {
                let mut other=Motion::new([1024,16,1024],kind,&body);
                assert!(other.load(&mut Scan::new(&bytes(&m))));
                // Pilot orders belong to the range/battle input state, not
                // the low-level motor snapshot used by this isolated test.
                other.pilot=m.pilot;copy=Some(other);
            }
            if (m.pos[0]!=at[0]||m.pos[2]!=at[2])&&first.is_none(){first=Some(tick);}
            let eps=Fx::from_ratio(1,50);
            let max_change=limits.accel.max(limits.brake).mul(factor).add(eps);
            assert!(m.drive.vel.sub(old.vel).abs()<=max_change,"{kind:?} abrupt speed at {tick}");
            let alpha=limits.turn_acc.mul(factor).to_int_trunc().max(1);
            assert!((m.drive.yaw_rate-old.yaw_rate).abs()<=alpha,"{kind:?} abrupt yaw at {tick}");
            assert_eq!(angle::diff(m.drive.head,old.head),m.drive.yaw_rate,"post-hoc yaw scaling");
            let travel=m.drive.vel.div(Fx::from_int(TICKS));
            assert!(m.pos[0].sub(at[0]).sub(travel.mul(angle::cos(m.drive.head))).abs()<=eps
                && m.pos[2].sub(at[2]).sub(travel.mul(angle::sin(m.drive.head))).abs()<=eps,
                "{kind:?} displayed drive velocity disagrees with actual movement at {tick}");
            if (200..260).contains(&tick){turns+=angle::diff(m.drive.head,old.head).abs();}
            let root=m.frame(&body);let mut real_supports=0;
            for (i,((rig,foot),old)) in m.rigs.iter().zip(&m.feet).zip(&previous).enumerate() {
                if old.planted()&&foot.planted(){assert_eq!(old.contact,foot.contact,"{kind:?} silent anchor at {tick}/{i}");}
                let c=foot.contact.unwrap();let d=c.pos.sub(root.base);
                let local=V3::new(d.dot(root.x),d.dot(root.y),d.dot(root.z));
                let solved=rig.solve_contact(local).ankle;
                assert!(solved.sub(local).len_sq()<=eps.mul(eps),"{kind:?} unreachable actual contact at {tick}/{i}");
                if foot.planted(){
                    outside_lane|=rig.lane_target(local).sub(local).len_sq()>eps.mul(eps);
                    let ankle=root.base.add(root.x.scale(solved.x)).add(root.y.scale(solved.y)).add(root.z.scale(solved.z));
                    let axes=c.axes();
                    let gap=rig.sole.probes().iter().map(|p|ankle.add(axes[0].scale(p.x))
                        .add(axes[1].scale(p.y)).add(axes[2].scale(p.z)).y.sub(Fx::from_int(16))).min().unwrap();
                    assert!(gap.abs()<=eps,"{kind:?} false planted flag at {tick}/{i}");real_supports+=1;
                }
            }
            if m.hull.grounded(){assert!(real_supports>0,"{kind:?} lost every real support at {tick}");}
            let trace=m.anchor_trace().unwrap();limited+=trace.window_limited;
            assert_eq!(trace.window_unready,0,"{kind:?} no bounded continuation at {tick}");
            assert!(trace.events().len()<=crate::gait::AnchorTrace::LIMIT);
        }
        assert!(first.is_some_and(|t|t<=2),"{kind:?} delayed command response");
        assert!(turns>angle::FULL/180,"{kind:?} turn controls froze");
        if kind==RangeUnitKind::Spider {assert!(limited>0,"exercise pre-commit window admission");}
        if kind==RangeUnitKind::Strider {assert!(outside_lane,"exercise reachable off-lane stance, not a lane-clamped surrogate");}
    }
}

#[test]
fn bipeds_keep_a_real_support_through_start_turn_reverse_sidestep_and_stop() {
    let world = World::new(2048, 512, 2048, 65536, Box::new(Flat));
    let rubble = nav::vehicle::Rubble::new();
    let changed = nav::vehicle::Changed::default();
    for kind in [RangeUnitKind::Commander, RangeUnitKind::Walker04] {
        let body = if kind == RangeUnitKind::Commander { voxel::unit::commander() }
                   else { voxel::unit::walker04() };
        let mut m = Motion::new([1024, 16, 1024], kind, &body);
        m.enabled = true;
        let mut copy: Option<Motion> = None;
        let mut first = None;
        let mut lifted = [false; 2];
        let mut max_gap = Fx::ZERO;
        let mut max_error = Fx::ZERO;
        let mut swings = [0; 2];
        let mut previously_planted = [true; 2];
        let mut forward=Fx::ZERO;let mut commanded=Fx::ZERO;let mut turned=0;
        for tick in 0..400 {
            let previous=m.feet.clone();let at=m.pos;let head=m.drive.head;
            let pilot = match tick {
                0..=79 => Some([1, 0]),
                80..=109 => Some([0, 0]),
                110..=159 => Some([0, 1]),
                160..=199 => Some([-1, 0]),
                200..=259 => Some([1, -1]),
                _ => Some([0, 0]),
            };
            for motion in std::iter::once(&mut m).chain(copy.iter_mut()) {
                motion.pilot = pilot;
                // Independent translation while the torso faces sideways.
                if tick == 300 {
                    motion.pilot = None;
                    let to = [motion.pos[0].to_int_trunc() + 100, motion.pos[2].to_int_trunc()];
                    motion.destination = Some(to);
                    motion.path = vec![to];
                    motion.cursor = 0;
                }
                if (301..350).contains(&tick) { motion.pilot = None; }
                if tick == 350 {
                    motion.destination = None;
                    motion.path.clear();
                }
                motion.step(&world, &body, true, true, crate::mobility::Mobility::intact(true),
                    &[], tick, 98, &rubble, &changed);
            }
            if let Some(copy) = &copy {
                assert_eq!(bytes(&m), bytes(copy), "{kind:?} support restore tick {tick}");
            } else if tick == 65 {
                let mut restored = Motion::new([1024, 16, 1024], kind, &body);
                assert!(restored.load(&mut Scan::new(&bytes(&m))));
                copy = Some(restored);
            }
            if m.pos[0] != Fx::from_int(1024) && first.is_none() { first = Some(tick); }
            if (50..80).contains(&tick) {
                forward=forward.add(m.pos[0].sub(at[0]));
                commanded=commanded.add(m.drive.vel.div(Fx::from_int(TICKS)));
            }
            if (110..160).contains(&tick) {turned+=angle::diff(m.drive.head,head).abs();}
            assert!(m.hull.grounded(), "{kind:?} body left flat floor at {tick}");
            assert!(m.feet.iter().any(|f| f.planted()), "{kind:?} both feet swinging at {tick}");
            let root = m.frame(&body);
            let mut touching = false;
            for (i, (rig, foot)) in m.rigs.iter().zip(&m.feet).enumerate() {
                let c = foot.contact.expect("contact after a grounded step");
                if previous[i].planted() && foot.planted() {
                    assert_eq!(previous[i].contact,foot.contact,"{kind:?} stance changed without liftoff tick={tick} leg={i}");
                }
                if previously_planted[i] && !foot.planted() { swings[i] += 1; }
                previously_planted[i] = foot.planted();
                if !foot.planted() { lifted[i] = true; continue; }
                let d = c.pos.sub(root.base);
                let target = V3::new(d.dot(root.x), d.dot(root.y), d.dot(root.z));
                let solved = rig.solve_contact(target).ankle;
                max_error = max_error.max(solved.sub(target).len_sq().sqrt());
                // Use the solved ankle actually drawn by leg_frames, not just
                // the planted flag or the desired target above empty space.
                let ankle = root.base.add(root.x.scale(solved.x)).add(root.y.scale(solved.y)).add(root.z.scale(solved.z));
                let axes = c.axes();
                let gap = rig.sole.probes().iter().map(|p|
                    ankle.add(axes[0].scale(p.x)).add(axes[1].scale(p.y)).add(axes[2].scale(p.z)).y
                        .sub(Fx::from_int(16))).min().unwrap();
                max_gap = max_gap.max(gap.abs());
                touching |= gap.abs() <= Fx::from_ratio(1, 50);
            }
            assert!(touching, "{kind:?} no actual sole on flat floor at {tick}; gap {max_gap:?}, IK {max_error:?}");
        }
        println!("BIPED_SUPPORT {kind:?} first={first:?} swings={swings:?} max_gap={max_gap:?} max_ik={max_error:?}");
        assert!(first.is_some_and(|t| t <= 2), "{kind:?} delayed movement");
        assert!(lifted.into_iter().all(|v| v), "{kind:?} froze a leg instead of walking");
        assert!(swings.into_iter().all(|v| v >= 3), "{kind:?} did not alternate legs");
        assert!(max_error <= Fx::from_ratio(1, 20), "{kind:?} unreachable support: {max_error:?}");
        assert!(max_gap <= Fx::from_ratio(1, 50), "{kind:?} floating/penetrating planted sole: {max_gap:?}");
        assert!(m.feet.iter().all(|f| f.planted()), "{kind:?} stopped with unfinished swing");
        // Walker 04's voxel mass still limits its acceleration on this short
        // interval. Compare the physical distance with its live motor speed.
        assert!(commanded>Fx::ONE && forward>=commanded.mul(Fx::from_ratio(90,100)),
            "{kind:?} lost forward progress: {forward:?} requested {commanded:?}");
        assert!(turned>=angle::FULL/12,"{kind:?} pivot stalled: {turned}");
    }
}
