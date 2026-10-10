//! Exact resumption of saved ground/body contacts, with eager preparation as reference.
use super::*;
use fx::blob::{Blob, Scan};

fn fixture(edge: i32, points: bool) -> (BlastRange, Explosion, ExplosionConfig) {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    // Validate ordinary placements first. The compact probe below is test-only
    // geometry near live terrain; it does not relax production placement checks.
    layout.units = (0..3)
        .map(|i| modding::range::RangeUnitDef {
            kind: RangeUnitKind::Tank,
            at: [170 + i * 100, 80, 336],
            allied: i == 0,
            weapon: None,
        })
        .collect();
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    c.energy = 1;
    c.half_cells = 8;
    c.cell_units = edge;
    c.unit_ablation_resistance = 1;
    c.unit_role_resistance_milli = [1000; 5];
    let mut r = BlastRange::with_layout(c, layout).unwrap();
    r.units[1] = RangeUnit::of_kind(RangeUnitKind::Strider, [270, 16, 336]);
    r.units[2] = RangeUnit::of_kind(RangeUnitKind::Spider, [440, 16, 336]);
    let u = &mut r.units[0];
    // Align the one-voxel-wide probe with world sample centres as well as
    // local voxel centres, so legacy point-sampling really hits its body.
    u.motion.pos = [Fx::from_int(46), Fx::from_int(4), Fx::from_ratio(81, 2)];
    u.body = UnitBody::new_scaled(4, 1, 1, 0, 100);
    for x in 0..4 {
        u.body.set(x, 0, 0, voxel::Part::Armour);
    }
    u.update_pose(0);
    r.matter = matter::Matter::new(&r.units);
    r.matter.begin(u32::MAX, 4);
    r.world.max_edits_per_tick = 1;
    let mut e = Explosion::with_damage(&r.world, [40, 2, 40], &c, &r.units, &r.surface_work);
    e.wave.advance_contacts(usize::MAX);
    assert!(e.wave.finished());
    e.point_contacts = points;
    push(&mut e, [40, 2, 40], 1_000_000);
    (r, e, c)
}

fn push(e: &mut Explosion, p: [i32; 3], pressure: u32) {
    let cell = std::array::from_fn(|a| (p[a] - e.origin[a]) / e.cell);
    assert!(cell.iter().all(|&v| v >= 0 && v < e.wave.size() as i32));
    e.pending.push_back(phys::blast::Hit {
        cell,
        pressure,
        broken: true,
    });
}

fn body_point(r: &BlastRange, e: &Explosion) -> [i32; 3] {
    let p = e.poses[0].world([Fx::from_ratio(1, 2); 3]);
    let p = [p.x, p.y, p.z].map(|v| (v.raw() >> 32) as i32);
    assert!(r.units[0].hit_at(p, &e.poses[0], &e.seg_poses[0]).is_some());
    p
}

fn step<const FAST: bool>(
    r: &mut BlastRange,
    e: &mut Explosion,
    c: &ExplosionConfig,
    material: bool,
) {
    if FAST {
        e.step(
            &mut r.world,
            0,
            &mut r.units,
            &mut r.surface_work,
            &mut r.surface_changed,
            c,
            material.then_some(&mut r.matter),
        );
    } else {
        if material {
            r.matter.begin_body_batch();
        }
        e.step_contacts::<false, false>(
            &mut r.world,
            0,
            &mut r.units,
            &mut r.surface_work,
            &mut r.surface_changed,
            c,
            material.then_some(&mut r.matter),
        );
        if material {
            r.matter.finish_body_batch();
        }
    }
}

fn wave_bytes(e: &Explosion) -> Vec<u8> {
    let mut b = Blob::new();
    e.save(&mut b);
    e.save_contacts(&mut b);
    b.v
}

fn same(a: &BlastRange, x: &Explosion, b: &BlastRange, y: &Explosion) {
    assert_eq!(a.state_hash(), b.state_hash());
    assert_eq!(a.snapshot(), b.snapshot());
    assert_eq!(wave_bytes(x), wave_bytes(y));
    assert_eq!(x.turret_pitches, y.turret_pitches);
    assert_eq!(a.surface_changed, b.surface_changed);
    assert_eq!(a.matter.admission.ground, b.matter.admission.ground);
    assert_eq!(a.matter.admission.body, b.matter.admission.body);
    assert!(a.matter.economy.ledger.balanced());
}

