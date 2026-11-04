//! Conservative cellular water with pressure, sleeping cells and bounded work.
//! Water is separate from terrain: it is never a wall, a support or mined rock.
use fx::{
    blob::{Blob, Scan},
    Fx,
};
use modding::{
    water::{Tuning, MATERIAL},
    Registry,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use voxel::{bodies::Body, Edit, World};
#[path = "range_water_erosion.rs"]
mod erosion;
#[path = "range_water_moisture.rs"]
mod moisture;
#[path = "range_water_motion.rs"]
mod motion;
use motion::Fall;
pub type Pos = [u32; 3];
pub const MESH_SIDE: u32 = 8;
const DIRS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [-1, 0, 0],
    [1, 0, 0],
    [0, 0, -1],
    [0, 0, 1],
    [0, 1, 0],
];
use super::membership::Positions;
use super::obstacles::Index as ObstacleIndex;
pub use super::obstacles::Obstacle;
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub units: u64,
    pub absorbed_units: u64,
    pub vapor_units: u64,
    pub initial_units: u64,
    pub cells: usize,
    pub active: usize,
    pub transferred: u64,
    pub checks: usize,
    pub transfers: usize,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Flux {
    pub from: Pos,
    pub to: Pos,
    pub units: u32,
}
pub struct Water {
    pub tuning: Tuning,
    // x,z,y makes a column an ordered range without another occupancy map.
    cells: BTreeMap<Pos, u32>,
    queue: VecDeque<Pos>,
    queued: Positions,
    contact_tick: u64,
    pub(super) flux: Vec<Flux>,
    pressure_active: bool,
    wet: BTreeMap<Pos, (u16, u32)>,
    wet_cursor: Option<Pos>,
    thermal_cursor: Option<Pos>,
    absorbed: u64,
    vapor: u64,
    pub wet_changed: Vec<Pos>,
    /// Bounded, presentation-only evaporation contacts for the current tick.
    pub vapor_events: Vec<(Pos, u32)>,
    /// Sub-unit thermal energy, carried with liquid and saved when nonzero.
    thermal_remainder: BTreeMap<Pos, u32>,
    initial: u64,
    total: u64,
    transferred: u64,
    pub checks: usize,
    pub transfers: usize,
    // Derived render metadata, omitted from saves and authoritative hashes.
    chunks: BTreeMap<Pos, (usize, u64)>,
    revision: u64,
    falling: BTreeMap<Pos, Fall>,
    fall_cursor: Option<Pos>,
    gravity: Fx,
}
impl Water {
    pub fn new() -> Self {
        Self {
            tuning: Tuning::stock(),
            cells: BTreeMap::new(),
            queue: VecDeque::new(),
            queued: Positions::default(),
            contact_tick: 0,
            flux: Vec::new(),
            pressure_active: true,
            wet: BTreeMap::new(),
            wet_cursor: None,
            thermal_cursor: None,
            absorbed: 0,
            vapor: 0,
            wet_changed: Vec::new(),
            vapor_events: Vec::new(),
            thermal_remainder: BTreeMap::new(),
            initial: 0,
            total: 0,
            transferred: 0,
            checks: 0,
            transfers: 0,
            chunks: BTreeMap::new(),
            revision: 0,
            falling: BTreeMap::new(),
            fall_cursor: None,
            gravity: crate::gravity_per_tick(
                &crate::Tuning::from_ron(include_str!("../../../data/sim.ron"))
                    .expect("water gravity"),
            ),
        }
    }
    pub fn amount(&self, p: Pos) -> u32 {
        self.cells.get(&key(p)).copied().unwrap_or(0)
    }
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.wet.is_empty()
    }
    pub(crate) fn has_free_water(&self) -> bool {
        !self.cells.is_empty()
    }
    pub(crate) fn has_water_in_x_span(&self, lo: i32, hi: i32) -> bool {
        if self.cells.is_empty() || hi < 0 || lo > hi {
            return false;
        }
        let a = lo.max(0) as u32;
        let b = hi.max(0) as u32;
        self.cells
            .range([a, 0, 0]..=[b, u32::MAX, u32::MAX])
            .next()
            .is_some()
    }
    pub fn stats(&self) -> Stats {
        Stats {
            units: self.total + self.absorbed + self.vapor,
            absorbed_units: self.absorbed,
            vapor_units: self.vapor,
            initial_units: self.initial,
            cells: self.cells.len(),
            active: self.queue.len(),
            transferred: self.transferred,
            checks: self.checks,
            transfers: self.transfers,
        }
    }
    pub fn cells(&self) -> impl Iterator<Item = (Pos, u32)> + '_ {
        self.cells.iter().map(|(&p, &v)| (key(p), v))
    }
    pub fn chunks(&self) -> impl Iterator<Item = (Pos, u64)> + '_ {
        self.chunks.iter().map(|(&p, &(_, version))| (p, version))
    }
    fn set(&mut self, p: Pos, amount: u32) {
        let old = self.amount(p);
        if old == amount {
            return;
        }
        self.total = self.total - u64::from(old) + u64::from(amount);
        if amount == 0 {
            self.cells.remove(&key(p));
            self.falling.remove(&p);
            self.thermal_remainder.remove(&p);
        } else {
            self.cells.insert(key(p), amount);
        }
        // Compression below the visible surface changes no mesh. Preserve
        // exact mass without rebuilding full internal water chunks on screen.
        if old >= self.tuning.units_per_voxel && amount >= self.tuning.units_per_voxel {
            return;
        }
        self.revision = self.revision.wrapping_add(1);
        let chunk = p.map(|v| v / MESH_SIDE);
        if old == 0 {
            self.chunks.entry(chunk).or_default().0 += 1;
        }
        if amount == 0 {
            let entry = self.chunks.get_mut(&chunk).expect("water chunk");
            entry.0 -= 1;
            if entry.0 == 0 {
                self.chunks.remove(&chunk);
            }
        }
        // Adjacent surface faces can change even when their cell did not.
        for d in [
            [0, 0, 0],
            [-1, 0, 0],
            [1, 0, 0],
            [0, -1, 0],
            [0, 1, 0],
            [0, 0, -1],
            [0, 0, 1],
        ] {
            let q = std::array::from_fn(|a| (p[a] as i32 + d[a]).max(0) as u32 / MESH_SIDE);
            if let Some(entry) = self.chunks.get_mut(&q) {
                entry.1 = self.revision;
            }
        }
    }
    fn wake(&mut self, p: Pos) {
        if !self.queued.contains(&p) && self.amount(p) > 0 {
            self.queued.insert(p);
            self.queue.push_back(p);
            self.pressure_active = true;
        }
    }
    fn wake_near(&mut self, p: Pos, size: Pos) {
        self.wake(p);
        for d in DIRS {
            if let Some(q) = neighbour(p, d, size) {
                self.wake(q);
            }
        }
    }
    pub fn edit_volume(&mut self, p: Pos, add: bool) -> bool {
        let old = self.amount(p);
        let value = if add { self.tuning.units_per_voxel } else { 0 };
        if (add && old >= value)
            || old == value
            || (old == 0 && self.cells.len() >= self.tuning.max_cells)
        {
            return false;
        }
        self.set(p, value);
        if value > 0 {
            self.falling.insert(p, Fall::at(self.contact_tick));
        }
        self.initial = self.initial - u64::from(old) + u64::from(value);
        if self.falling.is_empty() {
            self.fall_cursor = None;
        }
        self.wake(p);
        true
    }
    pub fn seed(&mut self, w: &World, layout: Option<&modding::range::RangeLayout>) {
        let Some(l) = layout else {
            return;
        };
        for b in &l.blocks {
            if b.material != MATERIAL {
                continue;
            }
            for z in b.min[2] as u32..b.max[2] as u32 {
                for y in b.min[1] as u32..b.max[1] as u32 {
                    for x in b.min[0] as u32..b.max[0] as u32 {
                        let p = [x, y, z];
                        if w.on_frame(x, y, z)
                            || w.solid(x, y, z)
                            || self.amount(p) > 0
                            || l.blocks
                                .iter()
                                .rev()
                                .find(|b| b.contains(p.map(|v| v as i32)))
                                .is_none_or(|b| b.material != MATERIAL)
                        {
                            continue;
                        }
                        assert!(
                            self.cells.len() < self.tuning.max_cells,
                            "authored water exceeds cell budget"
                        );
                        self.set(p, self.tuning.units_per_voxel);
                        if y > 1 && self.amount([x, y - 1, z]) == 0 && !w.solid(x, y - 1, z) {
                            self.falling.insert(p, Fall::at(self.contact_tick));
                        }
                        self.initial += u64::from(self.tuning.units_per_voxel);
                        self.wake(p);
                    }
                }
            }
        }
        // Interior of a filled volume has no free neighbouring cell and no
        // solid contact. It cannot flow or absorb until a neighbour changes;
        // wake_near will then enqueue it. Starting with the boundary avoids
        // spending the first seconds scanning thousands of inert cells.
        let size = [w.nx, w.ny, w.nz];
        let queued = std::mem::take(&mut self.queue);
        self.queued.clear();
        for p in queued {
            if DIRS.iter().all(|&d| {
                neighbour(p, d, size).is_some_and(|q| self.amount(q) >= self.tuning.units_per_voxel)
            }) {
                continue;
            }
            self.queued.insert(p);
            self.queue.push_back(p);
        }
    }
    pub fn observe(&mut self, edits: &[Edit], size: Pos) {
        if self.is_empty() {
            return;
        }
        for e in edits {
            self.wake_near([e.x, e.y, e.z], size);
            let above = [e.x, e.y + 1, e.z];
            if e.mat == 0 && inside(above, size) && self.amount(above) > 0 {
                self.falling.insert(above, Fall::at(self.contact_tick));
            }
        }
    }
    fn blocked(w: &World, p: Pos, obstacles: &ObstacleIndex) -> bool {
        w.solid(p[0], p[1], p[2]) || obstacles.contains(p)
    }
    fn transfer(&mut self, from: Pos, to: Pos, request: u32, size: Pos) -> u32 {
        if from == to || self.transfers >= self.tuning.transfers_per_tick {
            return 0;
        }
        let dst = self.amount(to);
        if dst == 0 && self.cells.len() >= self.tuning.max_cells {
            return 0;
        }
        let amount = request
            .min(self.amount(from))
            .min(self.tuning.max_cell_units - dst);
        if amount == 0 {
            return 0;
        }
        let thermal = self.take_moving_heat(from, amount);
        self.set(from, self.amount(from) - amount);
        self.set(to, dst + amount);
        if thermal > 0 {
            self.absorb_heat(to, thermal, size);
        }
        self.transferred += u64::from(amount);
        self.transfers += 1;
        // Pressure projection and obstacle ejection can jump many cells:
        // these are volume redistribution, not local current velocity.
        if (0..3).map(|a| from[a].abs_diff(to[a])).sum::<u32>() == 1 && to[1] <= from[1] {
            self.flux.push(Flux {
                from,
                to,
                units: amount,
            });
        }
        self.wake_near(from, size);
        self.wake_near(to, size);
        amount
    }
    fn rate(&self, n: u32) -> u32 {
        let n = n.min(self.tuning.max_flow);
        if n == 0 {
            0
        } else {
            (n * (1000 - self.tuning.viscosity_milli) / 1000).max(1)
        }
    }
    fn stable_bottom(&self, total: u32) -> u32 {
        let c = self.tuning.units_per_voxel;
        let k = self.tuning.compression;
        if total <= c {
            total
        } else if total < 2 * c + k {
            ((u64::from(c) * u64::from(c) + u64::from(total) * u64::from(k)) / u64::from(c + k))
                as u32
        } else {
            (total + k) / 2
        }
    }
    /// Conservative plug-flow projection within a bounded connected wet region.
    /// Intermediate cells exchange the same volume; walls and air gaps never
    /// connect endpoints. Large pools relax in overlapping regions from FIFO.
    fn project_pressure(
        &mut self,
        w: &World,
        obstacles: &ObstacleIndex,
        size: Pos,
        airborne: &Positions,
    ) {
        let Some(start) = self
            .queue
            .iter()
            .take(self.tuning.pressure_checks)
            .copied()
            .find(|p| !airborne.contains(p) && self.amount(*p) > 0)
            .or_else(|| {
                self.cells
                    .iter()
                    .take(self.tuning.pressure_checks)
                    .map(|(&p, _)| key(p))
                    .find(|p| !airborne.contains(p))
            })
        else {
            return;
        };
        let mut seen = Positions::default();
        seen.insert(start);
        let mut todo = VecDeque::from([start]);
        let mut surfaces = BTreeSet::new();
        let c = self.tuning.units_per_voxel;
        let mut work = 0;
        while let Some(p) = todo.pop_front() {
            if work >= self.tuning.pressure_checks {
                break;
            }
            work += 1;
            if airborne.contains(&p) || Self::blocked(w, p, obstacles) || self.amount(p) == 0 {
                continue;
            }
            if let Some(up) = neighbour(p, [0, 1, 0], size) {
                if self.amount(up) < self.tuning.surface_tension && !Self::blocked(w, up, obstacles)
                {
                    let supported = neighbour(p, [0, -1, 0], size).is_some_and(|q| {
                        Self::blocked(w, q, obstacles)
                            || self.amount(q) >= c * self.tuning.pressure_fill_milli / 1000
                    });
                    if supported {
                        surfaces.insert((p[1] * c + self.amount(p).min(c), linear(p, size), p));
                    }
                }
            }
            for d in DIRS {
                if seen.len() >= self.tuning.pressure_checks {
                    break;
                }
                if let Some(q) = neighbour(p, d, size) {
                    if self.amount(q) > 0 && !Self::blocked(w, q, obstacles) && seen.insert(q) {
                        todo.push_back(q);
                    }
                }
            }
        }
        let mut projected = false;
        for _ in 0..self.tuning.pressure_packets {
            if self.transfers >= self.tuning.transfers_per_tick || surfaces.len() < 2 {
                break;
            }
            let low = *surfaces.first().unwrap();
            let high = *surfaces.last().unwrap();
            if high.0 <= low.0 + self.tuning.surface_tension {
                break;
            }
            surfaces.remove(&low);
            surfaces.remove(&high);
            let target = if self.amount(low.2) >= c {
                let Some(q) = neighbour(low.2, [0, 1, 0], size) else {
                    continue;
                };
                q
            } else {
                low.2
            };
            let n = ((high.0 - low.0) / 2)
                .min(c.saturating_sub(self.amount(target)))
                .min(self.tuning.max_flow);
            if self.transfer(high.2, target, n, size) == 0 {
                continue;
            }
            projected = true;
            if self.amount(high.2) > 0 {
                surfaces.insert((high.2[1] * c + self.amount(high.2).min(c), high.1, high.2));
            }
            surfaces.insert((
                target[1] * c + self.amount(target).min(c),
                linear(target, size),
                target,
            ));
        }
        if !projected && seen.len() == self.cells.len() {
            self.pressure_active = false;
        }
    }
    fn wake_obstacles(&mut self, size: Pos, obstacles: &[Obstacle]) {
        if obstacles.is_empty() {
            return;
        }
        let count = obstacles.len().min(self.tuning.obstacle_checks);
        let first = (self.contact_tick as usize) % obstacles.len();
        let mut left = self.tuning.obstacle_checks;
        for i in 0..count {
            let obstacle = obstacles[(first + i) % obstacles.len()];
            let lo: Pos =
                std::array::from_fn(|a| obstacle.min[a].clamp(1, i64::from(size[a]) - 1) as u32);
            let hi: Pos =
                std::array::from_fn(|a| obstacle.max[a].clamp(1, i64::from(size[a]) - 1) as u32);
            if (0..3).any(|a| lo[a] >= hi[a]) {
                continue;
            }
            let extent: Pos = std::array::from_fn(|a| hi[a] - lo[a]);
            let volume = extent.iter().map(|&v| u64::from(v)).product::<u64>();
            let share = (left / (count - i)).max(1).min(volume as usize);
            left -= share;
            let start = self.contact_tick.wrapping_mul(share as u64) % volume;
            for j in 0..share {
                let index = (start + j as u64) % volume;
                let p = [
                    lo[0] + (index % u64::from(extent[0])) as u32,
                    lo[1] + (index / u64::from(extent[0]) % u64::from(extent[1])) as u32,
                    lo[2] + (index / u64::from(extent[0]) / u64::from(extent[1])) as u32,
                ];
                self.wake(p);
            }
        }
    }
    pub fn step(&mut self, w: &World, obstacles: &[Obstacle]) {
        let index = ObstacleIndex::new(obstacles.to_vec());
        self.step_indexed(w, obstacles, &index);
    }
    pub(super) fn step_indexed(
        &mut self,
        w: &World,
        obstacles: &[Obstacle],
        index: &ObstacleIndex,
    ) {
        self.flux.clear();
        self.checks = 0;
        self.transfers = 0;
        self.wet_changed.clear();
        self.dry(w);
        let size = [w.nx, w.ny, w.nz];
        self.wake_obstacles(size, obstacles);
        let obstacles = index;
        let turns = self.queue.len().min(self.tuning.checks_per_tick);
        let project = self.pressure_active
            && self.contact_tick % u64::from(self.tuning.pressure_interval) == 0;
        let pressure_reserve = if project {
            self.tuning
                .pressure_packets
                .min(self.tuning.transfers_per_tick / 4)
        } else {
            0
        };
        let transport_limit = self.tuning.transfers_per_tick - pressure_reserve;
        self.contact_tick = self.contact_tick.wrapping_add(1);
        let mut airborne = Positions::default();
        let mut supported = Positions::default();
        let mut motion_checks = self.tuning.motion.checks_per_tick;
        self.advance_falling(
            w,
            obstacles,
            &mut motion_checks,
            &mut airborne,
            &mut supported,
        );
        for _ in 0..turns {
            if self.transfers >= transport_limit {
                break;
            }
            let p = self.queue.pop_front().expect("water work");
            self.queued.remove(&p);
            if self.amount(p) == 0 {
                continue;
            }
            if airborne.contains(&p) || self.checks >= self.tuning.checks_per_tick {
                self.wake(p);
                continue;
            }
            self.checks += 1;
            self.absorb(w, p, size);
            if self.amount(p) == 0 {
                continue;
            }
            if Self::blocked(w, p, obstacles) {
                // Only occluded water gets this bounded displacement search;
                // ordinary flow never crosses a wall. No water is deleted.
                for distance in 1..=self.tuning.displacement_reach {
                    for d in [
                        [0, 1, 0],
                        [-1, 0, 0],
                        [1, 0, 0],
                        [0, 0, -1],
                        [0, 0, 1],
                        [0, -1, 0],
                    ] {
                        let Some(q) = neighbour(p, d.map(|v| v * distance as i32), size) else {
                            continue;
                        };
                        if !Self::blocked(w, q, obstacles) {
                            let n = self
                                .tuning
                                .max_cell_units
                                .saturating_sub(self.amount(q))
                                .min(self.tuning.max_flow);
                            self.transfer(p, q, n, size);
                            if self.amount(p) == 0 {
                                break;
                            }
                        }
                    }
                    if self.amount(p) == 0 {
                        break;
                    }
                }
                continue;
            }
            if self.fall_column(
                w,
                p,
                obstacles,
                &mut motion_checks,
                &mut airborne,
                &mut supported,
            ) {
                continue;
            }
            if let Some(q) =
                neighbour(p, [0, -1, 0], size).filter(|&q| !Self::blocked(w, q, obstacles))
            {
                let total = self.amount(p) + self.amount(q);
                let n = self.rate(self.stable_bottom(total).saturating_sub(self.amount(q)));
                let n = if self.amount(q) >= self.tuning.units_per_voxel
                    && n < self.tuning.surface_tension
                {
                    0
                } else {
                    n
                };
                self.transfer(p, q, n, size);
            }
            // An emptied source cannot send sideways/upward. Its neighbours
            // have already been woken by the conservative downward transfer.
            if self.amount(p) == 0 {
                continue;
            }
            // Rotate the stable neighbour order deterministically so repeated
            // visits do not always drain toward the same world direction.
            let mut sides = [None; 4];
            let mut count = 0;
            for &d in &DIRS[1..5] {
                if let Some(q) = neighbour(p, d, size).filter(|&q| !Self::blocked(w, q, obstacles))
                {
                    sides[count] = Some(q);
                    count += 1;
                }
            }
            sides[..count].sort_unstable_by_key(|q| linear(q.unwrap(), size));
            if count > 1 {
                let first = (self.contact_tick as usize + linear(p, size) as usize) % count;
                sides[..count].rotate_left(first);
            }
            for q in sides[..count].iter().flatten().copied() {
                // A wet vertical column transmits pressure without requiring
                // every interior cell to be processed first. Move a full
                // packet from its connected surface to a neighbouring lower
                // column. The path is checked cell by cell, so this cannot
                // cross a solid wall or an air gap. Work per candidate is
                // bounded, and transfer() preserves volume and heat.
                let capacity = self.tuning.units_per_voxel;
                if self.amount(p) >= capacity
                    && self.amount(q) < capacity
                    && neighbour(q, [0, -1, 0], size).is_some_and(|below| {
                        Self::blocked(w, below, obstacles) || self.amount(below) >= capacity
                    })
                {
                    let mut top = p;
                    for _ in 0..12 {
                        let Some(up) = neighbour(top, [0, 1, 0], size) else {
                            break;
                        };
                        // A partially filled top cell still adds hydrostatic
                        // head. Requiring a full cell stalled shallow streams
                        // just as their source column began to drain.
                        let volume = self.amount(up);
                        if volume == 0 || airborne.contains(&up) || Self::blocked(w, up, obstacles)
                        {
                            break;
                        }
                        top = up;
                        // A partial cell is the surface, not a connection to
                        // another cell above its unfilled portion.
                        if volume < capacity {
                            break;
                        }
                    }
                    let packet = self.rate(capacity - self.amount(q));
                    if top != p && self.transfer(top, q, packet, size) > 0 {
                        continue;
                    }
                }
                let diff = self.amount(p).saturating_sub(self.amount(q));
                let amount = if diff > self.tuning.surface_tension {
                    // Equalise a neighbouring pair in one visit. The old
                    // quarter-difference compounded the viscosity loss and
                    // made water spread like a much thicker fluid. Capping at
                    // half keeps each pair from overshooting or reversing.
                    diff / 2
                } else {
                    0
                };
                let pressure = 0;
                let n = self.rate(amount.max(pressure));
                if n >= self.tuning.surface_tension {
                    self.transfer(p, q, n, size);
                }
            }
            if let Some(q) =
                neighbour(p, [0, 1, 0], size).filter(|&q| !Self::blocked(w, q, obstacles))
            {
                let total = self.amount(p) + self.amount(q);
                let compression = self.amount(p).saturating_sub(self.stable_bottom(total));
                let pressure = 0;
                let n = self.rate(compression.max(pressure));
                if n >= self.tuning.surface_tension {
                    self.transfer(p, q, n, size);
                }
            }
        }
        if project {
            self.project_pressure(w, obstacles, size, &airborne);
        }
        if self.falling.is_empty() {
            self.fall_cursor = None;
        }
    }
    /// Ambient displaced volume at four sides of a cohesive body. A light
    /// body remains dynamic on water; draining the water releases it again.
    pub fn support_body(&self, b: &mut Body, w: &World, mats: &Registry) -> bool {
        if self.is_empty() || b.voxels.is_empty() {
            return false;
        }
        let density = b
            .voxels
            .first()
            .and_then(|v| mats.get(v.mat))
            .map_or(u32::MAX, |m| m.density);
        let water_density = mats.get(MATERIAL).map_or(0, |m| m.density);
        let h = u32::from(b.height()).min(self.tuning.body_depth);
        if h == 0 {
            return false;
        }
        let count = 4.min(self.tuning.body_samples);
        let rings = (self.tuning.body_samples / count).max(1);
        let mut columns = [0u64; 4];
        // Sample outside the immediate displacement wake as well as at the
        // hull. A body must not lose buoyancy because its own conservative
        // displacement briefly emptied the first ring of fluid cells.
        for gap in 1..=rings {
            let gap = gap as i64;
            let points = [
                [b.ox - gap, b.oz + i64::from(b.sz) / 2],
                [b.ox + i64::from(b.sx) + gap - 1, b.oz + i64::from(b.sz) / 2],
                [b.ox + i64::from(b.sx) / 2, b.oz - gap],
                [b.ox + i64::from(b.sx) / 2, b.oz + i64::from(b.sz) + gap - 1],
            ];
            for (i, p) in points.into_iter().take(count).enumerate() {
                if p.iter().any(|&v| v < 0) || b.oy < 0 {
                    continue;
                }
                let mut volume = 0;
                for y in 0..h {
                    volume += u64::from(
                        self.amount([p[0] as u32, (b.oy + y as i64) as u32, p[1] as u32])
                            .min(self.tuning.units_per_voxel),
                    );
                }
                columns[i] = columns[i].max(volume);
            }
        }
        let volume = columns.iter().sum::<u64>();
        if volume == 0 {
            return false;
        }
        b.vy =
            b.vy.mul(Fx::from_ratio((1000 - self.tuning.drag_milli) as i32, 1000));
        let weight = u64::from(density)
            * u64::from(h)
            * count as u64
            * u64::from(self.tuning.units_per_voxel);
        let buoyancy = u64::from(water_density) * volume;
        if density < water_density
            && b.voxels.len() <= self.tuning.buoyancy_cells
            && buoyancy >= weight
        {
            if buoyancy
                > weight
                    + u64::from(water_density)
                        * count as u64
                        * u64::from(self.tuning.units_per_voxel)
                && b.can_rise(w)
            {
                b.oy += 1;
                b.fallen = b.fallen.saturating_sub(1);
            }
            b.vy = Fx::ZERO;
            b.fy = Fx::ZERO;
            return true;
        }
        false
    }
    pub fn has_state(&self) -> bool {
        self.initial != 0
    }
    pub fn save_legacy(&self, b: &mut Blob) {
        b.u64(self.initial);
        b.u64(self.transferred);
        b.u64(self.contact_tick);
        b.bool(self.pressure_active);
        self.save_moisture(b);
        b.u32(self.cells.len() as u32);
        for (&p, &v) in &self.cells {
            for x in p {
                b.u32(x);
            }
            b.u32(v);
        }
        b.u32(self.queue.len() as u32);
        for p in &self.queue {
            for &x in p {
                b.u32(x);
            }
        }
    }
    pub fn load_legacy(&mut self, s: &mut Scan, size: Pos) -> Option<()> {
        *self = Self::new();
        self.initial = s.u64();
        self.transferred = s.u64();
        self.contact_tick = s.u64();
        self.pressure_active = s.bool();
        self.load_moisture(s, size)?;
        let count = s.u32() as usize;
        if count > self.tuning.max_cells || count > s.left() / 16 {
            return None;
        }
        let mut last = None;
        let mut total = 0u64;
        for _ in 0..count {
            let k = std::array::from_fn(|_| s.u32());
            let p = key(k);
            let v = s.u32();
            if !inside(p, size)
                || v == 0
                || v > self.tuning.max_cell_units
                || last.is_some_and(|old| old >= k)
            {
                return None;
            }
            self.set(p, v);
            total += u64::from(v);
            last = Some(k);
        }
        if total.checked_add(self.absorbed)?.checked_add(self.vapor)? != self.initial
            || self.initial == 0
        {
            return None;
        }
        let count = s.u32() as usize;
        if count > self.tuning.max_cells * 2 || count > s.left() / 12 {
            return None;
        }
        for _ in 0..count {
            let p = std::array::from_fn(|_| s.u32());
            if !inside(p, size) || !self.queued.insert(p) {
                return None;
            }
            self.queue.push_back(p);
        }
        s.ok().then_some(())
    }
}
fn key(p: Pos) -> Pos {
    [p[0], p[2], p[1]]
}
fn inside(p: Pos, size: Pos) -> bool {
    (0..3).all(|a| p[a] > 0 && p[a] + 1 < size[a])
}
fn neighbour(p: Pos, d: [i32; 3], size: Pos) -> Option<Pos> {
    let q = std::array::from_fn(|a| p[a] as i64 + i64::from(d[a]));
    if (0..3).any(|a| q[a] < 1 || q[a] + 1 >= i64::from(size[a])) {
        None
    } else {
        Some(q.map(|v| v as u32))
    }
}
fn linear(p: Pos, size: Pos) -> u64 {
    (u64::from(p[2]) * u64::from(size[1]) + u64::from(p[1])) * u64::from(size[0]) + u64::from(p[0])
}

