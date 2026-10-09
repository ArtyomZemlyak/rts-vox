//! Real competing terrain edits and wakeups, including a one-slot physical pool.
use super::*;
use voxel::{world::Edit, Generator};

struct Ground;
impl Generator for Ground {
    fn material(&self, x: u32, y: u32, _: u32) -> u16 {
        if y < 8 {
            if x < 16 {
                1
            } else {
                5
            }
        } else {
            0
        }
    }
}
fn fixture(cap: u32) -> (Matter, World, Felling) {
    let mut m = Matter::new(&[]);
    m.tuning.max_flying = cap;
    m.tuning.max_claims = 64;
    m.tuning.max_fragments = 64;
    m.tuning.rest_check_ticks = 1;
    m.fragment_size(1);
    m.begin(1_000_000, 1);
    let w = World::new(64, 96, 64, 4096, Box::new(Ground));
    let f = modding::navigation::Navigation::from_ron(include_str!(
        "../../../data/range-navigation.ron"
    ))
    .unwrap()
    .fell;
    (m, w, f)
}
fn confirm(m: &mut Matter, w: &mut World) {
    w.flush();
    m.debris.confirm(w.completed(), &mut m.economy, &m.tuning);
    assert!(m.economy.ledger.balanced());
    assert!(m.debris.fragment_pools(&m.tuning).flying <= m.tuning.max_flying as usize);
    assert_eq!(m.debris.claims(), 0);
}

#[test]
fn felling_cannot_take_every_slot_from_a_waiting_crater() {
    for waiting in [false, true] {
        let (mut m, mut w, f) = fixture(2);
        m.reserve_damage(waiting, 0);
        m.reserve_heat(0, 0);
        let cuts: Vec<_> = (20..40).map(|x| [x, 3, 20]).collect();
        m.felling_phase(&mut w, [(&cuts[..], [20, 3, 20])], &f);
        let result = m.ground(&mut w, [4, 3, 4], [4, 3, 4]);
        assert_eq!(
            matches!(result, ClaimResult::Applied),
            waiting,
            "legacy felling must reproduce the refusal; fair admission must pay the crater"
        );
        confirm(&mut m, &mut w);
        assert_eq!(w.material(4, 3, 4) == 0, waiting);
        assert_eq!(m.debris.removed, 2);
    }
}

#[test]
fn one_free_slot_rotates_between_felling_and_crater_without_losing_matter() {
    let (mut m, mut w, f) = fixture(1);
    let mut soil = 0;
    let mut wood = 0;
    for tick in 0..12 {
        for piece in &mut m.debris.fragments {
            piece.resting = true;
        }
        m.reserve_damage(true, tick);
        m.reserve_heat(0, 0);
        let cuts = [[20 + tick as u32, 3, 20]];
        m.felling_phase(&mut w, [(&cuts[..], cuts[0].map(|v| v as i32))], &f);
        let at = [4, 3, 4 + tick as i32];
        let result = m.ground(&mut w, at, at);
        assert_eq!(matches!(result, ClaimResult::Applied), tick % 2 == 1);
        confirm(&mut m, &mut w);
        for &(edit, before) in w.completed() {
            if edit.mat == 0 && before != 0 {
                if before == 1 {
                    soil += 1;
                } else {
                    wood += 1;
                }
            }
        }
    }
    assert_eq!((soil, wood), (6, 6));
    assert_eq!(m.debris.removed, 12);
    assert!(m.spent_mj <= m.supplied_mj);
}

#[test]
fn a_heat_forecast_cannot_reserve_the_whole_crater_pool() {
    let (mut m, mut w, f) = fixture(4);
    m.reserve_damage(true, 0);
    m.reserve_heat(20, 20);
    let cuts = [[20, 3, 20]];
    m.felling_phase(&mut w, [(&cuts[..], [20, 3, 20])], &f);
    assert_eq!(
        m.debris.claims(),
        0,
        "heat retains the routine queue's share"
    );
    for z in [4, 5] {
        assert!(matches!(
            m.ground(&mut w, [4, 3, z], [4, 3, z]),
            ClaimResult::Applied
        ));
    }
    assert!(matches!(
        m.ground(&mut w, [4, 3, 6], [4, 3, 6]),
        ClaimResult::Deferred
    ));
    confirm(&mut m, &mut w);
    assert_eq!(m.debris.removed, 2);
}

