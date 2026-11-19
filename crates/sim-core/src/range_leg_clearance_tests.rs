use super::*;
use crate::damage::{self, Frame};
use modding::range::RangeUnitKind;

struct Flat;
impl voxel::Generator for Flat {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 { u16::from(y < 16) }
}

fn occupied(body: &UnitBody) -> Vec<V3> {
    let half=Fx::from_ratio(1,2);
    let mut out=Vec::new();
    for z in 0..usize::from(body.vol.nz) { for y in 0..usize::from(body.vol.ny) {
        for x in 0..usize::from(body.vol.nx) {
            if body.vol.get(x,y,z) {out.push(V3::new(Fx::from_int(x as i32).add(half),Fx::from_int(y as i32).add(half),Fx::from_int(z as i32).add(half)));}
        }
    }} out
}
fn overlap(a: usize,b: usize,body: &UnitBody,frames: &[Frame],points: &[Vec<V3>]) -> bool {
    let volume=&body.segments[b].body.vol;
    // Exact transformed occupied cell centres, without rounding world coordinates.
    points[a].iter().any(|p| {
        let q=frames[b].local(frames[a].world([p.x,p.y,p.z]));
        if q.iter().any(|v| *v < Fx::ZERO) {return false;}
        let q=q.map(|v| v.to_int_trunc() as usize);
        q[0]<usize::from(volume.nx) && q[1]<usize::from(volume.ny) && q[2]<usize::from(volume.nz)
            && volume.get(q[0],q[1],q[2])
    })
}
fn bounds(body: &UnitBody,frame:&Frame) -> ([Fx;3],[Fx;3]) {
    let mut low=[Fx::MAX;3];let mut high=[Fx::MIN;3];
    for x in [0,i32::from(body.vol.nx)] {for y in [0,i32::from(body.vol.ny)] {for z in [0,i32::from(body.vol.nz)] {
        let p=frame.world([x,y,z].map(Fx::from_int));
        for (i,v) in [p.x,p.y,p.z].into_iter().enumerate() {low[i]=low[i].min(v);high[i]=high[i].max(v);}
    }}}
    (low,high)
}
fn audit(models: Vec<(String,RangeUnitKind,UnitBody)>) {
    let mut errors=Vec::new();
    for (name,kind,body) in models {
        let world=World::new(2048,512,2048,65536,Box::new(Flat));
        let points=body.segments.iter().map(|s| occupied(&s.body)).collect::<Vec<_>>();
        let rigs=crate::gait::rigs_of(&body);
        let parts=|i:usize| {
            let r=&rigs[i];
            [Some(r.thigh),Some(r.shin),r.knee_ball,r.ankle_ball,r.foot].into_iter().flatten().collect::<Vec<_>>()
        };
        let parts=(0..rigs.len()).map(parts).collect::<Vec<_>>();
        for pilot in [[1,0],[-1,0],[0,1],[0,-1],[1,1],[-1,1]] {
            let mut m=Motion::new([1024,16,1024],kind,&body);
            m.enabled=true;m.pilot=Some(pilot);
            'ticks: for tick in 0..180 {
                m.step(&world,&body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,
                    &nav::vehicle::Rubble::new(),&nav::vehicle::Changed::default());
                if tick%6!=0 {continue;}
                let root=m.frame(&body);
                let mut frames=damage::segment_frames(&root,&body,m.drive.head,m.drive.head);
                for (r,f) in rigs.iter().zip(&m.feet) {if let Some(c)=f.contact {crate::gait::leg_frames(r,&root,&c,&mut frames);}}
                let boxes=body.segments.iter().zip(&frames).map(|(s,f)| bounds(&s.body,f)).collect::<Vec<_>>();
                for a in 0..rigs.len() {for b in a+1..rigs.len() {
                    for &pa in &parts[a] {for &pb in &parts[b] {
                        if (0..3).any(|i| boxes[pa].1[i]<=boxes[pb].0[i] || boxes[pb].1[i]<=boxes[pa].0[i]) {continue;}
                        if overlap(pa,pb,&body,&frames,&points) || overlap(pb,pa,&body,&frames,&points) {
                            errors.push(format!("{name} input={pilot:?} tick={tick} legs={a}/{b} parts={pa}/{pb}"));
                            break 'ticks;
                        }
                    }}
                }}
            }
        }
    }
    for e in &errors {println!("LEG_OVERLAP {e}");}
    assert!(errors.is_empty(),"{} leg intersections",errors.len());
}

#[test]
fn range_walkers_do_not_step_through_other_legs() {
    audit(vec![
        ("commander".into(),RangeUnitKind::Commander,voxel::unit::commander()),
        ("walker04".into(),RangeUnitKind::Walker04,voxel::unit::walker04()),
        ("strider".into(),RangeUnitKind::Strider,voxel::unit::strider()),
        ("spider".into(),RangeUnitKind::Spider,voxel::unit::spider()),
    ]);
}

#[test]
fn roster_walkers_do_not_step_through_other_legs() {
    audit(voxel::roster::walking().into_iter().map(|(name,body)|
        (name.into(),RangeUnitKind::Strider,body)).collect());
}

#[test]
fn safe_lanes_keep_stance_contacts_and_leave_real_lifted_steps() {
    let mut errors=Vec::new();
    for kind in [RangeUnitKind::Commander,RangeUnitKind::Walker04,RangeUnitKind::Strider,RangeUnitKind::Spider] {
        let body=match kind {RangeUnitKind::Commander=>voxel::unit::commander(),RangeUnitKind::Walker04=>voxel::unit::walker04(),RangeUnitKind::Strider=>voxel::unit::strider(),_=>voxel::unit::spider()};
        let world=World::new(2048,512,2048,65536,Box::new(Flat));
        let mut m=Motion::new([1024,16,1024],kind,&body);
        m.enabled=true;m.pilot=Some([1,0]);
        let mut previous=vec![None;m.feet.len()];
        let mut slide=Fx::ZERO;let mut lift=Fx::ZERO;
        for tick in 0..120 {
            m.step(&world,&body,true,true,crate::mobility::Mobility::intact(true),&[],tick,98,
                &nav::vehicle::Rubble::new(),&nav::vehicle::Changed::default());
            for (i,(rig,f)) in m.rigs.iter().zip(&m.feet).enumerate() {
                let Some(c)=f.contact else {continue;};
                let current=f.planted().then_some(c.pos);
                if let (Some(p),Some(q))=(previous[i],current) {slide=slide.max(q.sub(p).len_sq().sqrt());}
                previous[i]=current;
                let bottom=rig.sole.probes().iter().map(|p|p.y).min().unwrap_or(Fx::ZERO);
                lift=lift.max(c.pos.y.add(bottom).sub(Fx::from_int(16)));
            }
        }
        println!("CONTACT {kind:?} stride={:?} slide={slide:?} lift={lift:?}",m.chassis.gait);
        if slide>=Fx::from_ratio(3,20) {errors.push(format!("{kind:?} skates: {slide:?}"));}
        if lift<=Fx::from_ratio(1,2) {errors.push(format!("{kind:?} lost its lifted step: {lift:?}"));}
        assert!(m.pos[0]>Fx::from_int(1025),"{kind:?} stopped moving");
    }
    assert!(errors.is_empty(),"{errors:?}");
}
