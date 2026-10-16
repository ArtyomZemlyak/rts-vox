//! Read-ahead is disposable: it must preserve ordered contacts and live matter.
use super::*;

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
                        let solid = e.solid.clone();
                        let mats = e.solid_mats.clone();
                        let state = saved(&e);
                        for policy in [1, 2] {
                            if policy == 1 {
                                e.read_contacts_with(&w, budget, &[], spent);
                            } else {
                                e.read_contacts_inner::<2>(&w, budget, &[], spent, &|_| {});
                            }
                            assert_eq!(e.solid, solid, "edge={edge} enabled={enabled} energy={energy} budget={budget} policy={policy}");
                            assert_eq!(e.solid_mats, mats);
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
