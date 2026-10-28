use super::granular::Flow;
use super::*;
use voxel::{bodies::Bodies, Edit};

#[test]
fn dune_rockfall_authored_scene_falls_descends_and_restores_without_losing_material() {
    let e=modding::laboratory::experiments::stock().into_iter().find(|e|e.id=="dune-rockfall").unwrap();
    let config=ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r=BlastRange::with_layout(config,e.layout).unwrap();r.control();
    r.set_physics_workers(1);r.set_navigation_workers(1);
    let mut tracks:BTreeMap<usize,(i64,i64,u32)>=BTreeMap::new();
    let mut back=None;
    for tick in 0..360 {
        r.step();
        if tick==35 {assert!(!r.bodies.list.is_empty(),"snapshot must capture active descent");let mut copy=BlastRange::restore(config,&r.snapshot()).unwrap();copy.set_physics_workers(1);copy.set_navigation_workers(1);back=Some(copy);}
        else if let Some(copy)=&mut back {copy.step();assert_eq!(r.state_hash(),copy.state_hash(),"tick {tick}");}
        for b in &r.bodies.list {
            if b.voxels.iter().all(|v|v.mat==6) {
                let track=tracks.entry(b.voxels.len()).or_insert((b.ox,b.ox,0));
                track.1=track.1.max(b.ox);track.2=track.2.max(b.fallen);
            }
        }
        assert!(r.matter.economy.ledger.balanced());
    }
    let mut counts=[0;2];
    for z in 270..420 {for y in 16..110 {for x in 270..440 {
        match r.world.material(x,y,z) {6=>counts[0]+=1,10=>counts[1]+=1,_=>{}}
    }}}
    for b in &r.bodies.list {for v in &b.voxels {match v.mat {6=>counts[0]+=1,10=>counts[1]+=1,_=>{}}}}
    let expected_sand:i32= (0..28).map(|h|(128-4*h)*(112-2*h)).sum();
    println!("dune tracks={tracks:?} counts={counts:?} sand={expected_sand} flow={:?}",r.material_flow());
    assert_eq!(counts,[1224,expected_sand]);
    assert_eq!(tracks.len(),4,"four independent coherent rocks must fall");
    assert!(tracks.values().all(|&(first,last,fallen)|last-first>=6 && fallen>=12),"rocks did not descend: {tracks:?}");
    assert!(r.material_flow().0>0 && r.material_flow().1>0);
}

struct Flat;
impl Generator for Flat {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 {
        if y < 4 {
            6
        } else {
            0
        }
    }
}
fn bytes(w: &World) -> Vec<u8> {
    let mut b = fx::blob::Blob::new();
    w.save(&mut b);
    b.v
}
fn world() -> World {
    World::new(64, 64, 64, 65536, Box::new(Flat))
}
fn put(w: &mut World, min: [u32; 3], max: [u32; 3], mat: u16) {
    for z in min[2]..max[2] {
        for y in min[1]..max[1] {
            for x in min[0]..max[0] {
                w.queue_transfer(Edit::put(x, y, z, mat));
            }
        }
    }
    w.flush_transfers();
}

#[test]
fn dune_contact_respects_walls_other_bodies_and_the_shared_query_budget() {
    let mut w=world();
    for x in 24..40 {put(&mut w,[x,4,24],[x+1,(40-x).min(12),40],10);}
    put(&mut w,[28,12,28],[31,15,31],6);
    let cells:Vec<_>=(28..31).flat_map(|z|(12..15).flat_map(move|y|(28..31).map(move|x|(x,y,z)))).collect();
    let mut b=voxel::bodies::Body::from_world_cells(&w,&cells).unwrap();
    for &(x,y,z) in &cells {w.queue_transfer(Edit::dig(x,y,z));}w.flush_transfers();
    b.fallen=8;b.fresh=false;
    let mut f=Flow::new();
    let start=(b.ox,b.oy,b.oz);
    let mut checks=0;
    assert!(f.slide_body(&mut b,&mut w,&mut checks,&[]));
    assert_eq!((b.ox,b.oy,b.oz),start,"budget exhaustion must defer, not teleport or bake");
    let wall=[(99,[31,4,24],[32,24,40])];
    let mut checks=f.tuning.slope.checks_per_tick;
    assert!(!f.slide_body(&mut b,&mut w,&mut checks,&wall));
    assert_eq!((b.ox,b.oy,b.oz),start,"a downhill wall must block the entire swept hull");
    for _ in 0..8 {let mut checks=f.tuning.slope.checks_per_tick;if !f.slide_body(&mut b,&mut w,&mut checks,&[]) {assert!(b.can_fall(&w));break;}}
    assert!(b.ox>start.0&&b.oy<start.1);
    assert!(f.pushed>0,"sliding contact must disturb real sand");
    assert!(b.tilt_axis>=3 && b.tilt_angle!=0,"unsupported rock should roll");
}

