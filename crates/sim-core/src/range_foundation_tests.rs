use super::*;
use phys::V3;

fn scene(kind: RangeUnitKind, battle: bool, c: ExplosionConfig) -> BlastRange {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    l.course = None;
    l.blocks.clear();
    l.clear_min = [64, 64];
    l.clear_max = [600, 600];
    l.battle = battle;
    l.units = vec![modding::range::RangeUnitDef {
        kind,
        allied: false,
        at: [320, FLOOR, 320],
        weapon: None,
    }];
    let probe = RangeUnit::of_kind(kind, l.units[0].at);
    let (lo, hi) = probe.bounds();
    if (0..3).any(|a| lo[a] < 1 || hi[a] >= l.size[a] as i32 - 1) {
        l.size = [4096, 2048, 4096];
        l.units[0].at = [1600, FLOOR, 1600];
        l.clear_max = [4032, 4032];
    }
    let mut r = BlastRange::with_layout(c, l).unwrap();
    r.control();
    r
}
fn shell() -> ExplosionConfig {
    ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap()
}
fn stationary() -> impl Iterator<Item = RangeUnitKind> {
    // Keep the legacy offset/excavation matrix at its original scale.
    // The new 140 m logistics building needs separate bounded probes below.
    RangeUnitKind::ALL
        .into_iter()
        .take(14)
        .filter(|k| k.emplacement())
}
fn burst(r: &mut BlastRange, at: [i32; 3]) {
    let c = r.config;
    let wave = Explosion::begin_impact(
        &r.world,
        at,
        [at[0], at[1] + 1, at[2]],
        &c,
        &r.units,
        r.matter.tuning.body_energy_pct,
    );
    if r.battle_mode() {
        r.battle.shell_config = c;
        r.battle.waves.push(Burst {
            weapon: Weapon::Blast,
            wave,
            energy: u64::from(c.material_energy_j) * 1000,
            side: None,
        });
    } else {
        r.matter.begin(c.material_energy_j, c.fragment_box);
        r.explosion = Some(wave);
    }
}

