use super::*;
use modding::range::{RangeLayout, RangeUnitDef};

#[cfg(test)]
fn world() -> BlastRange {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.size = [640, 192, 640];
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units = [(170, true), (450, false)]
        .map(|(x, allied)| RangeUnitDef {
            weapon: None,
            kind: RangeUnitKind::Tank,
            at: [x, 16, 336],
            allied,
        })
        .to_vec();
    BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap()
}

#[cfg(test)]
fn update(view: &mut View, world: &BlastRange, generation: u64) {
    view.update(
        world,
        generation,
        Cursor {
            generation,
            counts: &[],
        },
        Sampling {
            stride: NonZeroUsize::new(97).unwrap(),
            per_unit: 32,
        },
    );
}

#[cfg(test)]
#[test]
fn geometry_payload_owns_mutable_root_segments_and_ledgers_without_changing_save() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<View>();
    send_sync::<Mesh>();
    let mut world = world();
    world.units[0].dents.insert([0, 0, 0], 15);
    world.units[0].seg_dents.insert((0, [0, 0, 0]), 33);
    world.units[0].revision += 1;
    let before = world.snapshot();
    let mut view = View::default();
    update(&mut view, &world, 7);
    let mesh = view.mesh(&world, 7, 0).unwrap();
    assert_eq!(before, world.snapshot());
    assert_eq!((mesh.generation, mesh.tick, mesh.id), (7, 0, 0));
    assert_eq!(mesh.config, world.config);
    assert_eq!(mesh.allied, Some(true));
    assert_eq!(view.mesh(&world, 7, 1).unwrap().allied, Some(false));
    assert_eq!(mesh.body.vol.cols, world.units[0].body.vol.cols);
    assert_ne!(
        mesh.body.vol.cols.as_ptr(),
        world.units[0].body.vol.cols.as_ptr()
    );
    assert_eq!(mesh.dents.get(&[0, 0, 0]), Some(&15));
    assert_eq!(mesh.segment_dents.get(&(0, [0, 0, 0])), Some(&33));
    let root = mesh.body.vol.cols.clone();
    let segment = mesh.body.segments[0].body.vol.cols.clone();
    let gone = mesh.seg_gone.clone();
    world.units[0].body.vol.cols.fill(0);
    world.units[0].body.segments[0].body.vol.cols.fill(0);
    world.units[0].seg_gone.fill(true);
    world.units[0].dents.clear();
    world.units[0].seg_dents.clear();
    world.units[0].status.alive = false;
    world.config.steel_resistance += 1;
    assert_eq!(mesh.body.vol.cols, root);
    assert_eq!(mesh.body.segments[0].body.vol.cols, segment);
    assert_eq!(mesh.seg_gone, gone);
    assert_eq!(mesh.dents.get(&[0, 0, 0]), Some(&15));
    assert_eq!(mesh.segment_dents.get(&(0, [0, 0, 0])), Some(&33));
    assert!(!mesh.revision.dead);
    assert_ne!(mesh.config, world.config);
    assert!(mesh.private_payload_bytes() > mesh.body.bytes_private());
}

#[cfg(test)]
#[test]
fn geometry_same_tick_refresh_rejects_stale_damage_config_anchor_and_slot() {
    let mut world = world();
    let mut view = View::default();
    update(&mut view, &world, 9);
    let capacity = view.units.capacity();
    assert!(view.mesh(&world, 8, 0).is_none());
    assert!(view.mesh(&world, 9, usize::MAX).is_none());
    world.units[0].revision += 1;
    assert!(view.mesh(&world, 9, 0).is_none());
    update(&mut view, &world, 9);
    world.units[0].status.alive = false;
    assert!(view.mesh(&world, 9, 0).is_none());
    update(&mut view, &world, 9);
    assert!(view.mesh(&world, 9, 0).unwrap().revision.dead);
    world.config.steel_resistance += 1;
    assert!(view.mesh(&world, 9, 0).is_none());
    update(&mut view, &world, 9);
    world.units[0].at[0] += 1;
    assert!(view.mesh(&world, 9, 0).is_none());
    update(&mut view, &world, 9);
    assert!(view.mesh(&world, 9, 0).is_some());
    assert_eq!(view.tick, 0);
    assert_eq!(view.units.capacity(), capacity);
    world.units.pop();
    assert!(view.mesh(&world, 9, 1).is_none());
    update(&mut view, &world, 9);
    assert_eq!(view.units.len(), 1);
    world.tick += 1;
    assert!(view.mesh(&world, 9, 0).is_none());
}

#[cfg(test)]
#[test]
fn geometry_work_cost_changes_invalidate_completed_and_pending_mesh_keys_on_pause() {
    let mut world = world();
    let mut view = View::default();
    update(&mut view, &world, 3);
    let pending = view.mesh(&world, 3, 0).unwrap();
    let seen = view.units[0].revision;
    let physical_revision = world.units[0].revision;
    let mut config = world.config;
    config.unit_role_resistance_milli = config.unit_role_resistance_milli.map(|v| v * 2);
    assert_eq!(
        world.configure_weapon(config),
        crate::blast::battle::commands::Outcome::Applied
    );
    assert!(view.mesh(&world, 3, 0).is_none());
    update(&mut view, &world, 3);
    assert_ne!(seen, view.units[0].revision);
    assert_ne!(pending.revision, view.units[0].revision);
    assert_eq!(world.units[0].revision, physical_revision);
    assert_eq!(world.tick, 0);
    assert_eq!(
        pending.config.unit_role_resistance_milli,
        seen.role_resistance_milli
    );
    let mesh = view.mesh(&world, 3, 0).unwrap();
    assert_eq!(
        mesh.revision.role_resistance_milli,
        config.unit_role_resistance_milli
    );
}

