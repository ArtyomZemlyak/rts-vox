use super::*;
use modding::range::RangeUnitKind;

struct Flat;
impl voxel::Generator for Flat {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 { u16::from(y < 16) }
}

#[test]
fn walking_bodies_react_to_acceleration_brake_and_settle_without_losing_their_feet() {
    for kind in [RangeUnitKind::Commander, RangeUnitKind::Walker04,
        RangeUnitKind::Strider, RangeUnitKind::Spider] {
        let world = World::new(2048, 512, 2048, 65536, Box::new(Flat));
        let body = match kind {
            RangeUnitKind::Commander => voxel::unit::commander(),
            RangeUnitKind::Walker04 => voxel::unit::walker04(),
            RangeUnitKind::Strider => voxel::unit::strider(),
            _ => voxel::unit::spider(),
        };
        let mut m = Motion::new([1024, 16, 1024], kind, &body);
        m.enabled = true;
        let mut accelerating = Fx::ZERO;
        let mut braking = Fx::ZERO;
        let mut jump = Fx::ZERO;
        let mut last = m.hull.up;
        let mut restored = None;
        for tick in 0..400 {
            let pilot = if tick < 80 { [1, 0] } else { [0, 0] };
            m.pilot = Some(pilot);
            step(&mut m, &world, &body, &[], tick);
            if let Some(copy) = restored.as_mut() {
                let copy: &mut Motion = copy;
                copy.pilot = Some(pilot);
                step(copy, &world, &body, &[], tick);
                assert_eq!(motion_bytes(&m), motion_bytes(copy),
                    "{kind:?} restore tick {tick}");
            } else if tick == 15 {
                let mut copy = Motion::new([1024,16,1024],kind,&body);
                assert!(copy.load(&mut Scan::new(&motion_bytes(&m))));
                restored = Some(copy);
            }
            if tick < 80 { accelerating = accelerating.max(m.hull.up.x.neg()); }
            if (80..170).contains(&tick) { braking = braking.max(m.hull.up.x); }
            jump = jump.max(m.hull.up.sub(last).len_sq().sqrt());
            last = m.hull.up;
            assert!(m.hull.up.y > Fx::from_ratio(99,100), "{kind:?} excessive lean at {tick}");
        }
        println!("body {kind:?} accel={accelerating:?} brake={braking:?} max_step={jump:?} final={:?}",m.hull.up);
        assert!(accelerating > Fx::from_ratio(1,10000), "{kind:?} body ignores acceleration");
        assert!(braking > Fx::from_ratio(1,10000), "{kind:?} body ignores braking");
        assert!(jump < Fx::from_ratio(1,50), "{kind:?} sudden body rotation {jump:?}");
        assert!(m.hull.up.x.abs().max(m.hull.up.z.abs()) < Fx::from_ratio(1,10000),
            "{kind:?} body did not settle: {:?}",m.hull.up);
        assert!(m.feet.iter().all(|f| f.planted()), "{kind:?} stranded a foot");
    }
}

fn step(m: &mut Motion, w: &World, b: &UnitBody, others: &[Obstacle], tick: u32) {
    m.step(w,b,true,true,crate::mobility::Mobility::intact(true),others,tick,98,
        &nav::vehicle::Rubble::new(),&nav::vehicle::Changed::default());
}
fn motion_bytes(m: &Motion) -> Vec<u8> {
    let mut b = Blob::new(); m.save(&mut b); b.v
}

#[test]
fn biped_body_uses_sideways_route_motion_instead_of_its_torso_heading() {
    let world = World::new(2048,512,2048,65536,Box::new(Flat));
    for kind in [RangeUnitKind::Commander,RangeUnitKind::Walker04] {
        let body = if kind == RangeUnitKind::Commander {voxel::unit::commander()} else {voxel::unit::walker04()};
        let mut m = Motion::new([1024,16,1024],kind,&body);
        m.drive.head = angle::QUARTER;
        m.enabled = true;
        m.destination = Some([1400,1024]);
        m.path = vec![[1400,1024]];
        let mut x_lean=Fx::ZERO;
        let mut z_lean=Fx::ZERO;
        for tick in 0..20 {
            step(&mut m,&world,&body,&[],tick);
            x_lean=x_lean.max(m.hull.up.x.abs());
            z_lean=z_lean.max(m.hull.up.z.abs());
        }
        assert!(m.pos[0]>Fx::from_int(1024));
        assert!(x_lean>Fx::from_ratio(1,10000),"{kind:?} has no sideways body response");
        assert!(x_lean>z_lean.mul(Fx::from_int(2)),"{kind:?} leaned along torso: {x_lean:?}/{z_lean:?}");
    }
}
