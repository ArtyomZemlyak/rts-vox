//! Validated, explicit snapshot tail for deferred solid impacts (ADR-580).
use super::*;
use fx::blob::{Blob, Scan};

impl Pending {
    pub fn version(&self) -> u32 {
        if self.jobs.is_empty() {
            48
        } else if self.jobs.iter().any(|j| j.direct.is_some()) {
            50
        } else {
            49
        }
    }
    #[cfg(test)]
    pub fn save(&self, b: &mut Blob) {
        self.save_as(b, self.version());
    }
    pub fn save_as(&self, b: &mut Blob, version: u32) {
        let direct_tags = version >= 50;
        b.u32(self.jobs.len() as u32);
        for j in &self.jobs {
            let c = &j.config;
            for n in [
                c.energy,
                c.material_energy_j,
                c.fragment_box,
                c.unit_ablation_resistance,
                c.soil_resistance,
                c.concrete_resistance,
                c.steel_resistance,
            ] {
                b.u32(n);
            }
            for n in c.unit_role_resistance_milli {
                b.u32(n);
            }
            b.bool(c.spall.is_some());
            if let Some(s) = &c.spall {
                b.u8(s.radius);
                b.u32(s.work_milli);
            }
            for n in j.at {
                b.i32(n);
            }
            b.u32(j.ray.len() as u32);
            for p in &j.ray {
                for n in p {
                    b.i32(*n);
                }
            }
            b.usize(j.cursor);
            b.u32(j.work);
            b.bool(j.lateral.is_some());
            if let Some(n) = j.lateral {
                b.u32(n);
            }
            b.u32(j.seeds.len() as u32);
            for &cell in &j.seeds {
                save_cell(b, cell);
            }
            b.bool(j.spall.is_some());
            if let Some(doses) = &j.spall {
                b.u32(doses.len() as u32);
                for &(id, part, cell, dose) in doses {
                    save_cell(b, (id, part, cell));
                    b.u32(dose);
                }
            }
            b.usize(j.spall_cursor);
            b.u64(j.remaining_mj);
            let poses = j.poses.as_ref().expect("pending impact freezes contacts");
            b.u32(poses.len() as u32);
            for p in poses {
                b.usize(p.id);
                save_frame(b, &p.root);
                b.i32(p.delta);
                b.u32(p.segments.len() as u32);
                for f in &p.segments {
                    save_frame(b, f);
                }
            }
            b.u16s(&j.ground);
            if direct_tags {
                match &j.direct {
                    None => b.u8(0),
                    Some(Direct::Body(target)) => {
                        b.u8(1);
                        save_cell(b, *target);
                    }
                    Some(Direct::Ground(mat)) => {
                        b.u8(2);
                        b.u16(*mat);
                    }
                }
            }
        }
    }

