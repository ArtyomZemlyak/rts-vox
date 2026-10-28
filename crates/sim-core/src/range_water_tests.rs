use super::water::Water;
use super::*;
use voxel::Edit;

#[test]
fn water_free_flight_matches_gravity_and_preserves_a_falling_volume() {
    let w=world();
    let mut f=liquid(&w,[28,56,28],[30,59,30]);
    let volume=sum(&f);
    let mut copy=None;
    let mut previous=56;
    let mut drops=Vec::new();
    for n in 1..=20 {
        f.step(&w,&[]);
        let low=f.cells().map(|(p,_)|p[1]).min().unwrap() as i32;
        let analytic=56-98*n*n/(2*crate::TICKS_PER_SECOND*crate::TICKS_PER_SECOND);
        assert!((low-analytic).abs()<=1,"tick {n}: y={low}, ballistic y={analytic}");
        assert_eq!(sum(&f),volume);
        assert!(f.cells().all(|(p,_)|(28..30).contains(&p[0])&&(28..30).contains(&p[2])),"water must not diffuse sideways in free air");
        assert_eq!(f.cells().count(),12,"the whole finite volume must fall together");
        drops.push(previous-low);previous=low;
        if n==7 {
            let mut b=fx::blob::Blob::new();f.save(&mut b);
            let mut restored=Water::new();restored.load(&mut fx::blob::Scan::new(&b.v),[64;3]).unwrap();copy=Some(restored);
        }else if let Some(back)=&mut copy {
            back.step(&w,&[]);let mut a=fx::blob::Blob::new();let mut b=fx::blob::Blob::new();f.save(&mut a);back.save(&mut b);assert_eq!(a.v,b.v);
        }
    }
    assert!(drops[19]>drops[3],"free-flight velocity must actually accelerate");
    f.step(&w,&[]);
    assert_eq!(f.cells().map(|(p,_)|p[1]).min(),Some(4),"5.2 m free fall must reach the floor by 1.05 s");
    for _ in 0..30 {f.step(&w,&[]);assert_eq!(sum(&f),volume);}
    assert!(f.cells().all(|(p,_)|p[1]>=4),"fast water must not tunnel through the ground");
    assert!(f.cells().any(|(p,_)|p[1]==4),"water must reach the floor");
}

#[test]
fn water_free_fall_acceleration_does_not_depend_on_liquid_viscosity() {
    let w=world();let mut reference=None;
    for viscosity in [0,200,950] {
        let mut f=liquid(&w,[30,56,30],[31,57,31]);f.tuning.viscosity_milli=viscosity;
        let mut positions=Vec::new();
        for _ in 0..18 {f.step(&w,&[]);positions.push(f.cells().map(|(p,_)|p).collect::<Vec<_>>());}
        if let Some(expected)=&reference {assert_eq!(&positions,expected);}else{reference=Some(positions);}
    }
}

#[test]
fn water_free_flight_moves_a_whole_column_even_with_one_work_item_per_tick() {
    let w=world();let mut slow=liquid(&w,[30,40,30],[31,56,31]);
    let mut normal=liquid(&w,[30,40,30],[31,56,31]);slow.tuning.checks_per_tick=1;
    for _ in 0..15 {
        slow.step(&w,&[]);normal.step(&w,&[]);
        assert_eq!(slow.cells().collect::<Vec<_>>(),normal.cells().collect::<Vec<_>>());
        assert!(slow.checks<=1);
    }
}

