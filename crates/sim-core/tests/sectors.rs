//! Real shots and replay through every authored sector, cropped to keep CI memory bounded.
use modding::range::RangeLayout;
use sim_core::blast::{placement::Validator, BlastRange};

fn layout() -> RangeLayout {
    RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap()
}
fn crop(mut r: RangeLayout, id: usize) -> RangeLayout {
    let zone = r.zones[id].clone();
    let shift = [zone.min[0] - 80, 0, zone.min[1] - 80];
    let at = |p: [i32; 3]| std::array::from_fn(|a| p[a] - shift[a]);
    let xz = |p: [i32; 2]| [p[0] - shift[0], p[1] - shift[2]];
    let units: Vec<_> = r
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| zone.contains([u.at[0], u.at[2]], 0))
        .map(|(i, _)| i)
        .collect();
    r.units = units
        .iter()
        .map(|&i| {
            let mut u = r.units[i].clone();
            u.at = at(u.at);
            u
        })
        .collect();
    r.blocks.retain(|b| zone.contains([b.min[0], b.min[2]], 0));
    // Inter-sector roads belong to the full kilometre map, not this small
    // cropped fixture for one scripted shot.
    r.roads.clear();
    for b in &mut r.blocks {
        b.min = at(b.min);
        b.max = at(b.max);
    }
    r.shields.retain(|s| zone.contains([s.at[0], s.at[2]], 0));
    for s in &mut r.shields {
        s.at = at(s.at);
    }
    let c = r.course.as_mut().unwrap();
    c.min = xz(zone.min);
    c.max = xz(zone.max);
    c.ramps.retain(|b| zone.contains([b.min[0], b.min[2]], 0));
    for b in &mut c.ramps {
        b.min = at(b.min);
        b.max = at(b.max);
    }
    r.mounds.retain(|m| zone.contains([m.min[0], m.min[2]], 0));
    for m in &mut r.mounds {
        m.min = at(m.min);
        m.max = at(m.max);
    }
    c.patrols.retain(|p| units.contains(&p.unit));
    for p in &mut c.patrols {
        p.unit = units.iter().position(|&i| i == p.unit).unwrap();
        for point in &mut p.points {
            *point = xz(*point);
        }
    }
    r.gun = at(zone.spawn);
    r.target = at(zone.target);
    r.clear_min = xz(zone.min);
    r.clear_max = xz(zone.max);
    // Larger exploration sectors keep the same 80-cell margin as the original trials.
    r.size = [
        ((zone.max[0] - zone.min[0] + 160) as u32)
            .max(1024)
            .div_ceil(32)
            * 32,
        192,
        ((zone.max[1] - zone.min[1] + 160) as u32)
            .max(1024)
            .div_ceil(32)
            * 32,
    ];
    r.zones = vec![modding::range::RangeZone {
        min: xz(zone.min),
        max: xz(zone.max),
        spawn: r.gun,
        target: r.target,
        scenario: None,
        ..zone
    }];
    r
}
#[test]
fn every_authored_shooter_is_clear_and_the_layout_round_trips() {
    let mut r = layout();
    let v = Validator::default();
    for zone in r.zones.clone() {
        r.gun = zone.spawn;
        r.target = zone.target;
        assert_eq!(v.validate(&r), Ok(()), "{}", zone.name);
    }
    assert_eq!(RangeLayout::from_ron(&r.to_ron().unwrap()).unwrap(), r);
    let old = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    assert!(old.zones.is_empty());
    assert_eq!(old.gun, sim_core::blast::GUN);
    r.gun = r.units[0].at;
    assert!(v.validate(&r).is_err(), "custom shooter overlaps a target");
}
#[test]
fn walker_patrol_climbs_the_authored_steep_lane() {
    let base = layout();
    let id = base
        .zones
        .iter()
        .position(|z| z.name == "Лес и крутой склон")
        .unwrap();
    let r = crop(base, id);
    assert_eq!(r.units.len(), 2);
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../data/energy-cannon.ron"
    ))
    .unwrap();
    let mut sim = BlastRange::with_layout(config, r).unwrap();
    sim.patrols_running = true;
    let mut high = sim.units[1].motion.pos[1];
    let mut farthest = sim.units[1].motion.pos[0];
    for _ in 0..650 {
        sim.step();
        high = high.max(sim.units[1].motion.pos[1]);
        farthest = farthest.max(sim.units[1].motion.pos[0]);
    }
    assert!(
        high > fx::Fx::from_int(60),
        "walker never climbed: {high:?}"
    );
    assert!(
        farthest > fx::Fx::from_int(500),
        "walker did not cross the ramp: farthest={farthest:?}, final={:?}",
        sim.units[1].motion.pos
    );
    assert!(
        sim.removed_materials[4] + sim.removed_materials[5] > 0,
        "walker reached the slope without felling the authored trees"
    );
}
#[test]
fn each_sector_takes_a_real_shot_and_replays_mid_flight() {
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../data/energy-cannon.ron"
    ))
    .unwrap();
    for i in 0..layout().zones.len() {
        let r = crop(layout(), i);
        let name = r.zones[0].name.clone();
        let mut sim = BlastRange::with_layout(config, r.clone()).unwrap();
        assert_eq!(sim.gun.at, r.gun, "{name}");
        sim.control();
        assert!(sim.order_ground_fire(r.target), "{name}");
        while sim.shell.is_none() && sim.tick < 120 {
            sim.step();
        }
        assert!(sim.shell.is_some(), "no projectile in flight: {name}");
        let mut restored = BlastRange::restore(config, &sim.snapshot()).expect("sector snapshot");
        while sim.tick < 160 {
            sim.step();
            restored.step();
        }
        assert_eq!(sim.snapshot(), restored.snapshot(), "replay: {name}");
        assert!(
            sim.impact.is_some()
                || sim
                    .shields
                    .iter()
                    .any(|s| s.charge < sim.shield_tuning().capacity || s.down > 0),
            "no impact: {name}"
        );
        assert!(
            sim.removed > 0 || !sim.surface_work.is_empty() || !sim.shields.is_empty(),
            "no damage: {name}"
        );
    }
}

