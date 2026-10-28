//! Ground reservations must match a full connectivity recount after every
//! admitted, refused and removed cell, including bridges and duplicate claims.
use super::*;

fn recount(mask: u64, bit: u64, side: u8) -> i64 {
    i64::from(shape_component_count(mask | bit, side))
        - i64::from(shape_component_count(mask, side))
}

#[test]
fn local_component_delta_matches_full_recount() {
    for side in 1u8..=4 {
        let cells = u32::from(side).pow(3);
        let valid = u64::MAX >> (64 - cells);
        let masks = if side < 3 { 1usize << cells } else { 4096 };
        let mut seed = 0x18b321954ea737d1u64;
        for i in 0..masks {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mask = if side < 3 {
                i as u64
            } else {
                // Include dense prefixes/suffixes, sparse and arbitrary masks.
                match i % 4 {
                    0 => valid >> (i as u32 % cells),
                    1 => (valid << (i as u32 % cells)) & valid,
                    2 => seed & seed.rotate_left(17) & valid,
                    _ => seed & valid,
                }
            };
            for cell in 0..cells {
                let bit = 1u64 << cell;
                assert_eq!(
                    shape_added_components(mask, bit, side),
                    recount(mask, bit, side),
                    "side={side} mask={mask:016x} cell={cell}"
                );
            }
        }
    }
}

struct Ground;
impl voxel::Generator for Ground {
    fn material(&self, _: u32, y: u32, _: u32) -> u16 {
        if y < 16 {
            1
        } else {
            0
        }
    }
}

// Original admission path, with the full recount kept independent of the
// production incremental helper and its reservation update.
#[allow(clippy::too_many_arguments)]
fn reference(
    b: &mut Bursts,
    w: &mut voxel::World,
    p: [u32; 3],
    mat: Mat,
    at: [i32; 3],
    side: u8,
    t: &Blast,
    room: &mut u32,
) -> Result<(), GroundClaimError> {
    let claim = Claim {
        at,
        motion_mj: 11,
        side,
        mat,
    };
    let (key, bit) = Bursts::claim_group(p, &claim, t);
    if *room == 0 && !b.groups.contains_key(&key) {
        return Err(GroundClaimError::NoRoom);
    }
    let mask = b.groups.get(&key).copied().unwrap_or(0);
    let delta = recount(mask, bit, key.3);
    if delta > 0 && *room == 0 {
        return Err(GroundClaimError::NoRoom);
    }
    if b.claims.len() >= t.max_claims as usize {
        return Err(GroundClaimError::ClaimLimit);
    }
    if b.claims.contains_key(&p) {
        return Err(GroundClaimError::PendingCell);
    }
    if !w.inside(p[0], p[1], p[2]) {
        return Err(GroundClaimError::Outside);
    }
    if w.on_frame(p[0], p[1], p[2]) {
        return Err(GroundClaimError::Frame);
    }
    b.groups.insert(key, mask | bit);
    b.reserved = (b.reserved as i64 + delta) as usize;
    b.claims.insert(p, claim);
    w.queue_edit(Edit::dig_if(p[0], p[1], p[2], mat));
    *room = (*room as i64 - delta).max(0) as u32;
    Ok(())
}