fn flush(r: &mut BlastRange) {
    r.world.flush();
    r.matter
        .debris
        .confirm(r.world.completed(), &mut r.matter.economy, &r.matter.tuning);
    for f in &mut r.matter.debris.fragments {
        f.resting = true;
        f.velocity = phys::V3::ZERO;
    }
}

#[test]
fn deferred_ground_resumes_without_unit_geometry_and_keeps_every_subcell() {
    for edge in [1, 2, 4] {
        for points in [false, true] {
            let (mut a, mut x, c) = fixture(edge, points);
            let (mut b, mut y, _) = fixture(edge, points);
            // A later contact must still visit the frozen body after ground resumes.
            let p = body_point(&a, &x);
            push(&mut x, p, 1_000_000);
            push(&mut y, p, 1_000_000);
            for (r, e) in [(&mut a, &mut x), (&mut b, &mut y)] {
                r.matter.tuning.max_flying = 0;
                e.contact_cursor =
                    (edge as usize).pow(3).saturating_sub(1) * (r.units.len() + 1) + r.units.len();
            }
            let cursor = x.contact_cursor;
            let energy = a.matter.remaining_mj;
            for _ in 0..3 {
                step::<true>(&mut a, &mut x, &c, true);
                step::<false>(&mut b, &mut y, &c, true);
                same(&a, &x, &b, &y);
                assert_eq!(x.contact_cursor, cursor);
                assert_eq!(a.matter.remaining_mj, energy);
            }
            assert_eq!(a.matter.admission.wave_geometry, [0, 0, 3, 3]);
            assert_eq!(b.matter.admission.wave_geometry, [3, 9, 3, 0]);
            // Frozen transforms survive movement and aim changes during the wait.
            for r in [&mut a, &mut b] {
                r.units[0].motion.pos[0] = r.units[0].motion.pos[0].add(Fx::from_int(6));
                r.units[0].update_pose(1234);
                r.matter.tuning.max_flying = 8;
            }
            for _ in 0..160 {
                step::<true>(&mut a, &mut x, &c, true);
                step::<false>(&mut b, &mut y, &c, true);
                same(&a, &x, &b, &y);
                flush(&mut a);
                flush(&mut b);
                same(&a, &x, &b, &y);
                if x.pending.is_empty() {
                    break;
                }
            }
            assert!(x.pending.is_empty(), "edge={edge} points={points}");
            assert!(
                !a.units[0].removed.is_empty(),
                "later body contact was skipped"
            );
            assert!(a.matter.admission.ground[0] > 0);
        }
    }
}

#[test]
fn world_resume_retains_live_ground_outcomes_with_and_without_material_adapter() {
    for mode in 0..7 {
        let (mut a, mut x, mut c) = fixture(1, false);
        let (mut b, mut y, _) = fixture(1, false);
        for (r, e) in [(&mut a, &mut x), (&mut b, &mut y)] {
            e.contact_cursor = r.units.len();
            match mode {
                0 => e.pending.front_mut().unwrap().pressure = 0,
                1 => e.pending.front_mut().unwrap().pressure = 1,
                2 => r.matter.remaining_mj = 0,
                3 | 4 => {
                    assert!(matches!(
                        r.matter.ground(&mut r.world, [40, 2, 40], e.at),
                        matter::ClaimResult::Applied
                    ));
                }
                5 => {
                    r.world.queue_edit(voxel::world::Edit::dig(40, 2, 40));
                    r.world.flush();
                    assert_eq!(material(&r.world, [40, 2, 40]), 0);
                }
                _ => (),
            }
        }
        if mode == 4 {
            c.material_energy_j = 0;
        }
        let material = mode != 6;
        step::<true>(&mut a, &mut x, &c, material);
        step::<false>(&mut b, &mut y, &c, material);
        same(&a, &x, &b, &y);
        if mode == 3 {
            assert_eq!(x.pending.len(), 1, "duplicate must remain deferred");
            assert_eq!(a.matter.admission.ground[4], 1);
        } else {
            assert!(x.pending.is_empty());
        }
        if material {
            assert_eq!(a.matter.admission.wave_geometry[0], 0);
            assert_eq!(a.matter.admission.wave_geometry[3], 1);
        }
        if mode == 1 {
            assert_eq!(a.surface_work.get(&[40, 2, 40]), Some(&1));
        }
        if mode == 2 {
            assert_eq!(a.matter.admission.ground[1], 1);
        }
    }
}

