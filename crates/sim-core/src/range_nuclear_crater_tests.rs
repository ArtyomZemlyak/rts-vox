use super::*;

struct Soil;
impl Generator for Soil {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 {
        u16::from(y < 80)
    }
    fn batches_columns(&self) -> bool {
        true
    }
    fn column(&self, _: u32, _: u32, y: u32, out: &mut [u16]) {
        for (dy, m) in out.iter_mut().enumerate() {
            *m = u16::from(y + (dy as u32) < 80);
        }
    }
    fn material_candidates(&self, _: [u32; 3], _: [u32; 3]) -> Option<Vec<u16>> {
        Some(vec![0, 1])
    }
    fn uniform_box(&self, lo: [u32; 3], hi: [u32; 3]) -> Option<u16> {
        if hi[1] <= 80 {
            Some(1)
        } else if lo[1] >= 80 {
            Some(0)
        } else {
            None
        }
    }
}

#[test]
fn nuclear_crater_is_wide_cubic_and_filtered_contacts_match_scalar_soil() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let at = [512, 79, 512];
    let mut profiles = Vec::new();
    for filtered in [false, true] {
        let mut w = World::new(1024, 128, 1024, 65536, Box::new(Soil));
        // Non-soil material outside the crater still meets full blast pressure.
        for (x, mat) in [(1000, 2), (1001, 4), (1002, 5), (1003, 3)] {
            w.queue_edit(voxel::Edit::put(x, 79, 512, mat));
        }
        w.flush();
        let mut dents = SurfaceWork::new();
        let mut e = Explosion::with_damage(&w, at, &c, &[], &dents);
        e.nuclear_ground = true;
        let m = matter::Matter::new(&[]);
        let mut ticks = 0;
        while (e.working() || w.queued() > 0) && ticks < 2048 {
            e.advance_frontier(c.wave_work_per_tick, &w, &[]);
            if filtered {
                e.read_contacts_with(&w, c.wave_work_per_tick, &[], Some((&m, &c, u64::MAX)));
            }
            let before = w.queued();
            e.step_with_room::<true>(
                &mut w,
                c.wave_work_per_tick,
                &mut [],
                &mut dents,
                &mut Vec::new(),
                &c,
                None,
                None,
            );
            assert!(
                w.queued() - before <= c.wave_work_per_tick,
                "ground exceeded its slice"
            );
            w.flush();
            ticks += 1;
        }
        assert!(
            !e.working() && w.queued() == 0,
            "unfinished after {ticks} ticks"
        );
        let depth_at = |x, z| {
            (0..80)
                .rev()
                .find(|&y| w.material(x, y, z) != 0)
                .map_or(80, |y| 79 - y)
        };
        let profile: Vec<_> = (0..1024).map(|x| depth_at(x, 512)).collect();
        assert!(profile[512] >= 48, "crater too shallow: {}", profile[512]);
        assert!(profile[512] > profile[512 + 192]);
        assert!(profile[512 + 320] > 0, "crater did not widen");
        assert!(depth_at(512 + 288, 512 + 288) > 0, "lost square corners");
        assert_eq!(profile[512 + 480], 0, "crater stripped distant soil");
        assert!(dents.is_empty(), "strategic soil created per-voxel dents");
        for x in 1000..1004 {
            assert_eq!(w.material(x, 79, 512), 0);
        }
        println!(
            "filtered={filtered} ticks={ticks} centre_depth={} width={}",
            profile[512],
            profile.iter().filter(|&&d| d > 0).count()
        );
        profiles.push((profile, dents));
    }
    assert_eq!(
        profiles[0], profiles[1],
        "silent-soil filtering changed damage"
    );
}

#[test]
fn nuclear_crater_box_proofs_preserve_mixed_material_and_live_edits() {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let at = [512, 79, 512];
    let mut w = World::new(768, 128, 768, 65536, Box::new(Soil));
    let p = [0, 0, 512];
    assert!(confined_soil_box(&w, p, 64, 2400, 79, Some(at), &c));
    for mat in [2, 3, 4, 5, 6, 65535] {
        w.queue_edit(voxel::Edit::put(20, 40, 530, mat));
        w.flush();
        assert!(!confined_soil_box(&w, p, 64, 2400, 79, Some(at), &c));
    }
    assert_eq!(
        soil_lateral_work([i32::MAX, 0, i32::MIN], Some(at), &c),
        u32::MAX
    );
    assert_eq!(soil_lateral_work(p, None, &c), 0);
}

