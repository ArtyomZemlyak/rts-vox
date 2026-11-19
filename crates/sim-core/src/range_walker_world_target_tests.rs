use super::*;
use super::super::ai::{AiOrder, Decision};
use super::super::walker::world_target_clear;
use modding::range::{RangeBlock, RangeUnitDef};

fn arena(kind: RangeUnitKind, workers: usize, hard_cover: bool, hollow: bool) -> (BlastRange, [i32; 3]) {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-drive.ron")).unwrap();
    l.size = [4096, 2048, 4096];
    l.clear_min = [64, 64];
    l.clear_max = [4032, 4032];
    l.course = None;
    l.shields.clear();
    l.blocks = vec![
        RangeBlock { min: [2100, 16, 1588], max: [2104, 300, 1612], material: 5 },
        RangeBlock { min: [2098, 300, 1586], max: [2106, 304, 1614], material: 4 },
    ];
    if hollow {
        l.blocks.push(RangeBlock { min: [2102, 135, 1598], max: [2103, 145, 1602], material: 0 });
    }
    if hard_cover {
        l.blocks.push(RangeBlock { min: [1600, 16, 1588], max: [1604, 300, 1612], material: 3 });
    }
    l.units = (0..6).map(|i| RangeUnitDef {
        kind: if i == 0 { kind } else { RangeUnitKind::Tank },
        at: if i == 0 { [1100, 16, 1600] } else { [2800 + i * 100, 16, 2800] },
        allied: i == 0,
        weapon: None,
    }).collect();
    let mut aim = [2102, 140, 1600];
    if kind == RangeUnitKind::Commander {
        // Retain the original all-armed test's near-target geometry: this
        // smaller rig fits the original world without the Walker expansion.
        l.size = [768, 128, 768];
        l.clear_min = [40, 40];
        l.clear_max = [728, 728];
        for (i, def) in l.units.iter_mut().enumerate() {
            def.at = if i == 0 { [128, 16, 192] } else { [300 + i as i32 * 60, 16, 540] };
        }
        l.blocks = vec![
            RangeBlock { min: [400, 16, 180], max: [404, 70, 204], material: 5 },
            RangeBlock { min: [398, 70, 178], max: [406, 74, 206], material: 4 },
        ];
        aim = [402, 45, 192];
    }
    let cfg = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(cfg, l).unwrap();
    assert!(r.set_physics_workers(workers));
    assert!(r.set_navigation_workers(workers));
    r.start_battle().unwrap();
    for actor in &mut r.battle.actors {
        actor.order = Order::Hold;
        actor.next = u32::MAX;
    }
    r.battle.actors[0].next = 0;
    assert!(r.battle_order(0, Order::Ground(aim), None));
    (r, aim)
}

#[test]
fn ordered_vegetation_is_a_world_ray_contact_not_a_terrain_ceiling() {
    for hollow in [false, true] {
        let (r, aim) = arena(RangeUnitKind::Walker04, 1, false, hollow);
        let from = [1100, 140, 1600].map(Fx::from_int);
        let yielding = &r.navigation.fell.materials;
        assert!(r.world.column_ceiling(aim[0] as u32, aim[2] as u32) > aim[1] as u32);
        assert!(!voxel::los::line_of_sight(&r.world, (1100, 140, 1600), (2102, 140, 1600)).clear);
        assert!(world_target_clear(&r.world, from, aim, true, yielding));
        assert!(!world_target_clear(&r.world, from, aim, false, yielding));
        assert!(!world_target_clear(&r.world, from, aim, true, &[]));
        assert_eq!(r.world.material(2100, 140, 1600), 5, "eligibility removed physical wood");
        assert_eq!(r.world.material(2102, 140, 1600), if hollow { 0 } else { 5 });
    }
}

#[test]
fn ordered_vegetation_still_rejects_hard_cover_and_buried_points() {
    let (r, aim) = arena(RangeUnitKind::Walker04, 1, true, false);
    let from = [1100, 140, 1600].map(Fx::from_int);
    let yielding = &r.navigation.fell.materials;
    assert!(!world_target_clear(&r.world, from, aim, true, yielding));
    assert!(!world_target_clear(&r.world, from, [2102, 0, 1600], true, yielding));
    assert!(!world_target_clear(&r.world, from, [-1, 140, 1600], true, yielding));
    assert!(!world_target_clear(&r.world, from, [r.world.nx as i32, 140, 1600], true, yielding));
}

fn advance(r: &mut BlastRange) {
    if r.battle.ai.next_tick == r.tick {
        let plan = (1..r.units.len()).map(|actor| Decision { actor, order: AiOrder::Hold }).collect();
        r.submit_enemy_ai_plan(r.tick, plan).unwrap();
    }
    r.step();
    assert!(r.matter.economy.ledger.balanced());
    assert!(r.units.iter().all(|u| u.motion.cut.is_empty()), "contact felling replaced the shot");
}