#[test]
fn new_emplacements_have_complete_footings_and_hold_their_foundation() {
    for kind in RangeUnitKind::ALL
        .into_iter()
        .skip(14)
        .filter(|k| k.emplacement())
    {
        let mut r = scene(kind, true, shell());
        let f = foundation::blocks(r.layout.as_ref().unwrap())
            .pop()
            .unwrap();
        let [cx, _, cz] = r.units[0].at;
        for (x, z) in [(f.min[0], f.min[2]), (cx, cz), (f.max[0] - 1, f.max[2] - 1)] {
            assert_eq!(
                r.world.material(x as u32, (FLOOR - 1) as u32, z as u32),
                2,
                "{kind:?}"
            );
        }
        let before = r.units[0].motion.pos;
        r.units[0].motion.hull.vy = Fx::from_int(10);
        for _ in 0..3 {
            r.step();
        }
        assert_eq!(r.units[0].motion.pos, before, "{kind:?}");
        assert_eq!(r.units[0].motion.hull.vy, Fx::ZERO);
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[test]
fn foundations_cover_every_emplacement_and_both_generator_paths() {
    for kind in stationary() {
        let r = scene(kind, true, shell());
        let [cx, _, cz] = r.units[0].at;
        let l = r.layout.as_ref().unwrap();
        let f = foundation::blocks(l).pop().unwrap();
        let g = RangeTerrain::new(true, Some(l.clone()));
        let mut column = vec![0; 32];
        for (x, z) in [(f.min[0], f.min[2]), (cx, cz), (f.max[0] - 1, f.max[2] - 1)] {
            g.column(x as u32, z as u32, 0, &mut column);
            for y in 0..32 {
                assert_eq!(column[y as usize], g.material(x as u32, y, z as u32));
                if y as i32 >= f.min[1] && (y as i32) < f.max[1] {
                    assert_eq!(
                        column[y as usize],
                        foundation::tuning().material,
                        "{kind:?}"
                    );
                }
            }
        }
        assert_eq!(r.world.material(cx as u32, 15, cz as u32), 2);
        assert_eq!(r.world.material(cx as u32, 3, cz as u32), 1);
    }
    let r = scene(RangeUnitKind::Tank, true, shell());
    assert!(foundation::blocks(r.layout.as_ref().unwrap()).is_empty());
    assert_ne!(r.world.material(320, 15, 320), 2);
}

#[test]
fn the_stationary_test_gun_has_a_footing_but_a_hidden_battle_gun_does_not() {
    let r = scene(RangeUnitKind::Tank, false, shell());
    let mut l = r.layout.unwrap();
    l.gun_kind = RangeUnitKind::TurretT2;
    let manual = BlastRange::with_layout(shell(), l.clone()).unwrap();
    let [x, y, z] = l.gun;
    assert_eq!(manual.world.material(x as u32, (y - 1) as u32, z as u32), 2);
    l.battle = true;
    let battle = BlastRange::with_layout(shell(), l).unwrap();
    assert_ne!(battle.world.material(x as u32, (y - 1) as u32, z as u32), 2);
}

#[test]
fn rubble_cannot_lift_an_attached_building_and_zero_speed_does_not_anchor_a_tank() {
    for kind in stationary() {
        let mut r = scene(kind, true, shell());
        let pos = r.units[0].motion.pos;
        let [cx, _, cz] = r.units[0].at;
        let mut rubble = nav::vehicle::Rubble::from_sorted(
            (cx - 70..cx + 70)
                .flat_map(|x| (cz - 70..cz + 70).map(move |z| ([x, z], 40)))
                .collect(),
        );
        for enabled in [false, true] {
            let u = &mut r.units[0];
            u.motion.enabled = enabled;
            u.motion.step(
                &r.world,
                &u.body,
                true,
                false,
                crate::mobility::Mobility::intact(false),
                &[],
                0,
                98,
                &rubble,
                &nav::vehicle::Changed::everywhere(),
            );
            assert_eq!(u.motion.pos, pos, "{kind:?}");
            assert_eq!(u.motion.hull.up, V3::UP);
        }
        rubble = nav::vehicle::Rubble::new();
        let body = voxel::unit::tank();
        let mut tank = motion::Motion::new([320, FLOOR, 320], RangeUnitKind::Tank, &body);
        tank.speed = 0;
        tank.hull.vy = Fx::from_int(10);
        tank.passive(
            &r.world,
            &rubble,
            &body,
            98,
            crate::mobility::Mobility::intact(true),
            &[],
        );
        assert!(tank.pos[1] > pos[1], "zero-speed mobile tank was attached");
    }
}

#[test]
fn ordinary_ground_blast_keeps_all_buildings_rigid_in_both_modes() {
    for kind in stationary() {
        for battle in [false, true] {
            for offset in [
                [0, 0],
                [25, 0],
                [-25, 0],
                [0, 18],
                [0, -18],
                [25, 18],
                [-25, -18],
                [40, 0],
                [-40, 0],
            ] {
                let mut r = scene(kind, battle, shell());
                let pos = r.units[0].motion.pos;
                let [cx, _, cz] = r.units[0].at;
                burst(&mut r, [cx + offset[0], 15, cz + offset[1]]);
                for _ in 0..90 {
                    r.step();
                    assert_eq!(
                        r.units[0].motion.pos, pos,
                        "{kind:?} battle={battle} offset={offset:?}"
                    );
                    assert_eq!(r.units[0].motion.hull.vy, Fx::ZERO);
                    assert_eq!(r.units[0].motion.hull.up, V3::UP);
                    assert_eq!(r.units[0].motion.hull.w, V3::ZERO);
                    assert!(r.matter.economy.ledger.balanced());
                }
                assert!(
                    r.removed_materials[2] > 0,
                    "test must damage concrete: {kind:?} offset={offset:?}"
                );
            }
        }
    }
}

#[test]
fn heavy_ground_blast_can_destroy_a_turrets_footing() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/siege-shell.ron")).unwrap();
    // The actual T3 charge, without a class-specific concrete bypass.
    c.validate().unwrap();
    let mut r = scene(RangeUnitKind::TurretT1, true, c);
    let before = r.units[0].motion.pos[1];
    burst(&mut r, [320, 15, 320]);
    let mut min = before;
    for _ in 0..120 {
        r.step();
        min = min.min(r.units[0].motion.pos[1]);
        assert!(r.matter.economy.ledger.balanced());
    }
    assert!(r.removed_materials[2] > 0);
    assert!(
        min < before.sub(Fx::ONE),
        "heavy blast left turret anchored: {min:?}"
    );
}

#[test]
fn undermined_footings_fall_and_replay_across_workers() {
    for kind in stationary() {
        let mut r = scene(kind, true, shell());
        let f = foundation::blocks(r.layout.as_ref().unwrap())
            .pop()
            .unwrap();
        for x in f.min[0]..f.max[0] {
            for z in f.min[2]..f.max[2] {
                for y in 8..FLOOR {
                    r.world.queue_dig(x as u32, y as u32, z as u32);
                    if r.world.queued() >= 8192 {
                        r.world.flush();
                    }
                }
            }
        }
        while r.world.queued() > 0 {
            r.world.flush();
        }
        let bytes = r.snapshot();
        let mut copy = BlastRange::restore(r.config, &bytes).unwrap();
        assert_eq!(copy.snapshot(), bytes);
        r.set_physics_workers(1);
        copy.set_physics_workers(4);
        for _ in 0..90 {
            r.step();
            copy.step();
            assert_eq!(r.state_hash(), copy.state_hash(), "{kind:?}");
        }
        assert!(
            r.units[0].motion.pos[1] < Fx::from_int(10),
            "{kind:?} floats"
        );
    }
}

#[test]
fn warm_reset_rebuilds_moved_and_removed_footings() {
    let mut r = scene(RangeUnitKind::TurretT1, true, shell());
    let mut l = r.layout.clone().unwrap();
    l.units[0].at[0] += 100;
    assert!(!r.same_authored_terrain(&l));
    let mut warm = r
        .rebuilt_with_layout(shell(), l.clone(), &placement::Validator::default())
        .unwrap();
    let fresh = BlastRange::with_layout(shell(), l).unwrap();
    assert_eq!(warm.state_hash(), fresh.state_hash());
    assert_eq!(warm.world.material(420, 15, 320), 2);
    assert_ne!(warm.world.material(320, 15, 320), 2);
    let mut l = warm.layout.clone().unwrap();
    l.units[0].kind = RangeUnitKind::Tank;
    assert!(!warm.same_authored_terrain(&l));
    let warm = warm
        .rebuilt_with_layout(shell(), l.clone(), &placement::Validator::default())
        .unwrap();
    assert_eq!(
        warm.state_hash(),
        BlastRange::with_layout(shell(), l).unwrap().state_hash()
    );
}