#[test]
fn resumed_ground_finishes_a_partial_coarse_cell_before_reaching_the_next_actor() {
    for points in [false, true] {
        let (mut a, mut x, c) = fixture(2, points);
        let (mut b, mut y, _) = fixture(2, points);
        x.contact_cursor = a.units.len();
        y.contact_cursor = b.units.len();
        step::<true>(&mut a, &mut x, &c, true);
        step::<false>(&mut b, &mut y, &c, true);
        same(&a, &x, &b, &y);
        assert_eq!(x.queued, 8);
        assert_eq!(a.matter.debris.claims(), 8);
        assert!(x.pending.is_empty());
        // Local-cell waves have no later body samples; legacy points still
        // need bounds to decide whether subsequent subcells visit a body.
        assert_eq!(
            a.matter.admission.wave_geometry,
            [u64::from(points), 0, 1, 0]
        );
    }
}

#[test]
fn deferred_body_cursor_uses_eager_geometry_and_can_resume_after_settling() {
    let (mut a, mut x, c) = fixture(4, false);
    let (mut b, mut y, _) = fixture(4, false);
    for (r, e) in [(&mut a, &mut x), (&mut b, &mut y)] {
        e.pending.clear();
        push(e, body_point(r, e), 1_000_000);
        r.matter.begin(u32::MAX, 1);
        r.matter.tuning.max_flying = 1;
    }
    step::<true>(&mut a, &mut x, &c, true);
    step::<false>(&mut b, &mut y, &c, true);
    same(&a, &x, &b, &y);
    assert!(
        x.body_cursor > 0,
        "fixture must stop inside real body damage"
    );
    assert_eq!(x.contact_cursor % (a.units.len() + 1), 0);
    for _ in 0..2 {
        step::<true>(&mut a, &mut x, &c, true);
        step::<false>(&mut b, &mut y, &c, true);
        same(&a, &x, &b, &y);
    }
    assert_eq!(a.matter.admission.wave_geometry[3], 0);
    assert_eq!(
        a.matter.admission.wave_geometry[0],
        b.matter.admission.wave_geometry[0]
    );
    assert!(a.matter.admission.wave_geometry[1] > 0);
    assert!(a.matter.admission.wave_geometry[1] < b.matter.admission.wave_geometry[1]);
    for r in [&mut a, &mut b] {
        r.matter.tuning.max_flying = 128;
    }
    step::<true>(&mut a, &mut x, &c, true);
    step::<false>(&mut b, &mut y, &c, true);
    same(&a, &x, &b, &y);
    assert_eq!(a.units[0].removed.len(), 4);
    assert!(x.pending.is_empty());
}

#[test]
fn saved_world_contact_resumes_with_the_same_frozen_geometry_and_payment() {
    let (mut a, mut x, c) = fixture(2, false);
    let (mut b, mut y, _) = fixture(2, false);
    for (r, e) in [(&mut a, &mut x), (&mut b, &mut y)] {
        e.contact_cursor = r.units.len();
        r.matter.tuning.max_flying = 0;
    }
    step::<true>(&mut a, &mut x, &c, true);
    step::<false>(&mut b, &mut y, &c, true);
    let bytes = wave_bytes(&y);
    let mut scan = Scan::new(&bytes);
    y = Explosion::load(&mut scan, &b.units, b.size, true).unwrap();
    y.load_contacts(&mut scan, &b.units, false).unwrap();
    assert!(scan.ok() && scan.left() == 0);
    same(&a, &x, &b, &y);
    for r in [&mut a, &mut b] {
        r.matter.tuning.max_flying = 128;
    }
    step::<true>(&mut a, &mut x, &c, true);
    step::<false>(&mut b, &mut y, &c, true);
    same(&a, &x, &b, &y);
    assert!(x.pending.is_empty());
    assert_eq!(x.queued, 8);
}

