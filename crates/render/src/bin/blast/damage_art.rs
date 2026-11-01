//! Map each visual skin cell to nearby authoritative root material once.
//! A segment's own volume is authoritative since ADR-278 (its cells take
//! hits in their frame); the root projection still opens the plates that
//! merely cover damaged root steps.
use bevy::prelude::*;
use voxel::unit::UnitBody;

type Cell = [u8; 3];
pub struct SkinBindings {
    cells: Vec<Vec<Option<Cell>>>,
}
impl SkinBindings {
    pub fn new(body: &UnitBody) -> Self {
        let centre = Vec3::from(render::model::root_centre(body));
        let frames = render::model::segment_frames(body, 0.01);
        let mut cells = Vec::new();
        for (seg, frame) in body.segments.iter().zip(frames) {
            let b = &seg.body;
            let pivot = Vec3::from(render::model::pivot_centre(&seg.attach));
            let mut map = vec![None; b.vol.nx as usize * b.vol.ny as usize * b.vol.nz as usize];
            for z in 0..b.vol.nz {
                for y in 0..b.vol.ny {
                    for x in 0..b.vol.nx {
                        if !b.vol.get(x as usize, y as usize, z as usize) {
                            continue;
                        }
                        let local = Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5);
                        let point = centre + frame.pos + frame.rot * (local - pivot);
                        let q = point.floor().as_ivec3();
                        let mut nearest = None;
                        let mut distance = f32::INFINITY;
                        // A skin may sit outside the root bounds. Search the backing
                        // material, not the attachment coordinate or an air sample.
                        for dz in -6..=6 {
                            for dy in -6..=6 {
                                for dx in -6..=6 {
                                    let p = q + IVec3::new(dx, dy, dz);
                                    if p.cmplt(IVec3::ZERO).any()
                                        || p.cmpge(IVec3::new(
                                            body.vol.nx as i32,
                                            body.vol.ny as i32,
                                            body.vol.nz as i32,
                                        ))
                                        .any()
                                    {
                                        continue;
                                    }
                                    if !body.vol.get(p.x as usize, p.y as usize, p.z as usize) {
                                        continue;
                                    }
                                    let d =
                                        (p.as_vec3() + Vec3::splat(0.5) - point).length_squared();
                                    if d < distance {
                                        distance = d;
                                        nearest = Some([p.x as u8, p.y as u8, p.z as u8]);
                                    }
                                }
                            }
                        }
                        map[index(b, [x, y, z])] = nearest;
                    }
                }
            }
            cells.push(map);
        }
        Self { cells }
    }
    pub fn owner(&self, pristine: &UnitBody, seg: usize, p: Cell) -> Option<Cell> {
        let b = &pristine.segments[seg].body;
        if p[0] >= b.vol.nx || p[1] >= b.vol.ny || p[2] >= b.vol.nz {
            return None;
        }
        self.cells[seg][index(b, p)]
    }
    pub fn clipped(&self, pristine: &UnitBody, current: &UnitBody, seg: usize) -> UnitBody {
        let mut b = current.segments[seg].body.clone();
        for z in 0..b.vol.nz {
            for y in 0..b.vol.ny {
                for x in 0..b.vol.nx {
                    if let Some(p) = self.owner(pristine, seg, [x, y, z]) {
                        if !current.vol.get(p[0] as usize, p[1] as usize, p[2] as usize) {
                            b.remove(x as usize, y as usize, z as usize);
                        }
                    }
                }
            }
        }
        b
    }
}
fn index(b: &UnitBody, p: Cell) -> usize {
    p[0] as usize + b.vol.nx as usize * (p[1] as usize + b.vol.ny as usize * p[2] as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_range_cuts_exposed_panels_and_keeps_sheltered_panels() {
        let c = modding::explosion::ExplosionConfig::from_ron(include_str!(
            "../../../../../data/energy-cannon.ron"
        ))
        .unwrap();
        let layout = modding::range::RangeLayout::from_ron(include_str!(
            "../../../../../data/range-cover.ron"
        ))
        .unwrap();
        let mut range = sim_core::blast::BlastRange::with_layout(c, layout).unwrap();
        for _ in 0..250 {
            range.step();
        }
        let templates = [
            voxel::unit::tank(),
            voxel::unit::strider(),
            voxel::unit::spider(),
            voxel::unit::artillery(),
            voxel::unit::turret_t1(),
            voxel::unit::turret_t2(),
            voxel::unit::command_base(),
            voxel::unit::artillery_t2(),
            voxel::unit::artillery_turret_t2(),
            voxel::unit::artillery_turret_t3(),
            voxel::unit::nuclear_silo(),
        ];
        let bindings = templates.each_ref().map(SkinBindings::new);
        let lost: Vec<u32> = range
            .units
            .iter()
            .map(|u| {
                let kind = match u.kind {
                    modding::range::RangeUnitKind::Tank => 0,
                    modding::range::RangeUnitKind::Strider => 1,
                    modding::range::RangeUnitKind::Spider => 2,
                    modding::range::RangeUnitKind::Artillery => 3,
                    modding::range::RangeUnitKind::TurretT1 => 4,
                    modding::range::RangeUnitKind::TurretT2 => 5,
                    modding::range::RangeUnitKind::Base => 6,
                    modding::range::RangeUnitKind::ArtilleryT2 => 7,
                    modding::range::RangeUnitKind::ArtilleryTurretT2 => 8,
                    modding::range::RangeUnitKind::ArtilleryTurretT3 => 9,
                    modding::range::RangeUnitKind::NuclearSilo => 10,
                    modding::range::RangeUnitKind::Crawler => 11,
                    modding::range::RangeUnitKind::LightningTank => 0,
                    modding::range::RangeUnitKind::EidolonTank => 0,
                };
                let pristine = &templates[kind];
                pristine
                    .segments
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        s.body.solid_count()
                            - bindings[kind].clipped(pristine, &u.body, i).solid_count()
                    })
                    .sum()
            })
            .collect();
        println!("actual cover: projected skin cells removed={lost:?}");
        assert!(
            lost[0] > 20,
            "actual exposed skins still concealed destroyed root material"
        );
        assert_eq!(lost[1], 0);
    }
    #[test]
    fn outer_plate_opens_a_local_hole_while_its_mount_survives() {
        let pristine = voxel::unit::tank();
        let bindings = SkinBindings::new(&pristine);
        let mut damaged = pristine.clone();
        let seg = 0;
        let b = &pristine.segments[seg].body;
        let mut owners = std::collections::BTreeSet::new();
        for z in 4..10 {
            for y in 0..b.vol.ny {
                for x in 3..6 {
                    if let Some(p) = bindings.owner(&pristine, seg, [x, y, z]) {
                        owners.insert(p);
                    }
                }
            }
        }
        let anchor = pristine.segments[seg].attach.at;
        for p in owners {
            if p != [anchor.0 as u8, anchor.1 as u8, anchor.2 as u8] {
                damaged.remove(p[0] as usize, p[1] as usize, p[2] as usize);
            }
        }
        assert_eq!(
            pristine
                .vol
                .get(anchor.0 as usize, anchor.1 as usize, anchor.2 as usize),
            damaged
                .vol
                .get(anchor.0 as usize, anchor.1 as usize, anchor.2 as usize)
        );
        let cut = bindings.clipped(&pristine, &damaged, seg);
        assert!(cut.solid_count() > 0 && cut.solid_count() < b.solid_count());
        assert_eq!(
            bindings.clipped(&pristine, &pristine, seg).vol.cols,
            b.vol.cols
        );
        // The opposite side of this very plate remains intact.
        assert!(cut.vol.get(0, 0, b.vol.nz as usize - 1));
    }
}
