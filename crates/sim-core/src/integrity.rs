//! Spatial operability of installed components. Exterior armour has no HP.
//! Baselines are immutable template geometry; a save stores only damage.
use fx::Fx;
use voxel::{
    topo::unit::CoreBufs,
    unit::{Status, WeaponPath},
    Part, UnitBody,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub core: Fx,
    pub engine: Fx,
    pub gears: [Fx; 8],
    pub gear_count: u8,
    pub drive: Fx,
}

#[cfg(test)]
#[path = "integrity_review_tests.rs"]
mod integrity_review_tests;

/// Whether any single 18-connected void cavity of destroyed core cells spans
/// from one face of `[min .. max)` to the opposite face along X, Y, or Z and
/// removes at least `bore_pct`% of `total_cells`. Disconnected small holes do
/// not sum together; diagonal (`30°–45°`) through-shots are caught identically
/// to cardinal ones.
fn core_cavity_breached(
    min: [usize; 3],
    max: [usize; 3],
    ny: usize,
    words: usize,
    reached: &[u64],
    organ_mask: Option<&[u64]>,
    total_cells: u32,
    bore_pct: i32,
) -> bool {
    if bore_pct >= 100 || total_cells < 8 {
        return false;
    }
    let dims = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    if dims.iter().any(|&d| d < 2) {
        return false;
    }
    let vol = dims[0] * dims[1] * dims[2];
    let mut state = vec![0u8; vol];
    let mut destroyed = 0u32;
    for iz in 0..dims[2] {
        for iy in 0..dims[1] {
            let word_base = ((min[2] + iz) * ny + (min[1] + iy)) * words;
            for ix in 0..dims[0] {
                let gx = min[0] + ix;
                let word = word_base + gx / 64;
                let bit = 1u64 << (gx % 64);
                let was_core = organ_mask.map_or(true, |m| m[word] & bit != 0);
                if was_core && (reached[word] & bit == 0) {
                    state[(iz * dims[1] + iy) * dims[0] + ix] = 1;
                    destroyed += 1;
                }
            }
        }
    }
    if u64::from(destroyed) * 100 < u64::from(total_cells) * bore_pct as u64 {
        return false;
    }
    let mut stack = Vec::new();
    for idx in 0..vol {
        if state[idx] != 1 {
            continue;
        }
        state[idx] = 2;
        stack.clear();
        stack.push(idx);
        let mut comp_cells = 0u32;
        let mut touches = [0u8; 6];
        while let Some(cur) = stack.pop() {
            comp_cells += 1;
            let ix = cur % dims[0];
            let rem = cur / dims[0];
            let iy = rem % dims[1];
            let iz = rem / dims[1];
            if ix == 0 {
                touches[0] = 1;
            }
            if ix + 1 == dims[0] {
                touches[1] = 1;
            }
            if iy == 0 {
                touches[2] = 1;
            }
            if iy + 1 == dims[1] {
                touches[3] = 1;
            }
            if iz == 0 {
                touches[4] = 1;
            }
            if iz + 1 == dims[2] {
                touches[5] = 1;
            }
            for dz in -1..=1isize {
                for dy in -1..=1isize {
                    for dx in -1..=1isize {
                        let manhattan = dx.abs() + dy.abs() + dz.abs();
                        if manhattan == 0 || manhattan > 2 {
                            continue;
                        }
                        let nx = ix as isize + dx;
                        let ny_i = iy as isize + dy;
                        let nz = iz as isize + dz;
                        if nx < 0
                            || ny_i < 0
                            || nz < 0
                            || nx as usize >= dims[0]
                            || ny_i as usize >= dims[1]
                            || nz as usize >= dims[2]
                        {
                            continue;
                        }
                        let nidx =
                            (nz as usize * dims[1] + ny_i as usize) * dims[0] + nx as usize;
                        if state[nidx] == 1 {
                            state[nidx] = 2;
                            stack.push(nidx);
                        }
                    }
                }
            }
        }
        let spans_through = (touches[0] & touches[1]) != 0
            || (touches[2] & touches[3]) != 0
            || (touches[4] & touches[5]) != 0;
        if spans_through
            && u64::from(comp_cells) * 100 >= u64::from(total_cells) * bore_pct as u64
        {
            return true;
        }
    }
    false
}

