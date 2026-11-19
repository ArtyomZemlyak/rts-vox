use super::*;
use std::collections::BTreeSet;

fn equal(front: &Front, reference: &BTreeSet<Pair>) {
    assert_eq!(front.len(), reference.len());
    assert_eq!(front.first(), reference.first().copied());
    for pivot in reference
        .iter()
        .copied()
        .chain([(0, 0), (u64::MAX, u64::MAX)])
    {
        assert_eq!(
            front.after(pivot),
            reference
                .range((std::ops::Bound::Excluded(pivot), std::ops::Bound::Unbounded))
                .next()
                .copied()
        );
    }
    assert_eq!(
        front.iter().collect::<Vec<_>>(),
        reference.iter().copied().collect::<Vec<_>>()
    );
    assert_eq!(
        front
            .words
            .values()
            .map(|(m, _)| m.count_ones() as usize)
            .sum::<usize>()
            + front
                .nearby
                .values()
                .map(|&(word, _)| (0..8).filter(|&i| Front::lane(word, i) != 0).count())
                .sum::<usize>()
            + front.branches.len(),
        front.len()
    );
    for &(mask, verified) in front.words.values() {
        assert_ne!(mask, 0);
        assert_eq!(verified & !mask, 0, "verified contact bit outlived present bit");
    }
    for &(word, verified) in front.nearby.values() {
        assert_ne!(word, 0);
        let lanes: Vec<_> = (0..8)
            .map(|i| Front::lane(word, i))
            .take_while(|&v| v != 0)
            .collect();
        assert!(lanes.windows(2).all(|pair| pair[0] < pair[1]));
        let valid_v = ((1u16 << lanes.len()) - 1) as u8;
        assert_eq!(verified & !valid_v, 0, "verified nearby lane outlived active lanes");
        if lanes.len() < 8 {
            assert_eq!(word >> (lanes.len() * 16), 0);
        }
    }
}

#[test]
fn relative_seeds_keep_overflow_identity_after_inline_room_opens() {
    let key = super::super::fell_key;
    for cell in [[32, 32, 32], [-32, -32, -32], [1_048_570, -1_048_570, 0]] {
        let p = key(cell);
        let pairs: Vec<_> = (-24..=24)
            .map(|z| (p, key([cell[0], cell[1], cell[2] + z])))
            .chain([-16, 15, 16].into_iter().flat_map(|x| {
                [-16, 15, 16].into_iter().map(move |y| {
                    // Keep the extreme-coordinate fixture in the saved domain.
                    let base = [32, 32, cell[2]];
                    (key(base), key([base[0] + x, base[1] + y, base[2]]))
                })
            }))
            .collect();
        let mut front = Front::default();
        let mut reference = BTreeSet::new();
        for &pair in pairs.iter().rev() {
            assert_eq!(front.insert(pair), reference.insert(pair));
            equal(&front, &reference);
        }
        assert!(!front.nearby.is_empty());
        assert!(!front.branches.is_empty());
        for &pair in pairs.iter().step_by(3) {
            assert_eq!(front.remove(&pair), reference.remove(&pair));
            equal(&front, &reference);
        }
        // Reinsert every seed after removing inline lanes, including seeds
        // still in overflow. No pair may migrate into a duplicate record.
        for &pair in &pairs {
            assert_eq!(front.insert(pair), reference.insert(pair));
            assert_eq!(front.contains(&pair), reference.contains(&pair));
            equal(&front, &reference);
        }
        let restored: Front = front.iter().collect();
        equal(&restored, &reference);
        while let Some(pair) = reference.pop_first() {
            assert_eq!(front.pop_first(), Some(pair));
            equal(&front, &reference);
        }
    }
}

