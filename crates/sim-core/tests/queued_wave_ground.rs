//! Overlapping waves must not pay twice for an edit waiting to drain.
use sim_core::blast::matter::{ClaimResult, Matter};
use voxel::World;

struct Rock;
impl voxel::Generator for Rock {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 {
        if y < 16 {
            1
        } else {
            0
        }
    }
}

#[test]
fn a_second_wave_cannot_pay_for_pending_ground_with_or_without_a_piece() {
    for make_piece in [false, true] {
        let mut m = Matter::new(&[]);
        let mut w = World::new(32, 32, 32, 64, Box::new(Rock));
        m.begin(100_000, 4);
        if !make_piece {
            m.tuning.max_flying = 0;
        }
        let p = [12, 12, 12];
        assert!(matches!(
            m.ground_or_pulverize(&mut w, p, 1, p),
            ClaimResult::Applied
        ));
        assert_eq!(m.debris.claims(), usize::from(make_piece));
        // The second wave has no room even when the first made a piece.
        m.tuning.max_flying = 0;
        let before = (m.remaining_mj, m.spent_mj, w.queued());
        assert!(
            matches!(
                m.ground_or_pulverize(&mut w, p, 1, [20, 12, 20]),
                ClaimResult::Deferred
            ),
            "pending ground was admitted twice, make_piece={make_piece}"
        );
        assert_eq!((m.remaining_mj, m.spent_mj, w.queued()), before);
    }
}
