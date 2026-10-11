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