#[test]
fn water_motion_rejects_corrupt_velocity_and_loads_legacy_volume_without_motion() {
    let w=world();let mut f=liquid(&w,[30,50,30],[31,51,31]);f.step(&w,&[]);
    let mut b=fx::blob::Blob::new();f.save_legacy(&mut b);
    let mut old=Water::new();old.load_legacy(&mut fx::blob::Scan::new(&b.v),[64;3]).unwrap();
    assert!(!old.has_motion());assert_eq!(sum(&old),sum(&f));
    let mut motion=fx::blob::Blob::new();f.save_motion(&mut motion);
    assert!(motion.v.len()>=40);
    motion.v[16..24].copy_from_slice(&(-1i64).to_le_bytes());
    assert!(old.load_motion(&mut fx::blob::Scan::new(&motion.v),[64;3]).is_none());
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
fn liquid(w: &World, min: [i32; 3], max: [i32; 3]) -> Water {
    let mut l =
        modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
            .unwrap();
    l.blocks = vec![modding::range::RangeBlock {
        min,
        max,
        material: 11,
    }];
    let mut water = Water::new();
    water.seed(w, Some(&l));
    water
}
fn sum(w: &Water) -> u64 {
    w.cells().map(|(_, v)| u64::from(v)).sum()
}
#[test]
fn water_spread_from_a_grounded_column_is_prompt_and_conservative() {
    let w=world();
    let mut f=liquid(&w,[28,4,28],[32,12,32]);
    let initial=sum(&f);
    for tick in 1..=40 {
        f.step(&w,&[]);
        assert_eq!(sum(&f),initial);
        if [10,20,40].contains(&tick) {
            let threshold=f.tuning.units_per_voxel/8;
            let reach=f.cells().filter(|(p,v)| p[1]==4 && *v>=threshold)
                .map(|(p,_)|p[0].abs_diff(30).max(p[2].abs_diff(30))).max().unwrap_or(0);
            let sum_axis = |axis:usize, low:bool| f.cells().filter(|(p,_)| if low {p[axis]<28} else {p[axis]>=32}).map(|(_,v)|u64::from(v)).sum::<u64>();
            eprintln!("tick={tick} reach={reach} cells={} active={} checks={} transfers={} outward=[{},{},{},{}]",f.stats().cells,f.stats().active,f.checks,f.transfers,sum_axis(0,true),sum_axis(0,false),sum_axis(2,true),sum_axis(2,false));
            if tick==20 {
                assert!(reach>=9,"water spread too slowly: reach={reach}");
                assert!([sum_axis(0,true),sum_axis(0,false),sum_axis(2,true),sum_axis(2,false)].into_iter().all(|v|v>initial/5),"the flow favours one direction");
            }
        }
    }
}
#[test]
fn faster_water_respects_a_wall_and_passes_through_an_opening() {
    let mut w=world();
    put(&mut w,[30,4,1],[31,24,63],2);
    let mut f=liquid(&w,[25,4,28],[29,10,32]);
    let volume=sum(&f);
    for _ in 0..30 {f.step(&w,&[]);}
    assert!(f.cells().all(|(p,_)|p[0]<30),"water crossed a closed wall");
    assert_eq!(sum(&f),volume);
    put(&mut w,[30,4,27],[31,9,33],0);
    f.observe(w.edits(),[64;3]);
    for _ in 0..45 {f.step(&w,&[]);}
    let beyond=f.cells().filter(|(p,_)|p[0]>30).map(|(_,v)|u64::from(v)).sum::<u64>();
    assert!(beyond>=u64::from(f.tuning.units_per_voxel),"water did not cross the opening");
    assert_eq!(sum(&f),volume);
}
fn basin(w: &mut World) {
    for (min, max) in [
        ([12, 4, 12], [14, 24, 42]),
        ([40, 4, 12], [42, 24, 42]),
        ([14, 4, 12], [40, 24, 14]),
        ([14, 4, 40], [40, 24, 42]),
    ] {
        put(w, min, max, 2);
    }
}

#[test]
fn water_falls_spreads_and_preserves_exact_subvoxel_volume() {
    let mut w = world();
    basin(&mut w);
    let mut f = liquid(&w, [24, 20, 24], [28, 24, 28]);
    let volume = sum(&f);
    for _ in 0..700 {
        f.step(&w, &[]);
        assert_eq!(sum(&f), volume);
    }
    assert!(f.cells().all(|(p, _)| p[1] < 8), "water stayed aloft");
    assert!(
        f.cells().any(|(p, _)| p[0] < 24 || p[0] >= 28),
        "water did not spread"
    );
    assert!(f
        .cells()
        .all(|(p, _)| p[0] >= 14 && p[0] < 40 && p[2] >= 14 && p[2] < 40));
    assert_eq!(volume, 64 * u64::from(f.tuning.units_per_voxel));
}

#[test]
fn water_pressure_equalizes_vessels_through_a_submerged_passage() {
    let mut w = world();
    basin(&mut w);
    // Divider touches both side walls; its only opening is below y=6.
    put(&mut w, [26, 6, 14], [28, 24, 40], 2);
    let mut f = liquid(&w, [14, 4, 14], [26, 12, 40]);
    let volume = sum(&f);
    for _ in 0..900 {
        f.step(&w, &[]);
    }
    assert_eq!(sum(&f), volume);
    let left: u64 = f
        .cells()
        .filter(|(p, _)| p[0] < 26)
        .map(|(_, v)| u64::from(v))
        .sum();
    let right: u64 = f
        .cells()
        .filter(|(p, _)| p[0] >= 28)
        .map(|(_, v)| u64::from(v))
        .sum();
    assert!(
        right > volume / 4,
        "pressure never filled the far reservoir: {left} {right}"
    );
    assert!(
        f.cells()
            .any(|(p, v)| p[0] > 30 && p[1] >= 7 && v > f.tuning.units_per_voxel / 2),
        "no upward pressure flow"
    );
    assert!(
        left.abs_diff(right) < volume / 4,
        "levels did not approach each other: {left} {right}"
    );
}

#[test]
fn water_intact_dam_holds_then_opening_releases_the_same_water() {
    let mut w = world();
    basin(&mut w);
    put(&mut w, [26, 4, 14], [28, 24, 40], 2);
    let mut f = liquid(&w, [14, 4, 14], [26, 10, 40]);
    let volume = sum(&f);
    for _ in 0..200 {
        f.step(&w, &[]);
    }
    assert!(
        !f.cells().any(|(p, _)| p[0] >= 28),
        "water leaked through a wall"
    );
    put(&mut w, [26, 4, 22], [28, 8, 30], 0);
    f.observe(w.edits(), [64; 3]);
    for _ in 0..700 {
        f.step(&w, &[]);
    }
    assert!(
        f.cells()
            .any(|(p, v)| p[0] > 32 && v > f.tuning.units_per_voxel / 2),
        "opening did not release water"
    );
    assert_eq!(sum(&f), volume);
}

#[test]
fn water_inserting_a_solid_displaces_instead_of_destroying_liquid() {
    let mut w = world();
    basin(&mut w);
    let mut f = liquid(&w, [14, 4, 14], [40, 8, 40]);
    let volume = sum(&f);
    put(&mut w, [24, 4, 24], [28, 10, 28], 6);
    f.observe(w.edits(), [64; 3]);
    for _ in 0..600 {
        f.step(&w, &[]);
    }
    assert_eq!(sum(&f), volume);
    assert!(
        !f.cells().any(|(p, _)| w.solid(p[0], p[1], p[2])),
        "water stayed inside the inserted stone"
    );
}

#[test]
fn water_budget_midstream_restore_and_corruption_are_checked() {
    let mut w = world();
    basin(&mut w);
    let mut f = liquid(&w, [24, 18, 24], [28, 22, 28]);
    f.tuning.checks_per_tick = 7;
    f.tuning.transfers_per_tick = 5;
    for _ in 0..40 {
        f.step(&w, &[]);
        assert!(f.checks <= 7 && f.transfers <= 5);
    }
    assert!(f.stats().transferred > 0);
    let mut b = fx::blob::Blob::new();
    f.save(&mut b);
    let mut copy = Water::new();
    copy.load(&mut fx::blob::Scan::new(&b.v), [64; 3]).unwrap();
    copy.tuning = f.tuning.clone();
    for _ in 0..160 {
        f.step(&w, &[]);
        copy.step(&w, &[]);
        let mut a = fx::blob::Blob::new();
        let mut b = fx::blob::Blob::new();
        f.save(&mut a);
        copy.save(&mut b);
        assert_eq!(a.v, b.v);
    }
    let mut b = fx::blob::Blob::new();
    f.save(&mut b);
    b.v[0] ^= 1;
    assert!(Water::new()
        .load(&mut fx::blob::Scan::new(&b.v), [64; 3])
        .is_none());
}

#[test]
fn water_wood_floats_stone_sinks_and_draining_releases_a_float() {
    let mut w = world();
    basin(&mut w);
    let mats = modding::data::materials_from_ron(include_str!("../../../data/range-materials.ron"))
        .unwrap();
    let t = crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
    for mat in [5, 6] {
        let mut ww = world();
        basin(&mut ww);
        let mut f = liquid(&ww, [14, 4, 14], [40, 14, 40]);
        put(&mut ww, [24, 20, 24], [28, 24, 28], mat);
        let cells: Vec<_> = (24..28)
            .flat_map(|z| (20..24).flat_map(move |y| (24..28).map(move |x| (x, y, z))))
            .collect();
        let mut bodies = Bodies::new(8, crate::gravity_per_tick(&t));
        assert!(bodies.detach(&mut ww, &cells));
        ww.flush_transfers();
        for _ in 0..200 {
            bodies.step_supported(&mut ww, |b, w| f.support_body(b, w, &mats));
            ww.flush_transfers();
        }
        if mat == 5 {
            assert_eq!(bodies.list.len(), 1, "wood was baked into the river bottom");
            assert!(
                (10..=12).contains(&bodies.list[0].oy),
                "wood did not float near the surface: {}",
                bodies.list[0].oy
            );
            f = Water::new();
            for _ in 0..200 {
                bodies.step_supported(&mut ww, |b, w| f.support_body(b, w, &mats));
                ww.flush_transfers();
            }
            assert!(bodies.list.is_empty(), "drained float stayed in mid-air");
        } else {
            assert!(bodies.list.is_empty(), "stone floated");
            assert_eq!(ww.material(25, 4, 25), 6);
        }
    }
}

#[test]
fn water_authoring_is_not_solid_and_shared_engine_replays_with_fluid() {
    let e = modding::laboratory::experiments::stock()
        .into_iter()
        .find(|e| e.id == "water-fall")
        .unwrap();
    let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, e.layout).unwrap();
    r.control();
    assert!(r.water.stats().units > 0);
    assert_eq!(r.world.material(384, 40, 336), 0);
    let mut column = [0; 64];
    r.world.column(384, 336, 0, &mut column);
    assert_eq!(column[40], 0);
    for _ in 0..30 {
        r.step();
    }
    let mut copy = BlastRange::restore(r.config, &r.snapshot()).expect("water checkpoint");
    let volume = r.water.stats().units;
    for _ in 0..180 {
        r.step();
        copy.step();
        assert_eq!(r.state_hash(), copy.state_hash());
    }
    assert_eq!(
        sum(&r.water) + r.water.stats().absorbed_units + r.water.stats().vapor_units,
        volume
    );
    assert_eq!(r.water.stats().units, volume);
    assert!(r.matter.economy.ledger.balanced());
    assert!(r.impact.is_none());
}

