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
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use voxel::{
    store::{Mat, AIR},
    world::{snapshot::Snapshot, Edit},
};

#[path = "chunk.rs"]
pub mod chunk;
#[path = "corpse.rs"]
mod corpse;
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

    /// Two pieces settling in one pile cell become one upright heap on the
    /// floor with a cell for each of theirs, and the heap is floor in its
    /// columns (ADR-292).
    #[test]
    fn pieces_settling_together_become_an_upright_heap_that_is_floor() {
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
        assert_eq!(b.fragments.len(), 1, "the pieces did not merge");
        let f = &b.fragments[0];
        assert!(f.resting);
        assert_eq!((f.side, f.shape, f.count), (4, 0b11, 2));
        assert_eq!(f.basis, IDENTITY);
        // Its bottom sits on the ground at 12 and its top one cell up.
        assert_eq!(f.position.y, Fx::from_ratio(25, 2));
        let rubble = b.rubble(&w, &[]);
        assert_eq!(rubble.get(&[48, 48]), Some(&13));
        assert!(b.rubble(&w, &[1]).is_empty(), "a yielding heap is no floor");
        assert_eq!(rubble.get(&[49, 48]), Some(&13));
        assert_eq!(rubble.get(&[51, 48]), None);
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
        assert!(b.rubble(&w, &[]).is_empty(), "a piece on a roof is no heap");
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
    fn economy() -> Economy {
        let mats =
            modding::data::materials_from_ron(include_str!("../../../data/materials.ron")).unwrap();
        Economy::new(modding::Kinds::new(), mats, 2, &tuning())
    }
    fn world() -> voxel::World {
        voxel::World::new(96, 64, 96, 4096, Box::new(Ground))
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
        b.claims.insert(
            p,
            Claim {
                at: [48, 13, 48],
                motion_mj: 999,
                side: 2,
            },
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
        b.claims.insert(
            [48, 11, 48],
            Claim {
                at: [48, 13, 48],
                motion_mj: 999,
                side: 2,
            },
        );
        w.queue_edit(Edit::dig_if(48, 11, 48, 1));
        w.flush();
        b.confirm(w.completed(), &mut e, &t);
        assert_eq!(w.material(48, 11, 48), 3);
        assert_eq!(b.claims(), 0);
        assert!(b.fragments.is_empty());
        assert!(e.balanced());
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
}

/// A live hull a piece can land on (ADR-293): its footprint box about
/// `pos` turned by `head`, standing `height` above `pos.y`; `vel` is how
/// far it moved this tick, and a moving hull bulldozes the pieces it
/// drives into (ADR-294).
#[derive(Clone, Copy, Debug)]
pub struct Hull {
    pub pos: [Fx; 3],
    pub head: i32,
    pub half: [Fx; 2],
    pub height: Fx,
    pub vel: [Fx; 3],
    /// The rise the hull climbs rather than pushes: a piece no taller
    /// than this above the hull's bottom is floor to it (ADR-292).
    pub step: Fx,
}

/// What the pieces meet besides the world this tick: the live hulls
/// (ADR-293) and the materials a hull crushes rather than climbs
/// (ADR-294). The batch has neither.
#[derive(Clone, Copy)]
pub struct Surroundings<'a> {
    pub hulls: &'a [Hull],
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
    pub chunk: Option<chunk::Chunk>,
    pub held: u32,
    pub resting: bool,
}

#[derive(Default)]
pub struct Bursts {
    jobs: VecDeque<Job>,
    claims: BTreeMap<[u32; 3], Claim>,
    pub fragments: Vec<Fragment>,
    pub corpses: BTreeMap<u32, Fragment>,
    next_id: u64,
    pub removed: u64,
    pub spent_mj: u64,
    pub motion_mj: u64,
    pub body_motion_mj: u64,
    pub body_removed: u64,
}

pub const IDENTITY: [V3; 3] = [
    V3::new(Fx::ONE, Fx::ZERO, Fx::ZERO),
    V3::UP,
    V3::new(Fx::ZERO, Fx::ZERO, Fx::ONE),
];
impl Fragment {
    fn bounds(&self) -> ([Fx; 3], [Fx; 3]) {
        if let Some(chunk) = &self.chunk {
            return chunk.bounds;
        }
        let (lo, hi) = shape_bounds(self.shape, self.side);
        let scale = Fx::from_ratio(self.mm as i32, voxel::unit::WORLD_MM as i32);
        let mut low = [Fx::MAX; 3];
        let mut high = [Fx::MIN; 3];
        for mask in 0..8 {
            let mut v = V3::ZERO;
            for axis in 0..3 {
                v = v.add(self.basis[axis].scale(if mask & (1 << axis) == 0 {
                    lo[axis].mul(scale)
                } else {
                    hi[axis].mul(scale)
                }));
            }
            for (axis, c) in arr(v).into_iter().enumerate() {
                low[axis] = low[axis].min(c);
                high[axis] = high[axis].max(c);
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
/// Centre of mass of a piece in its box, cell units.
pub fn shape_centre(shape: u64, side: u8) -> V3 {
    let mut sum = [0i32; 3];
    for i in 0..box_cells(side) {
        if shape & (1 << i) != 0 {
            let p = shape_cell(i, side);
            for a in 0..3 {
                sum[a] += p[a];
            }
        }
    }
    v3(sum.map(|v| Fx::from_ratio(v, shape.count_ones().max(1) as i32)))
}
/// Face-connected components of a mask, each a piece of its own.
fn shape_components(mut left: u64, side: u8) -> Vec<u64> {
    let s = u32::from(side.clamp(1, 4));
    let strides = [1, s, s * s];
    let n = box_cells(side);
    let mut out = Vec::new();
    while left != 0 {
        let mut component = 1u64 << left.trailing_zeros();
        loop {
            let before = component;
            for i in 0..n {
                if component & (1 << i) != 0 {
                    let p = shape_cell(i, side);
                    for a in 0..3 {
                        if p[a] + 1 < s as i32 {
                            component |= left & (1 << (i + strides[a]));
                        }
                        if p[a] > 0 {
                            component |= left & (1 << (i - strides[a]));
                        }
                    }
                }
            }
            if before == component {
                break;
            }
        }
        left &= !component;
        out.push(component);
    }
    out
}
fn shape_bounds(shape: u64, side: u8) -> ([Fx; 3], [Fx; 3]) {
    let centre = arr(shape_centre(shape, side));
    let mut lo = [i32::from(side.clamp(1, 4)) - 1; 3];
    let mut hi = [0; 3];
    for i in 0..box_cells(side) {
        if shape & (1 << i) != 0 {
            let p = shape_cell(i, side);
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
    }
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
        if self.fragment_room(t) == 0
            || self.claims.len() >= t.max_claims as usize
            || self.claims.contains_key(&p)
        {
            return false;
        }
        self.claims.insert(
            p,
            Claim {
                at,
                motion_mj,
                side,
            },
        );
        world.queue_edit(Edit::dig_if(p[0], p[1], p[2], mat));
        true
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
    pub fn fragment_room(&self, t: &Blast) -> u32 {
        let moving = self.fragments.iter().filter(|f| !f.resting).count();
        (t.max_flying as usize)
            .saturating_sub(moving + self.claims.len())
            .min(
                (t.max_fragments as usize).saturating_sub(self.fragments.len() + self.claims.len()),
            ) as u32
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
            f.velocity = direction.scale(speed.div(norm.sqrt()));
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
        let moving = self.fragments.iter().filter(|f| !f.resting).count();
        let room = (t.max_flying as usize).saturating_sub(moving);
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
                    if self.claims.len() >= t.max_claims as usize
                        || self.claims.len() >= room
                        || self.claims.len() + self.fragments.len() >= t.max_fragments as usize
                    {
                        break;
                    }
                    let q = ijk(j.subcell, j.cell as usize);
                    let p: [i32; 3] =
                        std::array::from_fn(|a| j.origin[a] + hit.cell[a] * j.cell + q[a]);
                    j.subcell += 1;
                    if j.subcell == (j.cell as usize).pow(3) {
                        j.subcell = 0;
                        j.hits.pop_front();
                        j.body_hit_done = false;
                    }
                    if p[0] < 0
                        || p[1] < 0
                        || p[2] < 0
                        || p[0] >= world.nx as i32
                        || p[1] >= world.ny as i32
                        || p[2] >= world.nz as i32
                    {
                        continue;
                    }
                    let key = p.map(|v| v as u32);
                    if self.claims.contains_key(&key) || !world.solid(key[0], key[1], key[2]) {
                        continue;
                    }
                    let material = world.material(key[0], key[1], key[2]);
                    // Snapshot determines reach; current material determines the work
                    // and prevents a cheap old soil cell destroying new steel for free.
                    let Some(d) = mats.get(material) else {
                        continue;
                    };
                    if d.density == 0 {
                        continue;
                    }
                    let (cost, motion) = fracture_cost_mj(d.strength, voxel::unit::WORLD_MM, t);
                    if cost > j.energy_mj {
                        continue;
                    }
                    j.energy_mj -= cost;
                    self.spent_mj = self.spent_mj.saturating_add(cost);
                    self.claims.insert(
                        key,
                        Claim {
                            at: j.at,
                            motion_mj: motion,
                            side: t.fragment_box as u8,
                        },
                    );
                    world.queue_edit(Edit::dig_if(key[0], key[1], key[2], material));
                }
            }
            let done = j.wave.as_ref().is_some_and(|w| w.finished()) && j.hits.is_empty();
            if !done {
                self.jobs.push_back(j);
            }
        }
    }

    /// A no-op consumes its reservation too. Only solid -> air creates debris.
    pub fn confirm(&mut self, completed: &[(Edit, Mat)], e: &mut Economy, t: &Blast) {
        type GroundGroup = ([u32; 3], Mat, [i32; 3], u8);
        let mut groups: BTreeMap<GroundGroup, (u64, u64)> = BTreeMap::new();
        for &(edit, old) in completed {
            let Some(expected) = edit.expected_material() else {
                continue;
            };
            let key = [edit.x, edit.y, edit.z];
            let Some(claim) = self.claims.remove(&key) else {
                continue;
            };
            if old == AIR || old != expected {
                continue;
            }
            let side = u32::from(claim.side.clamp(1, 4));
            let origin = key.map(|v| v / side * side);
            let sub = (key[0] - origin[0])
                + side * (key[1] - origin[1])
                + side * side * (key[2] - origin[2]);
            let group = groups
                .entry((origin, old, claim.at, side as u8))
                .or_default();
            group.0 |= 1u64 << sub;
            group.1 = group.1.saturating_add(claim.motion_mj);
        }
        for ((key, old, at, side), (whole, total_motion_mj)) in groups {
            for shape in shape_components(whole, side) {
                let count = shape.count_ones();
                let motion_mj = total_motion_mj * count as u64 / whole.count_ones() as u64;
                self.spawn(key, old, at, side, shape, motion_mj, e, t);
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
        let norm = direction.len_sq().sqrt();
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
        let velocity = direction.scale(speed.div(norm));
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
        let (mut made, mut returned) = (0, 0);
        for ((key, m), whole) in groups {
            let at = key.map(|v| v as i32 + i32::from(side) / 2);
            for shape in shape_components(whole, side) {
                if self.fragment_room(t) > 0 && self.spawn(key, m, at, side, shape, 0, e, t) {
                    made += 1;
                    continue;
                }
                for i in 0..box_cells(side) {
                    if shape & (1 << i) != 0 {
                        let c = shape_cell(i, side);
                        world.queue_edit(Edit::put(
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

    pub fn step_fragments(
        &mut self,
        world: &voxel::World,
        e: &mut Economy,
        t: &Blast,
        gravity: Fx,
        tick: u64,
        around: Surroundings<'_>,
    ) {
        let (hulls, yielding) = (around.hulls, around.yielding);
        type RestingPile = ([i32; 3], Mat, Option<[u8; 3]>, u16);
        let mut piles: BTreeMap<RestingPile, usize> = BTreeMap::new();
        let mut merged = BTreeSet::new();
        // Waking pieces count against the flying budget like new ones; a
        // piece without room waits for its next turn.
        let mut room = (t.max_flying as usize)
            .saturating_sub(self.fragments.iter().filter(|f| !f.resting).count());
        for i in 0..self.fragments.len() {
            let f = &mut self.fragments[i];
            if f.chunk.is_none() && t.push_pct > 0 && (!f.resting || room > 0) {
                let (low, high) = f.bounds();
                if let Some(h) = hull_hit(arr(f.position), low, high, hulls, 0) {
                    // What the hull climbs as floor it does not push: a
                    // piece of a yielding material, or one rising above
                    // the hull's step, is bulldozed; the rest is ridden.
                    let climbs = !yielding.contains(&f.material)
                        && f.position.y.add(high[1]) <= h.pos[1].add(h.step);
                    if h.vel != [Fx::ZERO; 3] && !climbs {
                        // A moving hull bulldozes the piece it drove into
                        // (ADR-294): the piece leaves ahead of it at the
                        // hull's pace and a little up.
                        if f.resting {
                            f.resting = false;
                            room -= 1;
                            let held = f.held;
                            e.wrecks.retain(|w| w.held != held);
                        }
                        let push = Fx::from_ratio(t.push_pct as i32, 100);
                        let along = V3::new(h.vel[0], Fx::ZERO, h.vel[2]).scale(push);
                        let lift = along
                            .len_sq()
                            .sqrt()
                            .mul(Fx::from_ratio(t.push_up_pct as i32, 100));
                        f.velocity = V3::new(along.x, h.vel[1].mul(push).add(lift), along.z);
                    }
                }
            }
            if room > 0 && f.resting && due(f.id, tick, t) && !supported(f, world, hulls) {
                // The ground under it was taken (ADR-291): it falls again,
                // and its wreck entry goes until it settles anew.
                f.resting = false;
                room -= 1;
                let held = f.held;
                e.wrecks.retain(|w| w.held != held);
            }
            if advance_fragment(f, world, t, gravity, hulls) {
                let p = arr(f.position).map(|v| v.to_int_trunc());
                e.wrecks.push(Wreck {
                    held: f.held,
                    x: p[0],
                    y: p[1],
                    z: p[2],
                });
            }
            if !self.fragments[i].resting || self.fragments[i].chunk.is_some() {
                continue;
            }
            let f = &self.fragments[i];
            let key = (
                arr(f.position).map(|v| v.to_int_trunc().div_euclid(t.pile_cell as i32)),
                f.material,
                f.paint,
                f.mm,
            );
            if let Some(&j) = piles.get(&key) {
                let (a, b) = self.fragments.split_at_mut(i);
                let to = &mut a[j];
                let from = &mut b[0];
                e.ledger.transfer(from.held, to.held, u64::MAX);
                to.mass_g += from.mass_g;
                to.count += from.count;
                heap(to, world);
                merged.insert(from.held);
            } else {
                piles.insert(key, i);
            }
        }
        if !merged.is_empty() {
            self.fragments.retain(|f| !merged.contains(&f.held));
            e.wrecks.retain(|w| !merged.contains(&w.held));
            for held in merged {
                e.ledger.retire_empty_debris(held);
            }
        }
        for f in self.corpses.values_mut() {
            if f.resting && due(f.id, tick, t) && !supported(f, world, &[]) {
                f.resting = false;
            }
            advance_fragment(f, world, t, gravity, &[]);
        }
        for w in &mut e.wrecks {
            if let Some(f) = self.corpses.get(&w.held) {
                let p = arr(f.position).map(|v| v.to_int_trunc());
                w.x = p[0];
                w.y = p[1];
                w.z = p[2];
            }
        }
    }

    /// The columns resting pieces stand in and the height of their tops,
    /// for the contacts, feet and planner of the range (ADR-292). Pieces
    /// of `yielding` materials are left out: a hull crushes through a
    /// felled crown as through a standing one, and so is a piece lying on
    /// a hull's roof rather than on the world (ADR-293). Derived from the
    /// pieces every tick; nothing is saved.
    pub fn rubble(&self, world: &voxel::World, yielding: &[Mat]) -> nav::vehicle::Rubble {
        let mut out = nav::vehicle::Rubble::new();
        for f in self.fragments.iter().filter(|f| {
            f.resting
                && f.chunk.is_none()
                && !yielding.contains(&f.material)
                && supported(f, world, &[])
        }) {
            let (low, high) = f.bounds();
            let p = arr(f.position);
            // Floor of the bottom corners, ceiling of the top: the columns
            // the box covers and the height it reaches.
            let lo = [p[0].add(low[0]), p[2].add(low[2])].map(|v| (v.raw() >> 32) as i32);
            let hi = [p[0].add(high[0]), p[2].add(high[2])].map(|v| (v.raw() >> 32) as i32);
            let top = -(p[1].add(high[1]).neg().raw() >> 32);
            let top = top.clamp(0, i64::from(u16::MAX)) as u16;
            for z in lo[1].max(0)..=hi[1] {
                for x in lo[0].max(0)..=hi[0] {
                    let cell = out.entry([x, z]).or_insert(0);
                    *cell = (*cell).max(top);
                }
            }
        }
        out
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
        Self::load_material(s, world, t, None, 0, false)
    }
    /// `two_cell_masks`: the save predates ADR-284 and holds eight-bit
    /// masks without a box side; every piece is read as a two-cell box.
    pub fn load_material(
        s: &mut Scan,
        world: Option<&voxel::World>,
        t: &Blast,
        bodies: Option<&crate::damage::Bodies>,
        units: usize,
        two_cell_masks: bool,
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
                || out
                    .claims
                    .insert(
                        p,
                        Claim {
                            at,
                            motion_mj,
                            side,
                        },
                    )
                    .is_some()
            {
                return None;
            }
        }
        let count = s.usize();
        let corpses = s.usize();
        if count > t.max_fragments as usize || corpses > s.left() / 100 {
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
                f.chunk = Some(chunk::Chunk::load(s, bodies, f.mm, f.count)?);
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
        s.ok().then_some(out)
    }
}

/// A pile with more than one piece in it is a heap (ADR-292): its cells
/// fill a four-cell box from the bottom layer up, upright, standing on the
/// world under its centre, so that it has the height its matter takes.
/// Matter beyond the box stays in the container; the box only saturates.
pub(crate) fn heap(f: &mut Fragment, world: &voxel::World) {
    let n = f.count.min(box_cells(4));
    let mut shape = 0u64;
    let mut placed = 0;
    for y in 0..4 {
        for z in 0..4 {
            for x in 0..4 {
                if placed < n {
                    shape |= 1u64 << (x + 4 * y + 16 * z);
                    placed += 1;
                }
            }
        }
    }
    f.shape = shape;
    f.side = 4;
    f.basis = IDENTITY;
    let scale = Fx::from_ratio(f.mm as i32, voxel::unit::WORLD_MM as i32);
    let p = arr(f.position);
    let (x, z) = ((p[0].raw() >> 32) as i32, (p[2].raw() >> 32) as i32);
    if x < 0 || z < 0 || x as u32 >= world.nx || z as u32 >= world.nz {
        return;
    }
    let mut floor = 0;
    let mut y = (p[1].raw() >> 32).clamp(0, i64::from(world.ny) - 1) as i32;
    while y >= 0 {
        if world.solid(x as u32, y as u32, z as u32) {
            floor = y + 1;
            break;
        }
        y -= 1;
    }
    let centre = arr(shape_centre(shape, 4));
    f.position.y = Fx::from_int(floor).add(centre[1].add(Fx::from_ratio(1, 2)).mul(scale));
}
/// A resting piece looks under itself every `rest_check_ticks` ticks
/// (ADR-291), the pieces spread over the period by id so that no tick
/// checks them all.
fn due(id: u64, tick: u64, t: &Blast) -> bool {
    id.wrapping_add(tick)
        .is_multiple_of(u64::from(t.rest_check_ticks.max(1)))
}
/// Whether material or a live hull still holds the piece up: its box half
/// a unit lower, the most a settling step moves, meets the world or a hull.
fn supported(f: &Fragment, world: &voxel::World, hulls: &[Hull]) -> bool {
    let (low, high) = f.bounds();
    let mut p = arr(f.position);
    p[1] = p[1].sub(Fx::from_ratio(1, 2));
    fragment_contact(f, world, p, low, high, [world.nx, world.ny, world.nz])
        || hull_hit(p, low, high, hulls, 0).is_some()
}
/// Whether a piece's box at `p` meets a live hull (ADR-293): its vertical
/// extent against the hull's height, its horizontal reach against the
/// hull's footprint in the hull's own frame. A turned piece counts its
/// longest horizontal reach in every direction.
fn hull_hit(p: [Fx; 3], low: [Fx; 3], high: [Fx; 3], hulls: &[Hull], skip: u64) -> Option<&Hull> {
    if hulls.is_empty() {
        return None;
    }
    let reach = low[0].abs().max(high[0]).max(low[2].abs()).max(high[2]);
    let (bottom, top) = (p[1].add(low[1]), p[1].add(high[1]));
    hulls.iter().enumerate().find_map(|(i, h)| {
        if (i < 64 && skip & (1 << i) != 0) || top < h.pos[1] || bottom > h.pos[1].add(h.height) {
            return None;
        }
        let (c, s) = (angle::cos(h.head), angle::sin(h.head));
        let dx = p[0].sub(h.pos[0]);
        let dz = p[2].sub(h.pos[2]);
        let u = dx.mul(c).add(dz.mul(s));
        let v = dz.mul(c).sub(dx.mul(s));
        (u.abs() <= h.half[0].add(reach) && v.abs() <= h.half[1].add(reach)).then_some(h)
    })
}
/// Common translation/contact step for fragments and persistent dead hulls.
fn advance_fragment(
    f: &mut Fragment,
    world: &voxel::World,
    t: &Blast,
    gravity: Fx,
    hulls: &[Hull],
) -> bool {
    if f.resting {
        return false;
    }
    let bounds = [world.nx, world.ny, world.nz];

    f.velocity.y = f.velocity.y.sub(gravity);
    f.velocity = f.velocity.scale(Fx::from_ratio(t.drag_pct as i32, 100));
    let vmax = Fx::from_ratio(
        t.max_speed_mm_s as i32,
        voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
    );
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
    let mut delta = arr(f.velocity.scale(Fx::from_ratio(1, steps as i32)));
    let mut supported = false;
    let (low, high) = f.bounds();
    // Hulls the piece already overlaps do not pen it in (ADR-294): it may
    // leave them, and only entering one is a contact.
    let start = arr(f.position);
    let inside = hulls.iter().enumerate().fold(0u64, |m, (i, h)| {
        if i < 64 && hull_hit(start, low, high, std::slice::from_ref(h), 0).is_some() {
            m | (1 << i)
        } else {
            m
        }
    });
    for _ in 0..steps {
        for axis in 0..3 {
            if delta[axis] == Fx::ZERO {
                continue;
            }
            let mut p = arr(f.position);
            p[axis] = p[axis].add(delta[axis]);
            let blocked = fragment_contact(f, world, p, low, high, bounds)
                || hull_hit(p, low, high, hulls, inside).is_some();
            if blocked {
                supported |= axis == 1 && delta[axis] < Fx::ZERO;
                delta[axis] = Fx::ZERO;
                let mut velocity = arr(f.velocity);
                velocity[axis] = velocity[axis]
                    .neg()
                    .mul(Fx::from_ratio(t.bounce_pct as i32, 100));
                for (k, value) in velocity.iter_mut().enumerate() {
                    if k != axis {
                        *value = value.mul(Fx::from_ratio(t.friction_pct as i32, 100));
                    }
                }
                f.velocity = v3(velocity);
            } else {
                f.position = v3(p);
            }
        }
    }
    let sleep = Fx::from_ratio(
        t.sleep_mm_s as i32,
        voxel::unit::WORLD_MM as i32 * crate::TICKS_PER_SECOND,
    );
    if supported && f.velocity.len_sq() <= sleep.mul(sleep) {
        f.velocity = V3::ZERO;
        f.resting = true;
        return true;
    }
    false
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
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1].min(bounds[1] as i32 - 1) {
            for x in (lo[0]..=hi[0]).step_by(64) {
                if world.solid_bits(x as u32, y as u32, z as u32, (hi[0] - x + 1).min(64) as u32)
                    != 0
                {
                    return true;
                }
            }
        }
    }
    false
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
