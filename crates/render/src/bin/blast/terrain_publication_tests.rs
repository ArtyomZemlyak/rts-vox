use super::*;

fn crater() -> (BlastRange, GasInput, Vec<usize>) {
    let mut layout =
        RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap();
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    let mut sim = BlastRange::with_layout(config(false, false), layout).unwrap();
    let window = TerrainWindow {
        origin: [128, 0, 128],
        dims: [256, 96, 256],
    };
    let mut gas = GasInput::default();
    gas.terrain_words = Arc::new(vec![0; window.words()]);
    write_window(&sim, &mut gas, window, None);
    gas.params.distant.z = 4.0;
    gas.terrain_dirty.clear();
    sim.world.max_edits_per_tick = 65536;
    sim.world.max_materialize_per_tick = 1024;
    for x in 160..288 {
        for y in 0..16 {
            for z in 160..192 {
                sim.world.queue_edit(voxel::Edit::dig(x, y, z));
                sim.surface_work.insert([x as i32, y as i32, z as i32], 20);
            }
        }
    }
    for (dx, mat) in [2, 3, 6].into_iter().enumerate() {
        let p = [220 + dx as i32, 30, 300];
        sim.world
            .queue_edit(voxel::Edit::put(p[0] as u32, p[1] as u32, p[2] as u32, mat));
        sim.surface_work.insert(p, 200);
        sim.surface_changed.push(p);
    }
    sim.world.flush();
    assert!(sim.world.edits().len() > 50000);
    // Both outside the detailed window and overlapping the freshly dug cells.
    sim.surface_changed.extend((0..65536).map(|i| {
        if i % 2 == 0 {
            [100 + i % 64, 20, 500]
        } else {
            [160 + i % 128, 8, 170]
        }
    }));
    let starts = sim.units.iter().map(|u| u.removed.len()).collect();
    (sim, gas, starts)
}

fn copy_input(gas: &GasInput) -> GasInput {
    let mut copy = GasInput::default();
    copy.params = gas.params;
    copy.window = gas.window;
    copy.terrain_words = Arc::new(gas.terrain_words.as_ref().clone());
    copy.terrain_column_top = gas.terrain_column_top.clone();
    copy.distant_edits.clone_from(&gas.distant_edits);
    copy
}

#[test]
fn crater_publication_matches_full_rebuild_and_uploads_every_changed_word() {
    let (sim, mut gas, starts) = crater();
    let before = gas.terrain_words.as_ref().clone();
    apply_terrain(&sim, &mut gas, &starts);
    let mut full = GasInput::default();
    full.terrain_words = Arc::new(vec![0; gas.window.words()]);
    write_window(&sim, &mut full, gas.window, None);
    assert_eq!(gas.terrain_words, full.terrain_words);
    let mut uploaded = before;
    for span in &gas.terrain_dirty {
        uploaded[span.clone()].copy_from_slice(&gas.terrain_words[span.clone()]);
    }
    assert_eq!(&uploaded, gas.terrain_words.as_ref());
    let expected: std::collections::BTreeSet<_> = sim
        .world
        .edits()
        .iter()
        .map(|e| [e.x / 4, e.z / 4])
        .chain(
            sim.surface_changed
                .iter()
                .filter(|p| p[0] >= 0 && p[2] >= 0)
                .map(|p| [p[0] as u32 / 4, p[2] as u32 / 4]),
        )
        .collect();
    assert_eq!(gas.distant_edits, expected);
    gas.terrain_dirty.clear();
    let shared_upload = Arc::clone(&gas.terrain_words);
    apply_terrain(&sim, &mut gas, &starts);
    assert!(
        gas.terrain_dirty.is_empty(),
        "unchanged words were published again"
    );
    assert_eq!(gas.terrain_words, full.terrain_words);
    assert!(Arc::ptr_eq(&shared_upload, &gas.terrain_words), "an unchanged publication copied the shared GPU upload");
}

#[test]
fn off_window_edits_keep_shared_upload_and_update_distant_columns() {
    let layout = RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap();
    let mut sim = BlastRange::with_layout(config(false, false), layout).unwrap();
    let window = TerrainWindow { origin: [128, 0, 128], dims: [256, 96, 256] };
    let mut gas = GasInput::default();
    gas.terrain_words = Arc::new(vec![0; window.words()]);
    write_window(&sim, &mut gas, window, None);
    gas.params.distant.z = 4.0;
    gas.terrain_dirty.clear();
    gas.distant_edits.clear();
    let shared = Arc::clone(&gas.terrain_words);
    sim.surface_changed.push([500, 20, 500]);
    let starts = sim.units.iter().map(|u| u.removed.len()).collect::<Vec<_>>();
    let revision = gas.terrain_revision;
    apply_terrain(&sim, &mut gas, &starts);
    assert!(Arc::ptr_eq(&shared, &gas.terrain_words));
    assert!(gas.terrain_dirty.is_empty());
    assert!(gas.distant_edits.contains(&[125, 125]));
    assert_eq!(gas.terrain_revision, revision + 1);
}

