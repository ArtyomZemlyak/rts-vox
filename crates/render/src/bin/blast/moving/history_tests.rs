use super::*;
use fx::Fx;
use sim_core::{burst::Fragment, damage::V3};

fn piece(id: u64, x: i32) -> Fragment {
    Fragment {
        id,
        position: V3::new(Fx::from_int(x), Fx::from_int(12), Fx::from_int(20)),
        velocity: V3::ZERO,
        material: 3,
        paint: None,
        mm: 100,
        basis: sim_core::burst::IDENTITY,
        mass_g: 1,
        count: 1,
        shape: 1,
        side: 1,
        chunk: None,
        held: id as u32,
        resting: true,
    }
}

#[test]
fn tick_history_keeps_resting_origins_and_replaces_removed_and_reused_ids() {
    let mut sim = BlastRange::new(config(false, false));
    let window = TerrainWindow {
        origin: [0; 3],
        dims: [64; 3],
    };
    // Existing occupancy belongs to a separate tested subsystem; this test
    // exercises the real before-tick capture and reset path.
    let mut v = Visual {
        occupied: vec![0],
        ..default()
    };
    sim.matter.debris.fragments = vec![piece(7, 70), piece(2, 20), piece(9, 90)];
    before(&sim, &mut v, &window);
    assert_eq!(v.fragments.len(), 3);
    assert_eq!(*v.fragments.get(&7).unwrap(), Vec3::new(70.0, 12.0, 20.0));
    // Wake and move a resting piece. Its old origin is still available to
    // interpolate this tick, independent of the source slice's ID order.
    sim.matter.debris.fragments[0].resting = false;
    sim.matter.debris.fragments[0].position.x = Fx::from_int(74);
    assert_eq!(v.fragments.get(&7).unwrap().x, 70.0);
    before(&sim, &mut v, &window);
    assert_eq!(v.fragments.get(&7).unwrap().x, 74.0);
    sim.matter.debris.fragments = vec![piece(2, 200), piece(11, 110)];
    before(&sim, &mut v, &window);
    assert_eq!(v.fragments.len(), 2);
    assert!(v.fragments.get(&7).is_none());
    assert!(v.fragments.get(&9).is_none());
    assert_eq!(v.fragments.get(&2).unwrap().x, 200.0);
    reset(&mut v);
    assert!(v.fragments.is_empty());
    assert_eq!(v.occupied, vec![0]);
    sim.matter.debris.fragments.clear();
    before(&sim, &mut v, &window);
    assert!(v.fragments.is_empty());
}
