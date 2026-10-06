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
    let mut r = BlastRange::with_layout(c.clone(), layout).unwrap();
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
        e.step_contacts::<false>(
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
        // A successful first ground retry still needs geometry for the rest.
        assert_eq!(a.matter.admission.wave_geometry, [1, 3, 1, 0]);
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
        a.matter.admission.wave_geometry,
        b.matter.admission.wave_geometry
    );
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
