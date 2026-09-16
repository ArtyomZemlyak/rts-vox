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

pub struct Model {
    rules: modding::integrity::Integrity,
    engine: u32,
    paths: Vec<(WeaponPath, Vec<u32>)>,
    /// Cells of each leg's thigh and shin in the template (ADR-281).
    legs: Vec<u32>,
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
    pub fn new(template: &UnitBody) -> Self {
        Self {
            rules: modding::integrity::Integrity::from_ron(include_str!(
                "../../../data/unit-integrity.ron"
            ))
            .expect("unit integrity tuning"),
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
    pub fn buckled(&self, body: &UnitBody) -> Vec<bool> {
        body.legs
            .iter()
            .zip(&self.legs)
            .map(|(leg, original)| {
                let lost = original.saturating_sub(leg_cells(body, leg));
                *original > 0
                    && lost as u64 * 100 > u64::from(*original) * self.rules.leg_buckle_pct as u64
            })
            .collect()
    }

    pub fn evaluate(&self, body: &UnitBody, scratch: &mut CoreBufs) -> (Status, Report) {
        let mut status = body.status(scratch);
        let minimum = [body.core.0, body.core.1, body.core.2];
        let maximum = minimum.map(|v| v + body.core_size);
        let mut core_count = 0;
        let mut core_section = Fx::ONE;
        for axis in 0..3 {
            let path = WeaponPath {
                min: minimum,
                max: maximum,
                axis,
            };
            let values = sections(body, path, scratch.reached_cols());
            if axis == 0 {
                core_count = values.iter().sum();
            }
            for count in values {
                core_section = core_section.min(fraction(count, u32::from(body.core_size).pow(2)));
            }
        }
        let core = fraction(core_count, u32::from(body.core_size).pow(3));
        status.alive &= core >= Fx::from_ratio(self.rules.core_volume_pct, 100)
            && core_section >= Fx::from_ratio(self.rules.core_section_pct, 100);
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
}
