//! Bounded gravity failure for authored elevated plates. Connectivity alone
//! cannot tell a three-legged roof from a four-legged one: the plate can still
//! reach ground while a corner is carrying more bending load than it can bear.
use modding::range::{RangeBlock, RangeLayout};
use std::collections::{BTreeMap, BTreeSet};
use voxel::{
    bodies::Bodies,
    store::{Mat, AIR},
    world::Edit,
    World,
};

struct Support<'a> {
    block: &'a RangeBlock,
    centre: [i32; 2],
    area: u32,
}

fn support<'a>(w: &World, b: &'a RangeBlock, roof: &RangeBlock) -> Support<'a> {
    let x0 = b.min[0].max(roof.min[0]).max(0) as u32;
    let x1 = b.max[0].min(roof.max[0]).max(0) as u32;
    let z0 = b.min[2].max(roof.min[2]).max(0) as u32;
    let z1 = b.max[2].min(roof.max[2]).max(0) as u32;
    let mut area = 0;
    // Follow each column to the world foundation, including material changes.
    // A steel cap overlapping the deck is not an independent support when the
    // concrete pier below it has been severed. Stop at the first gap so empty
    // space below broad caps is cheap to reject.
    for z in z0..z1 {
        for x in x0..x1 {
            if (0..roof.min[1] as u32).rev().all(|y| w.solid(x, y, z)) {
                area += 1;
            }
        }
    }
    Support {
        block: b,
        centre: [(x0 as i32 + x1 as i32) / 2, (z0 as i32 + z1 as i32) / 2],
        area,
    }
}

/// A plate that tears crushes the supports too weak to carry it
/// (ADR-439). Their matter crumbles: each cell joins `crumbs`, which the
/// caller turns into pieces or sinks, and leaves the world through the
/// lane of transfers ahead of the plate's own digs (ADR-431) — or the
/// plate would bake onto the stump the moment it starts to fall. Inside
/// the slab's box the slab's material is the slab: it stays with a
/// quadrant that holds or falls with the one that tears. A cell already
/// leaving the world, as a body's source or as a crumb, is not taken
/// twice, and the world's frame is never taken (ADR-433).
fn crush<'a>(
    w: &mut World,
    roof: &RangeBlock,
    weak: impl Iterator<Item = &'a Support<'a>>,
    crumbs: &mut Vec<([u32; 3], Mat)>,
) {
    let mut leaving = BTreeSet::new();
    for e in w.transfers() {
        if e.mat == AIR {
            leaving.insert([e.x, e.y, e.z]);
        } else {
            leaving.remove(&[e.x, e.y, e.z]);
        }
    }
    // By cell, not by support: overlapping supports share cells.
    let mut cells = BTreeMap::new();
    for s in weak {
        for y in s.block.min[1].max(0)..s.block.max[1].min(roof.max[1]) {
            for z in
                s.block.min[2].max(roof.min[2] - 16).max(0)..s.block.max[2].min(roof.max[2] + 16)
            {
                for x in s.block.min[0].max(roof.min[0] - 16).max(0)
                    ..s.block.max[0].min(roof.max[0] + 16)
                {
                    let p = [x as u32, y as u32, z as u32];
                    let m = w.material(p[0], p[1], p[2]);
                    if m == s.block.material
                        && !(m == roof.material && roof.contains([x, y, z]))
                        && !w.on_frame(p[0], p[1], p[2])
                        && !leaving.contains(&p)
                    {
                        cells.insert(p, m);
                    }
                }
            }
        }
    }
    for (p, m) in cells {
        w.queue_transfer(Edit::dig(p[0], p[1], p[2]));
        crumbs.push((p, m));
    }
}