#[test]
fn ground_admission_and_release_match_original_reservations() {
    for side in 1..=4 {
        for initial_room in [0, 1, 7, 65536] {
            let mut t = crate::Tuning::from_ron(include_str!("../../../data/sim.ron"))
                .unwrap()
                .blast;
            t.small_piece_pct = 0;
            t.max_claims = 512;
            let mut actual = Bursts::default();
            let mut expected = Bursts::default();
            let mut w = voxel::World::new(64, 32, 64, 4096, Box::new(Ground));
            let mut reference_world = voxel::World::new(64, 32, 64, 4096, Box::new(Ground));
            let (mut room, mut reference_room) = (initial_room, initial_room);
            for i in 0..1600 {
                // Permutations build separated cells before joining bridges;
                // the second half revisits cells with overlapping blast owners.
                let k = (i * 173) % 1024;
                let mut p = [8 + k % 16, 4 + k / 16 % 4, 8 + k / 64];
                if i % 53 == 0 {
                    p[0] = 0;
                }
                if i % 71 == 0 {
                    p[2] = 64;
                }
                let mat = if i % 5 == 0 { 5 } else { 1 };
                let at = [20 + (i / 800) as i32, 16, 20];
                let a = actual.claim_voxel_checked(&mut w, p, mat, at, 11, side, &t, &mut room);
                let b = reference(
                    &mut expected,
                    &mut reference_world,
                    p,
                    mat,
                    at,
                    side,
                    &t,
                    &mut reference_room,
                );
                assert_eq!(a, b);
                assert_eq!(room, reference_room);
                assert_eq!(actual.reserved, expected.reserved);
                assert_eq!(actual.groups, expected.groups);
            }
            let mut a = Blob::new();
            let mut b = Blob::new();
            actual.save(&mut a);
            expected.save(&mut b);
            assert_eq!(a.v, b.v);
            assert_eq!(
                w.queued_edits().collect::<Vec<_>>(),
                reference_world.queued_edits().collect::<Vec<_>>()
            );
            let keys: Vec<_> = actual.claims.keys().copied().collect();
            for p in keys.into_iter().rev() {
                actual.take_claim(p, &t).unwrap();
                let recounted: usize = actual
                    .groups
                    .iter()
                    .map(|(key, &mask)| shape_component_count(mask, key.3) as usize)
                    .sum();
                assert_eq!(actual.reserved, recounted);
            }
            assert!(actual.groups.is_empty());
            assert_eq!(actual.reserved, 0);
        }
    }
}
#[test]
fn grouped_confirmation_matches_per_voxel_reservations_and_saved_state() {
    for side in 1..=4 {
        for cap in [0, 1, 32, 1024] {
            for slice in [1, 13, 1024] {
                for reserve in [0, 8] {
                    let mut t = crate::Tuning::from_ron(include_str!("../../../data/sim.ron"))
                        .unwrap()
                        .blast;
                    t.max_fragments = cap;
                    t.max_flying = cap;
                    let (mut actual, mut expected) = (Bursts::default(), Bursts::default());
                    let mut a = crate::blast::matter::Matter::new(&[]);
                    let mut b = crate::blast::matter::Matter::new(&[]);
                    let mut completed = Vec::new();
                    for i in 0..256u32 {
                        let k = i * 173 % 256;
                        let p = [8 + k % 8, 4 + k / 8 % 4, 8 + k / 32];
                        let mat = [1, 5, 3][(k / 64 % 3) as usize];
                        let claim = Claim {
                            at: [20 + (k / 128) as i32, 16, 20],
                            motion_mj: 11 + u64::from(k),
                            side,
                            mat,
                        };
                        actual.add_claim(p, claim, &t);
                        expected.add_claim(p, claim, &t);
                        // No-op removal, replaced material, successful dig,
                        // unrelated edit and repeated notification in one stream.
                        let old = match i % 11 {
                            0 => 0,
                            1 => 6,
                            _ => mat,
                        };
                        let edit = Edit::dig_if(p[0], p[1], p[2], mat);
                        completed.push((edit, old));
                        if i % 17 == 0 {
                            completed.push((edit, old));
                        }
                        if i % 29 == 0 {
                            completed.push((Edit::put(1, 1, 1, 2), 0));
                        }
                    }
                    for edits in completed.chunks(slice) {
                        actual.confirm_reserving(edits, &mut a.economy, &t, reserve);
                        expected.confirm_reserving_inner::<false>(
                            edits,
                            &mut b.economy,
                            &t,
                            reserve,
                        );
                        assert_eq!(actual.groups, expected.groups);
                        assert_eq!(actual.reserved, expected.reserved);
                        assert_eq!(actual.dusted, expected.dusted);
                        let (mut aa, mut bb) = (Blob::new(), Blob::new());
                        actual.save(&mut aa);
                        expected.save(&mut bb);
                        assert_eq!(
                            aa.v, bb.v,
                            "side={side} cap={cap} slice={slice} reserve={reserve}"
                        );
                        a.save(&mut aa);
                        b.save(&mut bb);
                        assert_eq!(aa.v, bb.v);
                        assert!(a.economy.balanced() && b.economy.balanced());
                    }
                    assert!(actual.claims.is_empty() && actual.groups.is_empty());
                    assert_eq!(actual.reserved, 0);
                }
            }
        }
    }
}