fn fair_step(r: &mut BlastRange, e: &mut Explosion, c: &ExplosionConfig, budget: usize) {
    e.step_with_room::<true>(
        &mut r.world,
        budget,
        &mut r.units,
        &mut r.surface_work,
        &mut r.surface_changed,
        c,
        Some(&mut r.matter),
        None,
    );
}

fn saved_queue(e: &Explosion) -> Vec<u8> {
    let mut b = Blob::new();
    e.save(&mut b);
    e.save_contacts(&mut b);
    e.save_resumes(&mut b);
    b.v
}

#[test]
fn battle_damage_reaches_a_body_before_an_older_crater_can_drain() {
    let (mut a, mut x, c) = fixture(4, false);
    let (mut legacy, mut old, _) = fixture(4, false);
    for (r, e) in [(&mut a, &mut x), (&mut legacy, &mut old)] {
        let p = body_point(r, e);
        push(e, p, 1_000_000);
        e.contact_cursor = r.units.len();
        r.matter.tuning.max_flying = 1;
        // One-cell pieces: every crater cell is a piece of its own and needs
        // a slot of its own, so the crater really waits on the pool. With
        // larger boxes its cells join one reserved piece (ADR-659) and the
        // crater never waits.
        r.matter.fragment_size(1);
    }
    for _ in 0..2 {
        fair_step(&mut a, &mut x, &c, 2);
        step::<true>(&mut legacy, &mut old, &c, true);
        for r in [&mut a, &mut legacy] {
            assert!(r.matter.debris.fragment_pools(&r.matter.tuning).flying <= 1);
            assert!(r.matter.economy.ledger.balanced());
            flush(r);
        }
    }
    assert!(
        !a.units[0].removed.is_empty(),
        "unit hit starved behind crater"
    );
    assert!(
        legacy.units[0].removed.is_empty(),
        "fixture no longer reproduces the old failure"
    );
    assert!(x.working(), "the unpaid terrain must remain queued");
    assert!(a.matter.admission.ground[2] > 0);
    for _ in 0..256 {
        fair_step(&mut a, &mut x, &c, 2);
        flush(&mut a);
        if !x.working() {
            break;
        }
    }
    assert!(!x.working(), "yielded ground contact was lost or stuck");
    assert!(x.contact_resumes.is_empty());
    assert_eq!(a.units[0].removed.len(), 4);
    assert_eq!(
        a.matter.debris.body_removed, 4,
        "paid body damage was replayed"
    );
    assert_eq!(a.matter.debris.removed, x.queued as u64);
    assert!(a.matter.economy.ledger.balanced());
}

#[test]
fn yielded_body_and_ground_cursors_restore_without_repaying_their_prefixes() {
    let (mut a, mut x, c) = fixture(4, false);
    let (mut b, mut y, _) = fixture(4, false);
    for (r, e) in [(&mut a, &mut x), (&mut b, &mut y)] {
        push(e, body_point(r, e), 1_000_000);
        e.contact_cursor = r.units.len();
        r.matter.tuning.max_flying = 1;
    }
    for turn in 0..256 {
        fair_step(&mut a, &mut x, &c, 2);
        fair_step(&mut b, &mut y, &c, 2);
        if turn < 3 {
            assert!(!y.contact_resumes.is_empty());
            let bytes = saved_queue(&y);
            let mut s = Scan::new(&bytes);
            y = Explosion::load(&mut s, &b.units, b.size, true).unwrap();
            y.load_contacts(&mut s, &b.units, false).unwrap();
            y.load_resumes(&mut s, &b.units).unwrap();
            assert!(s.ok() && s.left() == 0);
        }
        same(&a, &x, &b, &y);
        assert_eq!(saved_queue(&x), saved_queue(&y));
        flush(&mut a);
        flush(&mut b);
        if !x.working() {
            break;
        }
    }
    assert!(!x.working());
    assert_eq!(a.matter.debris.body_removed, 4);
    assert!(a.matter.economy.ledger.balanced());
}

