//! Pristine range placement, independent of the dense world's allocation.
//! The editor and scene constructor use the same occupied cells, including
//! articulated segments and the firing tank. Templates are cached by the caller.
use super::{RangeTerrain, RangeUnit, GUN};
use modding::range::{RangeLayout, RangeUnitKind};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejection {
    Layout(String),
    Outside,
    Terrain,
    Units,
}
impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Layout(s) => f.write_str(s),
            Self::Outside => f.write_str("a range unit extends outside the world"),
            Self::Terrain => f.write_str("a range unit intersects terrain or a block"),
            Self::Units => f.write_str("range units intersect each other or the firing tank"),
        }
    }
}

struct Footprint {
    columns: BTreeMap<[i32; 2], Vec<i32>>,
    min: [i32; 3],
    max: [i32; 3],
}
impl Footprint {
    fn of(kind: RangeUnitKind) -> Self {
        let unit = RangeUnit::of_kind(kind, GUN);
        let (lo, hi) = unit.bounds();
        let mut columns = BTreeMap::new();
        let mut min = [i32::MAX; 3];
        let mut max = [i32::MIN; 3];
        // The same world-cell centres used by damage/hit queries. Mapping
        // only segment voxel centres can leave holes in a rotated segment.
        for z in lo[2]..hi[2] {
            for x in lo[0]..hi[0] {
                let ys: Vec<_> = (lo[1]..hi[1])
                    .filter(|y| unit.solid([x, *y, z]))
                    .map(|y| y - GUN[1])
                    .collect();
                if let (Some(&bottom), Some(&top)) = (ys.first(), ys.last()) {
                    let p = [x - GUN[0], bottom, z - GUN[2]];
                    let q = [p[0] + 1, top + 1, p[2] + 1];
                    for a in 0..3 {
                        min[a] = min[a].min(p[a]);
                        max[a] = max[a].max(q[a]);
                    }
                    columns.insert([p[0], p[2]], ys);
                }
            }
        }
        Self { columns, min, max }
    }
    fn overlaps(&self, at: [i32; 3], other: &Self, there: [i32; 3]) -> bool {
        if (0..3).any(|a| {
            at[a] + self.min[a] >= there[a] + other.max[a]
                || there[a] + other.min[a] >= at[a] + self.max[a]
        }) {
            return false;
        }
        self.columns.iter().any(|(p, ys)| {
            let q = [p[0] + at[0] - there[0], p[1] + at[2] - there[2]];
            other.columns.get(&q).is_some_and(|zs| {
                ys.iter()
                    .any(|y| zs.binary_search(&(y + at[1] - there[1])).is_ok())
            })
        })
    }
}

pub struct Validator {
    footprints: [Footprint; 2],
}
impl Default for Validator {
    fn default() -> Self {
        Self {
            footprints: [
                Footprint::of(RangeUnitKind::Tank),
                Footprint::of(RangeUnitKind::Strider),
            ],
        }
    }
}
impl Validator {
    /// Work is bounded by nine template footprints, not the map's area.
    /// Layered authored blocks retain their last-writer material semantics.
    pub fn validate(&self, layout: &RangeLayout) -> Result<(), Rejection> {
        layout.validate().map_err(Rejection::Layout)?;
        let terrain = RangeTerrain::new(true, Some(layout.clone()));
        let mut units = vec![(&self.footprints[0], layout.gun)];
        units.extend(layout.units.iter().map(|u| {
            (
                &self.footprints[usize::from(u.kind == RangeUnitKind::Strider)],
                u.at,
            )
        }));
        for (i, &(shape, at)) in units.iter().enumerate() {
            if (0..3)
                .any(|a| at[a] + shape.min[a] < 0 || at[a] + shape.max[a] > layout.size[a] as i32)
            {
                return Err(Rejection::Outside);
            }
            if units[..i]
                .iter()
                .any(|&(other, there)| shape.overlaps(at, other, there))
            {
                return Err(Rejection::Units);
            }
            for (p, ys) in &shape.columns {
                let (x, z) = ((at[0] + p[0]) as u32, (at[2] + p[1]) as u32);
                let column = terrain.column_of(x, z);
                if ys
                    .iter()
                    .any(|y| terrain.at(&column, x, (at[1] + y) as u32, z) != 0)
                {
                    return Err(Rejection::Terrain);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use modding::range::{RangeBlock, RangeUnitDef};

    fn layout() -> RangeLayout {
        RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap()
    }
    #[test]
    fn stock_and_kilometre_layouts_share_the_bounded_check() {
        let v = Validator::default();
        v.validate(&layout()).unwrap();
        let large = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        v.validate(&large).unwrap();
    }
    #[test]
    fn the_shooter_is_occupied_and_a_block_cannot_enter_its_segments() {
        let v = Validator::default();
        let mut r = layout();
        r.units.push(RangeUnitDef {
            kind: RangeUnitKind::Tank,
            at: GUN,
        });
        assert_eq!(v.validate(&r), Err(Rejection::Units));
        r.units.pop();
        let gun = RangeUnit::new(GUN);
        let p = v.footprints[0]
            .columns
            .iter()
            .find_map(|(p, ys)| {
                ys.iter().find_map(|y| {
                    let q = [p[0] + GUN[0], y + GUN[1], p[1] + GUN[2]];
                    gun.hit(q).is_some_and(|(part, _)| part > 0).then_some(q)
                })
            })
            .expect("a segment-only cell");
        r.blocks.push(RangeBlock {
            min: p,
            max: p.map(|v| v + 1),
            material: 3,
        });
        assert_eq!(v.validate(&r), Err(Rejection::Terrain));
    }
    #[test]
    fn shared_bounding_boxes_are_allowed_when_occupied_cells_do_not_overlap() {
        let v = Validator::default();
        let f = &v.footprints[1];
        // Four narrow feet leave room underneath the raised walker hull.
        assert!(!f.columns.is_empty());
        let mut r = layout();
        let walker = &r.units[3];
        let p = [walker.at[0], walker.at[1], walker.at[2]];
        assert!(!f.columns.get(&[0, 0]).is_some_and(|ys| ys.contains(&0)));
        r.blocks.push(RangeBlock {
            min: p,
            max: p.map(|v| v + 1),
            material: 3,
        });
        v.validate(&r).unwrap();
    }
}
