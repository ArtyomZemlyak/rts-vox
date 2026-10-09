use super::*;

#[cfg(test)]
fn arena() -> BlastRange {
    let mut layout =
        modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
            .unwrap();
    layout.size = [640, 192, 640];
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    layout.battle = true;
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units = [(170, true), (450, false)]
        .map(|(x, allied)| modding::range::RangeUnitDef {
            weapon: None,
            kind: super::super::super::RangeUnitKind::Tank,
            at: [x, 16, 336],
            allied,
        })
        .to_vec();
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../data/energy-cannon.ron"
    ))
    .unwrap();
    BlastRange::with_layout(config, layout).unwrap()
}
#[cfg(test)]
fn request() -> Request {
    Request {
        x: 64..96,
        y: 8..64,
        z: 82,
    }
}
#[cfg(test)]
fn budget() -> Budget {
    Budget {
        columns: 32,
        cells: 32 * 56,
    }
}

#[cfg(test)]
#[test]
fn raw_columns_and_sparse_damage_are_owned_and_do_not_mutate_world() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Row>();
    send_sync::<Changes>();
    let mut sim = arena();
    sim.world.queue_edit(voxel::world::Edit::put(72, 24, 82, 3));
    sim.world.queue_dig(74, 8, 82);
    sim.world.flush();
    sim.surface_work.insert([72, 24, 82], 380);
    sim.surface_work.insert([72, 24, 83], 380);
    sim.surface_work.insert([72, 70, 82], 380);
    let before = sim.snapshot();
    let mut row = Row::default();
    row.update(&sim, 7, request(), budget()).unwrap();
    assert_eq!(before, sim.snapshot());
    assert_eq!(row.header.generation, 7);
    assert_eq!(row.damage.len(), 1);
    assert_eq!(row.damage[0].material, 3);
    assert_eq!(
        row.damage[0].cost,
        super::super::super::material_work(3, &sim.config).max(1)
    );
    for x in 64..96 {
        let (column, values) = row.column(x).unwrap();
        assert_eq!(column.top, sim.world.column_ceiling(x as u32, 82));
        let mut expected = vec![0; values.len()];
        sim.world.column(x as u32, 82, 8, &mut expected);
        assert_eq!(values, expected);
    }
    assert!(row.column(63).is_none());
    assert!(row.column(96).is_none());
    let captured = row.clone();
    sim.world.queue_dig(72, 24, 82);
    sim.world.flush();
    sim.surface_work.clear();
    sim.config.steel_resistance *= 2;
    row.update(&sim, 7, request(), budget()).unwrap();
    assert_ne!(row, captured);
    assert_eq!(captured.damage[0].work, 380);
    assert_eq!(captured.column(72).unwrap().1[16], 3);
    assert_eq!(row.column(72).unwrap().1.get(16).copied().unwrap_or(0), 0);
    assert!(row.damage.is_empty());
}

#[cfg(test)]
#[test]
fn same_tick_edits_and_cost_changes_refresh_without_reallocating() {
    let mut sim = arena();
    sim.world.queue_edit(voxel::world::Edit::put(72, 24, 82, 2));
    sim.world.flush();
    sim.surface_work.insert([72, 24, 82], 380);
    let mut row = Row::default();
    row.update(&sim, 1, request(), budget()).unwrap();
    let pointers = (
        row.columns.as_ptr(),
        row.materials.as_ptr(),
        row.damage.as_ptr(),
    );
    let old = row.clone();
    sim.config.concrete_resistance *= 2;
    row.update(&sim, 1, request(), budget()).unwrap();
    assert_eq!(row.header.tick, old.header.tick);
    assert_ne!(row.header.costs, old.header.costs);
    assert_eq!(row.damage[0].cost, old.damage[0].cost * 2);
    assert_eq!(
        pointers,
        (
            row.columns.as_ptr(),
            row.materials.as_ptr(),
            row.damage.as_ptr()
        )
    );
    row.update(
        &sim,
        2,
        Request {
            x: 64..72,
            y: 60..64,
            z: 82,
        },
        budget(),
    )
    .unwrap();
    assert_eq!(row.header.generation, 2);
    assert_eq!(row.columns.len(), 8);
    assert!(row.materials.is_empty());
    assert!(row.damage.is_empty());
}