#[test]
fn rolling_rock_pushes_a_chain_of_sand_without_losing_cells() {
    let mut w=world();
    put(&mut w,[24,4,26],[40,12,33],10);
    put(&mut w,[28,12,28],[31,15,31],6);
    let cells:Vec<_>=(28..31).flat_map(|z|(12..15).flat_map(move|y|(28..31).map(move|x|(x,y,z)))).collect();
    let b=voxel::bodies::Body::from_world_cells(&w,&cells).unwrap();
    for &(x,y,z) in &cells {w.queue_transfer(Edit::dig(x,y,z));}w.flush_transfers();
    let before=count(&w,10).0;
    let mut f=Flow::new(); let mut checks=f.tuning.slope.checks_per_tick;
    f.scuff(&b,&mut w,[29,11,28],&mut checks,&[]);
    assert!(f.pushed>=2,"contact should transmit movement through adjacent sand");
    assert_eq!(count(&w,10).0,before,"contact lost sand");
    assert!(f.pending()>0,"moved sand must wake normal granular flow");
}

#[test]
fn dune_contact_does_not_make_flat_beds_or_hard_ground_slide() {
    for material in [10,6] {
        let mut w=world();put(&mut w,[20,4,20],[44,12,44],material);
        put(&mut w,[28,12,28],[31,15,31],6);
        let cells:Vec<_>=(28..31).flat_map(|z|(12..15).flat_map(move|y|(28..31).map(move|x|(x,y,z)))).collect();
        let mut b=voxel::bodies::Body::from_world_cells(&w,&cells).unwrap();
        for &(x,y,z) in &cells {w.queue_transfer(Edit::dig(x,y,z));}w.flush_transfers();
        b.fallen=8;b.fresh=false;
        let mut f=Flow::new();let mut checks=f.tuning.slope.checks_per_tick;
        assert!(!f.slide_body(&mut b,&mut w,&mut checks,&[]));
        assert_eq!((b.ox,b.oy,b.oz),(28,12,28));
    }
}
fn count(w: &World, mat: u16) -> (usize, u64, usize) {
    let (mut n, mut height, mut spread) = (0, 0, 0);
    for z in 1..63 {
        for y in 4..40 {
            for x in 1..63 {
                if w.material(x, y, z) == mat {
                    n += 1;
                    height += u64::from(y);
                    if !(28..36).contains(&x) || !(28..36).contains(&z) {
                        spread += 1;
                    }
                }
            }
        }
    }
    (n, height, spread)
}
fn pile(mat: u16) -> (World, Flow) {
    let mut w = world();
    put(&mut w, [28, 4, 28], [36, 16, 36], mat);
    let mut flow = Flow::new();
    flow.observe(w.edits(), [64; 3]);
    (w, flow)
}

#[test]
fn granular_sand_spreads_settles_and_preserves_every_cell() {
    let (mut w, mut f) = pile(10);
    for _ in 0..700 {
        f.step(&mut w, |_| false);
    }
    let (n, _, spread) = count(&w, 10);
    assert_eq!(n, 768);
    assert!(spread > 100, "sand never spread: {spread}");
    assert!(f.moved > 1000);
    assert_eq!(f.pending(), 0, "pile never slept");
    let before = bytes(&w);
    f.step(&mut w, |_| false);
    assert_eq!(f.scanned, 0);
    assert_eq!(bytes(&w), before);
}