#[cfg(test)]
#[test]
fn geometry_removal_samples_bound_source_order_and_scope_cursor_to_generation() {
    let mut world = world();
    world.config.material_energy_j = 0;
    world.impact = Some([300, 24, 336]);
    for unit in &mut world.units {
        unit.removed = (0..10).map(|x| [x, 0, 0]).collect();
    }
    let mut view = View::default();
    let sampling = Sampling {
        stride: NonZeroUsize::new(3).unwrap(),
        per_unit: 2,
    };
    view.update(
        &world,
        7,
        Cursor {
            generation: 7,
            counts: &[2, 5],
        },
        sampling,
    );
    assert_eq!(
        view.removed
            .iter()
            .map(|r| (r.unit, r.cell[0]))
            .collect::<Vec<_>>(),
        vec![(0, 2), (0, 5), (1, 5), (1, 8)]
    );
    assert_eq!(
        view.removed[0].position,
        world.units[0].world_pos([2, 0, 0])
    );
    let capacity = view.removed.capacity();
    view.update(
        &world,
        7,
        Cursor {
            generation: 7,
            counts: &[usize::MAX],
        },
        sampling,
    );
    assert_eq!(
        view.removed
            .iter()
            .map(|r| (r.unit, r.cell[0]))
            .collect::<Vec<_>>(),
        vec![(1, 0), (1, 3)]
    );
    view.update(
        &world,
        8,
        Cursor {
            generation: 7,
            counts: &[10, 10],
        },
        sampling,
    );
    assert_eq!(
        view.removed
            .iter()
            .map(|r| (r.unit, r.cell[0]))
            .collect::<Vec<_>>(),
        vec![(0, 0), (0, 3), (1, 0), (1, 3)]
    );
    assert_eq!(view.removed.capacity(), capacity);
    let saved = view.clone();
    world.impact = None;
    update(&mut view, &world, 8);
    assert!(view.removed.is_empty());
    assert_eq!(saved.removed.len(), 4);
    world.impact = Some([300, 24, 336]);
    world.config.material_energy_j = 1;
    update(&mut view, &world, 8);
    assert!(view.impact.is_none() && view.removed.is_empty());
    world.config.material_energy_j = 0;
    view.update(
        &world,
        8,
        Cursor {
            generation: 8,
            counts: &[],
        },
        Sampling {
            per_unit: 0,
            ..sampling
        },
    );
    assert!(view.removed.is_empty());
}

#[cfg(test)]
#[test]
fn geometry_reset_and_layoutless_publication_replace_old_slots_and_keep_captured_mesh() {
    let mut world = world();
    let mut view = View::default();
    update(&mut view, &world, 2);
    let captured = view.mesh(&world, 2, 0).unwrap();
    let before = captured.body.vol.cols.clone();
    let mut layout = world.layout.clone().unwrap();
    layout.units.truncate(1);
    layout.units[0].kind = RangeUnitKind::Spider;
    world
        .reset_world(
            world.config,
            Some(layout),
            &crate::blast::placement::Validator::default(),
        )
        .unwrap();
    assert!(view.mesh(&world, 3, 0).is_none());
    update(&mut view, &world, 3);
    assert_eq!(view.units.len(), 1);
    assert_eq!(view.units[0].revision.kind, RangeUnitKind::Spider);
    assert_eq!(captured.body.vol.cols, before);
    assert_eq!(captured.generation, 2);
    world
        .reset_world(
            world.config,
            None,
            &crate::blast::placement::Validator::default(),
        )
        .unwrap();
    update(&mut view, &world, 4);
    assert_eq!(view.units.len(), world.units.len());
    assert_eq!(view.mesh(&world, 4, 0).unwrap().allied, None);
    world.units.clear();
    update(&mut view, &world, 4);
    assert!(view.units.is_empty() && view.removed.is_empty());
    assert!(view.mesh(&world, 4, 0).is_none());
}

#[cfg(test)]
#[test]
fn geometry_export_during_real_combat_and_restore_preserves_each_physical_step() {
    let mut world = world();
    let mut tuning = *world.enemy_ai_tuning();
    tuning.decision_ticks = 30;
    world.configure_enemy_ai(tuning).unwrap();
    world.set_ai_vs_ai(true);
    world.start_battle().unwrap();
    let mut control = BlastRange::restore(world.config, &world.snapshot()).unwrap();
    let mut view = View::default();
    let mut captured_damage = false;
    for tick in 0..240 {
        world.step();
        control.step();
        update(&mut view, &world, 5);
        let mesh = view.mesh(&world, 5, 0).unwrap();
        captured_damage |= mesh.revision.damage > 0 || mesh.revision.removed > 0;
        assert_eq!(world.state_hash(), control.state_hash());
        if tick == 60 || tick == 200 {
            let before = world.snapshot();
            world = BlastRange::restore(world.config, &before).unwrap();
            assert_eq!(before, world.snapshot());
            assert!(view.mesh(&world, 5, 0).is_some());
        }
    }
    assert!(world.battle.actors.iter().any(|a| a.fired > 0));
    assert!(world.units.iter().any(|u| !u.removed.is_empty()));
    assert!(captured_damage);
    assert_eq!(world.snapshot(), control.snapshot());
}