/// Fracture plate quadrants whose bending span exceeds their material limit.
/// Material strength/density and plate thickness set the allowable span;
/// support cross-section sets the compressive capacity. Only plates near this
/// tick's edits are visited, so the kilometre map has no per-frame plate pass.
/// The supports a tearing plate crushes go to `crumbs` ([`crush`]).
#[cfg(test)]
pub(super) fn fracture(
    layout: &RangeLayout,
    w: &mut World,
    bodies: &mut Bodies,
    edits: &[Edit],
    crumbs: &mut Vec<([u32; 3], Mat)>,
    material: impl Fn(u16) -> Option<(u32, u32, u32)>,
) -> usize {
    fracture_iter(layout, w, bodies, edits.iter().copied(), crumbs, material)
}

/// A block the plate check treats as a roof. Long narrow spans use the
/// same maximum cell budget as square roofs.
fn is_roof(roof: &RangeBlock) -> bool {
    let sx = roof.max[0] - roof.min[0];
    let sz = roof.max[2] - roof.min[2];
    let thick = roof.max[1] - roof.min[1];
    (48..=512).contains(&sx)
        && (48..=512).contains(&sz)
        && sx * sz <= 256 * 256
        && (2..=12).contains(&thick)
        && roof.min[1] > 8
}
/// Whether an edit makes the plate check look at this roof.
fn under(roof: &RangeBlock, e: &Edit) -> bool {
    (e.x as i32) >= roof.min[0]
        && (e.x as i32) < roof.max[0]
        && (e.z as i32) >= roof.min[2]
        && (e.z as i32) < roof.max[2]
        && (e.y as i32) <= roof.max[1]
}
/// The lowest edit in each tile of `1 << EditTiles::SHIFT` columns: a roof
/// over no tile that holds an edit at or below its top has no edit under
/// it, and the exact test is skipped for it.
struct EditTiles(BTreeMap<[i32; 2], i32>);
impl EditTiles {
    const SHIFT: i32 = 4;
    fn of(edits: impl Iterator<Item = Edit>) -> Self {
        let mut tiles = BTreeMap::new();
        for e in edits {
            let key = [e.x as i32 >> Self::SHIFT, e.z as i32 >> Self::SHIFT];
            let low = tiles.entry(key).or_insert(i32::MAX);
            *low = (*low).min(e.y as i32);
        }
        Self(tiles)
    }
    /// False only if no edit lies under the roof.
    fn may_reach(&self, roof: &RangeBlock) -> bool {
        let lo = [roof.min[0] >> Self::SHIFT, roof.min[2] >> Self::SHIFT];
        let hi = [
            (roof.max[0] - 1) >> Self::SHIFT,
            (roof.max[2] - 1) >> Self::SHIFT,
        ];
        self.0
            .range([lo[0], i32::MIN]..=[hi[0], i32::MAX])
            .any(|(k, &y)| (lo[1]..=hi[1]).contains(&k[1]) && y <= roof.max[1])
    }
}

/// Whether an edit would make the plate check look at any roof
/// (ADR-645: such an edit in a waiting region is kept for later).
pub(super) fn watches(layout: &RangeLayout, e: &Edit) -> bool {
    layout
        .blocks
        .iter()
        .any(|roof| is_roof(roof) && under(roof, e))
}