#[test]
fn relative_code_roundtrips_every_lane_value_and_signed_domain_edges() {
    let key = super::super::fell_key;
    for cell in [[0, 0, 0], [-1_048_560; 3], [1_048_560; 3]] {
        let p = key(cell);
        let mut previous = None;
        for x in -16..16 {
            for y in -16..16 {
                for z in -16..16 {
                    let pair = (p, key([cell[0] + x, cell[1] + y, cell[2] + z]));
                    let code = Front::relative(pair).unwrap();
                    assert_eq!(Front::absolute(p, code), pair);
                    assert!(previous.is_none_or(|old| old < code));
                    previous = Some(code);
                }
            }
        }
        for delta in [[-17, 0, 0], [0, 16, 0], [0, 0, -17]] {
            // The storage fast path is not a radius cutoff.
            let pair = (key([0; 3]), key(delta));
            assert_eq!(Front::relative(pair), None);
            let mut front: Front = [pair].into_iter().collect();
            assert!(front.contains(&pair));
            assert_eq!(front.pop_first(), Some(pair));
        }
    }
}

#[test]
fn word_front_keeps_every_seed_and_order_across_bit_boundaries() {
    let pairs: Vec<_> = [0, 1, 63, 64, u64::MAX]
        .into_iter()
        .flat_map(|p| {
            [0, 1, 62, 63, 64, 65, 127, 128, u64::MAX - 1, u64::MAX]
                .into_iter()
                .map(move |seed| (p, seed))
        })
        .collect();
    let mut reference = BTreeSet::new();
    let mut front = Front::default();
    for &pair in pairs.iter().rev().chain(pairs.iter()) {
        assert_eq!(front.insert(pair), reference.insert(pair));
        equal(&front, &reference);
    }
    for &pair in pairs.iter().step_by(2).chain(pairs.iter().step_by(2)) {
        assert_eq!(front.remove(&pair), reference.remove(&pair));
        equal(&front, &reference);
    }
    while let Some(pair) = reference.pop_first() {
        assert_eq!(front.pop_first(), Some(pair));
        // Refused work is still present with precisely the same identity/order.
        assert!(front.insert(pair));
        assert_eq!(front.pop_first(), Some(pair));
        equal(&front, &reference);
    }
    assert_eq!(front.pop_first(), None);
    assert_eq!(front.words(), 0);
}

#[test]
fn word_front_matches_set_through_revisits_removals_and_clear() {
    let mut front = Front::default();
    let mut reference = BTreeSet::new();
    let mut rng = 32847u64;
    for step in 0..30_000 {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let p = (rng >> 32) % 256;
        let seed = if step % 3 == 0 {
            p
        } else {
            ((rng >> 12) % 256) | ((rng & 1) << 63)
        };
        let pair = (p, seed);
        match (rng >> 8) % 5 {
            0 | 1 => assert_eq!(front.insert(pair), reference.insert(pair)),
            2 => assert_eq!(front.remove(&pair), reference.remove(&pair)),
            3 => assert_eq!(front.pop_first(), reference.pop_first()),
            _ => assert_eq!(front.contains(&pair), reference.contains(&pair)),
        }
        if step % 127 == 0 {
            equal(&front, &reference);
        }
        if step % 7919 == 0 {
            front.clear();
            reference.clear();
            equal(&front, &reference);
        }
    }
    equal(&front, &reference);
}

