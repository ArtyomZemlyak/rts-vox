//! Read-ahead is disposable: it must preserve ordered contacts and live matter.
use super::*;

#[test]
fn cavities_below_a_roof_skip_materials_and_recheck_live_occupancy() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct Roof(Arc<AtomicUsize>);
    impl Generator for Roof {
        fn material(&self, _: u32, y: u32, _: u32) -> u16 {
            u16::from(y >= 40)
        }
        fn batches_columns(&self) -> bool {
            true
        }
        fn column(&self, _: u32, _: u32, y: u32, out: &mut [u16]) {
            self.0.fetch_add(1, Ordering::Relaxed);
            for (i, mat) in out.iter_mut().enumerate() {
                *mat = u16::from(y + i as u32 >= 40);
            }
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let mut w = World::new(80, 80, 80, 65536, Box::new(Roof(calls.clone())));
    for edge in [2, 4, 9, 16, 32] {
        calls.store(0, Ordering::Relaxed);
        let mut mats = vec![123];
        let mask = read_cell_into(&w, [0; 3], edge, [0; 3], &subcell_offsets(edge), &mut mats);
        assert_eq!(mask, contact_mask::Mask::for_voxels((edge as usize).pow(3)));
        assert_eq!(mats, [123]);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }
    w.queue_edit(voxel::Edit::put(8, 8, 8, 65535));
    w.flush();
    let mut mats = Vec::new();
    let mask = read_cell_into(&w, [0; 3], 16, [0; 3], &subcell_offsets(16), &mut mats);
    assert_eq!(
        mask.indices_from(0).collect::<Vec<_>>(),
        [8 + 16 * (8 + 16 * 8)]
    );
    assert_eq!(mats, [65535]);
}

#[test]
fn fine_cells_share_column_preparation_without_scalar_material_queries() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct Columns {
        scalar: Arc<AtomicUsize>,
        columns: Arc<AtomicUsize>,
    }
    impl Generator for Columns {
        fn material(&self, _: u32, y: u32, _: u32) -> u16 {
            self.scalar.fetch_add(1, Ordering::Relaxed);
            if y % 2 == 0 {
                1
            } else {
                7
            }
        }
        fn batches_columns(&self) -> bool {
            true
        }
        fn column(&self, _: u32, _: u32, y: u32, out: &mut [u16]) {
            self.columns.fetch_add(1, Ordering::Relaxed);
            for (i, mat) in out.iter_mut().enumerate() {
                *mat = if (y + i as u32) % 2 == 0 { 1 } else { 7 };
            }
        }
    }
    let scalar = Arc::new(AtomicUsize::new(0));
    let columns = Arc::new(AtomicUsize::new(0));
    let w = World::new(
        32,
        32,
        32,
        100,
        Box::new(Columns {
            scalar: scalar.clone(),
            columns: columns.clone(),
        }),
    );
    for edge in [2, 3, 4] {
        scalar.store(0, Ordering::Relaxed);
        columns.store(0, Ordering::Relaxed);
        let mut mats = Vec::new();
        let mask = read_cell_into(&w, [10; 3], edge, [0; 3], &subcell_offsets(edge), &mut mats);
        assert_eq!(scalar.load(Ordering::Relaxed), 0);
        assert_eq!(columns.load(Ordering::Relaxed), (edge * edge) as usize);
        assert_eq!(mask.count_ones(), (edge * edge * edge) as u32);
        let expected: Vec<_> = subcell_offsets(edge)
            .iter()
            .map(|p| if (10 + p[1]) % 2 == 0 { 1 } else { 7 })
            .collect();
        assert_eq!(mats, expected);
    }
}

fn perforated_soil_fixture(edge: i32) -> (World, Explosion, ExplosionConfig, matter::Matter) {
    struct Soil;
    impl Generator for Soil {
        fn material(&self, _: u32, _: u32, _: u32) -> u16 {
            1
        }
        fn uniform_box(&self, _: [u32; 3], _: [u32; 3]) -> Option<u16> {
            Some(1)
        }
    }
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    c.cell_units = edge;
    c.material_energy_j = 0;
    let mut w = World::new(
        (edge * 16) as u32,
        (edge * 4) as u32,
        (edge * 16) as u32,
        65536,
        Box::new(Soil),
    );
    let mut e = Explosion::begin([edge * 8, edge * 8, edge * 8], &c, &[]);
    e.origin = [0; 3];
    e.soil_plane_y = Some(edge * 8);
    for z in 0..16 {
        for y in 0..4 {
            for x in 0..16 {
                w.queue_edit(voxel::Edit::put(
                    (x * edge) as u32,
                    (y * edge) as u32,
                    (z * edge) as u32,
                    0,
                ));
                e.pending.push_back(phys::blast::Hit {
                    cell: [x, y, z],
                    pressure: 1,
                    broken: true,
                });
            }
        }
    }
    w.flush();
    (w, e, c, matter::Matter::new(&[]))
}

