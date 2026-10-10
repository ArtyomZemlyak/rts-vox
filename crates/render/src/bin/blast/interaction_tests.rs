use super::*;
use std::time::Instant;

#[cfg(test)]
#[test]
fn ai_artillery_crater_is_published_to_visible_terrain() {
    artillery_crater_publication(false);
}

#[test]
fn ai_artillery_surface_crater_is_published_while_felling_competes() {
    artillery_crater_publication(true);
}

fn artillery_crater_publication(contended: bool) {
    use modding::range::{RangeUnitDef, RangeUnitKind};
    use sim_core::blast::battle::Weapon;
    let mut layout =
        RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.size = [640, 192, 640];
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units = vec![
        RangeUnitDef {
            kind: RangeUnitKind::Artillery,
            allied: false,
            at: [170, 16, 336],
            weapon: None,
        },
        RangeUnitDef {
            kind: RangeUnitKind::Tank,
            allied: true,
            at: [450, 16, 336],
            weapon: None,
        },
    ];
    if contended {
        layout.blocks.push(modding::range::RangeBlock {
            min: [250, 16, 250],
            max: [300, 64, 300],
            material: 5,
        });
    }
    let mut sim = BlastRange::with_layout(config(false, false), layout).unwrap();
    if contended {
        use sim_core::blast::battle::commands::{Command, DebrisField};
        sim.execute_command(Command::TuneDebris {
            field: DebrisField::MaxFlying,
            value: 128,
        });
        assert_eq!(
            sim.matter.fell(
                &mut sim.world,
                &[[260, 16, 260]],
                [260, 16, 260],
                &sim.navigation.fell
            ),
            1
        );
    }
    sim.start_battle().unwrap();
    assert!(sim.battle_order(1, sim_core::blast::battle::Order::Hold, None));
    let mut gas = GasInput::default();
    fill_terrain(&sim, &mut gas);
    let mut impacts = 0;
    let mut published = 0;
    let mut surface = 0;
    for _ in 0..600 {
        let start: Vec<_> = sim.units.iter().map(|u| u.removed.len()).collect();
        sim.step();
        impacts += sim
            .battle
            .impacts
            .iter()
            .filter(|p| p.weapon == Weapon::Blast)
            .count();
        apply_terrain(&sim, &mut gas, &start);
        for (edit, before) in sim.world.completed() {
            let p = [edit.x as i32, edit.y as i32, edit.z as i32];
            if edit.mat != 0 || !matches!(before, 1 | 7 | 8) || sim.units.iter().any(|u| u.solid(p))
            {
                continue;
            }
            if sim.world.material(edit.x, edit.y, edit.z) != 0 {
                continue;
            }
            let index = gas.window.index(p).expect("crater inside visible window");
            assert_eq!(
                gas.terrain_words[index / 8] >> (index % 8 * 4) & 15,
                0,
                "soil stayed visible at {p:?}"
            );
            published += 1;
            if edit.y == 15 && sim.world.material(edit.x, edit.y + 1, edit.z) == 0 {
                surface += 1;
            }
        }
        gas.terrain_dirty.clear();
        if impacts > 0 && published >= 64 && surface > 0 {
            break;
        }
    }
    assert!(
        sim.battle.actors[0].fired > 0 && impacts > 0,
        "enemy AI never fired artillery"
    );
    assert_eq!(
        sim.battle.actors[1].fired, 0,
        "the target must not dig by shooting back"
    );
    assert!(published >= 64, "AI impact did not publish a soil crater");
    assert!(
        surface > 0,
        "only buried soil changed; no surface crater was published"
    );
}

#[test]
fn tick_coordinator_does_not_enter_the_shared_render_compute_executor() {
    use bevy::ecs::system::{IntoSystem, System, SystemStateFlags};
    let mut system = IntoSystem::into_system(advance);
    system.initialize(&mut World::new());
    assert!(system.flags().contains(SystemStateFlags::NON_SEND));
    // A main-thread coordinator does not take an exclusive World borrow.
    assert!(!system.flags().contains(SystemStateFlags::EXCLUSIVE));
}

