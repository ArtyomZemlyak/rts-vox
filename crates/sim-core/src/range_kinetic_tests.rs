//! Material, replay and moving-contact regressions under fragment pressure.
use super::*;
use fx::blob::{Blob, Scan};

fn range() -> BlastRange {
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/kinetic-shell.ron")).unwrap();
    c.spall = None;
    c.fragment_box = 1;
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.size = [640, 128, 640];
    layout.course = None;
    layout.blocks.clear();
    layout.shields.clear();
    layout.units.truncate(1);
    layout.units[0].at = [285, 16, 336];
    let mut other = layout.units[0].clone();
    other.at = [439, 16, 336];
    layout.units.push(other);
    BlastRange::with_layout(c, layout).unwrap()
}
fn shot(r: &mut BlastRange) {
    let p = r.units[0].world_pos([30, 11, 0]);
    assert_eq!(r.units[0].hit(p), Some((0, [30, 11, 0])));
    let c = r.config.clone();
    r.kinetic_hit_at(
        p,
        p.map(Fx::from_int),
        [Fx::ZERO, Fx::ZERO, Fx::from_int(c.shell_speed)],
        &c,
    );
}
fn occupy(r: &mut BlastRange) {
    let cell = [40, 11, 0];
    let c = r.config.clone();
    let u = &mut r.units[0];
    assert!(u.body.vol.get(cell[0], cell[1], cell[2]));
    r.matter.begin(c.material_energy_j, 1);
    assert!(matches!(
        r.matter
            .body(0, u, cell, &u.root_frame(cell[1]), u.world_pos(cell)),
        matter::ClaimResult::Applied
    ));
    u.receive_cell(cell, unit_work(&u.body, cell, &c), &c);
    u.detached += 1;
    u.refresh_damage(0);
    r.matter.tuning.max_flying = 1;
    r.matter.tuning.max_fragments = 1;
    r.matter.tuning.max_resting_fragments = 1024;
}
fn settle(r: &mut BlastRange) {
    for f in &mut r.matter.debris.fragments {
        f.resting = true;
        f.velocity = phys::V3::ZERO;
        crate::burst::heap(f, &r.world);
    }
}
fn complete(r: &mut BlastRange) {
    for _ in 0..512 {
        if r.pending_kinetic_impacts() == 0 {
            return;
        }
        settle(r);
        r.step_kinetic_jobs();
        assert!(r.matter.economy.ledger.balanced());
    }
    panic!("finite impact never completed");
}
fn same_damage(a: &BlastRange, b: &BlastRange) {
    for (a, b) in a.units.iter().zip(&b.units) {
        assert_eq!(a.removed, b.removed);
        assert_eq!(a.dents, b.dents);
        assert_eq!(a.seg_removed, b.seg_removed);
        assert_eq!(a.seg_dents, b.seg_dents);
        assert_eq!(a.detached, b.detached);
        assert_eq!(a.seg_detached, b.seg_detached);
    }
    assert_eq!(a.matter.spent_mj, b.matter.spent_mj);
    assert_eq!(a.matter.supplied_mj, b.matter.supplied_mj);
}
#[test]
fn deferred_contact_waits_without_repaying_and_follows_original_local_cells() {
    let mut r = range();
    occupy(&mut r);
    let mut reference = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    shot(&mut reference);
    let before = r.units[0].revision;
    shot(&mut r);
    assert_eq!(r.units[0].revision, before);
    assert_eq!(r.pending_kinetic_impacts(), 1);
    let supplied = r.matter.supplied_mj;
    for _ in 0..10 {
        r.step_kinetic_jobs();
    }
    assert_eq!(r.matter.supplied_mj, supplied);
    assert_eq!(r.units[0].revision, before);
    r.units[0].motion.pos[0] = r.units[0].motion.pos[0].add(Fx::from_int(100));
    r.units[0].motion.drive.head = fx::angle::QUARTER;
    r.units[0].update_pose(fx::angle::QUARTER);
    // A different hull entering the old ray must not take this old hit.
    r.units[1].motion.pos = [285, 16, 336].map(Fx::from_int);
    r.units[1].update_pose(0);
    complete(&mut r);
    assert_eq!(r.units[0].removed, reference.units[0].removed);
    assert!(r.units[1].removed.is_empty());
    assert!(r.units[1].dents.is_empty());
    assert_eq!(r.matter.supplied_mj, supplied);
    assert_eq!(r.matter.spent_mj, reference.matter.spent_mj);
}
#[test]
fn partially_applied_channel_survives_save_and_matches_unconstrained_damage() {
    let mut r = range();
    let mut reference = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    shot(&mut reference);
    r.matter.tuning.max_flying = 1;
    r.matter.tuning.max_fragments = 1;
    r.matter.tuning.max_resting_fragments = 1024;
    shot(&mut r);
    assert!(r.units[0].removed.len() > 0);
    assert!(r.units[0].removed.len() < reference.units[0].removed.len());
    assert_eq!(r.pending_kinetic_impacts(), 1);
    assert_eq!(
        u32::from_le_bytes(r.snapshot()[..4].try_into().unwrap()),
        49
    );
    let mut restored =
        BlastRange::restore(r.config.clone(), &r.snapshot()).expect("pending snapshot");
    restored.matter.tuning = r.matter.tuning.clone();
    assert_eq!(r.snapshot(), restored.snapshot());
    complete(&mut r);
    complete(&mut restored);
    assert_eq!(r.snapshot(), restored.snapshot());
    same_damage(&r, &reference);
    assert_eq!(
        u32::from_le_bytes(r.snapshot()[..4].try_into().unwrap()),
        48
    );
}
#[test]
fn deferred_lateral_share_keeps_cursor_finite_work_and_replay() {
    let mut r = range();
    r.config.energy = 100;
    r.config.unit_ablation_resistance = 10;
    r.layout.as_mut().unwrap().unit_resistance = 10;
    r.config.unit_role_resistance_milli = [1000; 5];
    r.config.spall = Some(modding::explosion::Spall {
        radius: 1,
        work_milli: 900,
    });
    let mut reference = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    shot(&mut reference);
    r.matter.tuning.max_flying = 1;
    r.matter.tuning.max_fragments = 1;
    r.matter.tuning.max_resting_fragments = 1024;
    shot(&mut r);
    let job = r.kinetic.jobs.front().expect("spall must wait");
    assert!(job.spall.is_some());
    assert_eq!(job.cursor, job.ray.len());
    let mut b = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    b.matter.tuning = r.matter.tuning.clone();
    r.streams.tuning.kinetic.cells_per_tick = 1;
    b.streams.tuning.kinetic.cells_per_tick = 1;
    let start = r.kinetic_progress()[1];
    r.step_kinetic_jobs();
    assert_eq!(r.kinetic_progress()[1] - start, 1);
    b.step_kinetic_jobs();
    assert_eq!(r.snapshot(), b.snapshot());
    complete(&mut r);
    complete(&mut b);
    same_damage(&r, &reference);
    assert_eq!(r.snapshot(), b.snapshot());
    let work: u64 = r.units[0]
        .removed
        .iter()
        .map(|c| u64::from(unit_work(&r.units[0].body, c.map(usize::from), &r.config)))
        .sum::<u64>()
        + r.units[0]
            .dents
            .values()
            .map(|n| u64::from(*n))
            .sum::<u64>();
    assert!(work <= 100, "created work: {work}");
}
#[test]
fn corrupt_and_truncated_pending_tail_is_rejected_without_allocating_unbounded_lists() {
    let mut r = range();
    occupy(&mut r);
    shot(&mut r);
    let mut b = Blob::new();
    r.kinetic.save(&mut b);
    for n in 0..b.v.len() {
        assert!(
            Pending::load(&mut Scan::new(&b.v[..n]), &r.config, &r.units, r.size, 128).is_none(),
            "accepted cut {n}"
        );
    }
    b.v[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Pending::load(&mut Scan::new(&b.v), &r.config, &r.units, r.size, 128).is_none());
}