#[test]
fn packed_soil_proof_preserves_contacts_with_holes_and_stale_foreign_palette() {
    for edge in [2, 4, 9, 16] {
        let (mut w, mut e, c, m) = perforated_soil_fixture(edge);
        // A stale foreign palette entry must not force material preparation.
        w.queue_edit(voxel::Edit::put(1, 1, 1, 65535));
        w.flush();
        w.queue_edit(voxel::Edit::put(1, 1, 1, 1));
        w.flush();
        for budget in [0, 1, 3, 65536] {
            e.read_contacts_inner::<0>(&w, budget, &[], Some((&m, &c, 0)), &|_| {});
            let expected = resolved_cache(&e, &w);
            let state = saved(&e);
            e.read_contacts_with(&w, budget, &[], Some((&m, &c, 0)));
            assert_eq!(
                resolved_cache(&e, &w),
                expected,
                "edge={edge} budget={budget}"
            );
            assert_eq!(saved(&e), state);
            assert!(e.solid.iter().all(|v| v.2 == CONFINED_SOIL));
            assert!(e.solid_mats.is_empty());
        }
        w.queue_edit(voxel::Edit::put(1, 1, 1, 65535));
        w.flush();
        e.read_contacts_with(&w, 1, &[], Some((&m, &c, 0)));
        assert_ne!(e.solid[0].2, CONFINED_SOIL);
    }
}

#[test]
#[ignore = "manual packed-soil read benchmark; run alone in release"]
fn measure_perforated_soil_read() {
    use std::{hint::black_box, time::Instant};
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    for edge in [2, 4, 9, 16] {
        let (w, mut e, c, m) = perforated_soil_fixture(edge);
        let mut samples = [Vec::new(), Vec::new()];
        for repeat in 0..17 {
            for policy in if repeat % 2 == 0 { [0, 1] } else { [1, 0] } {
                let start = Instant::now();
                pool.install(|| {
                    if policy == 0 {
                        e.read_contacts_inner::<0>(&w, 65536, &[], Some((&m, &c, 0)), &|_| {});
                    } else {
                        e.read_contacts_with(&w, 65536, &[], Some((&m, &c, 0)));
                    }
                });
                if repeat > 1 {
                    samples[policy].push(start.elapsed().as_nanos());
                }
                black_box((&e.solid, &e.solid_mats));
            }
        }
        for (policy, ns) in samples.iter_mut().enumerate() {
            ns.sort_unstable();
            println!(
                "PERFORATED edge={edge} policy={policy} median_ns={} min_ns={} max_ns={}",
                ns[7], ns[0], ns[14]
            );
        }
    }
}