#[test]
fn yielded_queue_rejects_forged_cell_cursor_and_unbounded_counts() {
    let (mut r, mut e, c) = fixture(4, false);
    let p = body_point(&r, &e);
    push(&mut e, p, 1_000_000);
    e.contact_cursor = r.units.len();
    r.matter.tuning.max_flying = 0;
    fair_step(&mut r, &mut e, &c, 2);
    assert_eq!(e.contact_resumes.len(), 1);
    let mut bytes = Blob::new();
    e.save_resumes(&mut bytes);
    for forged in 0..4 {
        let mut bad = bytes.v.clone();
        match forged {
            0 => bad[..8].copy_from_slice(&u64::MAX.to_le_bytes()),
            1 => bad[8..20].copy_from_slice(
                &e.pending
                    .front()
                    .unwrap()
                    .cell
                    .into_iter()
                    .flat_map(i32::to_le_bytes)
                    .collect::<Vec<_>>(),
            ),
            2 => bad[20..28].copy_from_slice(&u64::MAX.to_le_bytes()),
            _ => bad[28..36].copy_from_slice(&u64::MAX.to_le_bytes()),
        }
        e.contact_resumes.clear();
        assert!(e.load_resumes(&mut Scan::new(&bad), &r.units).is_none());
    }
}

#[test]
fn battle_contact_queue_obeys_small_and_zero_work_slices() {
    let (mut r, _, mut c) = fixture(1, false);
    c.energy = 3500;
    let mut e = Explosion::with_damage(&r.world, [40, 2, 40], &c, &r.units, &r.surface_work);
    r.matter.tuning.max_flying = 0;
    for _ in 0..32 {
        let pops = e.wave.examined;
        fair_step(&mut r, &mut e, &c, 3);
        assert!(e.wave.examined - pops <= 3);
        assert!(e.pending.len() <= 3);
        assert!(e.contact_resumes.len() < e.pending.len());
        let queue = saved_queue(&e);
        let state = r.snapshot();
        fair_step(&mut r, &mut e, &c, 0);
        assert_eq!(saved_queue(&e), queue);
        assert_eq!(r.snapshot(), state);
    }
    assert_eq!(
        e.pending.len(),
        3,
        "fixture must exhaust material admission"
    );
    assert!(r.matter.economy.ledger.balanced());
}

// Reference keeps eager actor transforms while using the same fair queue rules.
fn fair_eager(r: &mut BlastRange, e: &mut Explosion, c: &ExplosionConfig, budget: usize) {
    r.matter.begin_body_batch();
    e.step_contacts::<false, true>(
        &mut r.world,
        budget,
        &mut r.units,
        &mut r.surface_work,
        &mut r.surface_changed,
        c,
        Some(&mut r.matter),
    );
    r.matter.finish_body_batch();
}

#[test]
fn lazy_wave_frames_match_eager_fair_queue_after_capacity_and_frozen_pose_changes() {
    for points in [false, true] {
        let (mut a, mut x, c) = fixture(2, points);
        let (mut b, mut y, _) = fixture(2, points);
        for (r, e) in [(&mut a, &mut x), (&mut b, &mut y)] {
            push(e, body_point(r, e), 1_000_000);
            e.contact_cursor = r.units.len();
            r.matter.tuning.max_flying = 0;
        }
        for turn in 0..64 {
            if turn == 3 {
                // Lazy construction must use the captured frames, even when
                // the live actor has turned and moved before room returns.
                for r in [&mut a, &mut b] {
                    r.units[0].motion.pos[0] = r.units[0].motion.pos[0].add(Fx::from_int(6));
                    r.units[0].update_pose(1234);
                    r.matter.tuning.max_flying = 128;
                }
            }
            let budget = [0, 1, 2, 8][turn % 4];
            fair_step(&mut a, &mut x, &c, budget);
            fair_eager(&mut b, &mut y, &c, budget);
            same(&a, &x, &b, &y);
            assert_eq!(saved_queue(&x), saved_queue(&y));
            flush(&mut a);
            flush(&mut b);
            same(&a, &x, &b, &y);
            if !x.working() {
                break;
            }
        }
        assert!(!x.working(), "point sampling={points}");
        assert!(a.matter.debris.body_removed > 0, "no real body contact");
        assert!(a.matter.admission.ground[0] > 0, "no real ground contact");
        assert!(a.matter.admission.wave_geometry[1] < b.matter.admission.wave_geometry[1]);
    }
}

