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
            || at.iter().any(|v| *v < 1)
        {
            return 0;
        }
        let Some(surface) = w.soil_height(at[0] as u32, at[2] as u32) else {
            return 0;
        };
        let depth = surface as i32 - at[1];
        if depth < self.t.minimum_burial {
            return 0;
        }
        if !(at[1] + 1..=surface as i32).any(|y| w.solid(at[0] as u32, y as u32, at[2] as u32)) {
            return 0;
        }
        let radius = (depth * self.t.radius_per_depth).min(self.t.radius);
        let reserved = energy / 1000 * u64::from(self.t.energy_milli); // exact whole joules
        if reserved == 0 {
            return 0;
        }
        self.jobs.push_back(Job {
            at,
            radius,
            tiles: tiles(at, radius, self.t.side),
            cursor: 0,
            energy: reserved,
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
        }
    }
    pub fn load(&mut self, s: &mut Scan, size: [u32; 3]) -> Option<()> {
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
            if (0..3).any(|a| at[a] < 1 || at[a] >= size[a] as i32 - 1)
                || radius < 1
                || radius > self.t.radius
                || energy > u64::from(u32::MAX) * 1000
            {
                return None;
            }
            let tiles = tiles(at, radius, self.t.side);
            if cursor > tiles.len() {
                return None;
            }
            self.jobs.push_back(Job {
                at,
                radius,
                tiles,
                cursor,
                energy,
            });
        }
        s.ok().then_some(())
    }
}
impl BlastRange {
    pub(super) fn step_upheaval(&mut self) {
        let t = self.upheaval.t;
        let mut checks = t.checks;
        let mut pieces = t.pieces;
        let turns = self.upheaval.jobs.len();
        for _ in 0..turns {
            let Some(mut j) = self.upheaval.jobs.pop_front() else {
                break;
            };
            while j.cursor < j.tiles.len()
                && checks >= (t.side * t.side) as usize * (t.layers as usize + 1)
                && pieces > 0
                && self.bodies.list.len() < self.bodies.max_bodies
            {
                let p = j.tiles[j.cursor];
                j.cursor += 1;
                let dx = p[0] + t.side / 2 - j.at[0];
                let dz = p[1] + t.side / 2 - j.at[2];
                let speed = t.edge_speed
                    + (t.speed - t.edge_speed) * (j.radius * j.radius - dx * dx - dz * dz).max(0)
                        / (j.radius * j.radius);
                let mut cells = Vec::new();
                let mut cost = 0u64;
                for z in p[1]..p[1] + t.side {
                    for x in p[0]..p[0] + t.side {
                        if x < 1
                            || z < 1
                            || x >= self.size[0] as i32 - 1
                            || z >= self.size[2] as i32 - 1
                        {
                            continue;
                        }
                        checks -= 1;
                        let top = self
                            .world
                            .soil_height(x as u32, z as u32)
                            .unwrap_or(0)
                            .min(self.world.column_ceiling(x as u32, z as u32));
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