struct CoreOrgan {
    min: [u8; 3],
    max: [u8; 3],
    total: u32,
    mask: Vec<u64>,
    sections: [Vec<u32>; 3],
}

impl CoreOrgan {
    fn from_template(template: &UnitBody) -> Option<Self> {
        let (nx, ny, nz) = (
            template.vol.nx as usize,
            template.vol.ny as usize,
            template.vol.nz as usize,
        );
        let words = nx.div_ceil(64);
        let mut mask = vec![0u64; ny * nz * words];
        let mut lo = [usize::MAX; 3];
        let mut hi = [0usize; 3];
        let mut total = 0u32;
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    if template.vol.get(x, y, z) && template.part_at(x, y, z) == Part::Core {
                        lo[0] = lo[0].min(x);
                        lo[1] = lo[1].min(y);
                        lo[2] = lo[2].min(z);
                        hi[0] = hi[0].max(x + 1);
                        hi[1] = hi[1].max(y + 1);
                        hi[2] = hi[2].max(z + 1);
                        mask[(z * ny + y) * words + x / 64] |= 1u64 << (x % 64);
                        total += 1;
                    }
                }
            }
        }
        if total == 0 {
            return None;
        }
        let min = lo.map(|v| v as u8);
        let max = hi.map(|v| v as u8);
        let mut sections: [Vec<u32>; 3] = Default::default();
        for axis in 0..3 {
            let a = (axis + 1) % 3;
            let b = (axis + 2) % 3;
            let len_s = hi[axis] - lo[axis];
            let len_a = hi[a] - lo[a];
            let len_b = hi[b] - lo[b];
            let mut sec = vec![0u32; len_s];
            for ia in 0..len_a {
                for ib in 0..len_b {
                    for is in 0..len_s {
                        let mut p = [0usize; 3];
                        p[axis] = lo[axis] + is;
                        p[a] = lo[a] + ia;
                        p[b] = lo[b] + ib;
                        let word = (p[2] * ny + p[1]) * words + p[0] / 64;
                        if mask[word] & (1u64 << (p[0] % 64)) != 0 {
                            sec[is] += 1;
                        }
                    }
                }
            }
            sections[axis] = sec;
        }
        Some(Self {
            min,
            max,
            total,
            mask,
            sections,
        })
    }
}

pub struct Model {
    rules: modding::integrity::Integrity,
    organ: Option<CoreOrgan>,
    engine: u32,
    paths: Vec<(WeaponPath, Vec<u32>)>,
    /// Cells of each leg's thigh and shin in the template (ADR-281).
    legs: Vec<u32>,
}

/// Whether a link's solid cells no longer join its two ends along its
/// length — the x axis of a leg's link (ADR-304). A link cut through
/// anywhere is severed, however many cells stand on either side of the
/// cut; a link with no cell at its hip end is severed too.
fn severed(b: &UnitBody) -> bool {
    let (nx, ny, nz) = (b.vol.nx as usize, b.vol.ny as usize, b.vol.nz as usize);
    if nx == 0 || ny == 0 || nz == 0 {
        return true;
    }
    let idx = |x: usize, y: usize, z: usize| x + nx * (y + ny * z);
    let mut seen = vec![false; nx * ny * nz];
    let mut stack = Vec::new();
    for z in 0..nz {
        for y in 0..ny {
            if b.vol.get(0, y, z) {
                seen[idx(0, y, z)] = true;
                stack.push((0usize, y, z));
            }
        }
    }
    while let Some((x, y, z)) = stack.pop() {
        if x + 1 == nx {
            return false;
        }
        let mut next = Vec::with_capacity(6);
        if x > 0 {
            next.push((x - 1, y, z));
        }
        next.push((x + 1, y, z));
        if y > 0 {
            next.push((x, y - 1, z));
        }
        if y + 1 < ny {
            next.push((x, y + 1, z));
        }
        if z > 0 {
            next.push((x, y, z - 1));
        }
        if z + 1 < nz {
            next.push((x, y, z + 1));
        }
        for (qx, qy, qz) in next {
            let i = idx(qx, qy, qz);
            if !seen[i] && b.vol.get(qx, qy, qz) {
                seen[i] = true;
                stack.push((qx, qy, qz));
            }
        }
    }
    true
}

