use super::*;

struct Ground;
impl voxel::Generator for Ground {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 {
        u16::from(y < 12)
    }
}
fn snapshot(b: &Bursts, w: &voxel::World, e: &Economy) -> Vec<u8> {
    let mut blob = Blob::new();
    b.save(&mut blob);
    w.save(&mut blob);
    let (containers, sunk, initial, free_head, free) = e.ledger.snapshot();
    blob.usize(containers.len());
    for c in containers {
        blob.u8(c.kind.code());
        blob.u64(c.held);
        blob.u64(c.cap);
        blob.bool(c.alive);
    }
    for n in sunk {
        blob.u64(n);
    }
    blob.u64(initial);
    blob.u32(free_head);
    blob.usize(free.len());
    for n in free {
        blob.u32(n);
    }
    blob.usize(e.wrecks.len());
    for w in &e.wrecks {
        blob.u32(w.held);
        blob.i32(w.x);
        blob.i32(w.y);
        blob.i32(w.z);
    }
    blob.v
}

#[test]
fn hull_queries_preserve_parallel_wake_reservations_moving_hulls_and_restore() {
    let tuning = crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
    let mut t = tuning.blast;
    t.max_fragments = 2000;
    t.max_flying = 2000;
    t.max_resting_fragments = 2048;
    let fixture = || {
        let mats =
            modding::data::materials_from_ron(include_str!("../../../data/materials.ron")).unwrap();
        let mut economy = Economy::new(modding::Kinds::new(), mats, 2, &tuning);
        let mut pieces = Bursts::default();
        let mut world = voxel::World::new(96, 64, 96, 4096, Box::new(Ground));
        for i in 0..1100u64 {
            let held = economy.ledger.create(matter::Kind::Debris, 1200, u64::MAX);
            pieces.fragments.push(Fragment {
                id: i + 1,
                position: V3::new(
                    Fx::from_int(8 + (i % 40) as i32 * 2),
                    Fx::from_int(18 + (i % 2) as i32 * 5),
                    Fx::from_int(8 + (i / 40) as i32 * 2),
                ),
                velocity: if i % 4 == 0 {
                    V3::new(Fx::from_ratio(1, 2), Fx::ZERO, Fx::ZERO)
                } else {
                    V3::ZERO
                },
                material: 1,
                paint: None,
                mm: 100,
                basis: IDENTITY,
                mass_g: 1200,
                count: 1,
                shape: 1,
                side: 1,
                chunk: None,
                held,
                resting: i % 4 != 0,
            });
        }
        pieces.next_id = 1101;
        assert!(pieces.claim_voxel(&mut world, [88, 10, 88], 1, [88, 10, 88], 0, 1, &t));
        (pieces, world, economy)
    };
    for workers in [1, 4] {
        let (mut a, mut wa, mut ea) = fixture();
        let (mut b, mut wb, mut eb) = fixture();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .unwrap();
        for tick in 0..20 {
            // Two overlapping moving hulls have opposite pushes: source
            // order must choose exactly the same one. Most hulls are distant.
            let hulls: Vec<_> = (0..80)
                .map(|i| Hull {
                    pos: if i < 2 {
                        [
                            Fx::from_int(40 + (tick / 4) as i32),
                            Fx::from_int(12),
                            Fx::from_int(32),
                        ]
                    } else {
                        [Fx::from_int(512 + i * 64), Fx::ZERO, Fx::from_int(512)]
                    },
                    head: (tick * 173) as i32,
                    half: [Fx::from_int(16), Fx::from_int(12)],
                    height: Fx::from_int(16),
                    vel: [
                        Fx::from_int(if i == 1 { -1 } else { 1 }),
                        Fx::ZERO,
                        Fx::ZERO,
                    ],
                    step: Fx::from_int(4),
                    solid: None,
                })
                .collect();
            let around = Surroundings {
                hulls: &hulls,
                yielding: &[],
            };
            let reserve = if tick < 3 { 2000 } else { 0 };
            let waiting_damage = (3..6).contains(&tick);
            pool.install(|| {
                a.step_fragments_kernel::<true, true>(
                    &wa,
                    &mut ea,
                    &t,
                    crate::gravity_per_tick(&tuning),
                    tick,
                    around,
                    reserve,
                    waiting_damage,
                    |_| {},
                    &|_, _| {},
                )
            });
            b.step_fragments_kernel::<false, false>(
                &wb,
                &mut eb,
                &t,
                crate::gravity_per_tick(&tuning),
                tick,
                around,
                reserve,
                waiting_damage,
                |_| {},
                &|_, _| {},
            );
            assert_eq!(
                snapshot(&a, &wa, &ea),
                snapshot(&b, &wb, &eb),
                "workers={workers}, tick={tick}"
            );
            assert_eq!(a.wakeups, b.wakeups);
            assert!(ea.balanced() && eb.balanced());
            if tick < 3 {
                assert_eq!(a.wakeups, 0, "heat/claim slots must not be stolen");
            }
            if tick == 3 {
                wa.flush();
                wb.flush();
                a.confirm(wa.completed(), &mut ea, &t);
                b.confirm(wb.completed(), &mut eb, &t);
                assert_eq!(a.claims(), 0);
                assert_eq!(a.removed, 1, "the real claim must be confirmed");
            }
            if tick == 5 {
                let mut bytes = Blob::new();
                b.save(&mut bytes);
                let mut scan = Scan::new(&bytes.v);
                b = Bursts::load(&mut scan, Some(&wb), &t).unwrap();
                assert!(scan.ok() && scan.left() == 0);
                // Diagnostic wakeups reset on load, never choose physics.
                a.wakeups = 0;
            }
        }
        assert!(a.wakeups > 0, "returning room must permit actual wakeups");
        assert_eq!(snapshot(&a, &wa, &ea), snapshot(&b, &wb, &eb));
        assert!(ea.balanced() && eb.balanced());
    }
}
