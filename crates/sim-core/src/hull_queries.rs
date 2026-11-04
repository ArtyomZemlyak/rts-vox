//! Immutable hull-contact queries for one physical phase.
use super::{hull_box_overlap, hull_hit, Hull};
use fx::Fx;

const LEAF: usize = 4;
const MIN_HULLS: usize = 16;

/// Derived broad phase over the borrowed, immutable source roster.
/// Exact voxel contacts and first-source tie breaking remain authoritative.
pub struct HullQueries<'h, 's> {
    hulls: &'h [Hull<'s>],
    nodes: Vec<Node>,
    order: Vec<usize>,
    centres: [[i64; 2]; 2],
}
struct Contact {
    p: [Fx; 3],
    low: [Fx; 3],
    high: [Fx; 3],
    lo: [Fx; 3],
    hi: [Fx; 3],
    reach: Fx,
    bottom: Fx,
    top: Fx,
    skip: u64,
}
struct Node {
    lo: [Fx; 3],
    hi: [Fx; 3],
    start: usize,
    end: usize,
    first: usize,
    children: Option<[usize; 2]>,
    moving: bool,
}
impl<'h, 's> HullQueries<'h, 's> {
    pub fn new(hulls: &'h [Hull<'s>]) -> Self {
        let mut out = Self::scalar(hulls);
        if hulls.len() < MIN_HULLS {
            return out;
        }
        let boxes: Option<Vec<_>> = hulls
            .iter()
            .map(|h| {
                if h.half.iter().any(|v| *v < Fx::ZERO) || h.height < Fx::ZERO {
                    return None;
                }
                let extent = h.half[0].raw().checked_add(h.half[1].raw())?;
                let lo = [
                    Fx(h.pos[0].raw().checked_sub(extent)?),
                    h.pos[1],
                    Fx(h.pos[2].raw().checked_sub(extent)?),
                ];
                let hi = [
                    Fx(h.pos[0].raw().checked_add(extent)?),
                    Fx(h.pos[1].raw().checked_add(h.height.raw())?),
                    Fx(h.pos[2].raw().checked_add(extent)?),
                ];
                Some((lo, hi))
            })
            .collect();
        let Some(boxes) = boxes else {
            return out;
        };
        out.centres = [[i64::MAX, i64::MIN]; 2];
        for h in hulls {
            for (a, k) in [0, 2].into_iter().enumerate() {
                out.centres[a][0] = out.centres[a][0].min(h.pos[k].raw());
                out.centres[a][1] = out.centres[a][1].max(h.pos[k].raw());
            }
        }
        out.order = (0..hulls.len()).collect();
        out.nodes.reserve(hulls.len());
        out.build(&boxes, 0, hulls.len());
        out
    }
    pub(super) fn scalar(hulls: &'h [Hull<'s>]) -> Self {
        Self {
            hulls,
            nodes: Vec::new(),
            order: Vec::new(),
            centres: [[0; 2]; 2],
        }
    }
    fn build(&mut self, boxes: &[([Fx; 3], [Fx; 3])], start: usize, end: usize) -> usize {
        let index = self.nodes.len();
        let first = *self.order[start..end].iter().min().unwrap();
        let moving = self.order[start..end]
            .iter()
            .any(|&i| self.hulls[i].vel != [Fx::ZERO; 3]);
        self.nodes.push(Node {
            lo: boxes[first].0,
            hi: boxes[first].1,
            start,
            end,
            first,
            children: None,
            moving,
        });
        let (lo, hi, children) = if end - start <= LEAF {
            // Spatial partitioning is derived only. A leaf still examines
            // canonical source slots in order, and a hit bounds later work.
            self.order[start..end].sort_unstable();
            let (mut lo, mut hi) = boxes[first];
            for &i in &self.order[start..end] {
                let (a, b) = boxes[i];
                for k in 0..3 {
                    lo[k] = lo[k].min(a[k]);
                    hi[k] = hi[k].max(b[k]);
                }
            }
            (lo, hi, None)
        } else {
            let span = |axis: usize| {
                let mut lo = i64::MAX;
                let mut hi = i64::MIN;
                for &i in &self.order[start..end] {
                    let raw = self.hulls[i].pos[axis].raw();
                    lo = lo.min(raw);
                    hi = hi.max(raw);
                }
                i128::from(hi) - i128::from(lo)
            };
            let axis = if span(0) >= span(2) { 0 } else { 2 };
            let hulls = self.hulls;
            self.order[start..end].sort_unstable_by_key(|&i| (hulls[i].pos[axis].raw(), i));
            let middle = start + (end - start) / 2;
            let left = self.build(boxes, start, middle);
            let right = self.build(boxes, middle, end);
            let lo = [0, 1, 2].map(|k| self.nodes[left].lo[k].min(self.nodes[right].lo[k]));
            let hi = [0, 1, 2].map(|k| self.nodes[left].hi[k].max(self.nodes[right].hi[k]));
            let children = if self.nodes[left].first <= self.nodes[right].first {
                [left, right]
            } else {
                [right, left]
            };
            (lo, hi, Some(children))
        };
        self.nodes[index] = Node {
            lo,
            hi,
            start,
            end,
            first,
            children,
            moving,
        };
        index
    }
    /// First exact contact in canonical source order, respecting the skip mask.
    pub fn hit(&self, p: [Fx; 3], low: [Fx; 3], high: [Fx; 3], skip: u64) -> Option<&'h Hull<'s>> {
        self.hit_with(p, low, high, skip, false, &mut |_| {})
    }
    pub fn hit_moving(
        &self,
        p: [Fx; 3],
        low: [Fx; 3],
        high: [Fx; 3],
        skip: u64,
    ) -> Option<&'h Hull<'s>> {
        self.hit_with(p, low, high, skip, true, &mut |_| {})
    }
    fn hit_with(
        &self,
        p: [Fx; 3],
        low: [Fx; 3],
        high: [Fx; 3],
        skip: u64,
        moving_only: bool,
        examined: &mut impl FnMut(usize),
    ) -> Option<&'h Hull<'s>> {
        if self.nodes.is_empty() {
            return self.scalar_hit(p, low, high, skip, moving_only);
        }
        let reach = low[0].abs().max(high[0]).max(low[2].abs()).max(high[2]);
        let (bottom, top) = (p[1].add(low[1]), p[1].add(high[1]));
        let bounds = || -> Option<([Fx; 3], [Fx; 3])> {
            if reach < Fx::ZERO || bottom > top {
                return None;
            }
            // The scalar predicate subtracts centres with wrapping Fx. If a
            // difference can wrap (or abs(MIN) is possible), do not replace it
            // by an ordered interval test, even for unusual external queries.
            for (a, k) in [0, 2].into_iter().enumerate() {
                for centre in self.centres[a] {
                    if p[k].raw().checked_sub(centre)? == i64::MIN {
                        return None;
                    }
                }
            }
            Some((
                [
                    Fx(p[0].raw().checked_sub(reach.raw())?),
                    bottom,
                    Fx(p[2].raw().checked_sub(reach.raw())?),
                ],
                [
                    Fx(p[0].raw().checked_add(reach.raw())?),
                    top,
                    Fx(p[2].raw().checked_add(reach.raw())?),
                ],
            ))
        };
        let Some((lo, hi)) = bounds() else {
            return self.scalar_hit(p, low, high, skip, moving_only);
        };
        let contact = Contact {
            p,
            low,
            high,
            lo,
            hi,
            reach,
            bottom,
            top,
            skip,
        };
        let mut best = None;
        self.find(0, &contact, moving_only, &mut best, examined);
        best.map(|i| &self.hulls[i])
    }
    fn scalar_hit(
        &self,
        p: [Fx; 3],
        low: [Fx; 3],
        high: [Fx; 3],
        skip: u64,
        moving_only: bool,
    ) -> Option<&'h Hull<'s>> {
        if !moving_only {
            return hull_hit(p, low, high, self.hulls, u128::from(skip));
        }
        self.hulls.iter().enumerate().find_map(|(i, h)| {
            if (i < 64 && skip & (1 << i) != 0) || h.vel == [Fx::ZERO; 3] {
                return None;
            }
            let reach = low[0].abs().max(high[0]).max(low[2].abs()).max(high[2]);
            let (bottom, top) = (p[1].add(low[1]), p[1].add(high[1]));
            (hull_box_overlap(p, reach, bottom, top, h) && h.meets_piece(p, low, high)).then_some(h)
        })
    }
    fn find(
        &self,
        index: usize,
        q: &Contact,
        moving_only: bool,
        best: &mut Option<usize>,
        examined: &mut impl FnMut(usize),
    ) {
        let node = &self.nodes[index];
        if (moving_only && !node.moving)
            || best.is_some_and(|i| node.first >= i)
            || (0..3).any(|k| node.hi[k] < q.lo[k] || node.lo[k] > q.hi[k])
        {
            return;
        }
        if let Some([left, right]) = node.children {
            self.find(left, q, moving_only, best, examined);
            self.find(right, q, moving_only, best, examined);
            return;
        }
        for &i in &self.order[node.start..node.end] {
            if best.is_some_and(|old| i >= old) {
                break;
            }
            examined(i);
            let h = &self.hulls[i];
            if (moving_only && h.vel == [Fx::ZERO; 3])
                || (i < 64 && q.skip & (1 << i) != 0)
                || !hull_box_overlap(q.p, q.reach, q.bottom, q.top, h)
            {
                continue;
            }
            if h.meets_piece(q.p, q.low, q.high) {
                *best = Some(i);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::burst::Solid;

    fn hull(i: usize) -> Hull<'static> {
        Hull {
            pos: [
                Fx::from_int((i % 12) as i32 * 16),
                Fx::from_ratio((i % 3) as i32, 2),
                Fx::from_int((i / 12) as i32 * 16),
            ],
            head: i as i32 * 911,
            half: [Fx::from_int(12), Fx::from_int(8)],
            height: Fx::from_int(12),
            vel: [Fx::from_int(i as i32), Fx::ZERO, Fx::ZERO],
            step: Fx::ONE,
            solid: None,
        }
    }
    fn identity(hit: Option<&Hull<'_>>) -> Option<Fx> {
        hit.map(|h| h.vel[0])
    }
    #[test]
    fn indexed_contacts_match_source_order_with_voxel_holes_turns_and_skip_masks() {
        let holes = |p: [i32; 3]| (p[0] + p[1] + p[2]).rem_euclid(3) == 0;
        let empty = |_: [i32; 3]| false;
        for count in [0, 1, 8, 15, 16, 17, 63, 64, 65, 96] {
            for geometry in [0, 1, 2] {
                let hulls: Vec<_> = (0..count)
                    .map(|i| {
                        // Source IDs deliberately interleave spatial positions.
                        let mut h = hull((i * 37) % count);
                        h.solid = match geometry {
                            1 => Some(Solid(&holes)),
                            2 => Some(Solid(&empty)),
                            _ => None,
                        };
                        h
                    })
                    .collect();
                let index = HullQueries::new(&hulls);
                assert_eq!(index.nodes.is_empty(), count < MIN_HULLS);
                for i in 0..384 {
                    let p = [
                        Fx::from_ratio(i % 180 * 3 - 12, 3),
                        Fx::from_ratio(i % 24 - 4, 2),
                        Fx::from_ratio(i % 100 * 2 - 5, 2),
                    ];
                    for size in [1, 3, 12] {
                        let low = [Fx::from_ratio(-size, 2); 3];
                        let high = [Fx::from_ratio(size, 2); 3];
                        for skip in [0, 1, u64::MAX, 0xa518_3ec2_d947_6b0f] {
                            assert_eq!(identity(index.hit(p, low, high, skip)), identity(hull_hit(p, low, high, &hulls, u128::from(skip))), "count={count}, geometry={geometry}, query={i}, size={size}, skip={skip}");
                        }
                    }
                }
                // Overlapping identical bounds must still select the first
                // source whose exact voxels answer, rather than a tree leaf.
                if count >= MIN_HULLS {
                    let mut overlap = hulls;
                    for h in &mut overlap {
                        h.pos = [Fx::ZERO; 3];
                    }
                    let query = HullQueries::new(&overlap);
                    let p = [Fx::ZERO, Fx::ONE, Fx::ZERO];
                    for skip in [0, 1, 3, u64::MAX] {
                        assert_eq!(
                            identity(query.hit(p, [Fx::ZERO; 3], [Fx::ONE; 3], skip)),
                            identity(hull_hit(p, [Fx::ZERO; 3], [Fx::ONE; 3], &overlap, u128::from(skip)))
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn hull_index_falls_back_when_fixed_point_intervals_or_differences_can_wrap() {
        let mut hulls: Vec<_> = (0..32).map(hull).collect();
        hulls[0].pos[0] = Fx(i64::MAX - 1);
        let invalid = HullQueries::new(&hulls);
        assert!(invalid.nodes.is_empty());
        let low = [Fx::from_int(-2); 3];
        let high = [Fx::from_int(2); 3];
        for raw in [i64::MIN, i64::MIN + 9, 0, i64::MAX - 9, i64::MAX] {
            let p = [Fx(raw), Fx::ONE, Fx::ZERO];
            assert_eq!(
                identity(invalid.hit(p, low, high, 0)),
                identity(hull_hit(p, low, high, &hulls, 0))
            );
        }
        hulls[0].pos[0] = Fx::ZERO;
        let valid = HullQueries::new(&hulls);
        assert!(!valid.nodes.is_empty());
        for raw in [i64::MIN, i64::MIN + 1, i64::MAX] {
            let p = [Fx(raw), Fx::ONE, Fx::ZERO];
            assert_eq!(
                identity(valid.hit(p, low, high, 0)),
                identity(hull_hit(p, low, high, &hulls, 0))
            );
        }
    }
    #[test]
    fn moving_contacts_match_filtered_scalar_reference() {
        let holes = |p: [i32; 3]| (p[0] + p[1] + p[2]).rem_euclid(3) == 0;
        let empty = |_: [i32; 3]| false;
        for count in [0, 1, 8, 15, 16, 17, 63, 64, 65, 96] {
            for geometry in [0, 1, 2] {
                let hulls: Vec<_> = (0..count)
                    .map(|i| {
                        let mut h = hull((i * 37) % count);
                        h.vel = if i % 2 == 0 { [Fx::ZERO; 3] } else { h.vel };
                        h.solid = match geometry {
                            1 => Some(Solid(&holes)),
                            2 => Some(Solid(&empty)),
                            _ => None,
                        };
                        h
                    })
                    .collect();
                let index = HullQueries::new(&hulls);
                for i in 0..128 {
                    let p = [
                        Fx::from_ratio(i % 180 * 3 - 12, 3),
                        Fx::from_ratio(i % 24 - 4, 2),
                        Fx::from_ratio(i % 100 * 2 - 5, 2),
                    ];
                    for size in [1, 3, 12] {
                        let low = [Fx::from_ratio(-size, 2); 3];
                        let high = [Fx::from_ratio(size, 2); 3];
                        for skip in [0, 1, u64::MAX, 0xa518_3ec2_d947_6b0f] {
                            let expected = hulls.iter().enumerate().find_map(|(i, h)| {
                                if (i < 64 && skip & (1 << i) != 0) || h.vel == [Fx::ZERO; 3] {
                                    return None;
                                }
                                let reach =
                                    low[0].abs().max(high[0]).max(low[2].abs()).max(high[2]);
                                let (bottom, top) = (p[1].add(low[1]), p[1].add(high[1]));
                                (hull_box_overlap(p, reach, bottom, top, h)
                                    && h.meets_piece(p, low, high))
                                .then_some(h)
                            });
                            assert_eq!(identity(index.hit_moving(p, low, high, skip)), identity(expected), "count={count}, geometry={geometry}, query={i}, size={size}, skip={skip}");
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn local_hull_contact_rejects_distant_roster_without_testing_every_hull() {
        for count in [64, 256, 1024] {
            let hulls: Vec<_> = (0..count)
                .map(|i| {
                    let mut h = hull(i);
                    h.pos[0] = Fx::from_int((i % 32) as i32 * 128);
                    h.pos[2] = Fx::from_int((i / 32) as i32 * 128);
                    h
                })
                .collect();
            let index = HullQueries::new(&hulls);
            let h = &hulls[count - 1];
            let p = [h.pos[0], Fx::from_int(4), h.pos[2]];
            let mut examined = 0;
            let hit = index.hit_with(p, [Fx::ZERO; 3], [Fx::ONE; 3], 0, false, &mut |_| {
                examined += 1
            });
            assert_eq!(identity(hit), Some(h.vel[0]));
            assert!(examined <= LEAF, "count={count}, examined={examined}");
        }
    }
}