#[test]
fn water_wetting_cools_wood_and_keeps_liquid_absorbed_and_vapour_balanced() {
    let mut w = world();
    basin(&mut w);
    put(&mut w, [24, 4, 24], [28, 8, 28], 5);
    let mut f = liquid(&w, [14, 4, 14], [40, 8, 40]);
    let initial = f.stats().units;
    for _ in 0..40 {
        f.step(&w, &[]);
    }
    let p = [24, 5, 24];
    assert!(f.moisture(p) > 0, "wood never became wet");
    let mats = modding::data::materials_from_ron(include_str!("../../../data/range-materials.ron"))
        .unwrap();
    let mut h = crate::heat::Heat::default();
    h.add(&w, p, 240);
    f.cool(&w, &mut h, &mats);
    assert!(h.cell(p).heat < 120, "wet wood did not cool");
    assert!(f.stats().vapor_units > 0);
    assert_eq!(f.stats().units, initial);
    put(&mut w, [24, 5, 24], [25, 6, 25], 0);
    for _ in 0..40 {
        f.step(&w, &[]);
    }
    assert_eq!(f.moisture(p), 0);
    assert_eq!(f.stats().units, initial);
    assert_eq!(
        sum(&f) + f.stats().absorbed_units + f.stats().vapor_units,
        initial
    );
    let mut b = fx::blob::Blob::new();
    f.save(&mut b);
    let mut copy = Water::new();
    copy.load(&mut fx::blob::Scan::new(&b.v), [64; 3]).unwrap();
    for _ in 0..20 {
        f.step(&w, &[]);
        copy.step(&w, &[]);
        let mut a = fx::blob::Blob::new();
        let mut b = fx::blob::Blob::new();
        f.save(&mut a);
        copy.save(&mut b);
        assert_eq!(a.v, b.v);
    }
}

#[test]
fn water_shared_float_scene_keeps_wood_dynamic_after_displacing_its_own_water() {
    let e = modding::laboratory::experiments::stock()
        .into_iter()
        .find(|e| e.id == "water-float")
        .unwrap();
    let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, e.layout).unwrap();
    r.control();
    assert!(!r.world.solid(376, 20, 330));
    for _ in 0..300 {
        r.step();
    }
    assert!(
        r.bodies
            .list
            .iter()
            .any(|b| b.voxels.first().is_some_and(|v| v.mat == 5) && b.oy >= 23),
        "wood sank after its own water displacement"
    );
    assert!(
        r.bodies
            .list
            .iter()
            .all(|b| b.voxels.first().is_none_or(|v| v.mat != 6)),
        "stone stayed afloat"
    );
    assert_eq!(r.water.stats().units, r.water.stats().initial_units);
}
