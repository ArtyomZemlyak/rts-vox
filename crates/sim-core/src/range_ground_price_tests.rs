use super::*;

#[test]
fn fracture_price_memo_tracks_every_input_and_collisions() {
    let mut m = Matter::new(&[]);
    for motion in [0, 15, 45, 99] {
        m.tuning.motion_pct = motion;
        for coefficient in [1, 100, 100_000, u32::MAX] {
            m.tuning.fracture_kj_m3_per_strength = coefficient;
            for strength in [1, 30, 120, modding::MAX_STRENGTH] {
                for mat in [1, 9, 1, 3, 7, 8, 9] {
                    let mut def = *m.economy.mats.get(mat).unwrap();
                    def.strength = strength;
                    m.economy.mats.replace(def).unwrap();
                    let expected = m.cost(mat);
                    assert_eq!(m.cost_cached(mat), expected);
                    assert_eq!(m.cost_cached(mat), expected);
                }
            }
        }
    }
    // The memo changes neither save bytes nor the material/energy ledger.
    let mut a = Blob::new();
    m.save(&mut a);
    m.prices = [None; 8];
    let mut b = Blob::new();
    m.save(&mut b);
    assert_eq!(a.v, b.v);
    assert!(m.economy.ledger.balanced());
}

/// The serial ground claim stage, excluding field propagation, units and
/// rendering. Setup and state checks stay outside the timed section.
#[test]
#[ignore = "ground claim/confirmation measurement; run serially"]
fn probe_ground_claim_pipeline() {
    struct Ground;
    impl voxel::Generator for Ground {
        fn material(&self, _: u32, y: u32, _: u32) -> u16 {
            if y < 20 {
                1
            } else {
                0
            }
        }
    }
    let cells: Vec<[i32; 3]> = (8..72)
        .flat_map(|z| (4..14).flat_map(move |y| (8..72).map(move |x| [x, y, z])))
        .collect();
    for cap in [0, 4096, 65536] {
        let mut times = Vec::new();
        let mut confirmations = Vec::new();
        let mut expected = None;
        for run in 0..10 {
            let mut m = Matter::new(&[]);
            m.tuning.max_claims = cap;
            m.tuning.max_flying = 65536;
            m.tuning.max_fragments = 65536;
            m.begin(u32::MAX, 4);
            let mut w = World::new(96, 32, 96, 65536, Box::new(Ground));
            // Prebuilt chunks isolate claim/confirm from terrain generation.
            for z in (0..96).step_by(32) {
                for x in (0..96).step_by(32) {
                    let ci = w.chunk_of(x, 8, z).unwrap();
                    let chunk = w.build_chunk(ci);
                    w.install_chunk(ci, chunk);
                }
            }
            m.begin_body_batch();
            let start = std::time::Instant::now();
            for &p in &cells {
                assert!(matches!(
                    m.ground_or_pulverize(&mut w, p, 1, [40, 16, 40]),
                    ClaimResult::Applied
                ));
            }
            let ns = start.elapsed().as_nanos();
            m.finish_body_batch();
            w.flush();
            let start = std::time::Instant::now();
            m.debris.confirm(w.completed(), &mut m.economy, &m.tuning);
            let confirm_ns = start.elapsed().as_nanos();
            assert!(m.economy.ledger.balanced());
            assert_eq!(w.edits().len(), cells.len());
            let mut bytes = Blob::new();
            m.save(&mut bytes);
            let state = (bytes.v, w.edits().to_vec());
            if let Some(ref expected) = expected {
                assert_eq!(&state, expected);
            } else {
                expected = Some(state);
            }
            if run >= 2 {
                times.push(ns);
                confirmations.push(confirm_ns);
            }
        }
        println!(
            "ground cap={cap} cells={} claim_mean_ns={} confirm_mean_ns={}",
            cells.len(),
            times.iter().sum::<u128>() / times.len() as u128,
            confirmations.iter().sum::<u128>() / confirmations.len() as u128
        );
    }
}

#[test]
#[ignore = "fracture price microbenchmark; run serially"]
fn measure_fracture_price_memo() {
    use std::hint::black_box;
    let mut m = Matter::new(&[]);
    for (name, materials) in [
        ("soil", &[1u16][..]),
        ("mixed", &[1u16, 3, 4, 5, 7, 8][..]),
        ("collisions", &[1u16, 9][..]),
    ] {
        for cached in [false, true, true, false] {
            let start = std::time::Instant::now();
            let mut checksum = 0u64;
            for i in 0..2_000_000 {
                let mat = black_box(materials[i % materials.len()]);
                let price = if cached {
                    black_box(&mut m).cost_cached(mat)
                } else {
                    black_box(&m).cost(mat)
                };
                checksum = checksum.wrapping_add(price.0).wrapping_add(price.1);
            }
            eprintln!(
                "{name} cached={cached} ns={:.3} checksum={checksum}",
                start.elapsed().as_secs_f64() * 1e9 / 2_000_000.0
            );
        }
    }
}
