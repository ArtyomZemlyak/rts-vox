//! Exact, bounded joins of touching, equally aligned resting geometry.
//! A broad-phase pile cell is never permission to move matter across that cell.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Geometry {
    pub position: V3,
    pub shape: u64,
    pub side: u8,
    mm: u16,
}
impl Geometry {
    pub fn of(f: &Fragment) -> Option<Self> {
        (f.resting
            && f.velocity == V3::ZERO
            && f.basis == IDENTITY
            && f.chunk.is_none()
            && f.count == f.shape.count_ones()
            && f.count < 64)
            .then_some(Self {
                position: f.position,
                shape: f.shape,
                side: f.side,
                mm: f.mm,
            })
    }
    fn origin(self) -> V3 {
        self.position
            .sub(shape_centre(self.shape, self.side).scale(Fx::from_ratio(
                i32::from(self.mm),
                i32::from(voxel::unit::WORLD_MM),
            )))
    }
    pub fn join(self, other: Self) -> Option<Self> {
        // Full 4³ masks are the legacy saturated-pile presentation. Do not
        // transform finite shards into that different representation on landing.
        if self.mm != other.mm || self.shape.count_ones() + other.shape.count_ones() >= 64 {
            return None;
        }
        let scale = Fx::from_ratio(i32::from(self.mm), i32::from(voxel::unit::WORLD_MM));
        let reach = scale.mul(Fx::from_int(4));
        if arr(other.position.sub(self.position))
            .iter()
            .any(|v| v.abs() > reach)
        {
            return None;
        }
        let origin = self.origin();
        let delta = arr(other.origin().sub(origin));
        let offset = delta.map(|v| v.div(scale).add(Fx::from_ratio(1, 2)).to_int_trunc());
        // Only numerical roundoff in centre/scale arithmetic is tolerated.
        // No visible grid snapping, settling, turning or rescaling is allowed.
        if (0..3).any(|a| delta[a].sub(Fx::from_int(offset[a]).mul(scale)).abs().raw() > 128) {
            return None;
        }
        let mut low = [i32::MAX; 3];
        let mut high = [i32::MIN; 3];
        for (g, offset) in [(self, [0; 3]), (other, offset)] {
            let mut mask = g.shape;
            while mask != 0 {
                let i = mask.trailing_zeros() as usize;
                mask &= mask - 1;
                let p = ijk(i, usize::from(g.side));
                for a in 0..3 {
                    low[a] = low[a].min(p[a] + offset[a]);
                    high[a] = high[a].max(p[a] + offset[a]);
                }
            }
        }
        let side = (0..3).map(|a| high[a] - low[a] + 1).max()?;
        if !(1..=4).contains(&side) {
            return None;
        }
        let rebox = |g: Self, offset: [i32; 3]| {
            let mut out = 0;
            let mut mask = g.shape;
            while mask != 0 {
                let i = mask.trailing_zeros() as usize;
                mask &= mask - 1;
                let p = ijk(i, usize::from(g.side));
                let p: [i32; 3] = std::array::from_fn(|a| p[a] + offset[a] - low[a]);
                out |= 1u64 << (p[0] + side * p[1] + side * side * p[2]);
            }
            out
        };
        let a = rebox(self, [0; 3]);
        let b = rebox(other, offset);
        if a & b != 0 || shape_grow(a, side as u8) & b == 0 {
            return None;
        }
        let shape = a | b;
        let position = origin.add(
            v3(low.map(Fx::from_int))
                .add(shape_centre(shape, side as u8))
                .scale(scale),
        );
        Some(Self {
            position,
            shape,
            side: side as u8,
            mm: self.mm,
        })
    }
    pub fn apply(self, f: &mut Fragment) {
        f.position = self.position;
        f.shape = self.shape;
        f.side = self.side;
        // Basis and scale remain exactly those of the original geometry.
    }
}
/// Candidate search is capped even for dense imported snapshots. Missing a
/// possible optimization is preferable to a quadratic merge or a changed shape.
pub(super) const HEADS: usize = 16;

#[cfg(test)]
mod tests {
    use super::*;
    fn at(mm: u16, p: [i32; 3]) -> Geometry {
        Geometry {
            position: v3(p.map(Fx::from_int)).scale(Fx::from_ratio(i32::from(mm), 100)),
            shape: 1,
            side: 1,
            mm,
        }
    }
    fn cells(g: Geometry) -> Vec<[i64; 3]> {
        let mut out = Vec::new();
        let mut mask = g.shape;
        let scale = Fx::from_ratio(i32::from(g.mm), 100);
        while mask != 0 {
            let i = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            out.push(
                arr(g
                    .origin()
                    .add(v3(ijk(i, usize::from(g.side)).map(Fx::from_int)).scale(scale)))
                .map(Fx::raw),
            );
        }
        out.sort();
        out
    }
    #[test]
    fn fragment_join_preserves_world_cells_at_every_material_scale_and_centroid() {
        for mm in [50, 100, 150, 200, 1000] {
            let parts = [
                at(mm, [10, 20, 30]),
                at(mm, [11, 20, 30]),
                at(mm, [10, 20, 31]),
            ];
            let mut g = parts[0];
            let mut expected = cells(g);
            for donor in &parts[1..] {
                expected.extend(cells(*donor));
                expected.sort();
                g = g.join(*donor).expect("face-connected cells should join");
                for (actual, expected) in cells(g).iter().zip(&expected) {
                    assert!((0..3).all(|a| (actual[a] - expected[a]).abs() <= 128));
                }
                assert_eq!(cells(g).len(), expected.len());
            }
        }
    }
    #[test]
    fn fragment_join_rejects_far_diagonal_overlapping_fractional_and_saturated_geometry() {
        let a = at(100, [10, 20, 30]);
        for b in [
            at(100, [20, 20, 30]),
            at(100, [11, 21, 30]),
            a,
            at(150, [11, 20, 30]),
        ] {
            assert!(a.join(b).is_none());
        }
        let mut b = at(100, [11, 20, 30]);
        b.position.x = b.position.x.add(Fx::from_ratio(1, 10));
        assert!(a.join(b).is_none());
        let g = Geometry {
            position: V3::ZERO,
            shape: u64::MAX >> 1,
            side: 4,
            mm: 100,
        };
        let last = Geometry {
            position: g.origin().add(v3([Fx::from_int(3); 3])),
            shape: 1,
            side: 1,
            mm: 100,
        };
        assert!(
            g.join(last).is_none(),
            "must not switch to legacy full-pile presentation"
        );
    }
    #[test]
    fn fragment_join_keeps_rotated_and_legacy_inventory_pieces_out_of_compaction() {
        let mut f = Fragment {
            id: 1,
            position: V3::ZERO,
            velocity: V3::ZERO,
            material: 2,
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
        assert!(Geometry::of(&f).is_some());
        f.basis.swap(0, 2);
        assert!(Geometry::of(&f).is_none());
        f.basis = IDENTITY;
        f.count = 4096;
        assert!(Geometry::of(&f).is_none());
        f.count = 1;
        f.resting = false;
        assert!(Geometry::of(&f).is_none());
    }
}