#[test]
fn nuclear_crater_zero_depth_cost_preserves_partial_soil_contacts() {
    let mut c =
        ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    c.soil_depth_cost = 0;
    c.cell_units = 4;
    c.half_cells = 2;
    assert!(c.validate().is_ok());
    let pressure = material_work(1, &c) - 1;
    let mut w = World::new(64, 32, 64, 65536, Box::new(Soil));
    let mut dents = SurfaceWork::new();
    let mut e = Explosion::begin([24, 15, 24], &c, &[]);
    e.nuclear_ground = true;
    e.origin = [16, 8, 16];
    e.pending.push_back(phys::blast::Hit {
        cell: [0; 3],
        pressure,
        broken: false,
    });
    let m = matter::Matter::new(&[]);
    e.read_contacts_with(&w, 65536, &[], Some((&m, &c, u64::MAX)));
    assert_eq!(e.solid.len(), 1);
    assert_eq!(
        e.solid[0].1.count_ones(),
        64,
        "disabled confinement must keep weak contacts"
    );
    assert_eq!(e.solid_mats, vec![1; 64]);
    assert!(e.ground_contact_known(
        &mut w,
        [16, 8, 16],
        1,
        pressure,
        &mut dents,
        &mut Vec::new(),
        &c,
        None
    ));
    assert_eq!(dents.get(&[16, 8, 16]), Some(&pressure));
    assert_eq!(w.queued(), 0);
}

#[test]
fn nuclear_crater_row_reader_matches_scalar_with_holes_edges_and_soil_materials() {
    struct Layer(u16);
    impl Generator for Layer {
        fn material(&self, _: u32, y: u32, _: u32) -> u16 {
            if y < 80 {
                self.0
            } else {
                0
            }
        }
        fn material_candidates(&self, _: [u32; 3], _: [u32; 3]) -> Option<Vec<u16>> {
            Some(vec![0, self.0])
        }
    }
    let c = ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    let at = [40, 79, 40];
    for mat in [1, 7, 8] {
        let mut w = World::new(128, 128, 128, 65536, Box::new(Layer(mat)));
        for p in [[1, 70, 12], [45, 63, 40], [50, 79, 50]] {
            w.queue_edit(voxel::Edit::dig(p[0], p[1], p[2]));
        }
        w.flush();
        for edge in [3, 16, 64] {
            for p in [[-8, 64, -8], [32, 32, 32], [100, 64, 100], [20, 79, 20]] {
                for pressure in [0, 283, 284, 1500, 2400] {
                    let mut actual = vec![42];
                    let fast =
                        read_terraced_soil(&w, p, edge, pressure, 79, at, &c, &mut actual).unwrap();
                    let offsets = subcell_offsets(edge);
                    let mut raw = Vec::new();
                    let mask = read_cell_into(&w, p, edge, [0; 3], &offsets, &mut raw);
                    let mut expected_mask = contact_mask::Mask::for_voxels((edge as usize).pow(3));
                    let mut expected = vec![42];
                    for (rank, sub) in mask.ranked_from(0) {
                        let q = std::array::from_fn(|a| p[a] + offsets[sub][a]);
                        if soil_contact_pressure(
                            pressure,
                            soil_load(q, 79, Some(at), &c),
                            raw[rank],
                            true,
                            &c,
                        ) > 0
                        {
                            expected_mask.insert(sub);
                            expected.push(raw[rank]);
                        }
                    }
                    assert_eq!(
                        fast, expected_mask,
                        "mat={mat} edge={edge} p={p:?} pressure={pressure}"
                    );
                    assert_eq!(actual, expected);
                }
            }
        }
        w.queue_edit(voxel::Edit::put(40, 75, 40, 65535));
        w.flush();
        let mut payload = vec![42];
        assert!(read_terraced_soil(&w, [32, 64, 32], 16, 2400, 79, at, &c, &mut payload).is_none());
        assert_eq!(payload, [42], "fallback must not append a partial payload");
    }
}