#[cfg(test)]
#[test]
fn invalid_or_over_budget_request_preserves_previous_publication() {
    let sim = arena();
    let mut row = Row::default();
    row.update(&sim, 3, request(), budget()).unwrap();
    let before = row.clone();
    for bad in [
        Request {
            x: -1..10,
            ..request()
        },
        Request {
            x: 640..641,
            ..request()
        },
        Request {
            // Deliberately reversed: the request must be refused.
            x: std::ops::Range { start: 96, end: 64 },
            ..request()
        },
        Request {
            y: 0..193,
            ..request()
        },
        Request {
            y: 8..8,
            ..request()
        },
        Request { z: -1, ..request() },
        Request {
            z: 640,
            ..request()
        },
    ] {
        assert_eq!(
            row.update(&sim, 4, bad, budget()),
            Err(Error::InvalidRequest)
        );
        assert_eq!(row, before);
    }
    for cap in [
        Budget {
            columns: 31,
            cells: usize::MAX,
        },
        Budget {
            columns: 32,
            cells: 1791,
        },
        Budget {
            columns: 0,
            cells: 0,
        },
    ] {
        assert_eq!(
            row.update(&sim, 4, request(), cap),
            Err(Error::BudgetExceeded)
        );
        assert_eq!(row, before);
    }
}

#[cfg(test)]
#[test]
fn changed_rows_filter_deduplicate_and_reuse_without_changing_source_order() {
    let mut sim = arena();
    for (x, z) in [(72, 82), (72, 82), (72, 83), (100, 84), (72, 90)] {
        sim.world.queue_dig(x, 8, z);
    }
    sim.world.flush();
    sim.surface_changed.extend([
        [73, 24, 82],
        [73, 24, 85],
        [73, 24, 85],
        [-1, 24, 86],
        [73, 24, -1],
    ]);
    let mut changes = Changes::default();
    changes.update(&sim, 1, 64..96, 80..88, 8).unwrap();
    assert_eq!(changes.rows, [82, 83, 85]);
    let before = changes.clone();
    let pointers = (changes.rows.as_ptr(), changes.touched.as_ptr());
    changes.update(&sim, 1, 64..96, 80..88, 8).unwrap();
    assert_eq!(changes, before);
    assert_eq!(pointers, (changes.rows.as_ptr(), changes.touched.as_ptr()));
    assert_eq!(
        changes.update(&sim, 2, 64..96, 80..88, 7),
        Err(Error::BudgetExceeded)
    );
    assert_eq!(changes, before);
    assert_eq!(
        changes.update(&sim, 2, std::ops::Range { start: 96, end: 64 }, 80..88, 8),
        Err(Error::InvalidRequest)
    );
    assert_eq!(changes, before);
    sim.world.flush();
    sim.surface_changed.clear();
    changes.update(&sim, 2, 64..96, 80..88, 8).unwrap();
    assert!(changes.rows.is_empty());
    assert_eq!(changes.header.generation, 2);
}

#[cfg(test)]
#[test]
fn clipped_height_and_reset_use_fresh_owned_generations() {
    let mut sim = arena();
    sim.world
        .queue_edit(voxel::world::Edit::put(638, 190, 638, 3));
    sim.world.flush();
    let mut row = Row::default();
    row.update(
        &sim,
        1,
        Request {
            x: 638..640,
            y: 188..192,
            z: 638,
        },
        Budget {
            columns: 2,
            cells: 8,
        },
    )
    .unwrap();
    assert_eq!(row.column(638).unwrap().0.top, 191);
    assert_eq!(row.column(638).unwrap().1, [0, 0, 3]);
    let captured = row.clone();
    let sim = BlastRange::new(sim.config);
    row.update(
        &sim,
        2,
        Request {
            x: 638..640,
            y: 188..192,
            z: 638,
        },
        Budget {
            columns: 2,
            cells: 8,
        },
    )
    .unwrap();
    assert_eq!(row.header.generation, 2);
    assert!(row.materials.is_empty());
    assert_eq!(captured.column(638).unwrap().1, [0, 0, 3]);
}