#[cfg(test)]
mod pressure_regressions {
    use super::*;
    struct Floor;
    impl voxel::Generator for Floor {
        fn material(&self, _x: u32, y: u32, _z: u32) -> u16 {
            if y < 4 {
                6
            } else {
                0
            }
        }
    }
    #[test]
    fn transfer_to_the_same_cell_does_not_create_water() {
        let mut water = Water::new();
        let p = [10, 10, 10];
        assert!(water.edit_volume(p, true));
        let before = water.stats();
        let amount = water.amount(p);
        assert_eq!(water.transfer(p, p, amount / 2, [64; 3]), 0);
        assert_eq!(water.amount(p), amount);
        assert_eq!(water.stats().units, before.units);
        assert_eq!(water.stats().transfers, before.transfers);
    }
    #[test]
    fn partially_filled_column_drives_a_stronger_outflow_without_losing_volume() {
        let mut world = World::new(64, 64, 64, 65536, Box::new(Floor));
        for p in [[29, 4, 30], [30, 4, 29], [30, 4, 31]] {
            world.queue_transfer(Edit::put(p[0], p[1], p[2], 2));
        }
        world.flush_transfers();
        let mut water = Water::new();
        let bottom = [30, 4, 30];
        let top = [30, 5, 30];
        let outlet = [31, 4, 30];
        assert!(water.edit_volume(bottom, true));
        assert!(water.edit_volume(top, true));
        let cap = water.tuning.units_per_voxel;
        water.set(top, cap * 3 / 4);
        water.initial -= u64::from(cap / 4);
        let before = water.stats().units;
        water.step(&world, &[]);
        assert_eq!(water.stats().units, before);
        assert!(
            water.amount(outlet) >= cap * 3 / 4,
            "partial upper water did not drive the outflow"
        );
        assert!(
            water.amount(bottom) >= cap * 3 / 4,
            "the source column drained unrealistically"
        );
        assert!(
            water.checks <= water.tuning.checks_per_tick
                && water.transfers <= water.tuning.transfers_per_tick
        );
    }
}