#[test]
fn the_authored_bridge_carries_a_tank_across_the_wide_trench() {
    let mut r = crop(layout(), 3);
    // Enter the bridge straight on; the other obstacles remain in the same world.
    let shift = [layout().zones[3].min[0] - 80, layout().zones[3].min[1] - 80];
    r.gun = [4900 - shift[0], 16, 700 - shift[1]];
    let goal = [5050 - shift[0], 700 - shift[1]];
    let config =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut sim = BlastRange::with_layout(config, r).unwrap();
    sim.control();
    sim.toggle_patrols();
    assert!(sim.order_move(goal));
    let mut crossed = false;
    for _ in 0..900 {
        sim.step();
        let p = sim.gun.motion.pos.map(|v| v.to_int_trunc());
        if (4940 - shift[0]..4996 - shift[0]).contains(&p[0]) {
            assert!(
                (674 - shift[1]..722 - shift[1]).contains(&p[2]),
                "missed deck: {p:?}"
            );
            assert!(p[1] >= 14, "fell through deck: {p:?}");
            crossed = true;
        }
    }
    let p = sim.gun.motion.pos.map(|v| v.to_int_trunc());
    assert!(crossed, "never crossed the bridge: {p:?}");
    assert!(
        (p[0] - goal[0]).abs() <= 4 && (p[2] - goal[1]).abs() <= 4,
        "did not arrive: {p:?}"
    );
}

#[test]
fn the_new_obstacle_patrol_completes_a_lap() {
    let r = crop(layout(), 3);
    let config =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let mut sim = BlastRange::with_layout(config, r).unwrap();
    sim.control();
    for _ in 0..4000 {
        sim.step();
        if sim.units[0].motion.laps > 0 {
            return;
        }
    }
    let u = &sim.units[0];
    panic!(
        "patrol did not complete a lap: at={:?} path={:?} blocked={}",
        u.at, u.motion.path, u.motion.blocked
    );
}

#[test]
fn tank_drives_the_new_saddle_three_crossings_and_factory_interior() {
    let config =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    for (name, start, goal, ticks) in [
        ("mountain saddle", [1800, 16, 2800], [1800, 3450], 900),
        (
            "three trench crossings",
            [1150, 16, 6200],
            [2740, 6200],
            1800,
        ),
        ("factory portals", [5000, 16, 7860], [5700, 7860], 1000),
    ] {
        let mut r = layout();
        r.gun = start;
        r.units.clear();
        r.shields.clear();
        r.course.as_mut().unwrap().patrols.clear();
        let mut sim = BlastRange::with_layout(config, r).unwrap();
        sim.control();
        assert!(sim.order_move(goal), "order rejected: {name}");
        for _ in 0..ticks {
            sim.step();
            let p = sim.gun.motion.pos.map(|v| v.to_int_trunc());
            assert!(p[1] >= 14, "fell below road/deck in {name}: {p:?}");
            if (p[0] - goal[0]).abs() <= 4 && (p[2] - goal[1]).abs() <= 4 {
                break;
            }
        }
        let p = sim.gun.motion.pos.map(|v| v.to_int_trunc());
        assert!(
            (p[0] - goal[0]).abs() <= 4 && (p[2] - goal[1]).abs() <= 4,
            "tank did not arrive in {name}: {p:?}"
        );
    }
}
