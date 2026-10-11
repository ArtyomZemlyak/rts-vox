//! Read-ahead is disposable: it must preserve ordered contacts and live matter.
use super::*;

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
    let w = World::new(320, 320, 320, 100, Box::new(Pattern));
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
    for edge in [1, 2, 4] {
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
                    let mut mask = 0;
                    let start = mats.len() as u32;
                    for (index, offset) in subcell_offsets(edge).iter().enumerate() {
                        let p =
                            std::array::from_fn(|a| e.origin[a] + hit.cell[a] * edge + offset[a]);
                        let mat = material(&w, p);
                        if mat != 0 {
                            mask |= 1u64 << index;
                            mats.push(mat);
                        }
                    }
                    expected.push((hit.cell, mask, start));
                    left = left.saturating_sub(1 + mask.count_ones() as usize);
                }
                e.read_contacts(&w, budget);
                assert_eq!(
                    e.solid, expected,
                    "edge={edge} budget={budget} changed={changed}"
                );
                assert_eq!(e.solid_mats, mats);
                assert_eq!(e.solid_next, 0);
                assert_eq!(saved(&e), original, "read-ahead changed saved work");
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
