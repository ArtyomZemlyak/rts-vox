//! Hulls that meet at speed break metal where they touched (ADR-455).
//! Motion refuses the pose that would put two hulls into each other and
//! hands the range the contact; the range shoves the other hull and, over
//! the closing speed the data sets, prices the impact in fracture.
use super::*;

impl BlastRange {
    /// The pair `a`, `b` met closing at `closing`, world units a second
    /// from `a` towards `b`. It loses what a perfectly inelastic impact
    /// loses, ½μw² with μ the pair's reduced mass and w the closing speed,
    /// and the data share of that breaks cells, half at each face. The
    /// range gun is a witness and takes no damage: its half is not spent.
    pub(super) fn contact_damage(&mut self, a: usize, b: usize, closing: phys::V3) {
        let t = self.matter.tuning;
        let mm_s = |v: Fx| ((i128::from(v.raw()) * i128::from(voxel::unit::WORLD_MM)) >> 32) as i64;
        let w = [closing.x, closing.y, closing.z].map(mm_s);
        let w2: u128 = w.iter().map(|v| u128::from(v.unsigned_abs()).pow(2)).sum();
        if t.contact_deform_pct == 0 || w2 <= u128::from(t.contact_min_mm_s).pow(2) {
            return;
        }
        let tonnes = |id: usize| {
            let m = if id == self.units.len() {
                &self.gun.motion
            } else {
                &self.units[id].motion
            };
            u64::from(m.chassis.mass.max(1).unsigned_abs())
        };
        let (ma, mb) = (tonnes(a), tonnes(b));
        let reduced_g = (ma * mb * 1_000_000 / (ma + mb)).min(u64::from(u32::MAX)) as u32;
        let half = voxel::impact::kinetic_mj(reduced_g, w)
            .saturating_mul(u64::from(t.contact_deform_pct))
            / 200;
        for (id, other) in [(a, b), (b, a)] {
            if id < self.units.len() {
                self.crush_face(id, other, half);
            }
        }
    }

