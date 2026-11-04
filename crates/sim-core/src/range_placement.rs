//! Pristine range placement, independent of the dense world's allocation.
//! The editor, scene constructor and production use the same occupied cells,
//! including articulated segments. Templates are cached by the caller.
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
    fn of(kind: RangeUnitKind, head: i32) -> Self {
        let mut unit = RangeUnit::of_kind(kind, GUN);
        unit.motion.drive.head = head;
        unit.update_pose(head);
        let (lo, hi) = unit.bounds();
        let mut columns = BTreeMap::new();
        let mut min = [i32::MAX; 3];
        let mut max = [i32::MIN; 3];
        if kind == RangeUnitKind::GrebenStorage {
            for x in 0..usize::from(unit.body.vol.nx) {
                for y in 0..usize::from(unit.body.vol.ny) {
                    for z in 0..usize::from(unit.body.vol.nz) {
                        if !unit.body.vol.get(x, y, z) {
                            continue;
                        }
                        let p = unit.world_pos([x, y, z]);
                        let p = [p[0] - GUN[0], p[1] - GUN[1], p[2] - GUN[2]];
                        for a in 0..3 {
                            min[a] = min[a].min(p[a]);
                            max[a] = max[a].max(p[a] + 1);
                        }
                        columns
                            .entry([p[0], p[2]])
                            .or_insert_with(Vec::new)
                            .push(p[1]);
                    }
                }
            }
            return Self { columns, min, max };
        }
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
    footprints: [std::sync::OnceLock<Footprint>; RangeUnitKind::COUNT * 4],
}
impl Default for Validator {
    fn default() -> Self {
        Self {
            footprints: std::array::from_fn(|_| std::sync::OnceLock::new()),
        }
    }
}
impl Validator {
    fn footprint(&self, kind: RangeUnitKind, reverse: bool) -> &Footprint {
        self.footprint_yaw(kind, u8::from(reverse) * 2)
    }
    fn footprint_yaw(&self, kind: RangeUnitKind, yaw: u8) -> &Footprint {
        self.footprints[kind.index() + usize::from(yaw) * RangeUnitKind::COUNT]
            .get_or_init(|| Footprint::of(kind, i32::from(yaw) * fx::angle::FULL / 4))
    }

    /// Prepare both spawn orientations outside the simulation tick. Only
    /// kinds named by a producer need these derived, unsaved masks.
    pub(super) fn prepare_product(&self, kind: RangeUnitKind) {
        for reverse in [false, true] {
            self.footprint(kind, reverse);
        }
    }

    /// Exact body-cell collision against the current world. The producer's
    /// probe still supplies the original box/grid; clipping preserves its
    /// bounds even where truncation at zero differs from an integer shift.
    /// No pose lookup or lazy mask construction happens during the tick.
    pub(super) fn product_intersects(
        &self,
        kind: RangeUnitKind,
        reverse: bool,
        world: &voxel::World,
        at: [i32; 3],
        bounds: ([i32; 3], [i32; 3]),
    ) -> bool {
        let shape = self.footprints[kind.index() + usize::from(reverse) * 2 * RangeUnitKind::COUNT]
            .get()
            .expect("product geometry is prepared at scene construction");
        let (lo, hi) = bounds;
        shape.columns.iter().any(|(p, ys)| {
            let (x, z) = (at[0] + p[0], at[2] + p[1]);
            x >= lo[0]
                && x < hi[0]
                && z >= lo[2]
                && z < hi[2]
                && ys.iter().any(|y| {
                    let y = at[1] + y;
                    y >= lo[1] && y < hi[1] && world.solid(x as u32, y as u32, z as u32)
                })
        })
    }