#[test]
fn ground_only_fair_queue_never_prepares_actor_frames() {
    let (mut a, mut x, c) = fixture(1, false);
    let (mut b, mut y, _) = fixture(1, false);
    for e in [&mut x, &mut y] {
        push(e, [41, 2, 40], 1_000_000);
        push(e, [42, 2, 40], 1_000_000);
    }
    for (r, e) in [(&mut a, &mut x), (&mut b, &mut y)] {
        e.contact_cursor = r.units.len();
        r.matter.tuning.max_flying = 0;
    }
    for turn in 0..8 {
        if turn == 3 {
            a.matter.tuning.max_flying = 128;
            b.matter.tuning.max_flying = 128;
        }
        fair_step(&mut a, &mut x, &c, 3);
        fair_eager(&mut b, &mut y, &c, 3);
        same(&a, &x, &b, &y);
        assert_eq!(saved_queue(&x), saved_queue(&y));
        assert_eq!(a.matter.admission.wave_geometry[1], 0);
        flush(&mut a);
        flush(&mut b);
    }
    assert!(!x.working());
    assert_eq!(x.queued, 3);
    assert!(b.matter.admission.wave_geometry[1] > 0);
}

// Author real soil in each tested coarse cell so every queue entry must
// actually claim matter; procedural air must not make the probe pass empty.
fn soil_contacts(r: &mut BlastRange, e: &Explosion, cells: &[[i32; 3]]) {
    for &p in cells {
        for dz in 0..e.cell {
            for dy in 0..e.cell {
                for dx in 0..e.cell {
                    r.world.queue_edit(voxel::world::Edit::put(
                        (p[0] + dx) as u32,
                        (p[1] + dy) as u32,
                        (p[2] + dz) as u32,
                        1,
                    ));
                }
            }
        }
    }
    while r.world.flush().queued > 0 {}
}

#[test]
fn completed_body_prefixes_skip_bounds_and_preserve_every_ground_subcell_on_restore() {
    for (edge, points) in [(1, false), (1, true), (2, false), (4, false)] {
        let (mut a, mut x, c) = fixture(edge, points);
        let (mut b, mut y, _) = fixture(edge, points);
        for (r, e) in [(&mut a, &mut x), (&mut b, &mut y)] {
            // This column is outside every physical body. Run the real body
            // prefix before refusing ground, instead of forging its cursors.
            push(e, [40, 2, 40 + edge], 1_000_000);
            push(e, [40, 2, 40 + edge * 2], 1_000_000);
            soil_contacts(
                r,
                e,
                &[[40, 2, 40], [40, 2, 40 + edge], [40, 2, 40 + edge * 2]],
            );
            r.matter.tuning.max_flying = 0;
        }
        fair_step(&mut a, &mut x, &c, 3);
        fair_eager(&mut b, &mut y, &c, 3);
        same(&a, &x, &b, &y);
        assert_eq!(saved_queue(&x), saved_queue(&y));
        assert_eq!(x.pending.len(), 3, "real ground waits required");
        assert!(x.only_ground_contacts(a.units.len()));
        let bytes = saved_queue(&y);
        let mut scan = Scan::new(&bytes);
        y = Explosion::load(&mut scan, &b.units, b.size, true).unwrap();
        y.load_contacts(&mut scan, &b.units, false).unwrap();
        y.load_resumes(&mut scan, &b.units).unwrap();
        assert!(scan.ok() && scan.left() == 0);
        let bounds = a.matter.admission.wave_geometry[0];
        for turn in 0..128 {
            if turn == 3 {
                for r in [&mut a, &mut b] {
                    r.units[0].motion.pos[0] = r.units[0].motion.pos[0].add(Fx::from_int(6));
                    r.units[0].update_pose(1234);
                    r.matter.tuning.max_flying = 128;
                }
            }
            let budget = [0, 1, 3, 8][turn % 4];
            fair_step(&mut a, &mut x, &c, budget);
            fair_eager(&mut b, &mut y, &c, budget);
            same(&a, &x, &b, &y);
            assert_eq!(saved_queue(&x), saved_queue(&y));
            assert_eq!(a.matter.admission.wave_geometry[0], bounds);
            flush(&mut a);
            flush(&mut b);
            if !x.working() {
                break;
            }
        }
        assert!(!x.working(), "edge={edge}");
        assert!(a.matter.admission.wave_ground_only[0] > 0);
        assert!(a.matter.admission.wave_ground_only[1] > 0);
        assert!(a.matter.admission.ground[0] > 0);
        // A completed pressure queue can still own conditional world edits.
        // Confirm every admitted claim too, without discarding its matter.
        for _ in 0..256 {
            if a.matter.debris.claims() == 0 {
                break;
            }
            flush(&mut a);
            flush(&mut b);
            same(&a, &x, &b, &y);
        }
        assert_eq!(a.matter.debris.claims(), 0);
        assert_eq!(a.matter.debris.removed, x.queued as u64);
    }
}