#[test]
fn consecutive_retry_batches_reuse_one_capacity_scan_without_changing_physics() {
    let mut cached = range();
    occupy(&mut cached);
    shot(&mut cached);
    let at = cached.units[0].world_pos([34, 11, 0]);
    let config = cached.config.clone();
    cached.kinetic_hit_at(
        at,
        at.map(Fx::from_int),
        [Fx::ZERO, Fx::ZERO, Fx::from_int(config.shell_speed)],
        &config,
    );
    assert_eq!(cached.pending_kinetic_impacts(), 2);
    let mut reference = BlastRange::restore(cached.config.clone(), &cached.snapshot()).unwrap();
    reference.matter.tuning = cached.matter.tuning.clone();
    for _ in 0..128 {
        if cached.pending_kinetic_impacts() == 0 {
            break;
        }
        settle(&mut cached);
        settle(&mut reference);
        let scans = cached.matter.admission.room_scans;
        cached.step_kinetic_jobs();
        assert_eq!(cached.matter.admission.room_scans - scans, 1);
        let mut budget = reference.streams.tuning.kinetic.cells_per_tick;
        for _ in 0..reference.kinetic.jobs.len() {
            if budget == 0 {
                break;
            }
            let mut job = reference.kinetic.jobs.pop_front().unwrap();
            if !reference.run_kinetic_job(&mut job, &mut budget, None) {
                reference.kinetic.jobs.push_back(job);
            }
        }
        assert_eq!(cached.snapshot(), reference.snapshot());
        assert!(cached.matter.economy.ledger.balanced());
    }
    assert_eq!(cached.pending_kinetic_impacts(), 0);
}