#[cfg(test)]
#[test]
fn real_combat_exports_preserve_physics_and_restored_state() {
    let mut sim = arena();
    let mut control = arena();
    for world in [&mut sim, &mut control] {
        assert!(world.set_ai_vs_ai(true));
        world.start_battle().unwrap();
    }
    let mut row = Row::default();
    let mut changes = Changes::default();
    for tick in 0..240 {
        sim.step();
        control.step();
        row.update(
            &sim,
            1,
            Request {
                x: 288..320,
                y: 0..64,
                z: 320 + (tick % 32),
            },
            Budget {
                columns: 32,
                cells: 2048,
            },
        )
        .unwrap();
        changes.update(&sim, 1, 288..320, 280..440, 160).unwrap();
        assert_eq!(sim.state_hash(), control.state_hash(), "tick {tick}");
        if tick == 60 || tick == 200 {
            let save = sim.snapshot();
            sim = BlastRange::restore(sim.config, &save).unwrap();
            assert_eq!(save, sim.snapshot());
        }
    }
    assert!(sim.battle.actors.iter().any(|a| a.fired > 0));
    assert!(sim.units.iter().any(|u| !u.removed.is_empty()));
}

#[cfg(test)]
#[test]
fn explicit_air_padding_preserves_edges_and_budget_without_weakening_strict_requests() {
    let mut sim = arena();
    sim.world
        .queue_edit(voxel::world::Edit::put(638, 190, 638, 3));
    sim.world.flush();
    let request = Request {
        x: 638..646,
        y: 188..196,
        z: 638,
    };
    let budget = Budget {
        columns: 8,
        cells: 64,
    };
    let mut row = Row::default();
    assert_eq!(
        row.update(&sim, 1, request.clone(), budget),
        Err(Error::InvalidRequest)
    );
    row.update_padded(&sim, 1, request.clone(), budget).unwrap();
    assert_eq!(row.column(638).unwrap().1, [0, 0, 3]);
    assert!(row.column(639).unwrap().1.is_empty());
    for x in 640..646 {
        assert_eq!(row.column(x).unwrap().0.top, 0);
        assert!(row.column(x).unwrap().1.is_empty());
    }
    let before = row.clone();
    assert_eq!(
        row.update_padded(
            &sim,
            2,
            request.clone(),
            Budget {
                columns: 8,
                cells: 63
            }
        ),
        Err(Error::BudgetExceeded)
    );
    assert_eq!(row, before);
    row.update_padded(&sim, 2, Request { z: 640, ..request }, budget)
        .unwrap();
    assert!(row.materials.is_empty());
    assert!(row.damage.is_empty());
    assert!(row.columns.iter().all(|c| c.top == 0 && c.len == 0));
    assert_eq!(row.header.size, [640, 192, 640]);
    assert_eq!(row.header.generation, 2);
    let mut changes = Changes::default();
    assert_eq!(
        changes.update(&sim, 2, 638..646, 638..646, 8),
        Err(Error::InvalidRequest)
    );
    changes
        .update_padded(&sim, 2, 638..646, 638..646, 8)
        .unwrap();
    assert_eq!(changes.rows, [638]);
    let before = changes.clone();
    assert_eq!(
        changes.update_padded(&sim, 3, 638..646, 638..646, 7),
        Err(Error::BudgetExceeded)
    );
    assert_eq!(changes, before);
    changes
        .update_padded(&sim, 3, 640..648, 640..648, 8)
        .unwrap();
    assert!(changes.rows.is_empty());
}