#[test]
fn granular_cohesive_soil_holds_a_steeper_bank_than_sand() {
    let mut results = Vec::new();
    for mat in [10, 1, 8] {
        let (mut w, mut f) = pile(mat);
        for _ in 0..700 {
            f.step(&mut w, |_| false);
        }
        let result = count(&w, mat);
        assert_eq!(result.0, 768);
        results.push(result);
    }
    assert!(
        results[0].2 > results[1].2 && results[1].2 > results[2].2,
        "{results:?}"
    );
    assert!(
        results[0].1 < results[1].1 && results[1].1 < results[2].1,
        "{results:?}"
    );
}

#[test]
fn granular_loss_of_a_sand_foundation_causes_a_soil_landslide() {
    let mut w = world();
    put(&mut w, [28, 4, 28], [36, 12, 36], 10);
    put(&mut w, [30, 12, 30], [34, 20, 34], 1);
    let before = count(&w, 1);
    let mut f = Flow::new();
    f.observe(w.edits(), [64; 3]);
    for _ in 0..500 {
        f.step(&mut w, |_| false);
    }
    let after = count(&w, 1);
    assert_eq!(after.0, before.0);
    assert_eq!(count(&w, 10).0, 512);
    assert!(
        after.1 + 2 * (after.0 as u64) < before.1,
        "soil did not follow its collapsing foundation: {before:?} -> {after:?}"
    );
}

#[test]
fn granular_rock_falls_and_drags_sand_without_losing_matter() {
    let mut w = world();
    put(&mut w, [24, 4, 24], [40, 8, 40], 10);
    put(&mut w, [30, 24, 30], [33, 27, 33], 6);
    let cells: Vec<_> = (30..33)
        .flat_map(|z| (24..27).flat_map(move |y| (30..33).map(move |x| (x, y, z))))
        .collect();
    let tuning = crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
    let mut bodies = Bodies::new(8, crate::gravity_per_tick(&tuning));
    assert!(bodies.detach(&mut w, &cells));
    w.flush_transfers();
    let mats = modding::data::materials_from_ron(include_str!("../../../data/range-materials.ron"))
        .unwrap();
    let mut f = Flow::new();
    for tick in 0..300 {
        f.impact(&mut w, &bodies, &mats, tick, |_| false);
        bodies.step(&mut w);
        w.flush_transfers();
        f.step(&mut w, |_| false);
    }
    assert!(f.pushed > 0, "falling rock did not displace sand");
    assert!(f.moved > 0, "displaced sand did not flow");
    assert_eq!(count(&w, 10).0, 1024);
    assert!(bodies.list.is_empty());
    assert_eq!(
        w.material(31, 7, 31),
        6,
        "rock did not penetrate the sand surface"
    );
    let rock = (1..63)
        .flat_map(|z| (4..40).flat_map(move |y| (1..63).map(move |x| (x, y, z))))
        .filter(|&(x, y, z)| w.material(x, y, z) == 6)
        .count();
    assert_eq!(rock, 27);
}

#[test]
fn granular_flow_budget_and_mid_patch_restore_preserve_continuation() {
    let (mut w, mut f) = pile(10);
    f.tuning.scans_per_tick = 17;
    f.tuning.moves_per_tick = 3;
    for _ in 0..600 {
        let before = f.moved;
        f.step(&mut w, |_| false);
        assert!(f.scanned <= 17);
        assert!(f.moved - before <= 3);
        if f.moved >= 3 {
            break;
        }
    }
    assert!(
        f.moved >= 3,
        "continuation test never reached moving material"
    );
    let mut blob = fx::blob::Blob::new();
    f.save(&mut blob);
    let mut restored = Flow::new();
    restored.tuning = f.tuning.clone();
    restored
        .load(&mut fx::blob::Scan::new(&blob.v), [64; 3])
        .unwrap();
    let mut other = world();
    assert!(other.load(&mut fx::blob::Scan::new(&bytes(&w))));
    for _ in 0..200 {
        f.step(&mut w, |_| false);
        restored.step(&mut other, |_| false);
        assert_eq!(bytes(&w), bytes(&other));
        let mut a = fx::blob::Blob::new();
        let mut b = fx::blob::Blob::new();
        f.save(&mut a);
        restored.save(&mut b);
        assert_eq!(a.v, b.v);
    }
}

