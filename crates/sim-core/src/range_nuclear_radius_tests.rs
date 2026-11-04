//! Open-air upper bound: no terrain, shields, or finite material budget can
//! conceal an excessive pressure reach. One receiver per trial avoids cover.
use super::*;

struct Air;
impl Generator for Air {
    fn material(&self, _: u32, _: u32, _: u32) -> u16 {
        0
    }
    fn column(&self, _: u32, _: u32, _: u32, out: &mut [u16]) {
        out.fill(0);
    }
}

fn pressure_trial(
    c: &ExplosionConfig,
    kind: RangeUnitKind,
    offset: [i32; 2],
) -> (bool, u32, u32, bool, bool, u64) {
    pressure_trial_grade(c, kind, offset, false)
}

fn pressure_trial_grade(
    c: &ExplosionConfig,
    kind: RangeUnitKind,
    offset: [i32; 2],
    legacy_spider: bool,
) -> (bool, u32, u32, bool, bool, u64) {
    let at = [256 + (-offset[0]).max(0), 64, 256 + (-offset[1]).max(0)];
    let mut units = vec![RangeUnit::of_kind(
        kind,
        [at[0] + offset[0], at[1], at[2] + offset[1]],
    )];
    if legacy_spider {
        let grade = [2000, 1500, 1500, 2000, 1500];
        units[0].body.ablation_milli = grade;
        for segment in &mut units[0].body.segments {
            segment.body.ablation_milli = grade;
        }
    }
    let before = units[0].body.solid_count()
        + units[0]
            .body
            .segments
            .iter()
            .map(|s| s.body.solid_count())
            .sum::<u32>();
    let mut world = World::new(
        (512 + offset[0].abs()) as u32,
        128,
        (512 + offset[1].abs()) as u32,
        100,
        Box::new(Air),
    );
    let mut dents = SurfaceWork::new();
    let mut e = Explosion::with_damage(&world, at, c, &units, &dents);
    for _ in 0..1024 {
        if !e.working() {
            break;
        }
        e.step(
            &mut world,
            c.wave_work_per_tick,
            &mut units,
            &mut dents,
            &mut Vec::new(),
            c,
            None,
        );
    }
    assert!(!e.working(), "unfinished pressure trial");
    let u = &mut units[0];
    let status = u.body.status(&mut u.connectivity);
    (
        status.alive,
        (u.removed.len() + u.seg_removed.len()) as u32,
        before,
        status.engine,
        status.weapon,
        u64::from(u.revision),
    )
}

#[test]
#[ignore = "diagnostic distance sweep; prints an open-air damage upper bound"]
fn nuclear_radius_diagnostic() {
    let candidate =
        ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let mut stock = candidate;
    stock.energy = 1_000_000;
    stock.unit_ablation_resistance = 1;
    println!("variant,kind,distance_units,alive,removed,initial,engine,weapon");
    for (name, c) in [("before", stock), ("candidate", candidate)] {
        for kind in [
            RangeUnitKind::Tank,
            RangeUnitKind::Spider,
            RangeUnitKind::TurretT2,
            RangeUnitKind::Base,
        ] {
            for distance in [0, 400, 800, 1000, 1200, 1400, 1600, 1800, 2000, 2240] {
                let (alive, removed, initial, engine, weapon, _) =
                    pressure_trial(&c, kind, [distance, 0]);
                println!(
                    "{name},{kind:?},{distance},{alive},{removed},{initial},{engine},{weapon}"
                );
            }
        }
    }
}

#[test]
fn stronger_nuclear_wave_destroys_spider_at_extended_range() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    for distance in [400, 800, 1100] {
        let result = pressure_trial(&c, RangeUnitKind::Spider, [distance, 0]);
        assert!(!result.0, "spider survived at {distance}: {result:?}");
    }
}

#[test]
fn nuclear_spider_is_destroyed_at_twice_the_old_survival_distance() {
    let current =
        ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let mut previous = current;
    previous.energy = 2400;
    let old = pressure_trial(&previous, RangeUnitKind::Spider, [600, 0]);
    let new = pressure_trial(&current, RangeUnitKind::Spider, [1200, 0]);
    assert!(old.0 && !new.0, "old at 60m={old:?}, new at 120m={new:?}");
}

#[test]
fn nuclear_spider_damage_uses_finite_strategic_payload() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let tuning =
        modding::range_battle::Tuning::from_ron(include_str!("../../../data/range-battle.ron"))
            .unwrap();
    let at = [256, 64, 256];
    let mut units = vec![RangeUnit::of_kind(RangeUnitKind::Spider, [1036, 64, 256])];
    let mut world = World::new(1600, 128, 512, 100, Box::new(Air));
    let mut dents = SurfaceWork::new();
    let mut matter = matter::Matter::new(&units);
    let energy = tuning.nuclear.material_energy_j * 1000;
    matter.begin_mj(energy, c.fragment_box);
    let mut e = Explosion::begin_material(at, &c, &units, tuning.nuclear.body_energy_pct);
    e.material_budget(energy, tuning.nuclear.body_energy_pct);
    e.capture(&world, &c, &units, &dents, usize::MAX);
    for _ in 0..128 {
        e.step(
            &mut world,
            c.wave_work_per_tick,
            &mut units,
            &mut dents,
            &mut Vec::new(),
            &c,
            Some(&mut matter),
        );
        let u = &mut units[0];
        if !u.body.status(&mut u.connectivity).alive {
            break;
        }
    }
    let u = &mut units[0];
    assert!(
        !u.body.status(&mut u.connectivity).alive,
        "spider survived real finite-energy blast; removed={} admission={:?}",
        u.removed.len(),
        matter.admission.body
    );
    assert!(matter.remaining_mj < energy);
    assert!(matter.economy.ledger.balanced());
}

#[test]
fn nuclear_open_air_has_a_lethal_inner_zone_and_surviving_outer_receivers() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    for kind in [
        RangeUnitKind::Tank,
        RangeUnitKind::Strider,
        RangeUnitKind::Spider,
        RangeUnitKind::Artillery,
        RangeUnitKind::ArtilleryT2,
        RangeUnitKind::TurretT1,
        RangeUnitKind::TurretT2,
        RangeUnitKind::ArtilleryTurretT2,
        RangeUnitKind::ArtilleryTurretT3,
        RangeUnitKind::Base,
    ] {
        // Axis, reverse axis and 3:4 diagonals have equal Euclidean radii.
        for direction in [[5, 0], [-5, 0], [0, 5], [3, 4]] {
            for (distance, alive) in [(400, false), (800, false), (2600, true)] {
                let offset = direction.map(|v| v * distance / 5);
                let (actual, removed, _, _, _, revision) = pressure_trial(&c, kind, offset);
                assert_eq!(
                    actual, alive,
                    "{kind:?} distance={distance} direction={direction:?}"
                );
                if distance >= 2600 {
                    assert_eq!(removed, 0, "distant hull damaged: {kind:?} {offset:?}");
                }
                if distance >= 2600 {
                    assert_eq!(revision, 0, "distant hull dented: {kind:?} {offset:?}");
                }
            }
        }
    }
}

#[test]
fn spider_reinforcement_does_not_reduce_nuclear_pressure_damage() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let current = pressure_trial(&c, RangeUnitKind::Spider, [400, 0]);
    let previous = pressure_trial_grade(&c, RangeUnitKind::Spider, [400, 0], true);
    assert_eq!(
        current, previous,
        "nuclear pressure changed with the reinforced grade"
    );
}
