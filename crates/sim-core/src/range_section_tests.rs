use super::*;
use crate::blast::BlastRange;
use modding::{
    explosion::ExplosionConfig,
    range::{RangeLayout, RangeUnitDef, RangeUnitKind},
};

#[test]
fn sections_born_below_the_crater_floor_keep_matter_and_roundtrip() {
    let mut r = damaged_roots_and_legs();
    // The shared fixture includes a pending terrain claim. Confirm it before
    // measuring the sections, so their boundary correction is the only change.
    r.world.flush();
    r.matter
        .debris
        .confirm(r.world.completed(), &mut r.matter.economy, &r.matter.tuning);
    for u in &mut r.units {
        u.pose.base.y = Fx::from_int(-80);
        for frame in &mut u.seg_frames {
            frame.base.y = Fx::from_int(-80);
        }
    }
    r.matter.step_unit_sections(&mut r.units);
    assert!(
        r.matter
            .debris
            .fragments
            .iter()
            .any(|f| f.chunk.is_some() && f.position.y < Fx::ZERO),
        "fixture must shed a real section below the map"
    );
    let before: u64 = r
        .matter
        .debris
        .fragments
        .iter()
        .map(|f| r.matter.economy.ledger.held(f.held))
        .sum();
    let count = r.matter.debris.fragments.len();
    r.world.flush();
    r.matter.flush(&r.world, true, 98, 0, &[], &[]);
    assert_eq!(count, r.matter.debris.fragments.len());
    assert_eq!(
        before,
        r.matter
            .debris
            .fragments
            .iter()
            .map(|f| r.matter.economy.ledger.held(f.held))
            .sum::<u64>()
    );
    assert!(r
        .matter
        .debris
        .fragments
        .iter()
        .all(|f| f.position.y >= Fx::ZERO));
    assert!(r.matter.economy.ledger.balanced());
    let bytes = r.snapshot();
    let restored = BlastRange::restore(r.config, &bytes).expect("below-floor section snapshot");
    assert_eq!(bytes, restored.snapshot());
    assert!(restored.matter.economy.ledger.balanced());
}

fn damaged_roots_and_legs() -> BlastRange {
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.course = None;
    layout.blocks.clear();
    layout.shields.clear();
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    layout.units = [
        RangeUnitKind::Tank,
        RangeUnitKind::Tank,
        RangeUnitKind::Strider,
    ]
    .into_iter()
    .enumerate()
    .map(|(id, kind)| RangeUnitDef {
        kind,
        weapon: None,
        allied: true,
        at: [170 + id as i32 * 140, 16, 336],
    })
    .collect();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    for id in 0..2 {
        let u = &mut r.units[id];
        // Cut the barrel into several actual disconnected slabs, retaining
        // the turret/core. All removed material is accounted for as a sink.
        for x in 54..usize::from(u.body.vol.nx) {
            if x == 54 || x % 3 == 0 {
                for z in 0..usize::from(u.body.vol.nz) {
                    for y in 0..usize::from(u.body.vol.ny) {
                        if u.body.remove(x, y, z) {
                            u.removed.push([x as u8, y as u8, z as u8]);
                        }
                    }
                }
            }
        }
        u.revision = u.removed.len() as u64;
        r.matter.annihilate_body(id, u.removed.len());
        u.refresh_damage(id);
        assert!(u.status.alive && u.section.is_some());
    }
    let id = 2;
    let u = &mut r.units[id];
    let mounts: BTreeSet<_> = u
        .body
        .legs
        .iter()
        .flat_map(|leg| u.segment_mounts(usize::from(leg.thigh)).iter().copied())
        .collect();
    for c in mounts {
        let [x, y, z] = c.map(usize::from);
        if u.body.remove(x, y, z) {
            u.removed.push(c);
        }
    }
    u.revision = u.removed.len() as u64;
    r.matter.annihilate_body(id, u.removed.len());
    u.refresh_damage(id);
    assert!(!u.seg_shed.is_empty());
    // Sleepers occupy parked slots, not flying slots. Include them before
    // the new root/leg chunks to exercise both independent capacities.
    for i in 0..3 {
        let amount = r.matter.economy.per_voxel(3);
        let held = r
            .matter
            .economy
            .ledger
            .create(Kind::Debris, amount, u64::MAX);
        r.matter.debris.next_id += 1;
        r.matter.debris.fragments.push(Fragment {
            id: r.matter.debris.next_id,
            position: V3::new(
                Fx::from_int(100 + i * 10),
                Fx::from_int(20),
                Fx::from_int(100),
            ),
            velocity: V3::ZERO,
            material: 3,
            paint: None,
            mm: 100,
            basis: crate::burst::IDENTITY,
            mass_g: 7850,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held,
            resting: true,
        });
    }
    // A pending terrain dig already owns a flying and working slot.
    let p = [240, 8, 240];
    let mat = r.world.material(p[0], p[1], p[2]);
    assert_ne!(mat, AIR);
    assert!(r.matter.debris.claim_voxel(
        &mut r.world,
        p,
        mat,
        p.map(|v| v as i32),
        0,
        1,
        &r.matter.tuning,
    ));
    r
}

