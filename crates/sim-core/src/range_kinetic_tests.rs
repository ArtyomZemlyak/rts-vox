//! Material, replay and moving-contact regressions under fragment pressure.
use super::*;
use fx::blob::{Blob, Scan};

fn range() -> BlastRange {
    range_with_t2(false)
}
fn range_with_t2(include_t2: bool) -> BlastRange {
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
    if include_t2 {
        layout.clear_min = [64, 64];
        layout.clear_max = [600, 600];
        layout.units.push(modding::range::RangeUnitDef {
            kind: RangeUnitKind::ArtilleryTurretT2,
            allied: true,
            at: [150, 16, 500],
            weapon: None,
        });
    }
    BlastRange::with_layout(c, layout).unwrap()
}
// Reference damage runs without the deliberately small working/flying pool.
// Restore now preserves tuning; opening the pool must be an explicit test action.
#[cfg(test)]
fn unconstrained(r: &BlastRange) -> BlastRange {
    let mut reference = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    assert_eq!(reference.matter.tuning, r.matter.tuning);
    reference.matter.tuning = r.debris_defaults;
    reference
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
#[cfg(test)]
#[test]
fn weapon_configuration_keeps_captured_pending_damage_and_restores_with_another_preset() {
    use crate::blast::battle::commands::{Command, Outcome};
    let mut r = range();
    occupy(&mut r);
    let mut reference = unconstrained(&r);
    shot(&mut reference);
    shot(&mut r);
    assert_eq!(r.pending_kinetic_impacts(), 1);
    let mut c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    c.energy = 1;
    c.unit_role_resistance_milli = [10_000; 5];
    assert_eq!(
        r.execute_command(Command::ConfigureWeapon { config: c }),
        Outcome::Applied
    );
    let mut copy = BlastRange::restore(c, &r.snapshot()).unwrap();
    assert_eq!(r.snapshot(), copy.snapshot());
    complete(&mut r);
    complete(&mut copy);
    same_damage(&r, &reference);
    assert_eq!(r.snapshot(), copy.snapshot());
    assert_eq!(r.pending_kinetic_impacts(), 0);
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
    let mut reference = unconstrained(&r);
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
    let mut reference = unconstrained(&r);
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
        55
    );
    let mut restored =
        BlastRange::restore(r.config.clone(), &r.snapshot()).expect("pending snapshot");
    assert_eq!(restored.matter.tuning, r.matter.tuning);
    assert_eq!(r.snapshot(), restored.snapshot());
    complete(&mut r);
    complete(&mut restored);
    assert_eq!(r.snapshot(), restored.snapshot());
    same_damage(&r, &reference);
    assert_eq!(r.kinetic.version(), 48);
    assert_eq!(
        u32::from_le_bytes(r.snapshot()[..4].try_into().unwrap()),
        55
    );
    r.matter.tuning = r.debris_defaults;
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
    let mut reference = unconstrained(&r);
    shot(&mut reference);
    r.matter.tuning.max_flying = 1;
    r.matter.tuning.max_fragments = 1;
    r.matter.tuning.max_resting_fragments = 1024;
    shot(&mut r);
    let job = r.kinetic.jobs.front().expect("spall must wait");
    assert!(job.spall.is_some());
    assert_eq!(job.cursor, job.ray.len());
    let mut b = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    assert_eq!(b.matter.tuning, r.matter.tuning);
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
            Pending::load(
                &mut Scan::new(&b.v[..n]),
                &r.config,
                &r.units,
                r.size,
                128,
                49
            )
            .is_none(),
            "accepted cut {n}"
        );
    }
    b.v[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Pending::load(&mut Scan::new(&b.v), &r.config, &r.units, r.size, 128, 49).is_none());
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

fn bullet(r: &mut BlastRange, cell: [usize; 3]) {
    let p = r.units[0].world_pos(cell);
    assert_eq!(r.units[0].hit(p), Some((0, cell)));
    let mut tuning = r.streams.tuning.machine_gun.clone();
    tuning.work = 1_000_000;
    r.stream_contact(p, &tuning, false, r.gun.at, &r.config.clone());
}

