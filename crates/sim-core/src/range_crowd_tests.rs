use super::*;
use modding::range::{RangeBlock, RangeUnitDef};

fn arena(count: usize, gate: bool) -> BlastRange {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    let side = if count > 10 { 4096 } else { 1600 };
    let cols = if count > 10 { 8 } else { 4 };
    l.size = [side, 128, side];
    l.battle = true;
    l.clear_min = [0, 0];
    l.clear_max = [side as i32; 2];
    l.blocks.clear();
    let c = l.course.as_mut().unwrap();
    c.min = [40, 40];
    c.max = [side as i32 - 40; 2];
    c.ramps.clear();
    c.patrols.clear();
    l.units = (0..count)
        .map(|id| RangeUnitDef {
            allied: true,
            kind: RangeUnitKind::Tank,
            at: [
                260 + (id / cols) as i32 * 90,
                16,
                550 + (id % cols) as i32 * 90,
            ],
            weapon: None,
        })
        .collect();
    if gate {
        l.blocks = vec![
            RangeBlock {
                min: [760, 16, 0],
                max: [800, 64, 640],
                material: 2,
            },
            RangeBlock {
                min: [760, 16, 760],
                max: [800, 64, 1600],
                material: 2,
            },
        ];
    }
    let mut s = BlastRange::with_layout(config(), l).unwrap();
    s.start_free_control().unwrap();
    s
}
fn config() -> ExplosionConfig {
    ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap()
}
fn assert_separate(s: &BlastRange) {
    for a in 0..s.units.len() {
        for b in a + 1..s.units.len() {
            let ua = &s.units[a];
            let ub = &s.units[b];
            assert_eq!(
                motion::Obstacle::of(&ua.motion, &ua.body)
                    .penetration(&motion::Obstacle::of(&ub.motion, &ub.body)),
                Fx::ZERO,
                "intersecting hulls {a}/{b} at {}",
                s.tick
            );
        }
    }
}
#[test]
fn ten_tanks_form_without_contact_and_replay_across_worker_counts() {
    let mut s = arena(10, false);
    s.set_navigation_workers(1);
    let ids: Vec<_> = (0..10).collect();
    let dest = s.battle_formation_destinations(&ids, [1100, 700]);
    assert_eq!(
        dest,
        s.battle_formation_destinations(
            &ids.iter()
                .rev()
                .copied()
                .chain([usize::MAX, 3, 3])
                .collect::<Vec<_>>(),
            [1100, 700]
        )
    );
    assert_eq!(s.battle_group_move(&dest, [1100, 700]), 10);
    for _ in 0..120 {
        s.step();
        assert_separate(&s);
    }
    let bytes = s.snapshot();
    let mut replicas: Vec<_> = [1, 4, 8]
        .into_iter()
        .map(|workers| {
            let mut r = BlastRange::restore(config(), &bytes).unwrap();
            r.set_navigation_workers(workers);
            r.set_physics_workers(workers);
            r
        })
        .collect();
    for tick in 120..900 {
        s.step();
        assert_separate(&s);
        for (id, u) in s.units.iter().enumerate() {
            assert!(!u.motion.blocked, "tank {id} blocked at {tick}");
            assert!(u.motion.drive.vel >= Fx::ZERO);
        }
        for r in &mut replicas {
            r.step();
            assert_eq!(
                s.units
                    .iter()
                    .map(|u| (
                        u.motion.pos,
                        u.motion.drive.head,
                        u.motion.drive.vel,
                        u.motion.destination
                    ))
                    .collect::<Vec<_>>(),
                r.units
                    .iter()
                    .map(|u| (
                        u.motion.pos,
                        u.motion.drive.head,
                        u.motion.drive.vel,
                        u.motion.destination
                    ))
                    .collect::<Vec<_>>()
            );
        }
    }
    assert!(s.units.iter().all(|u| u.motion.destination.is_none()));
    for r in replicas {
        assert_eq!(s.state_hash(), r.state_hash());
        assert_eq!(s.snapshot(), r.snapshot());
    }
    assert!(s.matter.economy.ledger.balanced());
}
#[test]
fn sixty_four_tanks_complete_as_one_group_without_intersections() {
    let mut s = arena(64, false);
    let dest = s.battle_formation_destinations(&(0..64).collect::<Vec<_>>(), [2200, 1800]);
    assert_eq!(s.battle_group_move(&dest, [2200, 1800]), 64);
    let (mut blocked, mut reverse) = (0, 0);
    for _ in 0..2400 {
        s.step();
        assert_separate(&s);
        blocked += s.units.iter().filter(|u| u.motion.blocked).count();
        reverse += s
            .units
            .iter()
            .filter(|u| u.motion.drive.vel < Fx::ZERO)
            .count();
    }
    assert!(s.units.iter().all(|u| u.motion.destination.is_none()));
    assert!(
        blocked == 0 && reverse == 0,
        "crowd recovery inflated: {blocked}/{reverse}"
    );
}
#[test]
fn formation_edge_slots_are_distinct_size_aware_and_bounded() {
    let mut s = arena(10, false);
    s.units[3].kind = RangeUnitKind::TurretT1;
    s.units[4].status.alive = false;
    s.units[5].body = voxel::unit::spider();
    for centre in [[0, 0], [1599, 1599], [800, 800], [i32::MIN, i32::MAX]] {
        let dest = s.battle_formation_destinations(&(0..10).collect::<Vec<_>>(), centre);
        assert_eq!(dest.len(), 8);
        for (i, &(id, p)) in dest.iter().enumerate() {
            assert!(p.iter().all(|&v| (40..1560).contains(&v)));
            assert_ne!(id, 3);
            assert_ne!(id, 4);
            for &(_, q) in &dest[..i] {
                let dx = i64::from(p[0] - q[0]);
                let dz = i64::from(p[1] - q[1]);
                assert!(dx * dx + dz * dz >= 240 * 240, "wide hull slots too close");
            }
        }
    }
    assert!(s.battle_formation_destinations(&[], [800, 800]).is_empty());
    assert!(s
        .battle_formation_destinations(&[usize::MAX], [800, 800])
        .is_empty());
    let mut tiny = arena(64, false);
    tiny.size = [128, 128, 128];
    assert!(tiny
        .battle_formation_destinations(&(0..64).collect::<Vec<_>>(), [64, 64])
        .is_empty());
}
#[test]
fn gate_convoy_remains_collision_free_and_all_orders_finish() {
    let mut s = arena(10, true);
    let dest: Vec<_> = s
        .units
        .iter()
        .enumerate()
        .map(|(id, u)| {
            (
                id,
                [
                    u.motion.pos[0].to_int_trunc() + 700,
                    u.motion.pos[2].to_int_trunc(),
                ],
            )
        })
        .collect();
    assert_eq!(s.battle_group_move(&dest, [1100, 700]), 10);
    for _ in 0..3600 {
        s.step();
        assert_separate(&s);
    }
    assert!(s.units.iter().all(|u| u.motion.destination.is_none()));
    assert!(s.matter.economy.ledger.balanced());
}