#[test]
fn cursor_walk_matches_after_across_mutations_and_boundaries() {
    use std::ops::Bound::{Excluded, Unbounded};
    let key = super::super::fell_key;
    let mut front = Front::default();
    let mut reference = BTreeSet::new();
    let mut rng = 918_273_645u64;
    for i in 0..4_096 {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let p = if i < 64 {
            [0, 1, 63, 64, 127, 128, u64::MAX][i % 7]
        } else {
            key([(rng as i32) & 63, ((rng >> 8) as i32) & 31, ((rng >> 16) as i32) & 63])
        };
        let seed = if i % 2 == 0 {
            p
        } else if i % 5 == 0 {
            u64::MAX - (i as u64 % 16)
        } else {
            key([
                ((rng as i32) & 63) + (((rng >> 24) as i32) % 39 - 19),
                (((rng >> 8) as i32) & 31) - (((rng >> 32) as i32) & 3),
                ((rng >> 16) as i32) & 63,
            ])
        };
        assert_eq!(front.insert((p, seed)), reference.insert((p, seed)));
    }
    let mut cursor = Cursor::default();
    let mut after = None;
    let mut steps = 0usize;
    while let Some(expected) = match after {
        Some(prev) => reference.range((Excluded(prev), Unbounded)).next().copied(),
        None => reference.first().copied(),
    } {
        let got = front.next(after, &mut cursor);
        assert_eq!(got, Some(expected), "step={steps} after={after:?}");
        if steps % 17 == 0 {
            assert!(front.remove(&expected));
            assert!(reference.remove(&expected));
            cursor.invalidate();
        }
        after = Some(expected);
        steps += 1;
    }
    assert_eq!(front.next(after, &mut cursor), None);
    equal(&front, &reference);
}

/// Build/query/drain the exact saved frontier, independent of simulation work.
/// Run alone in release with FELL_BENCH_SNAPSHOT pointing at a battle snapshot.
#[test]
#[ignore = "manual saved-frontier measurement; run alone in release"]
fn measure_saved_felling_front() {
    use std::{hint::black_box, time::Instant};
    let path = std::env::var("FELL_BENCH_SNAPSHOT").expect("snapshot path");
    let config =
        modding::explosion::ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron"))
            .unwrap();
    let snapshot = std::fs::read(path).unwrap();
    let sim = crate::blast::BlastRange::restore(config, &snapshot).unwrap();
    assert_eq!(sim.snapshot(), snapshot, "legacy snapshot bytes changed");
    let pairs: Vec<_> = sim.matter.felling.iter().collect();
    if let Ok(path) = std::env::var("FELL_BENCH_PAIRS") {
        let bytes: Vec<_> = pairs
            .iter()
            .flat_map(|&(p, seed)| p.to_le_bytes().into_iter().chain(seed.to_le_bytes()))
            .collect();
        std::fs::write(path, bytes).unwrap();
    }
    let queries: Vec<_> = pairs.iter().step_by(3).map(|&(p, _)| (p, p)).collect();
    println!(
        "records={} words={} queries={} contact_words={} nearby_words={} overflow={}",
        pairs.len(),
        sim.matter.felling.words(),
        queries.len(),
        sim.matter.felling.words.len(),
        sim.matter.felling.nearby.len(),
        sim.matter.felling.branches.len()
    );
    for run in [false, true, true, false] {
        let start = Instant::now();
        let mut front = if run {
            pairs.iter().copied().collect::<Front>()
        } else {
            Front::reference(pairs.iter().copied())
        };
        let build = start.elapsed().as_micros();
        let start = Instant::now();
        for _ in 0..8 {
            for pair in &queries {
                black_box(front.contains(black_box(pair)));
            }
        }
        let query = start.elapsed().as_micros();
        let start = Instant::now();
        for _ in 0..8 {
            for &pair in &queries {
                black_box(front.insert(black_box(pair)));
            }
        }
        let insert = start.elapsed().as_micros();
        let start = Instant::now();
        let mut count = 0;
        while let Some(pair) = front.pop_first() {
            black_box(pair);
            count += 1;
        }
        let drain = start.elapsed().as_micros();
        assert_eq!(front.len(), 0);
        println!("packed={run} build_us={build} query_us={query} insert_us={insert} drain_us={drain} drained={count}");
    }
}


