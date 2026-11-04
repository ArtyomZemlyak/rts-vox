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
    cell: i32,
    columns: BTreeMap<[i32; 2], Vec<i32>>,
    min: [i32; 3],
    max: [i32; 3],
}
impl Footprint {
    fn of(kind: RangeUnitKind, head: i32) -> Self {
        let mut unit = RangeUnit::of_kind(kind, GUN);
        unit.motion.drive.head = head;
        unit.update_pose(head);
        let mut columns = BTreeMap::new();
        let mut min = [i32::MAX; 3];
        let mut max = [i32::MIN; 3];
        if matches!(
            kind,
            RangeUnitKind::GrebenStorage
                | RangeUnitKind::KilFactoryT1
                | RangeUnitKind::KilFactoryT2
                | RangeUnitKind::KilFactoryT3
                | RangeUnitKind::KilFactoryT4
        ) {
            for x in 0..usize::from(unit.body.vol.nx) {
                for y in 0..usize::from(unit.body.vol.ny) {
                    for z in 0..usize::from(unit.body.vol.nz) {
                        if !unit.body.vol.get(x, y, z) {
                            continue;
                        }
                        let p = unit.world_pos([x, y, z]);
                        let p = [p[0] - GUN[0], p[1] - GUN[1], p[2] - GUN[2]];
                        for a in 0..3 {
                            min[a] = min[a].min(p[a] - 4);
                            max[a] = max[a].max(p[a] + 4);
                        }
                        columns
                            .entry([p[0], p[2]])
                            .or_insert_with(Vec::new)
                            .push(p[1]);
                    }
                }
            }
            return Self {
                cell: 8,
                columns,
                min,
                max,
            };
        }
        // Rasterise the same cell centres as hit queries, one authored part
        // at a time. A hollow walker no longer transforms every world point
        // against every unrelated leg or arm inside the union bounding box.
        let parts = std::iter::once((&unit.body, &unit.pose)).chain(
            unit.body
                .segments
                .iter()
                .zip(&unit.seg_frames)
                .map(|(seg, frame)| (&seg.body, frame)),
        );
        for (body, frame) in parts {
            let size = [
                i32::from(body.vol.nx),
                i32::from(body.vol.ny),
                i32::from(body.vol.nz),
            ];
            let mut lo = [i32::MAX; 3];
            let mut hi = [i32::MIN; 3];
            for k in 0..8 {
                let p = frame.world(std::array::from_fn(|a| {
                    fx::Fx::from_int(if k & (1 << a) == 0 { 0 } else { size[a] })
                }));
                let p = [p.x, p.y, p.z].map(fx::Fx::to_int_trunc);
                for a in 0..3 {
                    lo[a] = lo[a].min(p[a] - 1);
                    hi[a] = hi[a].max(p[a] + 1);
                }
            }
            for z in lo[2]..hi[2] {
                for x in lo[0]..hi[0] {
                    for y in lo[1]..hi[1] {
                        let Some(c) = super::local_in(body, [x, y, z], frame) else {
                            continue;
                        };
                        if !body.vol.get(c[0], c[1], c[2]) {
                            continue;
                        }
                        let p = [x - GUN[0], y - GUN[1], z - GUN[2]];
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
        }
        for ys in columns.values_mut() {
            ys.sort_unstable();
            ys.dedup();
        }
        Self {
            cell: 1,
            columns,
            min,
            max,
        }
    }
    fn contains(&self, p: [i32; 3]) -> bool {
        let centre = |a: usize| p[a] - (p[a] - self.min[a]).rem_euclid(self.cell) + self.cell / 2;
        self.columns
            .get(&[centre(0), centre(2)])
            .is_some_and(|ys| ys.binary_search(&centre(1)).is_ok())
    }
    fn terrain_intersects(&self, at: [i32; 3], terrain: &RangeTerrain) -> bool {
        self.columns.iter().any(|(p, ys)| {
            let x0 = at[0] + p[0] - self.cell / 2;
            let z0 = at[2] + p[1] - self.cell / 2;
            (z0..z0 + self.cell).any(|z| {
                (x0..x0 + self.cell).any(|x| {
                    let column = terrain.column_of(x as u32, z as u32);
                    // Conservative empty-sky rejection avoids expanding every
                    // 80 cm cell into 512 samples on an otherwise empty pad.
                    let mut top = column.ground as i32;
                    top = top.max(column.course.unwrap_or(0));
                    top = top.max(column.blocks.iter().filter(|b| b.2 != 0).map(|b| b.1).max().unwrap_or(0));
                    top = top.max(column.tree.map_or(0, |(base, h, _, _)| (base + h) as i32));
                    if column.wall {
                        top = top.max(66);
                    }
                    if column.target {
                        top = top.max(45);
                    }
                    if column.rock.is_some() {
                        top = top.max(column.ground as i32 + 5);
                    }
                    ys.iter()
                        .take_while(|y| at[1] + **y - self.cell / 2 < top)
                        .any(|y| {
                            let y0 = at[1] + y - self.cell / 2;
                            (y0..(y0 + self.cell).min(top)).any(|y| {
                                let mat = terrain.at(&column, x as u32, y as u32, z as u32);
                                #[cfg(test)]
                                if mat != 0 { eprintln!("placement obstruction [{x}, {y}, {z}] material={mat} cell={}", self.cell); }
                                mat != 0
                            })
                        })
                })
            })
        })
    }
    fn overlaps(&self, at: [i32; 3], other: &Self, there: [i32; 3]) -> bool {
        if (0..3).any(|a| {
            at[a] + self.min[a] >= there[a] + other.max[a]
                || there[a] + other.min[a] >= at[a] + self.max[a]
        }) {
            return false;
        }
        if self.cell > other.cell {
            return other.overlaps(there, self, at);
        }
        self.columns.iter().any(|(p, ys)| {
            ys.iter().any(|y| {
                let low = [
                    p[0] - self.cell / 2,
                    y - self.cell / 2,
                    p[1] - self.cell / 2,
                ];
                // Equal-size axis-aligned cells overlap iff one of these integer
                // corners lies in the other occupied cell. Fine/coarse checks use
                // the fine cell, so a one-world-cube obstruction is never missed.
                let edges: &[i32] = if self.cell == 1 {
                    &[0]
                } else {
                    &[0, self.cell - 1]
                };
                edges.iter().any(|dx| {
                    edges.iter().any(|dy| {
                        edges.iter().any(|dz| {
                            other.contains([
                                low[0] + dx + at[0] - there[0],
                                low[1] + dy + at[1] - there[1],
                                low[2] + dz + at[2] - there[2],
                            ])
                        })
                    })
                })
            })
        })
    }
}

// Stock bodies are immutable; all editors/scenarios can share the same
// exact masks. The table is bounded by kind and the four authored yaws.
fn footprints() -> &'static [std::sync::OnceLock<Footprint>; RangeUnitKind::COUNT * 4] {
    static CACHE: [std::sync::OnceLock<Footprint>; RangeUnitKind::COUNT * 4] =
        [const { std::sync::OnceLock::new() }; RangeUnitKind::COUNT * 4];
    &CACHE
}

#[derive(Default)]
pub struct Validator;
impl Validator {
    pub fn runtime_bounds(
        &self,
        kind: RangeUnitKind,
        at: [i32; 3],
        yaw: u8,
    ) -> ([i32; 3], [i32; 3]) {
        let shape = self.footprint_yaw(kind, yaw % 4);
        (
            std::array::from_fn(|a| at[a] + shape.min[a]),
            std::array::from_fn(|a| at[a] + shape.max[a]),
        )
    }
    /// Authoring and construction share a cached shape against the live world.
    /// The caller rechecks on commit, so a preview never grants permission.
    pub fn runtime(
        &self,
        sim: &super::BlastRange,
        kind: RangeUnitKind,
        at: [i32; 3],
        yaw: u8,
    ) -> Result<(), Rejection> {
        self.runtime_except(sim, kind, at, yaw, None)
    }
    pub(super) fn runtime_except(
        &self,
        sim: &super::BlastRange,
        kind: RangeUnitKind,
        at: [i32; 3],
        yaw: u8,
        skip: Option<usize>,
    ) -> Result<(), Rejection> {
        let shape = self.footprint_yaw(kind, yaw % 4);
        let lo = std::array::from_fn::<_, 3, _>(|a| at[a] + shape.min[a]);
        let hi = std::array::from_fn::<_, 3, _>(|a| at[a] + shape.max[a]);
        if (0..3).any(|a| lo[a] < 1 || hi[a] >= sim.size()[a] as i32 - 1) {
            return Err(Rejection::Outside);
        }
        if sim
            .units
            .iter()
            .enumerate()
            .filter(|(id, _)| Some(*id) != skip)
            .map(|(_, u)| u)
            .chain((!sim.battle_mode()).then_some(&sim.gun))
            .map(|u| u.bounds())
            .chain(sim.reserved_construction_bounds())
            .any(|(a, b)| (0..3).all(|k| lo[k] < b[k] && a[k] < hi[k]))
        {
            return Err(Rejection::Units);
        }
        // An entirely clear footprint above the live ceiling needs no cell
        // scan. This is exact even after a crater or a new obstacle this tick.
        if sim
            .world
            .column_box_ceiling([lo[0] as u32, lo[2] as u32], [hi[0] as u32, hi[2] as u32])
            <= lo[1] as u32
        {
            return Ok(());
        }
        if shape.columns.iter().any(|(p, ys)| {
            let x = at[0] + p[0] - shape.cell / 2;
            let z = at[2] + p[1] - shape.cell / 2;
            ys.iter().any(|y| {
                let y = at[1] + y - shape.cell / 2;
                // A coarse model cell occupies the whole cube, not its centre.
                // Read packed occupancy words instead of 512 scalar queries.
                if shape.cell == 1 {
                    sim.world.solid(x as u32, y as u32, z as u32)
                } else {
                    sim.world.any_solid_box(
                        [x as u32, y as u32, z as u32],
                        [
                            (x + shape.cell - 1) as u32,
                            (y + shape.cell - 1) as u32,
                            (z + shape.cell - 1) as u32,
                        ],
                    )
                }
            })
        }) {
            return Err(Rejection::Terrain);
        }
        Ok(())
    }
    fn footprint(&self, kind: RangeUnitKind, reverse: bool) -> &Footprint {
        self.footprint_yaw(kind, u8::from(reverse) * 2)
    }
    fn footprint_yaw(&self, kind: RangeUnitKind, yaw: u8) -> &Footprint {
        footprints()[kind.index() + usize::from(yaw) * RangeUnitKind::COUNT]
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
        let shape = footprints()[kind.index() + usize::from(reverse) * 2 * RangeUnitKind::COUNT]
            .get()
            .expect("product geometry is prepared at scene construction");
        let (lo, hi) = bounds;
        shape.columns.iter().any(|(p, ys)| {
            let x0 = at[0] + p[0] - shape.cell / 2;
            let z0 = at[2] + p[1] - shape.cell / 2;
            (z0.max(lo[2])..(z0 + shape.cell).min(hi[2])).any(|z| {
                (x0.max(lo[0])..(x0 + shape.cell).min(hi[0])).any(|x| {
                    ys.iter().any(|y| {
                        let y0 = at[1] + y - shape.cell / 2;
                        (y0.max(lo[1])..(y0 + shape.cell).min(hi[1]))
                            .any(|y| world.solid(x as u32, y as u32, z as u32))
                    })
                })
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
            if shape.terrain_intersects(at, &terrain) {
                return Err(Rejection::Terrain);
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
                        if units
                            .iter()
                            .any(|(shape, at)| shape.contains([x - at[0], y - at[1], z - at[2]]))
                        {
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

    #[test]
    fn live_placement_checks_coarse_cell_edges_and_same_tick_edits() {
        let mut l = layout();
        l.battle = true;
        l.units.clear();
        l.shields.clear();
        l.course = None;
        l.economy = None;
        l.size = [2048, 1024, 2048];
        l.clear_min = [1, 1];
        l.clear_max = [2047, 2047];
        l.blocks = vec![
            RangeBlock {
                min: [0; 3],
                max: [2048, 1024, 2048],
                material: 0,
            },
            RangeBlock {
                min: [0; 3],
                max: [2048, 16, 2048],
                material: 1,
            },
        ];
        let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../data/explosion.ron"
        ))
        .unwrap();
        let mut sim = super::super::BlastRange::with_layout(config, l).unwrap();
        let v = Validator::default();
        let at = [1000, 16, 1000];
        for kind in [
            RangeUnitKind::Tank,
            RangeUnitKind::GrebenStorage,
            RangeUnitKind::KilFactoryT1,
            RangeUnitKind::KilFactoryT4,
        ] {
            for yaw in 0..4 {
                let shape = v.footprint_yaw(kind, yaw);
                assert!(v.runtime(&sim, kind, at, yaw).is_ok(), "{kind:?}/{yaw}");
                // An overhang above the model defeats the ceiling shortcut,
                // but must not become a false collision with the body.
                let above = [at[0], at[1] + shape.max[1] + 8, at[2]];
                let write = |sim: &mut super::super::BlastRange, p: [i32; 3], mat| {
                    sim.world.queue_edit(voxel::Edit {
                        x: p[0] as u32,
                        y: p[1] as u32,
                        z: p[2] as u32,
                        mat,
                        by: voxel::NOBODY,
                    });
                    sim.world.flush();
                };
                write(&mut sim, above, 3);
                assert!(v.runtime(&sim, kind, at, yaw).is_ok());
                let (xz, ys) = shape.columns.iter().next().unwrap();
                let corner = [
                    at[0] + xz[0] - shape.cell / 2,
                    at[1] + ys[0] - shape.cell / 2,
                    at[2] + xz[1] - shape.cell / 2,
                ];
                write(&mut sim, corner, 3);
                assert_eq!(
                    v.runtime(&sim, kind, at, yaw),
                    Err(Rejection::Terrain),
                    "{kind:?}/{yaw}"
                );
                write(&mut sim, corner, 0);
                assert!(
                    v.runtime(&sim, kind, at, yaw).is_ok(),
                    "same-tick removal {kind:?}/{yaw}"
                );
                write(&mut sim, above, 0);
            }
        }
    }

    #[test]
    fn large_storage_cells_have_no_placement_holes_between_their_centres() {
        for kind in [
            RangeUnitKind::GrebenStorage,
            RangeUnitKind::KilFactoryT1,
            RangeUnitKind::KilFactoryT2,
            RangeUnitKind::KilFactoryT3,
            RangeUnitKind::KilFactoryT4,
        ] {
            for yaw in 0..4 {
                let f = Footprint::of(kind, yaw * fx::angle::QUARTER);
                let (p, ys) = f.columns.iter().next().unwrap();
                let centre = [p[0], ys[0], p[1]];
                let mut unit = RangeUnit::of_kind(kind, GUN);
                unit.motion.drive.head = yaw * fx::angle::QUARTER;
                unit.update_pose(unit.motion.drive.head);
                for offset in [[-4, -4, -4], [3, 3, 3], [1, 0, 0]] {
                    let q = std::array::from_fn(|a| centre[a] + offset[a]);
                    assert!(unit.solid(std::array::from_fn(|a| q[a] + GUN[a])));
                    assert!(f.contains(q), "yaw={yaw} cell={q:?}");
                    let point = Footprint {
                        cell: 1,
                        columns: BTreeMap::from([([q[0], q[2]], vec![q[1]])]),
                        min: q,
                        max: q.map(|v| v + 1),
                    };
                    assert!(f.overlaps([0; 3], &point, [0; 3]));
                    assert!(point.overlaps([0; 3], &f, [0; 3]));
                    let at = [1600, 16, 1600];
                    let point = std::array::from_fn(|a| at[a] + q[a]);
                    let mut l = layout();
                    l.size = [4096, 2048, 4096];
                    l.course = None;
                    l.units.clear();
                    l.blocks = vec![
                        RangeBlock {
                            min: [0; 3],
                            max: [4096, 2048, 4096],
                            material: 0,
                        },
                        RangeBlock {
                            min: point,
                            max: point.map(|v| v + 1),
                            material: 3,
                        },
                    ];
                    assert!(f.terrain_intersects(at, &RangeTerrain::new(true, Some(l))));
                }
            }
        }
    }

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
        let v = Validator::default();
        for kind in RangeUnitKind::ALL {
            v.prepare_product(kind);
            for reverse in [false, true] {
                let shape = v.footprint(kind, reverse);
                for at in [[95, 37, 127], [0; 3]] {
                    let unit = RangeUnit::placed(kind, !reverse, at, true);
                    let (lo, hi) = unit.bounds();
                    let stride = shape.cell as usize;
                    let edges: &[i32] = if shape.cell == 1 {
                        &[0]
                    } else {
                        &[0, shape.cell - 1]
                    };
                    let mut occupied = false;
                    for z in (lo[2]..hi[2]).step_by(stride) {
                        for y in (lo[1]..hi[1]).step_by(stride) {
                            for x in (lo[0]..hi[0]).step_by(stride) {
                                for &dx in edges {
                                    for &dy in edges {
                                        for &dz in edges {
                                            let p = [x + dx, y + dy, z + dz];
                                            if (0..3).any(|a| p[a] >= hi[a]) {
                                                continue;
                                            }
                                            let expected = unit.solid(p);
                                            let actual = shape
                                                .contains(std::array::from_fn(|a| p[a] - at[a]));
                                            assert_eq!(
                                                actual, expected,
                                                "{kind:?} reverse={reverse} at={at:?} cell={p:?}"
                                            );
                                            occupied |= expected;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    assert!(occupied);
                }
            }
        }
    }
}