    /// Breaks the face of unit `id` turned to hull `other` with `mj`
    /// millijoules. The cells farthest along the line to the other hull,
    /// within its height, break first, ties by part and linear index; what
    /// cannot pay for a whole cell dents the next one short of breaking,
    /// as a round's leftover does.
    fn crush_face(&mut self, id: usize, other: usize, mj: u64) {
        let (to, (lo, hi)) = {
            let o = if other == self.units.len() {
                &self.gun
            } else {
                &self.units[other]
            };
            (o.motion.pos, o.bounds())
        };
        let u = &self.units[id];
        let line = [to[0].sub(u.motion.pos[0]), to[2].sub(u.motion.pos[2])];
        let turret = u.root_frame(usize::from(u.body.turret_y));
        let mut face: Vec<(Fx, usize, usize, [usize; 3])> = Vec::new();
        for part in 0..=u.body.segments.len() {
            let body = match part {
                0 => &u.body,
                k if u.seg_gone[k - 1] => continue,
                k => &u.body.segments[k - 1].body,
            };
            let [nx, ny, nz] = [body.vol.nx, body.vol.ny, body.vol.nz].map(usize::from);
            for z in 0..nz {
                for y in 0..ny {
                    let frame = match part {
                        0 if y >= usize::from(u.body.turret_y) => &turret,
                        0 => &u.pose,
                        k => &u.seg_frames[k - 1],
                    };
                    for x in 0..nx {
                        if !body.vol.get(x, y, z) {
                            continue;
                        }
                        let p = frame.world(
                            [x, y, z].map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))),
                        );
                        let height = p.y.to_int_trunc();
                        if height < lo[1] || height > hi[1] {
                            continue;
                        }
                        let reach = p.x.mul(line[0]).add(p.z.mul(line[1]));
                        face.push((reach, part, (z * ny + y) * nx + x, [x, y, z]));
                    }
                }
            }
        }
        face.sort_unstable_by(|p, q| q.0.cmp(&p.0).then((p.1, p.2).cmp(&(q.1, q.2))));
        let price = self.matter.crush_price().max(1);
        let before = (u.removed.len(), u.seg_removed.len());
        let Self {
            matter,
            units,
            config,
            ..
        } = self;
        let u = &mut units[id];
        let receive = |u: &mut RangeUnit, part: usize, cell: [usize; 3], work: u32| {
            if part == 0 {
                u.receive_cell(cell, work, config);
            } else {
                u.receive_seg_cell(part - 1, cell, work, config);
            }
        };
        matter.aside(mj, |m| {
            for &(_, part, _, cell) in &face {
                if u.section_holds(part, cell.map(|v| v as u8)) {
                    continue;
                }
                let need = u.hit_resistance(Some((part, cell)), config).max(1);
                let frame = if part == 0 {
                    u.root_frame(cell[1])
                } else {
                    u.seg_frames[part - 1]
                };
                if m.remaining_mj < price
                    || !matches!(
                        m.crush(id, u, part, cell, &frame),
                        matter::ClaimResult::Applied
                    )
                {
                    let dent = u128::from(need) * u128::from(m.remaining_mj) / u128::from(price);
                    receive(u, part, cell, dent.min(u128::from(need - 1)) as u32);
                    break;
                }
                if part == 0 {
                    u.detached += 1;
                } else {
                    u.seg_detached += 1;
                }
                receive(u, part, cell, need);
            }
        });
        if (u.removed.len(), u.seg_removed.len()) != before {
            u.refresh_damage(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two tanks nose to nose a unit apart on open ground, each driving
    /// at the other at `speed` world units a second.
    fn head_on(speed: i32) -> BlastRange {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.course = None;
        layout.blocks.clear();
        layout.clear_min = [64, 64];
        layout.clear_max = [600, 600];
        layout.units = [300, 361]
            .map(|x| modding::range::RangeUnitDef {
                allied: false,
                kind: RangeUnitKind::Tank,
                at: [x, FLOOR, 336],
            })
            .to_vec();
        let mut r = BlastRange::with_layout(c, layout).unwrap();
        r.patrols_running = true;
        for _ in 0..20 {
            r.step();
        }
        for (id, goal, head) in [(0, 500, 0), (1, 100, fx::angle::FULL / 2)] {
            let m = &mut r.units[id].motion;
            m.enabled = true;
            m.destination = Some([goal, 336]);
            m.path = vec![[goal, 336]];
            m.drive.head = head;
            m.drive.vel = Fx::from_int(speed);
        }
        r
    }

    /// **Hulls that meet at speed break where they touched** (ADR-455).
    /// A crash only shoved: two tanks met head-on at full speed and both
    /// drove away whole.
    #[test]
    fn a_head_on_crash_breaks_both_faces() {
        let mut r = head_on(40);
        let held = |r: &BlastRange, id| r.matter.economy.ledger.held(r.matter.body_container(id));
        let bodies = [held(&r, 0), held(&r, 1)];
        let painted = |r: &BlastRange| {
            r.matter
                .debris
                .fragments
                .iter()
                .filter(|f| f.paint.is_some())
                .map(|f| (f.position, f.velocity, f.count as usize))
                .collect::<Vec<_>>()
        };
        assert!(painted(&r).is_empty());
        r.step();
        let lost = [0, 1].map(|id| r.units[id].removed.len() + r.units[id].seg_removed.len());
        assert!(lost[0] > 0 && lost[0] == lost[1], "cells broken: {lost:?}");
        // Closing at twice the drive, the pair loses ½μw²; the data share
        // of it, half on each side, pays for whole cells at the crush
        // price. One hull's speed alone would pay for a quarter of that.
        let m = u128::from(r.units[0].motion.chassis.mass.unsigned_abs());
        let reduced_g = m * m * 1_000_000 / (2 * m);
        let w = 2 * 40 * u128::from(voxel::unit::WORLD_MM);
        let half =
            reduced_g * w * w / 2_000_000 * u128::from(r.matter.tuning.contact_deform_pct) / 200;
        assert_eq!(lost[0] as u128, half / u128::from(r.matter.crush_price()));
        // What broke are the noses that met, not the far ends.
        for u in &r.units[..2] {
            let nose = usize::from(u.body.vol.nx) - 3;
            assert!(
                u.removed.iter().all(|c| usize::from(c[0]) >= nose),
                "broke away from the nose: {:?}",
                u.removed
            );
        }
        // The matter left each body for pieces that drop where the hulls
        // met, not thrown.
        let per = r.matter.economy.per_voxel(3);
        for id in 0..2 {
            assert_eq!(bodies[id] - held(&r, id), per * lost[id] as u64);
        }
        let pieces = painted(&r);
        assert_eq!(pieces.iter().map(|p| p.2).sum::<usize>(), lost[0] + lost[1]);
        let mid = r.units[0].motion.pos[0]
            .add(r.units[1].motion.pos[0])
            .div(Fx::from_int(2));
        assert!(
            pieces
                .iter()
                .all(|(p, v, _)| p.x.sub(mid).abs() < Fx::from_int(4)
                    && v.x == Fx::ZERO
                    && v.z == Fx::ZERO),
            "a piece left the place of the impact: {pieces:?}"
        );
    }

    /// **A slow nudge breaks nothing** (ADR-455): under the closing speed
    /// the data sets, hulls shove each other and neither is marked.
    #[test]
    fn a_slow_nudge_breaks_and_dents_nothing() {
        // From rest, a unit apart: they meet at a walking pace.
        let mut r = head_on(0);
        for _ in 0..40 {
            r.step();
            if r.units[0].motion.shove.x < Fx::ZERO {
                break;
            }
        }
        assert!(
            r.units[0].motion.shove.x < Fx::ZERO,
            "the nudge made no contact"
        );
        for u in &r.units[..2] {
            assert!(u.removed.is_empty() && u.seg_removed.is_empty());
            assert!(u.dents.is_empty() && u.segment_dents().is_empty());
        }
    }

    /// **The crash replays exactly** (ADR-455): a snapshot taken on the
    /// tick before the impact replays to the same hulls and pieces.
    #[test]
    fn a_crash_replays_exactly_from_a_snapshot() {
        let mut a = head_on(40);
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        for _ in 0..30 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        assert!(a.units[0].detached > 0);
    }
}