#[test]
fn waking_piles_leave_room_for_pending_crater_work() {
    for waiting in [false, true] {
        let (mut m, mut w, _) = fixture(2);
        for x in [20, 24, 28] {
            assert!(m.melt(&mut w, [x, 3, 20]));
            confirm(&mut m, &mut w);
            for piece in &mut m.debris.fragments {
                piece.resting = true;
                piece.position.y = Fx::from_int(80);
                piece.velocity = V3::ZERO;
            }
        }
        m.reserve_damage(waiting, 1);
        m.reserve_heat(0, 0);
        m.flush(&w, true, 98, 1, &[], &[]);
        assert_eq!(m.debris.wakeups, if waiting { 1 } else { 2 });
        assert!(m.debris.fragment_pools(&m.tuning).flying <= 2);
        let result = m.ground(&mut w, [4, 3, 4], [4, 3, 4]);
        assert_eq!(matches!(result, ClaimResult::Applied), waiting);
        confirm(&mut m, &mut w);
        assert_eq!(w.material(4, 3, 4) == 0, waiting);
    }
}

#[test]
fn failed_conditional_crater_edits_return_the_reserved_capacity() {
    let (mut m, mut w, _) = fixture(1);
    m.reserve_damage(true, 1);
    m.reserve_heat(0, 0);
    w.queue_edit(Edit::put(4, 3, 4, 3));
    assert!(matches!(
        m.ground(&mut w, [4, 3, 4], [4, 3, 4]),
        ClaimResult::Applied
    ));
    confirm(&mut m, &mut w);
    assert_eq!(w.material(4, 3, 4), 3);
    assert_eq!(m.debris.removed, 0);
    assert!(m.debris.fragments.is_empty());
    assert_eq!(m.claim_room(), 1);
}

#[test]
fn one_free_slot_does_not_alias_the_periodic_wake_deadline() {
    let (mut m, mut w, _) = fixture(1);
    m.tuning.rest_check_ticks = 8;
    m.debris.next_id = 1;
    assert!(m.melt(&mut w, [20, 3, 20]));
    confirm(&mut m, &mut w);
    let f = &mut m.debris.fragments[0];
    assert_eq!(f.id, 2);
    f.resting = true;
    f.position.y = Fx::from_int(80);
    f.velocity = V3::ZERO;
    for tick in 0..16 {
        m.reserve_damage(true, tick);
        m.reserve_heat(0, 0);
        m.flush(&w, true, 98, tick, &[], &[]);
        assert!(m.debris.fragment_pools(&m.tuning).flying <= 1);
        assert!(m.economy.ledger.balanced());
        if m.debris.wakeups > 0 {
            break;
        }
    }
    assert_eq!(
        m.debris.wakeups, 1,
        "periodic wake permanently missed its admission turn"
    );
    assert!(m.debris.fragments[0].position.y < Fx::from_int(80));
}

#[test]
fn wake_reserves_the_single_slot_for_the_next_ticks_waiting_crater() {
    let (mut m, mut w, f) = fixture(1);
    assert!(m.melt(&mut w, [20, 3, 20]));
    confirm(&mut m, &mut w);
    w.flush();
    let piece = &mut m.debris.fragments[0];
    piece.resting = true;
    piece.position.y = Fx::from_int(80);
    piece.velocity = V3::ZERO;

    m.reserve_damage(true, 0);
    m.reserve_heat(0, 0);
    m.flush(&w, true, 98, 0, &[], &[]);
    assert_eq!(m.debris.wakeups, 0, "wake took the next damage turn");

    m.reserve_damage(true, 1);
    m.reserve_heat(0, 0);
    let cuts = [[24, 3, 20]];
    m.felling_phase(&mut w, [(&cuts[..], [24, 3, 20])], &f);
    assert!(matches!(
        m.ground(&mut w, [4, 3, 4], [4, 3, 4]),
        ClaimResult::Applied
    ));
    confirm(&mut m, &mut w);
    assert_eq!(w.material(4, 3, 4), 0);
    assert_eq!(w.material(24, 3, 20), 5);
}