/// Cells of a leg's thigh and shin in a body.
fn leg_cells(body: &UnitBody, leg: &voxel::unit::Leg) -> u32 {
    [leg.thigh, leg.shin]
        .iter()
        .filter_map(|i| body.segments.get(usize::from(*i)))
        .map(|s| s.body.solid_count())
        .sum()
}

fn fraction(current: u32, original: u32) -> Fx {
    if original == 0 {
        Fx::ZERO
    } else {
        Fx::from_ratio(current as i32, original as i32).min(Fx::ONE)
    }
}

fn sections(body: &UnitBody, path: WeaponPath, reached: &[u64]) -> Vec<u32> {
    let axis = path.axis as usize;
    let a = (axis + 1) % 3;
    let b = (axis + 2) % 3;
    let words = body.vol.nx.div_ceil(64) as usize;
    (path.min[axis]..path.max[axis])
        .map(|slice| {
            let mut count = 0;
            for u in path.min[a]..path.max[a] {
                for v in path.min[b]..path.max[b] {
                    let mut p = [0usize; 3];
                    p[axis] = slice as usize;
                    p[a] = u as usize;
                    p[b] = v as usize;
                    let word = (p[2] * body.vol.ny as usize + p[1]) * words + p[0] / 64;
                    count += u32::from(reached[word] & (1 << (p[0] % 64)) != 0);
                }
            }
            count
        })
        .collect()
}

impl Model {
    pub fn severed_legs(&self, body: &UnitBody) -> Vec<bool> {
        body.legs
            .iter()
            .map(|leg| {
                [leg.thigh, leg.shin]
                    .iter()
                    .any(|i| severed(&body.segments[usize::from(*i)].body))
            })
            .collect()
    }
    pub fn new(template: &UnitBody) -> Self {
        Self {
            rules: modding::integrity::Integrity::from_ron(include_str!(
                "../../../data/unit-integrity.ron"
            ))
            .expect("unit integrity tuning"),
            organ: CoreOrgan::from_template(template),
            engine: template.subsystem_count(Part::Engine),
            paths: template
                .load_paths()
                .iter()
                .map(|p| (*p, sections(template, *p, &template.vol.cols)))
                .collect(),
            legs: template
                .legs
                .iter()
                .map(|l| leg_cells(template, l))
                .collect(),
        }
    }

    /// Which legs have lost more than the buckling share of their thigh and
    /// shin cells (ADR-281), in leg order. A leg with no cells in the
    /// template never buckles.
    /// A leg buckles when its links lost more than the buckling share of
    /// their cells (ADR-281), or when either link is cut through (ADR-304):
    /// a thin leg severed by a bolt stands on nothing whatever is left of
    /// it.
    pub fn buckled(&self, body: &UnitBody) -> Vec<bool> {
        body.legs
            .iter()
            .zip(&self.legs)
            .map(|(leg, original)| {
                let lost = original.saturating_sub(leg_cells(body, leg));
                let cut = [leg.thigh, leg.shin]
                    .iter()
                    .filter_map(|i| body.segments.get(usize::from(*i)))
                    .any(|s| severed(&s.body));
                *original > 0
                    && (cut
                        || lost as u64 * 100
                            > u64::from(*original) * self.rules.leg_buckle_pct as u64)
            })
            .collect()
    }