#[test]
fn surface_fast_path_preserves_the_full_material_palette_and_dent_levels() {
    let layout =
        RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap();
    let mut sim = BlastRange::with_layout(config(false, false), layout).unwrap();
    let p = [230, 30, 290];
    for work in [0, 1, 100, u32::MAX] {
        sim.surface_work.insert(p, work);
        for mat in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 13, 14, 16, 63, 64, u16::MAX] {
            let expected = surface_tint(
                mat,
                work,
                sim_core::blast::material_work(mat, &sim.config).max(1),
            );
            assert_eq!(
                surface_material(&sim, p, mat),
                expected,
                "mat={mat} work={work}"
            );
        }
    }
}

#[test]
fn production_between_capture_and_publication_keeps_new_actor_occupancy() {
    use modding::range::{RangeUnitDef, RangeUnitKind};
    use modding::range_economy::{Producer, RangeEconomy};
    use sim_core::blast::battle::commands::{Command, Outcome};
    let mut layout =
        RangeLayout::from_ron(include_str!("../../../../../data/fixture-drive.ron")).unwrap();
    let base = layout.units.len();
    layout.units.push(RangeUnitDef {
        kind: RangeUnitKind::Base,
        allied: true,
        at: [520, 16, 260],
        weapon: None,
    });
    let mut economy = RangeEconomy::stock();
    economy.kind_mut(RangeUnitKind::Tank).unwrap().build_secs = 1;
    economy.bank.as_mut().unwrap().base_store = 1 << 40;
    economy.production.as_mut().unwrap().producers = vec![Producer {
        kind: RangeUnitKind::Base,
        produces: vec![RangeUnitKind::Tank],
    }];
    layout.economy = Some(economy);
    let mut sim = BlastRange::with_layout(config(false, false), layout).unwrap();
    sim.start_free_control().unwrap();
    let cost = sim.production_cost(RangeUnitKind::Tank).unwrap();
    let store = sim.matter.economy.banks[0].stock;
    let e = &mut sim.matter.economy;
    e.ledger.reveal(e.ground, cost);
    assert_eq!(e.ledger.transfer(e.ground, store, cost), cost);
    assert_eq!(sim.side_store(true).unwrap().0, cost);
    assert_eq!(
        sim.execute_command(Command::Produce {
            actor: base,
            kind: RangeUnitKind::Tank
        }),
        Outcome::Applied
    );
    let window = TerrainWindow {
        origin: [384, 0, 128],
        dims: [256, 128, 256],
    };
    let mut gas = GasInput::default();
    gas.terrain_words = Arc::new(vec![0; window.words()]);
    write_window(&sim, &mut gas, window, None);
    let mut visual = moving::Visual::default();
    let authored = sim.units.len();
    for _ in 0..sim_core::TICKS_PER_SECOND * 2 {
        let starts: Vec<_> = sim.units.iter().map(|u| u.removed.len()).collect();
        moving::before(&sim, &mut visual, &window);
        sim.step();
        apply_terrain(&sim, &mut gas, &starts);
        moving::occupancy(&sim, &mut gas, &mut visual);
        if sim.units.len() > authored {
            assert_eq!(sim.units.len(), starts.len() + 1);
            let mut full = GasInput::default();
            full.terrain_words = Arc::new(vec![0; window.words()]);
            write_window(&sim, &mut full, window, None);
            assert_eq!(gas.terrain_words, full.terrain_words);
            assert!(sim.matter.economy.ledger.balanced());
            return;
        }
    }
    panic!("the producer never appended a unit");
}

#[test]
#[ignore = "isolated terrain-publication CPU benchmark"]
fn bench_crater_publication() {
    let (sim, input, starts) = crater();
    let mut samples = Vec::new();
    let mut words = 0;
    for i in 0..11 {
        let mut gas = copy_input(&input);
        let start = std::time::Instant::now();
        apply_terrain(std::hint::black_box(&sim), &mut gas, &starts);
        let elapsed = start.elapsed().as_nanos();
        words = gas.terrain_dirty.iter().map(|r| r.len()).sum::<usize>();
        std::hint::black_box(&gas);
        if i >= 2 {
            samples.push(elapsed);
        }
    }
    samples.sort_unstable();
    println!(
        "PUBLICATION edits={} surface={} median_ns={} max_ns={} upload_words={}",
        sim.world.edits().len(),
        sim.surface_changed.len(),
        samples[4],
        samples[8],
        words
    );
}