#[test]
fn granular_reserved_grain_waits_without_being_stolen() {
    let mut w = world();
    put(&mut w, [30, 20, 30], [31, 21, 31], 10);
    let mut f = Flow::new();
    f.observe(w.edits(), [64; 3]);
    for _ in 0..10 {
        f.step(&mut w, |p| p == [30, 20, 30]);
    }
    assert_eq!(w.material(30, 20, 30), 10);
    for _ in 0..120 {
        f.step(&mut w, |_| false);
    }
    assert_eq!(count(&w, 10).0, 1);
    assert_eq!(w.material(30, 4, 30), 10);
}

#[test]
fn granular_mixed_lab_scene_replays_and_balances_the_shared_engine() {
    let experiment = modding::laboratory::experiments::stock()
        .into_iter()
        .find(|e| e.id == "rock-sand")
        .unwrap();
    let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, experiment.layout).unwrap();
    r.control();
    let mut fell = false;
    for _ in 0..25 {
        r.step();
        fell |= r.bodies.list.iter().any(|b| b.fallen > 0);
    }
    let mut copy = BlastRange::restore(r.config, &r.snapshot()).expect("flow snapshot");
    for _ in 0..220 {
        r.step();
        copy.step();
        assert_eq!(r.state_hash(), copy.state_hash());
        fell |= r.bodies.list.iter().any(|b| b.fallen > 0);
    }
    assert!(fell, "the mixed scene's rock never fell");
    assert!(r.material_flow().0 > 0);
    assert!(r.material_flow().1 > 0, "rock did not entrain sand");
    assert!(r.matter.economy.ledger.balanced());
    assert!(
        r.impact.is_none(),
        "the causal chain must not use an invisible explosion"
    );
    let mut sand = 0;
    let mut rock = 0;
    for z in 300..372 {
        for y in 16..70 {
            for x in 348..420 {
                match r.world.material(x, y, z) {
                    10 => sand += 1,
                    6 => rock += 1,
                    _ => {}
                }
            }
        }
    }
    assert_eq!(sand, 4192, "shared collapse consumed flowing sand");
    assert_eq!(rock, 64, "shared collapse consumed falling stone");
}

#[test]
fn granular_free_sand_and_soil_accelerate_with_the_same_gravity() {
    let mut trajectories = Vec::new();
    for mat in [10, 1, 8] {
        let mut w = world();
        put(&mut w, [30, 36, 30], [31, 37, 31], mat);
        let mut f = Flow::new();
        f.observe(w.edits(), [64; 3]);
        for _ in 0..20 {
            f.step(&mut w, |_| false);
            if f.has_motion() { break; }
        }
        assert!(f.has_motion());
        let mut y = Vec::new();
        for _ in 0..9 {
            f.step(&mut w, |_| false);
            y.push((4..=36).find(|&y| w.material(30, y, 30) == mat).unwrap());
        }
        // Ten 50ms semi-implicit steps: 98 * (1+...+10) / 400 = 13.475 cells.
        assert_eq!(y.last(), Some(&23));
        assert!(y[0] - y[2] < y[6] - y[8], "fall did not accelerate: {y:?}");
        trajectories.push(y);
        for _ in 0..30 { f.step(&mut w, |_| false); }
        assert_eq!(w.material(30, 4, 30), mat);
        assert_eq!(count(&w, mat).0, 1);
    }
    assert!(trajectories.windows(2).all(|v| v[0] == v[1]));
}

#[test]
fn granular_airborne_columns_fall_together_with_a_stone_instead_of_hanging() {
    let mut w = world();
    for (x, mat) in [(24, 10), (32, 1), (44, 6)] {
        put(&mut w, [x, 24, 30], [x + 3, 40, 33], mat);
    }
    let tuning = crate::test_tuning();
    let mut bodies = Bodies::new(8, crate::gravity_per_tick(&tuning));
    let cells: Vec<_> = (30..33).flat_map(|z| (24..40).flat_map(move |y| (44..47).map(move |x| (x, y, z)))).collect();
    assert!(bodies.detach(&mut w, &cells));
    w.flush_transfers();
    bodies.list[0].fresh = false;
    let mut f = Flow::new();
    f.observe(w.edits(), [64; 3]);
    for tick in 1..=10 {
        f.step(&mut w, |_| false);
        bodies.step(&mut w);
        w.flush_transfers();
        let stone_y = bodies.list[0].oy as u32;
        for (x, mat) in [(24, 10), (32, 1)] {
            for y in stone_y..stone_y + 16 {
                assert_eq!(w.material(x + 1, y, 31), mat,
                    "airborne column lagged behind stone: tick={tick}, material={mat}, y={y}, stone={stone_y}");
            }
            assert_eq!(count(&w, mat).0, 144);
        }
    }
}