#[test]
fn scene_reset_commits_epoch_with_world_and_rejects_invalid_or_exhausted_requests() {
    let mut app = panel::hud::tests::app();
    let mut scene = app.world_mut().resource_mut::<Scene>();
    let validator = sim_core::blast::placement::Validator::default();
    let config = scene.sim.config;
    let layout = scene.sim.layout.clone().unwrap();
    let before = scene.sim.snapshot();
    let mut invalid = config;
    invalid.edits_per_tick = 0;
    assert!(scene
        .reset_world(invalid, Some(layout.clone()), &validator)
        .is_err());
    assert_eq!(scene.generation, 0);
    assert_eq!(before, scene.sim.snapshot());
    scene.generation = u64::MAX;
    assert!(scene
        .reset_world(config, Some(layout.clone()), &validator)
        .is_err());
    assert_eq!(scene.generation, u64::MAX);
    assert_eq!(before, scene.sim.snapshot());
    scene.generation = 0;
    let bits = scene.sim.world.bits().as_ptr();
    scene
        .reset_world(config, Some(layout.clone()), &validator)
        .unwrap();
    assert_eq!(scene.generation, 1);
    assert_eq!(bits, scene.sim.world.bits().as_ptr());
    let fresh = BlastRange::with_layout(config, layout).unwrap();
    assert_eq!(scene.sim.snapshot(), fresh.snapshot());
    scene.reset_world(config, None, &validator).unwrap();
    assert_eq!(scene.generation, 2);
    assert_eq!(scene.sim.snapshot(), BlastRange::new(config).snapshot());
}

#[test]
fn native_worker_override_preserves_battle_across_rebuild() {
    let layout = RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron")).unwrap();
    let mut workshop = Workshop::new(Some(layout), false);
    workshop.battle_preset(config(false, false)).unwrap();
    let layout = workshop.layout.unwrap();
    let mut serial = BlastRange::with_layout(config(false, false), layout.clone()).unwrap();
    let mut parallel = BlastRange::with_layout(config(false, false), layout.clone()).unwrap();
    let mut serial_profile = profile::Profile::new(None, 20.0);
    serial_profile.navigation_workers = Some(1);
    serial_profile.physics_workers = Some(1);
    let mut parallel_profile = profile::Profile::new(None, 20.0);
    parallel_profile.navigation_workers = Some(8);
    parallel_profile.physics_workers = Some(8);
    for round in 0..2 {
        if round > 0 {
            serial = serial
                .rebuilt_with_layout(config(false, false), layout.clone(), &workshop.validator)
                .unwrap();
            parallel = parallel
                .rebuilt_with_layout(config(false, false), layout.clone(), &workshop.validator)
                .unwrap();
        }
        serial_profile.configure_workers(&mut serial);
        parallel_profile.configure_workers(&mut parallel);
        assert_eq!(serial.worker_counts(), [1, 1]);
        assert_eq!(parallel.worker_counts(), [8, 8]);
        serial.start_battle().unwrap();
        parallel.start_battle().unwrap();
        assert_eq!(serial.snapshot(), parallel.snapshot());
        let initial = serial
            .units
            .iter()
            .map(|u| u.motion.pos)
            .collect::<Vec<_>>();
        for _ in 0..120 {
            serial.step();
            parallel.step();
        }
        assert!(serial
            .units
            .iter()
            .zip(initial)
            .any(|(u, p)| u.motion.pos != p));
        assert_eq!(serial.snapshot(), parallel.snapshot());
    }
}

#[test]
fn stock_battle_arena_is_already_authored_and_preset_preserves_terrain() {
    let layout = RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron")).unwrap();
    let tuning = modding::range_battle::Tuning::from_ron(include_str!(
        "../../../../../data/range-battle.ron"
    ))
    .unwrap();
    assert!(layout.zones.contains(&tuning.zone));
    let mut edit = Workshop::new(Some(layout.clone()), true);
    edit.battle_preset(config(false, false)).unwrap();
    let next = edit.layout.unwrap();
    assert_eq!(next.zones, layout.zones);
    assert_eq!(next.blocks, layout.blocks);
    assert!(next.battle);
}

#[test]
fn retained_terrain_removes_previous_actor_shadows_after_reset() {
    let mut layout =
        RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap();
    let config = config(false, false);
    let validator = sim_core::blast::placement::Validator::default();
    let mut sim = BlastRange::with_layout(config.clone(), layout.clone()).unwrap();
    let window = TerrainWindow {
        origin: [256, 0, 256],
        dims: [256, 192, 256],
    };
    let mut gas = GasInput::default();
    gas.terrain_words = Arc::new(vec![0; window.words()]);
    write_window(&sim, &mut gas, window, None);
    let before = gas.terrain_words.as_ref().clone();
    gas.terrain_dirty.clear();
    let mut visual = moving::Visual::default();
    moving::before(&sim, &mut visual, &window);
    layout.units[0].at[2] += 40;
    assert!(sim.same_authored_terrain(&layout));
    sim = sim.rebuilt_with_layout(config, layout, &validator).unwrap();
    moving::reset(&mut visual);
    moving::occupancy(&sim, &mut gas, &mut visual);
    let mut fresh = GasInput::default();
    fresh.terrain_words = Arc::new(vec![0; window.words()]);
    write_window(&sim, &mut fresh, window, None);
    assert_ne!(&before, fresh.terrain_words.as_ref());
    assert_eq!(gas.terrain_words, fresh.terrain_words);
    let mut uploaded = before;
    for r in &gas.terrain_dirty {
        uploaded[r.clone()].copy_from_slice(&gas.terrain_words[r.clone()]);
    }
    assert_eq!(&uploaded, fresh.terrain_words.as_ref());
}