#[test]
fn confined_mixed_soil_and_holes_preserve_ordered_read_prefix() {
    struct Soil;
    impl Generator for Soil {
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            [0, 1, 7, 8][((x + y + z) % 4) as usize]
        }
        fn material_candidates(&self, _: [u32; 3], _: [u32; 3]) -> Option<Vec<u16>> {
            Some(vec![0, 1, 7, 8])
        }
    }
    let mut w = World::new(96, 96, 96, 100, Box::new(Soil));
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    c.material_energy_j = 0;
    let m = matter::Matter::new(&[]);
    for edited in [false, true] {
        if edited {
            w.queue_edit(voxel::Edit::put(9, 9, 9, 6));
            w.flush();
        }
        for edge in [2, 4, 9, 16] {
            c.cell_units = edge;
            let mut e = Explosion::begin([48, 80, 48], &c, &[]);
            e.origin = [8; 3];
            e.soil_plane_y = Some(80);
            e.pending.extend((0..4).map(|x| phys::blast::Hit {
                cell: [x, 0, 0],
                pressure: 1000,
                broken: true,
            }));
            for nuclear in [false, true] {
                e.nuclear_ground = nuclear;
                for pressure in [1000, 1_000_000] {
                    for hit in &mut e.pending {
                        hit.pressure = pressure;
                    }
                    for budget in [0, 1, 3, 65536] {
                        e.read_contacts_inner::<0>(&w, budget, &[], Some((&m, &c, 0)), &|_| {});
                        let expected = resolved_cache(&e, &w);
                        let state = saved(&e);
                        e.read_contacts_with(&w, budget, &[], Some((&m, &c, 0)));
                        assert_eq!(resolved_cache(&e, &w), expected,
                            "edited={edited} edge={edge} nuclear={nuclear} pressure={pressure} budget={budget}");
                        assert_eq!(saved(&e), state);
                        if !edited && pressure == 1000 && budget > 0 {
                            assert!(e.solid.iter().all(|v| v.2 == CONFINED_SOIL));
                            assert!(e.solid_mats.is_empty());
                        } else if pressure == 1_000_000 {
                            assert!(e.solid.iter().all(|v| v.2 != CONFINED_SOIL));
                        } else if edited && budget > 0 {
                            assert_ne!(e.solid[0].2, CONFINED_SOIL);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn clipped_uniform_payloads_keep_mask_offsets_and_avoid_column_decoding() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct Soil(Arc<AtomicUsize>);
    impl Generator for Soil {
        fn material(&self, _: u32, y: u32, _: u32) -> u16 {
            if y < 15 {
                300
            } else {
                0
            }
        }
        fn batches_columns(&self) -> bool {
            true
        }
        fn column(&self, x: u32, z: u32, y: u32, out: &mut [u16]) {
            self.0.fetch_add(1, Ordering::Relaxed);
            for (i, mat) in out.iter_mut().enumerate() {
                *mat = self.material(x, y + i as u32, z);
            }
        }
        fn uniform_box(&self, _: [u32; 3], hi: [u32; 3]) -> Option<u16> {
            (hi[1] <= 15).then_some(300)
        }
    }
    let columns = Arc::new(AtomicUsize::new(0));
    let mut w = World::new(160, 96, 160, 100, Box::new(Soil(columns.clone())));
    for edited in [false, true] {
        if edited {
            w.queue_edit(voxel::Edit::dig_if(33, 8, 33, 300));
            w.queue_edit(voxel::Edit::put(65, 9, 65, 65535));
            w.flush();
        }
        for (edge, origin) in [
            (9, [-2, -2, -2]),
            (64, [-8, -49, -8]),
            (96, [0, -81, 0]),
            (96, [120, -81, 120]),
        ] {
            // Authored cells are bounded at64. Larger rectangles exercise
            // the uniform mask builder; mixed fallback coverage stays in
            // the supported authored range.
            if edited && edge > 64 {
                continue;
            }
            let offsets = subcell_offsets(edge);
            let mut expected = vec![42];
            let mut mask = contact_mask::Mask::for_voxels(offsets.len());
            for (index, offset) in offsets.iter().enumerate() {
                let mat = material(&w, std::array::from_fn(|a| origin[a] + offset[a]));
                if mat != 0 {
                    mask.insert(index);
                    expected.push(mat);
                }
            }
            columns.store(0, Ordering::Relaxed);
            let mut actual = vec![42];
            let got = read_cell_into(&w, origin, edge, [0; 3], &offsets, &mut actual);
            assert!(got == mask, "edited={edited} edge={edge} origin={origin:?}");
            assert_eq!(actual, expected);
            if !edited {
                assert_eq!(columns.load(Ordering::Relaxed), 0);
            }
        }
    }
}

#[test]
fn coarse_material_sets_preserve_cost_refusal_for_all_material_ids() {
    let all: Vec<_> = (0..=u16::MAX).rev().chain(0..=u16::MAX).collect();
    assert_eq!(material_kinds(&all), (0..=u16::MAX).collect::<Vec<_>>());
    let matter = matter::Matter::new(&[]);
    let mut c =
        ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    for source in [
        vec![],
        vec![1],
        vec![1, 7, 8],
        vec![2, 4, 5],
        vec![1, 2, 3, 7, 8],
    ] {
        let mats = source.repeat(1024);
        let kinds = material_kinds(&mats);
        for finite in [0, 1] {
            c.material_energy_j = finite;
            for pressure in [0, 40, 60, 1000, 1_000_000] {
                for left in [0, 1, 1000, u64::MAX] {
                    for (depth, top) in [([0, 80], 15), ([16, 16], 79)] {
                        assert_eq!(
                            spent_cell(&matter, &c, pressure, &kinds, left, depth, top),
                            spent_cell(&matter, &c, pressure, &mats, left, depth, top)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn compact_material_sets_match_wide_reader_at_word_boundaries() {
    for input in [
        vec![],
        vec![0],
        vec![63, 1, 0, 63, 1],
        vec![64, 63, 64, 65535, 0],
        (0..64).rev().collect(),
        [vec![1; 1000], vec![65535, 64, 1, 0]].concat(),
    ] {
        assert_eq!(material_kinds(&input), material_kinds_wide(&input, 0));
    }
}

#[test]
#[ignore = "manual material-set preparation benchmark; run alone in release"]
fn measure_material_kind_preparation() {
    use std::{hint::black_box, time::Instant};
    for count in [729, 4096, 262144] {
        for high in [false, true] {
            let mut mats: Vec<_> = (0..count).map(|i| [1, 7, 8][i % 3]).collect();
            if high {
                mats[count / 2] = 65535;
            }
            let repeats = (1_000_000 / count).max(1);
            let mut times = [Vec::new(), Vec::new()];
            for repeat in 0..9 {
                for compact in if repeat % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let start = Instant::now();
                    for _ in 0..repeats {
                        black_box(if compact {
                            material_kinds(black_box(&mats))
                        } else {
                            material_kinds_wide(black_box(&mats), 0)
                        });
                    }
                    times[usize::from(compact)].push(start.elapsed().as_nanos() / repeats as u128);
                }
            }
            for (compact, ns) in times.iter_mut().enumerate() {
                ns.sort_unstable();
                println!("KINDS count={count} high={high} compact={compact} median_ns={} min_ns={} max_ns={}", ns[4], ns[0], ns[8]);
            }
        }
    }
}

#[test]
fn uniform_contact_boxes_match_scalar_after_holes_and_foreign_material_edits() {
    struct Uniform(u16);
    impl Generator for Uniform {
        fn material(&self, _: u32, y: u32, _: u32) -> u16 {
            if y < 96 {
                self.0
            } else {
                0
            }
        }
        fn uniform_box(&self, _: [u32; 3], max: [u32; 3]) -> Option<u16> {
            (max[1] <= 96).then_some(self.0)
        }
    }
    for mat in [1, 300, 65535] {
        let mut w = World::new(140, 128, 140, 100, Box::new(Uniform(mat)));
        for edited in [false, true] {
            if edited {
                w.queue_edit(voxel::world::Edit::put(33, 33, 33, 0));
                w.queue_edit(voxel::world::Edit::put(65, 31, 65, 6));
                w.flush();
            }
            for edge in [1, 2, 4, 9, 16, 64] {
                let offsets = subcell_offsets(edge);
                for origin in [
                    [1, 1, 1],
                    [29, 29, 29],
                    [63, 30, 63],
                    [120, 90, 120],
                    [-2, -2, -2],
                ] {
                    let mut expected = Vec::new();
                    let mut mask = contact_mask::Mask::default();
                    for (i, q) in offsets.iter().enumerate() {
                        let m = material(&w, std::array::from_fn(|a| origin[a] + q[a]));
                        if m != 0 {
                            mask.insert(i);
                            expected.push(m);
                        }
                    }
                    let mut actual = vec![42];
                    let got = read_cell_into(&w, origin, edge, [0; 3], &offsets, &mut actual);
                    assert_eq!(
                        got, mask,
                        "mat={mat} edited={edited} edge={edge} origin={origin:?}"
                    );
                    assert_eq!(actual[0], 42);
                    assert_eq!(&actual[1..], expected);
                }
            }
        }
    }
}

// Compare disposable read caches by resolved contacts, not by whether an
// unaffordable cell needed a material payload during read-ahead.
fn resolved_cache(
    e: &Explosion,
    w: &World,
) -> (Vec<([i32; 3], contact_mask::Mask, u32)>, Vec<u16>) {
    let mut cells = Vec::new();
    let mut mats = Vec::new();
    let offsets = subcell_offsets(e.cell);
    for (cell, stored_mask, start) in &e.solid {
        let mut mask = stored_mask.clone();
        let base = mats.len() as u32;
        if *start == CONFINED_SOIL {
            // Nuclear payloads omit confined soil voxel by voxel too.
            if !e.nuclear_ground {
                for (i, o) in offsets.iter().enumerate() {
                    let q = std::array::from_fn(|a| e.origin[a] + cell[a] * e.cell + o[a]);
                    let mat = material(w, q);
                    if mat != 0 {
                        mask.insert(i);
                        mats.push(mat);
                    }
                }
            }
        } else if *start == UNPRICED {
            for i in mask.indices_from(0) {
                let q = std::array::from_fn(|a| e.origin[a] + cell[a] * e.cell + offsets[i][a]);
                mats.push(material(w, q));
            }
        } else {
            let start = *start as usize;
            mats.extend_from_slice(&e.solid_mats[start..start + mask.count_ones() as usize]);
        }
        cells.push((*cell, mask, base));
    }
    (cells, mats)
}

#[test]
fn confined_uniform_read_preserves_cells_hulls_and_falls_back_when_pressure_changes() {
    struct Soil;
    impl Generator for Soil {
        fn material(&self, _: u32, y: u32, _: u32) -> u16 {
            if y < 96 {
                1
            } else {
                0
            }
        }
        fn uniform_box(&self, _: [u32; 3], max: [u32; 3]) -> Option<u16> {
            (max[1] <= 96).then_some(1)
        }
    }
    let mut w = World::new(160, 128, 160, 100, Box::new(Soil));
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/siege-shell.ron")).unwrap();
    c.half_cells = 4;
    let units = [RangeUnit::of_kind(RangeUnitKind::Tank, [18, 17, 18])];
    let mut e = Explosion::begin([80, 95, 80], &c, &units);
    e.origin = [8; 3];
    e.point_contacts = false;
    let m = matter::Matter::new(&[]);
    e.pending.extend((0..4).map(|x| phys::blast::Hit {
        cell: [x, 1, 1],
        pressure: 1000,
        broken: true,
    }));
    let offsets = subcell_offsets(e.cell);
    let body_contacts = |e: &Explosion| {
        let mut contacts = Vec::new();
        let n = e.wave.size();
        for (cell, _, _) in &e.solid {
            let index = ((cell[2] as usize * n + cell[1] as usize) * n + cell[0] as usize) as u32;
            for (actor, map) in e.body_maps.iter().enumerate() {
                if let Some(map) = map {
                    let (start, count) = map.range(index);
                    for k in start..start + count {
                        contacts.push((actor, index, map.contact(k)));
                    }
                }
            }
        }
        contacts
    };
    for edited in [false, true] {
        if edited {
            w.queue_edit(voxel::world::Edit::put(18, 18, 18, 6));
            w.queue_edit(voxel::world::Edit::put(9, 18, 18, 0));
            w.flush();
        }
        for budget in [0, 1, 2, 4, 65536] {
            e.read_contacts_inner::<0>(&w, budget, &units, Some((&m, &c, u64::MAX)), &|_| {});
            let expected = resolved_cache(&e, &w);
            let maps = body_contacts(&e);
            if budget >= 4 {
                assert!(!maps.is_empty(), "the hull must receive contacts");
            }
            let saved_before = saved(&e);
            e.body_maps.clear();
            e.read_contacts_with(&w, budget, &units, Some((&m, &c, u64::MAX)));
            assert_eq!(
                resolved_cache(&e, &w),
                expected,
                "edited={edited} budget={budget}"
            );
            assert_eq!(saved(&e), saved_before);
            assert_eq!(body_contacts(&e), maps);
            if !edited && budget > 0 {
                assert_eq!(e.read_work.confined_cells, e.solid.len());
                assert!(e.solid_mats.is_empty());
            }
        }
    }
    // Start the next read on the unchanged portion of the edited chunk.
    // Mixed contacts at the front legitimately use an ordinary material batch.
    e.pending.rotate_left(2);
    e.read_contacts_with(&w, 65536, &units, Some((&m, &c, u64::MAX)));
    let cell = e
        .solid
        .iter()
        .find(|(_, _, start)| *start == CONFINED_SOIL)
        .unwrap()
        .0;
    let (mask, start) = e.solid_cell(&w, cell, &offsets, false, true);
    assert_eq!(start, CONFINED_SOIL as usize);
    assert!(e.ground_silent(
        &c,
        1000,
        &mask,
        start,
        std::array::from_fn(|a| e.origin[a] + cell[a] * e.cell)
    ));
    let (mask, start) = e.solid_cell(&w, cell, &offsets, false, false);
    assert_ne!(start, CONFINED_SOIL as usize);
    assert_eq!(mask.count_ones() as usize, offsets.len());
    assert!(e.solid_mats[start..start + offsets.len()]
        .iter()
        .all(|&mat| mat == 1));
}

#[test]
fn estimated_exhaustion_keeps_parallel_material_payload_for_affordable_live_share() {
    let (w, mut e) = fixture(9, 128);
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    c.material_energy_j = 1;
    let m = matter::Matter::new(&[]);
    let energy = unpaid_bounds(&m, &c, &w, e.domain()).unwrap().0 * 2;
    e.read_contacts_with(&w, 65536, &[], Some((&m, &c, energy)));
    assert!(
        e.solid.len() > 1,
        "estimate did not reach its exhausted suffix"
    );
    assert!(e.solid.iter().all(|(_, _, start)| *start != UNPRICED));
    assert!(!e.solid_mats.is_empty());
}

#[test]
fn occupancy_only_contacts_fall_back_for_live_energy_and_weak_pressure() {
    let (w, mut e) = fixture(9, 128);
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    c.material_energy_j = 1;
    let mut m = matter::Matter::new(&[]);
    e.read_contacts_with(&w, 65536, &[], Some((&m, &c, 0)));
    let saved_before = saved(&e);
    assert!(e.solid.iter().any(|(_, _, start)| *start == UNPRICED));
    let cells: Vec<_> = e.solid.iter().map(|(cell, _, _)| *cell).collect();
    let offsets = subcell_offsets(9);
    for i in [0, cells.len() - 1, 1] {
        let cell = cells[i];
        let (mask, start) = e.solid_cell(&w, cell, &offsets, true, false);
        assert_eq!(start, UNPRICED as usize);
        assert!(e.ground_spent(
            Some(&m),
            &c,
            100_000,
            &mask,
            start,
            (e.origin[1] + cell[1] * 9).max(0)
        ));
        let mut expected = Vec::new();
        let want = read_cell_into(&w, e.origin, 9, cell, &offsets, &mut expected);
        let (mask, start) = e.solid_cell(&w, cell, &offsets, false, false);
        assert_eq!(mask, want);
        assert_eq!(&e.solid_mats[start..start + expected.len()], expected);
        m.remaining_mj = 1_000_000_000_000;
        assert!(!e.ground_spent(
            Some(&m),
            &c,
            100_000,
            &mask,
            start,
            (e.origin[1] + cell[1] * 9).max(0)
        ));
        m.remaining_mj = 0;
        assert!(!e.ground_spent(
            Some(&m),
            &c,
            1,
            &mask,
            start,
            (e.origin[1] + cell[1] * 9).max(0)
        ));
    }
    assert_eq!(saved(&e), saved_before);
}

#[test]
fn bulk_unpaid_contacts_match_scalar_frame_and_resume_counters() {
    struct Air;
    impl Generator for Air {
        fn material(&self, _: u32, _: u32, _: u32) -> u16 {
            0
        }
    }
    let w = World::new(32, 24, 40, 100, Box::new(Air));
    let mut actual = matter::Matter::new(&[]);
    let mut reference = matter::Matter::new(&[]);
    for edge in [1, 2, 4, 9, 16] {
        for stride in [1, 5] {
            let mut mask = contact_mask::Mask::for_voxels((edge as usize).pow(3));
            for i in (0..(edge as usize).pow(3)).step_by(stride) {
                mask.insert(i);
            }
            let offsets = subcell_offsets(edge);
            for p in [
                [-3, -1, -2],
                [0, 0, 0],
                [5, 3, 7],
                [30, 22, 38],
                [31, 23, 39],
            ] {
                for from in [0, 1, 63, 128, (edge as usize).pow(3)] {
                    actual.admission.ground.fill(0);
                    reference.admission.ground.fill(0);
                    for i in mask.indices_from(from) {
                        reference.count_unpaid(&w, std::array::from_fn(|a| p[a] + offsets[i][a]));
                    }
                    actual.count_unpaid_mask(&w, p, edge, &mask, from);
                    assert_eq!(
                        actual.admission.ground, reference.admission.ground,
                        "edge={edge} stride={stride} p={p:?} from={from}"
                    );
                }
            }
        }
    }
}

#[test]
fn heavy_pressure_cells_destroy_ground_beyond_the_old_radius() {
    struct Flat;
    impl Generator for Flat {
        fn material(&self, _: u32, y: u32, _: u32) -> u16 {
            if y < 16 {
                1
            } else {
                0
            }
        }
    }
    let c = ExplosionConfig::from_ron(include_str!("../../../data/siege-shell.ron")).unwrap();
    let mut world = World::new(1024, 64, 1024, 4096, Box::new(Flat));
    let mut burst = Explosion::with_damage(&world, [512, 15, 512], &c, &[], &SurfaceWork::new());
    assert_eq!(burst.cell, 9);
    assert_eq!(burst.wave.size(), 65);
    let target = [722, 15, 512]; // 210 > the former 128-unit window.
    let mut dents = SurfaceWork::new();
    let mut changed = Vec::new();
    while !burst.wave.finished() {
        burst.wave.advance_contacts(65536);
    }
    let cell = std::array::from_fn(|a| (target[a] - burst.origin[a]) / burst.cell);
    let pressure = burst.wave.pressure_at(cell.map(|v| v as usize));
    assert!(
        pressure >= c.soil_resistance,
        "field never reached beyond the old radius"
    );
    // Isolate this real frontier contact; draining millions of other terrain
    // edits would measure the whole crater rather than this regression.
    burst.pending.push_back(phys::blast::Hit {
        cell,
        pressure,
        broken: true,
    });
    burst.step(
        &mut world,
        65536,
        &mut [],
        &mut dents,
        &mut changed,
        &c,
        None,
    );
    world.flush();
    assert_eq!(
        material(&world, target),
        0,
        "strong wave was clipped to the old radius"
    );
}

struct Pattern;
impl Generator for Pattern {
    fn material_candidates(&self, _: [u32; 3], _: [u32; 3]) -> Option<Vec<u16>> {
        Some(vec![0, 1, 2])
    }
    fn batches_columns(&self) -> bool {
        true // Exercise both read algorithms against the scalar oracle.
    }
    fn material(&self, x: u32, y: u32, z: u32) -> u16 {
        if (x + 3 * y + 5 * z).is_multiple_of(3) {
            1 + ((x + y + z) % 2) as u16
        } else {
            0
        }
    }
}

fn fixture(edge: i32, count: usize) -> (World, Explosion) {
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    c.cell_units = edge;
    c.half_cells = 32;
    let side = (65 * edge + 16).max(320) as u32;
    let w = World::new(side, side, side, 100, Box::new(Pattern));
    let mut e = Explosion::begin([32 * edge; 3], &c, &[]);
    // Make the ring genuinely wrap, rather than testing only its first slice.
    e.pending.reserve(count);
    let hit = |index: usize| phys::blast::Hit {
        cell: [
            (index % 65) as i32,
            (index / 65 % 65) as i32,
            (index / (65 * 65)) as i32,
        ],
        pressure: 100_000,
        broken: true,
    };
    e.pending.extend((0..count).map(hit));
    for index in 0..count / 3 {
        e.pending.pop_front();
        e.pending.push_back(hit(index));
    }
    (w, e)
}

fn saved(e: &Explosion) -> Vec<u8> {
    let mut b = fx::blob::Blob::new();
    e.save(&mut b);
    e.save_contacts(&mut b);
    e.save_resumes(&mut b);
    b.v
}

#[test]
fn read_ahead_matches_scalar_live_material_and_preserves_wrapped_contacts() {
    for edge in [1, 2, 4, 8, 9, 12, 16] {
        let (mut w, mut e) = fixture(edge, 128);
        // Keep edited samples off the world's immutable outer frame.
        // The scaling probe below deliberately keeps its original fixture.
        e.origin = e.origin.map(|v| v + 8);
        e.at = e.at.map(|v| v + 8);
        assert!(!e.pending.as_slices().1.is_empty());
        let original = saved(&e);
        for changed in [false, true] {
            if changed {
                let cell = e.pending.front().unwrap().cell;
                let p = std::array::from_fn::<_, 3, _>(|a| e.origin[a] + cell[a] * edge);
                w.queue_edit(voxel::world::Edit::put(
                    p[0] as u32,
                    p[1] as u32,
                    p[2] as u32,
                    6,
                ));
                w.flush();
                assert_eq!(material(&w, p), 6, "live edit was not applied");
            }
            for budget in [usize::MAX, 0, 1, 2, 5, 64, 65, 129] {
                // Independent scalar reference: no occupancy shortcut or read cache.
                let mut expected = Vec::new();
                let mut mats = Vec::new();
                let mut left = budget;
                for hit in &e.pending {
                    if left == 0 {
                        break;
                    }
                    let mut mask = contact_mask::Mask::default();
                    let start = mats.len() as u32;
                    for (index, offset) in subcell_offsets(edge).iter().enumerate() {
                        let p =
                            std::array::from_fn(|a| e.origin[a] + hit.cell[a] * edge + offset[a]);
                        let mat = material(&w, p);
                        if mat != 0 {
                            mask.insert(index);
                            mats.push(mat);
                        }
                    }
                    left = left.saturating_sub(1 + mask.count_ones() as usize);
                    expected.push((hit.cell, mask, start));
                }
                for policy in 0..=2 {
                    match policy {
                        0 => e.read_contacts_inner::<0>(&w, budget, &[], None, &|_| {}),
                        1 => e.read_contacts(&w, budget),
                        _ => e.read_contacts_inner::<2>(&w, budget, &[], None, &|_| {}),
                    }
                    assert_eq!(
                        e.solid, expected,
                        "edge={edge} budget={budget} changed={changed} policy={policy}"
                    );
                    assert_eq!(e.solid_mats, mats);
                    assert_eq!(e.solid_next, 0);
                    assert_eq!(saved(&e), original, "read-ahead changed saved work");
                }
            }
        }
    }
}

#[test]
fn empty_coarse_occupancy_does_not_allocate_wide_masks() {
    struct Sparse;
    impl Generator for Sparse {
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            u16::from(x == 80 && y == 31 && z == 60)
        }
    }
    let mut w = World::new(128, 64, 128, 100, Box::new(Sparse));
    for p in [[0, 0, 0], [-64, 0, 0], [0, -64, 0], [128, 0, 0], [0, 64, 0]] {
        assert!(matches!(
            read_occupancy(&w, p, 64, [0; 3]),
            contact_mask::Mask::Small(0)
        ));
    }
    let mask = read_occupancy(&w, [64, 0, 0], 64, [0; 3]);
    assert_eq!(mask.count_ones(), 1);
    assert_eq!(
        mask.indices_from(0).collect::<Vec<_>>(),
        [16 + 64 * (31 + 64 * 60)]
    );
    w.queue_edit(voxel::world::Edit::dig_if(80, 31, 60, 1));
    w.flush();
    assert!(matches!(
        read_occupancy(&w, [64, 0, 0], 64, [0; 3]),
        contact_mask::Mask::Small(0)
    ));
}

#[test]
fn exhausted_estimate_keeps_live_material_reads_physically_bounded() {
    struct Shallow;
    impl Generator for Shallow {
        fn material(&self, _: u32, y: u32, _: u32) -> u16 {
            u16::from(y < 2)
        }
        fn batches_columns(&self) -> bool {
            true
        }
    }
    let w = World::new(2048, 64, 64, 100, Box::new(Shallow));
    let mut c =
        ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    c.soil_depth_cost = 0;
    let mut e = Explosion::begin([1024, 0, 0], &c, &[]);
    e.origin = [0; 3];
    e.pending.extend((0..32).map(|x| phys::blast::Hit {
        cell: [x, 0, 0],
        pressure: 100_000,
        broken: true,
    }));
    let m = matter::Matter::new(&[]);
    // The estimate pays the first two voxels, but actual claims can defer.
    // That must not authorize one physical batch covering the remaining domain.
    let spent = Some((&m, &c, 2 * blast_ground_price(&m, &c, 1)));
    e.read_contacts_inner::<0>(&w, 65536, &[], spent, &|_| {});
    let expected = resolved_cache(&e, &w);
    let before = saved(&e);
    e.read_contacts_with(&w, 65536, &[], spent);
    assert_eq!(resolved_cache(&e, &w), expected);
    assert_eq!(saved(&e), before);
    assert_eq!(e.read_work.retained_cells, 32);
    assert_eq!(e.read_work.prepared_cells, 32);
    assert_eq!(e.read_work.prepared_solids, 32 * 8192);
    assert_eq!(e.read_work.peak_batch, 4);
}

#[test]
fn coarse_read_batches_preserve_energy_limited_prefixes() {
    let matter = matter::Matter::new(&[]);
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    for edge in [2, 9] {
        let (w, mut e) = fixture(edge, 128);
        for enabled in [false, true] {
            c.material_energy_j = u32::from(enabled);
            for energy in [0, 1000, 1_000_000_000_000] {
                // Weak contacts must not use a price-only proof: they can
                // still owe deformation work when their fracture is unpaid.
                for pressure in [1, 100_000] {
                    for hit in &mut e.pending {
                        hit.pressure = pressure;
                    }
                    for budget in [0, 1, 65, 129, 65536] {
                        let spent = Some((&matter, &c, energy));
                        e.read_contacts_inner::<0>(&w, budget, &[], spent, &|_| {});
                        let (solid, mats) = resolved_cache(&e, &w);
                        let state = saved(&e);
                        for policy in [1, 2] {
                            if policy == 1 {
                                e.read_contacts_with(&w, budget, &[], spent);
                            } else {
                                e.read_contacts_inner::<2>(&w, budget, &[], spent, &|_| {});
                            }
                            assert_eq!(resolved_cache(&e, &w), (solid.clone(), mats.clone()),
                                "edge={edge} enabled={enabled} energy={energy} budget={budget} policy={policy}");
                            assert_eq!(saved(&e), state);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn proven_air_read_keeps_the_same_body_contacts_and_ordered_visits() {
    struct Air;
    impl Generator for Air {
        fn material(&self, _: u32, _: u32, _: u32) -> u16 {
            0
        }
    }
    let world = World::new(128, 128, 128, 100, Box::new(Air));
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    c.energy = 1;
    c.cell_units = 9;
    c.half_cells = 8;
    let units = [RangeUnit::of_kind(RangeUnitKind::Tank, [42, 16, 42])];
    let mut e = Explosion::begin([64; 3], &c, &units);
    let n = e.wave.size();
    e.pending.extend((0..n.pow(3)).map(|i| phys::blast::Hit {
        cell: [(i % n) as i32, (i / n % n) as i32, (i / (n * n)) as i32],
        pressure: 100_000,
        broken: true,
    }));
    for budget in [0, 1, 64, 129, 65536] {
        e.body_maps.clear();
        e.read_contacts_inner::<0>(&world, budget, &units, None, &|_| {});
        let solid = e.solid.clone();
        let saved_work = saved(&e);
        let contacts = |e: &Explosion| {
            e.solid
                .iter()
                .flat_map(|(cell, _, _)| {
                    let index =
                        ((cell[2] as usize * n + cell[1] as usize) * n + cell[0] as usize) as u32;
                    e.body_maps.iter().flatten().flat_map(move |map| {
                        let (start, count) = map.range(index);
                        (start..start + count).map(move |k| (index, map.contact(k)))
                    })
                })
                .collect::<Vec<_>>()
        };
        let before = contacts(&e);
        if budget == 65536 {
            assert!(!before.is_empty(), "the body mapping must do work");
        }
        e.body_maps.clear();
        e.read_contacts_with(&world, budget, &units, None);
        assert_eq!(e.solid, solid);
        assert!(e.solid_mats.is_empty());
        assert_eq!(contacts(&e), before);
        assert_eq!(saved(&e), saved_work);
        assert_eq!(e.read_work.air_cells, e.solid.len());
    }
}

/// Compare physical preparation and discarded material in one release binary.
#[test]
#[ignore = "manual coarse contact read-ahead experiment; run alone"]
fn measure_coarse_contact_read_work() {
    use std::{hint::black_box, time::Instant};
    struct Ground {
        edge: u32,
        kind: u8,
    }
    impl Generator for Ground {
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            match self.kind {
                0 => 1,
                1 => 0,
                2 => u16::from((x + y + z).is_multiple_of(4)),
                _ => u16::from(z >= self.edge * 8),
            }
        }
    }
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    c.energy = 1;
    c.half_cells = 32;
    let matter = matter::Matter::new(&[]);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    println!("READ_PROBE kind,edge,budget,spent,policy,median_us,max_us,prepared_cells,retained_cells,prepared_solids,retained_solids,batches,peak_batch,air_cells");
    for edge in [2, 4, 9, 16] {
        c.cell_units = edge;
        for kind in 0..4 {
            let w = World::new(
                (65 * edge + 16) as u32,
                (4 * edge + 8) as u32,
                (33 * edge + 16) as u32,
                100,
                Box::new(Ground {
                    edge: edge as u32,
                    kind,
                }),
            );
            let mut e = Explosion::begin([32 * edge; 3], &c, &[]);
            e.origin = [8; 3];
            e.pending.extend((0..2048).map(|i| phys::blast::Hit {
                cell: [(i % 64) as i32, 0, (i / 64) as i32],
                pressure: 100_000,
                broken: true,
            }));
            for budget in [4096, 65536] {
                for spent in [false, true] {
                    c.material_energy_j = u32::from(spent);
                    // Reverse variant order on alternate cases to reduce drift.
                    let policies = if (kind + u8::from(spent)) % 2 == 0 {
                        [0, 1, 2]
                    } else {
                        [2, 1, 0]
                    };
                    for policy in policies {
                        let mut run = || match policy {
                            0 => e.read_contacts_inner::<0>(
                                &w,
                                budget,
                                &[],
                                Some((&matter, &c, 0)),
                                &|_| {},
                            ),
                            1 => e.read_contacts_inner::<1>(
                                &w,
                                budget,
                                &[],
                                Some((&matter, &c, 0)),
                                &|_| {},
                            ),
                            _ => e.read_contacts_inner::<2>(
                                &w,
                                budget,
                                &[],
                                Some((&matter, &c, 0)),
                                &|_| {},
                            ),
                        };
                        pool.install(&mut run);
                        let mut ns = Vec::new();
                        for _ in 0..7 {
                            let start = Instant::now();
                            pool.install(&mut run);
                            ns.push(start.elapsed().as_nanos());
                        }
                        ns.sort_unstable();
                        black_box((&e.solid, &e.solid_mats));
                        let a = e.read_work;
                        println!("READ_PROBE {kind},{edge},{budget},{spent},{policy},{:.3},{:.3},{},{},{},{},{},{},{}", ns[3] as f64 / 1000.0, ns[6] as f64 / 1000.0, a.prepared_cells, a.retained_cells, a.prepared_solids, a.retained_solids, a.batches, a.peak_batch, a.air_cells);
                    }
                }
            }
        }
    }
}

/// Run alone in release. This fixture isolates queue preparation, not a game tick.
#[test]
#[ignore = "manual read-ahead scaling measurement; run alone in release"]
fn measure_read_ahead_queue_scaling() {
    use std::hint::black_box;
    use std::time::Instant;
    println!("pending,budget,repeats,median_us,min_us,max_us,read_cells");
    for count in [64, 32_768, 137_065] {
        let (w, mut e) = fixture(2, count);
        for (budget, repeats) in [(0, 2000), (1, 2000), (64, 500), (4096, 20)] {
            for _ in 0..8 {
                e.read_contacts(&w, budget);
            }
            let mut times = Vec::new();
            for _ in 0..9 {
                let start = Instant::now();
                for _ in 0..repeats {
                    e.read_contacts(black_box(&w), black_box(budget));
                    black_box((&e.solid, &e.solid_mats));
                }
                times.push(start.elapsed().as_secs_f64() * 1e6 / f64::from(repeats));
            }
            times.sort_by(f64::total_cmp);
            println!(
                "{count},{budget},{repeats},{:.6},{:.6},{:.6},{}",
                times[4],
                times[0],
                times[8],
                e.solid.len()
            );
        }
    }
}

#[test]
fn coarse_contact_materials_match_dense_after_mixed_overlay_edits() {
    struct Pattern;
    impl Generator for Pattern {
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            if y < 41 && (x + y + z) % 11 != 0 {
                1 + (x % 3) as u16
            } else {
                0
            }
        }
        fn batches_columns(&self) -> bool {
            true
        }
    }
    let mut w = World::new(99, 80, 99, 65536, Box::new(Pattern));
    // Leave pristine spans beside the overlay, then commit all touched
    // chunks; compare complete occupancy and canonical material order.
    for all in [false, true] {
        for z in (0..99).step_by(32) {
            for y in (0..80).step_by(32) {
                for x in (0..99).step_by(32) {
                    if all || (x == 32 && y == 32 && z == 32) {
                        w.queue_edit(voxel::Edit::put(x + 1, y + 1, z + 1, 65535));
                    }
                }
            }
        }
        if all {
            for i in 0..300 {
                w.queue_edit(voxel::Edit::put(i % 32, i / 32, 1, 1000 + i as u16));
            }
        }
        for committed in [false, true] {
            if committed {
                w.flush();
            }
            for edge in [2, 3, 4, 9, 16, 64] {
                let offsets = subcell_offsets(edge);
                for origin in [
                    [1, 1, 1],
                    [29, 29, 29],
                    [63, 30, 63],
                    [-2, -2, -2],
                    [92, 75, 92],
                ] {
                    let mut expected = vec![42];
                    let mut mask = contact_mask::Mask::for_voxels((edge as usize).pow(3));
                    for (index, offset) in offsets.iter().enumerate() {
                        let p = std::array::from_fn(|a| origin[a] + offset[a]);
                        let mat = material(&w, p);
                        if mat != 0 {
                            mask.insert(index);
                            expected.push(mat);
                        }
                    }
                    let mut actual = vec![42];
                    let got = read_cell_into(&w, origin, edge, [0; 3], &offsets, &mut actual);
                    assert_eq!(
                        got, mask,
                        "all={all} committed={committed} edge={edge} origin={origin:?}"
                    );
                    assert_eq!(actual, expected);
                }
            }
        }
    }
}