#[test]
fn granular_mixed_falling_columns_preserve_layers_and_stop_at_reserved_floor() {
    let mut w = world();
    put(&mut w, [28, 24, 30], [31, 32, 33], 10);
    put(&mut w, [28, 32, 30], [31, 40, 33], 1);
    let mut f = Flow::new();
    f.observe(w.edits(), [64; 3]);
    let mut speed = Fx::ZERO;
    let mut height = Fx::ZERO;
    for _ in 0..8 {
        f.step(&mut w, |p| p[1] == 12);
        speed = speed.add(crate::gravity_per_tick(&crate::test_tuning()));
        height = height.add(speed);
        let distance = height.to_int_trunc() as u32;
        for y in 24..40 {
            assert_eq!(w.material(29, y - distance, 31), if y < 32 { 10 } else { 1 });
        }
        assert_eq!(count(&w, 10).0, 72);
        assert_eq!(count(&w, 1).0, 72);
    }
    for _ in 0..100 { f.step(&mut w, |p| p[1] == 12); }
    for z in 1..63 { for x in 1..63 { for y in 4..=12 {
        assert!(!matches!(w.material(x, y, z), 1 | 10));
    } } }
    assert_eq!(count(&w, 10).0, 72);
    assert_eq!(count(&w, 1).0, 72);
}

#[test]
fn granular_column_absorbs_old_grain_queue_and_restores_after_losing_support() {
    let mut w = world();
    put(&mut w, [28, 23, 30], [31, 24, 33], 6);
    put(&mut w, [28, 24, 30], [31, 40, 33], 10);
    let mut f = Flow::new();
    f.observe(w.edits(), [64; 3]);
    for _ in 0..2 { f.step(&mut w, |_| false); }
    assert!(f.has_motion(), "edges must already have individual motion entries");
    put(&mut w, [28, 23, 30], [31, 24, 33], 0);
    f.observe(w.edits(), [64; 3]);
    f.tuning.motion.active_per_tick = 1;
    for _ in 0..40 {
        f.step(&mut w, |_| false);
        let mut saved = fx::blob::Blob::new();
        f.save(&mut saved);
        let mut copy = Flow::new();
        copy.tuning = f.tuning.clone();
        copy.load(&mut fx::blob::Scan::new(&saved.v), [64; 3]).expect("column queue snapshot");
        let mut other = world();
        assert!(other.load(&mut fx::blob::Scan::new(&bytes(&w))));
        copy.step(&mut other, |_| false);
        f.step(&mut w, |_| false);
        assert_eq!(bytes(&w), bytes(&other));
        assert_eq!(count(&w, 10).0, 144);
    }
}

#[test]
fn granular_fast_fall_sweeps_reserved_cells_and_solid_floors() {
    for reserved in [false, true] {
        let mut w = world();
        if !reserved { put(&mut w, [1, 20, 1], [63, 21, 63], 6); }
        put(&mut w, [30, 36, 30], [31, 37, 31], 10);
        let mut f = Flow::new();
        f.observe(w.edits(), [64; 3]);
        for _ in 0..100 {
            f.step(&mut w, |p| reserved && p[1] == 20);
            assert!((4..20).all(|y| w.material(30, y, 30) != 10));
        }
        assert_eq!(w.material(30, 21, 30), 10);
        assert_eq!(count(&w, 10).0, 1);
    }
}

#[test]
fn granular_path_budget_and_active_capacity_remain_bounded_and_fair() {
    let (mut w, mut f) = pile(10);
    f.tuning.motion.active_cells = 16;
    f.tuning.motion.active_per_tick = 4;
    f.tuning.motion.path_checks_per_tick = 128;
    let before = count(&w, 10);
    for _ in 0..1500 {
        let moved = f.moved;
        f.step(&mut w, |_| false);
        assert!(f.path_checks <= 128);
        assert!(f.scanned <= f.tuning.scans_per_tick);
        assert!(f.moved - moved <= u64::from(f.tuning.moves_per_tick));
    }
    let after = count(&w, 10);
    assert_eq!(after.0, before.0);
    assert!(after.1 < before.1 && after.2 > 100, "queue starved: {after:?}");
}