/// Cloneable traversal keeps plate checks on the same complete batch without
/// allocating a second copy of a possibly million-cell collapse backlog.
pub(super) fn fracture_iter(
    layout: &RangeLayout,
    w: &mut World,
    bodies: &mut Bodies,
    edits: impl Iterator<Item = Edit> + Clone,
    crumbs: &mut Vec<([u32; 3], Mat)>,
    material: impl Fn(u16) -> Option<(u32, u32, u32)>,
) -> usize {
    let mut torn = 0;
    if !layout.blocks.iter().any(is_roof) {
        return torn;
    }
    let near = EditTiles::of(edits.clone());
    for roof in &layout.blocks {
        let sx = roof.max[0] - roof.min[0];
        let sz = roof.max[2] - roof.min[2];
        let thick = roof.max[1] - roof.min[1];
        if !is_roof(roof) {
            continue;
        }
        let Some((strength, density, bend_span)) = material(roof.material) else {
            continue;
        };
        if strength == 0
            || density == 0
            || !near.may_reach(roof)
            || !edits.clone().any(|e| under(roof, &e))
        {
            continue;
        }
        let needed = (sx as u64 * sz as u64 * thick as u64 * density as u64)
            .div_ceil(strength as u64 * 4000) as u32;
        let span = thick
            * if bend_span == 0 {
                4 + (strength / 80).min(16) as i32
            } else {
                bend_span.min(24) as i32
            };
        let supports: Vec<_> = layout
            .blocks
            .iter()
            .filter(|b| {
                b.material != 0
                    && b.max[1] >= roof.min[1]
                    && b.min[1] < roof.min[1]
                    && b.min[0] < roof.max[0]
                    && b.max[0] > roof.min[0]
                    && b.min[2] < roof.max[2]
                    && b.max[2] > roof.min[2]
            })
            .map(|b| support(w, b, roof))
            .collect();
        if supports.is_empty() {
            continue;
        }
        let live: Vec<_> = supports.iter().filter(|s| s.area >= needed).collect();
        // A wholly unsupported roof belongs to the ordinary connectivity
        // solver, which can keep it in one piece.
        if live.is_empty() {
            continue;
        }
        let mx = roof.min[0] + sx / 2;
        let mz = roof.min[2] + sz / 2;
        let mut crushed = false;
        for q in 0..4 {
            let x0 = if q & 1 == 0 { roof.min[0] } else { mx };
            let x1 = if q & 1 == 0 { mx } else { roof.max[0] };
            let z0 = if q & 2 == 0 { roof.min[2] } else { mz };
            let z1 = if q & 2 == 0 { mz } else { roof.max[2] };
            let centre = [(x0 + x1) / 2, (z0 + z1) / 2];
            let nearest = live
                .iter()
                .min_by_key(|s| {
                    let dx = centre[0] - s.centre[0];
                    let dz = centre[1] - s.centre[1];
                    dx * dx + dz * dz
                })
                .unwrap();
            let dx = centre[0] - nearest.centre[0];
            let dz = centre[1] - nearest.centre[1];
            if dx * dx + dz * dz <= span * span {
                continue;
            }
            if bodies.list.iter().any(|b| {
                b.fallen == 0
                    && b.oy <= roof.min[1] as i64
                    && (b.ox..b.ox + i64::from(b.sx)).contains(&i64::from(centre[0]))
                    && (b.oz..b.oz + i64::from(b.sz)).contains(&i64::from(centre[1]))
                    && b.voxels.iter().any(|v| {
                        b.ox + i64::from(v.x) == i64::from(centre[0])
                            && b.oy + i64::from(v.y) == i64::from(roof.min[1])
                            && b.oz + i64::from(v.z) == i64::from(centre[1])
                    })
            }) {
                continue;
            }
            let y = roof.min[1] as u32;
            if w.material(centre[0] as u32, y, centre[1] as u32) != roof.material {
                continue;
            }
            let mut cells = Vec::with_capacity(((x1 - x0) * (z1 - z0) * thick) as usize);
            for yy in roof.min[1]..roof.max[1] {
                for zz in z0..z1 {
                    for xx in x0..x1 {
                        if w.material(xx as u32, yy as u32, zz as u32) == roof.material {
                            cells.push((xx as u32, yy as u32, zz as u32));
                        }
                    }
                }
            }
            if cells.is_empty() {
                continue;
            }
            // Low fittings resting on this plate (parapets, surface panels)
            // tear at the same seam. Connectivity alone would leave a long
            // rail cantilevered from the surviving half after its deck fell.
            // Bound both overhang and height; tall structures keep their own
            // connectivity handling rather than enlarging this plate pass.
            let foot = [
                if x0 == roof.min[0] { x0 - 16 } else { x0 },
                if x1 == roof.max[0] { x1 + 16 } else { x1 },
                if z0 == roof.min[2] { z0 - 16 } else { z0 },
                if z1 == roof.max[2] { z1 + 16 } else { z1 },
            ];
            for fitting in layout.blocks.iter().filter(|b| {
                b.material != 0
                    && b.min[1] == roof.max[1]
                    && b.max[1] - b.min[1] <= 32
                    && b.min[0] < x1
                    && b.max[0] > x0
                    && b.min[2] < z1
                    && b.max[2] > z0
            }) {
                for yy in fitting.min[1]..fitting.max[1] {
                    for zz in fitting.min[2].max(foot[2])..fitting.max[2].min(foot[3]) {
                        for xx in fitting.min[0].max(foot[0])..fitting.max[0].min(foot[1]) {
                            if w.material(xx as u32, yy as u32, zz as u32) == fitting.material {
                                cells.push((xx as u32, yy as u32, zz as u32));
                            }
                        }
                    }
                }
            }
            cells.sort_unstable();
            cells.dedup();
            // A quadrant on the world's frame is anchored (ADR-433): it
            // does not tear, and nothing is crushed for it.
            if cells.iter().any(|&(x, y, z)| w.on_frame(x, y, z)) {
                continue;
            }
            // Once a roof, and before the first quadrant's own digs.
            if !crushed && bodies.list.len() < bodies.max_bodies {
                crushed = true;
                crush(w, roof, supports.iter().filter(|s| s.area < needed), crumbs);
            }
            if !bodies.detach(w, &cells) {
                continue;
            }
            let b = bodies.list.last_mut().unwrap();
            // Rotate about the edge toward the surviving support.
            if dx.abs() >= dz.abs() {
                b.tilt(2, -dx.signum());
            } else {
                b.tilt(1, dz.signum());
            }
            torn += 1;
        }
    }
    torn
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx::Fx;
    use voxel::Generator;

    struct Canopy {
        blocks: Vec<RangeBlock>,
    }
    impl Generator for Canopy {
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            if y < 5 {
                return 1;
            }
            self.blocks
                .iter()
                .rev()
                .find(|b| b.contains([x as i32, y as i32, z as i32]))
                .map_or(0, |b| b.material)
        }
    }
    fn fixture(mat: u16) -> (RangeLayout, World, Bodies) {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.blocks = vec![
            RangeBlock {
                min: [40, 5, 35],
                max: [50, 52, 45],
                material: mat,
            },
            RangeBlock {
                min: [40, 5, 110],
                max: [50, 52, 120],
                material: mat,
            },
            RangeBlock {
                min: [140, 5, 35],
                max: [150, 52, 45],
                material: mat,
            },
            RangeBlock {
                min: [140, 5, 110],
                max: [150, 52, 120],
                material: mat,
            },
            RangeBlock {
                min: [20, 52, 20],
                max: [180, 60, 134],
                material: mat,
            },
        ];
        let world = World::new(
            224,
            80,
            160,
            100_000,
            Box::new(Canopy {
                blocks: layout.blocks.clone(),
            }),
        );
        (layout, world, Bodies::new(16, Fx::from_ratio(20, 900)))
    }
    fn cut(w: &mut World, at: [u32; 2]) {
        for z in at[1]..at[1] + 10 {
            for x in at[0]..at[0] + 10 {
                w.queue_edit(Edit::dig(x, 30, z));
            }
        }
        w.flush();
    }
    #[test]
    fn long_bridge_counts_overlapping_piers_and_not_their_floating_caps() {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.blocks = vec![
            RangeBlock {
                min: [40, 5, 43],
                max: [60, 94, 103],
                material: 2,
            },
            RangeBlock {
                min: [415, 5, 52],
                max: [427, 94, 68],
                material: 2,
            },
            RangeBlock {
                min: [45, 78, 38],
                max: [70, 96, 108],
                material: 3,
            },
            RangeBlock {
                min: [405, 78, 38],
                max: [440, 96, 108],
                material: 3,
            },
            RangeBlock {
                min: [55, 90, 48],
                max: [425, 101, 98],
                material: 3,
            },
        ];
        let mut w = World::new(
            512,
            128,
            160,
            200_000,
            Box::new(Canopy {
                blocks: layout.blocks.clone(),
            }),
        );
        let mut bodies = Bodies::new(16, Fx::from_ratio(20, 900));
        let materials = |mat| match mat {
            2 => Some((140, 240, 5)),
            3 => Some((500, 785, 10)),
            _ => None,
        };
        w.queue_dig(415, 30, 52);
        w.flush();
        let edits = w.edits().to_vec();
        assert_eq!(
            fracture(
                &layout,
                &mut w,
                &mut bodies,
                &edits,
                &mut Vec::new(),
                materials
            ),
            0,
            "a small chip must not break the intact span"
        );
        for z in 52..68 {
            for x in 415..427 {
                w.queue_dig(x, 30, z);
            }
        }
        w.flush();
        let edits = w.edits().to_vec();
        assert_eq!(
            fracture(
                &layout,
                &mut w,
                &mut bodies,
                &edits,
                &mut Vec::new(),
                materials
            ),
            2,
            "the severed pier and its steel cap must not support the right half"
        );
        while w.queued() > 0 {
            w.flush();
        }
        for _ in 0..24 {
            bodies.step(&mut w);
        }
        assert!(
            bodies
                .list
                .iter()
                .any(|b| b.fallen > 0 && b.tilt_angle.abs() > 100),
            "the broken span must tip and descend"
        );
        let slab = bodies.list.iter().find(|b| b.tilt_axis == 2).unwrap();
        let end_y = |x| {
            slab.world_of(voxel::bodies::BodyVoxel {
                x,
                y: 0,
                z: 0,
                mat: 0,
            })
            .1
        };
        assert!(
            end_y(slab.sx - 1) < end_y(0),
            "free end must drop below the end next to the surviving pier"
        );
        assert_eq!(
            w.material(350, 95, 60),
            0,
            "detached deck stayed in the world"
        );
    }
    #[test]
    fn exhausted_body_pool_keeps_the_plate_and_support_stump() {
        let (layout, mut w, mut bodies) = fixture(2);
        bodies.max_bodies = 0;
        cut(&mut w, [40, 35]);
        let edits = w.edits().to_vec();
        assert_eq!(
            fracture(
                &layout,
                &mut w,
                &mut bodies,
                &edits,
                &mut Vec::new(),
                |_| Some((140, 240, 5))
            ),
            0
        );
        assert_eq!(w.queued(), 0, "refused fracture destroyed its support");
        assert_eq!(w.material(44, 45, 39), 2);
        assert_eq!(w.material(60, 55, 45), 2);
    }

    #[test]
    fn one_lost_concrete_support_tears_and_tips_a_corner() {
        let (layout, mut w, mut bodies) = fixture(2);
        cut(&mut w, [40, 35]);
        let edits = w.edits().to_vec();
        assert_eq!(
            fracture(
                &layout,
                &mut w,
                &mut bodies,
                &edits,
                &mut Vec::new(),
                |_| Some((140, 240, 5))
            ),
            1
        );
        assert_eq!(bodies.list[0].voxels.len(), 80 * 57 * 8);
        assert_ne!(bodies.list[0].tilt_axis, 0);
        for _ in 0..16 {
            if w.queued() == 0 {
                break;
            }
            w.flush();
        }
        assert_eq!(w.queued(), 0);
        for _ in 0..24 {
            bodies.step(&mut w);
        }
        assert!(
            bodies.list[0].tilt_angle.abs() > 100,
            "body: {:?}",
            (
                bodies.list[0].tilt_angle,
                bodies.list[0].fallen,
                bodies.list[0].oy,
                w.queued()
            )
        );
        assert!(bodies.list[0].fallen > 0);
    }
    fn count(w: &World, mat: Mat) -> usize {
        let mut n = 0;
        for z in 0..w.nz {
            for y in 0..w.ny {
                for x in 0..w.nx {
                    n += usize::from(w.material(x, y, z) == mat);
                }
            }
        }
        n
    }

    /// **A crushed support crumbles; it does not vanish** (ADR-439).
    /// Every cell a tearing plate takes is in the plate or in the crumbs,
    /// and a weak post under a quadrant that holds leaves the slab over it
    /// whole: inside the slab's box the slab's material is the slab.
    #[test]
    fn a_crushed_support_crumbles_and_leaves_the_slab_over_it() {
        let (mut layout, _, mut bodies) = fixture(2);
        // A post too thin to carry anything, up through the slab, under
        // the quadrant that holds.
        layout.blocks.insert(
            4,
            RangeBlock {
                min: [150, 5, 100],
                max: [152, 60, 102],
                material: 2,
            },
        );
        let mut w = World::new(
            224,
            80,
            160,
            100_000,
            Box::new(Canopy {
                blocks: layout.blocks.clone(),
            }),
        );
        cut(&mut w, [40, 35]);
        let before = count(&w, 2);
        let edits = w.edits().to_vec();
        let mut crumbs = Vec::new();
        assert_eq!(
            fracture(&layout, &mut w, &mut bodies, &edits, &mut crumbs, |_| Some(
                (140, 240, 5)
            )),
            1
        );
        while w.queued() > 0 {
            w.flush();
        }
        let held: usize = bodies.list.iter().map(|b| b.voxels.len()).sum();
        assert_eq!(
            count(&w, 2) + held + crumbs.len(),
            before,
            "matter vanished or doubled"
        );
        // The severed pier, 46 layers of 10×10, and the post below the
        // slab, 47 layers of 2×2.
        assert_eq!(crumbs.len(), 46 * 100 + 47 * 4);
        assert!(crumbs.iter().all(|&(_, m)| m == 2));
        assert_eq!(
            w.material(151, 55, 101),
            2,
            "the post took the slab over it"
        );
    }

    /// **A cell already leaving the world is not taken twice** (ADR-439).
    /// The piece of a pier the cascade tore off this tick holds its own
    /// matter; the crush must not hand the same cells to the crumbs.
    #[test]
    fn a_support_piece_already_falling_is_not_crushed_again() {
        let (layout, mut w, mut bodies) = fixture(2);
        cut(&mut w, [40, 35]);
        for z in 35..45 {
            for x in 40..50 {
                w.queue_dig(x, 20, z);
            }
        }
        w.flush();
        let before = count(&w, 2);
        let piece: Vec<_> = (21..30)
            .flat_map(|y| (35..45).flat_map(move |z| (40..50).map(move |x| (x, y, z))))
            .collect();
        assert!(bodies.detach(&mut w, &piece));
        let edits = w.edits().to_vec();
        let mut crumbs = Vec::new();
        assert_eq!(
            fracture(&layout, &mut w, &mut bodies, &edits, &mut crumbs, |_| Some(
                (140, 240, 5)
            )),
            1
        );
        while w.queued() > 0 {
            w.flush();
        }
        let held: usize = bodies.list.iter().map(|b| b.voxels.len()).sum();
        assert_eq!(
            count(&w, 2) + held + crumbs.len(),
            before,
            "matter vanished or doubled"
        );
        assert_eq!(crumbs.len(), 45 * 100 - 9 * 100);
    }

    /// **A quadrant on the world's frame crushes nothing** (ADR-433,
    /// ADR-439): the frame anchors it, so it cannot tear, and its
    /// supports stay standing.
    #[test]
    fn a_quadrant_on_the_frame_crushes_nothing() {
        let (mut layout, _, mut bodies) = fixture(2);
        layout.blocks[4].min[0] = 0;
        layout.blocks[4].max[0] = 160;
        let mut w = World::new(
            224,
            80,
            160,
            100_000,
            Box::new(Canopy {
                blocks: layout.blocks.clone(),
            }),
        );
        cut(&mut w, [40, 35]);
        let edits = w.edits().to_vec();
        let mut crumbs = Vec::new();
        assert_eq!(
            fracture(&layout, &mut w, &mut bodies, &edits, &mut crumbs, |_| Some(
                (140, 240, 5)
            )),
            0
        );
        assert!(crumbs.is_empty());
        assert_eq!(w.queued(), 0);
        assert_eq!(w.material(44, 45, 39), 2);
    }

    #[test]
    fn steel_span_holds_one_missing_support_but_not_two_on_a_side() {
        let (layout, mut w, mut bodies) = fixture(3);
        cut(&mut w, [40, 35]);
        let edits = w.edits().to_vec();
        assert_eq!(
            fracture(
                &layout,
                &mut w,
                &mut bodies,
                &edits,
                &mut Vec::new(),
                |_| Some((500, 785, 10))
            ),
            0
        );
        cut(&mut w, [40, 110]);
        let edits = w.edits().to_vec();
        assert_eq!(
            fracture(
                &layout,
                &mut w,
                &mut bodies,
                &edits,
                &mut Vec::new(),
                |_| Some((500, 785, 10))
            ),
            2
        );
    }

    #[test]
    fn energy_shot_can_trigger_a_plate_fracture_in_the_live_range() {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.size = [640, 128, 640];
        layout.course = None;
        layout.units.clear();
        layout.target = [345, 20, 340];
        layout.blocks = vec![RangeBlock {
            min: [320, 5, 320],
            max: [500, 17, 450],
            material: 0,
        }];
        for (x, z) in [(340, 335), (340, 410), (440, 335), (440, 410)] {
            layout.blocks.push(RangeBlock {
                min: [x, 5, z],
                max: [x + 10, 52, z + 10],
                material: 2,
            });
        }
        layout.blocks.push(RangeBlock {
            min: [320, 52, 320],
            max: [480, 60, 434],
            material: 2,
        });
        let mut config = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../data/energy-cannon.ron"
        ))
        .unwrap();
        // Keep the live scenario strong enough to sever this support when
        // the stock weapon is rebalanced. The fixture has the same slab
        // thickness and column cross-section as the authored range.
        let authored = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
        let power = authored.zones[2].scenario.as_ref().unwrap().power_milli;
        config.energy = config.energy * power / 1000;
        config.material_energy_j =
            (u64::from(config.material_energy_j) * u64::from(power) / 1000) as u32;
        let mut sim = super::super::BlastRange::with_layout(config, layout).unwrap();
        let mut tilted = false;
        for _ in 0..300 {
            sim.step();
            tilted |= sim.bodies.list.iter().any(|b| b.tilt_axis != 0);
            if tilted {
                break;
            }
        }
        assert!(tilted, "energy shot never fractured a roof corner: tick={} impact={:?} removed={} bodies={} section={}",
            sim.tick, sim.impact, sim.removed, sim.bodies.list.len(),
            support(&sim.world, &sim.layout.as_ref().unwrap().blocks[1],
                &sim.layout.as_ref().unwrap().blocks[5]).area);
        let mut max_angle = 0;
        let mut max_fall = 0;
        for _ in 0..60 {
            sim.step();
            for b in &sim.bodies.list {
                max_angle = max_angle.max(b.tilt_angle.abs());
                max_fall = max_fall.max(b.fallen);
            }
        }
        assert!(
            max_angle > 100 && max_fall > 0,
            "fractured plate neither tipped nor descended: angle={max_angle} fall={max_fall}"
        );
    }
}
