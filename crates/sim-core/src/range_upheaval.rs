//! Finite-energy underground heave. Whole real patches rise and fall again.
use super::{BlastRange, ExplosionConfig};
use fx::{
    blob::{Blob, Scan},
    Fx,
};
use modding::upheaval::Tuning;
use std::collections::VecDeque;
struct Job {
    at: [i32; 3],
    radius: i32,
    tiles: Vec<[i32; 2]>,
    cursor: usize,
    energy: u64,
    speed: i32,
}
// Scale length by the cube root of yield; keep the arithmetic deterministic.
fn yield_scale(energy: u32) -> i32 {
    let target = u128::from(energy) * 1_000_000_000 / 3500;
    let (mut lo, mut hi) = (0u128, 10_000u128);
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if mid * mid * mid <= target {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo as i32
}
fn tile_side(radius: i32, t: Tuning) -> i32 {
    if radius > 32 {
        (t.side * 2).min(16)
    } else {
        t.side
    }
}
// Use the live column when its surface is terrain, including laboratory
// additions and craters; an unrelated building above the ground should not
// become a stack of soil to lift.
fn ground_surface(w: &voxel::World, x: u32, z: u32) -> Option<u32> {
    let ceiling = w.column_ceiling(x, z);
    if ceiling == 0 {
        return None;
    }
    if matches!(w.material(x, ceiling - 1, z), 1 | 6 | 7 | 8 | 10) {
        Some(ceiling)
    } else {
        w.soil_height(x, z).map(|h| h.min(ceiling))
    }
}
pub(super) struct Upheaval {
    t: Tuning,
    jobs: VecDeque<Job>,
    pub raised: u64,
}
fn tiles(at: [i32; 3], radius: i32, side: i32) -> Vec<[i32; 2]> {
    let n = radius.div_euclid(side) + 1;
    let mut out = Vec::new();
    for z in -n..n {
        for x in -n..n {
            let dx = x * side + side / 2;
            let dz = z * side + side / 2;
            if dx * dx + dz * dz <= radius * radius {
                out.push([at[0] + x * side, at[2] + z * side]);
            }
        }
    }
    out.sort_by_key(|p| {
        let dx = p[0] + side / 2 - at[0];
        let dz = p[1] + side / 2 - at[2];
        (dx * dx + dz * dz, p[1], p[0])
    });
    out
}
impl Upheaval {
    pub fn new() -> Self {
        Self {
            t: Tuning::stock(),
            jobs: VecDeque::new(),
            raised: 0,
        }
    }
    pub fn reserve(
        &mut self,
        w: &voxel::World,
        at: [i32; 3],
        config: &ExplosionConfig,
        energy: u64,
    ) -> u64 {
        if self.jobs.len() >= self.t.jobs
            || config.delivery != modding::explosion::Delivery::Blast
            || (0..3).any(|a| at[a] < 1 || at[a] >= [w.nx, w.ny, w.nz][a] as i32 - 1)
        {
            return 0;
        }
        let Some(surface) = ground_surface(w, at[0] as u32, at[2] as u32) else {
            return 0;
        };
        let depth = surface as i32 - at[1];
        if depth < self.t.minimum_burial {
            return 0;
        }
        if !(at[1] + 1..=surface as i32).any(|y| w.solid(at[0] as u32, y as u32, at[2] as u32)) {
            return 0;
        }
        let scale = yield_scale(config.energy);
        let radius =
            ((depth * self.t.radius_per_depth * scale / 1000).max(self.t.side)).min(self.t.radius);
        let speed = self.t.speed * scale.clamp(1000, 3000) / 1000;
        let reserved = energy / 1000 * u64::from(self.t.energy_milli); // exact whole joules
        if reserved == 0 {
            return 0;
        }
        self.jobs.push_back(Job {
            at,
            radius,
            tiles: tiles(at, radius, tile_side(radius, self.t)),
            cursor: 0,
            energy: reserved,
            speed,
        });
        reserved
    }
    pub fn has_state(&self) -> bool {
        self.raised > 0 || !self.jobs.is_empty()
    }
    pub fn save(&self, b: &mut Blob) {
        b.u64(self.raised);
        b.u32(self.jobs.len() as u32);
        for j in &self.jobs {
            for v in j.at {
                b.i32(v);
            }
            b.i32(j.radius);
            b.u32(j.cursor as u32);
            b.u64(j.energy);
            b.i32(j.speed);
        }
    }
    pub fn load(&mut self, s: &mut Scan, size: [u32; 3], scaled: bool) -> Option<()> {
        *self = Self::new();
        self.raised = s.u64();
        let n = s.u32() as usize;
        if n > self.t.jobs || n > s.left() / 28 {
            return None;
        }
        for _ in 0..n {
            let at = std::array::from_fn(|_| s.i32());
            let radius = s.i32();
            let cursor = s.u32() as usize;
            let energy = s.u64();
            let speed = if scaled { s.i32() } else { self.t.speed };
            if (0..3).any(|a| at[a] < 1 || at[a] >= size[a] as i32 - 1)
                || radius < 1
                || radius > self.t.radius
                || energy > u64::from(u32::MAX) * 1000
                || speed < self.t.speed
                || speed > self.t.speed * 3
            {
                return None;
            }
            let tiles = tiles(at, radius, tile_side(radius, self.t));
            if cursor > tiles.len() {
                return None;
            }
            self.jobs.push_back(Job {
                at,
                radius,
                tiles,
                cursor,
                energy,
                speed,
            });
        }
        s.ok().then_some(())
    }
}
impl BlastRange {
    pub(super) fn step_upheaval(&mut self) {
        let t = self.upheaval.t;
        let mut checks = if self.upheaval.jobs.iter().any(|j| j.radius > 32) {
            t.checks.saturating_mul(4)
        } else {
            t.checks
        };
        let mut pieces = t.pieces;
        let turns = self.upheaval.jobs.len();
        for _ in 0..turns {
            let Some(mut j) = self.upheaval.jobs.pop_front() else {
                break;
            };
            let side = tile_side(j.radius, t);
            while j.cursor < j.tiles.len()
                && checks >= (side * side) as usize * (t.layers as usize + 1)
                && pieces > 0
                && self.bodies.list.len() < self.bodies.max_bodies
            {
                let p = j.tiles[j.cursor];
                j.cursor += 1;
                let dx = p[0] + side / 2 - j.at[0];
                let dz = p[1] + side / 2 - j.at[2];
                let speed = t.edge_speed
                    + (j.speed - t.edge_speed) * (j.radius * j.radius - dx * dx - dz * dz).max(0)
                        / (j.radius * j.radius);
                let mut cells = Vec::new();
                let mut cost = 0u64;
                for z in p[1]..p[1] + side {
                    for x in p[0]..p[0] + side {
                        if x < 1
                            || z < 1
                            || x >= self.size[0] as i32 - 1
                            || z >= self.size[2] as i32 - 1
                        {
                            continue;
                        }
                        checks -= 1;
                        let top = ground_surface(&self.world, x as u32, z as u32).unwrap_or(0);
                        let bottom = top.saturating_sub(t.layers).max((j.at[1] + 1) as u32);
                        for y in bottom..top {
                            checks -= 1;
                            let pos = [x as u32, y, z as u32];
                            if self.world.on_frame(pos[0], pos[1], pos[2])
                                || self.world.has_pending_dig(pos[0], pos[1], pos[2])
                                || self.matter.debris.claimed(pos)
                            {
                                continue;
                            }
                            let mat = self.world.material(pos[0], pos[1], pos[2]);
                            if !matches!(mat, 1 | 6 | 7 | 8 | 10) {
                                continue;
                            }
                            let Some(m) = self.matter.economy.mats.get(mat) else {
                                continue;
                            };
                            // mass[g] * velocity[mm/s]^2 / 2e6 = kinetic mJ.
                            let mm = u64::from(voxel::unit::WORLD_MM);
                            let mass = u64::from(m.density)
                                * u64::from(self.matter.tuning.density_kg_m3_per_density)
                                * mm.pow(3)
                                / 1_000_000;
                            let kinetic = (u128::from(mass) * u128::from(speed as u64 * mm).pow(2)
                                / 2_000_000) as u64;
                            cost = cost.saturating_add(
                                kinetic
                                    + u64::from(m.strength)
                                        * u64::from(t.fracture_j_per_strength)
                                        * 1000,
                            );
                            cells.push((pos[0], pos[1], pos[2]));
                        }
                    }
                }
                if cells.is_empty() || cost > j.energy {
                    continue;
                }
                if self.bodies.detach(&mut self.world, &cells) {
                    let b = self.bodies.list.last_mut().unwrap();
                    b.vy = Fx::from_ratio(-speed, crate::TICKS_PER_SECOND);
                    self.upheaval.raised += cells.len() as u64;
                    j.energy -= cost;
                    self.matter.spent_mj += cost;
                    // A mobile hull standing on this patch receives the
                    // patch's upward velocity. Pay its kinetic energy from
                    // the same finite reserve; fixed structures stay anchored.
                    for u in &mut self.units {
                        if !u.status.alive || u.kind.emplacement() {
                            continue;
                        }
                        let [ux, uy, uz] = u.motion.pos.map(Fx::to_int_trunc);
                        if ux < p[0] || ux >= p[0] + side || uz < p[1] || uz >= p[1] + side {
                            continue;
                        }
                        let Some(top) = ground_surface(&self.world, ux as u32, uz as u32) else {
                            continue;
                        };
                        if (uy - top as i32).abs() > 3
                            || !cells
                                .iter()
                                .any(|&(x, y, z)| x == ux as u32 && z == uz as u32 && y + 1 == top)
                        {
                            continue;
                        }
                        let old = u.motion.hull.vy.max(Fx::ZERO);
                        let new = Fx::from_int(speed).max(old);
                        if new == old {
                            continue;
                        }
                        let mm_per_unit = u64::from(voxel::unit::WORLD_MM);
                        let old_mm = (old.raw().max(0) as u128 * u128::from(mm_per_unit)) >> 32;
                        let new_mm = (new.raw().max(0) as u128 * u128::from(mm_per_unit)) >> 32;
                        let mass_kg = if u.motion.chassis.mass > 0 {
                            u64::from(u.motion.chassis.mass as u32) * 1000
                        } else {
                            ((u128::from(u.body.volume_mm3())
                                * u.motion.chassis.density.max(1) as u128)
                                / 1_000_000_000)
                                .clamp(1, u128::from(u64::MAX)) as u64
                        };
                        let work = (u128::from(mass_kg) * (new_mm * new_mm - old_mm * old_mm)
                            / 2000)
                            .min(u128::from(u64::MAX)) as u64;
                        if work <= j.energy {
                            u.motion.hull.vy = new;
                            j.energy -= work;
                            self.matter.spent_mj += work;
                        }
                    }
                    pieces -= 1;
                }
            }
            if j.cursor == j.tiles.len() {
                self.matter.remaining_mj = self.matter.remaining_mj.saturating_add(j.energy);
            } else {
                self.upheaval.jobs.push_back(j);
            }
        }
    }
}