#[test]
fn granular_accelerating_shared_scene_restores_velocity_and_rejects_corrupt_motion() {
    let e = modding::laboratory::experiments::stock().into_iter()
        .find(|e| e.id == "sand-flow").unwrap();
    let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, e.layout).unwrap();
    r.control();
    for _ in 0..8 { r.step(); }
    assert!(r.granular.has_motion());
    let snapshot = r.snapshot();
    assert_eq!(u32::from_le_bytes(snapshot[8..12].try_into().unwrap()), 71);
    let mut restored = BlastRange::restore(c, &snapshot).unwrap();
    for _ in 0..60 {
        r.step(); restored.step();
        assert_eq!(r.state_hash(), restored.state_hash());
    }
    let mut motion = fx::blob::Blob::new();
    r.granular.save_motion(&mut motion);
    motion.v[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Flow::new().load_motion(&mut fx::blob::Scan::new(&motion.v), r.size).is_none());
    // The old granular wire section remains independently readable.
    let mut legacy = fx::blob::Blob::new();
    r.granular.save_legacy(&mut legacy);
    let mut scan = fx::blob::Scan::new(&legacy.v);
    assert!(Flow::new().load_legacy(&mut scan, r.size).is_some());
    assert_eq!(scan.left(), 0);
}

#[test]
fn granular_same_contact_moves_sand_but_is_held_by_cohesive_soil() {
    let mats = modding::data::materials_from_ron(include_str!("../../../data/range-materials.ron"))
        .unwrap();
    let tuning = crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
    for mat in [10, 1] {
        let mut w = world();
        put(&mut w, [24, 4, 24], [40, 8, 40], mat);
        put(&mut w, [30, 9, 30], [33, 12, 33], 6);
        let cells: Vec<_> = (30..33)
            .flat_map(|z| (9..12).flat_map(move |y| (30..33).map(move |x| (x, y, z))))
            .collect();
        let mut bodies = Bodies::new(8, crate::gravity_per_tick(&tuning));
        assert!(bodies.detach(&mut w, &cells));
        w.flush_transfers();
        bodies.list[0].fresh = false;
        bodies.list[0].fallen = 1;
        bodies.list[0].oy = 8;
        let mut flow = Flow::new();
        flow.impact(&mut w, &bodies, &mats, 0, |_| false);
        assert_eq!(
            flow.pushed > 0,
            mat == 10,
            "contact ignored material yield: {mat}"
        );
        assert_eq!(count(&w, mat).0, 1024);
    }
}

#[test]
fn rolling_body_descends_and_lands_with_all_its_stone_and_sand() {
    let mut w=world();
    for x in 24..40 {put(&mut w,[x,4,24],[x+1,(40-x).min(12),40],10);}
    put(&mut w,[28,12,28],[31,15,31],6);
    let cells:Vec<_>=(28..31).flat_map(|z|(12..15).flat_map(move|y|(28..31).map(move|x|(x,y,z)))).collect();
    let mut b=voxel::bodies::Body::from_world_cells(&w,&cells).unwrap();
    for &(x,y,z) in &cells {w.queue_transfer(Edit::dig(x,y,z));}w.flush_transfers();
    b.fallen=8;b.fresh=false;
    let original_sand=count(&w,10).0;
    let original_stone=count(&w,6).0+cells.len();
    let mut bodies=Bodies::new(8,Fx::from_ratio(245,1000));bodies.list.push(b);
    let mut f=Flow::new();let mut rolled=false;let mut restored=false;let mut max_x=28;
    for _ in 0..100 {
        let mut checks=f.tuning.slope.checks_per_tick;
        bodies.step_contact(&mut w,|_,_|false,|body,world|f.slide_body(body,world,&mut checks,&[]));
        w.flush_transfers();
        for b in &bodies.list {rolled |= b.tilt_axis>=3 && b.tilt_angle!=0;max_x=max_x.max(b.ox);}
        if rolled && !restored {
            let mut data=fx::blob::Blob::new();bodies.save(&mut data);
            let mut copy=Bodies::new(8,Fx::from_ratio(245,1000));
            assert!(copy.load(&mut fx::blob::Scan::new(&data.v)));
            let a=&bodies.list[0];let b=&copy.list[0];
            assert_eq!((a.tilt_axis,a.tilt_angle,a.ox,a.oy,a.oz),(b.tilt_axis,b.tilt_angle,b.ox,b.oy,b.oz));
            for &v in &a.voxels {assert_eq!(a.world_of(v),b.world_of(v));}
            restored=true;
        }
        if bodies.list.is_empty(){break;}
    }
    assert!(rolled && restored && max_x>28,"the unsupported body should roll downhill and restore");
    let stone_in_bodies:usize=bodies.list.iter().map(|b|b.voxels.iter().filter(|v|v.mat==6).count()).sum();
    assert_eq!(count(&w,6).0+stone_in_bodies,original_stone);
    assert_eq!(count(&w,10).0,original_sand);
}

