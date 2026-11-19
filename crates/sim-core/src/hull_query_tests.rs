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
    for workers in [1, 4, 8] {
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

#[test]
fn zero_room_eviction_preserves_settled_contacts_outside_the_original_sweep() {
    let tuning = crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
    let mut t = tuning.blast;
    t.max_flying = 1;
    t.rest_check_ticks = 1;
    t.push_pct = 0;
    t.spin_deg_s = 0;
    let world = voxel::World::new(96, 64, 96, 4096, Box::new(Ground));
    let hulls = [Hull {
        pos: [Fx::from_int(48), Fx::from_int(16), Fx::from_int(48)],
        head: 0,
        half: [Fx::from_int(12); 2],
        height: Fx::from_int(12),
        vel: [Fx::ZERO; 3],
        step: Fx::ONE,
        solid: None,
    }];
    let run = |workers| {
        let mats =
            modding::data::materials_from_ron(include_str!("../../../data/materials.ron")).unwrap();
        let mut e = Economy::new(modding::Kinds::new(), mats, 2, &tuning);
        let mut b = Bursts::default();
        for (id, x, y, resting) in [(1, 48, 18, false), (2, 61, 17, true)] {
            b.fragments.push(Fragment {
                id,
                position: V3::new(Fx::from_int(x), Fx::from_int(y), Fx::from_int(48)),
                velocity: V3::ZERO,
                material: 1,
                paint: None,
                mm: 100,
                basis: IDENTITY,
                mass_g: 1200,
                count: 1,
                shape: 1,
                side: 1,
                chunk: None,
                held: e.ledger.create(matter::Kind::Debris, 1200, u64::MAX),
                resting,
            });
        }
        b.next_id = 3;
        rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .unwrap()
            .install(|| {
                b.step_fragments_reserved(
                    &world,
                    &mut e,
                    &t,
                    crate::gravity_per_tick(&tuning),
                    0,
                    Surroundings {
                        hulls: &hulls,
                        yielding: &[],
                    },
                    0,
                    false,
                    |_| {},
                    &|_, _| {},
                );
            });
        assert!(e.balanced());
        assert_eq!(
            b.fragments.len(),
            2,
            "contact must preserve the two finite cubes"
        );
        // Contact precedes sleeping/joining. The first bounce can still exceed
        // the sleep threshold; a query miss would instead fall through this
        // support outside the original sweep and retain downward velocity.
        let rider = b.fragments.iter().find(|f| f.id == 1).unwrap();
        assert!(rider.position.x.add(rider.bounds().0[0])
            >= hulls[0].pos[0].add(hulls[0].half[0]), "eviction left the rider inside the hull");
        assert!(rider.position.y >= Fx::from_int(18));
        assert!(rider.velocity.y >= Fx::ZERO, "missed the settled support");
        let support = b.fragments.iter().find(|f| f.id == 2).unwrap();
        assert_eq!(support.position.y, Fx::from_int(17));
        snapshot(&b, &world, &e)
    };
    let expected = run(1);
    assert_eq!(run(4), expected, "eviction must retain its settled support");
    assert_eq!(run(8), expected, "eviction must retain its settled support");
}

#[test]
fn coarse_piece_fallback_respects_a_proven_empty_hull_box() {
    struct Gap;
    impl Voxels for Gap {
        fn solid(&self,_:[i32;3])->bool { panic!("coarse box fallback must not scan unbounded cells") }
        fn may_overlap_box(&self,lo:[i32;3],hi:[i32;3])->bool {lo[1]<=100 && hi[1]>=100}
    }
    let voxels=Gap;
    let hull=Hull {pos:[Fx::ZERO;3],head:0,half:[Fx::from_int(100);2],height:Fx::from_int(120),
        vel:[Fx::ZERO;3],step:Fx::ZERO,solid:Some(Solid(&voxels))};
    let low=[Fx::from_int(-8);3];let high=[Fx::from_int(8);3];
    assert!(!hull.meets_piece([Fx::ZERO,Fx::from_int(20),Fx::ZERO],low,high));
    assert!(hull.meets_piece([Fx::ZERO,Fx::from_int(100),Fx::ZERO],low,high));
    assert!(!hull.meets([Fx::from_int(-20);3],[Fx::from_int(20);3],1));
}

#[test]
fn rotated_4x4x4_settled_shapes_match_scalar_oracle_across_multicell_queries() {
    let mut basis = IDENTITY;
    let c = fx::angle::cos(fx::angle::FULL / 7);
    let s = fx::angle::sin(fx::angle::FULL / 7);
    basis[0] = V3::new(c, Fx::ZERO, s);
    basis[2] = V3::new(s.neg(), Fx::ZERO, c);
    let masks = [
        0x0001_0003_0007_000fu64,
        0x8000_0000_0000_0001u64,
        0x0f0f_3333_5555_aaaau64,
    ];
    let make = |id: u64, pos: [i32; 3], shape: u64| Fragment {
        id,
        position: V3::new(Fx::from_int(pos[0]), Fx::from_int(pos[1]), Fx::from_int(pos[2])),
        velocity: V3::ZERO,
        material: 4,
        paint: None,
        mm: 100,
        basis,
        mass_g: 500,
        count: shape.count_ones(),
        shape,
        side: 4,
        chunk: None,
        held: u32::MAX,
        resting: true,
    };
    let scalar_meets = |f: &Fragment, lo: [Fx; 3], hi: [Fx; 3]| -> bool {
        let scale = Fx::from_ratio(f.mm as i32, voxel::unit::WORLD_MM as i32);
        let half = Fx::from_ratio(49, 100).mul(scale);
        let radius: [Fx; 3] = std::array::from_fn(|a| {
            arr(f.basis[0])[a]
                .abs()
                .add(arr(f.basis[1])[a].abs())
                .add(arr(f.basis[2])[a].abs())
                .mul(half)
        });
        let centre = arr(shape_centre(f.shape, f.side));
        for i in 0..box_cells(f.side) {
            if f.shape & (1u64 << i) == 0 {
                continue;
            }
            let q = shape_cell(i, f.side);
            let mut offset = V3::ZERO;
            for a in 0..3 {
                offset = offset.add(f.basis[a].scale(Fx::from_int(q[a]).sub(centre[a]).mul(scale)));
            }
            let p = arr(f.position.add(offset));
            if (0..3).all(|a| lo[a] < p[a].add(radius[a]) && p[a].sub(radius[a]) < hi[a]) {
                return true;
            }
        }
        false
    };
    for (idx, &mask_a) in masks.iter().enumerate() {
        let fa = make(idx as u64 + 1, [32, 20, 32], mask_a);
        let sa = SettledShape::new(&fa);
        for &mask_b in &masks {
            for dx in -5..=5 {
                let fb = make(99, [32 + dx, 21, 33], mask_b);
                let sb = SettledShape::new(&fb);
                let shift_lo = [Fx::from_ratio(dx, 3), Fx::from_ratio(-1, 2), Fx::ZERO];
                let shift_hi = [Fx::from_ratio(dx + 1, 3), Fx::ZERO, Fx::from_ratio(1, 4)];
                let lo = [Fx::from_int(28), Fx::from_int(17), Fx::from_int(28)];
                let hi = [Fx::from_int(38), Fx::from_int(25), Fx::from_int(38)];
                assert_eq!(sa.meets(lo, hi), scalar_meets(&fa, lo, hi));
                let mut buf = [([Fx::ZERO; 3], [Fx::ZERO; 3]); 64];
                let (n, m_lo, m_hi) = sb.shifted_cells(shift_lo, shift_hi, &mut buf);
                let fast_pair = sa.meets_shifted(lo, hi, &buf[..n], m_lo, m_hi);
                let scale = Fx::from_ratio(fa.mm as i32, voxel::unit::WORLD_MM as i32);
                let half = Fx::from_ratio(49, 100).mul(scale);
                let ra: [Fx; 3] = std::array::from_fn(|a| {
                    arr(fa.basis[0])[a]
                        .abs()
                        .add(arr(fa.basis[1])[a].abs())
                        .add(arr(fa.basis[2])[a].abs())
                        .mul(half)
                });
                let ca = arr(shape_centre(fa.shape, fa.side));
                let mut slow_pair = false;
                for i in 0..64 {
                    if fa.shape & (1u64 << i) == 0 {
                        continue;
                    }
                    let q = shape_cell(i, 4);
                    let mut off = V3::ZERO;
                    for a in 0..3 {
                        off = off.add(fa.basis[a].scale(Fx::from_int(q[a]).sub(ca[a]).mul(scale)));
                    }
                    let p = arr(fa.position.add(off));
                    let c_lo = [0, 1, 2].map(|a| p[a].sub(ra[a]));
                    let c_hi = [0, 1, 2].map(|a| p[a].add(ra[a]));
                    if (0..3).all(|a| lo[a] < c_hi[a] && c_lo[a] < hi[a])
                        && scalar_meets(
                            &fb,
                            [0, 1, 2].map(|a| c_lo[a].sub(shift_hi[a])),
                            [0, 1, 2].map(|a| c_hi[a].sub(shift_lo[a])),
                        )
                    {
                        slow_pair = true;
                        break;
                    }
                }
                assert_eq!(fast_pair, slow_pair, "dx={dx} mask_a={mask_a:#x} mask_b={mask_b:#x}");
            }
        }
    }
}