    /// Collision checks cover one footprint per kind and side; the terrain
    /// index itself scales with the authored map. Layered blocks retain
    /// their last-writer material semantics.
    pub fn validate(&self, layout: &RangeLayout) -> Result<(), Rejection> {
        self.prepare(layout).map(|_| ())
    }
    /// Keep the validated terrain index for a scene rebuild instead of
    /// constructing the same spatial bins again after placement checks.
    pub(super) fn prepare(&self, layout: &RangeLayout) -> Result<RangeTerrain, Rejection> {
        layout.validate().map_err(Rejection::Layout)?;
        let terrain = RangeTerrain::new(true, Some(layout.clone()));
        let mut units = vec![(self.footprint(layout.gun_kind, false), layout.gun)];
        if layout.battle || layout.gun_point {
            units.clear();
        }
        units.extend(
            layout
                .units
                .iter()
                .enumerate()
                .map(|(i, u)| (self.footprint_yaw(u.kind, layout.unit_yaw(i)), u.at)),
        );
        for (i, &(shape, at)) in units.iter().enumerate() {
            if (0..3)
                .any(|a| at[a] + shape.min[a] < 0 || at[a] + shape.max[a] > layout.size[a] as i32)
            {
                return Err(Rejection::Outside);
            }
            if layout.allow_overlap {
                continue;
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
                if let Some(_y) = ys
                    .iter()
                    .find(|y| terrain.at(&column, x, (at[1] + **y) as u32, z) != 0)
                {
                    #[cfg(test)]
                    eprintln!(
                        "placement collision {:?} at {:?}: cell [{x}, {}, {z}], material {}",
                        layout.units[i].kind,
                        at,
                        at[1] + _y,
                        terrain.at(&column, x, (at[1] + _y) as u32, z)
                    );
                    return Err(Rejection::Terrain);
                }
            }
        }
        for piece in &layout.initial_debris {
            for z in piece.min[2]..piece.max[2] {
                for x in piece.min[0]..piece.max[0] {
                    let column = terrain.column_of(x as u32, z as u32);
                    if terrain.at(&column, x as u32, (piece.min[1] - 1) as u32, z as u32) == 0 {
                        #[cfg(test)]
                        eprintln!("debris has no support at [{x}, {}, {z}]", piece.min[1] - 1);
                        return Err(Rejection::Terrain);
                    }
                    for y in piece.min[1]..piece.max[1] {
                        if terrain.at(&column, x as u32, y as u32, z as u32) != 0 {
                            #[cfg(test)]
                            eprintln!(
                                "debris intersects at [{x}, {y}, {z}]: {}",
                                terrain.at(&column, x as u32, y as u32, z as u32)
                            );
                            return Err(Rejection::Terrain);
                        }
                        if units.iter().any(|(shape, at)| {
                            shape
                                .columns
                                .get(&[x - at[0], z - at[2]])
                                .is_some_and(|ys| ys.contains(&(y - at[1])))
                        }) {
                            return Err(Rejection::Units);
                        }
                    }
                }
            }
        }
        Ok(terrain)
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
            weapon: None,
            allied: false,
            kind: RangeUnitKind::Tank,
            at: GUN,
        });
        assert_eq!(v.validate(&r), Err(Rejection::Units));
        r.units.pop();
        let gun = RangeUnit::new(GUN);
        let p = v
            .footprint(RangeUnitKind::Tank, false)
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
        let f = v.footprint(RangeUnitKind::Strider, false);
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

    #[test]
    fn canister_artillery_has_separate_cached_footprints_for_both_sides() {
        let v = Validator::default();
        for reverse in [false, true] {
            let t2 = v.footprint(RangeUnitKind::ArtilleryT2, reverse);
            let tank = v.footprint(RangeUnitKind::Tank, !reverse);
            assert!(!t2.columns.is_empty());
            assert!(
                !std::ptr::eq(t2, tank),
                "new kind aliased an old orientation"
            );
            let fresh = Footprint::of(
                RangeUnitKind::ArtilleryT2,
                if reverse { fx::angle::HALF } else { 0 },
            );
            assert_eq!(t2.columns, fresh.columns);
        }
    }

    #[test]
    fn product_masks_match_fresh_bodies_in_both_orientations_and_across_zero() {
        use std::collections::BTreeSet;
        let v = Validator::default();
        for kind in RangeUnitKind::ALL {
            v.prepare_product(kind);
            for reverse in [false, true] {
                let shape = v.footprint(kind, reverse);
                for at in [[95, 37, 127], [0; 3]] {
                    let unit = RangeUnit::placed(kind, !reverse, at, true);
                    let (lo, hi) = unit.bounds();
                    let mut expected = BTreeSet::new();
                    for z in lo[2]..hi[2] {
                        for y in lo[1]..hi[1] {
                            for x in lo[0]..hi[0] {
                                if unit.solid([x, y, z]) {
                                    expected.insert([x, y, z]);
                                }
                            }
                        }
                    }
                    let actual: BTreeSet<_> = shape
                        .columns
                        .iter()
                        .flat_map(|(p, ys)| {
                            ys.iter()
                                .map(move |y| [at[0] + p[0], at[1] + y, at[2] + p[1]])
                        })
                        .filter(|p| (0..3).all(|a| p[a] >= lo[a] && p[a] < hi[a]))
                        .collect();
                    assert!(!expected.is_empty());
                    assert!(
                        actual == expected,
                        "{kind:?} reverse={reverse} at={at:?}: missing {:?}, extra {:?}",
                        expected.difference(&actual).take(4).collect::<Vec<_>>(),
                        actual.difference(&expected).take(4).collect::<Vec<_>>()
                    );
                }
            }
        }
    }
}