#[test]
fn section_room_matches_recount_for_roots_limbs_claims_and_both_pool_caps() {
    for (flying, working, parked) in [
        (0, 32768, 131072),
        (1, 32768, 131072),
        (3, 32768, 131072),
        (4096, 2, 131072),
        (4096, 3, 0),
        (4096, 32768, 131072),
    ] {
        let mut fast = damaged_roots_and_legs();
        let mut slow = damaged_roots_and_legs();
        let stock = fast.matter.tuning.clone();
        for r in [&mut fast, &mut slow] {
            r.matter.tuning.max_flying = flying;
            r.matter.tuning.max_fragments = working;
            r.matter.tuning.max_resting_fragments = parked;
        }
        for phase in 0..48 {
            if phase == 8 {
                fast.matter.tuning = stock.clone();
                slow.matter.tuning = stock.clone();
            }
            let before = fast.matter.admission.room_scans;
            fast.matter.step_unit_sections(&mut fast.units);
            let mut reference = SectionRoom {
                available: None,
                recount: true,
            };
            let work = slow
                .matter
                .step_sections_in(&mut slow.units, &mut reference);
            slow.matter
                .shed_segments_in(&mut slow.units, work, &mut reference);
            assert!(fast.matter.admission.room_scans - before <= 1);
            assert_eq!(
                fast.snapshot(),
                slow.snapshot(),
                "phase={phase}, flying={flying}, working={working}, parked={parked}"
            );
            assert!(fast.matter.economy.ledger.balanced());
            // Chunks cannot bypass the claim's reservation or consume parked
            // capacity: whenever there was room, the cached debit stays exact.
            if phase >= 8 {
                assert_eq!(fast.matter.debris.claims(), 1);
            }
        }
        assert!(fast
            .units
            .iter()
            .all(|u| u.section.is_none() && u.seg_shed.is_empty()));
        assert!(
            fast.matter
                .debris
                .fragments
                .iter()
                .filter(|f| f.chunk.is_some())
                .count()
                > 4
        );
        assert!(fast.matter.admission.room_scans < slow.matter.admission.room_scans);
        let copy = BlastRange::restore(fast.config.clone(), &fast.snapshot()).unwrap();
        assert_eq!(fast.state_hash(), copy.state_hash());
    }
}

#[test]
fn section_room_does_not_scan_a_phase_without_ready_sections() {
    let config =
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    let mut r = BlastRange::with_layout(config, layout).unwrap();
    let before = r.snapshot();
    r.matter.step_unit_sections(&mut r.units);
    assert_eq!(r.matter.admission.room_scans, 0);
    assert_eq!(r.snapshot(), before);
}