fn cold_with_other_workers(r: &BlastRange, workers: usize) -> BlastRange {
    let mut copy = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    let other = if workers == 1 { 8 } else { 1 };
    assert!(copy.set_physics_workers(other));
    assert!(copy.set_navigation_workers(other));
    assert_eq!(r.state_hash(), copy.state_hash());
    copy
}

#[test]
fn walker_ordered_wood_fire_and_charge_restore_match_all_workers() {
    let mut reference = None;
    for workers in [1, 2, 4, 8] {
        let (mut r, _) = arena(RangeUnitKind::Walker04, workers, false, false);
        let at = [r.units[0].at[0], r.units[0].at[2]];
        let mut charging: Option<BlastRange> = None;
        let mut shooting: Option<BlastRange> = None;
        let mut hashes = Vec::new();
        for _ in 0..120 {
            advance(&mut r);
            for copy in [&mut charging, &mut shooting].into_iter().flatten() {
                advance(copy);
                assert_eq!(r.state_hash(), copy.state_hash(), "workers={workers},tick={}", r.tick);
            }
            assert_eq!([r.units[0].at[0], r.units[0].at[2]], at, "moved instead of firing");
            if charging.is_none() && r.battle.actors[0].charge.is_some() && r.battle.actors[0].fired == 0 {
                charging = Some(cold_with_other_workers(&r, workers));
            }
            if shooting.is_none() && r.battle.actors[0].fired > 0 {
                shooting = Some(cold_with_other_workers(&r, workers));
            }
            for copy in [&mut charging, &mut shooting].into_iter().flatten() {
                assert_eq!(r.state_hash(), copy.state_hash());
            }
            hashes.push(r.state_hash());
        }
        assert!(charging.is_some(), "no saved charging state");
        assert!(shooting.is_some(), "Walker04 never fired");
        assert!(r.battle.actors[0].fired > 0);
        assert!(r.removed_materials[5] > 0, "physical ray never damaged wood");
        if let Some(expected) = reference.as_ref() {
            assert_eq!(&hashes, expected, "workers={workers}");
        } else {
            reference = Some(hashes);
        }
    }
}

#[test]
fn commander_first_stock_wood_shot_restores_while_arms_are_aimed() {
    let (mut r, _) = arena(RangeUnitKind::Commander, 1, false, false);
    for _ in 0..600 {
        advance(&mut r);
        if r.battle.actors[0].fired > 0 {
            let bytes = r.snapshot();
            let mut last = ("", 0, None);
            let copy = BlastRange::restore_observed(r.config, &bytes, |name, offset, unit| {
                last = (name, offset, unit);
            });
            if copy.is_none() {
                let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out/forest-baseline629");
                std::fs::create_dir_all(&out).unwrap();
                std::fs::write(out.join("commander-firstshot.bin"), &bytes).unwrap();
            }
            assert!(copy.is_some(), "Commander first-shot restore rejected at {last:?}, arms={:?}", r.units[0].arm_targets);
            let mut copy = copy.unwrap();
            assert_eq!(r.state_hash(), copy.state_hash());
            for _ in 0..32 {
                advance(&mut r);
                advance(&mut copy);
                assert_eq!(r.state_hash(), copy.state_hash());
            }
            return;
        }
    }
    panic!("Commander never fired");
}

#[test]
fn commander_pose_codec_rejects_corrupt_reach_and_arm_metadata() {
    use fx::blob::{Blob, Scan};
    let (mut r, _) = arena(RangeUnitKind::Commander, 1, false, false);
    let valid = [[389, 25, 188], [316, -155, 224], [0, 0, 0], [0, 0, 0]];
    for (targets, accepted) in [
        (valid, true),
        ({ let mut p = valid; p[0][0] = i32::MAX; p }, false),
        ({ let mut p = valid; p[1][1] = i32::MIN; p }, false),
        ({ let mut p = valid; p[2][0] = 2; p }, false),
        ({ let mut p = valid; p[2][1] = 1; p }, false),
    ] {
        let mut bytes = Blob::new();
        bytes.u32(1);
        bytes.u32(0);
        for target in targets { for axis in target { bytes.i32(axis); } }
        let mut scan = Scan::new(&bytes.v);
        assert_eq!(r.load_walker_poses(&mut scan).is_some(), accepted, "{targets:?}");
        if accepted {
            assert_eq!(r.units[0].arm_targets, Some(targets));
            assert_eq!(scan.left(), 0);
        }
    }
}