#[test]
fn legacy_early_ground_cursor_keeps_later_body_samples() {
    let (mut a, mut x, c) = fixture(4, true);
    let (mut b, mut y, _) = fixture(4, true);
    for (r, e) in [(&mut a, &mut x), (&mut b, &mut y)] {
        e.pending.clear();
        push(e, body_point(r, e), 1_000_000);
        e.contact_cursor = r.units.len();
        r.matter.tuning.max_flying = 0;
    }
    // The first ground subcell is below the body. Later samples in this same
    // coarse cell do hit it; the ground cursor alone cannot bypass bounds.
    fair_step(&mut a, &mut x, &c, 1);
    fair_eager(&mut b, &mut y, &c, 1);
    same(&a, &x, &b, &y);
    assert_eq!(a.matter.admission.wave_ground_only[0], 0);
    for r in [&mut a, &mut b] {
        r.matter.tuning.max_flying = 128;
    }
    for _ in 0..128 {
        fair_step(&mut a, &mut x, &c, 1);
        fair_eager(&mut b, &mut y, &c, 1);
        same(&a, &x, &b, &y);
        assert_eq!(saved_queue(&x), saved_queue(&y));
        flush(&mut a);
        flush(&mut b);
        if !x.working() {
            break;
        }
    }
    assert!(!x.working());
    assert!(
        a.matter.debris.body_removed > 0,
        "legacy body sample was skipped"
    );
}

#[test]
fn new_body_contact_leaves_the_proven_ground_only_path() {
    let (mut a, mut x, c) = fixture(2, false);
    let (mut b, mut y, _) = fixture(2, false);
    for (r, e) in [(&mut a, &mut x), (&mut b, &mut y)] {
        push(e, [40, 2, 42], 1_000_000);
        soil_contacts(r, e, &[[40, 2, 40], [40, 2, 42]]);
        r.matter.tuning.max_flying = 0;
    }
    for _ in 0..3 {
        fair_step(&mut a, &mut x, &c, 3);
        fair_eager(&mut b, &mut y, &c, 3);
        same(&a, &x, &b, &y);
    }
    assert!(a.matter.admission.wave_ground_only[0] > 0);
    for (r, e) in [(&mut a, &mut x), (&mut b, &mut y)] {
        push(e, body_point(r, e), 1_000_000);
        r.matter.tuning.max_flying = 1;
    }
    for _ in 0..128 {
        fair_step(&mut a, &mut x, &c, 2);
        fair_eager(&mut b, &mut y, &c, 2);
        same(&a, &x, &b, &y);
        assert_eq!(saved_queue(&x), saved_queue(&y));
        flush(&mut a);
        flush(&mut b);
        if !x.working() {
            break;
        }
    }
    assert!(!x.working());
    assert!(
        a.matter.debris.body_removed > 0,
        "new body contact was starved"
    );
    assert!(a.matter.admission.ground[0] > 0);
}