    pub fn load(
        s: &mut Scan<'_>,
        base: &ExplosionConfig,
        units: &[RangeUnit],
        size: [u32; 3],
        max_jobs: usize,
        version: u32,
    ) -> Option<Self> {
        // Version 49 reserved penetrators independently of machine-gun flight.
        // Accept the finite legacy overlap; producers wait until it drains.
        let count = bounded_count(
            s,
            if version >= 50 {
                max_jobs * 2
            } else {
                max_jobs
            },
            100,
        )?;
        if count == 0 && version < 51 {
            return None;
        }
        let mut jobs = VecDeque::with_capacity(count);
        for _ in 0..count {
            let mut config = base.clone();
            config.delivery = modding::explosion::Delivery::Kinetic;
            config.energy = s.u32();
            config.material_energy_j = s.u32();
            config.fragment_box = s.u32();
            config.unit_ablation_resistance = s.u32();
            config.soil_resistance = s.u32();
            config.concrete_resistance = s.u32();
            config.steel_resistance = s.u32();
            config.unit_role_resistance_milli = std::array::from_fn(|_| s.u32());
            config.spall = if s.bool() {
                Some(modding::explosion::Spall {
                    radius: s.u8(),
                    work_milli: s.u32(),
                })
            } else {
                None
            };
            if config.energy == 0
                || !(1..=4).contains(&config.fragment_box)
                || config.unit_ablation_resistance > 1_000_000
                || [
                    config.soil_resistance,
                    config.concrete_resistance,
                    config.steel_resistance,
                ]
                .iter()
                .any(|n| !(1..=1_000_000).contains(n))
                || config
                    .unit_role_resistance_milli
                    .iter()
                    .any(|n| !(1..=10_000).contains(n))
                || config.spall.as_ref().is_some_and(|p| {
                    !(1..=4).contains(&p.radius) || !(1..=900).contains(&p.work_milli)
                })
            {
                return None;
            }
            let at = std::array::from_fn(|_| s.i32());
            if !valid_point(at, size) {
                return None;
            }
            let n = bounded_count(s, RAY_CELLS, 12)?;
            if n == 0 {
                return None;
            }
            let mut ray = Vec::with_capacity(n);
            for _ in 0..n {
                let p = std::array::from_fn(|_| s.i32());
                if !valid_point(p, size) || ray.last() == Some(&p) {
                    return None;
                }
                ray.push(p);
            }
            let cursor = s.usize();
            let work = s.u32();
            let lateral = if s.bool() { Some(s.u32()) } else { None };
            if cursor > n
                || u64::from(work) + u64::from(lateral.unwrap_or(0)) > u64::from(config.energy)
                || (lateral.is_some() && config.spall.is_none())
            {
                return None;
            }
            let nseeds = bounded_count(s, RAY_CELLS, 40)?;
            let mut seeds = Vec::with_capacity(nseeds);
            for _ in 0..nseeds {
                seeds.push(load_cell(s, units)?);
            }
            let spall = if s.bool() {
                if cursor != ray.len() {
                    return None;
                }
                let count = bounded_count(s, SPALL_CELLS, 44)?;
                let mut doses = Vec::with_capacity(count);
                let mut total = 0u64;
                let mut previous = None;
                for _ in 0..count {
                    let cell = load_cell(s, units)?;
                    let dose = s.u32();
                    if dose == 0 || previous.is_some_and(|p| p >= cell) {
                        return None;
                    }
                    total += u64::from(dose);
                    previous = Some(cell);
                    doses.push((cell.0, cell.1, cell.2, dose));
                }
                if total > u64::from(lateral.unwrap_or(0)) {
                    return None;
                }
                Some(doses)
            } else {
                None
            };
            let spall_cursor = s.usize();
            let remaining_mj = s.u64();
            if spall_cursor > spall.as_ref().map_or(0, Vec::len)
                || remaining_mj > u64::from(config.material_energy_j) * 1000
                || (cursor == ray.len() && spall.as_ref().is_some_and(|v| spall_cursor == v.len()))
            {
                return None;
            }
            let np = bounded_count(s, units.len(), 144)?;
            let mut poses = Vec::with_capacity(np);
            let mut previous = None;
            for _ in 0..np {
                let id = s.usize();
                if id >= units.len() || previous.is_some_and(|p| p >= id) {
                    return None;
                }
                previous = Some(id);
                let root = load_frame(s, size)?;
                let delta = s.i32();
                if !(-fx::angle::HALF..=fx::angle::HALF).contains(&delta) {
                    return None;
                }
                let ns = bounded_count(s, units[id].body.segments.len(), 128)?;
                if ns != units[id].body.segments.len() {
                    return None;
                }
                let mut segments = Vec::with_capacity(ns);
                for _ in 0..ns {
                    segments.push(load_frame(s, size)?);
                }
                poses.push(Pose {
                    id,
                    root,
                    delta,
                    segments,
                });
            }
            let ng = bounded_count(s, ray.len(), 2)?;
            if ng != ray.len() {
                return None;
            }
            let mut ground = Vec::with_capacity(ng);
            for _ in 0..ng {
                let mat = s.u16();
                if mat > 15 {
                    return None;
                }
                ground.push(mat);
            }
            let direct = if version >= 50 {
                match s.u8() {
                    0 => None,
                    1 => Some(Direct::Body(load_cell(s, units)?)),
                    2 => {
                        let mat = s.u16();
                        if mat == 0 || mat > 15 || ground != [mat] {
                            return None;
                        }
                        Some(Direct::Ground(mat))
                    }
                    _ => return None,
                }
            } else {
                None
            };
            if direct.is_some()
                && (ray.len() != 1
                    || cursor != 0
                    || work != config.energy
                    || lateral.is_some()
                    || !seeds.is_empty()
                    || spall.is_some()
                    || spall_cursor != 0
                    || !poses.is_empty()
                    || config.spall.is_some()
                    || config.fragment_box != 1)
            {
                return None;
            }
            jobs.push_back(Job {
                direct,
                config,
                at,
                ray,
                cursor,
                work,
                lateral,
                seeds,
                spall,
                spall_cursor,
                remaining_mj,
                poses: Some(poses),
                ground,
            });
        }
        let direct = jobs.iter().filter(|j| j.direct.is_some()).count();
        if direct > max_jobs || jobs.len() - direct > max_jobs {
            return None;
        }
        s.ok().then_some(Self {
            jobs,
            stats: [0; 3],
            direct_stats: [0; 3],
        })
    }
}

