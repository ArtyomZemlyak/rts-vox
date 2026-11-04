//! Material blasts in the ordinary world. Immutable inputs, bounded work and
//! confirmed voxel removals. The wave is an impulse envelope; fracture and
//! fragment kinetic energy draw from a separate, finite joule budget.
use crate::{
    economy::{Economy, Wreck},
    matter, NO_TARGET,
};
use fx::{
    angle,
    blob::{Blob, Scan},
    Fx,
};
use modding::{tuning::Blast, Registry};
use phys::{
    blast::{Hit, Wave},
    V3,
};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Query-only hash map with the deterministic Fx hasher (`fx::lookup`).
type FxLookup<K, V> = fx::lookup::Lookup<K, V, std::hash::BuildHasherDefault<rustc_hash::FxHasher>>;
use voxel::{
    store::{Mat, AIR},
    world::{snapshot::Snapshot, Edit},
};

/// Two competing admission queues share slots equally. The odd slot rotates
/// with the authoritative tick, including a pool with only one free slot.
pub(crate) fn routine_fragment_share(room: u32, damage_waiting: bool, tick: u64) -> u32 {
    let queues = 1 + u32::from(damage_waiting);
    room / queues + u32::from(tick % u64::from(queues) < u64::from(room % queues))
}

/// One independent physics job (or Rayon leaf batch), not an actor ID.
/// Timers and collection belong to the observer. The no-op observer carries
/// no clocks, allocation, thread lookup or synchronization into ordinary play.
pub(crate) struct PhysicsTask<'a, F: Fn(usize, &'static str)> {
    observe: &'a F,
    end: &'static str,
}
impl<'a, F: Fn(usize, &'static str)> PhysicsTask<'a, F> {
    pub(crate) fn new(observe: &'a F, begin: &'static str, end: &'static str) -> Self {
        observe(0, begin);
        Self { observe, end }
    }
}
impl<F: Fn(usize, &'static str)> Drop for PhysicsTask<'_, F> {
    fn drop(&mut self) {
        (self.observe)(0, self.end);
    }
}

#[path = "body_fragments.rs"]
pub(crate) mod body_fragments;
#[path = "chunk.rs"]
pub mod chunk;
#[path = "hull_queries.rs"]
mod hull_queries;
pub use hull_queries::HullQueries;
#[path = "corpse.rs"]
mod corpse;
#[path = "fragment_join.rs"]
mod fragment_join;
#[path = "fragment_join_memo.rs"]
mod fragment_join_memo;
#[cfg(test)]
#[path = "burst_ground_claim_tests.rs"]
mod ground_claim_tests;
#[cfg(test)]
#[path = "hull_query_tests.rs"]
mod hull_query_tests;
#[path = "shape_geometry.rs"]
mod shape_geometry;
#[path = "unit_debris.rs"]
mod unit_debris;

struct Job {
    at: [i32; 3],
    origin: [i32; 3],
    n: usize,
    cell: i32,
    input: Snapshot,
    cursor: usize,
    resistance: Vec<u32>,
    wave: Option<Wave>,
    hits: VecDeque<Hit>,
    subcell: usize,
    energy_mj: u64,
    body_hit_done: bool,
    body_energy_mj: u64,
    body_field: crate::body_wave::Field,
}

/// Fracture work and the reserved kinetic share, in millijoules.
pub(crate) fn fracture_cost_mj(strength: u32, mm: u16, t: &Blast) -> (u64, u64) {
    let fracture = u64::from(strength)
        .saturating_mul(u64::from(t.fracture_kj_m3_per_strength))
        .saturating_mul(u64::from(mm).pow(3))
        / 1000;
    let motion = fracture.saturating_mul(u64::from(t.motion_pct)) / u64::from(100 - t.motion_pct);
    (fracture.max(1).saturating_add(motion), motion)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn fragment_landing_does_not_absorb_remote_shards_but_exact_contacts_keep_matter_and_world_cells(
    ) {
        for distance in [1, 12] {
            let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
            w.flush();
            let mut t = tuning().blast;
            t.pile_cell = 32;
            t.max_flying = 0;
            t.push_pct = 0;
            for (id, x) in [(1, 48), (2, 48 + distance)] {
                let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
                let amount = e.per_voxel(2);
                e.ledger.reveal(e.ground, amount);
                e.ledger.transfer(e.ground, held, amount);
                b.fragments.push(Fragment {
                    id,
                    position: centre([x, 12, 48]),
                    velocity: V3::ZERO,
                    material: 2,
                    paint: None,
                    mm: 100,
                    basis: IDENTITY,
                    mass_g: 1200,
                    count: 1,
                    shape: 1,
                    side: 1,
                    chunk: None,
                    held,
                    resting: true,
                });
                e.wrecks.push(Wreck {
                    held,
                    x: x as i32,
                    y: 12,
                    z: 48,
                });
            }
            b.step_fragments(&w, &mut e, &t, Fx::ZERO, 0, Surroundings::NONE);
            assert!(e.balanced());
            if distance == 12 {
                assert_eq!(
                    b.fragments.len(),
                    2,
                    "3.2 m broad-phase must not teleport distant debris"
                );
                assert_eq!(b.fragments[0].position, centre([48, 12, 48]));
                assert_eq!(b.fragments[1].position, centre([60, 12, 48]));
                assert!(b
                    .fragments
                    .iter()
                    .all(|f| f.shape == 1 && f.side == 1 && f.mm == 100 && f.basis == IDENTITY));
            } else {
                assert_eq!(b.fragments.len(), 1);
                let f = &b.fragments[0];
                assert_eq!((f.shape, f.side, f.count), (3, 2, 2));
                assert_eq!(
                    f.position,
                    V3::new(
                        Fx::from_int(49),
                        Fx::from_ratio(25, 2),
                        Fx::from_ratio(97, 2)
                    )
                );
                assert_eq!(e.ledger.held(f.held), 2 * e.per_voxel(2));
                assert_eq!((e.wrecks[0].x, e.wrecks[0].y, e.wrecks[0].z), (49, 12, 48));
            }
        }
    }

    #[test]
    fn resting_join_memo_matches_cold_recomputation_after_geometry_and_support_changes() {
        let (mut warm, mut cold, mut ew, mut ec, mut w) = (
            Bursts::default(),
            Bursts::default(),
            economy(),
            economy(),
            world(),
        );
        let mut t = tuning();
        t.blast.rest_check_ticks = 1;
        t.blast.push_pct = 0;
        w.flush();
        for (b, e) in [(&mut warm, &mut ew), (&mut cold, &mut ec)] {
            for (id, x) in [(1, 48), (2, 60)] {
                let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
                let amount = e.per_voxel(2);
                e.ledger.reveal(e.ground, amount);
                e.ledger.transfer(e.ground, held, amount);
                b.fragments.push(Fragment {
                    id,
                    position: centre([x, 12, 48]),
                    velocity: V3::ZERO,
                    material: 2,
                    paint: None,
                    mm: 100,
                    basis: IDENTITY,
                    mass_g: 1200,
                    count: 1,
                    shape: 1,
                    side: 1,
                    chunk: None,
                    held,
                    resting: true,
                });
                e.wrecks.push(Wreck {
                    held,
                    x: x as i32,
                    y: 12,
                    z: 48,
                });
            }
        }
        // Pin both pieces over real terrain, then change an exact input and
        // finally remove support. No warm memo may suppress a join or wakeup.
        w.queue_edit(Edit::put(48, 11, 48, 2));
        w.queue_edit(Edit::put(60, 11, 48, 2));
        w.flush();
        for tick in 0..16 {
            if tick == 4 {
                for b in [&mut warm, &mut cold] {
                    b.fragments[1].paint = Some([1, 2, 3]);
                }
            }
            if tick == 6 {
                for b in [&mut warm, &mut cold] {
                    b.fragments[1].paint = None;
                    b.fragments[1].position = centre([49, 12, 48]);
                }
                w.queue_edit(Edit::put(49, 11, 48, 2));
                w.flush();
            }
            if tick == 10 {
                for x in 48..=60 {
                    w.queue_edit(Edit::dig(x, 11, 48));
                }
                w.flush();
            }
            cold.join_memo = Default::default();
            warm.step_fragments(
                &w,
                &mut ew,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
            cold.step_fragments(
                &w,
                &mut ec,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
            let (mut a, mut b) = (Blob::new(), Blob::new());
            warm.save(&mut a);
            cold.save(&mut b);
            assert_eq!(a.v, b.v, "warm memo diverged at tick {tick}");
            assert_eq!(warm.hash(), cold.hash());
            assert!(ew.balanced() && ec.balanced());
            if tick == 6 {
                assert_eq!(warm.fragments.len(), 1, "new contact was missed");
            }
        }
        assert!(
            warm.join_memo.hits > 0,
            "the stationary negative pass was never reused"
        );
        assert!(
            warm.wakeups > 0,
            "support changes must still wake memoized sleepers"
        );
    }

    #[test]
    fn fast_rest_schedule_matches_division_and_previous_deadline_at_wraparound() {
        let mut t = tuning().blast;
        for period in [0, 1, 2, 3, 7, 8, 16, 127, 128, u32::MAX] {
            t.rest_check_ticks = period;
            for id in [0, 1, 2, 7, 8, 15, 127, u64::MAX - 1, u64::MAX] {
                for tick in [0, 1, 2, 6, 7, 8, 127, u64::MAX - 1, u64::MAX] {
                    let now = id
                        .wrapping_add(tick)
                        .is_multiple_of(u64::from(period.max(1)));
                    assert_eq!(due(id, tick, &t), now);
                    for waiting in [false, true] {
                        let previous = waiting
                            && tick > 0
                            && id
                                .wrapping_add(tick - 1)
                                .is_multiple_of(u64::from(period.max(1)));
                        assert_eq!(
                            support_due(id, tick, &t, waiting),
                            now || previous,
                            "period={period} id={id} tick={tick} waiting={waiting}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn damage_waiting_keeps_the_missed_deadline_support_in_the_index() {
        for workers in [1, 4, 8] {
            rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap()
                .install(|| {
                    let (mut b, mut e, mut w) = (Bursts::default(), economy(), world());
                    let mut t = tuning();
                    t.blast.max_flying = 4;
                    t.blast.push_pct = 0;
                    w.queue_edit(Edit::put(48, 15, 48, 2));
                    w.flush();
                    // The upper shard rests on a thin plate's lip, with no world below it.
                    // Its previous deadline is now retried; neither piece is due today.
                    for (id, x, y, mm, material, shape, side) in [
                        (3, 487, 1610, 20, 2, 0x000f000f000f000fu64, 4),
                        (2, 4905, 1625, 10, 3, 1, 1),
                    ] {
                        b.fragments.push(Fragment {
                            id,
                            position: V3::new(
                                Fx::from_ratio(x, if id == 3 { 10 } else { 100 }),
                                Fx::from_ratio(y, 100),
                                Fx::from_ratio(485, 10),
                            ),
                            velocity: V3::ZERO,
                            material,
                            paint: None,
                            mm,
                            basis: IDENTITY,
                            mass_g: 1,
                            count: shape.count_ones(),
                            shape,
                            side,
                            chunk: None,
                            held: e.ledger.create(matter::Kind::Debris, 0, u64::MAX),
                            resting: true,
                        });
                    }
                    b.next_id = 4;
                    let tick = 7;
                    assert!(due(2, tick - 1, &t.blast));
                    assert!(b.fragments.iter().all(|f| !due(f.id, tick, &t.blast)));
                    let old_index = Settled::of(&b.fragments, &[], tick, &t.blast);
                    assert!(!supported(&b.fragments[1], &w, &[], &old_index, 1));
                    let before = b.hash();
                    b.step_fragments_reserved(
                        &w,
                        &mut e,
                        &t.blast,
                        crate::gravity_per_tick(&t),
                        tick,
                        Surroundings::NONE,
                        0,
                        true,
                        |_| {},
                        &|_, _| {},
                    );
                    assert_eq!(
                        b.wakeups, 0,
                        "the supported shard woke on the retry deadline"
                    );
                    assert_eq!(b.hash(), before);
                    assert_eq!(b.settled_queries, 1);
                    // The fix must not hide a real loss of support at the next deadline.
                    b.fragments.remove(0);
                    let y = b.fragments[0].position.y;
                    b.step_fragments_reserved(
                        &w,
                        &mut e,
                        &t.blast,
                        crate::gravity_per_tick(&t),
                        14,
                        Surroundings::NONE,
                        0,
                        true,
                        |_| {},
                        &|_, _| {},
                    );
                    assert_eq!(b.wakeups, 1);
                    assert!(b.fragments[0].position.y < y);
                    assert!(e.balanced());
                    assert!(!support_due(1, 0, &t.blast, true));
                });
        }
    }

    #[test]
    fn sleeping_history_keeps_its_matter_and_does_not_block_new_claims() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let mut t = tuning().blast;
        t.max_fragments = 1;
        t.max_flying = 1;
        t.max_resting_fragments = 32;
        for i in 0..32 {
            let p = [8 + (i % 8) * 8, 16, 8 + (i / 8) * 8];
            assert!(b.spawn(p, 1, p.map(|n| n as i32), 2, 1, 0, &mut e, &t));
            let f = b.fragments.last_mut().unwrap();
            f.resting = true;
            heap(f, &w);
        }
        let old = b.hash();
        assert_eq!(
            b.fragment_pools(&t),
            FragmentPools {
                working: 0,
                parked: 32,
                flying: 0
            }
        );
        assert_eq!(b.fragment_room(&t), 1);
        // Legacy data without the opt-in resting budget retains its old limit.
        let legacy = Blast {
            max_resting_fragments: 0,
            ..t
        };
        assert_eq!(b.fragment_room(&legacy), 0);
        assert_eq!(
            old,
            b.hash(),
            "classification does not modify old matter/shape"
        );
        let p = [70, 10, 70];
        assert!(b.claim_voxel(&mut w, p, 1, p.map(|n| n as i32), 0, 1, &t));
        assert_eq!(b.fragment_room(&t), 0);
        w.flush();
        b.confirm(w.completed(), &mut e, &t);
        assert_eq!(b.fragments.len(), 33);
        assert_eq!(b.fragment_pools(&t).working, 1);
        assert!(e.balanced());
        // A full storage budget remains finite, even when the new piece rests.
        let f = b.fragments.last_mut().unwrap();
        f.resting = true;
        heap(f, &w);
        assert_eq!(b.fragment_room(&t), 0);
        let held = b.fragments[0].held;
        let hauler = e.ledger.create(matter::Kind::Hauler, 0, u64::MAX);
        let amount = e.ledger.held(held);
        assert_eq!(e.ledger.transfer(held, hauler, amount), amount);
        b.sweep(&e);
        assert_eq!(b.fragments.len(), 32);
        assert_eq!(b.fragment_room(&t), 1);
        assert!(e.balanced());
        // Neither a nonzero velocity nor an active flag can masquerade as
        // static storage. A forged moving history must be refused by load.
        let mut bytes = Blob::new();
        b.save(&mut bytes);
        let copy = Bursts::load(&mut Scan::new(&bytes.v), Some(&w), &t).unwrap();
        assert_eq!(b.hash(), copy.hash());
        for f in &mut b.fragments {
            f.resting = false;
        }
        let mut invalid = Blob::new();
        b.save(&mut invalid);
        assert!(Bursts::load(&mut Scan::new(&invalid.v), Some(&w), &t).is_none());
        b.fragments[0].resting = true;
        b.fragments[0].velocity = V3::UP;
        assert!(!b.fragments[0].parkable());
    }

    #[test]
    fn sleeping_wakeups_respect_pending_claims_and_working_slots() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let tuning = tuning();
        let mut t = tuning.blast;
        t.max_fragments = 1;
        t.max_flying = 2; // Flying room exists; the working reservation is decisive.
        t.max_resting_fragments = 3;
        t.rest_check_ticks = 1;
        t.pile_cell = 16; // Three distinct piles; this test isolates wakeup admission.
        for x in [16, 32, 48] {
            assert!(b.spawn([x, 16, 16], 1, [x as i32, 16, 16], 2, 1, 0, &mut e, &t));
            let f = b.fragments.last_mut().unwrap();
            f.resting = true;
            heap(f, &w);
            e.wrecks.push(Wreck {
                held: f.held,
                x: x as i32,
                y: 12,
                z: 16,
            });
            w.queue_dig(x, 11, 16);
        }
        assert!(b.claim_voxel(&mut w, [70, 10, 70], 1, [70, 10, 70], 0, 1, &t));
        w.flush();
        b.step_fragments(
            &w,
            &mut e,
            &t,
            crate::gravity_per_tick(&tuning),
            0,
            Surroundings::NONE,
        );
        assert!(
            b.fragments.iter().all(|f| f.resting),
            "a wakeup stole the claim's working slot"
        );
        assert_eq!(b.wakeups, 0);
        b.clear_claims();
        b.step_fragments(
            &w,
            &mut e,
            &t,
            crate::gravity_per_tick(&tuning),
            1,
            Surroundings::NONE,
        );
        assert!(
            !b.fragments[0].resting,
            "the first unsupported piece must resume falling"
        );
        assert!(b.fragments[1..].iter().all(|f| f.resting));
        assert_eq!(b.wakeups, 1);
        // Runtime diagnostics cannot affect replay state or serialized bytes.
        let expected = b.hash();
        let mut bytes = Blob::new();
        b.save(&mut bytes);
        b.wakeups += 100;
        b.settled_queries = usize::MAX;
        b.settled_query_words = [usize::MAX; 2];
        assert_eq!(b.hash(), expected);
        let mut again = Blob::new();
        b.save(&mut again);
        assert_eq!(bytes.v, again.v);
        let restored = Bursts::load(&mut Scan::new(&bytes.v), Some(&w), &t).unwrap();
        assert_eq!(restored.wakeups, 0);
        assert_eq!(restored.settled_queries, 0);
        assert_eq!(restored.settled_query_words, [0; 2]);
        assert_eq!(restored.hash(), expected);
        assert_eq!(b.fragment_pools(&t).working, 1);
        assert_eq!(e.wrecks.len(), 2);
        assert!(e.balanced());
    }

    #[test]
    fn legacy_two_cell_fragments_fit_their_encoded_byte_bound() {
        let (mut b, w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning().blast;
        for x in [16, 32, 48, 64] {
            assert!(b.spawn([x, 16, 16], 1, [x as i32, 16, 16], 2, 1, 0, &mut e, &t));
            let f = b.fragments.last_mut().unwrap();
            f.resting = true;
            assert_eq!(f.side, 2);
            assert_eq!(f.shape, 1);
            assert!(f.paint.is_none() && f.chunk.is_none());
        }
        let mut current = Blob::new();
        b.save(&mut current);
        // Bare Bursts, with no jobs, claims or corpse records: the old
        // representation stores one byte of shape and omits the box side.
        let mut legacy = current.v[..80].to_vec();
        assert_eq!(current.v.len(), 80 + 4 * 160);
        for record in current.v[80..].chunks_exact(160) {
            legacy.extend_from_slice(&record[..146]);
            legacy.extend_from_slice(&record[154..]);
        }
        assert_eq!(legacy.len(), 80 + 4 * 152);
        let copy =
            Bursts::load_material(&mut Scan::new(&legacy), Some(&w), &t, None, 0, true, false)
                .unwrap();
        assert_eq!(b.hash(), copy.hash());
        // Truncated data must still fail before allocating fragment records.
        legacy.truncate(legacy.len() - 1);
        assert!(
            Bursts::load_material(&mut Scan::new(&legacy), Some(&w), &t, None, 0, true, false)
                .is_none()
        );
        assert!(e.balanced());
    }

    #[test]
    fn ordinary_fragment_record_does_not_store_an_inline_section_payload() {
        assert!(std::mem::size_of::<Fragment>() <= 192);
        assert!(std::mem::size_of::<Option<chunk::Chunk>>() > std::mem::size_of::<Fragment>());
    }

    #[test]
    fn claim_batch_reserves_capacity_without_spending_it_on_rejected_cells() {
        for n in [0, 16, 25000] {
            for resting_every in [1, 3, usize::MAX] {
                for (flying_extra, total_extra, claim_limit) in [
                    (0, 10, 64),
                    (10, 0, 64),
                    (2, 10, 64),
                    (10, 2, 64),
                    (10, 10, 1),
                    (10, 10, 64),
                ] {
                    let make = || Bursts {
                        fragments: (0..n)
                            .map(|i| Fragment {
                                id: i as u64 + 1,
                                position: V3::ZERO,
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
                                held: 0,
                                resting: i % resting_every == 0,
                            })
                            .collect(),
                        ..Bursts::default()
                    };
                    let (mut actual, mut reference) = (make(), make());
                    let (mut w, mut old_world) = (world(), world());
                    let mut t = tuning().blast;
                    let moving = actual.fragments.iter().filter(|f| !f.resting).count();
                    t.max_flying = moving as u32 + flying_extra;
                    t.max_fragments = n as u32 + total_extra;
                    t.max_claims = claim_limit;
                    for batch in 0..2 {
                        // A subsequent batch must see fragments waking up and
                        // outstanding claims; capacity is never carried over.
                        if batch == 1 && n > 0 {
                            actual.fragments[0].resting = false;
                            reference.fragments[0].resting = false;
                        }
                        let mut room = actual.fragment_room(&t);
                        let cells = [[0, 11, 48], [96, 11, 48]]
                            .into_iter()
                            .chain((40..60).flat_map(|x| [[x, 11, 48]; 2]));
                        for p in cells {
                            let before = room;
                            let accepted = actual.claim_voxel_with_room(
                                &mut w,
                                p,
                                1,
                                [48, 13, 48],
                                999,
                                2,
                                &t,
                                &mut room,
                            );
                            assert_eq!(
                                accepted,
                                reference.claim_voxel(
                                    &mut old_world,
                                    p,
                                    1,
                                    [48, 13, 48],
                                    999,
                                    2,
                                    &t,
                                ),
                                "n={n}, resting_every={resting_every}, batch={batch}, p={p:?}"
                            );
                            // A cell takes at most one slot: one for a new
                            // piece, none for a cell joining one (ADR-659);
                            // a rejected cell takes nothing.
                            assert!(before as i64 - room as i64 <= i64::from(accepted));
                            if !accepted {
                                assert_eq!(before, room);
                            }
                            assert_eq!(room, actual.fragment_room(&t));
                        }
                        let snapshot = |b: &Bursts, w: &voxel::World| {
                            let mut saved = Blob::new();
                            b.save(&mut saved);
                            w.save(&mut saved);
                            saved.v
                        };
                        assert_eq!(snapshot(&actual, &w), snapshot(&reference, &old_world));
                    }
                }
            }
        }
    }

    #[test]
    fn distant_hull_rejection_preserves_rotated_contacts() {
        for head in (0..angle::FULL).step_by((angle::FULL / 16) as usize) {
            let hull = Hull {
                pos: [Fx::from_int(20), Fx::from_int(12), Fx::from_int(30)],
                head,
                half: [Fx::from_int(7), Fx::from_int(13)],
                height: Fx::from_int(8),
                vel: [Fx::ZERO; 3],
                step: Fx::ONE,
                solid: None,
            };
            let low = [Fx::from_int(-2); 3];
            let high = [Fx::from_int(2); 3];
            let reach = Fx::from_int(2);
            let (c, s) = (angle::cos(head), angle::sin(head));
            for x in (-20..=60).step_by(4) {
                for z in (-10..=70).step_by(4) {
                    let p = [Fx::from_int(x), Fx::from_int(16), Fx::from_int(z)];
                    let dx = p[0].sub(hull.pos[0]);
                    let dz = p[2].sub(hull.pos[2]);
                    let u = dx.mul(c).add(dz.mul(s));
                    let v = dz.mul(c).sub(dx.mul(s));
                    let expected =
                        u.abs() <= hull.half[0].add(reach) && v.abs() <= hull.half[1].add(reach);
                    assert_eq!(
                        hull_box_hit(p, low, high, &[hull], 0, true).is_some(),
                        expected,
                        "head={head} x={x} z={z}"
                    );
                    let to = [p[0].add(Fx::from_int(3)), p[1], p[2].sub(Fx::ONE)];
                    let extent = [hull.half[0].add(reach), hull.half[1].add(reach)];
                    let rotated = [
                        extent[0]
                            .mul(c.abs())
                            .add(extent[1].mul(s.abs()))
                            .add(Fx::ONE),
                        extent[0]
                            .mul(s.abs())
                            .add(extent[1].mul(c.abs()))
                            .add(Fx::ONE),
                    ];
                    let old_near = [(0, 0), (2, 1)].iter().all(|&(axis, i)| {
                        p[axis].min(to[axis]) <= hull.pos[axis].add(rotated[i])
                            && p[axis].max(to[axis]) >= hull.pos[axis].sub(rotated[i])
                    });
                    assert_eq!(hull_near(p, to, low, high, &[hull], 0).0, old_near);
                }
            }
        }
    }
    // ADR-479 implementation retained as the differential oracle.
    #[test]
    fn rubble_column_radix_paths_match_stable_sort_with_duplicate_and_extreme_keys() {
        for count in [0, 1, 31, 4095, 4096, 4097, 8192, 131_074] {
            for wide in [false, true] {
                let limit = if wide { 65_536 } else { 64 };
                let mut actual: Vec<_> = (0..count)
                    .map(|i| {
                        let at = if i % 7 == 0 {
                            [limit - 1, limit - 1]
                        } else {
                            [(i * 17 + 3) % limit, (i * 31 + 1) % limit]
                        };
                        (at.map(|v| v), (i % 65_536) as u16)
                    })
                    .collect();
                let mut expected = actual.clone();
                // Independent stable comparison sort retains every duplicate's
                // original top/order, not just the final map's maximum height.
                expected.sort_by_key(|c| c.0);
                sort_columns(&mut actual);
                assert_eq!(actual, expected, "count={count} wide={wide}");
                let mut maxima = BTreeMap::new();
                for &(at, top) in &expected {
                    let kept = maxima.entry(at).or_insert(0u16);
                    *kept = (*kept).max(top);
                }
                let maxima: Vec<_> = maxima.into_iter().collect();
                merge_columns(&mut actual);
                assert_eq!(actual, maxima, "merged count={count} wide={wide}");
            }
        }
    }
    #[test]
    fn rubble_row_maxima_match_tree_for_unordered_overlaps_and_zero_tops() {
        for count in [4095, 4096, 4097, 8192, 131_074] {
            for layout in 0..4 {
                let mut rng = fx::rng::Rng::new(count, 696);
                let mut columns: Vec<_> = (0..count)
                    .map(|i| {
                        let at = match layout {
                            0 => [rng.below(64) as i32 * 3, rng.below(129) as i32],
                            1 => [rng.below(3) as i32 * 31, rng.below(65_536) as i32],
                            2 => [rng.below(65_536) as i32, rng.below(3) as i32 * 63],
                            _ => [65_535, 65_535],
                        };
                        // Some whole columns are present at height zero. The
                        // same Z has independent heights in adjacent X rows.
                        let top = if (at[0] + at[1]) % 7 == 0 {
                            0
                        } else {
                            (i % 65_536) as u16
                        };
                        (at, top)
                    })
                    .collect();
                let mut expected = BTreeMap::new();
                for &(at, top) in &columns {
                    let kept = expected.entry(at).or_insert(0u16);
                    *kept = (*kept).max(top);
                }
                let expected: Vec<_> = expected.into_iter().collect();
                merge_columns(&mut columns);
                assert_eq!(columns, expected, "count={count} layout={layout}");
                columns.reverse();
                merge_columns(&mut columns);
                assert_eq!(columns, expected, "idempotent maxima");
            }
        }
    }
    #[test]
    fn rubble_column_scratch_matches_tree_after_growth_shrink_and_worker_changes() {
        let mut scratch = ColumnMergeScratch::default();
        for workers in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            for (pass, (count, nx, nz)) in [
                (40_000, 2048, 1024),
                (6000, 64, 128),
                (0, 1, 1),
                (4095, 8192, 65536),
                (80_000, 4096, 4096),
                (4096, 4, 32),
            ]
            .into_iter()
            .enumerate()
            {
                let mut columns: Vec<_> = (0..count)
                    .map(|i| {
                        (
                            [(i * 37 + pass * 17) % nx, (i * 19 + pass * 31) % nz]
                                .map(|v| v as i32),
                            if i % 7 == 0 { 0 } else { (i % 65536) as u16 },
                        )
                    })
                    .collect();
                let mut tree = BTreeMap::new();
                for &(at, top) in &columns {
                    let value = tree.entry(at).or_insert(0u16);
                    *value = (*value).max(top);
                }
                let expected: Vec<_> = tree.into_iter().collect();
                pool.install(|| merge_columns_observed(&mut columns, &|_, _| {}, &mut scratch));
                assert_eq!(columns, expected, "workers={workers} pass={pass}");
                for stripe in &scratch.groups {
                    assert!(stripe.out.is_empty());
                    assert!(stripe.tops.iter().all(|&v| v == 0));
                    assert!(stripe.present.iter().all(|&v| v == 0));
                    assert!(stripe.blocks.iter().all(|&v| v == 0));
                }
            }
        }
    }
    #[test]
    fn rubble_row_batches_match_tree_across_dispatch_edges_and_workers() {
        for workers in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            for count in [16_383usize, 16_384, 16_385, 65_536] {
                for width in [511usize, 512, 513, 1025] {
                    let mut rng = fx::rng::Rng::new(count as u64, width as u64);
                    let mut columns: Vec<_> = (0..count)
                        .map(|i| {
                            let x = if i % 7 == 0 {
                                0
                            } else {
                                rng.below(width as u32)
                            };
                            let z = rng.below(4096);
                            let top = if (x + z) % 7 == 0 { 0 } else { i as u16 };
                            ([x as i32, z as i32], top)
                        })
                        .collect();
                    columns[0] = ([(width - 1) as i32, 4095], u16::MAX);
                    let mut expected = BTreeMap::new();
                    for &(at, top) in &columns {
                        let kept = expected.entry(at).or_insert(0u16);
                        *kept = (*kept).max(top);
                    }
                    let expected: Vec<_> = expected.into_iter().collect();
                    let trace = crate::task_trace::Trace::default();
                    pool.install(|| {
                        merge_columns_observed(
                            &mut columns,
                            &|stage, event| trace.record(stage, event),
                            &mut ColumnMergeScratch::default(),
                        )
                    });
                    assert_eq!(
                        columns, expected,
                        "workers={workers} count={count} width={width}"
                    );
                    trace.balanced(&["physics_rubble_merge_rows"]);
                    let jobs = if workers > 1 && count >= 16_384 && width >= 512 {
                        width.div_ceil(64)
                    } else {
                        1
                    };
                    assert_eq!(trace.jobs("physics_rubble_merge_rows_begin"), jobs);
                }
            }
        }
    }
    impl Bursts {
        fn reference_rubble(
            &self,
            world: &voxel::World,
            yielding: &[Mat],
            hulls: &[Hull],
        ) -> nav::vehicle::Rubble {
            // Floor is what something holds: the world, or a resting piece
            // under it — a pile on a rock piece is floor at the pile's top.
            // Only the world was asked, and a stacked piece was missing from
            // the map (ADR-458). A piece on a roof, or one whose support was
            // taken and whose turn to fall has not come, is not floor; one a
            // hull carries is not asked about the pieces (ADR-469).
            let mut floor = Vec::new();
            let mut stacked = Vec::new();
            for (i, f) in self.fragments.iter().enumerate() {
                if !f.resting || f.chunk.is_some() || yielding.contains(&f.material) {
                    continue;
                }
                if supported(f, world, &[], &Settled::default(), u32::MAX) {
                    floor.push(i);
                } else if !hulls
                    .iter()
                    .any(|h| h.carries(f.position.y.add(f.bounds().0[1]), f.position))
                {
                    stacked.push(i);
                }
            }
            if !stacked.is_empty() {
                let settled = Settled::around(&self.fragments, hulls, |i, _| {
                    stacked.binary_search(&i).is_ok()
                });
                floor.extend(
                    stacked
                        .into_iter()
                        .filter(|&i| supported(&self.fragments[i], world, &[], &settled, i as u32)),
                );
            }
            // A hull covers no column outside the box around its footprint:
            // a piece far from every hull asks none of them (ADR-465).
            let reach: Vec<([Fx; 2], [Fx; 2])> = hulls.iter().map(Hull::footprint_box).collect();
            let mut against: Vec<&Hull> = Vec::with_capacity(hulls.len());
            // The columns with their tops, gathered and sorted, and the map
            // built from them at once: thousands of single insertions into
            // the tree cost a millisecond a step on a battlefield (ADR-465).
            let mut columns: Vec<([i32; 2], u16)> = Vec::new();
            for f in floor.into_iter().map(|i| &self.fragments[i]) {
                let (low, high) = f.bounds();
                let p = arr(f.position);
                // Floor of the bottom corners, ceiling of the top: the columns
                // the box covers and the height it reaches.
                let lo = [p[0].add(low[0]), p[2].add(low[2])].map(|v| (v.raw() >> 32) as i32);
                let hi = [p[0].add(high[0]), p[2].add(high[2])].map(|v| (v.raw() >> 32) as i32);
                let top = -(p[1].add(high[1]).neg().raw() >> 32);
                let top = top.clamp(0, i64::from(u16::MAX)) as u16;
                against.clear();
                against.extend(hulls.iter().zip(&reach).filter_map(|(h, (b0, b1))| {
                    let near = (0..2).all(|a| {
                        Fx::from_int(lo[a]) <= b1[a] && Fx::from_int(hi[a]).add(Fx::ONE) >= b0[a]
                    });
                    (near && !h.covers_point([p[0], p[2]])).then_some(h)
                }));
                for z in lo[1].max(0)..=hi[1] {
                    for x in lo[0].max(0)..=hi[0] {
                        if against.iter().any(|h| h.covers(x, z)) {
                            continue;
                        }
                        columns.push(([x, z], top));
                    }
                }
            }
            // A column under several pieces holds the highest top.
            sort_columns(&mut columns);
            columns.dedup_by(|later, kept| {
                let same = later.0 == kept.0;
                if same {
                    kept.1 = kept.1.max(later.1);
                }
                same
            });
            nav::vehicle::Rubble::from_sorted(columns)
        }
    }
    #[test]
    fn narrow_support_index_matches_motion_index_contacts() {
        for workers in [1, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            for count in [31, 1023, 1024, 1025] {
                let fragments: Vec<_> = (0..count)
                    .map(|i| {
                        let side = (i % 4 + 1) as u8;
                        let shape = u64::MAX >> (64 - u32::from(side).pow(3));
                        let (c, s) = (angle::cos(i * 2731), angle::sin(i * 2731));
                        let mut f = Fragment {
                            id: i as u64 + 1,
                            position: V3::new(
                                Fx::from_int(i % 19 * 4 - 40).add(Fx::from_ratio(i % 4, 4)),
                                Fx::from_int(i / 19 % 7 * 3 - 9),
                                Fx::from_int(i / 133 * 5 - 30),
                            ),
                            velocity: V3::new(Fx::from_int(10), Fx::from_int(-2), Fx::ONE),
                            material: 1,
                            paint: None,
                            mm: 100,
                            basis: [
                                V3::new(c, s, Fx::ZERO),
                                V3::new(s.neg(), c, Fx::ZERO),
                                V3::new(Fx::ZERO, Fx::ZERO, Fx::ONE),
                            ],
                            mass_g: 1200,
                            count: shape.count_ones(),
                            shape,
                            side,
                            chunk: None,
                            held: i as u32 + 1,
                            resting: i % 6 != 0,
                        };
                        if i < 2 {
                            f.position = V3::new(
                                Fx::from_int(-8),
                                if i == 0 {
                                    Fx::from_int(2)
                                } else {
                                    Fx::from_ratio(17, 4)
                                },
                                Fx::from_int(-8),
                            );
                            f.basis = IDENTITY;
                            f.side = 2;
                            f.shape = 255;
                            f.count = 8;
                            f.resting = true;
                        }
                        f
                    })
                    .collect();
                let hull = Hull {
                    pos: [Fx::from_int(8), Fx::from_int(4), Fx::from_int(8)],
                    head: 2731,
                    half: [Fx::from_int(4), Fx::from_int(3)],
                    height: Fx::from_int(8),
                    vel: [Fx::ZERO; 3],
                    step: Fx::ONE,
                    solid: None,
                };
                for hulls in [&[][..], std::slice::from_ref(&hull)] {
                    let build = |reach| {
                        pool.install(|| {
                            Settled::around_bounds(
                                &fragments,
                                hulls,
                                |i, _| i == 1 || i % 7 == 0,
                                |_, f| f.bounds(),
                                reach,
                                &|_, _| {},
                            )
                        })
                    };
                    let wide = build(SettledReach::Motion);
                    let narrow = build(SettledReach::Support);
                    assert_eq!(narrow.boxes, wide.boxes);
                    assert!(narrow.cells.len() < wide.cells.len());
                    let (mut hits, mut misses) = (0, 0);
                    for (i, f) in fragments
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i == 1 || i % 7 == 0)
                    {
                        let (low, high) = f.bounds();
                        let mut p = arr(f.position);
                        p[1] = p[1].sub(Fx::from_ratio(1, 2));
                        let lo = [0, 1, 2].map(|a| p[a].add(low[a]));
                        let hi = [0, 1, 2].map(|a| p[a].add(high[a]));
                        for skip in [i as u32, u32::MAX] {
                            for above in [None, Some(f.position.y.add(low[1]))] {
                                let expected = wide.hit(lo, hi, skip, above);
                                assert_eq!(
                                    narrow.hit(lo, hi, skip, above),
                                    expected,
                                    "workers={workers} count={count} fragment={i} skip={skip}"
                                );
                                if expected {
                                    hits += 1;
                                } else {
                                    misses += 1;
                                }
                            }
                        }
                    }
                    assert!(hits > 0 && misses > 0);
                    assert!(narrow.hit(
                        [Fx::from_int(-9), Fx::from_ratio(11, 4), Fx::from_int(-9)],
                        [Fx::from_int(-7), Fx::from_ratio(19, 4), Fx::from_int(-7)],
                        1,
                        Some(Fx::from_ratio(13, 4)),
                    ));
                }
            }
        }
    }
    #[test]
    fn rubble_support_scopes_preserve_contacts_and_cover_cleanup() {
        let w = world();
        for workers in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            for count in [
                63usize, 64, 65, 127, 128, 129, 1023, 1024, 1025, 2047, 2048, 2049,
            ] {
                let mut b = Bursts::default();
                b.fragments.push(Fragment {
                    id: 1,
                    position: V3::new(Fx::from_int(50), Fx::from_int(14), Fx::from_int(50)),
                    velocity: V3::ZERO,
                    material: 2,
                    paint: None,
                    mm: 100,
                    basis: IDENTITY,
                    mass_g: 76800,
                    count: 64,
                    shape: u64::MAX,
                    side: 4,
                    chunk: None,
                    held: 0,
                    resting: true,
                });
                for i in 0..count {
                    b.fragments.push(Fragment {
                        id: i as u64 + 2,
                        position: centre(if i + 1 == count {
                            [60, 20, 60]
                        } else {
                            [49, 16, 49]
                        }),
                        mass_g: 1200,
                        count: 1,
                        shape: 1,
                        side: 1,
                        material: 1,
                        held: i as u32 + 1,
                        velocity: V3::ZERO,
                        paint: None,
                        mm: 100,
                        basis: IDENTITY,
                        chunk: None,
                        resting: true,
                    });
                }
                b.next_id = count as u64 + 2;
                let reference = b.reference_rubble(&w, &[], &[]);
                assert_eq!(reference.get(&[49, 49]), Some(&17));
                assert_eq!(reference.get(&[60, 60]), None);
                let trace = crate::task_trace::Trace::default();
                let before = b.hash();
                assert_ne!(before, 0);
                let actual = pool.install(|| {
                    b.rubble_observed(&w, &[], &[], &|stage, event| trace.record(stage, event))
                });
                assert_eq!(actual, reference, "workers={workers} count={count}");
                assert_eq!(b.hash(), before);
                trace.balanced(&[
                    "physics_rubble_prepare",
                    "physics_rubble_classify",
                    "physics_rubble_floor",
                    "physics_rubble_support_build",
                    "physics_rubble_support_query",
                    "physics_rubble_support_read",
                    "physics_rubble_support_drop",
                    "physics_rubble_gather_dispatch",
                    "physics_rubble_gather",
                    "physics_rubble_merge",
                    "physics_rubble_publish",
                ]);
                assert_eq!(trace.jobs("physics_rubble_support_query_begin"), 1);
                assert_eq!(
                    trace.jobs("physics_rubble_support_read_begin"),
                    if count >= 1024 && workers > 1 {
                        count.div_ceil(256)
                    } else {
                        1
                    },
                    "support batches workers={workers} count={count}"
                );
                let mut scratch = RubbleScratch::default();
                // Change membership within words and across word/batch edges
                // without advancing a tick. Old bits must not survive reuse.
                for pass in 0..3 {
                    for (i, f) in b.fragments.iter_mut().enumerate().skip(1) {
                        f.resting = i % 3 != pass;
                    }
                    let expected = b.reference_rubble(&w, &[], &[]);
                    assert_eq!(
                        pool.install(|| b.rubble_with_scratch(
                            &w,
                            &[],
                            &[],
                            &mut scratch,
                            &|_, _| {}
                        )),
                        expected,
                        "mask workers={workers} count={count} pass={pass}"
                    );
                    assert!(scratch.stacked_mask.is_empty());
                }
            }
        }
    }
    #[test]
    fn rubble_batches_match_reference_at_thresholds_and_trace_worker_calls() {
        let w = world();
        let pools: Vec<_> = [1, 2, 4, 8]
            .into_iter()
            .map(|n| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(n)
                    .build()
                    .unwrap()
            })
            .collect();
        for count in [0, 1023, 1024, 1537] {
            let mut b = Bursts::default();
            for i in 0..count {
                b.fragments.push(Fragment {
                    id: i + 1,
                    position: V3::new(
                        Fx::from_int(8 + (i % 40) as i32 * 2),
                        Fx::from_ratio(25, 2),
                        Fx::from_int(8 + (i / 40) as i32 * 2),
                    ),
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
                    held: i as u32,
                    resting: true,
                });
            }
            let expected = b.reference_rubble(&w, &[], &[]);
            assert_eq!(expected.is_empty(), count == 0);
            for pool in &pools {
                let trace = crate::task_trace::Trace::default();
                let actual = pool.install(|| {
                    b.rubble_observed(&w, &[], &[], &|stage, event| trace.record(stage, event))
                });
                assert_eq!(
                    actual,
                    expected,
                    "count={count} workers={}",
                    pool.current_num_threads()
                );
                trace.balanced(&[
                    "physics_rubble_floor",
                    "physics_rubble_gather",
                    "physics_rubble_columns",
                    "physics_rubble_publish",
                ]);
            }
        }
    }
    #[test]
    fn rubble_reused_boxes_match_reference_after_support_and_hull_changes() {
        let (mut b, mut w) = (Bursts::default(), world());
        let mut scratch = RubbleScratch::default();
        for i in 0..1537u32 {
            let side = (i % 4 + 1) as u8;
            let full = u64::MAX >> (64 - u32::from(side).pow(3));
            let shape = if i % 3 == 0 {
                full & 0x9d37_a518_6fc2_0be1
            } else {
                full
            };
            let turn = (i % 7) as i32 * 2731;
            let (c, s) = (angle::cos(turn), angle::sin(turn));
            b.fragments.push(Fragment {
                id: u64::from(i) + 1,
                position: V3::new(
                    Fx::from_int(32 + (i % 8) as i32 * 4),
                    Fx::from_int(12 + (i / 64) as i32 * 4).add(Fx::from_ratio(i32::from(side), 2)),
                    Fx::from_int(32 + (i / 8 % 8) as i32 * 4),
                ),
                velocity: V3::ZERO,
                material: (i % 3 + 1) as Mat,
                paint: None,
                mm: 100,
                basis: [
                    V3::new(c, Fx::ZERO, s),
                    V3::UP,
                    V3::new(s.neg(), Fx::ZERO, c),
                ],
                mass_g: 1200,
                count: shape.count_ones(),
                shape,
                side,
                chunk: None,
                held: 0,
                resting: i % 5 != 0,
            });
        }
        let pools: Vec<_> = [1, 2, 4, 8]
            .into_iter()
            .map(|n| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(n)
                    .build()
                    .unwrap()
            })
            .collect();
        let solid = |[x, y, z]: [i32; 3]| {
            (42..54).contains(&x) && (42..54).contains(&z) && (14..16).contains(&y)
        };
        for pass in 0..8 {
            let hull = Hull {
                pos: [
                    Fx::from_int(48 + pass * 3),
                    Fx::from_int(12),
                    Fx::from_int(48),
                ],
                head: pass * 2731,
                half: [Fx::from_int(6), Fx::from_int(6)],
                height: Fx::from_int(10),
                vel: [Fx::ZERO; 3],
                step: Fx::from_int(4),
                solid: (pass % 2 == 0).then_some(Solid(&solid)),
            };
            for hulls in [&[][..], std::slice::from_ref(&hull)] {
                for yielding in [&[][..], &[1][..], &[1, 2, 3][..]] {
                    let expected = b.reference_rubble(&w, yielding, hulls);
                    for pool in &pools {
                        assert_eq!(
                            pool.install(|| b.rubble(&w, yielding, hulls)),
                            expected,
                            "pass={pass} workers={}",
                            pool.current_num_threads()
                        );
                        assert_eq!(
                            pool.install(|| b.rubble_with_scratch(
                                &w,
                                yielding,
                                hulls,
                                &mut scratch,
                                &|_, _| {}
                            )),
                            expected,
                            "warm pass={pass} workers={}",
                            pool.current_num_threads()
                        );
                        assert!(scratch.bounds.is_empty());
                        assert!(scratch.settled.cells.is_empty());
                        assert!(scratch.settled.boxes.is_empty());
                        assert!(scratch.queries.is_empty());
                        assert!(scratch.stacked_mask.is_empty());
                    }
                }
            }
            // Queries after digging and fragment movement cannot reuse an
            // earlier result, even when no simulation tick has elapsed.
            for x in 32 + pass * 8..40 + pass * 8 {
                for z in 32..64 {
                    for y in 8..12 {
                        w.queue_edit(Edit::dig(x as u32, y, z));
                    }
                }
            }
            w.flush();
            for (i, f) in b.fragments.iter_mut().enumerate() {
                if i % 7 == 0 {
                    f.resting = !f.resting;
                    f.position.y = f.position.y.add(Fx::ONE);
                }
                if i % 11 == 0 {
                    f.mm = if f.mm == 100 { 160 } else { 100 };
                }
                if i % 13 == 0 {
                    f.shape ^= 1;
                    f.count = f.shape.count_ones();
                }
                if i % 17 == 0 {
                    f.side = (f.side % 4) + 1;
                }
                if i % 19 == 0 {
                    f.basis.rotate_left(1);
                }
            }
            // A sidecar slot follows the dense slice, not an old ID. Changed
            // geometry must be rechecked after compaction or imported reorder.
            b.fragments.rotate_left(17);
            if pass == 4 {
                b.fragments.truncate(1000);
            }
        }
    }
    #[test]
    fn rubble_scratch_keeps_capacity_without_retaining_old_contacts() {
        let (mut b, mut w) = (Bursts::default(), world());
        let mut scratch = RubbleScratch::default();
        for i in 0..2 {
            b.fragments.push(Fragment {
                id: i + 1,
                position: V3::new(
                    Fx::from_ratio(97, 2),
                    Fx::from_ratio(25 + i as i32 * 2, 2),
                    Fx::from_ratio(97, 2),
                ),
                velocity: V3::ZERO,
                material: 1,
                paint: None,
                mm: 100,
                basis: IDENTITY,
                mass_g: 1,
                count: 1,
                shape: 1,
                side: 1,
                chunk: None,
                held: i as u32,
                resting: true,
            });
        }
        let expected = b.reference_rubble(&w, &[], &[]);
        assert!(!expected.is_empty());
        assert_eq!(
            b.rubble_with_scratch(&w, &[], &[], &mut scratch, &|_, _| {}),
            expected
        );
        let bounds_pointer = scratch.bounds.as_ptr();
        let bounds_capacity = scratch.bounds.capacity();
        let mask_pointer = scratch.stacked_mask.as_ptr();
        let mask_capacity = scratch.stacked_mask.capacity();
        assert!(mask_capacity > 0);
        let arena_capacity: usize = scratch
            .settled
            .cells
            .stripes
            .iter()
            .map(|s| s.nodes.capacity())
            .sum();
        assert!(
            arena_capacity > 0,
            "fixture must build stacked support lists"
        );
        for _ in 0..4 {
            assert_eq!(
                b.rubble_with_scratch(&w, &[], &[], &mut scratch, &|_, _| {}),
                expected
            );
            assert_eq!(scratch.bounds.as_ptr(), bounds_pointer);
            assert_eq!(scratch.bounds.capacity(), bounds_capacity);
            assert!(scratch.stacked_mask.is_empty());
            assert_eq!(scratch.stacked_mask.as_ptr(), mask_pointer);
            assert_eq!(scratch.stacked_mask.capacity(), mask_capacity);
            assert_eq!(
                scratch
                    .settled
                    .cells
                    .stripes
                    .iter()
                    .map(|s| s.nodes.capacity())
                    .sum::<usize>(),
                arena_capacity
            );
            assert!(scratch
                .settled
                .cells
                .stripes
                .iter()
                .all(|s| s.nodes.is_empty()));
            assert!(scratch.settled.cells.requests.iter().all(Vec::is_empty));
        }
        for x in 47..=49 {
            for z in 47..=49 {
                w.queue_edit(Edit::dig(x, 11, z));
            }
        }
        w.flush();
        for f in &mut b.fragments {
            f.position.y = f.position.y.add(Fx::ONE);
        }
        b.fragments.reverse();
        assert_eq!(
            b.rubble_with_scratch(&w, &[], &[], &mut scratch, &|_, _| {}),
            b.reference_rubble(&w, &[], &[])
        );
        assert!(b
            .rubble_with_scratch(&w, &[1], &[], &mut scratch, &|_, _| {})
            .is_empty());
        let saved = std::mem::take(&mut b.fragments);
        assert!(b
            .rubble_with_scratch(&w, &[], &[], &mut scratch, &|_, _| {})
            .is_empty());
        assert_eq!(scratch.bounds.capacity(), bounds_capacity);
        assert!(scratch.stacked_mask.is_empty());
        assert_eq!(scratch.stacked_mask.capacity(), mask_capacity);
        b.fragments = saved;
        assert_eq!(
            b.rubble_with_scratch(&w, &[], &[], &mut scratch, &|_, _| {}),
            b.reference_rubble(&w, &[], &[])
        );
    }
    #[test]
    fn rubble_floor_proofs_follow_edits_geometry_and_source_compaction() {
        let (mut b, mut w) = (Bursts::default(), world());
        b.fragments.push(Fragment {
            id: 1,
            position: V3::new(
                Fx::from_ratio(97, 2),
                Fx::from_ratio(25, 2),
                Fx::from_ratio(97, 2),
            ),
            velocity: V3::ZERO,
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held: 0,
            resting: true,
        });
        let mut scratch = RubbleScratch::default();
        let mut dirty = nav::vehicle::Changed::default();
        for case in 0..9 {
            match case {
                2 => {
                    dirty.column(90, 90);
                } // unrelated page
                3 => {
                    for x in 47..=49 {
                        for z in 47..=49 {
                            w.queue_edit(Edit::dig(x, 11, z));
                            dirty.column(x as i32, z as i32);
                        }
                    }
                    w.flush();
                }
                4 => {
                    b.fragments[0].position.y = Fx::from_int(11);
                }
                5 => {
                    let f = Fragment {
                        id: 2,
                        position: V3::new(
                            Fx::from_int(51),
                            Fx::from_int(11),
                            Fx::from_ratio(97, 2),
                        ),
                        velocity: V3::ZERO,
                        material: 1,
                        paint: None,
                        mm: 100,
                        basis: IDENTITY,
                        mass_g: 1,
                        count: 1,
                        shape: 1,
                        side: 1,
                        chunk: None,
                        held: 1,
                        resting: true,
                    };
                    b.fragments.insert(0, f);
                }
                6 => {
                    b.fragments.remove(0);
                    b.fragments[0].shape = 3;
                    b.fragments[0].side = 2;
                }
                7 => {
                    b.fragments[0].basis = [V3::UP, IDENTITY[0], IDENTITY[2]];
                }
                8 => {
                    b.fragments[0].resting = false;
                }
                _ => {}
            }
            assert_eq!(
                b.rubble_with_scratch_changed(&w, &[], &[], &mut scratch, &|_, _| {}, Some(&dirty)),
                b.reference_rubble(&w, &[], &[]),
                "case {case}"
            );
            dirty = nav::vehicle::Changed::default();
        }
        b.fragments[0].resting = true;
        assert!(b
            .rubble_with_scratch_changed(&w, &[1], &[], &mut scratch, &|_, _| {}, Some(&dirty))
            .is_empty());
    }
    #[test]
    fn rubble_span_rows_match_cell_enumeration_with_covers_and_reused_storage() {
        let mut fragments = Vec::new();
        for i in 0..1537u32 {
            let turn = (i % 7) as i32 * 2731;
            let (c, s) = (angle::cos(turn), angle::sin(turn));
            fragments.push(Fragment {
                id: u64::from(i) + 1,
                position: V3::new(
                    Fx::from_ratio((i % 40) as i32 * 31 - 3, 2),
                    Fx::from_int(if i % 5 == 0 { -10 } else { 70_000 }),
                    Fx::from_ratio((i / 40) as i32 * 9 - 3, 2),
                ),
                velocity: V3::ZERO,
                material: 1,
                paint: None,
                mm: 150,
                basis: [
                    V3::new(c, Fx::ZERO, s),
                    V3::UP,
                    V3::new(s.neg(), Fx::ZERO, c),
                ],
                mass_g: 1,
                count: 3,
                shape: 7,
                side: 2,
                chunk: None,
                held: 0,
                resting: true,
            });
        }
        let mut hulls: Vec<_> = (0..64)
            .map(|i| Hull {
                pos: [Fx::from_int(i * 9), Fx::ZERO, Fx::from_int(i * 2)],
                head: i * 2731,
                half: [Fx::from_int(3), Fx::from_int(5)],
                height: Fx::ONE,
                vel: [Fx::ZERO; 3],
                step: Fx::ONE,
                solid: None,
            })
            .collect();
        for workers in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            let mut scratch = ColumnMergeScratch::default();
            for (revision, count) in [1537, 1537, 1537, 1024, 1024, 17, 1537, 1537, 1537]
                .into_iter()
                .enumerate()
            {
                if revision == 2 {
                    hulls[0].head += 3141;
                }
                if revision == 4 {
                    hulls[1].pos[0] = hulls[1].pos[0].add(Fx::ONE);
                }
                if revision == 6 {
                    fragments[0].position.y = Fx::ONE;
                }
                if revision == 7 {
                    fragments.reverse();
                    hulls.reverse();
                }
                let reach: Vec<_> = hulls.iter().map(Hull::footprint_box).collect();
                let floor: Vec<_> = (0..count).rev().collect();
                let bounds: Vec<_> = fragments.iter().map(|f| Some(f.bounds())).collect();
                // Independent old algorithm: emit every covered cell into an
                // ordered map, with no ranges, masks, row scatter or reuse.
                let mut expected = BTreeMap::new();
                for &i in &floor {
                    let f = &fragments[i];
                    let p = arr(f.position);
                    let (lo, hi) = f.bounds();
                    let low = [0, 2].map(|a| ((p[a].add(lo[a]).raw() >> 32) as i32).max(0));
                    let high = [0, 2].map(|a| (p[a].add(hi[a]).raw() >> 32) as i32);
                    let top =
                        (-(p[1].add(hi[1]).neg().raw() >> 32)).clamp(0, i64::from(u16::MAX)) as u16;
                    for z in low[1]..=high[1] {
                        for x in low[0]..=high[0] {
                            if hulls
                                .iter()
                                .any(|h| !h.covers_point([p[0], p[2]]) && h.covers(x, z))
                            {
                                continue;
                            }
                            let old = expected.entry([x, z]).or_insert(0u16);
                            *old = (*old).max(top);
                        }
                    }
                }
                let actual = pool.install(|| {
                    gather_column_spans(
                        &fragments,
                        &bounds,
                        &floor,
                        &hulls,
                        &reach,
                        &mut scratch,
                        &|_, _| {},
                    )
                });
                let Some(actual) = actual else {
                    assert_eq!(count, 17, "only the sparse input may fall back");
                    continue;
                };
                assert_eq!(
                    actual,
                    expected.into_iter().collect::<Vec<_>>(),
                    "workers={workers} count={count}"
                );
                // A legacy fallback may reuse the very same row buffers.
                let mut fallback = vec![([3, 5], 0), ([3, 5], u16::MAX), ([1, 2], 7)];
                pool.install(|| merge_columns_observed(&mut fallback, &|_, _| {}, &mut scratch));
                assert_eq!(fallback, vec![([1, 2], 7), ([3, 5], u16::MAX)]);
                if revision == 3 {
                    // Exercise the legacy dense emitter sharing cached buffers.
                    let mut dense = vec![([3, 5], 8); 20_000];
                    pool.install(|| merge_columns_observed(&mut dense, &|_, _| {}, &mut scratch));
                    assert_eq!(dense, vec![([3, 5], 8)]);
                }
            }
            let reach: Vec<_> = hulls.iter().map(Hull::footprint_box).collect();
            let empty = pool.install(|| {
                gather_column_spans(
                    &fragments,
                    &[],
                    &[],
                    &hulls,
                    &reach,
                    &mut scratch,
                    &|_, _| {},
                )
            });
            assert!(empty.is_none());
        }
    }
    #[test]
    fn rubble_row_cache_invalidates_cover_geometry_and_removed_maximum() {
        let piece = || Fragment {
            id: 1,
            position: V3::new(
                Fx::from_ratio(91, 2),
                Fx::from_int(13),
                Fx::from_ratio(97, 2),
            ),
            velocity: V3::ZERO,
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 9600,
            count: 8,
            shape: 0xff,
            side: 2,
            chunk: None,
            held: 0,
            resting: true,
        };
        let mut fragments = (0..1024).map(|_| piece()).collect::<Vec<_>>();
        let mut hulls = vec![Hull {
            pos: [Fx::from_int(50), Fx::from_int(12), Fx::from_int(48)],
            head: 0,
            half: [Fx::from_int(4), Fx::from_int(3)],
            height: Fx::from_int(3),
            vel: [Fx::ZERO; 3],
            step: Fx::from_int(4),
            solid: None,
        }];
        let mut scratch = ColumnMergeScratch::default();
        let mut previous = None;
        for revision in 0..10 {
            match revision {
                2 => hulls[0].head = angle::FULL / 4,
                3 => hulls[0].half = [Fx::ONE, Fx::from_int(4)],
                4 => hulls[0].pos[0] = Fx::from_int(40),
                5 => {
                    hulls.clear();
                    fragments[0].position.y = Fx::from_int(30);
                }
                6 => {
                    fragments.remove(0);
                }
                7 => hulls.push(Hull {
                    pos: [Fx::from_int(50), Fx::ZERO, Fx::from_int(48)],
                    head: 0,
                    half: [Fx::from_int(4), Fx::from_int(3)],
                    height: Fx::ONE,
                    vel: [Fx::ZERO; 3],
                    step: Fx::ONE,
                    solid: None,
                }),
                8 => {
                    hulls[0].pos[1] = Fx::from_int(100);
                    hulls[0].vel = [Fx::ONE; 3];
                }
                _ => {}
            }
            let floor: Vec<_> = (0..fragments.len()).collect();
            let bounds: Vec<_> = fragments.iter().map(|f| Some(f.bounds())).collect();
            let reach: Vec<_> = hulls.iter().map(Hull::footprint_box).collect();
            let mut expected = BTreeMap::<[i32; 2], u16>::new();
            for f in &fragments {
                let p = arr(f.position);
                let (lo, hi) = f.bounds();
                let low = [0, 2].map(|a| ((p[a].add(lo[a]).raw() >> 32) as i32).max(0));
                let high = [0, 2].map(|a| (p[a].add(hi[a]).raw() >> 32) as i32);
                let top =
                    (-(p[1].add(hi[1]).neg().raw() >> 32)).clamp(0, i64::from(u16::MAX)) as u16;
                for x in low[0]..=high[0] {
                    for z in low[1]..=high[1] {
                        if hulls
                            .iter()
                            .any(|h| !h.covers_point([p[0], p[2]]) && h.covers(x, z))
                        {
                            continue;
                        }
                        let old = expected.entry([x, z]).or_default();
                        *old = (*old).max(top);
                    }
                }
            }
            let actual = gather_column_spans(
                &fragments,
                &bounds,
                &floor,
                &hulls,
                &reach,
                &mut scratch,
                &|_, _| {},
            )
            .unwrap();
            assert_eq!(
                actual,
                expected.into_iter().collect::<Vec<_>>(),
                "revision={revision}"
            );
            if [2, 3, 4, 5, 6, 7].contains(&revision) {
                assert_ne!(
                    previous.as_ref().unwrap(),
                    &actual,
                    "mutation must affect output revision={revision}"
                );
            }
            if [1, 8, 9].contains(&revision) {
                assert_eq!(previous.as_ref().unwrap(), &actual);
            }
            previous = Some(actual);
        }
    }
    #[test]
    fn pile_grid_matches_floor_then_euclidean_division_at_signed_boundaries() {
        let mut positions: Vec<_> = (-2056..=2056).map(|i| Fx::from_ratio(i, 8)).collect();
        positions.extend([
            Fx(i64::MIN),
            Fx(i64::MIN + 1),
            Fx(i64::MAX),
            Fx::from_int(i32::MIN),
            Fx::from_int(i32::MAX),
        ]);
        let cells: Vec<u32> = (0..31)
            .map(|shift| 1u32 << shift)
            .chain([3, 5, 17, 31, 255, 1 << 31, u32::MAX - 1, u32::MAX])
            .collect();
        for cell in cells {
            let grid = PileGrid::new(cell);
            assert_eq!(
                grid.shift.is_some(),
                cell <= i32::MAX as u32 && cell.is_power_of_two()
            );
            for &x in &positions {
                let position = V3::new(x, Fx(x.raw() / 3), Fx(x.raw().wrapping_neg()));
                // Old i32 division by -1 also rejects MIN/-1. The normal path
                // is retained for all nonpositive cast divisors.
                if cell == u32::MAX && arr(position).iter().any(|v| v.to_int_trunc() == i32::MIN) {
                    continue;
                }
                let expected = arr(position).map(|v| v.to_int_trunc().div_euclid(cell as i32));
                assert_eq!(grid.at(position), expected, "cell={cell} raw={}", x.raw());
            }
        }
        assert_eq!(
            PileGrid::new(16).at(V3::new(
                Fx::from_ratio(-129, 8),
                Fx::from_ratio(-1, 8),
                Fx::from_ratio(127, 8)
            )),
            [-2, -1, 0]
        );
    }
    #[test]
    fn hull_support_rejects_below_first_roof_without_voxel_queries() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let queries = AtomicUsize::new(0);
        for base in [-129, -65, -64, -1, 0, 1, 63, 64, 129] {
            let voxels = |p: [i32; 3]| {
                queries.fetch_add(1, Ordering::Relaxed);
                p[1] >= base + 3 && (p[0] + p[2]).rem_euclid(3) != 0
            };
            for fraction in [0, 1, 3] {
                for head in [0, 2731, angle::FULL / 8, angle::FULL / 4] {
                    let hull = Hull {
                        pos: [
                            Fx::ZERO,
                            Fx::from_int(base).add(Fx::from_ratio(fraction, 4)),
                            Fx::ZERO,
                        ],
                        head,
                        half: [Fx::from_int(4), Fx::from_int(2)],
                        height: Fx::from_int(8),
                        vel: [Fx::ZERO; 3],
                        step: Fx::ONE,
                        solid: Some(Solid(&voxels)),
                    };
                    for solid in [hull.solid, None] {
                        let hull = Hull { solid, ..hull };
                        for x in [-6, -1, 0, 1, 6] {
                            let centre = V3::new(
                                Fx::from_ratio(x * 2 + 1, 2),
                                Fx::ZERO,
                                Fx::from_ratio(1, 2),
                            );
                            for offset in -8..=40 {
                                let bottom = Fx::from_int(base).add(Fx::from_ratio(offset, 4));
                                let reach = bottom.add(Fx::from_ratio(1, 2));
                                let reference =
                                    hull.lowest(centre, None).is_some_and(|roof| roof <= reach);
                                queries.store(0, Ordering::Relaxed);
                                assert_eq!(hull.carries(bottom, centre), reference);
                                if solid.is_some() && reach < Fx::from_int(base + 1) {
                                    assert_eq!(queries.load(Ordering::Relaxed), 0);
                                }
                                if solid.is_some() {
                                    let limit = ((reach.raw() >> 32) - i64::from(base)).max(0);
                                    assert!(queries.load(Ordering::Relaxed) as i64 <= limit);
                                }
                                assert_eq!(
                                    hull.shelters(bottom, centre),
                                    hull.lowest(centre, None).is_some_and(|roof| roof > reach)
                                );
                            }
                        }
                    }
                    let centre = V3::new(Fx::from_ratio(3, 2), Fx::ZERO, Fx::from_ratio(1, 2));
                    queries.store(0, Ordering::Relaxed);
                    assert!(hull.carries(Fx::from_int(base + 6), centre));
                    assert!(
                        queries.load(Ordering::Relaxed) > 0,
                        "valid roofs still use exact voxels"
                    );
                }
            }
        }
    }
    #[test]
    fn low_support_probe_does_not_scan_a_tall_hollow_hull() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let queries = AtomicUsize::new(0);
        let voxels = |p: [i32; 3]| {
            queries.fetch_add(1, Ordering::Relaxed);
            p[1] == 500
        };
        let hull = Hull {
            pos: [Fx::ZERO; 3],
            head: 0,
            half: [Fx::from_int(4); 2],
            height: Fx::from_int(512),
            vel: [Fx::ZERO; 3],
            step: Fx::ONE,
            solid: Some(Solid(&voxels)),
        };
        let centre = V3::new(Fx::from_ratio(1, 2), Fx::ZERO, Fx::from_ratio(1, 2));
        assert!(!hull.carries(Fx::from_ratio(5, 2), centre));
        assert_eq!(queries.load(Ordering::Relaxed), 3);
        queries.store(0, Ordering::Relaxed);
        assert!(hull.shelters(Fx::from_ratio(5, 2), centre));
        assert_eq!(queries.load(Ordering::Relaxed), 501);
        assert!(hull.carries(Fx::from_ratio(1001, 2), centre));
        assert!(!hull.carries(Fx::from_ratio(2001, 4), centre));
    }
    #[test]
    fn scattered_impulses_are_varied_replayable_and_never_add_energy() {
        let t = tuning().blast;
        let direction = V3::new(Fx::from_int(3), Fx::from_int(2), Fx::ONE);
        let mut velocities = BTreeSet::new();
        let mut speeds = BTreeSet::new();
        for id in 1..128 {
            let v = scattered_launch(id, direction, Fx::ONE, &t);
            assert_eq!(v, scattered_launch(id, direction, Fx::ONE, &t));
            assert!(v.len_sq() <= Fx::ONE);
            velocities.insert(arr(v).map(Fx::raw));
            speeds.insert(v.len_sq().raw());
        }
        assert!(velocities.len() > 120 && speeds.len() > 60);
        assert_eq!(scattered_launch(1, direction, Fx::ZERO, &t), V3::ZERO);
    }
    #[test]
    fn fracture_partition_contains_both_large_chunks_and_small_chips() {
        let t = tuning().blast;
        let mut groups = BTreeMap::<([u32; 3], u8), u64>::new();
        for z in 0..16 {
            for y in 0..8 {
                for x in 0..16 {
                    let p = [x, y, z];
                    let (origin, side) = fracture_box(p, 4, &t);
                    let s = u32::from(side);
                    let bit = x - origin[0] + s * (y - origin[1]) + s * s * (z - origin[2]);
                    let mask = groups.entry((origin, side)).or_default();
                    assert_eq!(*mask & (1 << bit), 0);
                    *mask |= 1 << bit;
                }
            }
        }
        assert!(groups.values().any(|m| m.count_ones() == 64));
        assert!(groups.values().any(|m| m.count_ones() == 8));
        assert_eq!(
            groups.values().map(|m| m.count_ones()).sum::<u32>(),
            16 * 8 * 16
        );
    }
    fn walked_shape_geometry(shape: u64, side: u8) -> (V3, ([Fx; 3], [Fx; 3])) {
        let mut sum = [0; 3];
        let mut lo = [i32::from(side) - 1; 3];
        let mut hi = [0; 3];
        for z in 0..side {
            for y in 0..side {
                for x in 0..side {
                    let i = u32::from(x)
                        + u32::from(side) * u32::from(y)
                        + u32::from(side).pow(2) * u32::from(z);
                    if shape & (1 << i) != 0 {
                        for (a, value) in [x, y, z].map(i32::from).into_iter().enumerate() {
                            sum[a] += value;
                            lo[a] = lo[a].min(value);
                            hi[a] = hi[a].max(value);
                        }
                    }
                }
            }
        }
        let centre = sum.map(|v| Fx::from_ratio(v, shape.count_ones().max(1) as i32));
        (
            v3(centre),
            (
                [0, 1, 2].map(|a| {
                    Fx::from_int(lo[a])
                        .sub(centre[a])
                        .sub(Fx::from_ratio(49, 100))
                }),
                [0, 1, 2].map(|a| {
                    Fx::from_int(hi[a])
                        .sub(centre[a])
                        .add(Fx::from_ratio(49, 100))
                }),
            ),
        )
    }
    #[test]
    fn bit_plane_geometry_matches_cell_walk_for_dense_sparse_and_empty_shapes() {
        for input_side in [0u8, 1, 2, 3, 4, 5, 255] {
            let side = input_side.clamp(1, 4);
            let cells = u32::from(side).pow(3);
            let full = u64::MAX >> (64 - cells);
            let mut shapes = vec![0, full];
            if cells < 64 {
                // Out-of-domain bits still count in the former denominator;
                // they must retain the scalar path rather than truncate.
                shapes.extend([full | (1 << cells), u64::MAX]);
            }
            if side == 4 {
                for count in 0..=64 {
                    let mut shape = 0u64;
                    let mut placed = 0;
                    for y in 0..4 {
                        for z in 0..4 {
                            for x in 0..4 {
                                if placed < count {
                                    shape |= 1u64 << (x + 4 * y + 16 * z);
                                    placed += 1;
                                }
                            }
                        }
                    }
                    assert_eq!(shape_geometry::STACKED[count], shape);
                    shapes.push(shape);
                }
            }
            for i in 0..cells {
                shapes.extend([1 << i, full ^ (1 << i)]);
            }
            if side <= 2 {
                shapes.extend(0..=full);
            } else {
                let mut bits = 0x9d37_a518_6fc2_0be1u64;
                for _ in 0..4096 {
                    bits ^= bits << 13;
                    bits ^= bits >> 7;
                    bits ^= bits << 17;
                    shapes.push(bits & full);
                }
            }
            for shape in shapes {
                let (centre, bounds) = walked_shape_geometry(shape, side);
                assert_eq!(
                    shape_centre(shape, input_side),
                    centre,
                    "side={side} shape={shape:x}"
                );
                assert_eq!(
                    shape_bounds(shape, input_side),
                    bounds,
                    "side={side} shape={shape:x}"
                );
            }
        }
    }
    #[test]
    fn compact_wake_queries_keep_source_slots_and_evaluate_every_fragment() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for len in [0, 1, 1023, 1024, 1100, 39_495] {
            let fragments: Vec<_> = (0..len)
                .map(|i| Fragment {
                    id: (len - i) as u64,
                    position: V3::ZERO,
                    velocity: V3::ZERO,
                    material: 1,
                    paint: None,
                    mm: 100,
                    basis: IDENTITY,
                    mass_g: 1,
                    count: 1,
                    shape: 1,
                    side: 1,
                    chunk: None,
                    held: i as u32,
                    resting: true,
                })
                .collect();
            for workers in [1, 4, 8] {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(workers)
                    .build()
                    .unwrap();
                for mode in 0..3 {
                    let seen = AtomicUsize::new(0);
                    let query = |(i, f): (usize, &Fragment)| {
                        assert_eq!(f.id, (len - i) as u64);
                        seen.fetch_add(1, Ordering::Relaxed);
                        let push = (mode != 0 && (mode == 2 || i % 257 == 0)).then_some(V3::ZERO);
                        let unsupported = mode == 1 && push.is_none() && i % 511 == 0;
                        (push, unsupported)
                    };
                    let dense = pool
                        .install(|| fragment_wake_queries::<false>(&fragments, query, &|_, _| {}));
                    let compact = pool
                        .install(|| fragment_wake_queries::<true>(&fragments, query, &|_, _| {}));
                    let expected: Vec<_> = dense
                        .into_iter()
                        .filter(|(_, (push, unsupported))| push.is_some() || *unsupported)
                        .collect();
                    assert_eq!(
                        compact, expected,
                        "len={len}, workers={workers}, mode={mode}"
                    );
                    assert_eq!(seen.load(Ordering::Relaxed), 2 * len);
                    assert!(compact.windows(2).all(|p| p[0].0 < p[1].0));
                    if mode == 0 {
                        assert!(compact.is_empty());
                    }
                }
            }
        }
    }

    #[test]
    fn settled_query_words_match_cells_across_negative_and_word_boundaries() {
        for bottom in [-129, -65, -64, -1, 0, 1, 63, 64, 127] {
            for height in [1, 2, 63, 64, 65, 131] {
                let mut words = SettledQueries::default();
                let mut cells = BTreeSet::new();
                let boxes = [
                    ([-2, bottom, 1], [0, bottom + height - 1, 2]),
                    ([-1, bottom - 2, 0], [1, bottom + 2, 1]),
                ];
                // Repeated and overlapping requests must retain every bit.
                for &(lo, hi) in boxes.iter().chain(boxes.iter().rev()) {
                    words.build(&[(lo, hi)], false, &|_, _| {});
                    for z in lo[2]..=hi[2] {
                        for y in lo[1]..=hi[1] {
                            for x in lo[0]..=hi[0] {
                                cells.insert([x, y, z]);
                            }
                        }
                    }
                }
                for z in -1..=3 {
                    for y in bottom - 3..=bottom + height.max(4) {
                        for x in -3..=2 {
                            let p = [x, y, z];
                            assert_eq!(words.contains(p), cells.contains(&p), "{p:?}");
                        }
                    }
                }
            }
        }
        assert!(!SettledQueries::default().contains([0; 3]));
    }
    #[test]
    fn settled_intersections_match_geometric_union_at_signed_word_limits() {
        let ranges = [
            ([-4, -130, -2], [-3, -129, 0]),
            ([-1, -65, 2], [2, -63, 3]),
            ([-2, -1, -2], [1, 1, 1]),
            ([3, 62, 0], [5, 65, 2]),
            ([0, 127, -1], [2, 129, 0]),
            ([7, i32::MIN, 4], [7, i32::MIN + 1, 4]),
            ([-7, i32::MAX - 1, -4], [-7, i32::MAX, -4]),
        ];
        // Duplicate requests reach the real paged path, without enlarging
        // the independent geometric oracle or relying on the old iterator.
        let dense: Vec<_> = ranges
            .iter()
            .copied()
            .cycle()
            .take(SettledQueries::PAGED_MIN_RANGES)
            .collect();
        for workers in [1, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            for paged in [false, true] {
                let mut queries = SettledQueries::default();
                pool.install(|| queries.build(&dense, paged, &|_, _| {}));
                assert!(queries.0.iter().all(|stripe| {
                    stripe.dense.as_ref().is_some_and(|d| d.plane.is_some()) == paged
                }));
                for x in [-9, -7, -4, -2, 0, 2, 3, 5, 7, 9] {
                    for z in [-5, -4, -2, 0, 2, 4, 5] {
                        for bottom in [
                            i32::MIN,
                            i32::MIN + 63,
                            -130,
                            -65,
                            -64,
                            -1,
                            0,
                            1,
                            62,
                            63,
                            64,
                            65,
                            127,
                            i32::MAX - 63,
                            i32::MAX,
                        ] {
                            for height in [0, 1, 2, 63, 64, 65, 130] {
                                let lo = [x, bottom, z];
                                let hi = [x + 2, bottom.saturating_add(height), z + 1];
                                let expected = ranges.iter().any(|&(a, b)| {
                                    (0..3).all(|axis| lo[axis] <= b[axis] && hi[axis] >= a[axis])
                                });
                                assert_eq!(
                                    queries.intersects(lo, hi),
                                    expected,
                                    "{lo:?}..{hi:?}, workers={workers}, paged={paged}"
                                );
                            }
                        }
                    }
                }
                for (lo, hi) in [
                    ([0, 3, 0], [1, 2, 1]),
                    ([0, 65, 0], [1, 63, 1]),
                    ([1, 0, 0], [0, 1, 1]),
                    ([0, 0, 1], [1, 1, 0]),
                ] {
                    assert!(!queries.intersects(lo, hi));
                }
                queries.clear();
                assert!(!queries.intersects([-9, -130, -5], [9, 130, 5]));
            }
        }
        // Full signed X/Z still use the sparse representation exactly.
        let mut queries = SettledQueries::default();
        for point in [[i32::MIN, -64, i32::MAX], [i32::MAX, 63, i32::MIN]] {
            queries.build(&[(point, point)], false, &|_, _| {});
            assert!(queries.intersects(point, point));
            let absent = [point[0], point[1] + 1, point[2]];
            assert!(!queries.intersects(absent, absent));
        }
    }
    #[test]
    fn dense_queries_match_sparse_words_columns_and_contact_order() {
        for count in [
            SettledQueries::PAGED_MIN_RANGES - 1,
            SettledQueries::PAGED_MIN_RANGES,
            SettledQueries::PAGED_MIN_RANGES + 1,
        ] {
            let ranges: Vec<_> = (0..count)
                .map(|i| {
                    let i = i as i32;
                    let x = i % 25 - 12;
                    let z = i % 21 - 10;
                    let y = [-65, -1, 0, 63, 64, 127][i as usize % 6];
                    ([x, y, z], [x + 2, y + 3, z + 1])
                })
                .collect();
            let mut reference = SettledQueries::default();
            reference.build(&ranges, false, &|_, _| {});
            let selected: Vec<_> = ranges.iter().copied().map(Some).collect();
            let mut expected_cells = SettledCells::default();
            expected_cells.build(&selected, &reference, false, &|_, _| {});
            for workers in [1, 4, 8] {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(workers)
                    .build()
                    .unwrap();
                let mut actual = SettledQueries::default();
                pool.install(|| actual.build(&ranges, true, &|_, _| {}));
                assert_eq!(actual.word_counts(), reference.word_counts());
                for (a, b) in actual.0.iter().zip(&reference.0) {
                    assert_eq!(a.ordered(), b.ordered());
                    assert_eq!(
                        a.dense.as_ref().is_some_and(|d| d.plane.is_some()),
                        count >= SettledQueries::PAGED_MIN_RANGES
                    );
                }
                for x in -15..=17 {
                    for z in -12..=13 {
                        assert_eq!(
                            actual.column(x, z, -130, 140).collect::<Vec<_>>(),
                            reference.column(x, z, -130, 140).collect::<Vec<_>>()
                        );
                    }
                }
                let mut cells = SettledCells::default();
                pool.install(|| cells.build(&selected, &actual, true, &|_, _| {}));
                assert_eq!(cells.ordered(), expected_cells.ordered());
            }
        }
    }
    #[test]
    fn dense_queries_preserve_additive_build_and_clear_across_layout_changes() {
        let compact = vec![([-12, -1, -3], [19, 65, 4]); SettledQueries::PAGED_MIN_RANGES];
        let sparse =
            vec![([10000, 0, -10000], [10000, 0, -10000]); SettledQueries::PAGED_MIN_RANGES];
        let small = vec![([1, 0, 1], [2, 128, 2]); SettledQueries::PAGED_MIN_RANGES];
        let mut actual = SettledQueries::default();
        let mut reference = SettledQueries::default();
        for ranges in [&compact, &sparse, &small] {
            actual.build(ranges, true, &|_, _| {});
            reference.build(ranges, false, &|_, _| {});
            assert_eq!(actual.word_counts(), reference.word_counts());
            for (a, b) in actual.0.iter().zip(&reference.0) {
                assert_eq!(a.ordered(), b.ordered());
            }
        }
        for ranges in [&small, &sparse, &compact, &Vec::new(), &small] {
            actual.clear();
            reference.clear();
            assert!(actual.is_empty());
            assert!(!actual.contains([10000, 0, -10000]));
            actual.build(ranges, true, &|_, _| {});
            reference.build(ranges, false, &|_, _| {});
            assert_eq!(actual.word_counts(), reference.word_counts());
            for (a, b) in actual.0.iter().zip(&reference.0) {
                assert_eq!(a.ordered(), b.ordered());
            }
        }
    }
    #[test]
    fn paged_query_limit_spills_a_partial_row_without_losing_or_duplicating_words() {
        let mut stripe = SettledQueryStripe::default();
        stripe.prepare(0, Some(([0, 0], [56, 16384])));
        SettledQueries::include(&mut stripe, 0, [0, 0, 0], [56, 0, 16367]);
        stripe.include_base_row(0, 24, 16368, 1);
        assert!(stripe.dense.as_ref().unwrap().plane.is_some());
        assert_eq!(
            stripe.dense.as_ref().unwrap().words.len(),
            DenseQueryWords::MAX_WORDS - 64
        );
        stripe.include_base_row(0, 56, 16384, 1);
        assert!(stripe.dense.as_ref().unwrap().plane.is_none());
        assert_eq!(stripe.base_len(), 16368 * 8 + 4 + 8);
        for z in [0, 15, 16, 16367, 16368, 16369, 16383, 16384] {
            for x in (0..=56).step_by(8) {
                assert_eq!(
                    stripe.base_word(x, z),
                    u64::from(z <= 16367 || z == 16384 || z == 16368 && x <= 24)
                );
            }
        }
        stripe.include_base_row(32, 56, 16384, 2);
        assert_eq!(stripe.base_len(), 16368 * 8 + 4 + 8);
        assert_eq!(stripe.base_word(32, 16384), 3);
        stripe.clear();
        stripe.prepare(0, Some(([0, 0], [0, 0])));
        stripe.include_base_row(0, 0, 0, 4);
        assert_eq!(stripe.base_len(), 1);
        assert_eq!(stripe.base_word(0, 0), 4);
    }
    #[test]
    fn dense_query_layout_bounds_memory_and_balances_mode_observers() {
        assert!(SettledQueries::dense_bounds(&[([0, 64, 0], [0, 64, 0]); 128]).is_none());
        assert!(SettledQueries::dense_bounds(&[([0, 1, 0], [0, 0, 0]); 128]).is_none());
        let far = [
            ([i32::MIN + 8, 0, i32::MIN], [i32::MIN + 8, 0, i32::MIN]),
            ([i32::MAX - 8, 0, i32::MAX], [i32::MAX - 8, 0, i32::MAX]),
        ];
        assert!(SettledQueries::dense_bounds(&far).is_none());
        assert!(SettledQueries::dense_bounds(&[([0, 0, 0], [65536, 0, 65536])]).is_none());
        assert!(SettledQueries::dense_bounds(&[
            ([0, 0, 0], [0, 0, 0]),
            ([1000, 0, 1000], [1000, 0, 1000])
        ])
        .is_some());
        for (ranges, mode) in [
            (
                vec![([-12, -1, -3], [19, 65, 4]); SettledQueries::PAGED_MIN_RANGES],
                "dense",
            ),
            (
                far.into_iter()
                    .cycle()
                    .take(SettledQueries::PAGED_MIN_RANGES)
                    .collect(),
                "sparse",
            ),
        ] {
            let events = std::sync::Mutex::new(BTreeMap::new());
            let task = |_: usize, name: &'static str| {
                *events.lock().unwrap().entry(name).or_insert(0usize) += 1;
            };
            let mut queries = SettledQueries::default();
            queries.build(&ranges, true, &task);
            let events = events.into_inner().unwrap();
            for (family, count) in [("layout", 1), (mode, 8)] {
                assert_eq!(
                    events[format!("physics_queries_{family}_begin").as_str()],
                    count
                );
                assert_eq!(
                    events[format!("physics_queries_{family}_end").as_str()],
                    count
                );
            }
            assert_eq!(events["physics_queries_begin"], 8);
            assert_eq!(events["physics_queries_end"], 8);
        }
    }
    #[test]
    fn split_query_words_preserve_signed_coordinates_and_both_planes_on_reuse() {
        let xs = [i32::MIN + 8, -100, -1, 0, 123, i32::MAX - 8];
        let zs = [i32::MIN, -100, -1, 0, 123, i32::MAX];
        let ranges: Vec<_> = xs
            .into_iter()
            .flat_map(|x| {
                zs.into_iter().flat_map(move |z| {
                    [-129, -65, -1, 0, 62, 63, 64, 127, 128, 1023]
                        .map(|y| ([x, y, z], [x, y + 1, z]))
                })
            })
            .collect();
        let mut expected = BTreeSet::new();
        for &(lo, hi) in &ranges {
            for y in lo[1]..=hi[1] {
                expected.insert([lo[0], y, lo[2]]);
            }
        }
        for workers in [1, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            let mut queries = SettledQueries::default();
            for parallel in [false, true] {
                pool.install(|| queries.build(&ranges, parallel, &|_, _| {}));
                let counts = queries.word_counts();
                assert_eq!(counts[0], xs.len() * zs.len());
                assert!(counts[1] > 0);
                for x in xs {
                    for z in zs {
                        for y in -132..=1026 {
                            let p = [x, y, z];
                            assert_eq!(queries.contains(p), expected.contains(&p));
                        }
                    }
                }
                queries.clear();
                assert_eq!(queries.word_counts(), [0; 2]);
                assert!(queries.is_empty());
                queries.build(&[([2, 0, 3], [2, 0, 3])], false, &|_, _| {});
                assert_eq!(queries.word_counts(), [1, 0]);
                assert!(queries.contains([2, 0, 3]));
                assert!(!queries.contains([2, 64, 3]));
                queries.clear();
            }
        }
    }
    #[test]
    fn vertical_word_iteration_matches_flat_cells_and_ordered_contact_lists() {
        let ranges = [
            ([-9, -130, -2], [9, -63, 2]),
            ([-8, -1, -1], [8, 65, 1]),
            ([-3, 127, 0], [4, 130, 2]),
            ([1, i32::MIN, 1], [1, i32::MIN + 5, 1]),
            ([1, i32::MAX - 5, 1], [1, i32::MAX, 1]),
        ];
        let expected: BTreeSet<_> = ranges
            .iter()
            .flat_map(|&(lo, hi)| {
                (lo[2]..=hi[2]).flat_map(move |z| {
                    (lo[1]..=hi[1]).flat_map(move |y| (lo[0]..=hi[0]).map(move |x| [x, y, z]))
                })
            })
            .collect();
        let selected = [
            Some(([-11, -135, -3], [10, 135, 3])),
            None,
            Some(([-8, -1, -1], [8, 0, 1])),
            Some(([1, i32::MIN, 1], [1, i32::MIN + 8, 1])),
            Some(([1, i32::MAX - 8, 1], [1, i32::MAX, 1])),
        ];
        let mut reference = BTreeMap::<_, Vec<u32>>::new();
        for (i, bounds) in selected.iter().enumerate() {
            let Some((lo, hi)) = bounds else {
                continue;
            };
            for &cell in &expected {
                if (0..3).all(|a| cell[a] >= lo[a] && cell[a] <= hi[a]) {
                    reference.entry(cell).or_default().push(i as u32);
                }
            }
        }
        for workers in [1, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            for parallel in [false, true] {
                let mut queries = SettledQueries::default();
                let mut cells = SettledCells::default();
                for _ in 0..2 {
                    pool.install(|| queries.build(&ranges, parallel, &|_, _| {}));
                    for x in -11..=11 {
                        for z in -3..=3 {
                            for (low, high) in [
                                (-140, -129),
                                (-130, -63),
                                (-65, 1),
                                (-1, 0),
                                (0, 0),
                                (1, 63),
                                (63, 65),
                                (64, 128),
                                (128, 140),
                                (65, 64),
                                (0, -1),
                                (i32::MIN, i32::MIN + 8),
                                (i32::MAX - 8, i32::MAX),
                            ] {
                                let actual: Vec<_> = queries.column(x, z, low, high).collect();
                                let ys: Vec<_> = (low..=high)
                                    .filter(|&y| expected.contains(&[x, y, z]))
                                    .collect();
                                assert_eq!(actual, ys, "workers={workers} parallel={parallel} x={x} z={z} low={low} high={high}");
                                assert_eq!(
                                    queries.intersects([x, low, z], [x, high, z]),
                                    !ys.is_empty()
                                );
                            }
                        }
                    }
                    pool.install(|| cells.build(&selected, &queries, parallel, &|_, _| {}));
                    assert_eq!(cells.ordered(), reference);
                    cells.clear();
                    queries.clear();
                    assert_eq!(queries.column(0, 0, -130, 130).next(), None);
                }
            }
        }
    }
    #[test]
    fn settled_index_matches_unpacked_requests_and_preserves_piece_indices() {
        let t = tuning().blast;
        let hull = Hull {
            pos: [Fx::from_int(8), Fx::ZERO, Fx::from_int(4)],
            head: 2731,
            half: [Fx::from_int(2); 2],
            height: Fx::from_int(6),
            vel: [Fx::ZERO; 3],
            step: Fx::ONE,
            solid: None,
        };
        let mut fragments: Vec<_> = (0..48u32)
            .map(|i| Fragment {
                id: u64::from(i) + 1,
                position: V3::new(
                    Fx::from_ratio((i % 8) as i32 * 5 - 9, 2),
                    Fx::from_ratio((i / 8) as i32 * 5 - 1, 2),
                    Fx::from_int((i % 3) as i32 * 4),
                ),
                velocity: V3::ZERO,
                material: 1,
                paint: None,
                mm: 100,
                basis: IDENTITY,
                mass_g: 1,
                count: 3,
                shape: 7,
                side: 2,
                chunk: None,
                held: i,
                resting: i % 3 != 0,
            })
            .collect();
        // Far sleepers and interleaved flying pieces leave gaps in the
        // indices held by the grid. Its box table must not compact them.
        fragments[1].position.x = Fx::from_int(1000);
        let mut index = Settled::default();
        let mut queries = SettledQueries::default();
        for tick in [0, 1, u64::from(t.rest_check_ticks)] {
            let world_boxes: Vec<_> = fragments
                .iter()
                .map(|f| {
                    let (lo, hi) = f.bounds();
                    let p = arr(f.position);
                    (
                        [0, 1, 2].map(|a| p[a].add(lo[a])),
                        [0, 1, 2].map(|a| p[a].add(hi[a])),
                    )
                })
                .collect();
            let mut want = BTreeSet::new();
            for (i, f) in fragments.iter().enumerate() {
                if f.resting && !due(f.id, tick, &t) {
                    continue;
                }
                let (lo, hi) = settled_cells(world_boxes[i].0, world_boxes[i].1);
                for z in lo[2] - 1..=hi[2] + 1 {
                    for y in lo[1] - 1..=hi[1] + 1 {
                        for x in lo[0] - 1..=hi[0] + 1 {
                            want.insert([x, y, z]);
                        }
                    }
                }
            }
            let mut expected = HashMap::<_, Vec<u32>>::new();
            for (i, f) in fragments.iter().enumerate() {
                if !f.resting || hull.carries(world_boxes[i].0[1], f.position) {
                    continue;
                }
                let (lo, hi) = settled_cells(world_boxes[i].0, world_boxes[i].1);
                for z in lo[2]..=hi[2] {
                    for y in lo[1]..=hi[1] {
                        for x in lo[0]..=hi[0] {
                            if want.contains(&[x, y, z]) {
                                expected.entry([x, y, z]).or_default().push(i as u32);
                            }
                        }
                    }
                }
            }
            Settled::of_into(
                &fragments,
                &[hull],
                tick,
                &t,
                &mut index,
                &mut queries,
                &mut |_| {},
                &|_, _| {},
            );
            assert!(!expected.is_empty());
            assert_eq!(index.cells.len(), expected.len());
            for (cell, pieces) in &expected {
                assert_eq!(
                    index.cells.get(cell).map(Iterator::collect::<Vec<_>>),
                    Some(pieces.clone())
                );
            }
            for (i, &(lo, hi)) in world_boxes.iter().enumerate() {
                for skip in [i as u32, u32::MAX] {
                    for from_above in [None, Some(lo[1]), Some(hi[1])] {
                        let (c0, c1) = settled_cells(lo, hi);
                        let hit = expected.iter().any(|(cell, pieces)| {
                            (0..3).all(|a| cell[a] >= c0[a] && cell[a] <= c1[a])
                                && pieces.iter().any(|&other| {
                                    let (qlo, qhi) = world_boxes[other as usize];
                                    other != skip
                                        && from_above.is_none_or(|bottom| qhi[1] <= bottom)
                                        && (0..3).all(|a| lo[a] < qhi[a] && qlo[a] < hi[a])
                                })
                        });
                        assert_eq!(index.hit(lo, hi, skip, from_above), hit);
                    }
                }
            }
        }
        for f in &mut fragments {
            f.resting = false;
        }
        Settled::of_into(
            &fragments,
            &[],
            0,
            &t,
            &mut index,
            &mut queries,
            &mut |_| {},
            &|_, _| {},
        );
        assert!(index.cells.is_empty() && index.boxes.is_empty());
        fragments[0].resting = true;
        let sleeping = Settled::of(&fragments[..1], &[], 0, &t);
        assert!(!due(fragments[0].id, 0, &t));
        assert!(sleeping.cells.is_empty() && sleeping.boxes.is_empty());
    }
    #[test]
    fn reduced_fragment_bounds_match_all_eight_rounded_corners() {
        for side in 1..=4 {
            let cells = box_cells(side);
            let mask = u64::MAX >> (64 - cells);
            let shapes: Vec<_> = if side == 2 {
                (0..=mask).collect()
            } else {
                vec![0, 1, 1 << (cells - 1), mask, mask & 0x9d37_a518_6fc2_0be1]
            };
            for shape in shapes {
                let centre = arr(walked_shape_geometry(shape, side).0);
                let mut lo = [i32::from(side) - 1; 3];
                let mut hi = [0; 3];
                for i in 0..cells {
                    if shape & (1 << i) != 0 {
                        let p = shape_cell(i, side);
                        for a in 0..3 {
                            lo[a] = lo[a].min(p[a]);
                            hi[a] = hi[a].max(p[a]);
                        }
                    }
                }
                let lo = [0, 1, 2].map(|a| {
                    Fx::from_int(lo[a])
                        .sub(centre[a])
                        .sub(Fx::from_ratio(49, 100))
                });
                let hi = [0, 1, 2].map(|a| {
                    Fx::from_int(hi[a])
                        .sub(centre[a])
                        .add(Fx::from_ratio(49, 100))
                });
                for mm in [1, 63, 100, 500, u16::MAX] {
                    let scale = Fx::from_ratio(i32::from(mm), voxel::unit::WORLD_MM as i32);
                    for turn in [0, 1, 2731, angle::FULL / 4, -11739] {
                        let (c, s) = (angle::cos(turn), angle::sin(turn));
                        let (cp, sp) = (angle::cos(turn / 3), angle::sin(turn / 3));
                        let basis = [
                            V3::new(c, Fx::ZERO, s),
                            V3::UP,
                            V3::new(s.neg(), Fx::ZERO, c),
                        ]
                        .map(|v| {
                            V3::new(
                                v.x,
                                v.y.mul(cp).sub(v.z.mul(sp)),
                                v.y.mul(sp).add(v.z.mul(cp)),
                            )
                        });
                        let mut expected = ([Fx::MAX; 3], [Fx::MIN; 3]);
                        for corner in 0..8 {
                            let mut p = V3::ZERO;
                            for a in 0..3 {
                                let endpoint = if corner & (1 << a) == 0 { lo[a] } else { hi[a] };
                                p = p.add(basis[a].scale(endpoint.mul(scale)));
                            }
                            for (a, value) in arr(p).into_iter().enumerate() {
                                expected.0[a] = expected.0[a].min(value);
                                expected.1[a] = expected.1[a].max(value);
                            }
                        }
                        let mut fragment = Fragment {
                            id: 1,
                            position: V3::ZERO,
                            velocity: V3::ZERO,
                            material: 1,
                            paint: None,
                            mm,
                            basis,
                            mass_g: 1,
                            count: shape.count_ones(),
                            shape,
                            side,
                            chunk: None,
                            held: 0,
                            resting: false,
                        };
                        assert_eq!(
                            fragment.bounds(),
                            expected,
                            "side={side} shape={shape:x} mm={mm} turn={turn}"
                        );
                        for position in [
                            V3::new(Fx::from_ratio(-17, 3), Fx(1), Fx::ONE),
                            V3::new(Fx::MIN, Fx::MAX, Fx(-1234567)),
                        ] {
                            fragment.position = position;
                            let p = arr(position);
                            let frozen = Settled {
                                boxes: vec![(
                                    [0, 1, 2].map(|a| p[a].add(expected.0[a])),
                                    [0, 1, 2].map(|a| p[a].add(expected.1[a])),
                                )],
                                ..Settled::default()
                            };
                            assert_eq!(frozen.fragment_bounds(0, &fragment), expected);
                            assert_eq!(frozen.fragment_bounds(u32::MAX, &fragment), expected);
                            assert_eq!(Settled::default().fragment_bounds(0, &fragment), expected);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn empty_corner_of_a_fragment_does_not_glue_it_to_a_wall() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.queue_edit(Edit::put(49, 14, 48, 2));
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        let start = centre([48, 13, 48]).add(shape_centre(7, 2));
        b.fragments.push(Fragment {
            id: 1,
            position: start,
            velocity: V3::new(Fx::from_int(-2), Fx::ZERO, Fx::ZERO),
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 3600,
            count: 3,
            shape: 7,
            side: 2,
            chunk: None,
            held,
            resting: false,
        });
        b.step_fragments(
            &w,
            &mut e,
            &t.blast,
            crate::gravity_per_tick(&t),
            0,
            Surroundings::NONE,
        );
        assert!(b.fragments[0].position.x < start.x.sub(Fx::ONE));
        assert!(!b.fragments[0].resting);
        for _ in 0..300 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                0,
                Surroundings::NONE,
            );
        }
        assert!(b.fragments[0].resting);
        assert!(b.fragments[0].position.y < Fx::from_int(14));
    }

    /// A piece on a shelf stays put while the shelf stands, and when the
    /// shelf is dug away it wakes within the check period, falls to the
    /// ground and settles again, its wreck entry following (ADR-291).
    #[test]
    fn a_resting_piece_falls_again_when_the_ground_under_it_is_dug() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.queue_edit(Edit::put(48, 14, 48, 2));
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        b.fragments.push(Fragment {
            id: 1,
            position: centre([48, 16, 48]),
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
            held,
            resting: false,
        });
        let mut tick = 0;
        while !b.fragments[0].resting {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
            tick += 1;
            assert!(tick < 300, "never rested");
        }
        let shelf = b.fragments[0].position.y;
        assert!(
            shelf > Fx::from_int(15) && shelf < Fx::from_int(16),
            "not on the shelf: {shelf:?}"
        );
        assert_eq!(e.wrecks.iter().filter(|w| w.held == held).count(), 1);
        for _ in 0..t.blast.rest_check_ticks * 2 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
            tick += 1;
        }
        assert!(b.fragments[0].resting);
        assert_eq!(b.fragments[0].position.y, shelf);
        w.queue_edit(Edit::dig(48, 14, 48));
        w.begin_tick();
        w.flush();
        assert!(!w.solid(48, 14, 48));
        let mut woke = false;
        for _ in 0..300 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
            tick += 1;
            woke |= !b.fragments[0].resting;
            if woke && b.fragments[0].resting {
                break;
            }
        }
        assert!(woke, "the piece never woke");
        assert!(b.fragments[0].resting, "the piece did not settle again");
        assert!(
            b.fragments[0].position.y < Fx::from_int(14),
            "still up: {:?}",
            b.fragments[0].position
        );
        let wrecks: Vec<_> = e.wrecks.iter().filter(|w| w.held == held).collect();
        assert_eq!(wrecks.len(), 1);
        assert!(wrecks[0].y < 14);
        assert!(e.balanced());
    }

    /// **A waking piece does not take a claim's flying slot** (ADR-443).
    /// A claim whose dig the edit ceiling deferred holds its slot into the
    /// next tick; a piece woken meanwhile took it, and the claim's piece
    /// then flew over the budget.
    #[test]
    fn a_waking_piece_waits_behind_a_claim() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let mut t = tuning();
        t.blast.max_flying = 1;
        w.queue_edit(Edit::put(48, 14, 48, 2));
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        b.fragments.push(Fragment {
            id: 1,
            position: centre([48, 16, 48]),
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
            held,
            resting: false,
        });
        let g = crate::gravity_per_tick(&t);
        let mut tick = 0;
        while !b.fragments[0].resting {
            b.step_fragments(&w, &mut e, &t.blast, g, tick, Surroundings::NONE);
            tick += 1;
            assert!(tick < 300, "never rested");
        }
        // A claim takes the one flying slot; the shelf goes.
        assert!(b.claim_voxel(&mut w, [20, 10, 20], 1, [20, 12, 20], 0, 1, &t.blast));
        w.queue_edit(Edit::dig(48, 14, 48));
        w.begin_tick();
        w.flush();
        for _ in 0..t.blast.rest_check_ticks * 2 {
            b.step_fragments(&w, &mut e, &t.blast, g, tick, Surroundings::NONE);
            tick += 1;
            assert!(b.fragments[0].resting, "the piece took the claim's slot");
        }
        b.clear_claims();
        let mut woke = false;
        for _ in 0..t.blast.rest_check_ticks * 2 {
            b.step_fragments(&w, &mut e, &t.blast, g, tick, Surroundings::NONE);
            tick += 1;
            woke |= !b.fragments[0].resting;
        }
        assert!(woke, "the piece never woke once the slot was free");
    }

    /// Landing retains the actual shard shape/turn and its support columns.
    #[test]
    fn pieces_settling_together_keep_their_shapes_and_remain_floor() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        for (id, x) in [(1, 48), (2, 49)] {
            let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
            b.fragments.push(Fragment {
                id,
                position: centre([x, 16, 48]),
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
                held,
                resting: false,
            });
        }
        for tick in 0..300 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        assert_eq!(b.fragments.iter().map(|f| f.count).sum::<u32>(), 2);
        assert!(b
            .fragments
            .iter()
            .all(|f| f.resting && f.mm == 100 && f.count == f.shape.count_ones()));
        let rubble = b.rubble(&w, &[], &[]);
        assert!(rubble.get(&[48, 48]).is_some_and(|h| (13..=14).contains(h)));
        assert!(
            b.rubble(&w, &[1], &[]).is_empty(),
            "a yielding heap is no floor"
        );
        assert!(rubble.get(&[49, 48]).is_some_and(|h| (13..=14).contains(h)));
        assert_eq!(rubble.get(&[51, 48]), None);
        assert!(e.balanced());
    }

    /// **A pile grows on what its piece rests on** (ADR-457). A merge
    /// dropped the pile onto the world under its centre: a soil piece
    /// lying on a rock piece sank into the rock when a second soil piece
    /// settled in its pile cell.
    #[test]
    fn a_pile_merged_on_a_resting_piece_stays_on_it() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        // A ledge of the world, four high, for the second soil piece.
        for y in 12..16 {
            w.queue_edit(Edit::put(54, y, 49, 2));
        }
        w.flush();
        let piece = |e: &mut Economy, id, position, material, shape, side, count| Fragment {
            id,
            position,
            velocity: V3::ZERO,
            material,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1200 * u64::from(count),
            count,
            shape,
            side,
            chunk: None,
            held: e.ledger.create(matter::Kind::Debris, 0, u64::MAX),
            resting: true,
        };
        // A rock block on the ground, 48..52 by 12..16 by 48..52; a soil
        // piece on it; another on the ledge, in the same pile cell.
        let rock = V3::new(Fx::from_int(50), Fx::from_int(14), Fx::from_int(50));
        b.fragments.push(piece(&mut e, 1, rock, 2, u64::MAX, 4, 64));
        b.fragments
            .push(piece(&mut e, 2, centre([49, 16, 49]), 1, 1, 1, 1));
        b.fragments
            .push(piece(&mut e, 3, centre([54, 16, 49]), 1, 1, 1, 1));
        b.step_fragments(
            &w,
            &mut e,
            &t.blast,
            crate::gravity_per_tick(&t),
            0,
            Surroundings::NONE,
        );
        assert_eq!(
            b.fragments.len(),
            3,
            "separated pieces must not teleport into a pile"
        );
        let pile = &b.fragments[1];
        assert_eq!((pile.material, pile.count), (1, 1));
        assert!(pile.resting);
        let bottom = pile.position.y.add(pile.bounds().0[1]);
        assert!(
            bottom >= Fx::from_int(16) && bottom < Fx::from_ratio(33, 2),
            "the pile sank into the rock under it: bottom {bottom:?}"
        );
        assert!(e.balanced());
    }

    /// **A piece lying on a piece is floor at its top** (ADR-458). The
    /// rubble map took only what the world held, and a pile on a rock
    /// piece was missing: the hull met the rock and not the pile on it.
    /// A piece whose support is gone is still no floor.
    #[test]
    fn a_stacked_piece_is_floor_and_a_hovering_one_is_not() {
        let (mut b, mut e) = (Bursts::default(), economy());
        let w = world();
        let piece = |e: &mut Economy, id, position, material, shape, side| Fragment {
            id,
            position,
            velocity: V3::ZERO,
            material,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1200,
            count: 1,
            shape,
            side,
            chunk: None,
            held: e.ledger.create(matter::Kind::Debris, 0, u64::MAX),
            resting: true,
        };
        // A rock block on the ground, 48..52 by 12..16 by 48..52, a soil
        // piece on it, and one left hanging at 20 over bare ground.
        let rock = V3::new(Fx::from_int(50), Fx::from_int(14), Fx::from_int(50));
        b.fragments.push(piece(&mut e, 1, rock, 2, u64::MAX, 4));
        b.fragments
            .push(piece(&mut e, 2, centre([49, 16, 49]), 1, 1, 1));
        b.fragments
            .push(piece(&mut e, 3, centre([60, 20, 60]), 1, 1, 1));
        let rubble = b.rubble(&w, &[], &[]);
        assert_eq!(rubble.get(&[50, 50]), Some(&16), "the rock is not floor");
        assert_eq!(
            rubble.get(&[49, 49]),
            Some(&17),
            "the stacked piece is not floor"
        );
        assert_eq!(rubble.get(&[60, 60]), None, "a hanging piece is floor");
    }

    /// **A piece on a roof holds nothing** (ADR-293, ADR-458): what falls
    /// on it lands on the roof. The test for a rider asked whether the
    /// piece's centre lay inside the hull's box, and a piece on a roof
    /// has it above: riders held what fell on them.
    #[test]
    fn a_piece_on_a_roof_holds_nothing() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        let hull = Hull {
            pos: [Fx::from_int(48), Fx::from_int(12), Fx::from_int(48)],
            head: 0,
            half: [Fx::from_int(6), Fx::from_int(6)],
            height: Fx::from_int(3),
            vel: [Fx::ZERO; 3],
            step: Fx::ZERO,
            solid: None,
        };
        let piece = |e: &mut Economy, id, at: [u32; 3], material| Fragment {
            id,
            position: centre(at),
            velocity: V3::ZERO,
            material,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1200,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held: e.ledger.create(matter::Kind::Debris, 0, u64::MAX),
            resting: false,
        };
        // A rider on the roof at 15, and a piece of another matter
        // dropped onto it from above.
        b.fragments.push(piece(&mut e, 1, [48, 17, 48], 1));
        let around = Surroundings {
            hulls: &[hull],
            yielding: &[],
        };
        let g = crate::gravity_per_tick(&t);
        let mut tick = 0;
        while !b.fragments[0].resting {
            b.step_fragments(&w, &mut e, &t.blast, g, tick, around);
            tick += 1;
            assert!(tick < 300, "the rider never rested");
        }
        b.fragments.push(piece(&mut e, 2, [48, 22, 48], 2));
        while !b.fragments[1].resting {
            b.step_fragments(&w, &mut e, &t.blast, g, tick, around);
            tick += 1;
            assert!(tick < 300, "the second piece never rested");
        }
        let bottom = |f: &Fragment| f.position.y.add(f.bounds().0[1]);
        assert!(
            bottom(&b.fragments[1]) < bottom(&b.fragments[0]).add(Fx::from_ratio(1, 2)),
            "the rider held the piece: {:?} over {:?}",
            b.fragments[1].position,
            b.fragments[0].position
        );
    }

    /// **A piece meets a hull where it meets the hull's voxels**
    /// (ADR-469), not its box: dropped over the deck, it rests on the deck
    /// under the turret's height and rides there; over the turret, on the
    /// turret; thrown across above the deck, it passes the box's side and
    /// stops at the turret; and one lying under the belly still leaves the
    /// box by the nearest side (ADR-302).
    #[test]
    fn a_piece_meets_the_hulls_voxels_and_not_its_box() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        // A deck at 14..16 over the whole footprint, a turret at 16..22
        // over the middle, and the belly clear under the deck.
        let shape = |[x, y, z]: [i32; 3]| {
            let over = |lo, hi| (lo..hi).contains(&x) && (lo..hi).contains(&z);
            over(42, 54) && (14..16).contains(&y) || over(46, 50) && (16..22).contains(&y)
        };
        let hull = Hull {
            pos: [Fx::from_int(48), Fx::from_int(12), Fx::from_int(48)],
            head: 0,
            half: [Fx::from_int(6), Fx::from_int(6)],
            height: Fx::from_int(10),
            vel: [Fx::ZERO; 3],
            step: Fx::ZERO,
            solid: Some(Solid(&shape)),
        };
        let around = Surroundings {
            hulls: &[hull],
            yielding: &[],
        };
        let g = crate::gravity_per_tick(&t);
        let mut tick = 0;
        let mut rest = |b: &mut Bursts, e: &mut Economy, at: [u32; 3], velocity: V3| {
            let velocity = velocity.scale(Fx::from_ratio(
                modding::clock::STREAM_HZ as i32,
                crate::TICKS_PER_SECOND,
            ));
            b.fragments.clear();
            b.fragments.push(Fragment {
                id: 1,
                position: centre(at),
                velocity,
                material: 1,
                paint: None,
                mm: 100,
                basis: IDENTITY,
                mass_g: 1200,
                count: 1,
                shape: 1,
                side: 1,
                chunk: None,
                held: e.ledger.create(matter::Kind::Debris, 0, u64::MAX),
                resting: false,
            });
            let start = tick;
            while !b.fragments[0].resting {
                b.step_fragments(&w, e, &t.blast, g, tick, around);
                tick += 1;
                assert!(tick - start < 300, "the piece from {at:?} never rested");
            }
            let f = &b.fragments[0];
            (f.position, f.position.y.add(f.bounds().0[1]))
        };
        let (at, bottom) = rest(&mut b, &mut e, [44, 30, 48], V3::ZERO);
        assert!(
            bottom >= Fx::from_int(16) && bottom < Fx::from_ratio(33, 2),
            "not on the deck: {at:?}"
        );
        assert!(hull.carries(bottom, at), "the deck does not carry it");
        let (at, bottom) = rest(&mut b, &mut e, [48, 30, 48], V3::ZERO);
        assert!(
            bottom >= Fx::from_int(22) && bottom < Fx::from_ratio(45, 2),
            "not on the turret: {at:?}"
        );
        // Across above the deck, from afar and from the box's very side,
        // where the piece's box lies in the hull's and its centre outside.
        for (from, speed) in [
            ([34, 18, 48], Fx::from_int(2)),
            ([41, 18, 48], Fx::from_ratio(1, 2)),
        ] {
            let across = V3::new(speed, Fx::ZERO, Fx::ZERO);
            let (at, bottom) = rest(&mut b, &mut e, from, across);
            assert!(
                at.x > Fx::from_int(42) && at.x < Fx::from_int(46),
                "did not stop between the box's side and the turret: {at:?}"
            );
            assert!(
                bottom >= Fx::from_int(16) && bottom < Fx::from_ratio(33, 2),
                "not on the deck: {at:?}"
            );
        }
        b.fragments[0].position = centre([44, 12, 48]);
        b.fragments[0].resting = false;
        b.step_fragments(&w, &mut e, &t.blast, g, tick, around);
        let at = b.fragments[0].position;
        assert!(at.x < Fx::from_int(42), "stayed under the belly: {at:?}");
        assert!(e.balanced());
    }

    /// A piece falling onto a live hull rests on its roof, and when the
    /// hull is gone from under it, it falls to the ground (ADR-293).
    #[test]
    fn a_piece_rests_on_a_hull_and_falls_when_the_hull_leaves() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        b.fragments.push(Fragment {
            id: 1,
            position: centre([48, 20, 48]),
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
            held,
            resting: false,
        });
        let hull = Hull {
            pos: [Fx::from_int(48), Fx::from_int(12), Fx::from_int(48)],
            head: angle::FULL / 8,
            half: [Fx::from_int(3), Fx::from_int(2)],
            height: Fx::from_int(3),
            vel: [Fx::ZERO; 3],
            step: Fx::ZERO,
            solid: None,
        };
        let mut tick = 0;
        while !b.fragments[0].resting {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings {
                    hulls: &[hull],
                    yielding: &[],
                },
            );
            tick += 1;
            assert!(tick < 300, "never rested");
        }
        let roof = b.fragments[0].position.y;
        assert!(
            roof > Fx::from_int(15) && roof < Fx::from_int(16),
            "not on the roof: {roof:?}"
        );
        assert!(
            b.rubble(&w, &[], &[]).is_empty(),
            "a piece on a roof is no heap"
        );
        let mut woke = false;
        for _ in 0..300 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
            tick += 1;
            woke |= !b.fragments[0].resting;
            if woke && b.fragments[0].resting {
                break;
            }
        }
        assert!(
            woke && b.fragments[0].resting,
            "the piece did not fall and settle"
        );
        assert!(
            b.fragments[0].position.y < Fx::from_int(13),
            "still up: {:?}",
            b.fragments[0].position
        );
        assert!(e.balanced());
    }

    /// A hull driving into a lying piece bulldozes it: the piece leaves
    /// ahead of the hull and settles again outside it (ADR-294).
    #[test]
    fn a_moving_hull_bulldozes_a_lying_piece_out_of_its_way() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        b.fragments.push(Fragment {
            id: 1,
            position: centre([48, 12, 48]),
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
            held,
            resting: true,
        });
        let mut hull = Hull {
            pos: [Fx::from_int(40), Fx::from_int(12), Fx::from_int(48)],
            head: 0,
            half: [Fx::from_int(3), Fx::from_int(2)],
            height: Fx::from_int(3),
            vel: [Fx::from_ratio(1, 2), Fx::ZERO, Fx::ZERO],
            step: Fx::ZERO,
            solid: None,
        };
        let mut tick = 0;
        let mut moved = false;
        for _ in 0..40 {
            hull.pos[0] = hull.pos[0].add(hull.vel[0]);
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings {
                    hulls: &[hull],
                    yielding: &[],
                },
            );
            tick += 1;
            moved |= !b.fragments[0].resting;
        }
        assert!(moved, "the hull drove through the piece");
        hull.vel = [Fx::ZERO; 3];
        for _ in 0..300 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings {
                    hulls: &[hull],
                    yielding: &[],
                },
            );
            tick += 1;
            if b.fragments[0].resting {
                break;
            }
        }
        let f = &b.fragments[0];
        let front = hull.pos[0].add(hull.half[0]);
        assert!(f.resting, "the piece never settled again");
        assert!(
            f.position.x >= front.sub(Fx::ONE),
            "the piece stayed under the hull: {:?}, front {front:?}",
            f.position
        );
        assert!(
            f.position.y < Fx::from_int(13),
            "the piece did not come down: {:?}",
            f.position
        );
        let wrecks: Vec<_> = e.wrecks.iter().filter(|w| w.held == held).collect();
        assert_eq!(wrecks.len(), 1, "the pushed piece lost its wreck marker");
        assert_eq!(wrecks[0].x, f.position.x.to_int_trunc());
        assert!(e.balanced());
    }

    /// Cells the world let go of become pieces at zero speed; with no
    /// room in the pool the whole shape goes back into the world, so no
    /// matter is lost (ADR-299).
    #[test]
    fn unsupported_cells_become_pieces_or_go_back_when_the_pool_is_full() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let mut t = tuning().blast;
        w.flush();
        let cells: Vec<([u32; 3], Mat)> = (0..8u32)
            .map(|i| ([48 + i % 2, 20 + i / 2 % 2, 48 + i / 4], 1))
            .collect();
        let (made, returned) = b.pieces_from_cells(&mut w, &cells, &mut e, &t, 2);
        assert_eq!((made, returned), (1, 0));
        assert_eq!(b.fragments[0].count, 8);
        assert_eq!(b.fragments[0].velocity, V3::ZERO);
        assert_eq!(e.ledger.held(b.fragments[0].held), 8 * e.per_voxel(1));
        assert!(e.balanced());
        t.max_fragments = 1;
        let more: Vec<([u32; 3], Mat)> = vec![([60, 20, 60], 1)];
        let (made, returned) = b.pieces_from_cells(&mut w, &more, &mut e, &t, 2);
        assert_eq!((made, returned), (0, 1));
        w.begin_tick();
        w.flush();
        assert!(w.solid(60, 20, 60), "the cell did not go back");
        assert!(e.balanced());
    }

    struct Ground;
    impl voxel::world::Generator for Ground {
        fn material(&self, _x: u32, y: u32, _z: u32) -> Mat {
            if y < 12 {
                1
            } else {
                AIR
            }
        }
    }
    fn tuning() -> crate::Tuning {
        crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap()
    }
    #[test]
    fn settled_irregular_piece_does_not_support_an_empty_corner() {
        let piece = Fragment {
            id: 1,
            position: V3::new(Fx::from_int(48), Fx::from_int(20), Fx::from_int(48)),
            velocity: V3::ZERO,
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1,
            count: 3,
            shape: 0b1_0011,
            side: 2,
            chunk: None,
            held: 0,
            resting: true,
        };
        let index = Settled::around(&[piece], &[], |_, _| true);
        let at = |x: i32, z: i32| {
            [
                Fx::from_int(48).add(Fx::from_ratio(x, 3)),
                Fx::from_int(20),
                Fx::from_int(48).add(Fx::from_ratio(z, 3)),
            ]
        };
        let meets = |p: [Fx; 3]| {
            index.hit(
                p.map(|v| v.sub(Fx::from_ratio(1, 10))),
                p.map(|v| v.add(Fx::from_ratio(1, 10))),
                u32::MAX,
                None,
            )
        };
        assert!(
            !meets(at(2, 2)),
            "an empty corner is not a physical support"
        );
        assert!(meets(at(2, -1)), "occupied material must remain collidable");
    }
    #[test]
    fn support_under_only_the_riders_empty_corner_does_not_suspend_it() {
        let rider = Fragment {
            id: 1,
            position: V3::new(Fx::from_int(48), Fx::from_int(20), Fx::from_int(48)),
            velocity: V3::ZERO,
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1,
            count: 3,
            shape: 0b1_0011,
            side: 2,
            chunk: None,
            held: 0,
            resting: false,
        };
        let make_support = |z: i32| Fragment {
            id: 2,
            position: V3::new(
                Fx::from_int(48).add(Fx::from_ratio(2, 3)),
                Fx::from_int(19),
                Fx::from_int(48).add(Fx::from_ratio(z, 3)),
            ),
            velocity: V3::ZERO,
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held: 0,
            resting: true,
        };
        let check = |support| {
            let pieces = [
                Fragment {
                    chunk: None,
                    ..rider
                },
                support,
            ];
            let index = Settled::around(&pieces, &[], |_, _| true);
            let (lo, hi) = pieces[0].bounds();
            index.supports(arr(pieces[0].position), lo, hi, 0)
        };
        assert!(
            !check(make_support(2)),
            "empty rider corner must not hold the piece"
        );
        assert!(
            check(make_support(-1)),
            "occupied rider corner must retain support"
        );
    }
    #[test]
    fn thin_stacked_fragments_sleep_until_their_support_is_removed() {
        let (mut b, mut e) = (Bursts::default(), economy());
        let mut w = world();
        let t = tuning();
        w.queue_edit(Edit::put(48, 15, 48, 2));
        w.flush();
        // A thin plate overhangs a ledge; a smaller shard sits on its lip,
        // with no terrain beneath it for the old endpoint probe to find.
        for (id, x, y, mm, material, shape, side) in [
            (1, 487, 1610, 20, 2, 0x000f000f000f000fu64, 4),
            (2, 4905, 1625, 10, 3, 1, 1),
        ] {
            b.fragments.push(Fragment {
                id,
                position: V3::new(
                    Fx::from_ratio(x, if id == 1 { 10 } else { 100 }),
                    Fx::from_ratio(y, 100),
                    Fx::from_ratio(485, 10),
                ),
                velocity: V3::ZERO,
                material,
                paint: None,
                mm,
                basis: IDENTITY,
                mass_g: 1,
                count: shape.count_ones(),
                shape,
                side,
                chunk: None,
                held: e.ledger.create(matter::Kind::Debris, 0, u64::MAX),
                resting: true,
            });
        }
        let hash = b.hash();
        let settled = Settled::around(&b.fragments, &[], |_, _| true);
        assert!(supported(&b.fragments[1], &w, &[], &settled, 1));
        assert_eq!(b.rubble(&w, &[], &[]).get(&[49, 48]), Some(&17));
        for tick in 0..300 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        assert_eq!(b.wakeups, 0, "a support probe skipped the thin plate below");
        assert_eq!(b.hash(), hash);
        b.fragments.remove(0);
        for tick in 300..316 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        assert!(b.wakeups > 0, "removing the plate did not wake its rider");
        assert!(b.fragments[0].position.y < Fx::from_ratio(1625, 100));
        assert!(e.balanced());
    }
    #[test]
    fn an_existing_ceiling_overlap_does_not_hold_a_piece_in_midair() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.queue_edit(Edit::put(48, 22, 48, 2));
        w.flush();
        b.fragments.push(Fragment {
            id: 1,
            position: V3::new(
                Fx::from_ratio(485, 10),
                Fx::from_ratio(21823, 1000),
                Fx::from_ratio(485, 10),
            ),
            velocity: V3::ZERO,
            material: 4,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1,
            count: 8,
            shape: 0x000f000f,
            side: 4,
            chunk: None,
            held: e.ledger.create(matter::Kind::Debris, 0, u64::MAX),
            resting: true,
        });
        for tick in 0..120 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        assert!(b.fragments[0].resting);
        assert!(
            b.fragments[0].position.y < Fx::from_int(14),
            "the ceiling was mistaken for a floor"
        );
        let stable = b.hash();
        let wakes = b.wakeups;
        for tick in 120..240 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        assert_eq!(b.hash(), stable);
        assert_eq!(b.wakeups, wakes);
    }
    #[test]
    fn escaping_an_old_overlap_still_blocks_new_solid_and_deeper_motion() {
        let mut w = world();
        w.queue_edit(Edit::put(48, 22, 48, 2));
        w.queue_edit(Edit::put(49, 22, 48, 2));
        w.flush();
        let f = Fragment {
            id: 1,
            position: V3::new(
                Fx::from_ratio(485, 10),
                Fx::from_ratio(218, 10),
                Fx::from_ratio(485, 10),
            ),
            velocity: V3::ZERO,
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1,
            count: 1,
            shape: 1,
            side: 1,
            chunk: None,
            held: 0,
            resting: false,
        };
        let (lo, hi) = f.bounds();
        let contact = |axis: usize, delta: Fx| {
            let mut p = arr(f.position);
            p[axis] = p[axis].add(delta);
            fragment_motion_contact(&f, &w, p, lo, hi, [w.nx, w.ny, w.nz])
        };
        assert!(
            !contact(1, Fx::from_ratio(-1, 10)),
            "cannot leave the ceiling"
        );
        assert!(
            contact(1, Fx::from_ratio(1, 10)),
            "moved deeper into the ceiling"
        );
        assert!(contact(0, Fx::from_ratio(1, 2)), "entered a new solid cell");
    }
    #[test]
    fn tumbling_does_not_rotate_a_piece_into_its_floor() {
        let w = world();
        let t = tuning();
        let motion = FragmentMotion::new(&t.blast);
        for id in 1..65 {
            let mut f = Fragment {
                id,
                position: V3::new(
                    Fx::from_int(48),
                    Fx::from_ratio(1401, 100),
                    Fx::from_int(48),
                ),
                velocity: V3::new(Fx::from_ratio(1, 2), Fx::ZERO, Fx::from_ratio(1, 2)),
                material: 1,
                paint: None,
                mm: 100,
                basis: IDENTITY,
                mass_g: 76800,
                count: 64,
                shape: u64::MAX,
                side: 4,
                chunk: None,
                held: 0,
                resting: false,
            };
            assert!(!fragment_contact(
                &f,
                &w,
                arr(f.position),
                f.bounds().0,
                f.bounds().1,
                [w.nx, w.ny, w.nz]
            ));
            advance_fragment(
                &mut f,
                &w,
                &t.blast,
                &motion,
                crate::gravity_per_tick(&t),
                &[],
                &Settled::default(),
                u32::MAX,
                true,
            );
            let (lo, hi) = f.bounds();
            assert!(
                !fragment_contact(&f, &w, arr(f.position), lo, hi, [w.nx, w.ny, w.nz]),
                "rotation penetrated the floor: id={id} pos={:?}",
                f.position
            );
        }
    }
    #[test]
    fn tumbling_does_not_rotate_a_piece_into_a_settled_neighbour() {
        let w = world();
        let t = tuning();
        let motion = FragmentMotion::new(&t.blast);
        for id in 1..65 {
            let mut f = Fragment {
                id,
                position: V3::new(Fx::from_int(48), Fx::from_int(20), Fx::from_int(48)),
                velocity: V3::new(Fx::from_ratio(1, 2), Fx::ZERO, Fx::from_ratio(1, 2)),
                material: 1,
                paint: None,
                mm: 100,
                basis: IDENTITY,
                mass_g: 76800,
                count: 64,
                shape: u64::MAX,
                side: 4,
                chunk: None,
                held: 0,
                resting: false,
            };
            let neighbour = Fragment {
                position: V3::new(Fx::from_int(52), f.position.y, f.position.z),
                velocity: V3::ZERO,
                resting: true,
                chunk: None,
                ..f
            };
            let input = Fragment { chunk: None, ..f };
            let settled = Settled::around(&[input, neighbour], &[], |_, _| true);
            advance_fragment(
                &mut f,
                &w,
                &t.blast,
                &motion,
                crate::gravity_per_tick(&t),
                &[],
                &settled,
                0,
                true,
            );
            assert!(f.velocity.x < Fx::ZERO, "fixture missed the neighbour");
            let (lo, hi) = f.bounds();
            let p = arr(f.position);
            assert!(
                !settled.hit(
                    [0, 1, 2].map(|a| p[a].add(lo[a])),
                    [0, 1, 2].map(|a| p[a].add(hi[a])),
                    0,
                    None
                ),
                "rotation penetrated the neighbour: id={id}"
            );
        }
    }
    /// A flying piece tumbles (ADR-309): thrown, it turns as it goes and
    /// keeps the face it landed on; laid down with no speed, it never
    /// turns. Its axes stay square to one another whatever it did.
    #[test]
    fn a_thrown_piece_tumbles_and_one_laid_down_does_not() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        let piece = |id, x: i32, velocity| Fragment {
            id,
            position: V3::new(
                Fx::from_int(x).add(Fx::from_ratio(1, 2)),
                Fx::from_int(20),
                Fx::from_int(48).add(Fx::from_ratio(1, 2)),
            ),
            velocity,
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
            resting: false,
        };
        b.fragments.push(piece(
            1,
            40,
            V3::new(Fx::from_int(2), Fx::ONE, Fx::ZERO).scale(Fx::from_ratio(
                modding::clock::STREAM_HZ as i32,
                crate::TICKS_PER_SECOND,
            )),
        ));
        b.fragments.push(piece(2, 56, V3::ZERO));
        for tick in 0..u64::from(modding::clock::HZ) {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        let thrown = &b.fragments[0];
        assert_ne!(thrown.basis, IDENTITY, "the thrown piece did not turn");
        for (i, row) in thrown.basis.iter().enumerate() {
            assert!(
                row.len_sq().sub(Fx::ONE).abs() < Fx::from_ratio(1, 100),
                "axis {i} is no longer of unit length: {:?}",
                row.len_sq()
            );
            let next = thrown.basis[(i + 1) % 3];
            let dot = row
                .x
                .mul(next.x)
                .add(row.y.mul(next.y))
                .add(row.z.mul(next.z));
            assert!(
                dot.abs() < Fx::from_ratio(1, 100),
                "axes {i} and {} are no longer square: {dot:?}",
                (i + 1) % 3
            );
        }
        // Dropped straight down with no speed of its own, it lands as it
        // was laid: falling alone is not tumbling.
        let still = &b.fragments[1];
        assert_eq!(still.basis[0], IDENTITY[0], "the still piece turned");
        // What it turned to when it stopped is what it keeps.
        let turned = thrown.basis;
        let already_resting = thrown.resting;
        for tick in u64::from(modding::clock::HZ)..u64::from(modding::clock::HZ) * 3 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        assert!(
            b.fragments[0].resting,
            "still in the air after three seconds"
        );
        if already_resting {
            assert_eq!(b.fragments[0].basis, turned, "a sleeping piece turned");
        }
        let resting_basis = b.fragments[0].basis;
        for tick in u64::from(modding::clock::HZ) * 3..u64::from(modding::clock::HZ) * 4 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        assert_eq!(
            b.fragments[0].basis, resting_basis,
            "a settled piece kept tumbling"
        );
    }
    /// A piece falls onto one that landed before it and stands on it
    /// (ADR-308) instead of sinking through it to the ground. Two
    /// materials, so that the heap of a cell (ADR-292) does not gather
    /// them into one and hide the answer.
    #[test]
    fn a_piece_stands_on_the_piece_that_landed_before_it() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        let piece = |id, material, y: i32, resting| Fragment {
            id,
            position: V3::new(
                Fx::from_int(48).add(Fx::from_ratio(1, 2)),
                Fx::from_int(y),
                Fx::from_int(48).add(Fx::from_ratio(1, 2)),
            ),
            velocity: V3::ZERO,
            material,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 9600,
            count: 8,
            shape: 0xff,
            side: 2,
            chunk: None,
            held,
            resting,
        };
        // Ground is at 12: one piece of soil already lying on it, one of
        // concrete falling from six units up.
        b.fragments.push(piece(1, 1, 13, true));
        b.fragments.push(piece(2, 2, 19, false));
        for tick in 0..240 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        assert_eq!(b.fragments.len(), 2, "the pieces merged");
        let (below, above) = (&b.fragments[0], &b.fragments[1]);
        assert!(above.resting && below.resting, "still falling");
        let (_, high) = below.bounds();
        let (low, _) = above.bounds();
        let top = below.position.y.add(high[1]);
        let bottom = above.position.y.add(low[1]);
        assert!(
            bottom >= top.sub(Fx::from_ratio(1, 2)),
            "the piece sank into the one below: bottom {bottom:?} against top {top:?}"
        );
        assert!(
            above.position.y > below.position.y,
            "the piece did not land on top: {:?} against {:?}",
            above.position.y,
            below.position.y
        );
    }
    /// **A fast piece stands on the piece below it too** (ADR-438). The
    /// settled index held only the cells beside a flying piece's box,
    /// and a piece thrown down at seven units a tick crossed the one
    /// lying below it within the tick without asking about it, and sank
    /// to the ground through it.
    #[test]
    fn a_fast_piece_stands_on_the_piece_below_it() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        let piece = |id, material, y: i32, vy: i32, resting| Fragment {
            id,
            position: V3::new(
                Fx::from_int(48).add(Fx::from_ratio(1, 2)),
                Fx::from_int(y),
                Fx::from_int(48).add(Fx::from_ratio(1, 2)),
            ),
            velocity: V3::new(Fx::ZERO, Fx::from_int(vy), Fx::ZERO),
            material,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 9600,
            count: 8,
            shape: 0xff,
            side: 2,
            chunk: None,
            held,
            resting,
        };
        b.fragments.push(piece(1, 1, 13, 0, true));
        b.fragments.push(piece(2, 2, 21, -7, false));
        for tick in 0..240 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        let (below, above) = (&b.fragments[0], &b.fragments[1]);
        assert!(above.resting && below.resting, "still falling");
        let (_, high) = below.bounds();
        let (low, _) = above.bounds();
        let top = below.position.y.add(high[1]);
        let bottom = above.position.y.add(low[1]);
        assert!(
            bottom >= top.sub(Fx::from_ratio(1, 2)),
            "the piece sank into the one below: bottom {bottom:?} against top {top:?}"
        );
    }
    #[test]
    fn overlapping_airborne_pieces_cannot_hold_each_other_up() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        for i in 0..2 {
            let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
            b.fragments.push(Fragment {
                id: i + 1,
                position: V3::new(
                    Fx::from_int(48),
                    Fx::from_int(30).add(Fx::from_ratio(i as i32, 4)),
                    Fx::from_int(48),
                ),
                velocity: V3::ZERO,
                material: i as u16 + 1,
                paint: None,
                mm: 100,
                basis: IDENTITY,
                mass_g: 9600,
                count: 8,
                shape: 0xff,
                side: 2,
                chunk: None,
                held,
                resting: true,
            });
        }
        for tick in 0..180 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                tick,
                Surroundings::NONE,
            );
        }
        assert_eq!(b.fragments.len(), 2);
        assert!(
            b.fragments
                .iter()
                .all(|f| f.resting && f.position.y < Fx::from_int(16)),
            "mutual support left pieces in mid-air"
        );
    }
    /// A piece that came to rest against a live hull's flank is no floor
    /// under that hull (ADR-302): the columns of the hull's box it reaches
    /// stay out of the map, its own columns outside stay in, and a piece
    /// the hull stands over is floor.
    #[test]
    fn a_piece_against_a_flank_is_no_floor_under_the_hull() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        // Two by two by two, its bottom on the ground at 12.
        let piece = |id, x: i32, z: i32| Fragment {
            id,
            position: V3::new(
                Fx::from_int(x).add(Fx::from_ratio(1, 2)),
                Fx::from_int(13),
                Fx::from_int(z).add(Fx::from_ratio(1, 2)),
            ),
            velocity: V3::ZERO,
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 9600,
            count: 8,
            shape: 0xff,
            side: 2,
            chunk: None,
            held,
            resting: true,
        };
        // Against the flank: its centre a column outside the box, its
        // edge reaching a column under it. Under the hull: centre inside.
        b.fragments.push(piece(1, 45, 48));
        b.fragments.push(piece(2, 50, 48));
        let hull = Hull {
            pos: [Fx::from_int(50), Fx::from_int(12), Fx::from_int(48)],
            head: 0,
            half: [Fx::from_int(4), Fx::from_int(3)],
            height: Fx::from_int(3),
            vel: [Fx::ZERO; 3],
            step: Fx::from_int(4),
            solid: None,
        };
        let free = b.rubble(&w, &[], &[]);
        assert!(
            free.contains_key(&[46, 48]),
            "no hull: every column is heap"
        );
        let map = b.rubble(&w, &[], &[hull]);
        assert!(
            !map.contains_key(&[46, 48]),
            "the flank piece lifts the hull"
        );
        assert!(
            map.contains_key(&[44, 48]),
            "the flank piece lost its own column"
        );
        assert!(
            map.contains_key(&[50, 48]),
            "the piece under the hull is no floor"
        );
    }
    /// A piece dug from under a hull at the map edge leaves the hull's box
    /// by the nearest side that keeps it on the map (ADR-428). By the
    /// nearest side it left the map, the edge held it there, and no save
    /// after it would load.
    #[test]
    fn a_piece_evicted_at_the_map_edge_stays_on_the_map() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        b.fragments.push(Fragment {
            id: 1,
            position: V3::new(Fx::from_ratio(1, 2), Fx::from_int(13), Fx::from_int(48)),
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
            held,
            resting: false,
        });
        b.next_id = 1;
        // Its box reaches past the map edge at x = 0; the piece lies a
        // unit and a half from the -x side and further from every other.
        let hull = Hull {
            pos: [Fx::from_int(2), Fx::from_int(12), Fx::from_int(48)],
            head: 0,
            half: [Fx::from_int(2), Fx::from_int(6)],
            height: Fx::from_int(3),
            vel: [Fx::ZERO; 3],
            step: Fx::ZERO,
            solid: None,
        };
        b.step_fragments(
            &w,
            &mut e,
            &t.blast,
            crate::gravity_per_tick(&t),
            0,
            Surroundings {
                hulls: &[hull],
                yielding: &[],
            },
        );
        let x = b.fragments[0].position.x;
        assert!(x > Fx::from_int(4), "the piece left by the far side: {x:?}");
        let mut bytes = Blob::new();
        b.save(&mut bytes);
        assert!(Bursts::load(&mut Scan::new(&bytes.v), Some(&w), &t.blast).is_some());
    }
    fn economy() -> Economy {
        let mats =
            modding::data::materials_from_ron(include_str!("../../../data/materials.ron")).unwrap();
        Economy::new(modding::Kinds::new(), mats, 2, &tuning())
    }
    fn world() -> voxel::World {
        voxel::World::new(96, 64, 96, 4096, Box::new(Ground))
    }
    #[test]
    fn many_flying_pieces_match_with_one_and_four_workers() {
        let w = world();
        let t = tuning();
        let run = |workers| {
            let (mut pieces, mut economy) = (Bursts::default(), economy());
            for i in 0..1200 {
                let held = economy.ledger.create(matter::Kind::Debris, 0, u64::MAX);
                pieces.fragments.push(Fragment {
                    id: i + 1,
                    position: V3::new(
                        Fx::from_int(12 + (i % 60) as i32),
                        Fx::from_int(36),
                        Fx::from_int(12 + (i / 60) as i32),
                    ),
                    velocity: V3::new(
                        Fx::from_ratio((i % 5) as i32 - 2, 4),
                        Fx::ZERO,
                        Fx::from_ratio((i % 7) as i32 - 3, 4),
                    ),
                    material: 1,
                    paint: None,
                    mm: 100,
                    basis: IDENTITY,
                    mass_g: 1,
                    count: 1,
                    shape: 1,
                    side: 1,
                    chunk: None,
                    held,
                    resting: false,
                });
            }
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            pool.install(|| {
                for tick in 0..8 {
                    pieces.step_fragments(
                        &w,
                        &mut economy,
                        &t.blast,
                        crate::gravity_per_tick(&t),
                        tick,
                        Surroundings::NONE,
                    );
                }
            });
            let mut bytes = Blob::new();
            pieces.save(&mut bytes);
            bytes.v
        };
        let expected = run(1);
        for workers in [2, 4, 8] {
            assert_eq!(expected, run(workers));
        }
    }

    #[test]
    fn cached_translation_bounds_match_direct_after_eviction_bounce_and_tumble() {
        let mut w = world();
        w.flush();
        let t = tuning().blast;
        let motion = FragmentMotion::new(&t);
        let hull = Hull {
            pos: [Fx::from_int(24), Fx::from_int(12), Fx::from_int(48)],
            head: angle::FULL / 7,
            half: [Fx::from_int(6); 2],
            height: Fx::from_int(8),
            vel: [Fx::ONE, Fx::ZERO, Fx::ZERO],
            step: Fx::ZERO,
            solid: None,
        };
        let run = |cached| {
            let mut f = Fragment {
                id: 173,
                position: V3::new(Fx::from_int(24), Fx::from_int(16), Fx::from_int(48)),
                velocity: V3::new(Fx::from_ratio(3, 2), Fx::from_ratio(-1, 3), Fx::ONE),
                material: 1,
                paint: None,
                mm: 125,
                basis: IDENTITY,
                mass_g: 1200,
                count: 8,
                shape: 255,
                side: 2,
                chunk: None,
                held: 0,
                resting: false,
            };
            let mut index = Settled::default();
            for _ in 0..90 {
                if cached {
                    let p = arr(f.position);
                    let (lo, hi) = f.bounds();
                    index.boxes = vec![(
                        [0, 1, 2].map(|a| p[a].add(lo[a])),
                        [0, 1, 2].map(|a| p[a].add(hi[a])),
                    )];
                }
                advance_fragment(
                    &mut f,
                    &w,
                    &t,
                    &motion,
                    Fx::from_ratio(1, 30),
                    &[hull],
                    &index,
                    0,
                    true,
                );
            }
            assert!(f.resting);
            assert_ne!(f.basis, IDENTITY);
            assert_ne!(f.position.x, Fx::from_int(24));
            (f.position, f.velocity, f.basis, f.resting)
        };
        assert_eq!(run(false), run(true));
    }
    #[test]
    fn simultaneous_support_losses_wake_in_piece_order_across_workers() {
        let w = world();
        let mut t = tuning();
        t.blast.max_flying = 7;
        t.blast.rest_check_ticks = 1;
        t.blast.push_pct = 0;
        let run = |workers, observed| {
            let (mut pieces, mut economy) = (Bursts::default(), economy());
            for i in 0..1200 {
                let held = economy.ledger.create(matter::Kind::Debris, 0, u64::MAX);
                pieces.fragments.push(Fragment {
                    id: i + 1,
                    position: V3::new(
                        Fx::from_int(8 + (i % 40) as i32 * 2),
                        Fx::from_int(36),
                        Fx::from_int(8 + (i / 40) as i32 * 2),
                    ),
                    velocity: V3::ZERO,
                    material: 1,
                    paint: None,
                    mm: 100,
                    basis: IDENTITY,
                    mass_g: 1,
                    count: 1,
                    shape: 1,
                    side: 1,
                    chunk: None,
                    held,
                    resting: true,
                });
            }
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            let events = std::sync::Mutex::new(Vec::new());
            pool.install(|| {
                pieces.step_fragments_observed(
                    &w,
                    &mut economy,
                    &t.blast,
                    crate::gravity_per_tick(&t),
                    0,
                    Surroundings::NONE,
                    |_| {},
                    &|slot, event| {
                        if observed {
                            events
                                .lock()
                                .unwrap()
                                .push((std::thread::current().id(), slot, event));
                        }
                    },
                );
            });
            if observed {
                let mut pending = HashMap::new();
                let mut families = BTreeSet::new();
                for (thread, slot, event) in events.into_inner().unwrap() {
                    assert_eq!(slot, 0);
                    if let Some(name) = event.strip_suffix("_begin") {
                        *pending.entry((thread, name)).or_insert(0usize) += 1;
                        families.insert(name);
                    } else {
                        let name = event.strip_suffix("_end").unwrap();
                        let count = pending
                            .get_mut(&(thread, name))
                            .expect("begin before end on same thread");
                        assert!(*count > 0);
                        *count -= 1;
                    }
                }
                assert!(pending.values().all(|&count| count == 0));
                for family in [
                    "physics_boxes",
                    "physics_queries",
                    "physics_candidates",
                    "physics_cells",
                    "physics_wake",
                    "physics_translate",
                    "physics_merge",
                ] {
                    assert!(families.contains(family), "missing {family}");
                }
            }
            let flying: Vec<_> = pieces
                .fragments
                .iter()
                .filter(|f| !f.resting)
                .map(|f| f.id)
                .collect();
            assert_eq!(flying, (1..=7).collect::<Vec<_>>());
            assert!(economy.balanced());
            let mut bytes = Blob::new();
            pieces.save(&mut bytes);
            bytes.v
        };
        let expected = run(1, false);
        for workers in [1, 2, 4, 8] {
            assert_eq!(expected, run(workers, true));
            assert_eq!(expected, run(workers, false));
        }
    }
    #[test]
    fn parallel_pile_grouping_matches_ordered_merging_and_ledger() {
        const GROUPS: usize = 256;
        fn reference(pieces: &mut Bursts, _world: &voxel::World, economy: &mut Economy, t: &Blast) {
            type PileKey = ([i32; 3], Mat, Option<[u8; 3]>, u16);
            let mut piles: HashMap<PileKey, Vec<usize>> = HashMap::new();
            let mut merged = BTreeSet::new();
            for i in 0..pieces.fragments.len() {
                let f = &pieces.fragments[i];
                let Some(geometry) = fragment_join::Geometry::of(f) else {
                    continue;
                };
                let key = (
                    arr(f.position).map(|v| v.to_int_trunc().div_euclid(t.pile_cell as i32)),
                    f.material,
                    f.paint,
                    f.mm,
                );
                let heads = piles.entry(key).or_default();
                let joined = heads.iter().find_map(|&j| {
                    Some((
                        j,
                        fragment_join::Geometry::of(&pieces.fragments[j])?.join(geometry)?,
                    ))
                });
                if let Some((j, shape)) = joined {
                    let (a, b) = pieces.fragments.split_at_mut(i);
                    let to = &mut a[j];
                    let from = &b[0];
                    economy.ledger.transfer(from.held, to.held, u64::MAX);
                    to.mass_g += from.mass_g;
                    to.count += from.count;
                    shape.apply(to);
                    merged.insert(from.held);
                } else if heads.len() < fragment_join::HEADS {
                    heads.push(i);
                }
            }
            pieces.fragments.retain(|f| !merged.contains(&f.held));
            economy.wrecks.retain(|w| !merged.contains(&w.held));
            for w in &mut economy.wrecks {
                if let Some(f) = pieces.fragments.iter().find(|f| f.held == w.held) {
                    let p = arr(f.position).map(Fx::to_int_trunc);
                    w.x = p[0];
                    w.y = p[1];
                    w.z = p[2];
                }
            }
            for held in merged {
                assert!(economy.ledger.retire_empty_debris(held));
            }
        }
        let w = world();
        let mut t = tuning();
        t.blast.max_flying = 0;
        t.blast.push_pct = 0;
        // The groups below lie one 8-unit pile cell apart.
        t.blast.pile_cell = 8;
        let build = || {
            let (mut pieces, mut economy) = (Bursts::default(), economy());
            // Interleave donors so grouping cannot use adjacency; different
            // materials, paint and scale at one position must stay separate.
            for donor in 0..4 {
                for group in 0..GROUPS {
                    let count = 1;
                    let material = 1 + (group % 2) as u16;
                    let held = economy.ledger.create(matter::Kind::Debris, 0, u64::MAX);
                    let amount = economy.per_voxel(material) * u64::from(count);
                    economy.ledger.reveal(economy.ground, amount);
                    economy.ledger.transfer(economy.ground, held, amount);
                    let position = V3::new(
                        Fx::from_int(4 + (group / 8 % 8) as i32 * 8).add(Fx::from_ratio(
                            donor as i32,
                            if group % 8 < 4 { 2 } else { 1 },
                        )),
                        Fx::from_ratio(if group < 64 { 25 } else { 65 }, 2),
                        Fx::from_int(4 + (group / 64) as i32 * 8),
                    );
                    pieces.fragments.push(Fragment {
                        id: (donor * GROUPS + group + 1) as u64,
                        position,
                        velocity: V3::ZERO,
                        material,
                        paint: Some([0, (group % 4) as u8, 0]),
                        mm: if group % 8 < 4 { 50 } else { 100 },
                        basis: IDENTITY,
                        mass_g: 1200 * u64::from(count),
                        count,
                        shape: 1,
                        side: 1,
                        chunk: None,
                        held,
                        resting: true,
                    });
                    economy.wrecks.push(Wreck {
                        held,
                        x: 0,
                        y: 0,
                        z: 0,
                    });
                }
            }
            pieces.next_id = (4 * GROUPS + 1) as u64;
            (pieces, economy)
        };
        let (mut expected, mut expected_economy) = build();
        reference(&mut expected, &w, &mut expected_economy, &t.blast);
        assert_eq!(expected.fragments.len(), GROUPS);
        let mut expected_bytes = Blob::new();
        expected.save(&mut expected_bytes);
        for workers in [1, 2, 4, 8] {
            let (mut pieces, mut economy) = build();
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            pool.install(|| {
                pieces.step_fragments(
                    &w,
                    &mut economy,
                    &t.blast,
                    crate::gravity_per_tick(&t),
                    0,
                    Surroundings::NONE,
                )
            });
            let mut bytes = Blob::new();
            pieces.save(&mut bytes);
            assert_eq!(bytes.v, expected_bytes.v, "workers={workers}");
            assert_eq!(economy.hash(), expected_economy.hash());
            assert_eq!(economy.wrecks.len(), expected_economy.wrecks.len());
            assert!(economy
                .wrecks
                .iter()
                .zip(&expected_economy.wrecks)
                .all(|(a, b)| (a.held, a.x, a.y, a.z) == (b.held, b.x, b.y, b.z)));
            assert!(economy.balanced());
        }
    }
    #[test]
    fn indexed_pruning_preserves_wreck_payloads_and_ordered_destruction() {
        use std::sync::{Arc, Mutex};
        struct Mark {
            wreck: Wreck,
            slot: u32,
            drops: Arc<Mutex<Vec<u32>>>,
        }
        impl Drop for Mark {
            fn drop(&mut self) {
                self.drops.lock().unwrap().push(self.slot);
            }
        }
        for workers in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            for len in [0, 1023, 1024, 25_000] {
                for mode in 0..5 {
                    let removed = (0..1027)
                        .flat_map(|id| match mode {
                            0 => vec![],
                            1 if id % 7 == 0 => vec![id],
                            2 => vec![id],
                            3 if id % 7 == 0 => vec![u32::MAX - id],
                            4 if id % 7 == 0 => vec![id, u32::MAX - id],
                            _ => vec![],
                        })
                        .collect();
                    let build = |drops: &Arc<Mutex<Vec<u32>>>| {
                        (0..len)
                            .map(|i| Mark {
                                wreck: Wreck {
                                    // Unsorted, repeated identities; every original
                                    // slot and its signed coordinates must survive.
                                    held: if mode == 3 || (mode == 4 && i % 2 == 0) {
                                        u32::MAX - i * 37 % 1027
                                    } else {
                                        i * 37 % 1027
                                    },
                                    x: i as i32 - 12_000,
                                    y: -(i as i32),
                                    z: (i as i32 * 17) % 1000,
                                },
                                slot: i,
                                drops: Arc::clone(drops),
                            })
                            .collect::<Vec<_>>()
                    };
                    let serial_drops = Arc::new(Mutex::new(Vec::new()));
                    let actual_drops = Arc::new(Mutex::new(Vec::new()));
                    let mut serial = build(&serial_drops);
                    let mut actual = build(&actual_drops);
                    retain_live(&mut serial, &removed, |m| m.wreck.held, false);
                    pool.install(|| retain_live(&mut actual, &removed, |m| m.wreck.held, true));
                    let surviving = |values: &[Mark]| {
                        values
                            .iter()
                            .map(|m| {
                                let w = m.wreck;
                                (m.slot, w.held, w.x, w.y, w.z)
                            })
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(
                        surviving(&actual),
                        surviving(&serial),
                        "workers={workers} len={len} mode={mode}"
                    );
                    assert_eq!(*actual_drops.lock().unwrap(), *serial_drops.lock().unwrap());
                    drop(actual);
                    drop(serial);
                    assert_eq!(*actual_drops.lock().unwrap(), *serial_drops.lock().unwrap());
                }
            }
        }
    }
    #[test]
    fn settled_candidates_keep_cell_lists_in_fragment_order_across_workers() {
        let t = tuning();
        let fragments: Vec<_> = (0..1400)
            .map(|i| Fragment {
                id: i + 1,
                position: V3::new(
                    Fx::from_int(12 + (i % 40) as i32),
                    Fx::from_int(18),
                    Fx::from_int(12 + ((i % 1200) / 40) as i32),
                ),
                velocity: V3::ZERO,
                material: 1,
                paint: None,
                mm: 100,
                basis: IDENTITY,
                mass_g: 1,
                count: 1,
                shape: 1,
                side: 1,
                chunk: None,
                held: i as u32,
                resting: i < 1200,
            })
            .collect();
        let run = |workers| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            pool.install(|| {
                let mut index = Settled::default();
                let mut queries = SettledQueries::default();
                Settled::of_into(
                    &fragments,
                    &[],
                    0,
                    &t.blast,
                    &mut index,
                    &mut queries,
                    &mut |_| {},
                    &|_, _| {},
                );
                let masks: Vec<Vec<([i32; 3], u64)>> =
                    queries.0.iter().map(SettledQueryStripe::ordered).collect();
                (index.boxes, index.cells.ordered(), masks)
            })
        };
        assert_eq!(run(1), run(4));
    }
    #[test]
    fn packed_world_box_contacts_match_the_ordered_row_reference() {
        let w = world();
        let reference = |p: [Fx; 3], low: [Fx; 3], high: [Fx; 3], bounds: [u32; 3]| {
            let lo = [0, 1, 2].map(|a| (p[a].add(low[a]).raw() >> 32) as i32);
            let hi = [0, 1, 2].map(|a| (p[a].add(high[a]).raw() >> 32) as i32);
            if lo[1] < 0
                || [0, 2]
                    .iter()
                    .any(|&a| lo[a] < 0 || hi[a] >= bounds[a] as i32)
            {
                return true;
            }
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1].min(bounds[1] as i32 - 1) {
                    for x in (lo[0]..=hi[0]).step_by(64) {
                        if w.solid_bits(
                            x as u32,
                            y as u32,
                            z as u32,
                            (hi[0] - x + 1).min(64) as u32,
                        ) != 0
                        {
                            return true;
                        }
                    }
                }
            }
            false
        };
        let mut rng = fx::rng::Rng::new(335, 12);
        for _ in 0..4000 {
            let p = [w.nx, w.ny, w.nz]
                .map(|n| Fx::from_ratio(rng.range(-100, n as i32 * 100 + 100), 100));
            let lo = [0, 1, 2].map(|_| Fx::from_ratio(rng.range(-300, 0), 100));
            let hi = [0, 1, 2].map(|_| Fx::from_ratio(rng.range(0, 300), 100));
            for bounds in [
                [w.nx, w.ny, w.nz],
                [w.nx, 0, w.nz],
                [w.nx + 8, w.ny + 8, w.nz + 8],
            ] {
                assert_eq!(
                    box_contact(&w, p, lo, hi, bounds),
                    reference(p, lo, hi, bounds)
                );
                assert_eq!(
                    box_contact(&w, p, hi, lo, bounds),
                    reference(p, hi, lo, bounds)
                );
            }
        }
    }

    #[test]
    fn compact_settled_links_preserve_full_lists_and_clear_wide_generations() {
        for workers in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            pool.install(|| {
                let selected: Vec<_> = (0..73)
                    .map(|i| (i % 7 != 0).then_some(([-2, -1, -1], [10, 2, 2])))
                    .collect();
                let mut want = SettledQueries::default();
                want.build(&[([-2, -1, -1], [10, 2, 2])], true, &|_, _| {});
                let mut expected: BTreeMap<_, Vec<_>> = BTreeMap::new();
                for (i, bounds) in selected.iter().enumerate() {
                    if let Some((lo, hi)) = bounds {
                        for x in lo[0]..=hi[0] {
                            for y in lo[1]..=hi[1] {
                                for z in lo[2]..=hi[2] {
                                    expected.entry([x, y, z]).or_default().push(i as u32);
                                }
                            }
                        }
                    }
                }
                let mut normal = SettledCells::default();
                normal.build(&selected, &want, true, &|_, _| {});
                let mut wide = SettledCells::default();
                // Real wide encoding/decoding, with a tiny derived threshold
                // instead of allocating billions of nodes for the fixture.
                wide.build_links::<3>(&selected, &want, true, &|_, _| {});
                assert!(normal.stripes.iter().all(|s| s.wide.is_empty()));
                assert!(wide.stripes.iter().any(|s| !s.wide.is_empty()));
                assert_eq!(normal.ordered(), expected);
                assert_eq!(wide.ordered(), expected);
                wide.clear();
                assert!(wide.stripes.iter().all(|s| s.wide.is_empty()));
                wide.build_links::<3>(&[Some(([0; 3], [0; 3]))], &want, false, &|_, _| {});
                assert_eq!(wide.ordered(), BTreeMap::from([([0; 3], vec![0])]));
                assert!(wide.stripes.iter().all(|s| s.wide.is_empty()));
            });
        }
    }
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn settled_link_iterator_retains_successors_above_u32_without_large_allocation() {
        for target in [
            u32::MAX as usize - 1,
            u32::MAX as usize,
            u32::MAX as usize + 17,
        ] {
            let nodes = [SettledCellNode {
                source: 37,
                next: u32::MAX,
            }];
            let wide = BTreeMap::from([(0, target)]);
            let mut list = SettledCellList {
                nodes: &nodes,
                wide: &wide,
                next: 1,
            };
            assert_eq!(list.next(), Some(37));
            assert_eq!(list.next, target + 1);
        }
        let nodes = [SettledCellNode {
            source: u32::MAX,
            next: 0,
        }];
        let wide = BTreeMap::new();
        let mut list = SettledCellList {
            nodes: &nodes,
            wide: &wide,
            next: 1,
        };
        assert_eq!(list.next(), Some(u32::MAX));
        assert_eq!(list.next(), None);
    }
    #[test]
    fn settled_rebuilds_preserve_sweeps_and_clear_empty_generations() {
        let t = tuning().blast;
        for workers in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            let mut fragments: Vec<_> = (0..1200u32)
                .map(|i| Fragment {
                    id: u64::from(i) + 1,
                    position: V3::new(
                        Fx::from_ratio((i % 29) as i32 - 18, 2),
                        Fx::from_ratio((i % 13) as i32 - 8, 2),
                        Fx::from_int((i % 17) as i32 - 10),
                    ),
                    velocity: V3::new(Fx::from_int(-3), Fx::ZERO, Fx::from_ratio(7, 2)),
                    material: 1,
                    paint: None,
                    mm: 100,
                    basis: IDENTITY,
                    mass_g: 1,
                    count: 3,
                    shape: 7,
                    side: 2,
                    chunk: None,
                    held: i,
                    resting: i % 3 != 0,
                })
                .collect();
            let mut index = Settled::default();
            let mut queries = SettledQueries::default();
            for (generation, len) in [1200, 1100, 17, 1200, 1200, 1200, 0]
                .into_iter()
                .enumerate()
            {
                for f in &mut fragments {
                    f.position.x = f.position.x.add(Fx::from_ratio(3, 2));
                    f.velocity.z = f.velocity.z.neg();
                    if generation >= 4 {
                        f.resting = generation == 4;
                    }
                }
                let fragments = &fragments[..len];
                let tick = generation as u64;
                let boxes: Vec<_> = fragments
                    .iter()
                    .map(|f| {
                        let (lo, hi) = f.bounds();
                        let p = arr(f.position);
                        (
                            [0, 1, 2].map(|a| p[a].add(lo[a])),
                            [0, 1, 2].map(|a| p[a].add(hi[a])),
                        )
                    })
                    .collect();
                let mut wanted = BTreeSet::new();
                for (f, &(lo, hi)) in fragments.iter().zip(&boxes) {
                    if generation == 4 || (f.resting && !due(f.id, tick, &t)) {
                        continue;
                    }
                    let velocity = arr(f.velocity);
                    let first = [0, 1, 2].map(|a| {
                        lo[a]
                            .add(velocity[a].min(Fx::ZERO))
                            .to_int_trunc()
                            .div_euclid(SETTLED_CELL)
                            - 1
                    });
                    let last = [0, 1, 2].map(|a| {
                        hi[a]
                            .add(velocity[a].max(Fx::ZERO))
                            .to_int_trunc()
                            .div_euclid(SETTLED_CELL)
                            + 1
                    });
                    for z in first[2]..=last[2] {
                        for y in first[1]..=last[1] {
                            for x in first[0]..=last[0] {
                                wanted.insert([x, y, z]);
                            }
                        }
                    }
                }
                let mut expected = BTreeMap::<_, Vec<u32>>::new();
                for (i, (f, &(lo, hi))) in fragments.iter().zip(&boxes).enumerate() {
                    if !f.resting {
                        continue;
                    }
                    let first = lo.map(|v| v.to_int_trunc().div_euclid(SETTLED_CELL));
                    let last = hi.map(|v| v.to_int_trunc().div_euclid(SETTLED_CELL));
                    for z in first[2]..=last[2] {
                        for y in first[1]..=last[1] {
                            for x in first[0]..=last[0] {
                                let cell = [x, y, z];
                                if wanted.contains(&cell) {
                                    expected.entry(cell).or_default().push(i as u32);
                                }
                            }
                        }
                    }
                }
                pool.install(|| {
                    Settled::around_bounds_into(
                        fragments,
                        &[],
                        |_, f| generation != 4 && (!f.resting || due(f.id, tick, &t)),
                        |_, f| f.bounds(),
                        SettledReach::Motion,
                        &mut index,
                        &mut queries,
                        &mut |_| {},
                        &|_, _| {},
                    )
                });
                for (i, fragment) in fragments.iter().enumerate() {
                    assert_eq!(
                        index.fragment_bounds(i as u32, fragment),
                        fragment.bounds(),
                        "workers={workers}, generation={generation}, fragment={i}"
                    );
                }
                assert_eq!(
                    index.cells.ordered(),
                    expected,
                    "workers={workers}, generation={generation}"
                );
                if wanted.is_empty() || !fragments.iter().any(|f| f.resting) {
                    assert!(index.boxes.is_empty() && queries.is_empty());
                } else {
                    assert_eq!(index.boxes, boxes);
                    for z in -14..=14 {
                        for y in -10..=10 {
                            for x in -14..=14 {
                                let cell = [x, y, z];
                                assert_eq!(queries.contains(cell), wanted.contains(&cell));
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn parallel_contact_stripes_match_ordered_reference_and_clear_reused_lists() {
        let selected: Vec<_> = (0..1400)
            .map(|i| {
                (i % 5 != 0).then(|| {
                    let x = i % 23 - 15;
                    let y = i % 9 - 4;
                    let z = i % 11 - 7;
                    ([x, y, z], [x + i % 12, y + 1, z + 1])
                })
            })
            .collect();
        let mut want = SettledQueries::default();
        let mut index = SettledCells::default();
        for ranges in [
            vec![([-19, -6, -8], [18, 8, 7])],
            vec![([-9, -2, -4], [0, 2, 0]), ([5, 1, 2], [15, 8, 5])],
            Vec::new(),
        ] {
            want.clear();
            want.build(&ranges, false, &|_, _| {});
            let mut expected = BTreeMap::<_, Vec<u32>>::new();
            for (i, bounds) in selected.iter().enumerate() {
                let Some((lo, hi)) = *bounds else {
                    continue;
                };
                for z in lo[2]..=hi[2] {
                    for y in lo[1]..=hi[1] {
                        for x in lo[0]..=hi[0] {
                            if ranges.iter().any(|(a, b)| {
                                (0..3).all(|axis| {
                                    [x, y, z][axis] >= a[axis] && [x, y, z][axis] <= b[axis]
                                })
                            }) {
                                expected.entry([x, y, z]).or_default().push(i as u32);
                            }
                        }
                    }
                }
            }
            for workers in [1, 2, 4, 8] {
                index.clear();
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(workers)
                    .build()
                    .unwrap();
                pool.install(|| index.build(&selected, &want, true, &|_, _| {}));
                assert_eq!(index.len(), expected.len());
                assert_eq!(index.is_empty(), expected.is_empty());
                assert_eq!(index.ordered(), expected, "workers={workers}");
                for (cell, list) in &expected {
                    assert_eq!(
                        index.get(cell).map(Iterator::collect::<Vec<_>>),
                        Some(list.clone())
                    );
                }
            }
            index.clear();
            index.build(&selected, &want, false, &|_, _| {});
            assert_eq!(index.ordered(), expected);
        }
    }
    #[test]
    fn parallel_query_stripes_match_cells_across_negative_word_boundaries_and_rebuilds() {
        let ranges: Vec<_> = (0..160)
            .map(|i| {
                let x = i % 17 - 8;
                let y = (i % 5 - 2) * 64 - 2;
                let z = i % 7 - 3;
                ([x, y, z], [x + i % 9, y + 4, z + 1])
            })
            .collect();
        let mut expected = BTreeSet::new();
        for &(lo, hi) in &ranges {
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        expected.insert([x, y, z]);
                    }
                }
            }
        }
        for workers in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            let mut queries = SettledQueries::default();
            for parallel in [false, true] {
                queries.clear();
                pool.install(|| queries.build(&ranges, parallel, &|_, _| {}));
                for z in -4..=5 {
                    for y in -132..=132 {
                        for x in -9..=17 {
                            let cell = [x, y, z];
                            assert_eq!(queries.contains(cell), expected.contains(&cell));
                        }
                    }
                }
                queries.clear();
                pool.install(|| queries.build(&[], parallel, &|_, _| {}));
                assert!(queries.is_empty());
                pool.install(|| {
                    queries.build(&[([-1, -65, 2], [-1, -65, 2])], parallel, &|_, _| {})
                });
                assert!(queries.contains([-1, -65, 2]));
                assert!(!queries.contains([-1, -64, 2]));
                assert!(!queries.contains([0, -65, 2]));
            }
        }
    }
    fn tick(b: &mut Bursts, w: &mut voxel::World, e: &mut Economy, t: &Blast) {
        b.advance(w, &e.mats, t);
        w.begin_tick();
        w.flush();
        b.confirm(w.completed(), e, t);
        assert!(e.balanced());
    }
    #[test]
    fn fracture_is_spatial_bounded_and_does_not_create_matter_or_kinetic_energy() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning().blast;
        assert!(b.enqueue(&w, [48, 13, 48], 16, 300_000, &t));
        for _ in 0..200 {
            tick(&mut b, &mut w, &mut e, &t);
            if b.pending() == 0 && b.claims() == 0 {
                break;
            }
        }
        assert_eq!(b.pending(), 0);
        assert_eq!(b.claims(), 0);
        assert!(b.removed > 0);
        assert!(b.spent_mj <= 300_000_000);
        assert!(b.motion_mj <= b.spent_mj * t.motion_pct as u64 / 100);
        let total: u64 = b.fragments.iter().map(|f| e.ledger.held(f.held)).sum();
        assert_eq!(total, b.removed * e.per_voxel(1));
        for f in &b.fragments {
            let p = arr(f.position).map(|v| v.to_int_trunc() as u32);
            assert!(!w.solid(p[0], p[1], p[2]));
            assert!(f.velocity.y > Fx::ZERO);
        }
        assert!(w.solid(48, 1, 48));
    }
    #[test]
    fn confirmed_noop_releases_claim_without_duplicating_a_removed_voxel() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning().blast;
        let p = [48, 11, 48];
        // Another command removes the same cell first, ahead of the blast claim.
        w.queue_dig(p[0], p[1], p[2]);
        b.add_claim(
            p,
            Claim {
                at: [48, 13, 48],
                motion_mj: 999,
                side: 2,
                mat: 1,
            },
            &t,
        );
        w.queue_edit(Edit::dig_if(p[0], p[1], p[2], 1));
        w.flush();
        b.confirm(w.completed(), &mut e, &t);
        assert_eq!(b.claims(), 0);
        assert!(b.fragments.is_empty());
        assert_eq!(b.removed, 0);
        assert!(e.balanced());
    }
    #[test]
    fn delayed_fracture_cannot_erase_a_new_stronger_material_for_the_old_cost() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning().blast;
        w.queue_edit(Edit::put(48, 11, 48, 3));
        b.add_claim(
            [48, 11, 48],
            Claim {
                at: [48, 13, 48],
                motion_mj: 999,
                side: 2,
                mat: 1,
            },
            &t,
        );
        w.queue_edit(Edit::dig_if(48, 11, 48, 1));
        w.flush();
        b.confirm(w.completed(), &mut e, &t);
        assert_eq!(w.material(48, 11, 48), 3);
        assert_eq!(b.claims(), 0);
        assert!(b.fragments.is_empty());
        assert!(e.balanced());
    }
    /// A box's claims hold one pool slot per connected component, not
    /// one per cell (ADR-659): a block of eight cells fills one slot and
    /// becomes one piece; a cell apart from it would be a second piece
    /// and waits for a slot.
    #[test]
    fn a_ground_component_holds_one_slot_not_one_per_cell() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let mut t = tuning().blast;
        t.small_piece_pct = 0;
        t.max_flying = 1;
        let at = [48, 13, 48];
        for z in 48..50 {
            for y in 8..10 {
                for x in 48..50 {
                    assert!(b.claim_voxel(&mut w, [x, y, z], 1, at, 10, 4, &t));
                }
            }
        }
        assert_eq!((b.claims(), b.reserved()), (8, 1));
        assert_eq!(b.fragment_room(&t), 0);
        // Same box, not touching the block: a second piece, no slot.
        let mut room = b.fragment_room(&t);
        assert_eq!(
            b.claim_voxel_checked(&mut w, [51, 8, 51], 1, at, 10, 4, &t, &mut room),
            Err(GroundClaimError::NoRoom)
        );
        w.flush();
        b.confirm(w.completed(), &mut e, &t);
        assert_eq!(b.dusted, 0);
        assert_eq!((b.claims(), b.reserved()), (0, 0));
        assert_eq!(b.fragments.len(), 1);
        assert_eq!(b.fragments[0].count, 8);
        assert_eq!(b.removed, 8);
        assert!(e.balanced());
    }
    /// A bridge cell that did not dig splits its component in two. The
    /// pool had one slot for it: one half flies, the other is pulverised
    /// like a crater's cells, its matter never entering the economy
    /// (ADR-659). Nothing is put back: no transfer outlives the tick.
    #[test]
    fn a_split_component_without_a_slot_is_pulverised() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let mut t = tuning().blast;
        t.small_piece_pct = 0;
        t.max_flying = 1;
        let at = [48, 13, 48];
        // Another command takes the middle cell ahead of the blast.
        w.queue_dig(49, 9, 48);
        for x in 48..51 {
            assert!(b.claim_voxel(&mut w, [x, 9, 48], 1, at, 10, 4, &t));
        }
        assert_eq!((b.claims(), b.reserved()), (3, 1));
        w.flush();
        b.confirm(w.completed(), &mut e, &t);
        assert_eq!(b.fragments.len(), 1);
        assert_eq!((b.dusted, b.removed), (1, 1));
        assert_eq!((b.claims(), b.reserved()), (0, 0));
        assert_eq!(w.transfers_queued(), 0);
        assert!(e.balanced());
        let solid = [48, 49, 50].iter().filter(|&&x| w.solid(x, 9, 48)).count();
        assert_eq!(solid, 0, "every confirmed cell left the world");
    }
    /// The slots a group's confirmed cells free go to its own pieces
    /// before any other group's extra piece (ADR-443, ADR-659): a group
    /// split in another box cannot take a reserved slot.
    #[test]
    fn freed_slots_pay_their_own_pieces_first() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let mut t = tuning().blast;
        t.small_piece_pct = 0;
        t.max_flying = 2;
        let at = [48, 13, 48];
        // Group in box 44..47: a line split by a cell taken first, so it
        // confirms as two pieces on one reserved slot.
        w.queue_dig(45, 9, 44);
        for x in 44..47 {
            assert!(b.claim_voxel(&mut w, [x, 9, 44], 1, at, 10, 4, &t));
        }
        // Group in box 52..55: one cell, one slot.
        assert!(b.claim_voxel(&mut w, [52, 9, 52], 1, at, 10, 4, &t));
        assert_eq!(b.reserved(), 2);
        w.flush();
        b.confirm(w.completed(), &mut e, &t);
        // Each group's own piece flies; the split group's extra piece has
        // no slot left and is pulverised.
        assert_eq!(b.fragments.len(), 2);
        assert!(b.fragments.iter().any(|f| f.position.x > Fx::from_int(50)));
        assert_eq!(b.dusted, 1);
        assert!(e.balanced());
    }
    /// Groups are derived from the claims: a save keeps each claim's
    /// material, and the load holds exactly the slots the save held.
    #[test]
    fn claim_groups_survive_a_save() {
        let (mut b, mut w) = (Bursts::default(), world());
        let mut t = tuning().blast;
        t.small_piece_pct = 0;
        for p in [[48, 9, 48], [49, 9, 48], [51, 9, 51], [60, 9, 60]] {
            assert!(b.claim_voxel(&mut w, p, 1, [48, 13, 48], 10, 4, &t));
        }
        assert_eq!(b.reserved(), 3);
        let mut bytes = Blob::new();
        b.save(&mut bytes);
        let copy = Bursts::load(&mut Scan::new(&bytes.v), Some(&w), &t).unwrap();
        assert_eq!(copy.reserved(), 3);
        assert_eq!(copy.hash(), b.hash());
    }
    /// The bit-parallel components against a plain flood fill over cells.
    #[test]
    fn components_match_a_cell_by_cell_flood_fill() {
        let flood = |mask: u64, side: u8| -> Vec<u64> {
            let s = i32::from(side);
            let n = s * s * s;
            let mut left = mask;
            let mut out = Vec::new();
            while left != 0 {
                let start = left.trailing_zeros() as i32;
                let mut part = 0u64;
                let mut stack = vec![start];
                while let Some(i) = stack.pop() {
                    if part & (1 << i) != 0 || left & (1 << i) == 0 {
                        continue;
                    }
                    part |= 1 << i;
                    let (x, y, z) = (i % s, i / s % s, i / (s * s));
                    for (dx, dy, dz) in [
                        (1, 0, 0),
                        (-1, 0, 0),
                        (0, 1, 0),
                        (0, -1, 0),
                        (0, 0, 1),
                        (0, 0, -1),
                    ] {
                        let (a, b, c) = (x + dx, y + dy, z + dz);
                        if (0..s).contains(&a) && (0..s).contains(&b) && (0..s).contains(&c) {
                            let j = a + s * b + s * s * c;
                            if j < n {
                                stack.push(j);
                            }
                        }
                    }
                }
                left &= !part;
                out.push(part);
            }
            out
        };
        let mut rng = fx::rng::Rng::new(661, 3);
        for side in 1..=4u8 {
            let n = u32::from(side).pow(3);
            for _ in 0..3000 {
                let density = 20 + rng.below(60);
                let mut mask = 0u64;
                for i in 0..n {
                    if rng.below(100) < density {
                        mask |= 1 << i;
                    }
                }
                assert_eq!(
                    shape_components(mask, side),
                    flood(mask, side),
                    "{mask:#x} side {side}"
                );
            }
        }
    }
    #[test]
    fn component_count_matches_the_component_list() {
        let mut rng = fx::rng::Rng::new(659, 1);
        for side in 1..=4u8 {
            let n = u32::from(side).pow(3);
            for _ in 0..2000 {
                let mut mask = 0u64;
                for i in 0..n {
                    if rng.below(100) < 45 {
                        mask |= 1 << i;
                    }
                }
                let parts = shape_components(mask, side);
                assert_eq!(shape_component_count(mask, side) as usize, parts.len());
                assert_eq!(parts.iter().fold(0, |a, p| a | p), mask);
                for part in parts {
                    assert_eq!(shape_components(part, side), [part]);
                }
            }
        }
    }
    #[test]
    fn different_work_slices_produce_identical_fracture_and_matter() {
        let run = |samples, edits| {
            let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
            let mut t = tuning().blast;
            t.samples_per_tick = samples;
            t.wave_pops_per_tick = edits;
            t.edits_per_tick = edits;
            b.enqueue(&w, [48, 12, 48], 4, 20_000, &t);
            for _ in 0..5000 {
                tick(&mut b, &mut w, &mut e, &t);
                if b.pending() == 0 && b.claims() == 0 {
                    break;
                }
            }
            assert_eq!(b.pending(), 0);
            assert!(b.removed > 0);
            let mut material = Vec::new();
            for z in 40..56 {
                for y in 4..20 {
                    for x in 40..56 {
                        material.push(w.material(x, y, z));
                    }
                }
            }
            (
                material,
                b.removed,
                b.spent_mj,
                b.fragments.iter().map(|f| f.mass_g).sum::<u64>(),
            )
        };
        assert_eq!(run(7, 1), run(10000, 10000));
    }
    #[test]
    fn fast_cube_cannot_tunnel_through_a_wall_one_world_voxel_thick() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        for y in 12..40 {
            for z in 20..76 {
                w.queue_edit(Edit::put(52, y, z, 2));
            }
        }
        w.flush();
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        b.fragments.push(Fragment {
            id: 1,
            position: centre([48, 17, 48]),
            velocity: V3::new(Fx::from_int(10), Fx::ZERO, Fx::ZERO),
            material: 1,
            paint: None,
            mm: 100,
            basis: IDENTITY,
            mass_g: 1200,
            count: 1,
            shape: 1,
            side: 2,
            chunk: None,
            held,
            resting: false,
        });
        for _ in 0..100 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                0,
                Surroundings::NONE,
            );
            assert!(b.fragments[0].position.x < Fx::from_int(52));
        }
    }
    #[test]
    fn grouping_keeps_every_cell_once_and_never_glues_disconnected_corners() {
        for shape in 1u64..=255 {
            let pieces = shape_components(shape, 2);
            let mut seen = 0u64;
            for piece in pieces {
                assert_eq!(seen & piece, 0);
                seen |= piece;
                assert_eq!(shape_components(piece, 2), [piece]);
                let (lo, hi) = shape_bounds(piece, 2);
                for a in 0..3 {
                    assert!(lo[a] < hi[a]);
                }
            }
            assert_eq!(seen, shape);
        }
        assert_eq!(shape_components(0b10000001, 2), [1, 128]);
    }
    /// A three-cell box groups the same way: face neighbours only, every
    /// cell once, and a row across the box is one piece (ADR-284).
    #[test]
    fn a_wider_box_groups_by_faces_and_keeps_every_cell_once() {
        // A full row along x at y = z = 0, and the far corner alone.
        let row = 0b111u64;
        let corner = 1u64 << 26;
        assert_eq!(shape_components(row | corner, 3), [row, corner]);
        // Cells 2 and 3 are x = 2, y = 0 and x = 0, y = 1: not neighbours.
        assert_eq!(shape_components(0b1100, 3), [0b100, 0b1000]);
        // A column along z: strides of nine.
        let column = 1 | (1 << 9) | (1 << 18);
        assert_eq!(shape_components(column, 3), [column]);
        let c = arr(shape_centre(column, 3));
        assert_eq!(c, [Fx::ZERO, Fx::ZERO, Fx::ONE]);
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..200 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let shape = seed & ((1u64 << 27) - 1);
            if shape == 0 {
                continue;
            }
            let mut seen = 0u64;
            for piece in shape_components(shape, 3) {
                assert_eq!(seen & piece, 0);
                seen |= piece;
                assert_eq!(shape_components(piece, 3), [piece]);
            }
            assert_eq!(seen, shape);
        }
        // Four cells a side uses every bit of the mask.
        let all = u64::MAX;
        assert_eq!(shape_components(all, 4), [all]);
        assert!(shape_bounds(all, 4).1[0] > Fx::from_int(1));
    }
    #[test]
    fn a_sealed_strong_wall_protects_soil_behind_it() {
        let mut w = world();
        let mut e = economy();
        let mut t = tuning().blast;
        e.mats
            .insert(modding::MatDef {
                id: 65,
                density: 780,
                strength: 100000,
                bend_span: 0,
                harmful: false,
                fuel: 0,
                kindling: 0,
                melt: 0,
                loose: false,
            })
            .unwrap();
        for z in 28..=68 {
            for y in 0..36 {
                w.queue_edit(Edit::put(52, y, z, 65));
            }
        }
        w.flush();
        let mut b = Bursts::default();
        t.samples_per_tick = 1_000_000;
        t.wave_pops_per_tick = 100_000;
        t.edits_per_tick = 100_000;
        b.enqueue(&w, [48, 13, 48], 16, 3_000_000, &t);
        for _ in 0..100 {
            tick(&mut b, &mut w, &mut e, &t);
        }
        assert!(b.removed > 0);
        assert_eq!(w.material(52, 11, 48), 65);
        assert_eq!(w.material(55, 11, 48), 1);
        assert_eq!(w.material(48, 11, 48), AIR);
    }
    fn targets() -> (
        crate::damage::Bodies,
        Vec<Option<[crate::damage::Frame; 2]>>,
    ) {
        let mut body = voxel::UnitBody::new_scaled(3, 3, 3, 1, 100);
        for z in 0..3 {
            for y in 0..3 {
                for x in 0..3 {
                    body.set(x, y, z, voxel::Part::Armour);
                }
            }
        }
        body.set(1, 1, 1, voxel::Part::Core);
        assert!(body.seat_core());
        let frame =
            |p: [i32; 3]| crate::damage::Frame::new(&body, v3(p.map(Fx::from_int)), 0, V3::UP);
        let frames = vec![
            Some([frame([45, 12, 53]); 2]),
            Some([frame([56, 12, 48]); 2]),
        ];
        let mut bodies = crate::damage::Bodies::new(vec![body]);
        bodies.spawn_at(0, 0);
        bodies.spawn_at(1, 0);
        (bodies, frames)
    }
    #[test]
    fn the_same_wave_damages_an_exposed_body_but_is_stopped_by_its_cover() {
        let run = |wall: bool| {
            let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
            let (mut bodies, frames) = targets();
            let mut t = tuning();
            t.impact.work_kj_m3 = [20_000; 5];
            e.mats
                .insert(modding::MatDef {
                    id: 65,
                    density: 780,
                    strength: 100000,
                    bend_span: 0,
                    harmful: false,
                    fuel: 0,
                    kindling: 0,
                    melt: 0,
                    loose: false,
                })
                .unwrap();
            if wall {
                for z in 28..=68 {
                    for y in 0..40 {
                        w.queue_edit(Edit::put(52, y, z, 65));
                    }
                }
                w.flush();
            }
            assert!(b.enqueue(&w, [48, 15, 48], 16, 3_000_000, &t.blast));
            b.capture_last(&bodies, &frames, &t.blast);
            for _ in 0..2000 {
                b.advance_material(&mut w, &e.mats, Some(&mut bodies), &t);
                w.begin_tick();
                w.flush();
                b.confirm(w.completed(), &mut e, &t.blast);
                bodies.step(
                    &t.impact.work_kj_m3,
                    t.impact.cells_per_tick,
                    t.impact.jobs_per_tick,
                );
                if b.pending() == 0 && b.claims() == 0 && bodies.pending.is_empty() {
                    break;
                }
            }
            assert_eq!(b.pending(), 0);
            assert!(b.spent_mj <= 3_000_000_000);
            assert!(b.removed > 0);
            assert!(e.balanced());
            [
                bodies.state(0).unwrap().removed.len(),
                bodies.state(1).unwrap().removed.len(),
            ]
        };
        let covered = run(true);
        let open = run(false);
        assert!(covered[0] > 0, "exposed unit was untouched: {covered:?}");
        assert_eq!(covered[1], 0, "wave crossed sealed cover");
        assert!(
            open[0] > 0 && open[1] > 0,
            "without cover both bodies must be reached: {open:?}"
        );
    }
    /// A blast refused for want of room has no job; a late capture must
    /// leave the last job alone, whose wave may already hold its
    /// resistance buffer (ADR-287).
    #[test]
    fn a_refused_blast_does_not_recapture_the_last_job() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let (mut bodies, frames) = targets();
        let mut t = tuning();
        t.impact.work_kj_m3 = [20_000; 5];
        t.blast.max_pending = 2;
        // Both jobs sample within the first tick; their waves then take
        // many ticks, so the queue stays full.
        t.blast.samples_per_tick = 200_000;
        t.blast.wave_pops_per_tick = 500;
        assert!(b.enqueue(&w, [48, 15, 48], 16, 3_000_000_000, &t.blast));
        b.capture_last(&bodies, &frames, &t.blast);
        assert!(b.enqueue(&w, [40, 15, 48], 16, 3_000_000_000, &t.blast));
        let tick = |b: &mut Bursts,
                    w: &mut voxel::World,
                    bodies: &mut crate::damage::Bodies,
                    e: &mut Economy| {
            b.advance_material(w, &e.mats, Some(bodies), &t);
            w.begin_tick();
            w.flush();
            b.confirm(w.completed(), e, &t.blast);
            bodies.step(
                &t.impact.work_kj_m3,
                t.impact.cells_per_tick,
                t.impact.jobs_per_tick,
            );
        };
        tick(&mut b, &mut w, &mut bodies, &mut e);
        // Both jobs are still at work, the last with its wave holding the
        // resistance buffer; the queue is full.
        assert_eq!(b.pending(), 2);
        // Refused for want of room; the old caller captured anyway.
        assert!(!b.enqueue(&w, [56, 15, 48], 16, 3_000_000_000, &t.blast));
        b.capture_last(&bodies, &frames, &t.blast);
        for _ in 0..2000 {
            tick(&mut b, &mut w, &mut bodies, &mut e);
            if b.pending() == 0 && b.claims() == 0 && bodies.pending.is_empty() {
                break;
            }
        }
        assert_eq!(b.pending(), 0);
        assert!(e.balanced());
    }
    #[test]
    fn unit_wave_build_and_damage_cursors_resume_exactly() {
        let (mut b, mut w, e) = (Bursts::default(), world(), economy());
        let (mut bodies, frames) = targets();
        let mut t = tuning();
        t.blast.samples_per_tick = 4000;
        t.blast.edits_per_tick = 31;
        t.impact.work_kj_m3 = [20_000; 5];
        b.enqueue(&w, [48, 15, 48], 16, 3_000_000, &t.blast);
        b.capture_last(&bodies, &frames, &t.blast);
        for cut in [1, 15, 80, 130] {
            for _ in 0..cut {
                b.advance_material(&mut w, &e.mats, Some(&mut bodies), &t);
            }
            let mut bytes = Blob::new();
            b.save(&mut bytes);
            let mut back = Bursts::load_material(
                &mut Scan::new(&bytes.v),
                Some(&w),
                &t.blast,
                Some(&bodies),
                2,
                false,
                true,
            )
            .unwrap();
            assert_eq!(b.hash(), back.hash());
            let mut saved = Blob::new();
            bodies.save(&mut saved);
            let (mut other, _) = targets();
            assert!(other.load(&mut Scan::new(&saved.v), 2, t.impact.max_pending));
            for _ in 0..7 {
                b.advance_material(&mut w, &e.mats, Some(&mut bodies), &t);
                back.advance_material(&mut w, &e.mats, Some(&mut other), &t);
                assert_eq!(b.hash(), back.hash());
                assert_eq!(bodies.hash(), other.hash());
            }
        }
    }

    #[test]
    fn partial_build_wave_and_claims_resume_after_serialization() {
        for cut in [1, 5, 40, 80] {
            let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
            let mut t = tuning().blast;
            t.samples_per_tick = 3000;
            t.wave_pops_per_tick = 100;
            t.edits_per_tick = 13;
            b.enqueue(&w, [48, 13, 48], 8, 60_000, &t);
            for _ in 0..cut {
                tick(&mut b, &mut w, &mut e, &t);
            }
            let mut bytes = Blob::new();
            b.save(&mut bytes);
            let mut back = Bursts::load(&mut Scan::new(&bytes.v), Some(&w), &t).unwrap();
            assert_eq!(b.hash(), back.hash());
            for end in [0, 31, bytes.v.len() - 1] {
                assert!(Bursts::load(&mut Scan::new(&bytes.v[..end]), Some(&w), &t).is_none());
            }
            // Advance the independent pure field builders against identical terrain.
            // Claims/fragments remain pending here so neither side edits shared inputs.
            let mut wb = world();
            let mut saved = Blob::new();
            w.save(&mut saved);
            assert!(wb.load(&mut Scan::new(&saved.v)));
            for _ in 0..10 {
                b.advance(&mut w, &e.mats, &t);
                back.advance(&mut wb, &e.mats, &t);
                assert_eq!(b.hash(), back.hash());
            }
        }
    }
    #[test]
    fn all_flying_voxels_land_merge_locally_and_remain_reclaimable() {
        let (mut b, mut w, mut e) = (Bursts::default(), world(), economy());
        let t = tuning();
        b.enqueue(&w, [48, 13, 48], 8, 60_000, &t.blast);
        for _ in 0..100 {
            tick(&mut b, &mut w, &mut e, &t.blast);
        }
        let n = b.removed;
        for _ in 0..800 {
            b.step_fragments(
                &w,
                &mut e,
                &t.blast,
                crate::gravity_per_tick(&t),
                0,
                Surroundings::NONE,
            );
        }
        assert!(n > 0);
        assert!(b.fragments.iter().all(|f| f.resting));
        assert_eq!(b.fragments.iter().map(|f| f.count as u64).sum::<u64>(), n);
        assert_eq!(e.wrecks.len(), b.fragments.len());
        assert!(e.balanced());
        let tank = e.ledger.create(matter::Kind::Hauler, 0, u64::MAX);
        for f in &b.fragments {
            e.ledger.transfer(f.held, tank, u64::MAX);
        }
        b.sweep(&e);
        assert!(b.fragments.is_empty());
        assert_eq!(e.ledger.held(tank), n * e.per_voxel(1));
        assert!(e.balanced());
    }
}

#[derive(Clone, Copy)]
struct Claim {
    at: [i32; 3],
    motion_mj: u64,
    /// Cells per side of the box this cell's piece is grouped in (ADR-284).
    side: u8,
    /// The material the dig expects; the piece is made of it.
    mat: Mat,
}

/// The pieces one box, material and blast make share their box; a claim
/// belongs to the open group of exactly these.
type ClaimGroup = ([u32; 3], Mat, [i32; 3], u8);

/// A live hull a piece can land on (ADR-293): its footprint box about
/// `pos` turned by `head`, standing `height` above `pos.y`; `vel` is how
/// far it moved this tick, and a moving hull bulldozes the pieces it
/// drives into (ADR-294).
#[derive(Clone, Copy, Debug)]
pub struct Hull<'a> {
    pub pos: [Fx; 3],
    pub head: i32,
    pub half: [Fx; 2],
    pub height: Fx,
    pub vel: [Fx; 3],
    /// The rise the hull climbs rather than pushes: a piece no taller
    /// than this above the hull's bottom is floor to it (ADR-292).
    pub step: Fx,
    /// The hull's own voxels, as it stands now — turned turret, lost
    /// cells and segments with it (ADR-469). A piece meets the hull only
    /// where it meets them; without them the box is the hull.
    pub solid: Option<Solid<'a>>,
}
/// A hull's voxels as world cells (ADR-469).
pub trait Voxels: Sync {
    /// Whether the world cell `p` lies in the hull's voxels.
    fn solid(&self, p: [i32; 3]) -> bool;
}
impl<F: Fn([i32; 3]) -> bool + Sync> Voxels for F {
    fn solid(&self, p: [i32; 3]) -> bool {
        self(p)
    }
}
/// A hull's voxels for the pieces (ADR-469).
#[derive(Clone, Copy)]
pub struct Solid<'a>(pub &'a dyn Voxels);
impl std::fmt::Debug for Solid<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Solid")
    }
}
/// The most cells a piece's box asks of a hull's voxels, wherever it
/// lies; a larger piece — a whole section — meets a hull by its box
/// (ADR-469).
const SOLID_SAMPLES: i64 = 64;
/// The most cells of the box a piece sweeps in a tick asked of a hull's
/// voxels at once; past it, each substep asks its own (ADR-469).
const SWEPT_SAMPLES: i64 = 512;
/// Whether a piece's box `low..high` meets hulls by their voxels
/// (ADR-469): nowhere does it cover more than `SOLID_SAMPLES` cells —
/// `floor(extent) + 2` along each axis at most.
fn by_voxels(low: [Fx; 3], high: [Fx; 3]) -> bool {
    (0..3).fold(1i64, |n, a| {
        n * (high[a].sub(low[a]).to_int_trunc() as i64 + 2)
    }) <= SOLID_SAMPLES
}
impl Hull<'_> {
    /// Whether the world box `lo..hi` shares a cell with the hull's voxels
    /// (ADR-469): the cells it covers, floor of each corner, as the
    /// world's are asked. Without the voxels, or past `cap` cells, it does.
    fn meets(&self, lo: [Fx; 3], hi: [Fx; 3], cap: i64) -> bool {
        let Some(Solid(voxels)) = self.solid else {
            return true;
        };
        let lo = lo.map(|v| (v.raw() >> 32) as i32);
        let hi = hi.map(|v| (v.raw() >> 32) as i32);
        let cells = (0..3).fold(1i64, |n, a| n * i64::from(hi[a] - lo[a] + 1));
        if cells > cap {
            return true;
        }
        (lo[1]..=hi[1])
            .any(|y| (lo[2]..=hi[2]).any(|z| (lo[0]..=hi[0]).any(|x| voxels.solid([x, y, z]))))
    }
    /// Whether a piece's box at `p`, lying within the hull's box, meets
    /// the hull (ADR-469): its voxels, or its box for a piece too large
    /// to ask them.
    fn meets_piece(&self, p: [Fx; 3], low: [Fx; 3], high: [Fx; 3]) -> bool {
        !by_voxels(low, high)
            || self.meets(
                std::array::from_fn(|a| p[a].add(low[a])),
                std::array::from_fn(|a| p[a].add(high[a])),
                i64::MAX,
            )
    }
    /// Whether a point of the ground plane lies under the hull's box.
    pub fn covers_point(&self, p: [Fx; 2]) -> bool {
        let (dx, dz) = (p[0].sub(self.pos[0]), p[1].sub(self.pos[2]));
        // The turned box lies within `half[0] + half[1]` of its centre
        // along both world axes: most points are farther.
        let round = self.half[0].add(self.half[1]);
        if dx.abs() > round || dz.abs() > round {
            return false;
        }
        let (c, s) = (fx::angle::cos(self.head), fx::angle::sin(self.head));
        let u = dx.mul(c).add(dz.mul(s));
        let v = dz.mul(c).sub(dx.mul(s));
        u.abs() <= self.half[0] && v.abs() <= self.half[1]
    }
    /// The world box around the hull's footprint, a unit wider than the
    /// turned rectangle for the rounding of its frame (ADR-465): no point
    /// outside it lies under the hull.
    pub fn footprint_box(&self) -> ([Fx; 2], [Fx; 2]) {
        let (c, s) = (
            fx::angle::cos(self.head).abs(),
            fx::angle::sin(self.head).abs(),
        );
        let [a, b] = self.half;
        let extent = [a.mul(c).add(b.mul(s)), a.mul(s).add(b.mul(c))].map(|e| e.add(Fx::ONE));
        (
            [self.pos[0].sub(extent[0]), self.pos[2].sub(extent[1])],
            [self.pos[0].add(extent[0]), self.pos[2].add(extent[1])],
        )
    }
    /// The top of the hull's lowest voxel in the world column under
    /// `centre` (ADR-469); the box's top without the voxels. `None` where
    /// the centre is not over the box or the column holds no voxel below
    /// the optional roof ceiling. A non-voxel hull retains its box roof.
    fn lowest(&self, centre: V3, ceiling: Option<Fx>) -> Option<Fx> {
        if !self.covers_point([centre.x, centre.z]) {
            return None;
        }
        let top = self.pos[1].add(self.height);
        let Some(Solid(voxels)) = self.solid else {
            return Some(top);
        };
        let [x, z] = [centre.x, centre.z].map(|v| (v.raw() >> 32) as i32);
        let base = (self.pos[1].raw() >> 32) as i32;
        let end = -(top.neg().raw() >> 32) as i32;
        // Voxel y has roof y + 1. A support query cannot use any roof
        // above its reach, so stop before floor(reach), even for negative
        // or fractional heights. Shelter queries still scan the whole hull.
        let end = ceiling.map_or(end, |reach| end.min((reach.raw() >> 32) as i32));
        (base..end)
            .find(|&y| voxels.solid([x, y, z]))
            .map(|y| Fx::from_int(y + 1))
    }
    /// Whether a piece whose box bottom is `bottom` and centre `centre`
    /// rides on the hull (ADR-293, ADR-458): its centre over the box and
    /// its bottom no lower than the support probe's half unit under the
    /// roof — on the roof, or on what lies on it. The roof is the hull's
    /// voxels in the column under the centre (ADR-469): a piece on the
    /// rear deck rides there, and one under the belly does not.
    pub fn carries(&self, bottom: Fx, centre: V3) -> bool {
        let reach = bottom.add(Fx::from_ratio(1, 2));
        // `lowest` starts at floor(pos.y); the first possible voxel roof is
        // that integer + 1. A lower support probe cannot ride this hull,
        // regardless of heading, holes or its column's actual roof. Reject
        // before footprint/trigonometry/voxel queries in parallel debris jobs.
        if self.solid.is_some()
            && (reach.raw() >> 32) < i64::from((self.pos[1].raw() >> 32) as i32) + 1
        {
            return false;
        }
        self.lowest(centre, Some(reach))
            .is_some_and(|roof| roof <= reach)
    }
    /// Whether a piece whose box bottom is `bottom` and centre `centre`
    /// lies under the hull (ADR-302, ADR-469): its centre over the box
    /// and the hull's voxels in that column all above it — under the
    /// belly, or dug from under it.
    fn shelters(&self, bottom: Fx, centre: V3) -> bool {
        let reach = bottom.add(Fx::from_ratio(1, 2));
        self.lowest(centre, None).is_some_and(|roof| roof > reach)
    }
    /// Whether the column's centre lies under the hull's box.
    pub fn covers(&self, x: i32, z: i32) -> bool {
        self.covers_point([
            Fx::from_int(x).add(Fx::from_ratio(1, 2)),
            Fx::from_int(z).add(Fx::from_ratio(1, 2)),
        ])
    }
}

/// What the pieces meet besides the world this tick: the live hulls
/// (ADR-293) and the materials a hull crushes rather than climbs
/// (ADR-294). The batch has neither.
#[derive(Clone, Copy)]
pub struct Surroundings<'a> {
    pub hulls: &'a [Hull<'a>],
    pub yielding: &'a [Mat],
}
impl Surroundings<'static> {
    pub const NONE: Self = Self {
        hulls: &[],
        yielding: &[],
    };
}

/// A real removed world voxel, or a settled local pile of them. Coordinates and
/// velocity are in world units and units/tick. Matter stays in `held` until haul.
pub struct Fragment {
    pub id: u64,
    pub position: V3,
    pub velocity: V3,
    pub material: Mat,
    /// Owner, finish code and subsystem role for an actual piece of a unit.
    pub paint: Option<[u8; 3]>,
    pub mm: u16,
    pub basis: [V3; 3],
    pub mass_g: u64,
    pub count: u32,
    /// Occupied cells of the piece's box, bit `x + side*y + side*side*z`
    /// (ADR-284).
    pub shape: u64,
    /// Cells per side of that box, `1..=4`.
    pub side: u8,
    // Ordinary debris does not pay for the large section payload inline.
    // The box changes storage only; chunk geometry/serialization are identical.
    pub chunk: Option<Box<chunk::Chunk>>,
    pub held: u32,
    pub resting: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GroundClaimError {
    NoRoom,
    ClaimLimit,
    PendingCell,
    Outside,
    Frame,
}

type RestingPile = ([i32; 3], Mat, Option<[u8; 3]>, u16);
#[derive(Default)]
pub struct Bursts {
    join_memo: fragment_join_memo::Memo,
    join_shards: [Vec<(RestingPile, usize)>; 8],
    jobs: VecDeque<Job>,
    claims: BTreeMap<[u32; 3], Claim>,
    /// Open claims of each group as a box mask. Each connected component
    /// of a mask holds one slot of the fragment pool, since it becomes
    /// at most one piece (ADR-659). Derived from `claims`; not saved.
    groups: BTreeMap<ClaimGroup, u64>,
    /// Slots the open claims hold: components summed over the groups.
    reserved: usize,
    /// Reused only within a tick; neither cache affects save or ordering.
    settled_scratch: Settled,
    query_scratch: SettledQueries,
    /// Query ranges prepared by the last fragment phase; diagnostic only.
    pub settled_queries: usize,
    pub fragments: Vec<Fragment>,
    pub corpses: BTreeMap<u32, Fragment>,
    pub(crate) next_id: u64,
    pub removed: u64,
    pub spent_mj: u64,
    pub motion_mj: u64,
    pub body_motion_mj: u64,
    pub body_removed: u64,
    /// Diagnostic only: actual resting-to-flying transitions; not saved or hashed.
    pub wakeups: u64,
    /// Last motion mask's zero-word/other-word entries; not saved or hashed.
    pub settled_query_words: [usize; 2],
    /// Diagnostic only: confirmed cells pulverised for want of a slot
    /// (ADR-659); not saved or hashed.
    pub dusted: u64,
}

pub const IDENTITY: [V3; 3] = [
    V3::new(Fx::ONE, Fx::ZERO, Fx::ZERO),
    V3::UP,
    V3::new(Fx::ZERO, Fx::ZERO, Fx::ONE),
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FragmentPools {
    pub working: usize,
    pub parked: usize,
    pub flying: usize,
}
impl Fragment {
    fn parkable(&self) -> bool {
        self.resting && self.velocity == V3::ZERO && self.chunk.is_none()
    }
    /// Keep the centre over the map (ADR-428). A save refuses a piece
    /// whose centre lies off it, and one would get there: a hull at the
    /// map edge hangs its cells over it. The box may still cross the edge;
    /// the edge holds the piece where it lands.
    pub(crate) fn keep_on_map(&mut self, columns: [u32; 2]) {
        let last = |n: u32| Fx::from_int(n as i32).sub(Fx(1));
        self.position.x = self.position.x.clamp(Fx::ZERO, last(columns[0]));
        self.position.z = self.position.z.clamp(Fx::ZERO, last(columns[1]));
        self.position.y = self.position.y.max(Fx::ZERO);
    }
    /// The piece's box in its own turn, around its centre.
    pub fn bounds(&self) -> ([Fx; 3], [Fx; 3]) {
        // A section's box is already in world axes about its centre, as its
        // captured frames placed its cells; its basis is the turn since it
        // left, and the box turns with it (ADR-468). On the identity basis
        // a section starts with, the sums below give the box unchanged.
        let (lo, hi) = match &self.chunk {
            Some(chunk) => chunk.bounds,
            None => {
                let (lo, hi) = shape_bounds(self.shape, self.side);
                if self.mm == voxel::unit::WORLD_MM {
                    (lo, hi)
                } else {
                    let scale = Fx::from_ratio(self.mm as i32, voxel::unit::WORLD_MM as i32);
                    (lo.map(|v| v.mul(scale)), hi.map(|v| v.mul(scale)))
                }
            }
        };
        if self.basis == IDENTITY {
            // Multiplication by zero/one and addition of zero are exact in
            // Fx. Retain endpoint ordering for empty shapes or unusual
            // captured bounds, just as the general reduction below does.
            return (
                [0, 1, 2].map(|a| lo[a].min(hi[a])),
                [0, 1, 2].map(|a| lo[a].max(hi[a])),
            );
        }
        let mut low = [Fx::ZERO; 3];
        let mut high = [Fx::ZERO; 3];
        // Each corner sum chooses one endpoint independently per local
        // axis. Reduce the already rounded endpoint products, then add in
        // the original axis order: no centre/extent formula with different
        // Q32 rounding, and no eight-corner transform.
        for axis in 0..3 {
            let a = arr(self.basis[axis].scale(lo[axis]));
            let b = arr(self.basis[axis].scale(hi[axis]));
            for component in 0..3 {
                low[component] = low[component].add(a[component].min(b[component]));
                high[component] = high[component].add(a[component].max(b[component]));
            }
        }
        (low, high)
    }
}
fn mix(h: &mut u64, v: u64) {
    *h = h.wrapping_mul(0x100000001b3).wrapping_add(v);
}
fn ijk(i: usize, n: usize) -> [i32; 3] {
    [(i % n) as i32, (i / n % n) as i32, (i / (n * n)) as i32]
}
fn arr(v: V3) -> [Fx; 3] {
    [v.x, v.y, v.z]
}
fn v3(a: [Fx; 3]) -> V3 {
    V3::new(a[0], a[1], a[2])
}
/// A coarse box chooses either its full extent or half-sized sub-boxes.
/// All cells agree on that partition; no overlapping or disconnected solids
/// are invented. Seeded locally, so replay needs no new RNG state.
pub(crate) fn fracture_box(p: [u32; 3], max_side: u8, t: &Blast) -> ([u32; 3], u8) {
    let max_side = u32::from(max_side.clamp(1, 4));
    let tile = p.map(|v| v / max_side);
    let mut seed = 0;
    for v in tile {
        mix(&mut seed, u64::from(v));
    }
    let mut rng = fx::rng::Rng::new(seed, 475);
    let side = if rng.below(100) < t.small_piece_pct {
        (max_side / 2).max(1)
    } else {
        max_side
    };
    let base = tile.map(|v| v * max_side);
    (
        std::array::from_fn(|a| base[a] + (p[a] - base[a]) / side * side),
        side as u8,
    )
}

/// Disorder redirects the finite impulse and only reduces its speed.
fn scattered_launch(id: u64, direction: V3, speed: Fx, t: &Blast) -> V3 {
    let span = direction
        .x
        .abs()
        .max(direction.y.abs())
        .max(direction.z.abs());
    if span == Fx::ZERO || speed == Fx::ZERO {
        return V3::ZERO;
    }
    let mut rng = fx::rng::Rng::new(id, 476);
    let spread = t.scatter_pct as i32;
    let jitter = v3(std::array::from_fn(|_| {
        span.mul(Fx::from_ratio(rng.range(-spread, spread), 100))
    }));
    let mut direction = direction.add(jitter);
    if direction.len_sq() == Fx::ZERO {
        direction = V3::UP;
    }
    let speed = speed.mul(Fx::from_ratio(rng.range(t.speed_min_pct as i32, 100), 100));
    direction.scale(speed.div(direction.len_sq().sqrt()))
}

/// Cells of a piece's box.
fn box_cells(side: u8) -> u32 {
    u32::from(side.clamp(1, 4)).pow(3)
}
/// The cell a bit of a piece's mask names, in box coordinates.
fn shape_cell(i: u32, side: u8) -> [i32; 3] {
    let s = i32::from(side.clamp(1, 4));
    let i = i as i32;
    [i % s, (i / s) % s, i / (s * s)]
}
/// Bit masks of the four possible coordinate planes of each shape axis.
/// Generated at compile time for all supported box sizes, including 3³.
const SHAPE_PLANES: [[[u64; 4]; 3]; 4] = {
    let mut sizes = [[[0; 4]; 3]; 4];
    let mut s = 1;
    while s <= 4 {
        let mut i = 0;
        while i < s * s * s {
            sizes[s - 1][0][i % s] |= 1 << i;
            sizes[s - 1][1][i / s % s] |= 1 << i;
            sizes[s - 1][2][i / (s * s)] |= 1 << i;
            i += 1;
        }
        s += 1;
    }
    sizes
};
fn shape_sums(shape: u64, planes: &[[u64; 4]; 3]) -> [i32; 3] {
    planes.map(|p| {
        (shape & p[1]).count_ones() as i32
            + 2 * (shape & p[2]).count_ones() as i32
            + 3 * (shape & p[3]).count_ones() as i32
    })
}
/// Centre of mass of a piece in its box, cell units.
pub fn shape_centre(shape: u64, side: u8) -> V3 {
    if let Some(geometry) = shape_geometry::known(shape, side) {
        return v3(geometry.centre);
    }
    let planes = &SHAPE_PLANES[usize::from(side.clamp(1, 4)) - 1];
    let sum = shape_sums(shape, planes);
    v3(sum.map(|v| Fx::from_ratio(v, shape.count_ones().max(1) as i32)))
}
/// Face-connected components of a mask, each a piece of its own.
pub(crate) fn shape_components(mut left: u64, side: u8) -> Vec<u64> {
    let mut out = Vec::new();
    while left != 0 {
        let component = shape_component_at(left, side);
        left &= !component;
        out.push(component);
    }
    out
}
/// How many pieces [`shape_components`] makes of a mask, without the list.
pub(crate) fn shape_component_count(mut left: u64, side: u8) -> u32 {
    let mut n = 0;
    while left != 0 {
        left &= !shape_component_at(left, side);
        n += 1;
    }
    n
}
/// Change in component count when one cell joins a group. Only old components
/// touching that cell can merge: 1 minus their count. Unrelated components
/// never need visiting, and a flood stops once it connects all remaining
/// neighbours. No allocation or persistent connectivity cache is needed.
fn shape_added_components(mask: u64, bit: u64, side: u8) -> i64 {
    debug_assert!(bit.is_power_of_two());
    if mask & bit != 0 {
        return 0;
    }
    let mut neighbours = shape_grow(bit, side) & mask;
    let mut delta = 1;
    while neighbours != 0 {
        delta -= 1;
        let mut component = 1u64 << neighbours.trailing_zeros();
        loop {
            neighbours &= !component;
            if neighbours == 0 {
                return delta;
            }
            let next = shape_grow(component, side) & mask;
            if next == component {
                break;
            }
            component = next;
        }
    }
    delta
}
/// The component of `left` holding its lowest cell.
fn shape_component_at(left: u64, side: u8) -> u64 {
    let mut component = 1u64 << left.trailing_zeros();
    loop {
        let next = shape_grow(component, side) & left;
        if next == component {
            return component;
        }
        component = next;
    }
}
/// `m` and every cell sharing a face with it, inside the box.
fn shape_grow(m: u64, side: u8) -> u64 {
    let s = usize::from(side.clamp(1, 4));
    let mut out = m;
    let mut stride = 1;
    for axis in &SHAPE_PLANES[s - 1] {
        out |= (m & !axis[s - 1]) << stride;
        out |= (m & !axis[0]) >> stride;
        stride *= s;
    }
    out
}
fn shape_bounds(shape: u64, side: u8) -> ([Fx; 3], [Fx; 3]) {
    if let Some(geometry) = shape_geometry::known(shape, side) {
        return geometry.bounds;
    }
    let side = side.clamp(1, 4);
    let planes = &SHAPE_PLANES[usize::from(side) - 1];
    let sum = shape_sums(shape, planes);
    let lo = planes.map(|p| {
        p.iter()
            .position(|mask| shape & mask != 0)
            .unwrap_or(usize::from(side) - 1) as i32
    });
    let hi = planes.map(|p| p.iter().rposition(|mask| shape & mask != 0).unwrap_or(0) as i32);
    let centre = sum.map(|v| Fx::from_ratio(v, shape.count_ones().max(1) as i32));
    (
        std::array::from_fn(|a| {
            Fx::from_int(lo[a])
                .sub(centre[a])
                .sub(Fx::from_ratio(49, 100))
        }),
        std::array::from_fn(|a| {
            Fx::from_int(hi[a])
                .sub(centre[a])
                .add(Fx::from_ratio(49, 100))
        }),
    )
}
fn centre(p: [u32; 3]) -> V3 {
    v3(p.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))))
}
fn save_v(b: &mut Blob, v: V3) {
    for x in arr(v) {
        b.i64(x.raw());
    }
}
fn load_v(s: &mut Scan) -> V3 {
    v3(std::array::from_fn(|_| Fx(s.i64())))
}

impl Job {
    fn sample(&self, i: usize) -> ([i32; 3], usize) {
        let c3 = (self.cell * self.cell * self.cell) as usize;
        let coarse = i / c3;
        let a = ijk(coarse, self.n);
        let b = ijk(i % c3, self.cell as usize);
        (
            std::array::from_fn(|k| self.origin[k] + a[k] * self.cell + b[k]),
            coarse,
        )
    }
    fn samples(&self) -> usize {
        self.n.pow(3) * (self.cell as usize).pow(3)
    }
}

impl Bursts {
    /// Reserve one conditional terrain edit for an external spatial envelope.
    /// The caller pays fracture work; confirmation remains the only source of
    /// terrain fragments and uses the same bounded claim pool as ordinary combat.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn claim_voxel(
        &mut self,
        world: &mut voxel::World,
        p: [u32; 3],
        mat: Mat,
        at: [i32; 3],
        motion_mj: u64,
        side: u8,
        t: &Blast,
    ) -> bool {
        let mut room = self.fragment_room(t);
        self.claim_voxel_with_room(world, p, mat, at, motion_mj, side, t, &mut room)
    }

    /// Ordered claim-only batch: fragments must remain unchanged while room
    /// is consumed. A claim that starts a new component of its group takes
    /// one slot; one that joins or bridges components takes none and gives
    /// back what the bridge frees (ADR-659).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn claim_voxel_with_room(
        &mut self,
        world: &mut voxel::World,
        p: [u32; 3],
        mat: Mat,
        at: [i32; 3],
        motion_mj: u64,
        side: u8,
        t: &Blast,
        room: &mut u32,
    ) -> bool {
        self.claim_voxel_checked(world, p, mat, at, motion_mj, side, t, room)
            .is_ok()
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn claim_voxel_checked(
        &mut self,
        world: &mut voxel::World,
        p: [u32; 3],
        mat: Mat,
        at: [i32; 3],
        motion_mj: u64,
        side: u8,
        t: &Blast,
        room: &mut u32,
    ) -> Result<(), GroundClaimError> {
        // The edit queue drops digs on the world's frame (ADR-034); a claim
        // there would never be confirmed and never released, and every
        // wait on claims would wait forever (ADR-420). Callers check it
        // first; this is the last line.
        let claim = Claim {
            at,
            motion_mj,
            side,
            mat,
        };
        // With no slot left only a cell joining a held component can be
        // claimed, and a group nobody holds has none (ADR-681).
        let (key, bit) = Self::claim_group(p, &claim, t);
        let group = self.groups.entry(key);
        let existing = match &group {
            std::collections::btree_map::Entry::Occupied(entry) => Some(*entry.get()),
            std::collections::btree_map::Entry::Vacant(_) => None,
        };
        if *room == 0 && existing.is_none() {
            return Err(GroundClaimError::NoRoom);
        }
        let mask = existing.unwrap_or(0);
        let delta = shape_added_components(mask, bit, key.3);
        if delta > 0 && *room == 0 {
            return Err(GroundClaimError::NoRoom);
        }
        if self.claims.len() >= t.max_claims as usize {
            return Err(GroundClaimError::ClaimLimit);
        }
        let cell = match self.claims.entry(p) {
            std::collections::btree_map::Entry::Occupied(_) => {
                return Err(GroundClaimError::PendingCell);
            }
            std::collections::btree_map::Entry::Vacant(cell) => cell,
        };
        if !world.inside(p[0], p[1], p[2]) {
            return Err(GroundClaimError::Outside);
        }
        if world.on_frame(p[0], p[1], p[2]) {
            return Err(GroundClaimError::Frame);
        }
        // Admission has not changed the group. Reuse its exact mask and
        // delta instead of recomputing fracture_box and connectivity.
        *group.or_insert(0) = mask | bit;
        self.reserved = self
            .reserved
            .checked_add_signed(delta as isize)
            .expect("ground reservation");
        cell.insert(claim);
        world.queue_edit(Edit::dig_if(p[0], p[1], p[2], mat));
        *room = (*room as i64 - delta).max(0) as u32;
        Ok(())
    }
    pub fn pending(&self) -> usize {
        self.jobs.len()
    }
    pub fn claims(&self) -> usize {
        self.claims.len()
    }
    pub fn claimed(&self, p: [u32; 3]) -> bool {
        self.claims.contains_key(&p)
    }
    /// Claimed cells in ascending order.
    pub fn claimed_cells(&self) -> impl Iterator<Item = &[u32; 3]> + '_ {
        self.claims.keys()
    }
    /// Pool slots the open claims hold: one per connected component of
    /// each group, not one per cell (ADR-659).
    pub fn reserved(&self) -> usize {
        self.reserved
    }
    /// The group a claim of `p` belongs to and its bit in the group's box.
    fn claim_group(p: [u32; 3], c: &Claim, t: &Blast) -> (ClaimGroup, u64) {
        let (origin, side) = fracture_box(p, c.side, t);
        let s = u32::from(side);
        let sub = (p[0] - origin[0]) + s * (p[1] - origin[1]) + s * s * (p[2] - origin[2]);
        ((origin, c.mat, c.at, side), 1u64 << sub)
    }
    /// Slots a claim of `p` would change the reservation by: one for a new
    /// component, none for a cell that joins one, less for a bridge.
    fn claim_delta(&self, p: [u32; 3], c: &Claim, t: &Blast) -> i64 {
        let (key, bit) = Self::claim_group(p, c, t);
        let mask = self.groups.get(&key).copied().unwrap_or(0);
        shape_added_components(mask, bit, key.3)
    }
    fn add_claim(&mut self, p: [u32; 3], c: Claim, t: &Blast) {
        let (key, bit) = Self::claim_group(p, &c, t);
        let mask = self.groups.entry(key).or_default();
        let delta = shape_added_components(*mask, bit, key.3);
        *mask |= bit;
        self.reserved = self
            .reserved
            .checked_add_signed(delta as isize)
            .expect("ground reservation");
        self.claims.insert(p, c);
    }
    fn take_claim(&mut self, p: [u32; 3], t: &Blast) -> Option<Claim> {
        let c = self.claims.remove(&p)?;
        let (key, bit) = Self::claim_group(p, &c, t);
        if let Some(mask) = self.groups.get_mut(&key) {
            let present = *mask & bit != 0;
            *mask &= !bit;
            // Taking a bridge splits its component: the rest holds more.
            if present {
                let delta = -shape_added_components(*mask, bit, key.3);
                self.reserved = self
                    .reserved
                    .checked_add_signed(delta as isize)
                    .expect("ground reservation");
            }
            if *mask == 0 {
                self.groups.remove(&key);
            }
        }
        Some(c)
    }
    /// Rebuild the claim groups after the box tuning changed: a group's
    /// key comes from the tuning, and a reload rebuilds them from the
    /// claims under the tuning it loads with, so a running match must too.
    pub(crate) fn regroup(&mut self, t: &Blast) {
        let claims = std::mem::take(&mut self.claims);
        self.groups.clear();
        self.reserved = 0;
        for (p, c) in claims {
            self.add_claim(p, c, t);
        }
    }
    #[cfg(test)]
    fn clear_claims(&mut self) {
        self.claims.clear();
        self.groups.clear();
        self.reserved = 0;
    }
    pub fn fragment_room(&self, t: &Blast) -> u32 {
        let pool = self.fragment_pools(t);
        (t.max_flying as usize)
            .saturating_sub(pool.flying + self.reserved)
            .min((t.max_fragments as usize).saturating_sub(pool.working + self.reserved))
            as u32
    }
    /// Counts are derived from physical state, never a second copy of matter.
    /// Ordinary sleeping pieces retain shape, pose, ID, Ledger and collisions.
    /// Whole chunks and pieces still moving stay in the working pool.
    pub fn fragment_pools(&self, t: &Blast) -> FragmentPools {
        self.fragment_pool_snapshot(t).0
    }
    /// The admitted sleepers are exactly the parkable entries before this
    /// source-order boundary. Carry it with the counts: no second full scan
    /// or per-piece allocation is needed to freeze wakeup reservations.
    fn fragment_pool_snapshot(&self, t: &Blast) -> (FragmentPools, usize) {
        let mut flying = 0;
        let mut parked = 0;
        let mut end = 0;
        for (i, f) in self.fragments.iter().enumerate() {
            flying += usize::from(!f.resting);
            if parked < t.max_resting_fragments as usize && f.parkable() {
                parked += 1;
                end = i + 1;
            }
        }
        (
            FragmentPools {
                working: self.fragments.len() - parked,
                parked,
                flying,
            },
            end,
        )
    }
    pub(crate) fn emit_body_fragment(
        &mut self,
        mut f: Fragment,
        direction: V3,
        energy_mj: u64,
        t: &Blast,
    ) {
        let norm = direction.len_sq();
        if norm > Fx::ZERO && energy_mj > 0 {
            let denom = f.mass_g.max(1) as u128
                * (voxel::unit::WORLD_MM as u128).pow(2)
                * (crate::TICKS_PER_SECOND as u128).pow(2);
            let speed =
                Fx((((energy_mj as u128 * 2_000_000) << 32) / denom).min(i64::MAX as u128) as i64)
                    .sqrt()
                    .min(Fx::from_ratio(
                        t.max_speed_mm_s as i32,
                        voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
                    ));
            f.velocity = scattered_launch(self.next_id + 1, direction, speed, t);
        }
        // Truncate toward zero for this diagnostic: rounding a negative axis
        // away from zero could report more kinetic energy than was reserved.
        let mm_s = arr(f.velocity).map(|v| {
            (v.raw() as i128 * voxel::unit::WORLD_MM as i128 * crate::TICKS_PER_SECOND as i128
                / (1i128 << 32)) as i64
        });
        self.body_motion_mj = self
            .body_motion_mj
            .saturating_add(voxel::impact::kinetic_mj(
                f.mass_g.min(u32::MAX as u64) as u32,
                mm_s,
            ));
        self.next_id += 1;
        f.id = self.next_id;
        self.body_removed += f.count as u64;
        self.fragments.push(f);
    }
    /// A whole section inherits the parent's motion. That energy was already
    /// present in the vehicle and is not counted as new blast kinetic energy.
    pub(crate) fn emit_section(&mut self, mut f: Fragment) {
        assert!(f.chunk.is_some());
        self.next_id += 1;
        f.id = self.next_id;
        self.body_removed += u64::from(f.count);
        self.fragments.push(f);
    }
    pub fn ready(&self, t: &Blast) -> bool {
        self.jobs.len() < t.max_pending as usize
    }
    pub fn enqueue(
        &mut self,
        world: &voxel::World,
        at: [i32; 3],
        radius: i32,
        energy_j: u32,
        t: &Blast,
    ) -> bool {
        if !self.ready(t) || energy_j == 0 || !(1..=128).contains(&radius) {
            return false;
        }
        if at[0] < 0
            || at[1] < 0
            || at[2] < 0
            || at[0] >= world.nx as i32
            || at[1] >= world.ny as i32
            || at[2] >= world.nz as i32
        {
            return false;
        }
        let cell = (t.cell as i32).max((radius + 31) / 32);
        let half = (radius + cell - 1) / cell;
        let n = (half * 2 + 1) as usize;
        let origin = at.map(|v| v - half * cell);
        let high = origin.map(|v| v + n as i32 * cell);
        let Ok(input) = Snapshot::capture(world, origin, high) else {
            return false;
        };
        self.jobs.push_back(Job {
            at,
            origin,
            n,
            cell,
            input,
            cursor: 0,
            resistance: vec![0; n.pow(3)],
            wave: None,
            hits: VecDeque::new(),
            subcell: 0,
            energy_mj: energy_j as u64 * 1000,
            body_hit_done: false,
            body_energy_mj: 0,
            body_field: crate::body_wave::Field::default(),
        });
        true
    }

    /// Ordered jobs share all budgets. A cap postpones work and never loses
    /// an explosion, a voxel, or a fragment. Only the normal edit queue mutates terrain.
    pub fn capture_last(
        &mut self,
        bodies: &crate::damage::Bodies,
        frames: &[Option<[crate::damage::Frame; 2]>],
        t: &Blast,
    ) {
        let Some(j) = self.jobs.back_mut() else {
            return;
        };
        // Only a job that has not started may take bodies: one that has
        // sampled, captured or handed its resistance to its wave keeps
        // what it has (ADR-287). The load stand caught a refused blast
        // recapturing the last job after its wave had emptied the buffer.
        if j.cursor > 0 || j.wave.is_some() || !j.body_field.empty() {
            return;
        }
        j.body_field =
            crate::body_wave::Field::capture(bodies, frames, j.origin, j.n as i32 * j.cell);
        if !j.body_field.empty() {
            j.body_energy_mj = j.energy_mj * t.body_energy_pct as u64 / 100;
            j.energy_mj -= j.body_energy_mj;
        }
    }
    pub fn advance(&mut self, world: &mut voxel::World, mats: &Registry, t: &Blast) {
        self.advance_inner(world, mats, t, None, None);
    }
    pub fn advance_material(
        &mut self,
        world: &mut voxel::World,
        mats: &Registry,
        bodies: Option<&mut crate::damage::Bodies>,
        t: &crate::Tuning,
    ) {
        self.advance_inner(world, mats, &t.blast, bodies, Some(&t.impact));
    }
    fn advance_inner(
        &mut self,
        world: &mut voxel::World,
        mats: &Registry,
        t: &Blast,
        mut bodies: Option<&mut crate::damage::Bodies>,
        impact: Option<&modding::tuning::Impact>,
    ) {
        let mut samples = t.samples_per_tick;
        let mut pops = t.wave_pops_per_tick;
        let mut edits = t.edits_per_tick;
        let pool = self.fragment_pools(t);
        let room = (t.max_flying as usize).saturating_sub(pool.flying);
        let working = pool.working;
        for _ in 0..self.jobs.len() {
            let mut j = self.jobs.pop_front().unwrap();
            while j.cursor < j.samples() && samples > 0 {
                samples -= 1;
                let (p, c) = j.sample(j.cursor);
                let m = j.input.material(p);
                // Unknown solid materials are barriers, never silently air.
                let r = if m == AIR {
                    0
                } else {
                    mats.get(m).map_or(1_000_000, |d| {
                        d.strength.saturating_mul(t.resistance_per_strength)
                    })
                };
                j.resistance[c] = j.resistance[c].max(r);
                j.cursor += 1;
            }
            if j.cursor == j.samples() && !j.body_field.finished() {
                if let Some(impact) = impact {
                    j.body_field.build(
                        j.origin,
                        j.cell,
                        j.n,
                        &mut j.resistance,
                        &impact.work_kj_m3,
                        t.body_resistance_ppm,
                        &mut samples,
                    );
                }
            }
            if j.cursor == j.samples() && j.body_field.finished() && j.wave.is_none() {
                let amplitude = (j.n as u32 / 2) * 100;
                j.wave = Some(
                    Wave::new(j.n, amplitude.max(1), std::mem::take(&mut j.resistance)).unwrap(),
                );
            }
            if let Some(wave) = j.wave.as_mut() {
                if j.hits.is_empty() && pops > 0 {
                    let before = wave.examined;
                    j.hits.extend(wave.advance(pops as usize));
                    pops -= (wave.examined - before) as u32;
                }
                while let Some(hit) = j.hits.front().copied() {
                    if edits == 0 {
                        break;
                    }
                    if !j.body_hit_done {
                        if let (Some(bodies), Some(impact)) = (bodies.as_deref_mut(), impact) {
                            let before = j.body_energy_mj;
                            let done = j.body_field.hit(
                                hit,
                                j.origin,
                                j.cell,
                                j.n,
                                j.at,
                                &mut j.body_energy_mj,
                                bodies,
                                impact,
                                t.body_resistance_ppm,
                                &mut edits,
                            );
                            self.spent_mj = self.spent_mj.saturating_add(before - j.body_energy_mj);
                            if !done {
                                break;
                            }
                        }
                        j.body_hit_done = true;
                    }
                    if edits == 0 {
                        break;
                    }
                    // Air visits and rejected/duplicate candidates consume work too.
                    edits -= 1;
                    if !hit.broken || j.energy_mj == 0 {
                        j.hits.pop_front();
                        j.subcell = 0;
                        j.body_hit_done = false;
                        continue;
                    }
                    if self.claims.len() >= t.max_claims as usize {
                        break;
                    }
                    let q = ijk(j.subcell, j.cell as usize);
                    let p: [i32; 3] =
                        std::array::from_fn(|a| j.origin[a] + hit.cell[a] * j.cell + q[a]);
                    let candidate = self.candidate(world, mats, p, j.energy_mj, t).map(
                        |(key, mat, cost, motion_mj)| {
                            let claim = Claim {
                                at: j.at,
                                motion_mj,
                                side: t.fragment_box as u8,
                                mat,
                            };
                            (key, cost, claim)
                        },
                    );
                    // A cell that would start a new piece needs a free slot;
                    // one that joins a reserved piece does not (ADR-659).
                    // Without a slot the cell waits for the next tick.
                    if let Some((key, _, claim)) = &candidate {
                        if self.claim_delta(*key, claim, t) > 0
                            && (self.reserved >= room
                                || self.reserved + working >= t.max_fragments as usize)
                        {
                            break;
                        }
                    }
                    j.subcell += 1;
                    if j.subcell == (j.cell as usize).pow(3) {
                        j.subcell = 0;
                        j.hits.pop_front();
                        j.body_hit_done = false;
                    }
                    let Some((key, cost, claim)) = candidate else {
                        continue;
                    };
                    j.energy_mj -= cost;
                    self.spent_mj = self.spent_mj.saturating_add(cost);
                    let material = claim.mat;
                    self.add_claim(key, claim, t);
                    world.queue_edit(Edit::dig_if(key[0], key[1], key[2], material));
                }
            }
            let done = j.wave.as_ref().is_some_and(|w| w.finished()) && j.hits.is_empty();
            if !done {
                self.jobs.push_back(j);
            }
        }
    }

    /// The cell a wave may claim at `p` with `energy_mj` left: its key,
    /// material, fracture cost and motion. `None` for a cell it passes.
    fn candidate(
        &self,
        world: &voxel::World,
        mats: &Registry,
        p: [i32; 3],
        energy_mj: u64,
        t: &Blast,
    ) -> Option<([u32; 3], Mat, u64, u64)> {
        if p[0] < 0
            || p[1] < 0
            || p[2] < 0
            || p[0] >= world.nx as i32
            || p[1] >= world.ny as i32
            || p[2] >= world.nz as i32
        {
            return None;
        }
        let key = p.map(|v| v as u32);
        // The frame takes no edit (ADR-034): a claim there would
        // never clear, and the energy paid for it would buy
        // nothing (ADR-420).
        if self.claims.contains_key(&key)
            || !world.solid(key[0], key[1], key[2])
            || world.on_frame(key[0], key[1], key[2])
        {
            return None;
        }
        let material = world.material(key[0], key[1], key[2]);
        // Snapshot determines reach; current material determines the work
        // and prevents a cheap old soil cell destroying new steel for free.
        let d = mats.get(material)?;
        if d.density == 0 {
            return None;
        }
        let (cost, motion) = fracture_cost_mj(d.strength, voxel::unit::WORLD_MM, t);
        if cost > energy_mj {
            return None;
        }
        Some((key, material, cost, motion))
    }

    /// A no-op consumes its reservation too. Only solid -> air creates debris.
    pub fn confirm(&mut self, completed: &[(Edit, Mat)], e: &mut Economy, t: &Blast) {
        self.confirm_reserving(completed, e, t, 0);
    }
    /// [`Bursts::confirm`] keeping `reserve` slots free for what the tick
    /// set aside (thermal pieces).
    ///
    /// A group reserves a slot per connected component of its open claims
    /// (ADR-659). The slots its confirmed cells free go to its own pieces
    /// first, every group before any extra piece. A component confirmed in
    /// parts, or split by a cell that did not dig, makes more pieces than
    /// it reserved; those extra pieces are made while the pool has room
    /// beyond `reserve`, and a shape without room is pulverised like a
    /// crater's cells: its matter never entered the economy.
    pub(crate) fn confirm_reserving(
        &mut self,
        completed: &[(Edit, Mat)],
        e: &mut Economy,
        t: &Blast,
        reserve: u32,
    ) {
        self.confirm_reserving_inner::<true>(completed, e, t, reserve);
    }

    // The serial oracle retains per-voxel reservation updates in tests.
    fn confirm_reserving_inner<const GROUPED: bool>(
        &mut self,
        completed: &[(Edit, Mat)],
        e: &mut Economy,
        t: &Blast,
        reserve: u32,
    ) {
        let mut groups: BTreeMap<ClaimGroup, (u64, u64)> = BTreeMap::new();
        // Components each touched group held before this confirmation.
        let mut held: BTreeMap<ClaimGroup, u32> = BTreeMap::new();
        for &(edit, old) in completed {
            if edit.mat != AIR {
                continue;
            }
            let Some(expected) = edit.expected_material() else {
                continue;
            };
            let key = [edit.x, edit.y, edit.z];
            let claim = if GROUPED {
                self.claims.remove(&key)
            } else {
                self.claims.get(&key).copied()
            };
            let Some(claim) = claim else { continue };
            let (group, bit) = Self::claim_group(key, &claim, t);
            held.entry(group).or_insert_with(|| {
                shape_component_count(self.groups.get(&group).copied().unwrap_or(0), group.3)
            });
            if GROUPED {
                if let Some(mask) = self.groups.get_mut(&group) {
                    *mask &= !bit;
                }
            } else {
                self.take_claim(key, t);
            }
            if old == AIR || old != expected {
                continue;
            }
            let entry = groups.entry(group).or_default();
            entry.0 |= bit;
            entry.1 = entry.1.saturating_add(claim.motion_mj);
        }
        if GROUPED {
            // No admission or spawn observes reservations inside this atomic
            // confirmation. Recount each touched group once after its removals;
            // a removed bridge can split the survivors, including on a no-op.
            let (mut before_total, mut after_total) = (0usize, 0usize);
            for (group, &before) in &held {
                before_total += before as usize;
                if let std::collections::btree_map::Entry::Occupied(entry) =
                    self.groups.entry(*group)
                {
                    let mask = *entry.get();
                    after_total += shape_component_count(mask, group.3) as usize;
                    if mask == 0 {
                        entry.remove();
                    }
                }
            }
            self.reserved = self
                .reserved
                .checked_sub(before_total)
                .and_then(|rest| rest.checked_add(after_total))
                .expect("ground confirmation reservation");
        }
        if groups.is_empty() {
            return;
        }
        // Pieces per group, and how many of them the group's freed slots
        // already pay for.
        let mut pieces: Vec<(ClaimGroup, u64, u64, Vec<u64>, usize)> = Vec::new();
        for (group, (whole, motion)) in groups {
            let now = shape_component_count(self.groups.get(&group).copied().unwrap_or(0), group.3);
            let freed = held.get(&group).copied().unwrap_or(0).saturating_sub(now) as usize;
            pieces.push((
                group,
                whole,
                motion,
                shape_components(whole, group.3),
                freed,
            ));
        }
        let mut spawn = |b: &mut Self,
                         (key, old, at, side): ClaimGroup,
                         whole: u64,
                         motion: u64,
                         shape: u64| {
            let count = shape.count_ones();
            let motion_mj = motion * count as u64 / whole.count_ones() as u64;
            b.spawn(key, old, at, side, shape, motion_mj, e, t)
        };
        for &(group, whole, motion, ref shapes, freed) in &pieces {
            for &shape in shapes.iter().take(freed) {
                spawn(self, group, whole, motion, shape);
            }
        }
        // Extra pieces take what the pool has left.
        let mut room = self.fragment_room(t).saturating_sub(reserve);
        for (group, whole, motion, shapes, freed) in pieces {
            for shape in shapes.into_iter().skip(freed) {
                if room == 0 {
                    self.dusted += u64::from(shape.count_ones());
                    continue;
                }
                if spawn(self, group, whole, motion, shape) {
                    room -= 1;
                }
            }
        }
    }

    /// One piece: the `shape` cells of `old` in the `side`-box at `key`,
    /// thrown from `at` with `motion_mj` of kinetic energy, its matter
    /// revealed from the ground into a container of its own. `false` for
    /// a material the registry does not know.
    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &mut self,
        key: [u32; 3],
        old: Mat,
        at: [i32; 3],
        side: u8,
        shape: u64,
        motion_mj: u64,
        e: &mut Economy,
        t: &Blast,
    ) -> bool {
        let count = shape.count_ones();
        let Some(d) = e.mats.get(old) else {
            return false;
        };
        let mass_g = (d.density as u64 * t.density_kg_m3_per_density as u64)
            .saturating_mul((voxel::unit::WORLD_MM as u64).pow(3))
            / 1_000_000
            * count as u64;
        let position = centre(key).add(shape_centre(shape, side));
        let mut direction = position.sub(v3(at.map(Fx::from_int)));
        // Ground reflection lifts ejecta. This redirects, without adding to,
        // the assigned kinetic energy; no renderer randomness enters physics.
        direction.y = direction
            .y
            .abs()
            .add(
                Fx::from_ratio(t.lift_pct as i32, 100)
                    .mul(direction.x.abs().add(direction.z.abs())),
            )
            .add(Fx::ONE);
        // speed² in (world units/tick)², computed before Fx to avoid overflow.
        let denom = mass_g.max(1) as u128
            * (voxel::unit::WORLD_MM as u128).pow(2)
            * (crate::TICKS_PER_SECOND as u128).pow(2);
        let v2_raw = ((motion_mj as u128 * 2_000_000) << 32) / denom;
        let speed = Fx(v2_raw.min(i64::MAX as u128) as i64)
            .sqrt()
            .min(Fx::from_ratio(
                t.max_speed_mm_s as i32,
                voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
            ));
        let mut velocity = scattered_launch(self.next_id + 1, direction, speed, t);
        // Ground reflection keeps ejecta above the surface after scatter too.
        velocity.y = velocity.y.abs();
        let mm_s = arr(velocity).map(|v| {
            v.mul(Fx::from_int(
                voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
            ))
            .to_int_trunc() as i64
        });
        self.motion_mj = self.motion_mj.saturating_add(voxel::impact::kinetic_mj(
            mass_g.min(u32::MAX as u64) as u32,
            mm_s,
        ));
        let held = e.ledger.create(matter::Kind::Debris, 0, u64::MAX);
        let amount = e.per_voxel(old) * count as u64;
        e.ledger.reveal(e.ground, amount);
        e.ledger.transfer(e.ground, held, amount);
        self.next_id += 1;
        self.removed += count as u64;
        self.fragments.push(Fragment {
            id: self.next_id,
            position,
            velocity,
            material: old,
            paint: None,
            mm: voxel::unit::WORLD_MM,
            basis: IDENTITY,
            mass_g,
            count,
            shape,
            side,
            chunk: None,
            held,
            resting: false,
        });
        true
    }

    /// Cells the world let go of (ADR-299: terrain left without support)
    /// become pieces at zero speed, grouped by box and material like burst
    /// debris. A shape the pool has no room for goes back into the world
    /// cell by cell, so that no matter is lost; it will be found hanging
    /// again. Returns pieces made and cells put back.
    pub fn pieces_from_cells(
        &mut self,
        world: &mut voxel::World,
        cells: &[([u32; 3], Mat)],
        e: &mut Economy,
        t: &Blast,
        side: u8,
    ) -> (usize, usize) {
        let side = side.clamp(1, 4);
        let groups = crumb_boxes(cells, side);
        let (mut made, mut returned) = (0, 0);
        // One scan of the pool, not one per shape: each spawn takes one
        // slot from both the flying and the working budget.
        let mut room = if groups.is_empty() {
            0
        } else {
            self.fragment_room(t)
        };
        for ((key, m), whole) in groups {
            let at = key.map(|v| v as i32 + i32::from(side) / 2);
            for shape in shape_components(whole, side) {
                if room > 0 && self.spawn(key, m, at, side, shape, 0, e, t) {
                    room -= 1;
                    made += 1;
                    continue;
                }
                // No room for a piece: the cell stays in the world. Its
                // detach dig is a transfer and drains first; so does this
                // put-back, right behind it (ADR-431).
                for i in 0..box_cells(side) {
                    if shape & (1 << i) != 0 {
                        let c = shape_cell(i, side);
                        world.queue_transfer(Edit::put(
                            key[0] + c[0] as u32,
                            key[1] + c[1] as u32,
                            key[2] + c[2] as u32,
                            m,
                        ));
                        returned += 1;
                    }
                }
            }
        }
        (made, returned)
    }

    /// How many pieces [`Bursts::pieces_from_cells`] makes of these cells:
    /// one per connected shape of each box and material.
    pub fn pieces_needed(cells: &[([u32; 3], Mat)], side: u8) -> usize {
        let side = side.clamp(1, 4);
        crumb_boxes(cells, side)
            .into_values()
            .map(|whole| shape_components(whole, side).len())
            .sum()
    }

    pub fn step_fragments(
        &mut self,
        world: &voxel::World,
        e: &mut Economy,
        t: &Blast,
        gravity: Fx,
        tick: u64,
        around: Surroundings<'_>,
    ) {
        self.step_fragments_observed(world, e, t, gravity, tick, around, |_| {}, &|_, _| {});
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn step_fragments_observed(
        &mut self,
        world: &voxel::World,
        e: &mut Economy,
        t: &Blast,
        gravity: Fx,
        tick: u64,
        around: Surroundings<'_>,
        completed: impl FnMut(&'static str),
        task: &(impl Fn(usize, &'static str) + Sync),
    ) {
        self.step_fragments_reserved(
            world,
            e,
            t,
            gravity,
            tick,
            around,
            0,
            self.jobs
                .iter()
                .any(|j| j.energy_mj > 0 || j.body_energy_mj > 0),
            completed,
            task,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn step_fragments_reserved(
        &mut self,
        world: &voxel::World,
        e: &mut Economy,
        t: &Blast,
        gravity: Fx,
        tick: u64,
        around: Surroundings<'_>,
        thermal_reserve: u32,
        damage_waiting: bool,
        completed: impl FnMut(&'static str),
        task: &(impl Fn(usize, &'static str) + Sync),
    ) {
        self.step_fragments_kernel::<true, true>(
            world,
            e,
            t,
            gravity,
            tick,
            around,
            thermal_reserve,
            damage_waiting,
            completed,
            task,
        );
    }
    #[allow(clippy::too_many_arguments)]
    fn step_fragments_kernel<const INDEXED_HULLS: bool, const COMPACT_WAKE: bool>(
        &mut self,
        world: &voxel::World,
        e: &mut Economy,
        t: &Blast,
        gravity: Fx,
        tick: u64,
        around: Surroundings<'_>,
        thermal_reserve: u32,
        damage_waiting: bool,
        mut completed: impl FnMut(&'static str),
        task: &(impl Fn(usize, &'static str) + Sync),
    ) {
        let (hulls, yielding) = (around.hulls, around.yielding);
        let hull_queries = if INDEXED_HULLS {
            HullQueries::new(hulls)
        } else {
            HullQueries::scalar(hulls)
        };
        let motion = FragmentMotion::new(t);
        // Tilted hulls on a crater floor can shed cells whose captured centre
        // is below the map. Apply the common boundary rule before freezing
        // support queries, including newly admitted sections and resting pieces.
        let (mut flying, mut parked, mut parked_end) = (0, 0, 0);
        for (i, f) in self.fragments.iter_mut().enumerate() {
            flying += usize::from(!f.resting);
            if parked < t.max_resting_fragments as usize && f.parkable() {
                parked += 1;
                parked_end = i + 1;
            }
            f.keep_on_map([world.nx, world.nz]);
        }
        let pool = FragmentPools {
            working: self.fragments.len() - parked,
            parked,
            flying,
        };
        for f in self.corpses.values_mut() {
            f.keep_on_map([world.nx, world.nz]);
        }
        let mut merged = BTreeSet::new();
        let mut stale_wrecks = BTreeSet::new();
        let mut settled = std::mem::take(&mut self.settled_scratch);
        let mut queries = std::mem::take(&mut self.query_scratch);
        Settled::for_phase_into(
            &self.fragments,
            hulls,
            tick,
            t,
            damage_waiting,
            &mut settled,
            &mut queries,
            &mut completed,
            task,
        );
        self.settled_queries = settled.requests;
        self.settled_query_words = queries.word_counts();
        completed("debris_settled");
        // Waking pieces count against the flying budget like new ones; a
        // piece without room waits for its next turn. A claim holds its
        // flying slot until its dig lands and it becomes a piece: a
        // waking piece does not take it (ADR-443). A group of claims holds
        // a slot per component, not per cell (ADR-659).
        let mut room = (t.max_flying as usize)
            .saturating_sub(pool.flying + self.reserved + thermal_reserve as usize);
        let mut working_room = (t.max_fragments as usize)
            .saturating_sub(pool.working + self.reserved + thermal_reserve as usize);
        if damage_waiting {
            // The routine queue (heat, felling, wakeups) shares free capacity
            // with waiting damage. Keep the damage share for the next tick;
            // otherwise old pieces can repeatedly take every newly free slot.
            let free = room.min(working_room);
            let routine = routine_fragment_share(
                (free + thermal_reserve as usize) as u32,
                true,
                tick.wrapping_add(1),
            )
            .saturating_sub(thermal_reserve) as usize;
            let reserve = free.saturating_sub(routine);
            room -= reserve;
            working_room -= reserve;
        }
        // Wakeups claim working slots in the frozen source order. A parked
        // piece never borrows a slot reserved by a pending terrain dig.
        // These queries read the frozen surroundings and only this piece.
        // Compute candidates independently; the ordered pass below still
        // allocates flying slots and therefore resolves competing wakeups.
        let query = |(i, f): (usize, &Fragment)| {
            let can_wake = room > 0 && (!(i < parked_end && f.parkable()) || working_room > 0);
            let mut push = None;
            if f.chunk.is_none() && t.push_pct > 0 && (!f.resting || can_wake) {
                let (low, high) = settled.fragment_bounds(i as u32, f);
                if let Some(h) = hull_queries.hit_moving(arr(f.position), low, high, 0) {
                    // What the hull climbs as floor it does not push: a
                    // piece of a yielding material, or one rising above
                    // the hull's step, is bulldozed; the rest is ridden.
                    let climbs = !yielding.contains(&f.material)
                        && f.position.y.add(high[1]) <= h.pos[1].add(h.step);
                    if h.vel != [Fx::ZERO; 3] && !climbs {
                        // A moving hull bulldozes the piece it drove into
                        // (ADR-294): the piece leaves ahead of it at the
                        // hull's pace and a little up.
                        let along = V3::new(h.vel[0], Fx::ZERO, h.vel[2]).scale(motion.push);
                        let lift = along.len_sq().sqrt().mul(motion.push_up);
                        push = Some(V3::new(
                            along.x,
                            h.vel[1].mul(motion.push).add(lift),
                            along.z,
                        ));
                    }
                }
            }
            // A single free slot alternates between damage and wakeups. Check
            // the missed deadline too: otherwise that alternation can alias an
            // even rest-check period and leave a piece unsupported forever.
            let unsupported = can_wake
                && push.is_none()
                && f.resting
                && support_due(f.id, tick, t, damage_waiting)
                && !supported(f, world, hulls, &settled, i as u32);
            (push, unsupported)
        };
        let mut motion_pending = pool.flying > 0;
        let wakeups = fragment_wake_queries::<COMPACT_WAKE>(&self.fragments, query, task);
        for (i, (push, unsupported)) in wakeups {
            let f = &mut self.fragments[i];
            let parked = i < parked_end && f.parkable();
            let can_wake = room > 0 && (!parked || working_room > 0);
            if let Some(velocity) = push.filter(|_| !f.resting || can_wake) {
                if f.resting {
                    f.resting = false;
                    self.wakeups += 1;
                    room -= 1;
                    working_room -= usize::from(parked);
                    stale_wrecks.insert(f.held);
                }
                f.velocity = velocity;
                motion_pending = true;
            }
            if can_wake && f.resting && unsupported {
                // The ground under it was taken (ADR-291): it falls again,
                // and its wreck entry goes until it settles anew.
                f.resting = false;
                motion_pending = true;
                self.wakeups += 1;
                room -= 1;
                working_room -= usize::from(parked);
                stale_wrecks.insert(f.held);
            }
        }
        completed("debris_wake");
        // All support checks and flying-slot claims above retain fragment
        // order. Translation reads only the frozen world, hulls and settled
        // index; no fragment observes another fragment's new position until
        // the ordered pile/ledger pass below.
        let translate = |(i, f): (usize, &mut Fragment)| {
            advance_fragment(
                f, world, t, &motion, gravity, hulls, &settled, i as u32, true,
            )
            .then(|| {
                let p = arr(f.position).map(|v| v.to_int_trunc());
                Wreck {
                    held: f.held,
                    x: p[0],
                    y: p[1],
                    z: p[2],
                }
            })
        };
        let mut landed: Vec<Wreck> = if !motion_pending {
            Vec::new()
        } else if self.fragments.len() >= 1024 {
            self.fragments
                .par_iter_mut()
                .enumerate()
                .map_init(
                    || PhysicsTask::new(task, "physics_translate_begin", "physics_translate_end"),
                    |_, item| translate(item),
                )
                .filter_map(|landed| landed)
                .collect()
        } else {
            self.fragments
                .iter_mut()
                .enumerate()
                .filter_map(translate)
                .collect()
        };
        completed("debris_translate");
        // Equal keys always enter the same shard; source slots stay ordered
        // inside it. Each independent table therefore chooses the same first
        // pile owner as the original single table, without shared mutation.
        let parallel = self.fragments.len() >= 1024;
        let mut memo = std::mem::take(&mut self.join_memo);
        let mut build_keys = || {
            let pile_grid = PileGrid::new(t.pile_cell);
            let mut shards = std::mem::take(&mut self.join_shards);
            for shard in &mut shards {
                shard.clear();
            }
            for (i, f) in self.fragments.iter().enumerate() {
                if let Some(geometry) = fragment_join::Geometry::of(f) {
                    memo.push(
                        i,
                        (pile_grid.at(f.position), f.material, f.paint, f.mm),
                        f,
                        geometry,
                    );
                }
            }
            let changed = memo.changed();
            if changed.as_ref().is_none_or(|keys| !keys.is_empty()) {
                for input in &memo.current {
                    let key = input.key;
                    if changed
                        .as_ref()
                        .is_some_and(|keys| !keys.contains_key(&key))
                    {
                        continue;
                    }
                    let hash = (key.0[0] as u32).wrapping_mul(0x9e3779b9).rotate_left(13)
                        ^ (key.0[1] as u32).wrapping_mul(0xc2b2ae35)
                        ^ (key.0[2] as u32).wrapping_mul(0x85ebca6b);
                    let shard = if parallel { (hash >> 29) as usize } else { 0 };
                    shards[shard].push((key, input.index));
                }
            }
            (shards, changed)
        };
        let (shards, changed) = if parallel && !stale_wrecks.is_empty() {
            let wake_stale = std::mem::take(&mut stale_wrecks);
            rayon::join(build_keys, || {
                retain_live(&mut e.wrecks, &wake_stale, |w| w.held, true);
            })
            .0
        } else {
            let result = build_keys();
            if !stale_wrecks.is_empty() {
                let wake_stale = std::mem::take(&mut stale_wrecks);
                retain_live(&mut e.wrecks, &wake_stale, |w| w.held, true);
            }
            result
        };
        completed("debris_merge_keys");
        let group = |entries: &Vec<(RestingPile, usize)>| {
            let _task = PhysicsTask::new(task, "physics_merge_begin", "physics_merge_end");
            let mut piles: FxLookup<RestingPile, Vec<(usize, fragment_join::Geometry)>> =
                FxLookup::with_capacity(entries.len());
            let mut pairs = Vec::new();
            for &(key, donor) in entries {
                let geometry = fragment_join::Geometry::of(&self.fragments[donor]).unwrap();
                let heads = piles.entry(key).or_default();
                let joined = heads.iter_mut().find_map(|(head, shape)| {
                    let next = shape.join(geometry)?;
                    *shape = next;
                    Some((*head, next))
                });
                if let Some((head, shape)) = joined {
                    pairs.push((head, donor, shape));
                } else if heads.len() < fragment_join::HEADS {
                    heads.push((donor, geometry));
                }
            }
            pairs
        };
        let groups: Vec<_> = if changed.as_ref().is_some_and(|keys| keys.is_empty()) {
            Vec::new()
        } else if parallel {
            shards.par_iter().map(group).collect()
        } else {
            shards[..1].iter().map(group).collect()
        };
        self.join_shards = shards;
        completed("debris_merge_groups");
        let mut transfers: Vec<_> = groups.into_iter().flatten().collect();
        memo.finish();
        self.join_memo = memo;
        let mut moved_heads = BTreeMap::new();
        transfers.sort_unstable_by_key(|&(_, donor, _)| donor);
        for (head, donor, shape) in transfers {
            let (a, b) = self.fragments.split_at_mut(donor);
            let to = &mut a[head];
            let from = &b[0];
            e.ledger.transfer(from.held, to.held, u64::MAX);
            to.mass_g += from.mass_g;
            to.count += from.count;
            shape.apply(to);
            moved_heads.insert(to.held, arr(to.position).map(Fx::to_int_trunc));
            merged.insert(from.held);
        }
        completed("debris_advance");
        if !merged.is_empty() {
            retain_live(&mut self.fragments, &merged, |f| f.held, true);
            stale_wrecks.extend(merged.iter().copied());
        }
        completed("debris_compact");
        // A waking piece can land again in this same tick. Remove its old
        // wreck once, then append the newly landed one in fragment order.
        if !stale_wrecks.is_empty() {
            retain_live(&mut e.wrecks, &stale_wrecks, |w| w.held, true);
        }
        completed("debris_wreck_prune");
        if !moved_heads.is_empty() {
            for w in e.wrecks.iter_mut().chain(landed.iter_mut()) {
                if let Some(p) = moved_heads.get(&w.held) {
                    w.x = p[0];
                    w.y = p[1];
                    w.z = p[2];
                }
            }
        }
        e.wrecks
            .extend(landed.into_iter().filter(|w| !merged.contains(&w.held)));
        completed("debris_wreck_append");
        for held in merged {
            e.ledger.retire_empty_debris(held);
        }
        completed("debris_retire");
        for f in self.corpses.values_mut() {
            if f.resting
                && due(f.id, tick, t)
                && !supported(f, world, &[], &Settled::default(), u32::MAX)
            {
                f.resting = false;
            }
            // A dead hull lies as it fell: whole hulls do not tumble.
            advance_fragment(
                f,
                world,
                t,
                &motion,
                gravity,
                &[],
                &Settled::default(),
                u32::MAX,
                false,
            );
        }
        completed("debris_corpses");
        for w in &mut e.wrecks {
            if let Some(f) = self.corpses.get(&w.held) {
                let p = arr(f.position).map(|v| v.to_int_trunc());
                w.x = p[0];
                w.y = p[1];
                w.z = p[2];
            }
        }
        self.settled_scratch = settled;
        self.query_scratch = queries;
        completed("debris_finish");
    }

    /// The columns resting pieces stand in and the height of their tops,
    /// for the contacts, feet and planner of the range (ADR-292). Pieces
    /// of `yielding` materials are left out: a hull crushes through a
    /// felled crown as through a standing one, and so is a piece lying on
    /// a hull's roof rather than on the world (ADR-293). Derived from the
    /// pieces every tick; nothing is saved.
    /// A piece that came to rest against a live hull's flank — its centre
    /// outside the hull's box, its edge reaching under — is no floor under
    /// that hull (ADR-302): the columns of the box it covers stay out of
    /// the map, while a piece the hull stands over is floor as before.
    pub fn rubble(
        &self,
        world: &voxel::World,
        yielding: &[Mat],
        hulls: &[Hull],
    ) -> nav::vehicle::Rubble {
        self.rubble_observed(world, yielding, hulls, &|_, _| {})
    }
    pub(crate) fn rubble_observed(
        &self,
        world: &voxel::World,
        yielding: &[Mat],
        hulls: &[Hull],
        task: &(impl Fn(usize, &'static str) + Sync),
    ) -> nav::vehicle::Rubble {
        self.rubble_with_scratch(world, yielding, hulls, &mut RubbleScratch::default(), task)
    }
    /// A standalone query has no edit history and reads support afresh.
    pub(crate) fn rubble_with_scratch(
        &self,
        world: &voxel::World,
        yielding: &[Mat],
        hulls: &[Hull],
        scratch: &mut RubbleScratch,
        task: &(impl Fn(usize, &'static str) + Sync),
    ) -> nav::vehicle::Rubble {
        self.rubble_with_scratch_changed(world, yielding, hulls, scratch, task, None)
    }
    /// A located change set is valid only since this scratch's previous call.
    /// Hull and stacked-piece support is always read from the current phase.
    pub(crate) fn rubble_with_scratch_changed(
        &self,
        world: &voxel::World,
        yielding: &[Mat],
        hulls: &[Hull],
        scratch: &mut RubbleScratch,
        task: &(impl Fn(usize, &'static str) + Sync),
        changed: Option<&nav::vehicle::Changed>,
    ) -> nav::vehicle::Rubble {
        // Floor is what something holds: the world, or a resting piece
        // under it — a pile on a rock piece is floor at the pile's top.
        // Only the world was asked, and a stacked piece was missing from
        // the map (ADR-458). A piece on a roof, or one whose support was
        // taken and whose turn to fall has not come, is not floor; one a
        // hull carries is not asked about the pieces (ADR-469).
        // Reuse local boxes only within this immutable query: support edits,
        // hull movement and fragment changes are visible on the next call.
        let RubbleScratch {
            bounds,
            settled,
            queries,
            stacked_mask,
            column_merge,
            floor_proofs,
        } = scratch;
        {
            let _task = PhysicsTask::new(
                task,
                "physics_rubble_prepare_begin",
                "physics_rubble_prepare_end",
            );
            bounds.clear();
            bounds.resize(self.fragments.len(), None);
            if changed.is_some() {
                floor_proofs.resize(self.fragments.len(), None);
            } else {
                floor_proofs.clear();
            }
            stacked_mask.clear();
        }
        let mut floor = Vec::new();
        let mut stacked = Vec::new();
        // Each batch owns its boxes and candidate lists. World/hulls/fragments
        // stay immutable; indexed collection preserves the original ascending
        // fragment order required by the ordered column gather.
        let classify = |start: usize,
                        boxes: &mut [Option<([Fx; 3], [Fx; 3])>],
                        proofs: &mut [Option<RubbleFloorProof>]| {
            let _task = PhysicsTask::new(
                task,
                "physics_rubble_floor_begin",
                "physics_rubble_floor_end",
            );
            let mut floor = Vec::new();
            let mut stacked = Vec::new();
            for (offset, slot) in boxes.iter_mut().enumerate() {
                let i = start + offset;
                let f = &self.fragments[i];
                if !f.resting || f.chunk.is_some() || yielding.contains(&f.material) {
                    if let Some(proof) = proofs.get_mut(offset) {
                        *proof = None;
                    }
                    continue;
                }
                let old = proofs.get(offset).copied().flatten().filter(|proof| {
                    proof.matches(f)
                        && changed.is_some_and(|dirty| !dirty.touches(proof.lo, proof.hi))
                });
                let (low, high, on_world) = if let Some(proof) = old {
                    (proof.low, proof.high, proof.on_world)
                } else {
                    let (low, high) = f.bounds();
                    let mut p = arr(f.position);
                    p[1] = p[1].sub(Fx::from_ratio(1, 2));
                    let contact =
                        fragment_contact(f, world, p, low, high, [world.nx, world.ny, world.nz]);
                    if let Some(proof) = proofs.get_mut(offset) {
                        *proof = (f.basis == IDENTITY)
                            .then(|| RubbleFloorProof::new(f, low, high, contact));
                    }
                    (low, high, contact)
                };
                *slot = Some((low, high));
                if on_world {
                    floor.push(i);
                } else if !hulls
                    .iter()
                    .any(|h| h.carries(f.position.y.add(low[1]), f.position))
                {
                    stacked.push(i);
                }
            }
            (floor, stacked)
        };
        {
            // Includes dispatch, pool join and ordered candidate concatenation.
            // Leaf timings alone cannot explain waiting at the join.
            let _task = PhysicsTask::new(
                task,
                "physics_rubble_classify_begin",
                "physics_rubble_classify_end",
            );
            if self.fragments.len() >= 1024 {
                let groups: Vec<_> = if floor_proofs.is_empty() {
                    bounds
                        .par_chunks_mut(256)
                        .enumerate()
                        .map(|(batch, boxes)| classify(batch * 256, boxes, &mut []))
                        .collect()
                } else {
                    bounds
                        .par_chunks_mut(256)
                        .zip(floor_proofs.par_chunks_mut(256))
                        .enumerate()
                        .map(|(batch, (boxes, proofs))| classify(batch * 256, boxes, proofs))
                        .collect()
                };
                for (on_world, on_pieces) in groups {
                    floor.extend(on_world);
                    stacked.extend(on_pieces);
                }
            } else {
                (floor, stacked) = classify(0, bounds, floor_proofs);
            }
        }
        if !stacked.is_empty() {
            {
                let _task = PhysicsTask::new(
                    task,
                    "physics_rubble_support_build_begin",
                    "physics_rubble_support_build_end",
                );
                // Dense source indices need only one bit each. This is the
                // same immutable candidate set, rebuilt on every call; no
                // binary search per fragment or cross-tick membership cache.
                stacked_mask.resize(self.fragments.len().div_ceil(64), 0);
                for &i in &stacked {
                    stacked_mask[i / 64] |= 1u64 << (i % 64);
                }
                Settled::around_bounds_into(
                    &self.fragments,
                    hulls,
                    |i, _| stacked_mask[i / 64] & (1u64 << (i % 64)) != 0,
                    |i, f| bounds[i].unwrap_or_else(|| f.bounds()),
                    SettledReach::Support,
                    settled,
                    queries,
                    &mut |_| {},
                    task,
                );
            }
            // These pieces already failed the world support test above.
            // No world or hull changes occur while this query is running.
            {
                let _task = PhysicsTask::new(
                    task,
                    "physics_rubble_support_query_begin",
                    "physics_rubble_support_query_end",
                );
                let supported = |&i: &usize| {
                    let f = &self.fragments[i];
                    let (low, high) = bounds[i].unwrap();
                    settled.supports(arr(f.position), low, high, i as u32)
                };
                if stacked.len() >= 1024 && rayon::current_num_threads() > 1 {
                    // Every query reads the same frozen index. Keep each
                    // cell's contact order and join indexed batches in source
                    // order; no landing, wakeup or world edits occur here.
                    let groups: Vec<Vec<_>> = stacked
                        .par_chunks(256)
                        .map(|indices| {
                            let _task = PhysicsTask::new(
                                task,
                                "physics_rubble_support_read_begin",
                                "physics_rubble_support_read_end",
                            );
                            indices.iter().copied().filter(supported).collect()
                        })
                        .collect();
                    for group in groups {
                        floor.extend(group);
                    }
                } else {
                    let _task = PhysicsTask::new(
                        task,
                        "physics_rubble_support_read_begin",
                        "physics_rubble_support_read_end",
                    );
                    floor.extend(stacked.into_iter().filter(supported));
                }
            }
            // Temporary contact tables can own many small lists. Their release
            // must be visible separately from support queries and pool waits.
            let _task = PhysicsTask::new(
                task,
                "physics_rubble_support_drop_begin",
                "physics_rubble_support_drop_end",
            );
            // Release logical entries, keeping their empty buffers for the
            // next full rebuild. No support information survives this call.
            settled.cells.clear();
            for requests in &mut settled.cells.requests {
                requests.clear();
            }
            settled.boxes.clear();
            settled.requests = 0;
            queries.clear();
            stacked_mask.clear();
        }
        // A hull covers no column outside the box around its footprint:
        // a piece far from every hull asks none of them (ADR-465).
        let _columns_task = PhysicsTask::new(
            task,
            "physics_rubble_columns_begin",
            "physics_rubble_columns_end",
        );
        let reach: Vec<([Fx; 2], [Fx; 2])> = hulls.iter().map(Hull::footprint_box).collect();
        if floor.len() >= 1024 && hulls.len() <= 64 {
            if let Some(columns) = gather_column_spans(
                &self.fragments,
                bounds,
                &floor,
                hulls,
                &reach,
                column_merge,
                task,
            ) {
                bounds.clear();
                let _publish = PhysicsTask::new(
                    task,
                    "physics_rubble_publish_begin",
                    "physics_rubble_publish_end",
                );
                return nav::vehicle::Rubble::from_sorted(columns);
            }
        }
        // A batch reuses its hull filter and owns its output. Joining batches
        // in floor order keeps both rounding and equal-column input order.
        let gather = |floor: &[usize]| {
            let _task = PhysicsTask::new(
                task,
                "physics_rubble_gather_begin",
                "physics_rubble_gather_end",
            );
            let mut against: Vec<&Hull> = Vec::with_capacity(hulls.len());
            let mut columns: Vec<([i32; 2], u16)> = Vec::new();
            for &i in floor {
                let f = &self.fragments[i];
                let (low, high) = bounds[i].unwrap();
                let p = arr(f.position);
                // Floor of the bottom corners, ceiling of the top: the columns
                // the box covers and the height it reaches.
                let lo = [p[0].add(low[0]), p[2].add(low[2])].map(|v| (v.raw() >> 32) as i32);
                let hi = [p[0].add(high[0]), p[2].add(high[2])].map(|v| (v.raw() >> 32) as i32);
                let top = -(p[1].add(high[1]).neg().raw() >> 32);
                let top = top.clamp(0, i64::from(u16::MAX)) as u16;
                against.clear();
                against.extend(hulls.iter().zip(&reach).filter_map(|(h, (b0, b1))| {
                    let near = (0..2).all(|a| {
                        Fx::from_int(lo[a]) <= b1[a] && Fx::from_int(hi[a]).add(Fx::ONE) >= b0[a]
                    });
                    (near && !h.covers_point([p[0], p[2]])).then_some(h)
                }));
                for z in lo[1].max(0)..=hi[1] {
                    for x in lo[0].max(0)..=hi[0] {
                        if against.iter().any(|h| h.covers(x, z)) {
                            continue;
                        }
                        columns.push(([x, z], top));
                    }
                }
            }
            columns
        };
        let mut columns = {
            let _task = PhysicsTask::new(
                task,
                "physics_rubble_gather_dispatch_begin",
                "physics_rubble_gather_dispatch_end",
            );
            if floor.len() >= 1024 {
                let groups: Vec<_> = floor.par_chunks(256).map(gather).collect();
                let mut columns = Vec::with_capacity(groups.iter().map(Vec::len).sum());
                for group in groups {
                    columns.extend(group);
                }
                columns
            } else {
                gather(&floor)
            }
        };
        // A column under several pieces holds the highest top. Collapse
        // duplicate heights while ordering, without sorting every copy by Z.
        {
            let _task = PhysicsTask::new(
                task,
                "physics_rubble_merge_begin",
                "physics_rubble_merge_end",
            );
            merge_columns_observed(&mut columns, task, column_merge);
        }
        bounds.clear();
        let _publish = PhysicsTask::new(
            task,
            "physics_rubble_publish_begin",
            "physics_rubble_publish_end",
        );
        nav::vehicle::Rubble::from_sorted(columns)
    }
    /// Remove the resting pieces whose matter a digger took whole
    /// (ADR-674): their geometry, their wreck entries and their emptied
    /// containers, in order and in the tick they were taken. What rested on
    /// them finds no support at its next rest check and falls (ADR-291).
    pub(crate) fn retire_reclaimed(&mut self, e: &mut Economy, taken: &BTreeSet<u32>) {
        retain_live(&mut self.fragments, taken, |f| f.held, true);
        retain_live(&mut e.wrecks, taken, |w| w.held, true);
        for &held in taken {
            assert!(
                e.ledger.retire_empty_debris(held),
                "a reclaimed piece leaves empty debris"
            );
        }
    }
    /// Before empty wreck containers are released/reused, remove their geometry.
    pub fn sweep(&mut self, e: &Economy) {
        self.corpses.retain(|held, _| e.ledger.held(*held) > 0);
        self.fragments
            .retain(|f| !f.resting || e.ledger.held(f.held) > 0);
    }

    pub fn hash(&self) -> u64 {
        if self.next_id == 0 && self.jobs.is_empty() && self.claims.is_empty() && self.spent_mj == 0
        {
            return 0;
        }
        let mut h = 0;
        for v in [
            self.next_id,
            self.removed,
            self.spent_mj,
            self.motion_mj,
            self.body_motion_mj,
            self.body_removed,
            self.jobs.len() as u64,
        ] {
            mix(&mut h, v);
        }
        for j in &self.jobs {
            for v in j.at.into_iter().chain(j.origin) {
                mix(&mut h, v as u64);
            }
            for v in [
                j.n as u64,
                j.cell as u64,
                j.input.hash(),
                j.cursor as u64,
                j.subcell as u64,
                j.energy_mj,
                j.body_energy_mj,
                j.body_field.hash(),
                j.body_hit_done as u64,
                j.wave.as_ref().map_or(0, |w| w.examined as u64 + 1),
                j.hits.len() as u64,
            ] {
                mix(&mut h, v);
            }
            // Resistance and frontier are pure functions of this immutable input
            // and exact build/pop cursors. No full 65³ traversal on each hash tick.
            for hit in &j.hits {
                for v in hit.cell {
                    mix(&mut h, v as u64);
                }
                mix(&mut h, hit.pressure as u64);
                mix(&mut h, hit.broken as u64);
            }
        }
        mix(&mut h, self.claims.len() as u64);
        for (p, c) in &self.claims {
            for v in *p {
                mix(&mut h, v as u64);
            }
            for v in c.at {
                mix(&mut h, v as u64);
            }
            mix(&mut h, c.motion_mj);
            mix(&mut h, u64::from(c.side));
            mix(&mut h, u64::from(c.mat));
        }
        mix(&mut h, self.fragments.len() as u64);
        if !self.corpses.is_empty() {
            mix(&mut h, self.corpses.len() as u64);
        }
        for f in self.fragments.iter().chain(self.corpses.values()) {
            for v in [
                f.id,
                f.material as u64,
                f.mm as u64,
                f.paint.map_or(0, |p| {
                    1 + p[0] as u64 + ((p[1] as u64) << 8) + ((p[2] as u64) << 16)
                }),
                f.mass_g,
                f.count as u64,
                f.shape,
                u64::from(f.side),
                f.held as u64,
                f.resting as u64,
            ] {
                mix(&mut h, v);
            }
            if let Some(c) = &f.chunk {
                mix(&mut h, c.hash());
            }
            for v in arr(f.position)
                .into_iter()
                .chain(arr(f.velocity))
                .chain(f.basis.into_iter().flat_map(arr))
            {
                mix(&mut h, v.raw() as u64);
            }
        }
        h
    }

    pub fn save(&self, b: &mut Blob) {
        for v in [
            self.next_id,
            self.removed,
            self.spent_mj,
            self.motion_mj,
            self.body_motion_mj,
            self.body_removed,
        ] {
            b.u64(v);
        }
        b.usize(self.jobs.len());
        for j in &self.jobs {
            for v in j.at.into_iter().chain(j.origin) {
                b.i32(v);
            }
            b.usize(j.n);
            b.i32(j.cell);
            j.input.save(b);
            b.usize(j.cursor);
            b.u32s(&j.resistance);
            b.bool(j.wave.is_some());
            if let Some(w) = &j.wave {
                w.save(b);
            }
            b.usize(j.hits.len());
            for hit in &j.hits {
                for v in hit.cell {
                    b.i32(v);
                }
                b.u32(hit.pressure);
                b.bool(hit.broken);
            }
            b.usize(j.subcell);
            b.u64(j.energy_mj);
            b.u64(j.body_energy_mj);
            b.bool(j.body_hit_done);
            j.body_field.save(b);
        }
        b.usize(self.claims.len());
        for (p, c) in &self.claims {
            for v in *p {
                b.u32(v);
            }
            for v in c.at {
                b.i32(v);
            }
            b.u64(c.motion_mj);
            b.u8(c.side);
            b.u16(c.mat);
        }
        b.usize(self.fragments.len());
        b.usize(self.corpses.len());
        for f in self.fragments.iter().chain(self.corpses.values()) {
            b.u64(f.id);
            save_v(b, f.position);
            save_v(b, f.velocity);
            b.u16(f.material);
            b.u16(f.mm);
            b.bool(f.paint.is_some());
            if let Some(p) = f.paint {
                for v in p {
                    b.u8(v);
                }
            }
            for v in f.basis {
                save_v(b, v);
            }
            b.u64(f.mass_g);
            b.u32(f.count);
            b.u64(f.shape);
            b.u8(f.side);
            b.u32(f.held);
            b.bool(f.resting);
            b.bool(f.chunk.is_some());
            if let Some(c) = &f.chunk {
                c.save(b);
            }
        }
    }
    pub fn load(s: &mut Scan, world: Option<&voxel::World>, t: &Blast) -> Option<Self> {
        Self::load_material(s, world, t, None, 0, false, true)
    }
    /// `two_cell_masks`: the save predates ADR-284 and holds eight-bit
    /// masks without a box side; every piece is read as a two-cell box.
    /// `claim_materials`: each claim holds the material its dig expects
    /// (ADR-659). An older save reads it from the world, where the dig
    /// has not landed yet.
    #[allow(clippy::too_many_arguments)]
    pub fn load_material(
        s: &mut Scan,
        world: Option<&voxel::World>,
        t: &Blast,
        bodies: Option<&crate::damage::Bodies>,
        units: usize,
        two_cell_masks: bool,
        claim_materials: bool,
    ) -> Option<Self> {
        let mut out = Self {
            next_id: s.u64(),
            removed: s.u64(),
            spent_mj: s.u64(),
            motion_mj: s.u64(),
            body_motion_mj: s.u64(),
            body_removed: s.u64(),
            ..Self::default()
        };
        let count = s.usize();
        if count > t.max_pending as usize {
            return None;
        }
        for _ in 0..count {
            let at = std::array::from_fn(|_| s.i32());
            let origin = std::array::from_fn(|_| s.i32());
            let n = s.usize();
            let cell = s.i32();
            if !(3..=65).contains(&n) || n.is_multiple_of(2) || !(1..=4).contains(&cell) {
                return None;
            }
            let w = world?;
            let limits = [w.nx, w.ny, w.nz];
            if (0..3).any(|k| {
                at[k] < 0
                    || at[k] as u32 >= limits[k]
                    || origin[k] as i64 != at[k] as i64 - (n as i64 / 2) * cell as i64
            }) {
                return None;
            }
            let input = Snapshot::load(s, world?)?;
            let cursor = s.usize();
            let resistance = s.u32s();
            let wave = if s.bool() { Some(Wave::load(s)?) } else { None };
            if cursor > n.pow(3) * (cell as usize).pow(3)
                || (wave.is_none() && resistance.len() != n.pow(3))
                || (wave.is_some()
                    && (cursor != n.pow(3) * (cell as usize).pow(3) || !resistance.is_empty()))
            {
                return None;
            }
            if wave.as_ref().is_some_and(|w| w.size() != n) {
                return None;
            }
            let count = s.usize();
            if count > n.pow(3) {
                return None;
            }
            let mut hits = VecDeque::new();
            for _ in 0..count {
                let cell = std::array::from_fn(|_| s.i32());
                let pressure = s.u32();
                let broken = s.bool();
                if cell.iter().any(|&v| v < 0 || v >= n as i32) || pressure > 1_000_000 {
                    return None;
                }
                hits.push_back(Hit {
                    cell,
                    pressure,
                    broken,
                });
            }
            let subcell = s.usize();
            let energy_mj = s.u64();
            let body_energy_mj = s.u64();
            let body_hit_done = s.bool();
            let body_field = crate::body_wave::Field::load(s, bodies, units, n)?;
            if subcell >= (cell as usize).pow(3)
                || !body_field.valid_domain(origin, cell, n, hits.front().copied())
                || (wave.is_some() && !body_field.finished())
            {
                return None;
            }
            out.jobs.push_back(Job {
                at,
                origin,
                n,
                cell,
                input,
                cursor,
                resistance,
                wave,
                hits,
                subcell,
                energy_mj,
                body_energy_mj,
                body_field,
                body_hit_done,
            });
        }
        let count = s.usize();
        if count > t.max_claims as usize {
            return None;
        }
        for _ in 0..count {
            let p: [u32; 3] = std::array::from_fn(|_| s.u32());
            let at = std::array::from_fn(|_| s.i32());
            let motion_mj = s.u64();
            let side = if two_cell_masks { 2 } else { s.u8() };
            let w = world?;
            if p[0] >= w.nx
                || p[1] >= w.ny
                || p[2] >= w.nz
                || !(1..=4).contains(&side)
                || out.claims.contains_key(&p)
            {
                return None;
            }
            let mat = if claim_materials {
                s.u16()
            } else {
                w.material(p[0], p[1], p[2])
            };
            // A claim expects a solid cell; a saved one holding air is
            // corrupt. An older save reads a cell another dig already
            // emptied as air: the claim's own dig will find nothing, and
            // its group holds its slot until then, as the save did.
            if claim_materials && mat == AIR {
                return None;
            }
            out.add_claim(
                p,
                Claim {
                    at,
                    motion_mj,
                    side,
                    mat,
                },
                t,
            );
        }
        let count = s.usize();
        let corpses = s.usize();
        let stored_limit =
            (t.max_fragments as usize).saturating_add(t.max_resting_fragments as usize);
        let record_bytes = if two_cell_masks { 152 } else { 160 };
        if count > stored_limit || count > s.left() / record_bytes || corpses > s.left() / 100 {
            return None;
        }
        let mut ids = BTreeSet::new();
        let mut helds = BTreeSet::new();
        for i in 0..count + corpses {
            let mut f = Fragment {
                id: s.u64(),
                position: load_v(s),
                velocity: load_v(s),
                material: s.u16(),
                mm: s.u16(),
                paint: if s.bool() {
                    Some([s.u8(), s.u8(), s.u8()])
                } else {
                    None
                },
                basis: std::array::from_fn(|_| load_v(s)),
                mass_g: s.u64(),
                count: s.u32(),
                shape: if two_cell_masks {
                    u64::from(s.u8())
                } else {
                    s.u64()
                },
                side: if two_cell_masks { 2 } else { s.u8() },
                chunk: None,
                held: s.u32(),
                resting: s.bool(),
            };
            if s.bool() {
                f.chunk = Some(Box::new(chunk::Chunk::load(s, bodies, f.mm, f.count)?));
            }
            if f.id == 0
                || f.id > out.next_id
                || !ids.insert(f.id)
                || !helds.insert(f.held)
                || f.count == 0
                || f.shape == 0
                || !(1..=4).contains(&f.side)
                || (f.side < 4 && f.shape >> box_cells(f.side) != 0)
                // A pile that fell again (ADR-291) flies with more cells
                // than its shape shows; a piece never has fewer.
                || (f.chunk.is_none() && f.count < f.shape.count_ones())
                || (f.material == AIR && f.paint.is_none())
                || f.mm == 0
                || f.mm > 1000
                || f.paint.is_some_and(|p| p[0] >= 8 || p[1] > 6 || p[2] > 4)
                || f.basis
                    .iter()
                    .any(|v| v.len_sq().sub(Fx::ONE).abs() > Fx::from_ratio(1, 100))
                || f.held == NO_TARGET
            {
                return None;
            }
            let w = world?;
            let limits = [w.nx, w.ny, w.nz];
            let vmax = Fx::from_ratio(
                t.max_speed_mm_s as i32,
                voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
            );
            if (0..3).any(|k| {
                arr(f.position)[k] < Fx::ZERO
                    || (k != 1 && arr(f.position)[k] >= Fx::from_int(limits[k] as i32))
                    || (k == 1 && arr(f.position)[k] > Fx::from_int(i32::MAX / 2))
            }) || arr(f.velocity).iter().any(|v| v.abs() > vmax)
                || f.mass_g == 0
            {
                return None;
            }
            let is_corpse = f.chunk.as_ref().is_some_and(|c| c.body.is_some());
            if is_corpse != (i >= count) {
                return None;
            }
            if is_corpse {
                out.corpses.insert(f.held, f);
            } else {
                out.fragments.push(f);
            }
        }
        // Storage slots do not admit active work. Do not accept a forged save
        // that disguises moving pieces/whole sections as sleeping history.
        (s.ok() && out.fragment_pools(t).working <= t.max_fragments as usize).then_some(out)
    }
}

/// A pile stood on the world under its centre: how tests lay one down.
#[cfg(test)]
pub(crate) fn heap(f: &mut Fragment, world: &voxel::World) {
    restack(f);
    if let Some(floor) = world_floor(f, world) {
        seat(f, floor);
    }
}
/// Stands a restacked pile with its bottom layer on `floor`.
#[cfg(test)]
fn seat(f: &mut Fragment, floor: Fx) {
    let scale = Fx::from_ratio(f.mm as i32, voxel::unit::WORLD_MM as i32);
    let centre = arr(shape_centre(f.shape, 4));
    f.position.y = floor.add(centre[1].add(Fx::from_ratio(1, 2)).mul(scale));
}
/// The top of the world under a piece's centre, looking down from its
/// height; `None` off the map.
#[cfg(test)]
fn world_floor(f: &Fragment, world: &voxel::World) -> Option<Fx> {
    let p = arr(f.position);
    let (x, z) = ((p[0].raw() >> 32) as i32, (p[2].raw() >> 32) as i32);
    if x < 0 || z < 0 || x as u32 >= world.nx || z as u32 >= world.nz {
        return None;
    }
    let mut y = (p[1].raw() >> 32).clamp(0, i64::from(world.ny) - 1) as i32;
    while y >= 0 {
        if world.solid(x as u32, y as u32, z as u32) {
            return Some(Fx::from_int(y + 1));
        }
        y -= 1;
    }
    Some(Fx::ZERO)
}
/// Preserve the floor conversion of Fx::to_int_trunc (its legacy name).
/// For a positive power-of-two cell, two arithmetic floor shifts combine.
struct PileGrid {
    divisor: i32,
    shift: Option<u32>,
}
impl PileGrid {
    fn new(cell: u32) -> Self {
        let divisor = cell as i32;
        Self {
            divisor,
            shift: (divisor > 0 && cell.is_power_of_two())
                .then(|| fx::FRAC_BITS + cell.trailing_zeros()),
        }
    }
    #[inline(always)]
    fn at(&self, position: V3) -> [i32; 3] {
        if let Some(shift) = self.shift {
            arr(position).map(|v| (v.raw() >> shift) as i32)
        } else {
            arr(position).map(|v| v.to_int_trunc().div_euclid(self.divisor))
        }
    }
}
/// A pile with more than one piece in it is a heap (ADR-292): its cells
/// fill a four-cell box from the bottom layer up, upright, so that it has
/// the height its matter takes. Matter beyond the box stays in the
/// container; the box only saturates. Where it stands is the caller's
/// (ADR-457).
#[cfg(test)]
fn restack(f: &mut Fragment) {
    let n = f.count.min(box_cells(4));
    f.shape = shape_geometry::STACKED[n as usize];
    f.side = 4;
    f.basis = IDENTITY;
}
/// A resting piece looks under itself every `rest_check_ticks` ticks
/// (ADR-291), the pieces spread over the period by id so that no tick
/// checks them all.
fn due(id: u64, tick: u64, t: &Blast) -> bool {
    rest_phase(id, tick, t) == 0
}
/// A power-of-two period needs no integer division. Other authored periods
/// retain the exact remainder. This is scheduling, never a change of cadence.
fn rest_phase(id: u64, tick: u64, t: &Blast) -> u64 {
    let period = u64::from(t.rest_check_ticks.max(1));
    let turn = id.wrapping_add(tick);
    if period.is_power_of_two() {
        turn & (period - 1)
    } else {
        turn % period
    }
}
/// Preparation and the support consumer share the same deadline. A missed
/// previous turn is phase one now; when the wrapped sum is zero, the current
/// turn is already due. Thus even overflow needs only one remainder (ADR-707).
fn support_due(id: u64, tick: u64, t: &Blast, damage_waiting: bool) -> bool {
    let phase = rest_phase(id, tick, t);
    phase == 0 || (damage_waiting && tick > 0 && phase == 1)
}
/// All predicates read the frozen phase input. Empty answers have no commit
/// effects, including no reservation changes; keep only actions and their
/// original indices. Chunk-index collection preserves admission order.
fn fragment_wake_queries<const COMPACT: bool>(
    fragments: &[Fragment],
    query: impl Fn((usize, &Fragment)) -> (Option<V3>, bool) + Sync,
    task: &(impl Fn(usize, &'static str) + Sync),
) -> Vec<(usize, (Option<V3>, bool))> {
    if !COMPACT {
        // Dense oracle retains every answer, including the no-op commits.
        return if fragments.len() >= 1024 {
            fragments
                .par_iter()
                .enumerate()
                .map_init(
                    || PhysicsTask::new(task, "physics_wake_begin", "physics_wake_end"),
                    |_, item| (item.0, query(item)),
                )
                .collect()
        } else {
            fragments
                .iter()
                .enumerate()
                .map(|item| (item.0, query(item)))
                .collect()
        };
    }
    let action = |(i, f)| {
        let answer = query((i, f));
        (answer.0.is_some() || answer.1).then_some((i, answer))
    };
    if fragments.len() < 1024 {
        return fragments.iter().enumerate().filter_map(action).collect();
    }
    let groups: Vec<Vec<_>> = fragments
        .par_chunks(256)
        .enumerate()
        .map(|(chunk, pieces)| {
            let _task = PhysicsTask::new(task, "physics_wake_begin", "physics_wake_end");
            pieces
                .iter()
                .enumerate()
                .filter_map(|(offset, f)| action((chunk * 256 + offset, f)))
                .collect()
        })
        .collect();
    let mut actions = Vec::with_capacity(groups.iter().map(Vec::len).sum());
    for group in groups {
        actions.extend(group);
    }
    actions
}
/// Whether material or a live hull still holds the piece up: its box half
/// a unit lower, the most a settling step moves, meets the world or a hull.
fn supported(
    f: &Fragment,
    world: &voxel::World,
    hulls: &[Hull],
    settled: &Settled,
    skip: u32,
) -> bool {
    let (low, high) = settled.fragment_bounds(skip, f);
    let mut p = arr(f.position);
    p[1] = p[1].sub(Fx::from_ratio(1, 2));
    fragment_contact(f, world, p, low, high, [world.nx, world.ny, world.nz])
        || hull_hit(p, low, high, hulls, 0).is_some()
        || settled.supports(arr(f.position), low, high, skip)
}
/// Canonical maxima in X/Z order. A dense gather scatters once by X, then
/// reduces Z into a bounded row; presence bits also retain zero-height
/// columns. Rows are independent and no ordering of equal inputs matters
/// to the exact maximum. Scratch is proportional to input, not world area.
#[cfg(test)]
fn merge_columns(columns: &mut Vec<([i32; 2], u16)>) {
    merge_columns_observed(columns, &|_, _| {}, &mut ColumnMergeScratch::default());
}
fn merge_columns_observed(
    columns: &mut Vec<([i32; 2], u16)>,
    task: &(impl Fn(usize, &'static str) + Sync),
    scratch: &mut ColumnMergeScratch,
) {
    debug_assert!(columns
        .iter()
        .all(|(c, _)| c.iter().all(|v| (0..1 << 16).contains(v))));
    if columns.len() >= 4096 {
        let mut maximum = [0usize; 2];
        for &(at, _) in columns.iter() {
            for a in 0..2 {
                maximum[a] = maximum[a].max(at[a] as usize);
            }
        }
        let words = maximum[1] / 64 + 1;
        if maximum[0] + maximum[1] + 2 <= columns.len() {
            let ColumnMergeScratch {
                starts,
                rows,
                groups,
                ..
            } = scratch;
            starts.clear();
            starts.resize(maximum[0] + 2, 0);
            for &(at, _) in columns.iter() {
                starts[at[0] as usize + 1] += 1;
            }
            for x in 0..starts.len() - 1 {
                starts[x + 1] += starts[x];
            }
            // X is implicit in the row. Pack its Z/top into four bytes,
            // instead of copying the twelve-byte full column twice.
            // Every current input overwrites one scatter slot before any read.
            rows.resize(columns.len(), 0);
            for &([x, z], top) in columns.iter() {
                let x = x as usize;
                rows[starts[x]] = ((z as u32) << 16) | u32::from(top);
                starts[x] += 1;
            }
            let parallel =
                rows.len() >= 16_384 && starts.len() > 512 && rayon::current_num_threads() > 1;
            // Each batch owns complete X rows, so maxima are scalar and
            // independent. Indexed writes and concatenation retain X/Z
            // order. Small maps and single-worker pools keep the serial path.
            let emit = |start: usize, ends: &[usize], stripe: &mut ColumnRows| {
                let ColumnRows {
                    tops,
                    present,
                    blocks,
                    out,
                    cache,
                } = stripe;
                cache.input_ends.clear();
                cache.output_ends.clear();
                out.clear();
                let _task = PhysicsTask::new(
                    task,
                    "physics_rubble_merge_rows_begin",
                    "physics_rubble_merge_rows_end",
                );
                let mut begin = if start == 0 { 0 } else { starts[start - 1] };
                if ends.last().is_none_or(|&end| end == begin) {
                    return;
                }
                out.reserve(ends.last().copied().unwrap() - begin);
                // Store top + 1: zero is absent, including when top is zero.
                // Reset touched entries on emission; no branch per duplicate.
                tops.resize(maximum[1] + 1, 0);
                present.resize(words, 0);
                blocks.resize(words.div_ceil(64), 0);
                // Scatter cursors are now the ends of their rows, including
                // empty ones. Emit only set bits, in ascending Z within each X.
                for (offset, &end) in ends.iter().enumerate() {
                    let x = start + offset;
                    let mut active = 0u64;
                    for &entry in &rows[begin..end] {
                        let z = (entry >> 16) as usize;
                        tops[z] = tops[z].max((entry & 0xffff) + 1);
                        let word = z / 64;
                        present[word] |= 1u64 << (z % 64);
                        blocks[word / 64] |= 1u64 << (word % 64);
                        active |= 1u64 << (word / 64);
                    }
                    // Two summary levels skip every empty word, even when two
                    // columns are at opposite edges of the 16-bit domain. Each
                    // visited word holds an output: traversal is linear in N.
                    while active != 0 {
                        let block = active.trailing_zeros() as usize;
                        let mut occupied = std::mem::take(&mut blocks[block]);
                        while occupied != 0 {
                            let word = block * 64 + occupied.trailing_zeros() as usize;
                            let mut bits = std::mem::take(&mut present[word]);
                            while bits != 0 {
                                let z = word * 64 + bits.trailing_zeros() as usize;
                                out.push(([x as i32, z as i32], (tops[z] - 1) as u16));
                                tops[z] = 0;
                                bits &= bits - 1;
                            }
                            occupied &= occupied - 1;
                        }
                        active &= active - 1;
                    }
                    begin = end;
                }
            };
            // Span gathering retains completed outputs; the legacy serial
            // allocation swap requires these logical buffers to be empty.
            for stripe in groups.iter_mut() {
                stripe.out.clear();
                stripe.cache.input_ends.clear();
                stripe.cache.output_ends.clear();
            }
            columns.clear();
            let ends = &starts[..starts.len() - 1];
            groups.resize_with(
                if parallel { ends.len().div_ceil(64) } else { 1 },
                ColumnRows::default,
            );
            if parallel {
                groups
                    .par_iter_mut()
                    .enumerate()
                    .for_each(|(batch, stripe)| {
                        let start = batch * 64;
                        emit(start, &ends[start..(start + 64).min(ends.len())], stripe);
                    });
            } else {
                // Keep the serial path on the input allocation, as before.
                std::mem::swap(columns, &mut groups[0].out);
                emit(0, ends, &mut groups[0]);
                std::mem::swap(columns, &mut groups[0].out);
            }
            // Logical outputs are drained in ascending X; only capacity survives.
            for stripe in groups {
                columns.append(&mut stripe.out);
            }
            return;
        }
    }
    sort_columns(columns);
    columns.dedup_by(|later, kept| {
        let same = later.0 == kept.0;
        if same {
            kept.1 = kept.1.max(later.1);
        }
        same
    });
}
/// Stable ascending columns (ADR-465/641). Dense maps use two full-axis
/// passes; small/sparse maps retain four byte passes, over Z and then X.
/// Coordinates lie in `0..65536`: the world is no wider than 16384.
fn sort_columns(columns: &mut Vec<([i32; 2], u16)>) {
    debug_assert!(columns
        .iter()
        .all(|(c, _)| c.iter().all(|v| (0..1 << 16).contains(v))));
    // Large rubble maps repeat many coordinates. Count both complete axes
    // from the immutable input once, then scatter Z and X stably. Histograms
    // are independent of the first scatter's order. Keep byte passes for
    // small/sparse inputs whose coordinate range costs more than their data.
    if columns.len() >= 4096 {
        let mut maximum = [0usize; 2];
        for &(at, _) in columns.iter() {
            for a in 0..2 {
                maximum[a] = maximum[a].max(at[a] as usize);
            }
        }
        if maximum[0] + maximum[1] + 2 <= columns.len() {
            let mut starts = maximum.map(|v| vec![0usize; v + 2]);
            for &(at, _) in columns.iter() {
                starts[0][at[0] as usize + 1] += 1;
                starts[1][at[1] as usize + 1] += 1;
            }
            for start in &mut starts {
                for d in 0..start.len() - 1 {
                    start[d + 1] += start[d];
                }
            }
            let mut out = vec![([0, 0], 0u16); columns.len()];
            for axis in [1, 0] {
                for c in columns.iter() {
                    let d = c.0[axis] as usize;
                    out[starts[axis][d]] = *c;
                    starts[axis][d] += 1;
                }
                std::mem::swap(columns, &mut out);
            }
            return;
        }
    }
    let mut out = vec![([0, 0], 0u16); columns.len()];
    for (axis, shift) in [(1, 0), (1, 8), (0, 0), (0, 8)] {
        let digit = |c: &([i32; 2], u16)| ((c.0[axis] >> shift) & 0xff) as usize;
        let mut start = [0usize; 257];
        for c in columns.iter() {
            start[digit(c) + 1] += 1;
        }
        for d in 0..256 {
            start[d + 1] += start[d];
        }
        for c in columns.iter() {
            let d = digit(c);
            out[start[d]] = *c;
            start[d] += 1;
        }
        std::mem::swap(columns, &mut out);
    }
}
/// The boxes of resting pieces, in the cells a flying piece may reach
/// (ADR-308). Built once a tick: a piece that landed is ground for the
/// next one, and without this they fall through one another.
#[derive(Default)]
pub(crate) struct Settled {
    /// Cell of `SETTLED_CELL` units to the indices of the pieces whose box covers
    /// it, in index order.
    cells: SettledCells,
    /// Frozen world boxes in original fragment order, including unindexed
    /// pieces. The cell lists use those indices throughout the tick, even
    /// when pieces wake or merge. Reuses the construction scratch buffer.
    boxes: Vec<([Fx; 3], [Fx; 3])>,
    /// Frozen geometry of indexed irregular pieces; broad boxes remain the
    /// first filter and single-cell/chunk contacts keep their existing path.
    shape_index: Vec<u32>,
    shapes: Vec<SettledShape>,
    /// Number of prepared query ranges, not a second physics state.
    requests: usize,
}
#[derive(Clone, Copy)]
struct SettledShape {
    pos: V3,
    basis: [V3; 3],
    centre: [Fx; 3],
    radius: [Fx; 3],
    scale: Fx,
    shape: u64,
    side: u8,
}
impl SettledShape {
    fn new(f: &Fragment) -> Self {
        let scale = Fx::from_ratio(f.mm as i32, voxel::unit::WORLD_MM as i32);
        let half = Fx::from_ratio(49, 100).mul(scale);
        let radius = std::array::from_fn(|a| {
            arr(f.basis[0])[a]
                .abs()
                .add(arr(f.basis[1])[a].abs())
                .add(arr(f.basis[2])[a].abs())
                .mul(half)
        });
        Self {
            pos: f.position,
            basis: f.basis,
            centre: arr(shape_centre(f.shape, f.side)),
            radius,
            scale,
            shape: f.shape,
            side: f.side,
        }
    }
    fn meets(&self, lo: [Fx; 3], hi: [Fx; 3]) -> bool {
        let mut bits = self.shape;
        while bits != 0 {
            let i = bits.trailing_zeros();
            bits &= bits - 1;
            if i >= box_cells(self.side) {
                continue;
            }
            let q = shape_cell(i, self.side);
            let mut offset = V3::ZERO;
            for a in 0..3 {
                offset = offset.add(
                    self.basis[a].scale(Fx::from_int(q[a]).sub(self.centre[a]).mul(self.scale)),
                );
            }
            let p = arr(self.pos.add(offset));
            if (0..3).all(|a| lo[a] < p[a].add(self.radius[a]) && p[a].sub(self.radius[a]) < hi[a])
            {
                return true;
            }
        }
        false
    }
    fn meets_pair(
        &self,
        lo: [Fx; 3],
        hi: [Fx; 3],
        other: &SettledShape,
        shift_lo: [Fx; 3],
        shift_hi: [Fx; 3],
    ) -> bool {
        let mut bits = self.shape;
        while bits != 0 {
            let i = bits.trailing_zeros();
            bits &= bits - 1;
            if i >= box_cells(self.side) {
                continue;
            }
            let q = shape_cell(i, self.side);
            let mut offset = V3::ZERO;
            for a in 0..3 {
                offset = offset.add(
                    self.basis[a].scale(Fx::from_int(q[a]).sub(self.centre[a]).mul(self.scale)),
                );
            }
            let p = arr(self.pos.add(offset));
            let cell_lo = [0, 1, 2].map(|a| p[a].sub(self.radius[a]));
            let cell_hi = [0, 1, 2].map(|a| p[a].add(self.radius[a]));
            if (0..3).all(|a| lo[a] < cell_hi[a] && cell_lo[a] < hi[a])
                && other.meets(
                    [0, 1, 2].map(|a| cell_lo[a].sub(shift_hi[a])),
                    [0, 1, 2].map(|a| cell_hi[a].sub(shift_lo[a])),
                )
            {
                return true;
            }
        }
        false
    }
}
/// A bounded bit mask replaces repeated tree lookups for dense container IDs.
/// Compaction and destruction keep their original slot order.
fn retain_live<T>(
    values: &mut Vec<T>,
    removed: &BTreeSet<u32>,
    held: impl Fn(&T) -> u32,
    indexed: bool,
) {
    if indexed && values.len() >= 1024 && removed.len() >= 8 {
        let first = *removed.first().expect("nonempty");
        let last = *removed.last().expect("nonempty");
        let span = u64::from(last) - u64::from(first) + 1;
        // Sparse or restored extreme IDs never cause an unbounded allocation.
        if span <= 262_144 && span.div_ceil(4) <= values.len() as u64 {
            let mut mask = vec![0u64; span.div_ceil(64) as usize];
            for &id in removed {
                let offset = id - first;
                mask[(offset / 64) as usize] |= 1u64 << (offset % 64);
            }
            values.retain(|v| {
                !held(v).checked_sub(first).is_some_and(|offset| {
                    mask.get((offset / 64) as usize)
                        .is_some_and(|word| word & (1u64 << (offset % 64)) != 0)
                })
            });
            return;
        }
    }
    values.retain(|v| !removed.contains(&held(v)));
}
/// Side of a cell of the settled index, world units.
const SETTLED_CELL: i32 = 4;
#[derive(Default)]
pub(crate) struct RubbleScratch {
    bounds: Vec<Option<([Fx; 3], [Fx; 3])>>,
    settled: Settled,
    queries: SettledQueries,
    stacked_mask: Vec<u64>,
    column_merge: ColumnMergeScratch,
    floor_proofs: Vec<Option<RubbleFloorProof>>,
}
/// Exact world-contact answer for upright geometry. Source slot/id and every
/// shape input are checked; dirty X/Z pages include a conservative margin.
#[derive(Clone, Copy)]
struct RubbleFloorProof {
    id: u64,
    position: V3,
    shape: u64,
    mm: u16,
    side: u8,
    low: [Fx; 3],
    high: [Fx; 3],
    lo: [i32; 2],
    hi: [i32; 2],
    on_world: bool,
}
impl RubbleFloorProof {
    fn matches(&self, f: &Fragment) -> bool {
        self.id == f.id
            && self.position == f.position
            && self.shape == f.shape
            && self.mm == f.mm
            && self.side == f.side
            && f.basis == IDENTITY
    }
    fn new(f: &Fragment, low: [Fx; 3], high: [Fx; 3], on_world: bool) -> Self {
        let p = arr(f.position);
        Self {
            id: f.id,
            position: f.position,
            shape: f.shape,
            mm: f.mm,
            side: f.side,
            low,
            high,
            lo: [0, 2].map(|a| p[a].add(low[a]).to_int_trunc() - 2),
            hi: [0, 2].map(|a| p[a].add(high[a]).to_int_trunc() + 2),
            on_world,
        }
    }
}
/// Scratch only: support membership is rebuilt from current inputs. Exact row
/// results may be reused only after comparing their complete current inputs.
#[derive(Default)]
struct ColumnMergeScratch {
    starts: Vec<usize>,
    rows: Vec<u32>,
    groups: Vec<ColumnRows>,
    spans: Vec<RubbleColumnSpan>,
    span_rows: Vec<RubbleRowSpan>,
    cover_keys: Vec<HullCoverKey>,
}
#[derive(Clone, Copy)]
struct RubbleColumnSpan {
    lo: [i32; 2],
    hi: [i32; 2],
    top: u16,
    hulls: u64,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct RubbleRowSpan {
    z: u32,
    top: u16,
    hulls: u64,
}
/// Exactly the fields read by Hull::covers/covers_point. Roof/voxel support
/// is classified afresh before gathering; it is never cached here.
#[derive(Clone, Copy, PartialEq, Eq)]
struct HullCoverKey {
    pos: [Fx; 2],
    half: [Fx; 2],
    head: i32,
}
#[derive(Default)]
struct ColumnRowCache {
    first_x: usize,
    inputs: Vec<RubbleRowSpan>,
    input_ends: Vec<usize>,
    output_ends: Vec<usize>,
    previous: Vec<([i32; 2], u16)>,
}
/// Gather exact footprint ranges before enumerating cells. Each X row then
/// consumes its ranges directly into maxima, avoiding a duplicate-heavy voxel
/// list and its second scatter. Unusual sparse/wide inputs keep the old builder.
#[allow(clippy::too_many_arguments)]
fn gather_column_spans(
    fragments: &[Fragment],
    bounds: &[Option<([Fx; 3], [Fx; 3])>],
    floor: &[usize],
    hulls: &[Hull],
    reach: &[([Fx; 2], [Fx; 2])],
    scratch: &mut ColumnMergeScratch,
    task: &(impl Fn(usize, &'static str) + Sync),
) -> Option<Vec<([i32; 2], u16)>> {
    let ColumnMergeScratch {
        starts,
        groups,
        spans,
        span_rows,
        cover_keys,
        ..
    } = scratch;
    {
        let _task = PhysicsTask::new(
            task,
            "physics_rubble_gather_dispatch_begin",
            "physics_rubble_gather_dispatch_end",
        );
        let prepare = |floor: &[usize]| {
            let _task = PhysicsTask::new(
                task,
                "physics_rubble_gather_begin",
                "physics_rubble_gather_end",
            );
            let mut out = Vec::with_capacity(floor.len());
            for &i in floor {
                let f = &fragments[i];
                let (low, high) = bounds[i].unwrap();
                let p = arr(f.position);
                let lo = [p[0].add(low[0]), p[2].add(low[2])].map(|v| (v.raw() >> 32) as i32);
                let hi = [p[0].add(high[0]), p[2].add(high[2])].map(|v| (v.raw() >> 32) as i32);
                let mut covers = 0u64;
                for (id, (h, (b0, b1))) in hulls.iter().zip(reach).enumerate() {
                    let near = (0..2).all(|a| {
                        Fx::from_int(lo[a]) <= b1[a] && Fx::from_int(hi[a]).add(Fx::ONE) >= b0[a]
                    });
                    if near && !h.covers_point([p[0], p[2]]) {
                        covers |= 1u64 << id;
                    }
                }
                let top = -(p[1].add(high[1]).neg().raw() >> 32);
                let lo = lo.map(|v| v.max(0));
                if (0..2).all(|a| lo[a] <= hi[a]) {
                    out.push(RubbleColumnSpan {
                        lo,
                        hi,
                        top: top.clamp(0, i64::from(u16::MAX)) as u16,
                        hulls: covers,
                    });
                }
            }
            out
        };
        spans.clear();
        let prepared: Vec<_> = floor.par_chunks(256).map(prepare).collect();
        for mut group in prepared {
            spans.append(&mut group);
        }
        let mut maximum = [0usize; 2];
        let mut cells = 0u64;
        for span in spans.iter() {
            if span.hi.iter().any(|&v| v >= 65_536) {
                return None;
            }
            for a in 0..2 {
                maximum[a] = maximum[a].max(span.hi[a] as usize);
            }
            cells += (span.hi[0] - span.lo[0] + 1) as u64 * (span.hi[1] - span.lo[1] + 1) as u64;
        }
        if cells < (maximum[0] + maximum[1] + 2) as u64 {
            return None;
        }
        starts.clear();
        starts.resize(maximum[0] + 2, 0);
        for span in spans.iter() {
            for x in span.lo[0]..=span.hi[0] {
                starts[x as usize + 1] += 1;
            }
        }
        for x in 0..starts.len() - 1 {
            starts[x + 1] += starts[x];
        }
        span_rows.resize(*starts.last().unwrap(), RubbleRowSpan::default());
        for span in spans.iter() {
            let row = RubbleRowSpan {
                z: ((span.lo[1] as u32) << 16) | span.hi[1] as u32,
                top: span.top,
                hulls: span.hulls,
            };
            for x in span.lo[0]..=span.hi[0] {
                span_rows[starts[x as usize]] = row;
                starts[x as usize] += 1;
            }
        }
    }
    let _task = PhysicsTask::new(
        task,
        "physics_rubble_merge_begin",
        "physics_rubble_merge_end",
    );
    let max_z = spans.iter().map(|s| s.hi[1] as usize).max().unwrap_or(0);
    let words = max_z / 64 + 1;
    let ends = &starts[..starts.len() - 1];
    let parallel =
        span_rows.len() >= 16_384 && ends.len() > 512 && rayon::current_num_threads() > 1;
    let keys: Vec<_> = hulls
        .iter()
        .map(|h| HullCoverKey {
            pos: [h.pos[0], h.pos[2]],
            half: h.half,
            head: h.head,
        })
        .collect();
    let mut changed_hulls = 0u64;
    for id in 0..keys.len().max(cover_keys.len()) {
        if keys.get(id) != cover_keys.get(id) {
            changed_hulls |= 1u64 << id;
        }
    }
    *cover_keys = keys;
    let emit = |start: usize, ends: &[usize], stripe: &mut ColumnRows| {
        let _task = PhysicsTask::new(
            task,
            "physics_rubble_merge_rows_begin",
            "physics_rubble_merge_rows_end",
        );
        // Keep last outputs while writing this call's output. Every reused row
        // compares full descriptors (not just a hash) and all its cover inputs.
        let cache = &mut stripe.cache;
        std::mem::swap(&mut stripe.out, &mut cache.previous);
        stripe.out.clear();
        stripe.tops.resize(max_z + 1, 0);
        stripe.present.resize(words, 0);
        stripe.blocks.resize(words.div_ceil(64), 0);
        let mut begin = if start == 0 { 0 } else { starts[start - 1] };
        let input_start = begin;
        let mut new_output_ends = Vec::with_capacity(ends.len());
        for (offset, &end) in ends.iter().enumerate() {
            let x = start + offset;
            let current = &span_rows[begin..end];
            let old_begin = if offset == 0 {
                0
            } else {
                cache.input_ends.get(offset - 1).copied().unwrap_or(0)
            };
            let same = cache.first_x == start
                && cache
                    .input_ends
                    .get(offset)
                    .is_some_and(|&old_end| cache.inputs.get(old_begin..old_end) == Some(current))
                && (changed_hulls == 0 || current.iter().all(|s| s.hulls & changed_hulls == 0));
            if same {
                let out_begin = if offset == 0 {
                    0
                } else {
                    cache.output_ends[offset - 1]
                };
                stripe
                    .out
                    .extend_from_slice(&cache.previous[out_begin..cache.output_ends[offset]]);
                new_output_ends.push(stripe.out.len());
                begin = end;
                continue;
            }
            let mut active = 0u64;
            for span in &span_rows[begin..end] {
                let top = u32::from(span.top) + 1;
                for z in (span.z >> 16) as usize..=(span.z & 0xffff) as usize {
                    if stripe.tops[z] >= top {
                        continue;
                    }
                    let mut mask = span.hulls;
                    let mut covered = false;
                    while mask != 0 {
                        let id = mask.trailing_zeros() as usize;
                        if hulls[id].covers(x as i32, z as i32) {
                            covered = true;
                            break;
                        }
                        mask &= mask - 1;
                    }
                    if covered {
                        continue;
                    }
                    stripe.tops[z] = top;
                    let word = z / 64;
                    stripe.present[word] |= 1u64 << (z % 64);
                    stripe.blocks[word / 64] |= 1u64 << (word % 64);
                    active |= 1u64 << (word / 64);
                }
            }
            while active != 0 {
                let block = active.trailing_zeros() as usize;
                let mut occupied = std::mem::take(&mut stripe.blocks[block]);
                while occupied != 0 {
                    let word = block * 64 + occupied.trailing_zeros() as usize;
                    let mut bits = std::mem::take(&mut stripe.present[word]);
                    while bits != 0 {
                        let z = word * 64 + bits.trailing_zeros() as usize;
                        stripe
                            .out
                            .push(([x as i32, z as i32], (stripe.tops[z] - 1) as u16));
                        stripe.tops[z] = 0;
                        bits &= bits - 1;
                    }
                    occupied &= occupied - 1;
                }
                active &= active - 1;
            }
            new_output_ends.push(stripe.out.len());
            begin = end;
        }
        cache.first_x = start;
        cache.inputs.clear();
        cache
            .inputs
            .extend_from_slice(&span_rows[input_start..begin]);
        cache.input_ends.clear();
        cache
            .input_ends
            .extend(ends.iter().map(|&end| end - input_start));
        cache.output_ends = new_output_ends;
    };
    groups.resize_with(
        if parallel { ends.len().div_ceil(64) } else { 1 },
        ColumnRows::default,
    );
    if parallel {
        groups
            .par_iter_mut()
            .enumerate()
            .for_each(|(batch, stripe)| {
                let start = batch * 64;
                emit(start, &ends[start..(start + 64).min(ends.len())], stripe);
            });
    } else {
        emit(0, ends, &mut groups[0]);
    }
    let mut columns = Vec::with_capacity(groups.iter().map(|s| s.out.len()).sum());
    for group in groups {
        columns.extend_from_slice(&group.out);
    }
    Some(columns)
}
#[derive(Default)]
struct ColumnRows {
    tops: Vec<u32>,
    present: Vec<u64>,
    blocks: Vec<u64>,
    out: Vec<([i32; 2], u16)>,
    cache: ColumnRowCache,
}
// Bounds remain in the immutable selected slice. A stripe needs only the
// source and its first owned X, rather than another copy of both corners.
type SettledCellRequest = (u32, i32);
/// X modulo eight owns each contact list. Each worker prepends indices in
/// reverse fragment order, retaining ascending source order without a tail.
/// No merge or shared insertion is needed.
#[derive(Default)]
struct SettledCells {
    stripes: [SettledCellStripe; 8],
    requests: [Vec<SettledCellRequest>; 8],
    len: usize,
}
#[derive(Default)]
struct SettledCellStripe {
    cells: FxLookup<[i32; 3], usize>,
    nodes: Vec<SettledCellNode>,
    wide: BTreeMap<usize, usize>,
}
struct SettledCellNode {
    source: u32,
    next: u32,
}
struct SettledCellList<'a> {
    nodes: &'a [SettledCellNode],
    wide: &'a BTreeMap<usize, usize>,
    // One-based arena slot; zero terminates without an overflow lookup.
    next: usize,
}
impl Iterator for SettledCellList<'_> {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        if self.next == 0 {
            return None;
        }
        let index = self.next - 1;
        let node = &self.nodes[index];
        self.next = if node.next == u32::MAX {
            self.wide.get(&index).copied().expect("wide settled link") + 1
        } else {
            node.next as usize
        };
        Some(node.source)
    }
}
impl SettledCells {
    fn clear(&mut self) {
        self.clear_parallel(false);
    }
    fn clear_parallel(&mut self, parallel: bool) {
        let clear = |stripe: &mut SettledCellStripe| {
            stripe.cells.clear();
            stripe.nodes.clear();
            stripe.wide.clear();
        };
        if parallel {
            self.stripes.par_iter_mut().for_each(clear);
        } else {
            self.stripes.iter_mut().for_each(clear);
        }
        self.len = 0;
    }
    fn is_empty(&self) -> bool {
        self.len == 0
    }
    #[cfg(test)]
    fn len(&self) -> usize {
        self.len
    }
    fn get(&self, cell: &[i32; 3]) -> Option<SettledCellList<'_>> {
        let stripe = &self.stripes[cell[0].rem_euclid(8) as usize];
        stripe.cells.get(cell).map(|head| SettledCellList {
            nodes: &stripe.nodes,
            wide: &stripe.wide,
            next: *head + 1,
        })
    }
    #[cfg(test)]
    fn ordered(&self) -> BTreeMap<[i32; 3], Vec<u32>> {
        self.stripes
            .iter()
            .flat_map(|stripe| {
                stripe
                    .cells
                    .sorted_entries()
                    .into_iter()
                    .map(|(&cell, head)| {
                        (
                            cell,
                            SettledCellList {
                                nodes: &stripe.nodes,
                                wide: &stripe.wide,
                                next: *head + 1,
                            }
                            .collect(),
                        )
                    })
            })
            .collect()
    }
    fn build(
        &mut self,
        selected: &[Option<([i32; 3], [i32; 3])>],
        want: &SettledQueries,
        parallel: bool,
        task: &(impl Fn(usize, &'static str) + Sync),
    ) {
        self.build_links::<{ u32::MAX }>(selected, want, parallel, task);
    }
    // A smaller test limit exercises real wide links without a multi-GiB
    // arena. Production reserves u32::MAX for wide links; zero terminates.
    fn build_links<const COMPACT_LIMIT: u32>(
        &mut self,
        selected: &[Option<([i32; 3], [i32; 3])>],
        want: &SettledQueries,
        parallel: bool,
        task: &(impl Fn(usize, &'static str) + Sync),
    ) {
        {
            let _task = PhysicsTask::new(
                task,
                "physics_cell_partition_begin",
                "physics_cell_partition_end",
            );
            for requests in &mut self.requests {
                requests.clear();
            }
            // Visit each source once. The first eight X cells name exactly
            // the stripes intersecting this box, including negative cells.
            // Appending in source order preserves every contact list's order.
            for (i, bounds) in selected.iter().enumerate() {
                let Some((lo, hi)) = *bounds else {
                    continue;
                };
                for x in (lo[0]..=hi[0]).take(8) {
                    self.requests[x.rem_euclid(8) as usize].push((i as u32, x));
                }
            }
        }
        let fill = |(stripe, requests): (&mut SettledCellStripe, &Vec<SettledCellRequest>)| {
            let _task = PhysicsTask::new(task, "physics_cells_begin", "physics_cells_end");
            // Prepend sources in reverse order: each final contact list is
            // still in canonical source order, without a tail or a second
            // arena write to patch the preceding source's successor.
            for &(i, first_x) in requests.iter().rev() {
                let (lo, hi) = selected[i as usize].expect("selected settled source");
                for z in lo[2]..=hi[2] {
                    for x in (first_x..=hi[0]).step_by(8) {
                        for y in want.column(x, z, lo[1], hi[1]) {
                            let index = stripe.nodes.len();
                            let head = stripe.cells.entry([x, y, z]).or_insert(usize::MAX);
                            let next = if *head == usize::MAX {
                                0
                            } else if *head + 1 < COMPACT_LIMIT as usize {
                                (*head + 1) as u32
                            } else {
                                // The compact sentinel never truncates a
                                // full-width successor; tests force this path.
                                stripe.wide.insert(index, *head);
                                u32::MAX
                            };
                            *head = index;
                            stripe.nodes.push(SettledCellNode { source: i, next });
                        }
                    }
                }
            }
        };
        if parallel {
            self.stripes
                .par_iter_mut()
                .zip(&self.requests)
                .for_each(fill);
        } else {
            self.stripes.iter_mut().zip(&self.requests).for_each(fill);
        }
        self.len = self.stripes.iter().map(|stripe| stripe.cells.len()).sum();
    }
}
/// Membership-only query mask. A word covers 64 vertical index cells;
/// insertion ORs a range instead of hashing every Y coordinate. No path
/// iterates the hash table: contact order comes from the fragment slice.
#[derive(Default)]
struct SettledQueries([SettledQueryStripe; 8]);
#[derive(Default)]
struct SettledQueryStripe {
    // Word zero covers 64 index cells (256 world units). Its word coordinate
    // is implicit; full signed X/Z still take exactly eight bytes. The tuple
    // hashes both coordinates separately, avoiding the poor low-bit spread
    // of concatenating them into one integer with FxHasher 1.1.
    base: FxLookup<(i32, i32), u64>,
    // Other heights, including negative margins, retain the exact 3D key.
    other: FxLookup<[i32; 3], u64>,
    dense: Option<Box<DenseQueryWords>>,
}
#[derive(Clone, Copy)]
struct DenseQueryPlane {
    x: i32,
    z: i32,
    nx: usize,
    nz: usize,
}
impl DenseQueryPlane {
    // The caller selects this stripe by X modulo eight.
    fn index(self, x: i32, z: i32) -> Option<(usize, usize)> {
        let dx = i64::from(x) - i64::from(self.x);
        let dz = i64::from(z) - i64::from(self.z);
        if dx < 0 || dz < 0 {
            return None;
        }
        let (x, z) = ((dx as usize) >> 3, dz as usize);
        (x < self.nx && z < self.nz)
            .then(|| ((z >> 4) * self.tile_nx() + (x >> 2), (z & 15) * 4 + (x & 3)))
    }
    fn tile_nx(self) -> usize {
        self.nx.div_ceil(4)
    }
    fn point(self, tile: usize, offset: usize) -> (i32, i32) {
        (
            (i64::from(self.x) + ((tile % self.tile_nx()) * 4 + offset % 4) as i64 * 8) as i32,
            (i64::from(self.z) + ((tile / self.tile_nx()) * 16 + offset / 4) as i64) as i32,
        )
    }
}
#[derive(Default)]
struct DenseQueryWords {
    plane: Option<DenseQueryPlane>,
    // Zero is an empty page; other values are its first word index plus one.
    // A stripe page covers four owned X columns by sixteen Z rows.
    tiles: Vec<u32>,
    words: Vec<u64>,
    entries: usize,
}
impl DenseQueryWords {
    const PAGE_WORDS: usize = 64;
    const MAX_WORDS: usize = 1 << 17;

    fn for_each_word(&self, mut visit: impl FnMut((i32, i32), u64)) {
        if let Some(plane) = self.plane {
            for (tile, &start) in self.tiles.iter().enumerate() {
                if start != 0 {
                    for (offset, &word) in self.words[start as usize - 1..][..Self::PAGE_WORDS]
                        .iter()
                        .enumerate()
                    {
                        if word != 0 {
                            visit(plane.point(tile, offset), word);
                        }
                    }
                }
            }
        }
    }
}
impl SettledQueryStripe {
    fn clear(&mut self) {
        self.base.clear();
        self.other.clear();
        if let Some(dense) = &mut self.dense {
            dense.plane = None;
            dense.tiles.clear();
            dense.words.clear();
            dense.entries = 0;
        }
    }
    fn base_len(&self) -> usize {
        self.base.len() + self.dense.as_ref().map_or(0, |d| d.entries)
    }
    fn base_word(&self, x: i32, z: i32) -> u64 {
        if let Some(dense) = &self.dense {
            if let Some(plane) = dense.plane {
                return plane.index(x, z).map_or(0, |(tile, offset)| {
                    let start = dense.tiles[tile];
                    if start == 0 {
                        0
                    } else {
                        dense.words[start as usize - 1 + offset]
                    }
                });
            }
        }
        self.base.get(&(x, z)).copied().unwrap_or(0)
    }
    fn include_base_row(&mut self, x: i32, hi_x: i32, z: i32, mask: u64) {
        if let Some(dense) = &mut self.dense {
            if let Some(plane) = dense.plane {
                let (mut tile, mut offset) =
                    plane.index(x, z).expect("query inside prepared plane");
                let mut remaining = ((i64::from(hi_x) - i64::from(x)) / 8 + 1) as usize;
                debug_assert_ne!(mask, 0);
                while remaining != 0 {
                    let mut start = dense.tiles[tile];
                    if start == 0 {
                        if dense.words.len() == DenseQueryWords::MAX_WORDS {
                            break;
                        }
                        start = dense.words.len() as u32 + 1;
                        dense
                            .words
                            .resize(dense.words.len() + DenseQueryWords::PAGE_WORDS, 0);
                        dense.tiles[tile] = start;
                    }
                    let len = remaining.min(4 - (offset & 3));
                    let first = start as usize - 1 + offset;
                    for word in &mut dense.words[first..first + len] {
                        dense.entries += usize::from(*word == 0);
                        *word |= mask;
                    }
                    remaining -= len;
                    tile += 1;
                    offset &= !3;
                }
                if remaining == 0 {
                    return;
                }
                // Replay the whole row after spilling; OR preserves the
                // prefix already written before reaching the page limit.
                self.spill_dense();
            }
        }
        for x in (x..=hi_x).step_by(8) {
            *self.base.entry((x, z)).or_default() |= mask;
        }
    }
    fn spill_dense(&mut self) {
        if let Some(dense) = &mut self.dense {
            let base = &mut self.base;
            dense.for_each_word(|point, word| *base.entry(point).or_default() |= word);
            dense.plane = None;
            dense.tiles.clear();
            dense.words.clear();
            dense.entries = 0;
        }
    }
    fn prepare(&mut self, stripe: i32, bounds: Option<([i32; 2], [i32; 2])>) {
        // Preserve the old additive build contract too. Production clears
        // each generation; an appended build converts existing dense words
        // in deterministic row order before continuing in the sparse table.
        self.spill_dense();
        if !self.base.is_empty() {
            return;
        }
        let Some((lo, hi)) = bounds else { return };
        let x = i64::from(lo[0]) + (i64::from(stripe) - i64::from(lo[0])).rem_euclid(8);
        if x > i64::from(hi[0]) {
            return;
        }
        let plane = DenseQueryPlane {
            x: x as i32,
            z: lo[1],
            nx: ((i64::from(hi[0]) - x) / 8 + 1) as usize,
            nz: (i64::from(hi[1]) - i64::from(lo[1]) + 1) as usize,
        };
        let dense = self.dense.get_or_insert_with(Default::default);
        dense
            .tiles
            .resize(plane.tile_nx() * plane.nz.div_ceil(16), 0);
        dense.plane = Some(plane);
    }
}
#[cfg(test)]
impl SettledQueryStripe {
    fn ordered(&self) -> Vec<([i32; 3], u64)> {
        let mut words: BTreeMap<_, _> = self
            .base
            .sorted_entries()
            .into_iter()
            .map(|(&(x, z), &mask)| ([x, 0, z], mask))
            .collect();
        for (&key, &mask) in self.other.sorted_entries() {
            assert!(words.insert(key, mask).is_none());
        }
        if let Some(dense) = &self.dense {
            dense.for_each_word(|(x, z), mask| assert!(words.insert([x, 0, z], mask).is_none()));
        }
        words.into_iter().collect()
    }
}
impl SettledQueries {
    // Page-directory initialization is a fixed cost across the whole bbox.
    // Keep smaller batches on the hash path, including ordinary early battles.
    const PAGED_MIN_RANGES: usize = 8192;

    fn clear(&mut self) {
        self.clear_parallel(false);
    }
    fn clear_parallel(&mut self, parallel: bool) {
        if parallel {
            self.0.par_iter_mut().for_each(SettledQueryStripe::clear);
        } else {
            self.0.iter_mut().for_each(SettledQueryStripe::clear);
        }
    }
    fn is_empty(&self) -> bool {
        self.0
            .iter()
            .all(|stripe| stripe.base_len() == 0 && stripe.other.is_empty())
    }
    fn word_counts(&self) -> [usize; 2] {
        let mut counts = [0; 2];
        for stripe in &self.0 {
            counts[0] += stripe.base_len();
            counts[1] += stripe.other.len();
        }
        counts
    }
    fn build(
        &mut self,
        ranges: &[([i32; 3], [i32; 3])],
        parallel: bool,
        task: &(impl Fn(usize, &'static str) + Sync),
    ) {
        let dense_bounds = {
            let _task = PhysicsTask::new(
                task,
                "physics_queries_layout_begin",
                "physics_queries_layout_end",
            );
            if parallel && ranges.len() >= Self::PAGED_MIN_RANGES {
                Self::dense_bounds(ranges)
            } else {
                None
            }
        };
        // X modulo eight owns the word, so workers never share a table and
        // need no unordered merge. Ranges are captured in fragment order.
        let fill = |(stripe, table): (usize, &mut SettledQueryStripe)| {
            let _task = PhysicsTask::new(task, "physics_queries_begin", "physics_queries_end");
            table.prepare(stripe as i32, dense_bounds);
            let active = table.dense.as_ref().is_some_and(|d| d.plane.is_some());
            let _mode = PhysicsTask::new(
                task,
                if active {
                    "physics_queries_dense_begin"
                } else {
                    "physics_queries_sparse_begin"
                },
                if active {
                    "physics_queries_dense_end"
                } else {
                    "physics_queries_sparse_end"
                },
            );
            for &(lo, hi) in ranges {
                Self::include(table, stripe as i32, lo, hi);
            }
        };
        if parallel {
            self.0.par_iter_mut().enumerate().for_each(fill);
        } else {
            self.0.iter_mut().enumerate().for_each(fill);
        }
    }
    /// Bounded page directory; word storage is allocated only on first touch.
    fn dense_bounds(ranges: &[([i32; 3], [i32; 3])]) -> Option<([i32; 2], [i32; 2])> {
        let (mut lo, mut hi) = ([i32::MAX; 2], [i32::MIN; 2]);
        let mut present = false;
        for &(a, b) in ranges {
            if (0..3).any(|k| a[k] > b[k]) {
                return None;
            }
            if a[1] > 63 || b[1] < 0 {
                continue;
            }
            present = true;
            for (k, axis) in [0, 2].into_iter().enumerate() {
                lo[k] = lo[k].min(a[axis]);
                hi[k] = hi[k].max(b[axis]);
            }
        }
        if !present {
            return None;
        }
        let nx = (i64::from(hi[0]) - i64::from(lo[0]) + 1) as u64;
        let nz = (i64::from(hi[1]) - i64::from(lo[1]) + 1) as u64;
        // At most 1 MiB of page-directory entries across all eight stripes,
        // plus at most 1 MiB of words per stripe. Huge coordinate domains
        // retain the exact signed-key hash representation immediately.
        let pages = 8u64
            .checked_mul(nx.div_ceil(32))?
            .checked_mul(nz.div_ceil(16))?;
        (pages <= 1 << 18).then_some((lo, hi))
    }
    fn include(table: &mut SettledQueryStripe, stripe: i32, lo: [i32; 3], hi: [i32; 3]) {
        // The first owned column may lie past i32::MAX. Compare before
        // narrowing; both signed endpoints remain valid sparse keys.
        let x0 = i64::from(lo[0]) + (i64::from(stripe) - i64::from(lo[0])).rem_euclid(8);
        if x0 > i64::from(hi[0]) {
            return;
        }
        let x0 = x0 as i32;
        let first = lo[1].div_euclid(64);
        let last = hi[1].div_euclid(64);
        for z in lo[2]..=hi[2] {
            for word in first..=last {
                let low = if word == first {
                    lo[1].rem_euclid(64)
                } else {
                    0
                };
                let high = if word == last {
                    hi[1].rem_euclid(64)
                } else {
                    63
                };
                let mask = (u64::MAX << low) & (u64::MAX >> (63 - high));
                if word == 0 {
                    table.include_base_row(x0, hi[0], z, mask);
                } else {
                    for x in (x0..=hi[0]).step_by(8) {
                        *table.other.entry([x, word, z]).or_default() |= mask;
                    }
                }
            }
        }
    }
    fn word(&self, x: i32, z: i32, word: i32) -> u64 {
        let stripe = &self.0[x.rem_euclid(8) as usize];
        if word == 0 {
            stripe.base_word(x, z)
        } else {
            stripe.other.get(&[x, word, z]).copied().unwrap_or(0)
        }
    }
    #[cfg(test)]
    fn contains(&self, [x, y, z]: [i32; 3]) -> bool {
        self.word(x, z, y.div_euclid(64)) & (1 << y.rem_euclid(64)) != 0
    }
    /// Read each intersecting word once, then yield only its requested bits.
    /// The full signed height domain remains available, including word zero.
    fn column(&self, x: i32, z: i32, low: i32, high: i32) -> impl Iterator<Item = i32> + '_ {
        let first = low.div_euclid(64);
        let last = high.div_euclid(64);
        (first..=last).flat_map(move |word| {
            let start = if word == first { low.rem_euclid(64) } else { 0 };
            let end = if word == last {
                high.rem_euclid(64)
            } else {
                63
            };
            let mut mask = self.word(x, z, word) & (u64::MAX << start) & (u64::MAX >> (63 - end));
            std::iter::from_fn(move || {
                if mask == 0 {
                    return None;
                }
                let y = word * 64 + mask.trailing_zeros() as i32;
                mask &= mask - 1;
                Some(y)
            })
        })
    }
    fn intersects(&self, lo: [i32; 3], hi: [i32; 3]) -> bool {
        let first = lo[1].div_euclid(64);
        if first == hi[1].div_euclid(64) {
            // Most fragment boxes occupy one vertical word. Membership
            // needs only its masked bits, not a column iterator or a Y
            // coordinate. Share the range mask across every X/Z column.
            let mask =
                (u64::MAX << lo[1].rem_euclid(64)) & (u64::MAX >> (63 - hi[1].rem_euclid(64)));
            return (lo[2]..=hi[2])
                .any(|z| (lo[0]..=hi[0]).any(|x| self.word(x, z, first) & mask != 0));
        }
        (lo[2]..=hi[2])
            .any(|z| (lo[0]..=hi[0]).any(|x| self.column(x, z, lo[1], hi[1]).next().is_some()))
    }
}
/// Loose cells grouped by fragment box and material, as cell masks.
fn crumb_boxes(cells: &[([u32; 3], Mat)], side: u8) -> BTreeMap<([u32; 3], Mat), u64> {
    let s = u32::from(side);
    let mut groups: BTreeMap<([u32; 3], Mat), u64> = BTreeMap::new();
    for &(p, m) in cells {
        if m == AIR {
            continue;
        }
        let origin = p.map(|v| v / s * s);
        let sub = (p[0] - origin[0]) + s * (p[1] - origin[1]) + s * s * (p[2] - origin[2]);
        *groups.entry((origin, m)).or_default() |= 1u64 << sub;
    }
    groups
}
fn settled_cells(lo: [Fx; 3], hi: [Fx; 3]) -> ([i32; 3], [i32; 3]) {
    let cell = |v: Fx| v.to_int_trunc().div_euclid(SETTLED_CELL);
    (
        [0, 1, 2].map(|a| cell(lo[a])),
        [0, 1, 2].map(|a| cell(hi[a])),
    )
}
/// The index contains only cells its immutable caller will actually query.
/// Motion needs a swept box and gravity margin; rubble asks only the box
/// lowered by half a unit. Contact geometry and insertion order are unchanged.
#[derive(Clone, Copy)]
enum SettledReach {
    Motion,
    Support,
}
impl SettledReach {
    fn cells(self, f: &Fragment, (mut lo, hi): ([Fx; 3], [Fx; 3])) -> ([i32; 3], [i32; 3]) {
        match self {
            Self::Support => {
                lo[1] = lo[1].sub(Fx::from_ratio(1, 2));
                settled_cells(lo, hi)
            }
            Self::Motion => {
                // The full existing reach is required by flying pieces:
                // velocity plus a cell beyond for gravity (ADR-438).
                let v = arr(f.velocity);
                let reach_lo = [0, 1, 2].map(|a| lo[a].add(v[a].min(Fx::ZERO)));
                let reach_hi = [0, 1, 2].map(|a| hi[a].add(v[a].max(Fx::ZERO)));
                let (c0, c1) = settled_cells(reach_lo, reach_hi);
                (c0.map(|v| v - 1), c1.map(|v| v + 1))
            }
        }
    }
}
impl Settled {
    /// The world boxes were built from this immutable phase input. Subtract
    /// that same centre using wrapping Fx arithmetic: (p + bound) - p is
    /// exactly bound, including fractional and overflow edge cases. Wake
    /// changes velocity/resting only; each translation reads its start pose.
    /// An empty/unbuilt index and corpse queries retain direct geometry.
    fn fragment_bounds(&self, index: u32, f: &Fragment) -> ([Fx; 3], [Fx; 3]) {
        self.boxes.get(index as usize).map_or_else(
            || f.bounds(),
            |&(lo, hi)| {
                let p = arr(f.position);
                (
                    [0, 1, 2].map(|a| lo[a].sub(p[a])),
                    [0, 1, 2].map(|a| hi[a].sub(p[a])),
                )
            },
        )
    }

    /// The resting pieces around those that will ask about them this tick:
    /// the flying ones, and the resting ones whose turn it is to look for
    /// their support (ADR-291). Pieces on a hull's roof (ADR-293) are left
    /// out: they ride, they are not ground.
    #[cfg(test)]
    fn of(fragments: &[Fragment], hulls: &[Hull], tick: u64, t: &Blast) -> Settled {
        Self::around(fragments, hulls, |_, f| !f.resting || due(f.id, tick, t))
    }
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)] // Independent phase and worker observers.
    fn of_into(
        fragments: &[Fragment],
        hulls: &[Hull],
        tick: u64,
        t: &Blast,
        out: &mut Settled,
        want: &mut SettledQueries,
        completed: &mut impl FnMut(&'static str),
        task: &(impl Fn(usize, &'static str) + Sync),
    ) {
        Self::for_phase_into(fragments, hulls, tick, t, false, out, want, completed, task);
    }
    #[allow(clippy::too_many_arguments)]
    fn for_phase_into(
        fragments: &[Fragment],
        hulls: &[Hull],
        tick: u64,
        t: &Blast,
        damage_waiting: bool,
        out: &mut Settled,
        want: &mut SettledQueries,
        completed: &mut impl FnMut(&'static str),
        task: &(impl Fn(usize, &'static str) + Sync),
    ) {
        Self::around_bounds_into(
            fragments,
            hulls,
            |_, f| !f.resting || support_due(f.id, tick, t, damage_waiting),
            |_, f| f.bounds(),
            SettledReach::Motion,
            out,
            want,
            completed,
            task,
        );
    }
    /// The resting pieces around those `asks` picks, by index and piece;
    /// a hull's riders left out as in [`Settled::of`].
    #[cfg(test)]
    fn around(
        fragments: &[Fragment],
        hulls: &[Hull],
        asks: impl Fn(usize, &Fragment) -> bool + Sync,
    ) -> Settled {
        Self::around_bounds(
            fragments,
            hulls,
            asks,
            |_, f| f.bounds(),
            SettledReach::Motion,
            &|_, _| {},
        )
    }
    /// A caller that already measured pieces can supply the same local
    /// boxes. They must describe this exact immutable fragment slice.
    #[cfg(test)]
    fn around_bounds(
        fragments: &[Fragment],
        hulls: &[Hull],
        asks: impl Fn(usize, &Fragment) -> bool + Sync,
        bounds: impl Fn(usize, &Fragment) -> ([Fx; 3], [Fx; 3]) + Sync,
        reach: SettledReach,
        task: &(impl Fn(usize, &'static str) + Sync),
    ) -> Settled {
        let mut out = Settled::default();
        Self::around_bounds_into(
            fragments,
            hulls,
            asks,
            bounds,
            reach,
            &mut out,
            &mut SettledQueries::default(),
            &mut |_| {},
            task,
        );
        out
    }
    #[allow(clippy::too_many_arguments)] // Independent phase and worker observers.
    fn around_bounds_into(
        fragments: &[Fragment],
        hulls: &[Hull],
        asks: impl Fn(usize, &Fragment) -> bool + Sync,
        bounds: impl Fn(usize, &Fragment) -> ([Fx; 3], [Fx; 3]) + Sync,
        reach: SettledReach,
        out: &mut Settled,
        want: &mut SettledQueries,
        completed: &mut impl FnMut(&'static str),
        task: &(impl Fn(usize, &'static str) + Sync),
    ) {
        let parallel = fragments.len() >= 1024;
        out.cells.clear_parallel(parallel);
        out.shapes.clear();
        out.shape_index.clear();
        out.requests = 0;
        want.clear_parallel(parallel);
        completed("settled_clear");
        if !fragments.iter().any(|f| f.resting) {
            out.boxes.clear();
            return;
        }
        // Only membership is queried. Fragment insertion and contact order
        // still follow the ordered fragment slice, not the hash table.
        out.boxes
            .resize(fragments.len(), ([Fx::ZERO; 3], [Fx::ZERO; 3]));
        let world_box = |(i, f): (usize, &Fragment)| {
            let (low, high) = bounds(i, f);
            let p = arr(f.position);
            let lo = [0, 1, 2].map(|a| p[a].add(low[a]));
            let hi = [0, 1, 2].map(|a| p[a].add(high[a]));
            (lo, hi)
        };
        let ranges = if fragments.len() >= 1024 {
            // Both reads belong to the same immutable fragment. A worker
            // writes only its boxes and owns its compact list of requests.
            // Collect by chunk index, then preserve fragment order on join.
            let groups: Vec<Vec<_>> = out
                .boxes
                .par_chunks_mut(256)
                .enumerate()
                .map(|(chunk, boxes)| {
                    let _task = PhysicsTask::new(task, "physics_boxes_begin", "physics_boxes_end");
                    let mut ranges = Vec::new();
                    for (offset, box_at) in boxes.iter_mut().enumerate() {
                        let i = chunk * 256 + offset;
                        let f = &fragments[i];
                        *box_at = world_box((i, f));
                        if asks(i, f) {
                            ranges.push(reach.cells(f, *box_at));
                        }
                    }
                    ranges
                })
                .collect();
            completed("settled_boxes");
            let mut ranges = Vec::with_capacity(groups.iter().map(Vec::len).sum());
            for group in groups {
                ranges.extend(group);
            }
            ranges
        } else {
            for (i, box_at) in out.boxes.iter_mut().enumerate() {
                *box_at = world_box((i, &fragments[i]));
            }
            completed("settled_boxes");
            fragments
                .iter()
                .enumerate()
                .filter(|&(i, f)| asks(i, f))
                .map(|(i, f)| reach.cells(f, out.boxes[i]))
                .collect()
        };
        out.requests = ranges.len();
        completed("settled_ranges");
        want.build(
            &ranges,
            fragments.len() >= 1024 && ranges.len() >= 128,
            task,
        );
        completed("settled_queries");
        if want.is_empty() {
            out.boxes.clear();
            return;
        }
        // Most resting pieces are nowhere near a live hull. Reject them
        // against a conservative box before doing the rotated voxel roof
        // query, without changing which hull can carry a piece.
        let hull_boxes: Vec<_> = hulls.iter().map(Hull::footprint_box).collect();
        let candidate = |(i, f): (usize, &Fragment)| {
            if !f.resting {
                return None;
            }
            let (lo, hi) = out.boxes[i];
            let (c0, c1) = settled_cells(lo, hi);
            if !want.intersects(c0, c1) {
                return None;
            }
            // The test asked whether the piece's centre lay inside a
            // hull's box, and the centre of a piece on a roof lies above
            // it: riders held what fell on them (ADR-458). Asked of the
            // pieces the queries reach alone: a hull's voxels answer it
            // (ADR-469).
            if hulls.iter().zip(&hull_boxes).any(|(h, (min, max))| {
                let x = f.position.x;
                let z = f.position.z;
                x >= min[0]
                    && x <= max[0]
                    && z >= min[1]
                    && z <= max[1]
                    && h.carries(lo[1], f.position)
            }) {
                return None;
            }
            Some((c0, c1))
        };
        let selected: Vec<_> = if fragments.len() >= 1024 {
            fragments
                .par_iter()
                .enumerate()
                .map_init(
                    || PhysicsTask::new(task, "physics_candidates_begin", "physics_candidates_end"),
                    |_, item| candidate(item),
                )
                .collect()
        } else {
            fragments.iter().enumerate().map(candidate).collect()
        };
        completed("settled_candidates");
        let Settled {
            cells,
            shape_index,
            shapes,
            ..
        } = out;
        let mut build_shapes = || {
            shape_index.resize(fragments.len(), u32::MAX);
            for (i, entry) in selected.iter().enumerate() {
                let f = &fragments[i];
                let cells = box_cells(f.side);
                let full = if cells == 64 {
                    u64::MAX
                } else {
                    (1u64 << cells) - 1
                };
                // An axis-aligned full box has no empty corners to refine.
                if (entry.is_some() || asks(i, f))
                    && f.chunk.is_none()
                    && f.shape.count_ones() > 1
                    && !(f.basis == IDENTITY && f.shape == full)
                {
                    shape_index[i] = shapes.len() as u32;
                    shapes.push(SettledShape::new(f));
                }
            }
        };
        let mut build_cells = || cells.build(&selected, want, fragments.len() >= 1024, task);
        if fragments.len() >= 1024 {
            rayon::join(build_shapes, build_cells);
        } else {
            build_shapes();
            build_cells();
        }
        completed("settled_cells");
    }
    /// Sweep down to the support, rather than teleporting the whole box
    /// half a unit down: that endpoint can lie below a thin plate entirely.
    fn supports(&self, p: [Fx; 3], low: [Fx; 3], high: [Fx; 3], skip: u32) -> bool {
        let mut lo = [0, 1, 2].map(|a| p[a].add(low[a]));
        let hi = [0, 1, 2].map(|a| p[a].add(high[a]));
        let bottom = lo[1];
        lo[1] = bottom.sub(Fx::from_ratio(1, 2));
        self.hit_refined(lo, hi, skip, Some(bottom))
    }
    /// Whether the box `lo..hi` shares space with a resting piece other
    /// than `skip`. The pieces are boxes here, as they are to the world.
    fn hit(&self, lo: [Fx; 3], hi: [Fx; 3], skip: u32, from_above: Option<Fx>) -> bool {
        self.hit_impl(lo, hi, skip, from_above, false)
    }
    fn hit_refined(&self, lo: [Fx; 3], hi: [Fx; 3], skip: u32, from_above: Option<Fx>) -> bool {
        self.hit_impl(lo, hi, skip, from_above, true)
    }
    fn hit_impl(
        &self,
        lo: [Fx; 3],
        hi: [Fx; 3],
        skip: u32,
        from_above: Option<Fx>,
        refine_moving: bool,
    ) -> bool {
        if self.cells.is_empty() {
            return false;
        }
        let moving = refine_moving
            .then(|| {
                self.shape_index
                    .get(skip as usize)
                    .and_then(|&n| self.shapes.get(n as usize))
            })
            .flatten();
        let shifts = moving
            .and_then(|_| self.boxes.get(skip as usize))
            .map(|(old_lo, old_hi)| {
                (
                    [0, 1, 2].map(|a| lo[a].sub(old_lo[a])),
                    [0, 1, 2].map(|a| hi[a].sub(old_hi[a])),
                )
            });
        let (c0, c1) = settled_cells(lo, hi);
        for z in c0[2]..=c1[2] {
            for y in c0[1]..=c1[1] {
                for x in c0[0]..=c1[0] {
                    let Some(list) = self.cells.get(&[x, y, z]) else {
                        continue;
                    };
                    for i in list {
                        if i == skip {
                            continue;
                        }
                        let (qlo, qhi) = self.boxes[i as usize];
                        // Support is a top face reached from above, not
                        // an existing overlap. Otherwise two intersecting
                        // pieces can hold each other in mid-air forever.
                        if from_above.is_some_and(|bottom| qhi[1] > bottom) {
                            continue;
                        }
                        if !(0..3).all(|a| lo[a] < qhi[a] && qlo[a] < hi[a]) {
                            continue;
                        }
                        let shape = self
                            .shape_index
                            .get(i as usize)
                            .and_then(|&n| self.shapes.get(n as usize));
                        let intersects = match (shape, moving, shifts) {
                            (Some(q), Some(m), Some((sl, sh))) => q.meets_pair(lo, hi, m, sl, sh),
                            (Some(q), _, _) => q.meets(lo, hi),
                            (None, Some(m), Some((sl, sh))) => m.meets(
                                [0, 1, 2].map(|a| qlo[a].sub(sh[a])),
                                [0, 1, 2].map(|a| qhi[a].sub(sl[a])),
                            ),
                            _ => true,
                        };
                        if intersects {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
}
/// Whether a piece's box at `p` meets a live hull (ADR-293): its vertical
/// extent against the hull's height, its horizontal reach against the
/// hull's footprint in the hull's own frame. A turned piece counts its
/// longest horizontal reach in every direction.
fn hull_hit<'h, 's>(
    p: [Fx; 3],
    low: [Fx; 3],
    high: [Fx; 3],
    hulls: &'h [Hull<'s>],
    skip: u64,
) -> Option<&'h Hull<'s>> {
    hull_box_hit(p, low, high, hulls, skip, true)
}
/// [`hull_hit`] by the hulls' boxes alone, or with their voxels when
/// `exact` (ADR-469).
fn hull_box_hit<'h, 's>(
    p: [Fx; 3],
    low: [Fx; 3],
    high: [Fx; 3],
    hulls: &'h [Hull<'s>],
    skip: u64,
    exact: bool,
) -> Option<&'h Hull<'s>> {
    if hulls.is_empty() {
        return None;
    }
    let reach = low[0].abs().max(high[0]).max(low[2].abs()).max(high[2]);
    let (bottom, top) = (p[1].add(low[1]), p[1].add(high[1]));
    hulls.iter().enumerate().find_map(|(i, h)| {
        if (i < 64 && skip & (1 << i) != 0) || !hull_box_overlap(p, reach, bottom, top, h) {
            return None;
        }
        (!exact || h.meets_piece(p, low, high)).then_some(h)
    })
}
#[inline]
fn hull_box_overlap(p: [Fx; 3], reach: Fx, bottom: Fx, top: Fx, h: &Hull<'_>) -> bool {
    if top < h.pos[1] || bottom > h.pos[1].add(h.height) {
        return false;
    }
    let dx = p[0].sub(h.pos[0]);
    let dz = p[2].sub(h.pos[2]);
    // A rotated rectangle cannot extend further than the sum of its
    // half-axes on either world axis. Most debris is far from a hull.
    let extent = h.half[0].add(h.half[1]).add(reach);
    if dx.abs() > extent || dz.abs() > extent {
        return false;
    }
    let (c, s) = (angle::cos(h.head), angle::sin(h.head));
    let u = dx.mul(c).add(dz.mul(s));
    let v = dz.mul(c).sub(dx.mul(s));
    u.abs() <= h.half[0].add(reach) && v.abs() <= h.half[1].add(reach)
}
/// Coefficients common to every piece in one fixed tick. Constructing them
/// once avoids thousands of identical Q32.32 integer divisions in a battle.
struct FragmentMotion {
    push: Fx,
    push_up: Fx,
    drag: Fx,
    max_speed: Fx,
    sleep: Fx,
    bounce: Fx,
    friction: Fx,
    spin_rate: Fx,
}
impl FragmentMotion {
    fn new(t: &Blast) -> Self {
        let drag = Fx::from_ratio(t.drag_pct as i32, 100);
        let friction = Fx::from_ratio(t.friction_pct as i32, 100);
        Self {
            push: Fx::from_ratio(t.push_pct as i32, 100),
            push_up: Fx::from_ratio(t.push_up_pct as i32, 100),
            drag: if crate::TICKS_PER_SECOND == 20 {
                drag.mul(drag.sqrt())
            } else {
                drag
            },
            max_speed: Fx::from_ratio(
                t.max_speed_mm_s as i32,
                voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
            ),
            sleep: Fx::from_ratio(
                t.sleep_mm_s as i32,
                voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
            ),
            bounce: Fx::from_ratio(t.bounce_pct as i32, 100),
            friction: if crate::TICKS_PER_SECOND == 20 {
                friction.mul(friction.sqrt())
            } else {
                friction
            },
            spin_rate: Fx::from_ratio(
                t.spin_deg_s as i32 * (fx::angle::FULL / 360),
                modding::clock::STREAM_HZ as i32,
            ),
        }
    }
}
/// Common translation/contact step for fragments and persistent dead hulls.
#[allow(clippy::too_many_arguments)]
fn advance_fragment(
    f: &mut Fragment,
    world: &voxel::World,
    t: &Blast,
    motion: &FragmentMotion,
    gravity: Fx,
    hulls: &[Hull],
    settled: &Settled,
    skip: u32,
    spin: bool,
) -> bool {
    if f.resting {
        return false;
    }
    let bounds = [world.nx, world.ny, world.nz];

    f.velocity.y = f.velocity.y.sub(gravity);
    f.velocity = f.velocity.scale(motion.drag);
    let vmax = motion.max_speed;
    if f.velocity.len_sq() > vmax.mul(vmax) {
        f.velocity = f.velocity.scale(vmax.div(f.velocity.len_sq().sqrt()));
    }
    // At most half a voxel per axis substep. Packed occupancy tests
    // cover the full box; the terrain volume has no ceiling.
    let steps = arr(f.velocity)
        .iter()
        .map(|v| v.abs().mul(Fx::from_int(2)).to_int_trunc() as usize + 1)
        .max()
        .unwrap();
    let delta = arr(f.velocity.scale(Fx::from_ratio(1, steps as i32)));
    let mut supported = false;
    let (low, high) = settled.fragment_bounds(skip, f);
    // A piece in a live hull's box and under its voxels — born there, from
    // a cell dug under the hull — leaves by the nearest side before it
    // flies (ADR-302, ADR-469): the blast throws it out from under the
    // hull rather than letting it pile up under the belly and lift the
    // hull off the ground. Hulls the piece so leaves, or already meets,
    // do not pen it in (ADR-294): it may leave them, and only entering
    // one is a contact.
    let start = arr(f.position);
    let (mut inside, mut under) = (0u64, 0u64);
    let reach = low[0].abs().max(high[0]).max(low[2].abs()).max(high[2]);
    let (bottom, top) = (start[1].add(low[1]), start[1].add(high[1]));
    for (i, h) in hulls.iter().enumerate().take(64) {
        if !hull_box_overlap(start, reach, bottom, top, h) {
            continue;
        }
        if h.shelters(start[1].add(low[1]), f.position) {
            under |= 1 << i;
            inside |= 1 << i;
        } else if h.meets_piece(start, low, high) {
            inside |= 1 << i;
        }
    }
    for (i, h) in hulls.iter().enumerate().take(64) {
        if under & (1 << i) != 0 {
            evict(f, h, low, high, [bounds[0], bounds[2]]);
        }
    }
    // Most pieces in flight meet nothing in a tick (ADR-465): when the box
    // they sweep through all the substeps is clear of the world, of every
    // hull they do not already overlap and of the settled pieces, no
    // substep is refused, and where they get to is the substeps' sum —
    // the same bits, without asking each substep again.
    let from = arr(f.position);
    let to: [Fx; 3] =
        std::array::from_fn(|a| from[a].add(delta[a].mul(Fx::from_int(steps as i32))));
    let swept_lo: [Fx; 3] = std::array::from_fn(|a| from[a].min(to[a]).add(low[a]));
    let swept_hi: [Fx; 3] = std::array::from_fn(|a| from[a].max(to[a]).add(high[a]));
    let (near, apart) = hull_near(from, to, low, high, hulls, inside);
    if !box_contact(world, [Fx::ZERO; 3], swept_lo, swept_hi, bounds)
        && !near
        && !settled.hit(swept_lo, swept_hi, skip, None)
    {
        f.position = v3(to);
    } else {
        substeps(
            f,
            world,
            motion,
            hulls,
            settled,
            skip,
            steps,
            delta,
            apart,
            (low, high),
            &mut supported,
        );
    }
    if spin && !supported {
        let basis = f.basis;
        tumble(f, t, motion);
        if f.basis != basis {
            let (low, high) = f.bounds();
            let p = arr(f.position);
            if fragment_contact(f, world, p, low, high, bounds)
                || hull_hit(p, low, high, hulls, inside).is_some()
                || settled.hit(
                    [0, 1, 2].map(|a| p[a].add(low[a])),
                    [0, 1, 2].map(|a| p[a].add(high[a])),
                    skip,
                    None,
                )
            {
                f.basis = basis;
            }
        }
    }
    let sleep = motion.sleep;
    if supported && f.velocity.len_sq() <= sleep.mul(sleep) {
        f.velocity = V3::ZERO;
        f.resting = true;
        return true;
    }
    false
}

/// Whether a piece moving from `from` to `to` with its box `low..high`
/// may meet a live hull it does not already overlap (`skip`): the hull's
/// height against the swept vertical extent, and the world box around
/// the hull's footprint, widened by the piece's reach and a unit for the
/// rounding of the hull's own frame, against the centre's swept box; and
/// the swept box against the hull's voxels (ADR-469). A bound from
/// outside: [`hull_hit`] decides the contact itself. With it, `skip` and
/// the first 64 hulls that no substep can meet.
fn hull_near(
    from: [Fx; 3],
    to: [Fx; 3],
    low: [Fx; 3],
    high: [Fx; 3],
    hulls: &[Hull],
    skip: u64,
) -> (bool, u64) {
    let reach = low[0].abs().max(high[0]).max(low[2].abs()).max(high[2]);
    let (bottom, top) = (
        from[1].min(to[1]).add(low[1]),
        from[1].max(to[1]).add(high[1]),
    );
    let exact = by_voxels(low, high);
    let swept_lo: [Fx; 3] = std::array::from_fn(|a| from[a].min(to[a]).add(low[a]));
    let swept_hi: [Fx; 3] = std::array::from_fn(|a| from[a].max(to[a]).add(high[a]));
    let horizontal_lo = [from[0].min(to[0]), from[2].min(to[2])];
    let horizontal_hi = [from[0].max(to[0]), from[2].max(to[2])];
    let (mut near, mut apart) = (false, skip);
    for (i, h) in hulls.iter().enumerate() {
        if i < 64 && skip & (1 << i) != 0 {
            continue;
        }
        // Superset of the rotated footprint on both world axes. The exact
        // rotated and voxel checks below still decide contact.
        let extent = h.half[0].add(h.half[1]).add(reach).add(reach).add(Fx::ONE);
        if top < h.pos[1]
            || bottom > h.pos[1].add(h.height)
            || horizontal_lo[0] > h.pos[0].add(extent)
            || horizontal_hi[0] < h.pos[0].sub(extent)
            || horizontal_lo[1] > h.pos[2].add(extent)
            || horizontal_hi[1] < h.pos[2].sub(extent)
        {
            if i < 64 {
                apart |= 1 << i;
            }
            continue;
        }
        let reaches = top >= h.pos[1] && bottom <= h.pos[1].add(h.height) && {
            let (c, s) = (angle::cos(h.head).abs(), angle::sin(h.head).abs());
            let (a, b) = (h.half[0].add(reach), h.half[1].add(reach));
            let extent = [a.mul(c).add(b.mul(s)), a.mul(s).add(b.mul(c))].map(|e| e.add(Fx::ONE));
            [(0, 0), (2, 1)].iter().all(|&(k, e)| {
                from[k].min(to[k]) <= h.pos[k].add(extent[e])
                    && from[k].max(to[k]) >= h.pos[k].sub(extent[e])
            })
        };
        if reaches && (!exact || h.meets(swept_lo, swept_hi, SWEPT_SAMPLES)) {
            near = true;
        } else if i < 64 {
            apart |= 1 << i;
        }
    }
    (near, apart)
}

/// The substeps of [`advance_fragment`] when the piece may meet
/// something: half a voxel at most per axis, each refused move bouncing
/// its axis and rubbing the others.
#[allow(clippy::too_many_arguments)]
fn substeps(
    f: &mut Fragment,
    world: &voxel::World,
    motion: &FragmentMotion,
    hulls: &[Hull],
    settled: &Settled,
    skip: u32,
    steps: usize,
    mut delta: [Fx; 3],
    inside: u64,
    local_bounds: ([Fx; 3], [Fx; 3]),
    supported: &mut bool,
) {
    let bounds = [world.nx, world.ny, world.nz];
    // Eviction changes position, not shape/basis; tumble happens afterward.
    let (low, high) = local_bounds;
    // A clear starting pose cannot acquire a penetration through moves the
    // strict query accepts. Only legacy/merged overlaps need the separation
    // query; ordinary impacts do not rescan the starting shape per substep.
    let embedded = fragment_contact(f, world, arr(f.position), low, high, bounds);
    for _ in 0..steps {
        for axis in 0..3 {
            if delta[axis] == Fx::ZERO {
                continue;
            }
            let mut p = arr(f.position);
            p[axis] = p[axis].add(delta[axis]);
            let world_contact = if embedded {
                fragment_motion_contact(f, world, p, low, high, bounds)
            } else {
                fragment_contact(f, world, p, low, high, bounds)
            };
            let blocked = world_contact
                || hull_hit(p, low, high, hulls, inside).is_some()
                || (if axis == 1 && delta[axis] < Fx::ZERO {
                    Settled::hit_refined
                } else {
                    Settled::hit
                })(
                    settled,
                    [0, 1, 2].map(|a| p[a].add(low[a])),
                    [0, 1, 2].map(|a| p[a].add(high[a])),
                    skip,
                    (axis == 1 && delta[axis] < Fx::ZERO).then_some(f.position.y.add(low[1])),
                );
            if blocked {
                *supported |= axis == 1 && delta[axis] < Fx::ZERO;
                delta[axis] = Fx::ZERO;
                let mut velocity = arr(f.velocity);
                velocity[axis] = velocity[axis].neg().mul(motion.bounce);
                for (k, value) in velocity.iter_mut().enumerate() {
                    if k != axis {
                        *value = value.mul(motion.friction);
                    }
                }
                f.velocity = v3(velocity);
            } else {
                f.position = v3(p);
            }
        }
    }
}

/// A flying piece tumbles (ADR-309): it turns about one of its own axes,
/// chosen by its number, as fast as it goes. The turn lives in the basis
/// the piece already carries, so nothing is added to what is saved, and
/// a piece that lands keeps the face it landed on. A piece dropped into
/// place with no speed does not turn at all.
fn tumble(f: &mut Fragment, t: &Blast, motion: &FragmentMotion) {
    if t.spin_deg_s == 0 {
        return;
    }
    // The spin comes from the throw, not from the fall: a piece dropped
    // straight down has nothing to turn it, and one that flies sideways or
    // bounces does.
    let speed = f
        .velocity
        .x
        .mul(f.velocity.x)
        .add(f.velocity.z.mul(f.velocity.z))
        .sqrt();
    // Degrees a second for each unit of speed a tick, into binary angle
    // a tick: `deg × FULL / 360 / ticks`.
    let mut rate = speed.mul(motion.spin_rate);
    // A section turns as fast as a piece whose surface moves as fast: the
    // rate of a piece a unit across, over the section's half-width in
    // units (ADR-468). A turret does not spin like a crumb.
    if let Some(chunk) = &f.chunk {
        let (lo, hi) = chunk.bounds;
        let half = (0..3)
            .map(|a| hi[a].sub(lo[a]))
            .max()
            .unwrap_or(Fx::ZERO)
            .div(Fx::from_int(2));
        rate = rate.div(half.max(Fx::ONE));
    }
    let mut rng = fx::rng::Rng::new(f.id, 477);
    rate = rate.mul(Fx::from_ratio(rng.range(t.speed_min_pct as i32, 100), 100));
    let turn = rate.to_int_trunc() * if rng.below(2) == 0 { -1 } else { 1 };
    if turn == 0 {
        return;
    }
    let axis = rng.below(3) as usize;
    let (c, s) = (fx::angle::cos(turn), fx::angle::sin(turn));
    let (i, j) = ((axis + 1) % 3, (axis + 2) % 3);
    for row in &mut f.basis {
        let v = arr(*row);
        let mut out = v;
        out[i] = v[i].mul(c).sub(v[j].mul(s));
        out[j] = v[i].mul(s).add(v[j].mul(c));
        *row = v3(out).renormalised();
    }
}
/// Move a piece out of a hull's box by the nearest side, in the hull's
/// own frame, keeping its height (ADR-302). At the map edge the nearest
/// side can lie off the map; the nearest one that keeps the centre on
/// it is taken instead (ADR-428).
fn evict(f: &mut Fragment, h: &Hull, low: [Fx; 3], high: [Fx; 3], columns: [u32; 2]) {
    let (c, s) = (fx::angle::cos(h.head), fx::angle::sin(h.head));
    let p = arr(f.position);
    let (dx, dz) = (p[0].sub(h.pos[0]), p[2].sub(h.pos[2]));
    let u = dx.mul(c).add(dz.mul(s));
    let v = dz.mul(c).sub(dx.mul(s));
    let reach = high[0].max(high[2]).max(low[0].neg()).max(low[2].neg());
    let clear = reach.add(Fx::from_ratio(1, 2));
    let out_u = h.half[0].add(clear);
    let out_v = h.half[1].add(clear);
    // The four sides, nearest first; a tie keeps this order.
    let mut sides = [
        (
            out_u.sub(u.abs()),
            [if u.raw() < 0 { out_u.neg() } else { out_u }, v],
        ),
        (
            out_v.sub(v.abs()),
            [u, if v.raw() < 0 { out_v.neg() } else { out_v }],
        ),
        (
            out_u.add(u.abs()),
            [if u.raw() < 0 { out_u } else { out_u.neg() }, v],
        ),
        (
            out_v.add(v.abs()),
            [u, if v.raw() < 0 { out_v } else { out_v.neg() }],
        ),
    ];
    sides.sort_by_key(|side| side.0);
    let at = |[nu, nv]: [Fx; 2]| {
        [
            h.pos[0].add(nu.mul(c)).sub(nv.mul(s)),
            h.pos[2].add(nu.mul(s)).add(nv.mul(c)),
        ]
    };
    let on_map =
        |q: [Fx; 2]| (0..2).all(|a| q[a] >= Fx::ZERO && q[a] < Fx::from_int(columns[a] as i32));
    let q = sides
        .iter()
        .map(|side| at(side.1))
        .find(|q| on_map(*q))
        .unwrap_or_else(|| at(sides[0].1));
    f.position = V3::new(q[0], p[1], q[1]);
    f.keep_on_map(columns);
}

fn box_contact(
    world: &voxel::World,
    p: [Fx; 3],
    low: [Fx; 3],
    high: [Fx; 3],
    bounds: [u32; 3],
) -> bool {
    // Floor, including negative fractions at the map boundary.
    let lo: [i32; 3] = std::array::from_fn(|k| (p[k].add(low[k]).raw() >> 32) as i32);
    let hi: [i32; 3] = std::array::from_fn(|k| (p[k].add(high[k]).raw() >> 32) as i32);
    if lo[1] < 0
        || [0, 2]
            .iter()
            .any(|&k| lo[k] < 0 || hi[k] >= bounds[k] as i32)
    {
        return true;
    }
    let hi = [hi[0], hi[1].min(bounds[1] as i32 - 1), hi[2]];
    if (0..3).any(|a| lo[a] > hi[a]) {
        return false;
    }
    world.any_solid_box(lo.map(|v| v as u32), hi.map(|v| v as u32))
}
fn fragment_contact(
    f: &Fragment,
    world: &voxel::World,
    p: [Fx; 3],
    low: [Fx; 3],
    high: [Fx; 3],
    bounds: [u32; 3],
) -> bool {
    if !box_contact(world, p, low, high, bounds) {
        return false;
    }
    if f.chunk.is_some() || f.shape.count_ones() == 1 {
        return true;
    }
    // A connected 2³ mask need not be a cuboid. Its empty corners must not
    // collide with material that was deliberately left in the wall.
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
        if f.shape & (1 << i) != 0 {
            let q = shape_cell(i, f.side);
            let mut offset = V3::ZERO;
            for a in 0..3 {
                offset = offset.add(f.basis[a].scale(Fx::from_int(q[a]).sub(centre[a]).mul(scale)));
            }
            let at = arr(v3(p).add(offset));
            if box_contact(world, at, radius.map(Fx::neg), radius, bounds) {
                return true;
            }
        }
    }
    false
}

/// A pre-existing penetration (old saves, a pile growing beside terrain)
/// is not a new impact. Permit separation from those same world cells;
/// entering any other solid cell or moving deeper remains a collision.
fn fragment_motion_contact(
    f: &Fragment,
    world: &voxel::World,
    p: [Fx; 3],
    low: [Fx; 3],
    high: [Fx; 3],
    bounds: [u32; 3],
) -> bool {
    if !fragment_contact(f, world, p, low, high, bounds) {
        return false;
    }
    let from = arr(f.position);
    if !fragment_contact(f, world, from, low, high, bounds) {
        return true;
    }
    let axis = (0..3).find(|&a| p[a] != from[a]).expect("one moving axis");
    let delta = p[axis].sub(from[axis]);
    let separates = |offset: [Fx; 3], lo: [Fx; 3], hi: [Fx; 3]| {
        let now = [0, 1, 2].map(|a| from[a].add(offset[a]));
        let next = [0, 1, 2].map(|a| p[a].add(offset[a]));
        let qlo = [0, 1, 2].map(|a| (next[a].add(lo[a]).raw() >> 32) as i32);
        let qhi = [0, 1, 2].map(|a| (next[a].add(hi[a]).raw() >> 32) as i32);
        if qlo[1] < 0
            || [0, 2]
                .iter()
                .any(|&a| qlo[a] < 0 || qhi[a] >= bounds[a] as i32)
        {
            return false;
        }
        for y in qlo[1]..=qhi[1].min(bounds[1] as i32 - 1) {
            for z in qlo[2]..=qhi[2] {
                for x in qlo[0]..=qhi[0] {
                    if !world.solid(x as u32, y as u32, z as u32) {
                        continue;
                    }
                    let cell = [x, y, z];
                    let already = (0..3).all(|a| {
                        now[a].add(lo[a]) < Fx::from_int(cell[a] + 1)
                            && now[a].add(hi[a]) >= Fx::from_int(cell[a])
                    });
                    let centre = now[axis].add(lo[axis].add(hi[axis]).div(Fx::from_int(2)));
                    let away = centre.sub(Fx::from_int(cell[axis]).add(Fx::from_ratio(1, 2)));
                    if !already || (away != Fx::ZERO && (away < Fx::ZERO) != (delta < Fx::ZERO)) {
                        return false;
                    }
                }
            }
        }
        true
    };
    if f.chunk.is_some() || f.shape.count_ones() == 1 {
        return !separates([Fx::ZERO; 3], low, high);
    }
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
        if f.shape & (1 << i) == 0 {
            continue;
        }
        let cell = shape_cell(i, f.side);
        let mut offset = V3::ZERO;
        for a in 0..3 {
            offset = offset.add(f.basis[a].scale(Fx::from_int(cell[a]).sub(centre[a]).mul(scale)));
        }
        if !separates(arr(offset), radius.map(Fx::neg), radius) {
            return true;
        }
    }
    false
}
