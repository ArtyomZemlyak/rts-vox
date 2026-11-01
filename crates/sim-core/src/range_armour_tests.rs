use super::*;

#[test]
fn electric_tank_armour_is_weaker_than_its_previous_t2_grade() {
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    c.unit_ablation_resistance = 180;
    let electric = RangeUnit::of_kind(RangeUnitKind::LightningTank, [512, 16, 512]);
    let strider = RangeUnit::of_kind(RangeUnitKind::Strider, [512, 16, 512]);
    let mut previous = electric.body.clone();
    previous.ablation_milli = [1500, 1500, 1500, 1500, 1250];
    previous.fracture_milli = [3000, 3000, 3000, 3000, 2000];
    assert_eq!(strider.body.ablation_milli, previous.ablation_milli);
    assert_eq!(strider.body.fracture_milli, previous.fracture_milli);
    let dims = [electric.body.vol.nx, electric.body.vol.ny, electric.body.vol.nz].map(usize::from);
    let p = (0..dims[0])
        .flat_map(|x| (0..dims[1]).flat_map(move |y|
            (0..dims[2]).map(move |z| [x, y, z])))
        .find(|p| electric.body.vol.get(p[0], p[1], p[2])
            && electric.body.part_at(p[0], p[1], p[2]) == voxel::Part::Armour)
        .expect("electric tank armour");
    assert!(unit_work(&electric.body, p, &c) < unit_work(&previous, p, &c));
    let current = electric.body.clone();
    let mut matter = matter::Matter::new(&[electric]);
    let (old, old_motion) = matter.body_price(&previous, p);
    let (new, new_motion) = matter.body_price(&current, p);
    assert!(new - new_motion < old - old_motion);
    assert_eq!(new_motion, old_motion);
}

#[test]
fn grades_cover_segments_and_the_unchanged_t1_template() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    for kind in RangeUnitKind::ALL {
        let u = RangeUnit::of_kind(kind, [512, 16, 512]);
        assert!(u
            .body
            .segments
            .iter()
            .all(|s| s.body.ablation_milli == u.body.ablation_milli
                && s.body.fracture_milli == u.body.fracture_milli));
        assert!(maximum_unit_work(&u.body, &c) >= role_work(&u.body, 3, &c));
        let copy = u.body.resampled(u.body.mm).unwrap();
        assert_eq!(copy.ablation_milli, u.body.ablation_milli);
        assert_eq!(copy.fracture_milli, u.body.fracture_milli);
        if matches!(
            kind,
            RangeUnitKind::Tank | RangeUnitKind::Artillery | RangeUnitKind::TurretT1
        ) {
            assert_eq!(u.body.ablation_milli, [1000; 5]);
            assert_eq!(u.body.fracture_milli, [1000; 5]);
        }
    }
    let spider = RangeUnit::of_kind(RangeUnitKind::Spider, [512, 16, 512]);
    let path = spider.body.weapon_paths()[0];
    let mut supports = 0;
    for x in path.min[0]..58 {
        for y in path.min[1]..path.max[1] {
            for z in path.min[2]..path.max[2] {
                let [x, y, z] = [x, y, z].map(usize::from);
                if spider.body.vol.get(x, y, z) {
                    assert_eq!(spider.body.part_at(x, y, z), voxel::Part::Weapon);
                    supports += 1;
                }
            }
        }
    }
    assert!(supports > 0, "breech support fixture must be real geometry");
}

#[test]
fn spider_t3_artillery_resistance_is_lower_but_remains_above_t1() {
    let mut c=ExplosionConfig::from_ron(include_str!("../../../data/siege-shell.ron")).unwrap();
    // The range installs its common unit resistance on each battle weapon.
    c.unit_ablation_resistance=180;
    let spider=RangeUnit::of_kind(RangeUnitKind::Spider,[512,16,512]);
    let mut previous=spider.body.clone();
    previous.ablation_milli=[4500,3000,2500,20000,2000];
    let mut ordinary=spider.body.clone();
    ordinary.ablation_milli=[1000;5];
    let dims=[spider.body.vol.nx,spider.body.vol.ny,spider.body.vol.nz].map(usize::from);
    for role in [voxel::Part::Armour,voxel::Part::Core,voxel::Part::Engine,voxel::Part::Weapon,voxel::Part::Sensor] {
        let p=(0..dims[0]).flat_map(|x| (0..dims[1]).flat_map(move |y| (0..dims[2]).map(move |z|[x,y,z])))
            .find(|p| spider.body.vol.get(p[0],p[1],p[2]) && spider.body.part_at(p[0],p[1],p[2])==role).expect("spider subsystem geometry");
        let now=unit_work(&spider.body,p,&c);
        let old=unit_work(&previous,p,&c);
        let t1=unit_work(&ordinary,p,&c);
        assert!(now<old && now>t1,"{role:?}: old={old}, new={now}, T1={t1}");
    }
}