fn bounded_count(s: &mut Scan<'_>, max: usize, bytes: usize) -> Option<usize> {
    let n = s.u32() as usize;
    (n <= max && n.saturating_mul(bytes) <= s.left()).then_some(n)
}
fn valid_point(p: [i32; 3], size: [u32; 3]) -> bool {
    (0..3).all(|a| i64::from(p[a]) >= -4096 && i64::from(p[a]) <= i64::from(size[a]) + 4096)
}
fn save_cell(b: &mut Blob, (id, part, cell): Cell) {
    b.usize(id);
    b.usize(part);
    for n in cell {
        b.usize(n);
    }
}
fn load_cell(s: &mut Scan<'_>, units: &[RangeUnit]) -> Option<Cell> {
    let id = s.usize();
    let part = s.usize();
    let cell = std::array::from_fn(|_| s.usize());
    let u = units.get(id)?;
    let body = if part == 0 {
        &u.body
    } else {
        &u.body.segments.get(part - 1)?.body
    };
    let size = [body.vol.nx, body.vol.ny, body.vol.nz].map(usize::from);
    (s.ok() && (0..3).all(|a| cell[a] < size[a])).then_some((id, part, cell))
}
fn save_frame(b: &mut Blob, f: &Frame) {
    for v in [f.base, f.x, f.y, f.z] {
        for n in [v.x, v.y, v.z] {
            b.fx(n);
        }
    }
    b.fx(f.scale);
    for n in f.centre {
        b.fx(n);
    }
}
fn load_frame(s: &mut Scan<'_>, size: [u32; 3]) -> Option<Frame> {
    let mut vector = || phys::V3 {
        x: s.fx(),
        y: s.fx(),
        z: s.fx(),
    };
    let base = vector();
    let x = vector();
    let y = vector();
    let z = vector();
    let scale = s.fx();
    let centre = std::array::from_fn(|_| s.fx());
    let bases = [base.x, base.y, base.z];
    if !s.ok()
        || (0..3).any(|a| {
            bases[a] < Fx::from_int(-4096) || bases[a] > Fx::from_int(size[a] as i32 + 4096)
        })
        || [x, y, z]
            .iter()
            .flat_map(|v| [v.x, v.y, v.z])
            .any(|n| n < Fx::from_int(-2) || n > Fx::from_int(2))
        || scale <= Fx::ZERO
        || scale > Fx::from_int(10000)
        || centre
            .iter()
            .any(|n| *n < Fx::from_int(-1024) || *n > Fx::from_int(1024))
    {
        return None;
    }
    Some(Frame {
        base,
        x,
        y,
        z,
        scale,
        centre,
    })
}