#[test]
#[ignore = "manual kilometre-range interaction benchmark"]
fn bench_range_interactions() {
    let layout = RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron")).unwrap();
    let config = weapon_config(true, false, 1000, Delivery::Blast);
    let mark = Instant::now();
    let mut sim = BlastRange::with_layout(config.clone(), layout.clone()).unwrap();
    println!(
        "INTERACTION initial-world ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mut gas = GasInput::default();
    let mark = Instant::now();
    fill_terrain(&sim, &mut gas);
    println!(
        "INTERACTION initial-terrain ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mut edit = Workshop::new(Some(layout.clone()), true);
    let mark = Instant::now();
    edit.validator.validate(&layout).unwrap();
    println!(
        "INTERACTION validation ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
        let mark = Instant::now();
        let actor = sim_core::blast::RangeUnit::of_kind(kind, layout.gun);
        std::hint::black_box(&actor);
        println!(
            "INTERACTION actor-{kind:?} ms={:.3}",
            mark.elapsed().as_secs_f64() * 1000.0
        );
    }
    let mark = Instant::now();
    let collapse = voxel::collapse::CollapseLayer::new(&sim.world);
    std::hint::black_box(&collapse);
    println!(
        "INTERACTION collapse-init ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    drop(collapse);
    let mark = Instant::now();
    let matter = sim_core::blast::matter::Matter::new(&sim.units);
    std::hint::black_box(&matter);
    println!(
        "INTERACTION matter-init ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    drop(matter);
    let mark = Instant::now();
    sim.world.restore_pristine();
    println!(
        "INTERACTION pristine-world ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mut visual = moving::Visual::default();
    moving::before(&sim, &mut visual, &gas.window);
    let mut next = layout.clone();
    // The authored kilometre range already occupies every unit slot.
    next.units[0] = modding::range::RangeUnitDef {
        weapon: None,
        kind: RangeUnitKind::Tank,
        at: [1480, 16, 560],
        allied: true,
    };
    let mark = Instant::now();
    edit.commit(next, config.clone()).unwrap();
    let (pending_config, pending_layout) = edit.pending.take().unwrap();
    sim = sim
        .rebuilt_with_layout(pending_config, pending_layout, &edit.validator)
        .unwrap();
    println!(
        "INTERACTION place-unit ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mark = Instant::now();
    moving::reset(&mut visual);
    moving::occupancy(&sim, &mut gas, &mut visual);
    println!(
        "INTERACTION retained-terrain ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mark = Instant::now();
    fill_terrain(&sim, &mut gas);
    println!(
        "INTERACTION placed-terrain ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mark = Instant::now();
    edit.battle_preset(config.clone()).unwrap();
    let (pending_config, pending_layout) = edit.pending.take().unwrap();
    sim = sim
        .rebuilt_with_layout(pending_config, pending_layout, &edit.validator)
        .unwrap();
    println!(
        "INTERACTION battle-reset ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mark = Instant::now();
    sim = sim
        .rebuilt_with_layout(config.clone(), sim.layout.clone().unwrap(), &edit.validator)
        .unwrap();
    println!(
        "INTERACTION battle-repeat ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mut changed = sim.layout.clone().unwrap();
    changed.blocks[0].material = 5;
    let mark = Instant::now();
    sim = sim
        .rebuilt_with_layout(config.clone(), changed, &edit.validator)
        .unwrap();
    println!(
        "INTERACTION one-block-material ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mark = Instant::now();
    sim = sim
        .rebuilt_with_layout(config.clone(), layout, &edit.validator)
        .unwrap();
    println!(
        "INTERACTION block-revert ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mark = Instant::now();
    recentre_terrain(&sim, &mut gas, [9090, 16, 8800]);
    println!(
        "INTERACTION far-sector ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mark = Instant::now();
    recentre_terrain(&sim, &mut gas, [400, 16, 336]);
    println!(
        "INTERACTION return-sector ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mark = Instant::now();
    recentre_terrain(&sim, &mut gas, [650, 16, 336]);
    println!(
        "INTERACTION one-step-pan ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    let mark = Instant::now();
    recentre_terrain(&sim, &mut gas, [400, 16, 336]);
    println!(
        "INTERACTION one-step-return ms={:.3}",
        mark.elapsed().as_secs_f64() * 1000.0
    );
    streaming::bench(&sim, &gas, [9090, 16, 8800]);
}