#[test]
fn physical_volume_and_grade_charge_fracture_without_amplifying_fragment_velocity() {
    let u = RangeUnit::new([256, 16, 256]);
    let mut matter = matter::Matter::new(&[u]);
    let mut coupon = UnitBody::new_scaled(1, 1, 1, 1, 100);
    coupon.set(0, 0, 0, voxel::Part::Armour);
    let (base, motion) = matter.body_price(&coupon, [0; 3]);
    assert_eq!(
        (base, motion),
        crate::burst::fracture_cost_mj(500, 100, &matter.tuning)
    );
    coupon.mm = 200;
    let (large, large_motion) = matter.body_price(&coupon, [0; 3]);
    assert_eq!(large - large_motion, (base - motion) * 8);
    coupon.fracture_milli = [8000; 5];
    let (reinforced, reinforced_motion) = matter.body_price(&coupon, [0; 3]);
    assert_eq!(reinforced - reinforced_motion, (large - large_motion) * 8);
    assert_eq!(large_motion, reinforced_motion);
    // The memo must follow live material/tuning changes rather than preserving
    // the previous material price across a saved or configured world.
    matter.tuning.fracture_kj_m3_per_strength *= 2;
    let (changed, changed_motion) = matter.body_price(&coupon, [0; 3]);
    assert_eq!(
        changed - changed_motion,
        (reinforced - reinforced_motion) * 2
    );
    assert!(changed_motion >= reinforced_motion * 2);
}

#[test]
fn grade_changes_local_work_without_global_hp_or_damage_immunity() {
    let mut c =
        ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let mut u = RangeUnit::of_kind(RangeUnitKind::Spider, [512, 16, 512]);
    let core = [u.body.core.0, u.body.core.1, u.body.core.2].map(usize::from);
    let work = unit_work(&u.body, core, &c);
    assert!(
        work < c.energy,
        "T3's core must remain vulnerable to a close nuclear wave"
    );
    let other = [core[0] + 1, core[1], core[2]];
    let original = u.hit_resistance(Some((0, other)), &c);
    u.receive_cell(core, work - 1, &c);
    assert!(u.body.vol.get(core[0], core[1], core[2]));
    assert_eq!(u.hit_resistance(Some((0, core)), &c), 1);
    assert_eq!(u.hit_resistance(Some((0, other)), &c), original);
    u.receive_cell(core, 1, &c);
    assert!(!u.body.vol.get(core[0], core[1], core[2]));
    // Even saturated authoring factors respect the wave's fixed work range.
    c.unit_ablation_resistance = 1_000_000;
    c.unit_role_resistance_milli = [10000; 5];
    assert_eq!(unit_work(&u.body, other, &c), 1_000_000);
}

#[test]
fn t3_capture_bound_includes_reinforced_parts_instead_of_stopping_at_t1_work() {
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    c.half_cells = 4;
    c.energy = 100;
    c.cell_units = 4;
    let mut u = RangeUnit::new([64, 32, 64]);
    u.body = UnitBody::new_scaled(2, 1, 1, 1, 100);
    u.body.set(0, 0, 0, voxel::Part::Armour);
    u.body.set(1, 0, 0, voxel::Part::Weapon);
    u.body.ablation_milli = [2000, 1500, 1500, 2000, 1500];
    u.update_pose(0);
    struct Air;
    impl Generator for Air {
        fn material(&self, _: u32, _: u32, _: u32) -> u16 {
            0
        }
    }
    let world = World::new(128, 128, 128, 100, Box::new(Air));
    let at = u.world_pos([0; 3]);
    let units = [u];
    let e = Explosion::with_damage(&world, at, &c, &units, &SurfaceWork::new());
    let n = e.wave.size();
    let mut strongest = 0;
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let value = e.wave.resistance_at([x, y, z]);
                if value != u32::MAX {
                    strongest = strongest.max(value);
                }
            }
        }
    }
    assert_eq!(strongest, unit_work(&units[0].body, [1, 0, 0], &c));
}