    pub fn evaluate(&self, body: &UnitBody, scratch: &mut CoreBufs) -> (Status, Report) {
        let mut status = body.status(scratch);
        let reached = scratch.reached_cols();
        let ny = body.vol.ny as usize;
        let words = body.vol.nx.div_ceil(64) as usize;
        let minimum = [body.core.0, body.core.1, body.core.2];
        let maximum = minimum.map(|v| v + body.core_size);
        let face_rays = u32::from(body.core_size).pow(2);
        let mut core_count = 0u32;
        let mut core_section = Fx::ONE;
        for axis in 0..3 {
            let a = (axis + 1) % 3;
            let b = (axis + 2) % 3;
            for slice in minimum[axis]..maximum[axis] {
                let mut count = 0u32;
                for u in minimum[a]..maximum[a] {
                    for v in minimum[b]..maximum[b] {
                        let mut p = [0usize; 3];
                        p[axis] = slice as usize;
                        p[a] = u as usize;
                        p[b] = v as usize;
                        let word = (p[2] * ny + p[1]) * words + p[0] / 64;
                        count += u32::from(reached[word] & (1u64 << (p[0] % 64)) != 0);
                    }
                }
                if axis == 0 {
                    core_count += count;
                }
                core_section = core_section.min(fraction(count, face_rays));
            }
        }
        let mut perforated = body.core_size >= 2
            && self.rules.core_bore_pct < 100
            && core_cavity_breached(
                minimum.map(usize::from),
                maximum.map(usize::from),
                ny,
                words,
                reached,
                None,
                u32::from(body.core_size).pow(3),
                self.rules.core_bore_pct,
            );
        let mut core = fraction(core_count, u32::from(body.core_size).pow(3));
        if let Some(organ) = &self.organ {
            if organ.mask.len() == reached.len() {
                let organ_connected: u32 = organ
                    .mask
                    .iter()
                    .zip(reached)
                    .map(|(m, r)| (m & r).count_ones())
                    .sum();
                core = core.min(fraction(organ_connected, organ.total));
                for axis in 0..3 {
                    let a = (axis + 1) % 3;
                    let b = (axis + 2) % 3;
                    let lo = organ.min.map(usize::from);
                    let hi = organ.max.map(usize::from);
                    for (is, &base) in organ.sections[axis].iter().enumerate() {
                        if base == 0 {
                            continue;
                        }
                        let mut count = 0u32;
                        for u in lo[a]..hi[a] {
                            for v in lo[b]..hi[b] {
                                let mut p = [0usize; 3];
                                p[axis] = lo[axis] + is;
                                p[a] = u;
                                p[b] = v;
                                let word = (p[2] * ny + p[1]) * words + p[0] / 64;
                                count += u32::from(
                                    (organ.mask[word] & reached[word]) & (1u64 << (p[0] % 64)) != 0,
                                );
                            }
                        }
                        core_section = core_section.min(fraction(count, base));
                    }
                }
                if !perforated && self.rules.core_bore_pct < 100 {
                    perforated = core_cavity_breached(
                        organ.min.map(usize::from),
                        organ.max.map(usize::from),
                        ny,
                        words,
                        reached,
                        Some(&organ.mask),
                        organ.total,
                        self.rules.core_bore_pct,
                    );
                }
            }
        }
        status.alive &= core >= Fx::from_ratio(self.rules.core_volume_pct, 100)
            && core_section >= Fx::from_ratio(self.rules.core_section_pct, 100)
            && !perforated;
        let engine = fraction(
            body.connected_subsystem_count(Part::Engine, scratch),
            self.engine,
        );
        status.engine &=
            status.alive && engine >= Fx::from_ratio(self.rules.engine_volume_pct, 100);
        status.weapon &= status.alive;
        status.sensor &= status.alive;
        let mut gears = [Fx::ZERO; 8];
        let mut working = 0;
        let mut sum = Fx::ZERO;
        for (i, (path, baseline)) in self.paths.iter().enumerate() {
            let current = sections(body, *path, scratch.reached_cols());
            let weakest = current
                .iter()
                .zip(baseline)
                .map(|(a, b)| fraction(*a, *b))
                .min()
                .unwrap_or(Fx::ZERO);
            if weakest >= Fx::from_ratio(self.rules.gear_section_pct, 100) {
                gears[i] = weakest;
                working += 1;
                sum = sum.add(weakest);
            }
        }
        let drive = if self.paths.is_empty() {
            Fx::ONE
        } else if working * 100 >= self.paths.len() * self.rules.required_gears_pct as usize {
            sum.div(Fx::from_int(self.paths.len() as i32))
        } else {
            Fx::ZERO
        };
        (
            status,
            Report {
                core,
                engine,
                gears,
                gear_count: self.paths.len() as u8,
                drive,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inspect(model: &Model, body: &UnitBody) -> (Status, Report) {
        model.evaluate(body, &mut CoreBufs::new())
    }
    fn remove_box(body: &mut UnitBody, min: [u8; 3], max: [u8; 3]) {
        for z in min[2]..max[2] {
            for y in min[1]..max[1] {
                for x in min[0]..max[0] {
                    body.remove(x as usize, y as usize, z as usize);
                }
            }
        }
    }
    #[test]
    fn a_core_chip_survives_but_a_severed_core_cannot_run_on_its_remaining_mass() {
        let mut tank = voxel::unit::tank();
        let model = Model::new(&tank);
        let (x, y, z) = tank.core;
        tank.remove(x as usize, y as usize, z as usize);
        assert!(inspect(&model, &tank).0.alive);
        let mut split = voxel::unit::tank();
        let size = split.core_size;
        remove_box(&mut split, [x + 1, y, z], [x + 2, y + size, z + size]);
        let (status, report) = inspect(&model, &split);
        assert!(report.core >= Fx::from_ratio(3, 4));
        assert!(!status.alive && !status.engine && !status.weapon);
        let mut remnant = voxel::unit::tank();
        let size = remnant.core_size;
        remove_box(&mut remnant, [x, y, z], [x + size, y + size, z + size]);
        remnant.set(x as usize, y as usize, z as usize, Part::Core);
        assert!(remnant.status(&mut CoreBufs::new()).alive);
        assert!(!inspect(&model, &remnant).0.alive);
    }
    #[test]
    fn a_track_break_disables_driving_without_killing_the_engine_or_weapon() {
        let mut tank = voxel::unit::tank();
        let model = Model::new(&tank);
        assert_eq!(inspect(&model, &tank).1.drive, Fx::ONE);
        remove_box(&mut tank, [30, 0, 1], [31, 10, 7]);
        let (status, report) = inspect(&model, &tank);
        assert!(status.alive && status.engine && status.weapon);
        assert_eq!(report.engine, Fx::ONE);
        assert_eq!(report.gears[0], Fx::ZERO);
        assert_eq!(report.gears[1], Fx::ONE);
        assert_eq!(report.drive, Fx::ZERO);
    }
    #[test]
    fn an_engine_remnant_is_not_a_functioning_motor() {
        let mut tank = voxel::unit::tank();
        let model = Model::new(&tank);
        for z in 0..tank.vol.nz as usize {
            for y in 0..tank.vol.ny as usize {
                for x in 0..tank.vol.nx as usize {
                    if tank.part_at(x, y, z) == Part::Engine && [x, y, z] != [19, 8, 15] {
                        tank.remove(x, y, z);
                    }
                }
            }
        }
        let (status, report) = inspect(&model, &tank);
        assert!(status.alive && status.weapon);
        assert!(report.engine > Fx::ZERO);
        assert!(!status.engine);
    }
    #[test]
    fn the_walker_limps_on_three_attachments_and_stops_on_two() {
        let mut body = voxel::unit::strider();
        let model = Model::new(&body);
        assert_eq!(inspect(&model, &body).1.drive, Fx::ONE);
        for i in 0..2 {
            let p = body.load_paths()[i];
            remove_box(&mut body, p.min, p.max);
            let (status, report) = inspect(&model, &body);
            assert!(status.alive && status.engine);
            assert_eq!(report.gears[i], Fx::ZERO);
            if i == 0 {
                assert!(report.drive > Fx::ZERO && report.drive < Fx::ONE);
            } else {
                assert_eq!(report.drive, Fx::ZERO);
            }
        }
    }
    #[test]
    fn loss_of_noncritical_armour_has_no_scalar_health_penalty() {
        let mut tank = voxel::unit::tank();
        let model = Model::new(&tank);
        let (_, whole) = inspect(&model, &tank);
        let before = tank.solid_count();
        remove_box(&mut tank, [40, 12, 8], [48, 17, 22]);
        let (status, after) = inspect(&model, &tank);
        assert!(tank.solid_count() < before);
        assert!(status.alive && status.engine);
        assert_eq!(after, whole);
    }
    #[test]
    fn disabled_drivetrain_resumes_from_geometry_and_cannot_accelerate() {
        use crate::damage::Bodies;
        use fx::blob::{Blob, Scan};
        let mut b = Bodies::new(vec![voxel::unit::tank()]);
        b.spawn_at(0, 0);
        let cells = (0..10)
            .flat_map(|y| (1..7).map(move |z| [30, y, z]))
            .collect();
        b.queue_cells(0, b.generations[0], cells, 1_000_000, 0, phys::V3::ZERO, 16);
        b.step(&[1; 5], 1000, 1000);
        let s = b.state(0).unwrap();
        assert!(s.status.alive && s.status.engine);
        assert_eq!(s.mobility.traction, Fx::ZERO);
        let limits = s.mobility.limits(
            &crate::drive::Limits::of(&crate::test_tuning().tracked, 30),
            false,
        );
        let (drive, _) = crate::drive::advance(
            crate::drive::Drive::default(),
            0,
            Fx::from_int(60),
            &limits,
            Fx::ONE,
            Fx::from_ratio(1, 30),
        );
        assert_eq!(drive.vel, Fx::ZERO);
        let mut save = Blob::new();
        b.save(&mut save);
        let mut loaded = Bodies::new(vec![voxel::unit::tank()]);
        assert!(loaded.load(&mut Scan::new(&save.v), 1, 16));
        assert_eq!(loaded.state(0).unwrap().integrity, s.integrity);
        assert_eq!(loaded.state(0).unwrap().mobility, s.mobility);
    }

    #[test]
    fn resampling_preserves_a_broken_load_path_in_physical_space() {
        for mm in [50, 100, 200] {
            let mut tank = voxel::unit::tank().resampled(mm).unwrap();
            assert_eq!(tank.load_paths().len(), 2);
            let model = Model::new(&tank);
            let p = tank.load_paths()[0];
            let mut min = p.min;
            let mut max = p.max;
            min[0] = (u16::from(p.min[0]) + u16::from(p.max[0])).div_ceil(2) as u8;
            max[0] = min[0] + 1;
            remove_box(&mut tank, min, max);
            let (status, report) = inspect(&model, &tank);
            assert!(status.alive && status.engine);
            assert_eq!(report.gears[0], Fx::ZERO, "{mm} mm track");
            assert_eq!(report.drive, Fx::ZERO);
        }
    }

    #[test]
    fn through_breach_across_core_at_cardinal_or_oblique_angle_destroys_unit() {
        let base = voxel::unit::commander();
        let model = Model::new(&base);
        let (cx, cy, cz) = (
            base.core.0 as usize,
            base.core.1 as usize,
            base.core.2 as usize,
        );
        let cs = base.core_size as usize;
        assert!(cs >= 6);

        let mut pinhole = base.clone();
        for dx in 0..cs {
            pinhole.remove(cx + dx, cy + cs / 2, cz + cs / 2);
        }
        assert!(inspect(&model, &pinhole).0.alive);

        let mut blind_dent = base.clone();
        for dx in 2..cs {
            for dy in 2..6 {
                for dz in 2..6 {
                    blind_dent.remove(cx + dx, cy + dy, cz + dz);
                }
            }
        }
        assert!(inspect(&model, &blind_dent).0.alive);

        let mut cardinal_bore = base.clone();
        for dx in 0..cs {
            for dy in 2..6 {
                for dz in 2..6 {
                    cardinal_bore.remove(cx + dx, cy + dy, cz + dz);
                }
            }
        }
        let (status, report) = inspect(&model, &cardinal_bore);
        assert!(report.core >= Fx::from_ratio(60, 100));
        assert!(!status.alive && !status.engine && !status.weapon);

        let mut oblique_bore = base;
        for dx in 0..cs {
            let z_shift = dx / 2;
            for dy in 2..6 {
                for dz in (1 + z_shift)..(4 + z_shift) {
                    if dz < cs {
                        oblique_bore.remove(cx + dx, cy + dy, cz + dz);
                    }
                }
            }
        }
        let (status_oblique, _) = inspect(&model, &oblique_bore);
        assert!(!status_oblique.alive);
    }
}