#[test]
fn small_dune_world_rolls_a_falling_rock_and_replays() {
    use modding::range::RangeBlock;
    let e=modding::laboratory::experiments::stock().into_iter().find(|e|e.id=="dune-rockfall").unwrap();
    let mut l=e.layout;l.size=[128,128,128];l.clear_min=[1,1];l.clear_max=[127,127];
    l.gun=[32,24,64];l.target=[64,16,64];l.blocks.clear();
    for h in 0..12 {
        l.blocks.push(RangeBlock {min:[40+2*h,16+h,48],max:[88-h,17+h,80],material:10});
    }
    l.blocks.push(RangeBlock {min:[75,37,60],max:[78,40,63],material:6});
    let config=ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r=BlastRange::with_layout(config,l).unwrap();r.control();r.set_physics_workers(1);r.set_navigation_workers(1);
    let mut rotated=false;let mut moved=false;let mut replay=false;
    for _ in 0..240 {
        r.step();
        for b in &r.bodies.list {
            if b.voxels.iter().all(|v|v.mat==6) {
                rotated |= b.tilt_axis>=3 && b.tilt_angle!=0;
                moved |= b.ox!=75;
            }
        }
        if rotated && !replay {
            let mut copy=BlastRange::restore(config,&r.snapshot()).unwrap();
            copy.set_physics_workers(1);copy.set_navigation_workers(1);
            for _ in 0..5 {r.step();copy.step();assert_eq!(r.state_hash(),copy.state_hash());}
            replay=true;
        }
        assert!(r.matter.economy.ledger.balanced());
        if rotated && moved && replay {break;}
    }
    assert!(rotated && moved && replay,"rock did not roll in the actual simulation: rotated={rotated}, moved={moved}");
}

#[test]
fn rocks_of_each_dune_size_roll_within_the_shared_budget() {
    for side in [3u32,5,7,9] {
        let mut w=world();
        for x in 24..44 {put(&mut w,[x,4,20],[x+1,(40u32.saturating_sub(x)).clamp(4,12),48],10);}
        put(&mut w,[28,12,28],[28+side,12+side,28+side],6);
        let cells:Vec<_>=(28..28+side).flat_map(|z|(12..12+side).flat_map(move|y|(28..28+side).map(move|x|(x,y,z)))).collect();
        let mut b=voxel::bodies::Body::from_world_cells(&w,&cells).unwrap();
        for &(x,y,z) in &cells {w.queue_transfer(Edit::dig(x,y,z));}w.flush_transfers();
        b.fallen=8;b.fresh=false;
        let mut bodies=Bodies::new(8,Fx::from_ratio(245,1000));bodies.list.push(b);
        let mut f=Flow::new();let mut rolled=false;
        for _ in 0..80 {
            let mut checks=f.tuning.slope.checks_per_tick;
            bodies.step_contact(&mut w,|_,_|false,|body,world|f.slide_body(body,world,&mut checks,&[]));
            w.flush_transfers();
            rolled |= bodies.list.iter().any(|b|b.tilt_axis>=3 && b.tilt_angle!=0);
            if rolled || bodies.list.is_empty() {break;}
        }
        assert!(rolled,"rock of {side} cells did not roll");
    }
}
