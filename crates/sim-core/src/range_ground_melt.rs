//! Ordered terrain melt admission with capacity local to one claim-only pass.
use super::Matter;
use crate::heat::Heat;
use voxel::World;

impl Matter {
    pub(in crate::blast) fn melt_ground(&mut self, world: &mut World, heat: &mut Heat) {
        let mut room = None;
        // Heat::forget only removes stored heat. Preserve the transient source
        // list and its order, including blocked candidates, exactly as before.
        for i in 0..heat.melting.len() {
            let p = heat.melting[i];
            let mat = world.material(p[0], p[1], p[2]);
            let transform = self
                .phase_changes
                .iter()
                .flatten()
                .find(|t| t.from == mat)
                .copied();
            if let Some(t) = transform.filter(|t| {
                self.economy
                    .mats
                    .get(t.from)
                    .zip(self.economy.mats.get(t.to))
                    .is_some_and(|(a, b)| a.density == b.density)
            }) {
                if !world.on_frame(p[0], p[1], p[2])
                    && !world.has_pending_dig(p[0], p[1], p[2])
                    && !self.debris.claimed(p)
                {
                    // A phase change is a bounded transfer, not a fresh cold
                    // voxel. Apply this tick, conditional on its source still
                    // being present; never resurrect material removed earlier.
                    world.queue_transfer(voxel::Edit::replace_if(p[0], p[1], p[2], t.from, t.to));
                    heat.melted_total += 1;
                }
            } else if let Some(flow) = self
                .melt_flow
                .iter()
                .flatten()
                .find(|f| f.solid == mat)
                .copied()
                .filter(|f| {
                    self.economy
                        .mats
                        .get(f.solid)
                        .zip(self.economy.mats.get(f.liquid))
                        .is_some_and(|(a, b)| a.density == b.density)
                })
            {
                if !world.on_frame(p[0], p[1], p[2])
                    && !world.has_pending_dig(p[0], p[1], p[2])
                    && !self.debris.claimed(p)
                {
                    world.queue_transfer(voxel::Edit::replace_if(
                        p[0],
                        p[1],
                        p[2],
                        flow.solid,
                        flow.liquid,
                    ));
                    heat.mark_molten(p);
                    heat.melted_total += 1;
                }
            } else if self.melt_flow.iter().flatten().any(|f| f.liquid == mat) {
                // Already fluid: future heat deposits keep it hot.
            } else if self.melt_with_room(world, p, &mut room, true) {
                heat.forget(p);
                heat.melted_total += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx::blob::{Blob, Scan};

    struct Ground;
    impl voxel::Generator for Ground {
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            if [x, y, z] == [28, 3, 28] {
                99 // Unknown material must not consume a reservation.
            } else if y < 8 {
                [1, 2, 3, 5][((x + y + z) % 4) as usize]
            } else {
                0
            }
        }
    }
    fn fixture() -> (Matter, World, Heat) {
        let mut m = Matter::new(&[]);
        m.melt_flow = [None; 3]; // This fixture is the legacy fragment oracle.
        let mut w = World::new(32, 32, 32, 4096, Box::new(Ground));
        let mut h = Heat::default();
        for p in [[2, 2, 2], [10, 2, 10], [18, 2, 18]] {
            assert!(m.melt(&mut w, p));
        }
        w.flush();
        m.debris.confirm(w.completed(), &mut m.economy, &m.tuning);
        assert_eq!(m.debris.fragments.len(), 3);
        for f in &mut m.debris.fragments[..2] {
            f.resting = true;
        }
        // Existing reservations from another system reduce available capacity.
        assert!(m.melt(&mut w, [3, 3, 3]));
        h.melting = (4..12)
            .map(|x| [x, 3, 4])
            .chain([
                [4, 3, 4],
                [3, 3, 3],
                [0, 3, 4],
                [28, 3, 28],
                [4, 10, 4],
                [32, 3, 4],
            ])
            .collect();
        for p in h.melting.clone() {
            h.add(&w, p, 50_000);
        }
        (m, w, h)
    }
    fn eager(m: &mut Matter, w: &mut World, h: &mut Heat) {
        for p in h.melting.clone() {
            if m.melt(w, p) {
                h.forget(p);
                h.melted_total += 1;
            }
        }
    }
    fn bytes(m: &Matter, w: &World, h: &Heat) -> Vec<u8> {
        let mut b = Blob::new();
        m.save(&mut b);
        w.save(&mut b);
        w.save_transfers(&mut b);
        h.save(&mut b);
        b.v
    }
    fn same(a: &(Matter, World, Heat), b: &(Matter, World, Heat)) {
        let old = bytes(&a.0, &a.1, &a.2);
        let fast = bytes(&b.0, &b.1, &b.2);
        assert!(
            old == fast,
            "snapshot mismatch: lengths {} / {}, first differing byte {:?}",
            old.len(),
            fast.len(),
            old.iter().zip(&fast).position(|(a, b)| a != b)
        );
        assert_eq!(a.0.debris.hash(), b.0.debris.hash());
        assert_eq!(
            a.0.debris.fragment_pools(&a.0.tuning),
            b.0.debris.fragment_pools(&b.0.tuning)
        );
        assert_eq!(a.1.completed(), b.1.completed());
        assert_eq!(a.2.melting, b.2.melting);
        assert_eq!(
            a.0.admission.terrain_melt[..2],
            b.0.admission.terrain_melt[..2]
        );
        assert!(a.0.economy.balanced() && b.0.economy.balanced());
    }
    fn confirm(s: &mut (Matter, World, Heat)) {
        s.1.flush();
        s.0.debris
            .confirm(s.1.completed(), &mut s.0.economy, &s.0.tuning);
    }
    #[test]
    fn ground_melt_reservations_match_eager_caps_duplicates_and_partial_flush() {
        for resting in [0, 1, 8] {
            for claims in [0, 1, 3, 32] {
                for (flying, working) in [(0, 32), (2, 32), (32, 3), (32, 32)] {
                    let mut old = fixture();
                    let mut fast = fixture();
                    for s in [&mut old, &mut fast] {
                        s.0.tuning.max_resting_fragments = resting;
                        s.0.tuning.max_claims = claims;
                        s.0.tuning.max_flying = flying;
                        s.0.tuning.max_fragments = working;
                        s.1.max_edits_per_tick = 1;
                    }
                    for tick in 0..5 {
                        let start = fast.0.admission.terrain_melt;
                        eager(&mut old.0, &mut old.1, &mut old.2);
                        fast.0.melt_ground(&mut fast.1, &mut fast.2);
                        assert_eq!(fast.0.admission.terrain_melt[2] - start[2], 1);
                        same(&old, &fast);
                        confirm(&mut old);
                        confirm(&mut fast);
                        same(&old, &fast);
                        // The next pass must recount physical state and tuning.
                        if tick == 1 {
                            for s in [&mut old, &mut fast] {
                                s.0.tuning.max_claims = 32;
                                s.0.tuning.max_flying = 32;
                                s.0.tuning.max_fragments = 32;
                            }
                        }
                    }
                    if resting == 8 && claims == 32 && flying == 32 && working == 32 {
                        assert_eq!(fast.2.melted_total, 8);
                        assert!(
                            fast.0.debris.claims() > 0,
                            "partial flush leaves real pending work"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn ground_melt_empty_or_invalid_batch_does_not_scan_capacity() {
        let (mut m, mut w, mut h) = fixture();
        for candidates in [vec![], vec![[0, 3, 4], [28, 3, 28], [4, 10, 4], [32, 3, 4]]] {
            h.melting = candidates;
            let start = m.admission.terrain_melt;
            let old = bytes(&m, &w, &h);
            m.melt_ground(&mut w, &mut h);
            assert_eq!(old, bytes(&m, &w, &h));
            assert_eq!(m.admission.terrain_melt[2..], start[2..]);
        }
    }
    #[test]
    fn ground_melt_noop_confirmation_and_load_release_capacity() {
        let mut old = fixture();
        let mut fast = fixture();
        for s in [&mut old, &mut fast] {
            s.0.tuning.max_flying = 3;
            s.0.tuning.max_resting_fragments = 8;
            s.1.max_edits_per_tick = 4096;
            // Earlier dig wins; conditional melt is confirmed as NoOp.
            s.1.queue_dig(4, 3, 4);
        }
        eager(&mut old.0, &mut old.1, &mut old.2);
        fast.0.melt_ground(&mut fast.1, &mut fast.2);
        same(&old, &fast);
        assert_eq!(fast.2.melted_total, 1);
        assert_eq!(fast.0.debris.claims(), 2);
        confirm(&mut old);
        confirm(&mut fast);
        same(&old, &fast);
        assert_eq!(fast.0.debris.claims(), 0);
        assert_eq!(fast.0.debris.fragments.len(), 4, "NoOp creates no fragment");
        let mut saved = Blob::new();
        fast.0.save(&mut saved);
        let mut loaded = Matter::new(&[]);
        loaded.tuning = fast.0.tuning;
        loaded.melt_flow = fast.0.melt_flow; // Restore the fixture's legacy thermal configuration too.
        loaded.load(&mut Scan::new(&saved.v), &fast.1, &[]).unwrap();
        assert_eq!(loaded.admission, super::super::AdmissionProfile::default());
        fast.0 = loaded;
        old.0.admission = super::super::AdmissionProfile::default();
        eager(&mut old.0, &mut old.1, &mut old.2);
        fast.0.melt_ground(&mut fast.1, &mut fast.2);
        same(&old, &fast);
        assert_eq!(
            fast.2.melted_total, 2,
            "fresh admission uses the released NoOp slot"
        );
        confirm(&mut old);
        confirm(&mut fast);
        same(&old, &fast);
    }
}