#[test]
fn cursor_seek_insert_and_clear_resync_match_sorted_frontier() {
    use std::ops::Bound::{Excluded,Unbounded};
    let mut front=Front::default();let mut reference=BTreeSet::new();
    let initial=[(0,0),(63,0),(63,63),(63,64),(64,63),(64,64),(64,65),(127,127),(u64::MAX,u64::MAX)];
    for pair in initial {front.insert(pair);reference.insert(pair);}
    let mut cursor=Cursor::default();
    for after in [None,Some((63,0)),Some((63,62)),Some((63,63)),Some((63,64)),Some((64,63)),Some((u64::MAX,0)),Some((u64::MAX,u64::MAX))] {
        cursor.invalidate();let mut at=after;
        loop {
            let expected=match at {None=>reference.first().copied(),Some(p)=>reference.range((Excluded(p),Unbounded)).next().copied()};
            assert_eq!(front.next(at,&mut cursor),expected,"after={at:?}");
            let Some(p)=expected else{break;};at=Some(p);
        }
        for pair in [(1,1),(63,1),(64,0),(65,64),(128,129)] {assert_eq!(front.insert(pair),reference.insert(pair));}
        cursor.invalidate();equal(&front,&reference);
    }
    front.clear();reference.clear();cursor.invalidate();assert_eq!(front.next(None,&mut cursor),None);
    front.insert((64,64));cursor.invalidate();assert_eq!(front.next(None,&mut cursor),Some((64,64)));
}

#[test]
fn verified_runs_and_lane_shifts_preserve_exact_order_and_profile_invalidation() {
    let key = super::super::fell_key;
    let mut front = Front::default();
    let mut reference = BTreeSet::new();
    front.sync_profile([256, 64, 256], &[5, 4]);
    let base = [16, 8, 64];
    for dz in 0..20 {
        let p = key([base[0], base[1], base[2] + dz]);
        assert!(front.insert_contact(p, dz < 14).0);
        assert!(reference.insert((p, p)));
    }
    // Insert nearby lanes out of order and mark middle lanes verified; verify shifts keep verified flags aligned.
    let p_near = key([20, 8, 64]);
    let s2 = key([20, 8, 66]);
    let s0 = key([20, 8, 62]);
    let s1 = key([20, 8, 65]);
    let s3 = key([20, 8, 68]);
    assert!(front.insert((p_near, s2)) && reference.insert((p_near, s2)));
    front.mark_verified(&(p_near, s2));
    assert!(front.insert((p_near, s0)) && reference.insert((p_near, s0)));
    assert!(front.insert((p_near, s3)) && reference.insert((p_near, s3)));
    assert!(front.insert((p_near, s1)) && reference.insert((p_near, s1)));
    front.mark_verified(&(p_near, s1));
    equal(&front, &reference);

    // Check run batching stops at max_take=6 and stops before unverified dz=14..20.
    let mut cursor = Cursor::default();
    let run1 = front.peek_verified_contact_run(None, 6, &mut cursor).unwrap();
    assert_eq!(run1.3, 6);
    assert_eq!(run1.1, key([base[0], base[1], base[2]]));
    assert_eq!(run1.2, key([base[0], base[1], base[2] + 5]));
    front.advance_contact_word(run1.0, run1.4, run1.5, &mut cursor);
    let after1 = Some((run1.2, run1.2));

    let run2 = front.peek_verified_contact_run(after1, 64, &mut cursor).unwrap();
    assert_eq!(run2.3, 8, "run must stop at first unverified bit dz=14");
    assert_eq!(run2.2, key([base[0], base[1], base[2] + 13]));
    front.advance_contact_word(run2.0, run2.4, run2.5, &mut cursor);
    let mut at = Some((run2.2, run2.2));

    let mut verified_seen = Vec::new();
    while let Some((pair, v)) = front.next_verified(at, &mut cursor) {
        if pair.0 == p_near && v {
            verified_seen.push(pair.1);
        }
        at = Some(pair);
    }
    assert_eq!(verified_seen, vec![s1, s2], "lane insertion shifts must preserve exact verified lane owners");

    // Removing s0 shifts s1 and s2 down without losing their verified flags.
    assert!(front.remove(&(p_near, s0)) && reference.remove(&(p_near, s0)));
    equal(&front, &reference);
    cursor.invalidate();
    let mut verified_after_remove = Vec::new();
    let mut at2 = None;
    while let Some((pair, v)) = front.next_verified(at2, &mut cursor) {
        if pair.0 == p_near && v {
            verified_after_remove.push(pair.1);
        }
        at2 = Some(pair);
    }
    assert_eq!(verified_after_remove, vec![s1, s2]);

    // Changing profile clears all verified flags while preserving every queued pair.
    front.sync_profile([256, 64, 256], &[4]);
    equal(&front, &reference);
    cursor.invalidate();
    let mut at3 = None;
    while let Some((pair, v)) = front.next_verified(at3, &mut cursor) {
        assert!(!v, "changed material profile must clear verified flag on {pair:?}");
        at3 = Some(pair);
    }
}