#[test]
fn machine_gun_continuation_follows_local_cell_and_does_not_repay() {
    let mut r = range();
    occupy(&mut r);
    let mut reference = unconstrained(&r);
    bullet(&mut reference, [30, 11, 0]);
    let before = r.units[0].revision;
    bullet(&mut r, [30, 11, 0]);
    assert_eq!(r.units[0].revision, before);
    assert_eq!(r.pending_machine_gun_contacts(), 1);
    let supplied = r.matter.supplied_mj;
    for _ in 0..30 {
        r.step_kinetic_jobs();
    }
    assert_eq!(r.matter.supplied_mj, supplied);
    r.units[0].motion.pos[0] = r.units[0].motion.pos[0].add(Fx::from_int(100));
    r.units[0].motion.drive.head = fx::angle::QUARTER;
    r.units[0].update_pose(fx::angle::QUARTER);
    r.units[1].motion.pos = [285, 16, 336].map(Fx::from_int);
    r.units[1].update_pose(0);
    complete(&mut r);
    same_damage(&r, &reference);
    assert_eq!(r.machine_gun_continuation_progress(), [1, 31, 31]);
}

#[test]
fn mixed_machine_gun_and_penetrator_queue_replays_and_validates_its_tail() {
    let mut r = range();
    occupy(&mut r);
    bullet(&mut r, [30, 11, 0]);
    shot(&mut r);
    assert_eq!(r.pending_kinetic_impacts(), 2);
    assert_eq!(r.kinetic.version(), 50);
    let mut copy = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    assert_eq!(copy.matter.tuning, r.matter.tuning);
    assert_eq!(r.snapshot(), copy.snapshot());
    complete(&mut r);
    complete(&mut copy);
    assert_eq!(r.snapshot(), copy.snapshot());
    assert_eq!(r.kinetic.version(), 48);
    let mut r = range();
    occupy(&mut r);
    bullet(&mut r, [30, 11, 0]);
    let mut b = Blob::new();
    r.kinetic.save(&mut b);
    for n in 0..b.v.len() {
        assert!(Pending::load(
            &mut Scan::new(&b.v[..n]),
            &r.config,
            &r.units,
            r.size,
            128,
            50
        )
        .is_none());
    }
    // Body target ID is the final 40-byte cell tuple.
    let start = b.v.len() - 40;
    b.v[start..start + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(Pending::load(&mut Scan::new(&b.v), &r.config, &r.units, r.size, 128, 50).is_none());
}

#[test]
fn deferred_machine_gun_does_not_redirect_after_another_event_removes_its_source() {
    let mut r = range();
    occupy(&mut r);
    bullet(&mut r, [30, 11, 0]);
    settle(&mut r);
    // Another bullet takes the exact source first.
    bullet(&mut r, [30, 11, 0]);
    assert!(!r.units[0].body.vol.get(30, 11, 0));
    let supplied = r.matter.supplied_mj;
    let spent = r.matter.spent_mj;
    let removed = r.units[0].removed.clone();
    complete(&mut r);
    assert_eq!(r.matter.supplied_mj, supplied);
    assert_eq!(r.matter.spent_mj, spent);
    assert_eq!(r.units[0].removed, removed);
}

#[test]
fn deferred_machine_gun_preserves_terrain_contact_and_credited_dents() {
    let mut r = range();
    occupy(&mut r);
    let p = [100, 15, 100];
    let mat = material(&r.world, p);
    assert_ne!(mat, 0);
    let mut tuning = r.streams.tuning.machine_gun.clone();
    tuning.work = material_work(mat, &r.config) / 2 + 1;
    r.surface_work.insert(p, tuning.work);
    let mut reference = unconstrained(&r);
    reference.stream_contact(p, &tuning, false, r.gun.at, &r.config.clone());
    r.stream_contact(p, &tuning, false, r.gun.at, &r.config.clone());
    assert_eq!(r.pending_machine_gun_contacts(), 1);
    assert_eq!(material(&r.world, p), mat);
    let supplied = r.matter.supplied_mj;
    let mut restored = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    assert_eq!(restored.matter.tuning, r.matter.tuning);
    complete(&mut r);
    complete(&mut restored);
    r.world.flush();
    restored.world.flush();
    assert_eq!(material(&r.world, p), 0);
    assert!(!r.surface_work.contains_key(&p));
    assert_eq!(r.matter.supplied_mj, supplied);
    assert_eq!(r.matter.spent_mj, reference.matter.spent_mj);
    assert_eq!(r.snapshot(), restored.snapshot());
}

#[test]
fn old_independent_flight_reservations_can_drain_without_overflowing_new_queue() {
    let mut r = range();
    occupy(&mut r);
    let cap = r.streams.tuning.kinetic.max_jobs;
    for _ in 0..cap {
        shot(&mut r);
    }
    assert_eq!(r.kinetic.version(), 49);
    // Old producers could additionally have a full set of MG bullets in flight.
    for _ in 0..cap {
        bullet(&mut r, [30, 11, 0]);
    }
    assert_eq!(r.pending_kinetic_impacts(), cap * 2);
    assert!(!r.solid_flight_room(&[], 1));
    let mut restored = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    assert_eq!(restored.matter.tuning, r.matter.tuning);
    assert_eq!(r.snapshot(), restored.snapshot());
    complete(&mut r);
    complete(&mut restored);
    assert_eq!(r.snapshot(), restored.snapshot());
    assert!(r.solid_flight_room(&[], 1));
}

#[test]
fn stock_machine_gun_dents_are_credited_once_before_and_after_waiting() {
    let mut r = range();
    occupy(&mut r);
    let mut reference = unconstrained(&r);
    let p = r.units[0].world_pos([30, 11, 0]);
    let mut tuning = r.streams.tuning.machine_gun.clone();
    tuning.work = 500;
    // Force multiple real denting hits before destructive fracture.
    r.config.unit_ablation_resistance = 2000;
    reference.config = r.config.clone();
    for _ in 0..32 {
        r.stream_contact(p, &tuning, false, r.gun.at, &r.config.clone());
        reference.stream_contact(p, &tuning, false, r.gun.at, &r.config.clone());
        if r.pending_machine_gun_contacts() > 0 {
            break;
        }
    }
    assert_eq!(r.pending_machine_gun_contacts(), 1);
    assert!(r.units[0].dents.contains_key(&[30, 11, 0]));
    let supplied = r.matter.supplied_mj;
    complete(&mut r);
    same_damage(&r, &reference);
    assert_eq!(r.matter.supplied_mj, supplied);
}

#[test]
fn heavy_artillery_snapshot_preserves_mixed_pending_contacts_and_the_empty_tail() {
    let mut r = range_with_t2(true);
    assert_eq!(
        u32::from_le_bytes(r.snapshot()[..4].try_into().unwrap()),
        52
    );
    occupy(&mut r);
    bullet(&mut r, [30, 11, 0]);
    shot(&mut r);
    assert_eq!(r.pending_kinetic_impacts(), 2);
    let snapshot = r.snapshot();
    assert_eq!(u32::from_le_bytes(snapshot[..4].try_into().unwrap()), 55);
    let mut copy = BlastRange::restore(r.config.clone(), &snapshot).unwrap();
    assert_eq!(copy.matter.tuning, r.matter.tuning);
    assert_eq!(snapshot, copy.snapshot());
    complete(&mut r);
    complete(&mut copy);
    assert_eq!(r.snapshot(), copy.snapshot());
    assert_eq!(r.pending_kinetic_impacts(), 0);
    let empty = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    assert_eq!(r.snapshot(), empty.snapshot());
    assert!(r.matter.economy.ledger.balanced());
}
