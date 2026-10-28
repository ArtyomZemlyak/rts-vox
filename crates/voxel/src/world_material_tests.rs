use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn edited_material_boxes_do_not_require_a_procedural_certificate() {
    struct Unknown(Arc<AtomicUsize>);
    impl Generator for Unknown {
        fn material(&self, _: u32, _: u32, _: u32) -> Mat {
            300
        }
        fn uniform_box(&self, _: [u32; 3], _: [u32; 3]) -> Option<Mat> {
            self.0.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let mut w = World::new(64, 64, 64, 65536, Box::new(Unknown(calls.clone())));
    assert!(!w.box_is_material_or_air([0; 3], [64; 3], 300));
    for z in [1, 33] {
        for y in [1, 33] {
            for x in [1, 33] {
                w.queue_edit(Edit::put(x, y, z, 65535));
            }
        }
    }
    while w.flush().queued != 0 {}
    for z in [1, 33] {
        for y in [1, 33] {
            for x in [1, 33] {
                w.queue_edit(Edit::put(x, y, z, 300));
            }
        }
    }
    w.queue_edit(Edit::put(2, 2, 2, 0));
    w.flush();
    calls.store(0, Ordering::Relaxed);
    assert!(w.box_is_material_or_air([0; 3], [64; 3], 300));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    w.queue_edit(Edit::put(40, 40, 40, 65535));
    assert!(w.box_is_material_or_air([0; 3], [64; 3], 300));
    w.flush();
    assert!(!w.box_is_material_or_air([0; 3], [64; 3], 300));
    assert!(w.box_is_material_or_air([0; 3], [32; 3], 300));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[test]
fn material_with_air_certificate_reads_live_overlay_and_clips_chunks() {
    struct Known;
    impl Generator for Known {
        fn material(&self, _: u32, _: u32, _: u32) -> Mat {
            300
        }
        fn uniform_box(&self, _: [u32; 3], _: [u32; 3]) -> Option<Mat> {
            Some(300)
        }
    }
    let mut w = World::new(99, 67, 97, 65536, Box::new(Known));
    for stage in 0..4 {
        match stage {
            1 => {
                for z in (0..97).step_by(7) {
                    for x in (0..99).step_by(5) {
                        w.queue_edit(Edit::put(x, 31, z, 0));
                    }
                }
                w.flush();
            }
            2 => w.queue_edit(Edit::put(33, 31, 33, 65535)),
            3 => {
                w.flush();
            }
            _ => {}
        }
        for min in [[0, 0, 0], [1, 30, 2], [31; 3], [33, 31, 33], [65, 60, 65]] {
            for edge in [1, 2, 9, 32, 64, 99] {
                let size = [w.nx, w.ny, w.nz];
                let max = std::array::from_fn(|a| (min[a] + edge).min(size[a]));
                let expected = (min[1]..max[1]).all(|y| {
                    (min[2]..max[2])
                        .all(|z| (min[0]..max[0]).all(|x| [0, 300].contains(&w.material(x, y, z))))
                });
                assert_eq!(
                    w.box_is_material_or_air(min, max, 300),
                    expected,
                    "stage={stage} min={min:?} max={max:?}"
                );
            }
        }
    }
    assert!(!w.box_is_material_or_air([0; 3], [0; 3], 300));
    assert!(!w.box_is_material_or_air([0; 3], [100; 3], 300));
    assert!(!w.box_is_material_or_air([0; 3], [2; 3], 1));
}

// Original append/sort reader, independent oracle for wide material queries.
fn candidate_reference(w: &World, min: [u32; 3], max: [u32; 3]) -> Option<Vec<Mat>> {
    let size = [w.nx, w.ny, w.nz];
    if (0..3).any(|a| min[a] >= max[a] || max[a] > size[a]) {
        return None;
    }
    let mut out = w.gen.material_candidates(min, max)?;
    for z in min[2] / CS as u32..=(max[2] - 1) / CS as u32 {
        for y in min[1] / CS as u32..=(max[1] - 1) / CS as u32 {
            for x in min[0] / CS as u32..=(max[0] - 1) / CS as u32 {
                if let Some(chunk) =
                    &w.chunks[w.chunk_index(x * CS as u32, y * CS as u32, z * CS as u32)]
                {
                    out.extend_from_slice(chunk.material_candidates());
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    Some(out)
}

fn candidate_fixture(palette_size: usize) -> World {
    struct Known;
    impl Generator for Known {
        fn material(&self, _: u32, _: u32, _: u32) -> Mat {
            0
        }
        fn material_candidates(&self, _: [u32; 3], _: [u32; 3]) -> Option<Vec<Mat>> {
            Some(vec![65535, 1, 0, 1])
        }
    }
    let mut w = World::new(512, 32, 512, 100, Box::new(Known));
    // Material-query-only fixture: palette content is what the certificate
    // unions; occupancy is not consulted.
    for (i, slot) in w.chunks.iter_mut().enumerate() {
        let mut chunk = Paletted::new();
        for j in 0..palette_size {
            let stride = if palette_size > 8 { 509 } else { 1 };
            chunk.set(j % 32, j / 32, 0, (j * stride + i % 3) as u16);
        }
        *slot = Some(Arc::new(chunk));
    }
    w
}

#[test]
fn wide_material_candidates_match_palette_union_at_partial_chunk_edges() {
    let w = candidate_fixture(128);
    for min in [[0, 0, 0], [31, 3, 31], [129, 4, 67], [503, 2, 497]] {
        for edge in [1, 9, 32, 33, 64, 99, 512] {
            let size = [w.nx, w.ny, w.nz];
            let max = std::array::from_fn(|a| (min[a] + edge).min(size[a]));
            assert_eq!(
                w.material_candidates(min, max),
                candidate_reference(&w, min, max)
            );
        }
    }
}

#[test]
#[ignore = "manual material-candidate timing, no CI threshold"]
fn measure_material_candidate_union() {
    use std::{hint::black_box, time::Instant};
    for palette_size in [8, 128] {
        let w = candidate_fixture(palette_size);
        for (edge, iterations) in [(9, 20_000), (64, 5_000), (96, 5_000), (512, 500)] {
            let (min, max) = ([0; 3], [edge, edge.min(32), edge]);
            for reference in [true, false] {
                let start = Instant::now();
                for _ in 0..iterations {
                    black_box(if reference {
                        candidate_reference(black_box(&w), min, max)
                    } else {
                        w.material_candidates(min, max)
                    });
                }
                println!(
                    "palette={palette_size} edge={edge} reference={reference} ns_per_query={}",
                    start.elapsed().as_nanos() / iterations
                );
            }
        }
    }
}

#[test]
fn navigation_rows_match_scalar_tiles_after_edits_profiles_and_reset() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut w = world(&calls);
    let width = w.nx.div_ceil(8);
    let rows = w.nz.div_ceil(8);
    for stage in 0..4 {
        match stage {
            1 => w.prepare_navigation([4, 5]),
            2 => {
                w.queue_edit(Edit::put(32, 60, 32, 65535));
                w.queue_edit(Edit::dig(3, 34, 5));
                w.flush();
            }
            3 => w.restore_pristine(),
            _ => {}
        }
        for profile in [[4, 5], [2, 7], [0, u16::MAX], [4, 5]] {
            for z in 0..rows {
                for x in 0..=width {
                    for count in [0, (width - x).min(1), width - x] {
                        let row: Vec<_> = w
                            .floor_tile_row(x, z, count as usize, profile)
                            .unwrap()
                            .collect();
                        let expected: Vec<_> = (x..x + count)
                            .map(|tx| w.floor_tile(tx, z, profile).unwrap())
                            .collect();
                        assert_eq!(
                            row, expected,
                            "stage={stage} profile={profile:?} x={x} z={z}"
                        );
                    }
                }
            }
        }
    }
    assert!(w.floor_tile_row(width, 0, 1, [4, 5]).is_none());
    assert!(w.floor_tile_row(0, rows, 0, [4, 5]).is_none());
    assert!(w.floor_tile_row(u32::MAX, 0, 0, [4, 5]).is_none());
    assert!(w.floor_tile_row(0, 0, usize::MAX, [4, 5]).is_none());
}

#[test]
fn navigation_row_does_not_scan_unconsumed_tiles() {
    let calls = Arc::new(AtomicUsize::new(0));
    let w = world(&calls);
    let before = calls.load(Ordering::Relaxed);
    let mut row = w
        .floor_tile_row(0, 0, w.nx.div_ceil(8) as usize, [4, 5])
        .unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), before);
    assert!(row.next().is_some());
    assert!(calls.load(Ordering::Relaxed) > before);
    assert!(w.prepared_floor_tile(0, 0, [4, 5]).is_some());
    assert!(w.prepared_floor_tile(1, 0, [4, 5]).is_none());
    drop(row);
    let before = calls.load(Ordering::Relaxed);
    assert_eq!(w.floor_tile_row(0, 0, 1, [4, 5]).unwrap().count(), 1);
    assert_eq!(calls.load(Ordering::Relaxed), before);
}

#[test]
fn material_planes_match_scalars_across_packed_widths_chunk_edges_and_pristine_spans() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut w = world(&calls);
    for edited in [false, true] {
        if edited {
            for i in 0..300 {
                w.queue_edit(Edit::put(32 + i % 24, 2 + i / 24, 33, 1000 + i as u16));
            }
            w.flush();
        }
        for min in [[0, 0, 0], [7, 3, 8], [29, 1, 33], [63, 30, 31]] {
            for width in [1usize, 8, 32, 64] {
                let width = width.min((w.nx - min[0]) as usize);
                let height = 64.min((w.ny - min[1]) as usize);
                let mut out = vec![u16::MAX; 72 * height];
                w.material_plane(min, width, height, 72, &mut out);
                for y in 0..height {
                    for x in 0..width {
                        assert_eq!(
                            out[y * 72 + x],
                            w.material(min[0] + x as u32, min[1] + y as u32, min[2])
                        );
                    }
                    assert!(out[y * 72 + width..(y + 1) * 72]
                        .iter()
                        .all(|&v| v == u16::MAX));
                }
            }
        }
    }
}

#[test]
fn rectangular_ceiling_matches_live_columns_at_edges_and_after_edits() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut w = world(&calls);
    for edited in [false, true] {
        if edited {
            w.queue_edit(Edit::put(33, 55, 33, 2));
            w.queue_edit(Edit::dig_if(6, 34, 5, 7));
            w.flush();
        }
        for min in [[0, 0], [1, 3], [31, 31], [64, 32], [73, 45], [u32::MAX; 2]] {
            for size in [1, 9, 32, 64, 128] {
                let max = min.map(|v| v.saturating_add(size));
                let mut expected = 0;
                for z in min[1].min(w.nz)..max[1].min(w.nz) {
                    for x in min[0].min(w.nx)..max[0].min(w.nx) {
                        expected = expected.max(w.column_ceiling(x, z));
                    }
                }
                assert_eq!(w.column_box_ceiling(min, max), expected);
            }
        }
    }
}

#[test]
fn material_candidates_cover_committed_overlays_and_unknown_generators_fall_back() {
    struct Known;
    impl Generator for Known {
        fn material(&self, _: u32, y: u32, _: u32) -> Mat {
            if y < 16 {
                1
            } else {
                0
            }
        }
        fn material_candidates(&self, _: [u32; 3], _: [u32; 3]) -> Option<Vec<Mat>> {
            Some(vec![0, 1])
        }
    }
    let mut w = World::new(96, 96, 96, 100, Box::new(Known));
    let (lo, hi) = ([29; 3], [66; 3]);
    assert_eq!(w.material_candidates(lo, hi), Some(vec![0, 1]));
    w.queue_edit(Edit::put(33, 33, 33, 65535));
    assert_eq!(w.material_candidates(lo, hi), Some(vec![0, 1]));
    w.flush();
    assert_eq!(w.material_candidates(lo, hi), Some(vec![0, 1, 65535]));
    assert_eq!(w.material_candidates([0; 3], [32; 3]), Some(vec![0, 1]));
    let mut blob = fx::blob::Blob::new();
    w.save(&mut blob);
    let mut restored = World::new(96, 96, 96, 100, Box::new(Known));
    assert!(restored.load(&mut fx::blob::Scan::new(&blob.v)));
    assert_eq!(
        restored.material_candidates(lo, hi),
        w.material_candidates(lo, hi)
    );
    let candidates = restored.material_candidates(lo, hi).unwrap();
    for z in 29..66 {
        for y in 29..66 {
            for x in 29..66 {
                assert!(candidates.contains(&restored.material(x, y, z)));
            }
        }
    }
    assert_eq!(w.material_candidates([0; 3], [97; 3]), None);
    let calls = Arc::new(AtomicUsize::new(0));
    assert_eq!(world(&calls).material_candidates([0; 3], [32; 3]), None);
}

#[test]
fn uniform_box_certificates_reject_bounds_and_committed_edits() {
    struct Uniform;
    impl Generator for Uniform {
        fn material(&self, _: u32, _: u32, _: u32) -> Mat {
            300
        }
        fn uniform_box(&self, _: [u32; 3], _: [u32; 3]) -> Option<Mat> {
            Some(300)
        }
    }
    let mut w = World::new(99, 67, 99, 100, Box::new(Uniform));
    let min = [29; 3];
    let max = [66; 3];
    assert_eq!(w.uniform_box(min, max), Some(300));
    for (lo, hi) in [
        ([0; 3], [0; 3]),
        ([0; 3], [100; 3]),
        ([66; 3], [29; 3]),
        ([u32::MAX; 3], [u32::MAX; 3]),
    ] {
        assert_eq!(w.uniform_box(lo, hi), None);
    }
    w.queue_edit(Edit::put(33, 33, 33, AIR));
    assert_eq!(w.uniform_box(min, max), Some(300)); // Queued changes are not committed.
    w.flush();
    assert_eq!(w.material(33, 33, 33), AIR);
    assert_eq!(w.uniform_box(min, max), None);
    // An edit elsewhere in the same chunk must not defeat a valid local
    // proof, nor may a foreign/air voxel inside the box survive that proof.
    assert_eq!(w.uniform_box([34; 3], [63; 3]), Some(300));
    assert_eq!(w.uniform_box([33; 3], [34; 3]), None);
    assert_eq!(w.uniform_box([66, 1, 1], [98, 32, 32]), Some(300));
    w.queue_edit(Edit::put(31, 31, 31, 65535));
    w.flush();
    assert_eq!(w.uniform_box([1; 3], [32; 3]), None);
    assert_eq!(w.material(31, 31, 31), 65535);
    assert_eq!(w.uniform_box([1; 3], [31; 3]), Some(300));
    w.queue_edit(Edit::put(33, 33, 33, 300));
    w.queue_edit(Edit::put(31, 31, 31, 300));
    w.flush();
    assert_eq!(w.uniform_box([1; 3], [66; 3]), Some(300));
}

struct Terrain(Arc<AtomicUsize>);
impl Terrain {
    fn at(x: u32, y: u32, z: u32) -> Mat {
        if y < 5 + (x + 3 * z) % 9 {
            2
        } else if y == 34 && x.is_multiple_of(3) && z.is_multiple_of(5) {
            7 // Disconnected overhangs across chunk boundaries.
        } else {
            AIR
        }
    }
}
impl Generator for Terrain {
    fn material(&self, x: u32, y: u32, z: u32) -> Mat {
        self.0.fetch_add(1, Ordering::Relaxed);
        Self::at(x, y, z)
    }
    fn column(&self, x: u32, z: u32, y0: u32, out: &mut [Mat]) {
        for (i, value) in out.iter_mut().enumerate() {
            *value = Self::at(x, y0 + i as u32, z);
        }
    }
}
fn world(calls: &Arc<AtomicUsize>) -> World {
    World::new(73, 67, 45, 100, Box::new(Terrain(Arc::clone(calls))))
}

#[test]
fn empty_material_reads_do_not_evaluate_generator_on_any_worker() {
    let calls = Arc::new(AtomicUsize::new(0));
    let w = world(&calls);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    for workers in [1, 2, 4, 8] {
        std::thread::scope(|scope| {
            for worker in 0..workers {
                let w = &w;
                scope.spawn(move || {
                    let mut cursor = World::mat_cursor();
                    for x in (worker..w.nx).step_by(workers as usize) {
                        for z in 0..w.nz {
                            assert_eq!(w.material(x, 63, z), AIR);
                            assert_eq!(w.material_cached(&mut cursor, x, 63, z), AIR);
                        }
                    }
                    for p in [[w.nx, 0, 0], [0, w.ny, 0], [0, 0, w.nz], [u32::MAX; 3]] {
                        assert_eq!(w.material(p[0], p[1], p[2]), AIR);
                        assert_eq!(w.material_cached(&mut cursor, p[0], p[1], p[2]), AIR);
                    }
                });
            }
        });
        assert_eq!(calls.load(Ordering::Relaxed), 0, "workers={workers}");
    }
    assert_eq!(w.material(1, 1, 1), 2);
    let mut cursor = World::mat_cursor();
    assert_eq!(w.material_cached(&mut cursor, 1, 1, 1), 2);
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

fn check_columns(w: &World) {
    // Column evaluation and immutable snapshots do not use the occupancy
    // shortcut, so both independently check its answers and edited layers.
    let snapshot = snapshot::Snapshot::capture(w, [0; 3], [73, 67, 45]).unwrap();
    let mut cursor = World::mat_cursor();
    for z in [0, 1, 5, 31, 32, 33, 44] {
        for x in [0, 1, 3, 31, 32, 33, 64, 65, 72] {
            let mut column = [AIR; 67];
            w.column(x, z, 0, &mut column);
            for (y, &expected) in column.iter().enumerate() {
                let y = y as u32;
                assert_eq!(w.material(x, y, z), expected, "{x},{y},{z}");
                assert_eq!(w.material_cached(&mut cursor, x, y, z), expected);
                assert_eq!(snapshot.material([x as i32, y as i32, z as i32]), expected);
            }
        }
    }
}

#[test]
fn material_reads_match_columns_and_snapshots_through_edits_load_and_reset() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut w = world(&calls);
    check_columns(&w);
    let old = snapshot::Snapshot::capture(&w, [0; 3], [73, 67, 45]).unwrap();
    w.queue_edit(Edit::put(33, 63, 33, 3));
    w.queue_dig(3, 34, 5);
    assert_eq!(w.material(33, 63, 33), AIR, "queued matter is not applied");
    assert_eq!(w.material(3, 34, 5), 7);
    w.flush();
    assert_eq!(w.material(33, 63, 33), 3);
    assert_eq!(w.material(3, 34, 5), AIR);
    assert_eq!(old.material([33, 63, 33]), AIR);
    assert_eq!(old.material([3, 34, 5]), 7);
    check_columns(&w);
    w.queue_transfer(Edit::dig(33, 63, 33));
    w.queue_transfer(Edit::put(65, 63, 44, 9));
    w.flush();
    check_columns(&w);
    let mut saved = fx::blob::Blob::new();
    w.save(&mut saved);
    let mut loaded = world(&calls);
    assert!(loaded.load(&mut fx::blob::Scan::new(&saved.v)));
    check_columns(&loaded);
    loaded.restore_pristine();
    check_columns(&loaded);
    assert_eq!(loaded.material(65, 63, 44), AIR);
    assert_eq!(loaded.material(3, 34, 5), 7);
}

#[test]
fn columns_evaluate_only_pristine_spans_and_merge_adjacent_chunks() {
    struct Counted {
        calls: Arc<AtomicUsize>,
        voxels: Arc<AtomicUsize>,
    }
    impl Generator for Counted {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            1 + ((x + 3 * y + z) % 7) as Mat
        }
        fn column(&self, x: u32, z: u32, y0: u32, out: &mut [Mat]) {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.voxels.fetch_add(out.len(), Ordering::Relaxed);
            for (i, dst) in out.iter_mut().enumerate() {
                *dst = self.material(x, y0 + i as u32, z);
            }
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let voxels = Arc::new(AtomicUsize::new(0));
    let mut w = World::new(
        35,
        99,
        35,
        100,
        Box::new(Counted {
            calls: Arc::clone(&calls),
            voxels: Arc::clone(&voxels),
        }),
    );
    let reset = || {
        calls.store(0, Ordering::Relaxed);
        voxels.store(0, Ordering::Relaxed);
    };
    let mut out = [AIR; 108];
    reset();
    w.column(2, 3, 0, &mut out);
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "adjacent pristine chunks share evaluation"
    );
    assert_eq!(voxels.load(Ordering::Relaxed), 99);
    w.queue_edit(Edit::put(2, 40, 3, 9));
    w.flush();
    for (y, n, expected_calls, expected_voxels) in [
        (32, 32, 0, 0),
        (0, 108, 2, 67),
        (31, 34, 2, 2),
        (99, 9, 0, 0),
        (0, 0, 0, 0),
    ] {
        reset();
        w.column(2, 3, y, &mut out[..n]);
        assert_eq!(calls.load(Ordering::Relaxed), expected_calls);
        assert_eq!(voxels.load(Ordering::Relaxed), expected_voxels);
        for (dy, &mat) in out[..n].iter().enumerate() {
            assert_eq!(mat, w.material(2, y + dy as u32, 3), "y={y} dy={dy}");
        }
    }
}