#[test]
fn verified_full_words_lanes_and_context_match_scalar_order() {
    let key=super::super::fell_key;let dims=[256,64,256];
    let mut front=Front::default();let mut reference=BTreeSet::new();front.sync_profile(dims,&[5,4]);
    for z in 64..128 {let p=key([20,8,z]);front.insert_contact(p,true);reference.insert((p,p));}
    let mut cursor=Cursor::default();let run=front.peek_verified_contact_run(None,64,&mut cursor).unwrap();
    assert_eq!(run.3,64);assert_eq!(run.1,key([20,8,64]));assert_eq!(run.2,key([20,8,127]));
    let owner=key([20,8,96]);let mut marked=BTreeSet::new();
    for dz in [4,-3,2,-1,1,-2,3,-4,5] {
        let pair=(owner,key([20,8,96+dz]));assert!(front.insert(pair));reference.insert(pair);
        if dz!=5 && dz%2!=0 {front.mark_verified(&pair);marked.insert(pair);}
    }
    equal(&front,&reference);
    for budget in [0,1,2,7,8,63,64,65,u32::MAX] {
        let mut cursor=Cursor::default();let mut after=None;let mut got=Vec::new();
        loop {
            if let Some((word,first,last,n,remaining,verified))=front.peek_verified_contact_run(after,budget,&mut cursor) {
                let batch:Vec<_>=reference.iter().copied().filter(|p| after.is_none_or(|a|*p>a) && *p<=(last,last)).collect();
                assert_eq!(batch.len(),n as usize);assert_eq!(batch.first().copied(),Some((first,first)));assert!(batch.iter().all(|p|p.0==p.1));
                got.extend(batch);front.advance_contact_word(word,remaining,verified,&mut cursor);after=Some((last,last));
            } else if let Some((pair,verified))=front.next_verified(after,&mut cursor) {
                assert_eq!(verified,pair.0==pair.1 || marked.contains(&pair));got.push(pair);after=Some(pair);
            } else {break;}
        }
        assert_eq!(got,reference.iter().copied().collect::<Vec<_>>(),"budget={budget}");
    }
    for dz in [-4,-3,-2,-1,1,2,3,4] {
        let pair=(owner,key([20,8,96+dz]));assert_eq!(front.remove(&pair),reference.remove(&pair));marked.remove(&pair);equal(&front,&reference);
    }
    // Reordering the same material set keeps proof bits. Dimensions/material
    // changes, unsupported long profiles, and clear invalidate all hints.
    front.sync_profile(dims,&[4,5]);let mut c=Cursor::default();assert!(front.next_verified(None,&mut c).unwrap().1);
    for (next_dims,next_materials) in [(dims,vec![4]),([256,65,256],vec![4]),(dims,vec![1,2,3,4,5])] {
        front.sync_profile(next_dims,&next_materials);let mut c=Cursor::default();let mut after=None;
        while let Some((pair,verified))=front.next_verified(after,&mut c) {assert!(!verified);after=Some(pair);}
        equal(&front,&reference);
    }
    while let Some(pair)=reference.pop_first() {assert_eq!(front.pop_first(),Some(pair));equal(&front,&reference);}
    front.sync_profile(dims,&[4]);front.insert_contact(key([20,8,64]),true);front.clear();assert_eq!(front.len(),0);assert!(front.verified_profile.is_none());
}
