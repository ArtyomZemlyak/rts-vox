//! Material-aware explosion service and a deterministic single-gun range.
//! The renderer only reads this state. Gas consumes impact events and solid
//! geometry; no GPU result is allowed to alter the authoritative world.

use fx::Fx;
#[path = "range_matter.rs"]
pub mod matter;
#[path = "range_motion.rs"]
pub mod motion;
use crate::shield::{inside, Point, Shield};
use modding::explosion::{ExplosionConfig, ProjectileMotion};
use modding::range::{RangeLayout, RangeShieldDef, RangeUnitKind};
use motion::Motion;
use phys::blast::Wave;
use std::collections::BTreeMap;
use voxel::topo::unit::CoreBufs;
use voxel::unit::{Status, UnitBody};
use voxel::{Generator, World};

pub const SIZE: [u32; 3] = [640, 192, 640];
pub const FLOOR: i32 = 16;
pub const GUN: [i32; 3] = [128, FLOOR, 336];
/// Muzzle of the vanilla 100 mm tank template relative to its base centre.
pub const MUZZLE: [i32; 3] = [GUN[0] + 30, GUN[1] + 23, GUN[2]];
pub const TARGET: [i32; 3] = [236, 24, 336];
pub const ENERGY_TARGET: [i32; 3] = [396, 24, 336];
pub const WITNESS: [i32; 3] = [377, FLOOR, 305];

/// A stationary 100 mm body. Damage is a bounded, ordered ablation ledger;
/// the pristine template minus this ledger completely specifies its state.
pub struct RangeUnit {
    pub kind: RangeUnitKind,
    pub motion: Motion,
    pub pose: crate::damage::Frame,
    pub body: UnitBody,
    pub at: [i32; 3],
    pub removed: Vec<[u8; 3]>,
    /// Removed cells transferred to persistent debris, excluded from the sink.
    pub detached: usize,
    /// Local work already spent deforming surviving body cells.
    pub dents: BTreeMap<[u8; 3], u32>,
    pub revision: u64,
    pub status: Status,
    pub mobility: crate::mobility::Mobility,
    pub integrity: crate::integrity::Report,
    original_shape: crate::burst::chunk::Shape,
    integrity_model: crate::integrity::Model,
    connectivity: CoreBufs,
    section: Option<crate::damage::SectionJob>,
}
impl RangeUnit {
    pub fn new(at: [i32; 3]) -> Self {
        Self::of_kind(RangeUnitKind::Tank, at)
    }
    pub fn of_kind(kind: RangeUnitKind, at: [i32; 3]) -> Self {
        let body = match kind {
            RangeUnitKind::Tank => voxel::unit::tank(),
            RangeUnitKind::Strider => voxel::unit::strider(),
        };
        let mut connectivity = CoreBufs::new();
        let integrity_model = crate::integrity::Model::new(&body);
        let (status, integrity) = integrity_model.evaluate(&body, &mut connectivity);
        let motion = Motion::new(at, kind, &body);
        let pose = motion.frame(&body);
        let original_shape = crate::burst::chunk::Shape::from_body(&body);
        Self {
            motion,
            pose,
            kind,
            body,
            at,
            removed: Vec::new(),
            detached: 0,
            dents: BTreeMap::new(),
            revision: 0,
            status,
            mobility: crate::mobility::Mobility::intact(status.engine)
                .with_traction(integrity.drive),
            integrity,
            original_shape,
            integrity_model,
            connectivity,
            section: None,
        }
    }
    fn refresh_damage(&mut self, id: usize) {
        self.update_response();
        if !self.status.alive {
            self.section = None;
        } else if self.status.detached > 0 && self.section.is_none() {
            let mut mask = self.body.vol.clone();
            for (w, r) in mask.cols.iter_mut().zip(self.connectivity.reached_cols()) {
                *w &= !r;
            }
            self.section = Some(crate::damage::SectionJob::new(id as u32, 0, mask));
        }
    }
    fn update_response(&mut self) {
        (self.status, self.integrity) = self
            .integrity_model
            .evaluate(&self.body, &mut self.connectivity);
        let engine = if self.status.alive && self.status.engine {
            self.integrity.engine
        } else {
            Fx::ZERO
        };
        self.mobility = crate::mobility::Mobility::from_shapes(
            &self.original_shape,
            &crate::burst::chunk::Shape::from_body(&self.body),
            [self.body.vol.nx, self.body.vol.ny, self.body.vol.nz],
            engine,
        )
        .with_traction(self.integrity.drive);
    }
    pub fn update_pose(&mut self) {
        self.at = self.motion.pos.map(|v| v.to_int_trunc());
        self.pose = self.motion.frame(&self.body);
    }
    pub fn world_pos(&self, p: [usize; 3]) -> [i32; 3] {
        if self.motion.enabled {
            let v = self
                .pose
                .world(p.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2))));
            return [v.x, v.y, v.z].map(|v| v.to_int_trunc());
        }
        [
            self.at[0] + p[0] as i32 - self.body.vol.nx as i32 / 2,
            self.at[1] + p[1] as i32,
            self.at[2] + p[2] as i32 - self.body.vol.nz as i32 / 2,
        ]
    }
    pub fn local_at(&self, p: [i32; 3], pose: &crate::damage::Frame) -> Option<[usize; 3]> {
        if (p[0] - pose.base.x.to_int_trunc()).abs() > 80
            || (p[2] - pose.base.z.to_int_trunc()).abs() > 80
        {
            return None;
        }
        let v = p.map(|v| Fx::from_int(v).add(Fx::from_ratio(1, 2)));
        let q = pose
            .local(phys::V3::new(v[0], v[1], v[2]))
            .map(|v| (v.raw() >> 32) as i32);
        let size = [self.body.vol.nx, self.body.vol.ny, self.body.vol.nz];
        (0..3)
            .all(|a| q[a] >= 0 && q[a] < size[a] as i32)
            .then_some(q.map(|v| v as usize))
    }
    pub fn local(&self, p: [i32; 3]) -> Option<[usize; 3]> {
        if self.motion.enabled {
            return self.local_at(p, &self.pose);
        }
        let origin = self.world_pos([0; 3]);
        let q: [i32; 3] = std::array::from_fn(|a| p[a] - origin[a]);
        let size = [self.body.vol.nx, self.body.vol.ny, self.body.vol.nz];
        (0..3)
            .all(|a| q[a] >= 0 && q[a] < i32::from(size[a]))
            .then_some(q.map(|v| v as usize))
    }
    pub fn solid(&self, p: [i32; 3]) -> bool {
        self.local(p)
            .is_some_and(|[x, y, z]| self.body.vol.get(x, y, z))
    }
    pub fn resistance(&self, p: [i32; 3], c: &ExplosionConfig) -> u32 {
        let Some(cell) = self.local(p) else {
            return 0;
        };
        let [x, y, z] = cell;
        if !self.body.vol.get(x, y, z) {
            return 0;
        }
        unit_work(&self.body, cell, c)
            .saturating_sub(self.dents.get(&cell.map(|v| v as u8)).copied().unwrap_or(0))
    }
    #[cfg(test)]
    fn receive(&mut self, p: [i32; 3], pressure: u32, c: &ExplosionConfig) {
        let Some(cell) = self.local(p) else {
            return;
        };
        self.receive_cell(cell, pressure, c);
    }
    fn receive_cell(&mut self, cell: [usize; 3], pressure: u32, c: &ExplosionConfig) {
        let [x, y, z] = cell;
        if !self.body.vol.get(x, y, z) {
            return;
        }
        let key = cell.map(|v| v as u8);
        let need = unit_work(&self.body, cell, c)
            .saturating_sub(self.dents.get(&key).copied().unwrap_or(0));
        if pressure == 0 {
            return;
        }
        if pressure >= need {
            if self.body.remove(x, y, z) {
                self.removed.push(key);
                self.dents.remove(&key);
                self.revision += 1;
            }
        } else {
            let cell = key;
            *self.dents.entry(cell).or_default() += pressure;
            self.revision += 1;
        }
    }
    /// Explicit matter sink for this anti-matter weapon, never particle count.
    pub fn annihilated_mm3(&self) -> u64 {
        (self.removed.len() - self.detached) as u64 * u64::from(self.body.mm).pow(3)
    }
}

pub fn unit_work(body: &UnitBody, p: [usize; 3], c: &ExplosionConfig) -> u32 {
    (u64::from(c.unit_ablation_resistance)
        * u64::from(c.unit_role_resistance_milli[body.part_at(p[0], p[1], p[2]) as usize])
        / 1000)
        .clamp(1, 1_000_000) as u32
}

pub type SurfaceWork = BTreeMap<[i32; 3], u32>;
pub fn material_work(m: u16, c: &ExplosionConfig) -> u32 {
    match m {
        0 => 0,
        1 | 4 | 5 | 7 | 8 => c.soil_resistance,
        2 | 6 => c.concrete_resistance,
        _ => c.steel_resistance,
    }
}

pub struct RangeTerrain {
    extended: bool,
    layout: Option<RangeLayout>,
}
/// A flat firing lane surrounded by terraced woodland. Integer-only world data.
pub fn range_height(x: u32, z: u32) -> u32 {
    let mut ridge = 0;
    for (cx, cz, radius, height) in [
        (65, 65, 108, 25),
        (181, 36, 112, 32),
        (322, 100, 111, 29),
        (363, 319, 89, 18),
    ] {
        let dx = x as i32 - cx;
        let dz = z as i32 - (cz + 128);
        let r2 = radius * radius;
        ridge = ridge.max(((r2 - dx * dx - dz * dz).max(0) * height / r2) as u32);
    }
    // Blend the wooded hills into the level range without a straight escarpment.
    ridge = ridge * (302u32.saturating_sub(z) + x.saturating_sub(285)).min(48) / 48;
    let relief = if ridge > 0 {
        let a = (x / 48).wrapping_mul(17) ^ (z / 48).wrapping_mul(31);
        let b = (x / 48 + 1).wrapping_mul(17) ^ (z / 48).wrapping_mul(31);
        let c = (x / 48).wrapping_mul(17) ^ (z / 48 + 1).wrapping_mul(31);
        let d = (x / 48 + 1).wrapping_mul(17) ^ (z / 48 + 1).wrapping_mul(31);
        let u = x % 48;
        let v = z % 48;
        ((a % 7 * (48 - u) + b % 7 * u) * (48 - v) + (c % 7 * (48 - u) + d % 7 * u) * v) / (48 * 48)
    } else {
        0
    };
    FLOOR as u32 + ridge + relief
}
impl Generator for RangeTerrain {
    fn material(&self, x: u32, y: u32, z: u32) -> u16 {
        let shift = if self.extended { 160 } else { 0 };
        let layout_pad = self.layout.as_ref().is_some_and(|r| {
            x as i32 >= r.clear_min[0]
                && (x as i32) < r.clear_max[0]
                && z as i32 >= r.clear_min[1]
                && (z as i32) < r.clear_max[1]
        });
        if let Some(r) = &self.layout {
            if let Some(b) = r
                .blocks
                .iter()
                .rev()
                .find(|b| b.contains([x as i32, y as i32, z as i32]))
            {
                return b.material;
            }
        }
        if let Some(c) = self.layout.as_ref().and_then(|r| r.course.as_ref()) {
            if x as i32 >= c.min[0]
                && (x as i32) < c.max[0]
                && z as i32 >= c.min[1]
                && (z as i32) < c.max[1]
            {
                let mut h = FLOOR;
                for r in &c.ramps {
                    if (x as i32) >= r.min[0]
                        && (x as i32) < r.max[0]
                        && (z as i32) >= r.min[2]
                        && (z as i32) < r.max[2]
                    {
                        let v = if r.axis == 0 { x as i32 } else { z as i32 };
                        let n = r.max[r.axis] - r.min[r.axis];
                        let along = if r.reverse {
                            r.max[r.axis] - v
                        } else {
                            v - r.min[r.axis]
                        };
                        h = h.max(r.min[1] + along * (r.max[1] - r.min[1]) / n);
                    }
                }
                return if (y as i32) < h {
                    if y as i32 == h - 1 {
                        8
                    } else {
                        1
                    }
                } else {
                    0
                };
            }
        }
        let unit_pad = self.extended && (337..415).contains(&x) && (282..322).contains(&z);
        let h = if layout_pad || unit_pad || (self.extended && x < 448 && (298..387).contains(&z)) {
            FLOOR as u32
        } else {
            range_height(x, z)
        };
        if y < h {
            let lane = (322..351).contains(&z) && x < 254 + shift;
            return if y + 1 == h {
                if lane {
                    8
                } else {
                    7
                }
            } else {
                1
            };
        }
        // A massive rear wall channels the gas upward and sideways.
        if self.layout.is_none()
            && (256 + shift..264 + shift).contains(&x)
            && (291..383).contains(&z)
            && y < 66
        {
            // Ribbed revetment, with a broken parapet instead of a blank slab.
            if (y < 55 || !(z / 9).is_multiple_of(3)) && (x > 257 + shift || z % 12 < 3) {
                return 3;
            }
        }
        // Hollow concrete target, with a face struck by the shell.
        if self.layout.is_none()
            && (232 + shift..250 + shift).contains(&x)
            && (321..352).contains(&z)
            && y < 45
            && (x < 236 + shift || x >= 247 + shift || !(325..348).contains(&z) || y >= 41)
        {
            return 2;
        }
        // Voxel trees are actual matter: rays, shadows and gas see the same tree.
        // Leave the vehicle, firing lane and immediate target readable.
        let cx = x / 28;
        let cz = z / 28;
        let seed = cx.wrapping_mul(73856093) ^ cz.wrapping_mul(19349663);
        let tx = cx * 28 + 8 + seed % 11;
        let tz = cz * 28 + 8 + (seed >> 7) % 11;
        let layout_tree = self.layout.as_ref().is_some_and(|r| {
            tx as i32 >= r.clear_min[0] - 10
                && (tx as i32) < r.clear_max[0] + 10
                && tz as i32 >= r.clear_min[1] - 10
                && (tz as i32) < r.clear_max[1] + 10
        });
        let unit_trees = self.extended && (328..425).contains(&tx) && (273..332).contains(&tz);
        if !layout_tree
            && !unit_trees
            && !(tx < 288 + shift && (298..=386).contains(&tz))
            && !seed.is_multiple_of(5)
        {
            let base = range_height(tx, tz);
            let tall = 19 + (seed >> 13) % 18;
            let dx = x as i32 - tx as i32;
            let dz = z as i32 - tz as i32;
            let dy = y as i32 - base as i32;
            if dy >= 0 && dy < tall as i32 {
                if dx.abs() <= 1 && dz.abs() <= 1 && dy < tall as i32 - 4 {
                    return 5;
                }
                let crown = tall as i32 - dy;
                let radius = (crown / 3 + (dy % 5) / 2).clamp(1, 9);
                if dy > 5 && dx * dx + dz * dz < radius * radius {
                    return 4;
                }
            }
        }
        // Small broken rocks on the shoulders, out of the firing lane. No
        // taller than the course step: a hull climbs a shoulder rock rather
        // than steering round it (ADR-274).
        if !layout_pad && !unit_pad && !(x < 254 + shift && (319..354).contains(&z)) {
            let stone = (x / 13).wrapping_mul(9173) ^ (z / 13).wrapping_mul(6151);
            let dx = x as i32 % 13 - 6;
            let dz = z as i32 % 13 - 6;
            if stone.is_multiple_of(7)
                && y >= h
                && y < h + 2 + stone % 3
                && dx * dx + dz * dz < (5 - (y - h).min(4)) as i32 * 3
            {
                return 6;
            }
        }
        0
    }
}

pub struct Explosion {
    pub at: [i32; 3],
    origin: [i32; 3],
    cell: i32,
    pub wave: Wave,
    pub queued: usize,
    // Next subcell/actor in the first pending contact; committed work is never replayed.
    contact_cursor: usize,
    poses: Vec<crate::damage::Frame>,
    pending: std::collections::VecDeque<phys::blast::Hit>,
}

impl Explosion {
    /// Capture material resistance once. Results do not depend on how many
    /// edits the world has drained when a later piece of the wave is visited.
    pub fn new(w: &World, at: [i32; 3], c: &ExplosionConfig, units: &[RangeUnit]) -> Self {
        Self::with_damage(w, at, c, units, &SurfaceWork::new())
    }
    pub fn with_damage(
        w: &World,
        at: [i32; 3],
        c: &ExplosionConfig,
        units: &[RangeUnit],
        dents: &SurfaceWork,
    ) -> Self {
        let n = c.half_cells * 2 + 1;
        let h = c.half_cells as i32 * c.cell_units;
        let origin = at.map(|v| v - h);
        let mut resistance = Vec::with_capacity(n * n * n);
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    let p = [
                        origin[0] + x as i32 * c.cell_units,
                        origin[1] + y as i32 * c.cell_units,
                        origin[2] + z as i32 * c.cell_units,
                    ];
                    let mut r = 0;
                    for dz in 0..c.cell_units {
                        for dy in 0..c.cell_units {
                            for dx in 0..c.cell_units {
                                let q = [p[0] + dx, p[1] + dy, p[2] + dz];
                                let m = material(w, q);
                                for u in units {
                                    r = r.max(u.resistance(q, c));
                                }
                                r = r.max(
                                    material_work(m, c)
                                        .saturating_sub(dents.get(&q).copied().unwrap_or(0)),
                                );
                            }
                        }
                    }
                    resistance.push(r);
                }
            }
        }
        Self {
            at,
            pending: Default::default(),
            contact_cursor: 0,
            poses: units.iter().map(|u| u.pose).collect(),
            origin,
            cell: c.cell_units,
            wave: Wave::new(n, c.energy, resistance).expect("validated explosion"),
            queued: 0,
        }
    }

    // Separate authoritative outputs keep world edits and material claims explicit.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        w: &mut World,
        budget: usize,
        units: &mut [RangeUnit],
        dents: &mut SurfaceWork,
        changed: &mut Vec<[i32; 3]>,
        c: &ExplosionConfig,
        mut matter: Option<&mut matter::Matter>,
    ) {
        if self.pending.is_empty() {
            self.pending.extend(self.wave.advance_contacts(budget));
        }
        let edge = self.cell as usize;
        let actors = units.len() + 1;
        while let Some(hit) = self.pending.front().copied() {
            let p = std::array::from_fn::<_, 3, _>(|a| self.origin[a] + hit.cell[a] * self.cell);
            while self.contact_cursor < edge.pow(3) * actors {
                let subcell = self.contact_cursor / actors;
                let actor = self.contact_cursor % actors;
                let q = [
                    p[0] + (subcell % edge) as i32,
                    p[1] + (subcell / edge % edge) as i32,
                    p[2] + (subcell / (edge * edge)) as i32,
                ];
                // Advance only after successful application or an unaffordable
                // hit. Pool pressure retries this exact actor on the next tick.
                if let Some(u) = units.get_mut(actor) {
                    if let Some(cell) = u.local_at(q, &self.poses[actor]) {
                        let [x, y, z] = cell;
                        // A cell already cut loose leaves with its whole
                        // section; the wave neither ablates it nor waits for
                        // it. Every other cell takes the impulse as it arrives.
                        let leaving = u
                            .section
                            .as_ref()
                            .is_some_and(|j| j.holds(cell.map(|v| v as u8)));
                        if u.body.vol.get(x, y, z) && !leaving {
                            let need = unit_work(&u.body, cell, c).saturating_sub(
                                u.dents.get(&cell.map(|v| v as u8)).copied().unwrap_or(0),
                            );
                            let mut apply = true;
                            if hit.pressure >= need && hit.pressure > 0 {
                                if let Some(m) = matter.as_deref_mut() {
                                    match m.body(actor, u, cell, &self.poses[actor], self.at) {
                                        matter::ClaimResult::Applied => u.detached += 1,
                                        matter::ClaimResult::Unaffordable => apply = false,
                                        matter::ClaimResult::Deferred => return,
                                    }
                                }
                            }
                            if apply {
                                u.receive_cell(cell, hit.pressure, c);
                            }
                        }
                    }
                } else {
                    let m = material(w, q);
                    if m != 0 && hit.pressure > 0 {
                        let total = dents
                            .get(&q)
                            .copied()
                            .unwrap_or(0)
                            .saturating_add(hit.pressure);
                        let mut apply = true;
                        if total >= material_work(m, c) {
                            if let Some(m) = matter.as_deref_mut() {
                                match m.ground(w, q, self.at) {
                                    matter::ClaimResult::Applied => (),
                                    matter::ClaimResult::Unaffordable => apply = false,
                                    matter::ClaimResult::Deferred => return,
                                }
                            } else {
                                w.queue_dig(q[0] as u32, q[1] as u32, q[2] as u32);
                            }
                            if apply {
                                dents.remove(&q);
                                self.queued += 1;
                            }
                        } else {
                            dents.insert(q, total);
                        }
                        if apply {
                            changed.push(q);
                        }
                    }
                }
                self.contact_cursor += 1;
            }
            self.contact_cursor = 0;
            self.pending.pop_front();
        }
    }
}

pub fn material(w: &World, p: [i32; 3]) -> u16 {
    if p[0] < 0
        || p[1] < 0
        || p[2] < 0
        || p[0] >= w.nx as i32
        || p[1] >= w.ny as i32
        || p[2] >= w.nz as i32
    {
        return 0;
    }
    w.material(p[0] as u32, p[1] as u32, p[2] as u32)
}

/// A dome on the range (ADR-275): geometry from the layout, charge in the
/// wave units of the weapons.
#[derive(Clone, Debug)]
pub struct RangeShield {
    pub at: [i32; 3],
    pub radius: i32,
    pub charge: u32,
    /// Ticks left before a collapsed dome stands again; zero while it stands.
    pub down: u32,
    pub hits: u32,
}
impl RangeShield {
    fn new(d: &RangeShieldDef, capacity: u32) -> Self {
        Self {
            at: d.at,
            radius: d.radius,
            charge: capacity,
            down: 0,
            hits: 0,
        }
    }
    /// The predicate's view of the dome: a collapsed one catches nothing.
    pub fn dome(&self) -> Shield {
        Shield {
            x: self.at[0],
            y: self.at[1],
            z: self.at[2],
            r: self.radius,
            owner: 0,
            charge: if self.down > 0 {
                0
            } else {
                self.charge.min(i32::MAX as u32) as i32
            },
        }
    }
    pub fn standing(&self) -> bool {
        self.down == 0 && self.charge > 0
    }
}
/// A round caught by a dome this tick, for the renderer's flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShieldHit {
    pub shield: usize,
    pub at: [i32; 3],
    pub drained: u32,
    pub collapsed: bool,
}
fn shot_point(p: [i32; 3]) -> Point {
    Point {
        x: p[0],
        y: p[1],
        z: p[2],
        owner: 1,
    }
}

pub struct BlastRange {
    pub matter: matter::Matter,
    pub world: World,
    pub config: ExplosionConfig,
    pub tick: u32,
    pub shell: Option<[Fx; 3]>,
    velocity: [Fx; 3],
    pub impact: Option<[i32; 3]>,
    pub impact_tick: u32,
    pub explosion: Option<Explosion>,
    pub removed: usize,
    pub units: Vec<RangeUnit>,
    pub layout: Option<RangeLayout>,
    // Immutable generator/roster selection, independent of the equipped weapon.
    extended: bool,
    witness: bool,
    pub removed_materials: [usize; 16],
    pub surface_work: SurfaceWork,
    /// Transient renderer notification; authoritative values live above.
    pub surface_changed: Vec<[i32; 3]>,
    pub fire_tick: u32,
    pub gun: RangeUnit,
    pub player: bool,
    pub patrols_running: bool,
    pub attack: Option<usize>,
    pub aim: Option<[i32; 3]>,
    pub turret: crate::turret::Traverse,
    turret_limits: crate::turret::Limits,
    navigation: modding::navigation::Navigation,
    pub shields: Vec<RangeShield>,
    /// Transient renderer notification, like `surface_changed`.
    pub shield_hits: Vec<ShieldHit>,
    shield_tuning: modding::shield::ShieldTuning,
}

impl BlastRange {
    pub fn target(&self) -> [i32; 3] {
        if let Some(id) = self.attack {
            if let Some(u) = self.units.get(id) {
                let mut p = u.world_pos([
                    u.body.core.0 as usize,
                    u.body.core.1 as usize,
                    u.body.core.2 as usize,
                ]);
                let dx = Fx::from_int(p[0]).sub(self.gun.motion.pos[0]);
                let dz = Fx::from_int(p[2]).sub(self.gun.motion.pos[2]);
                let time = dx
                    .mul(dx)
                    .add(dz.mul(dz))
                    .sqrt()
                    .div(Fx::from_int(self.config.shell_speed))
                    .max(Fx::from_ratio(1, 1000));
                let travel = u.motion.drive.vel.mul(time);
                p[0] += travel
                    .mul(fx::angle::cos(u.motion.drive.head))
                    .to_int_trunc();
                p[2] += travel
                    .mul(fx::angle::sin(u.motion.drive.head))
                    .to_int_trunc();
                return p;
            }
        }
        if let Some(p) = self.aim {
            return p;
        }
        self.layout.as_ref().map_or(
            if self.config.motion == ProjectileMotion::Direct {
                ENERGY_TARGET
            } else {
                TARGET
            },
            |r| r.target,
        )
    }

    pub fn aim_angle(&self) -> i32 {
        let target = self.target();
        let x = Fx::from_int(target[0]).sub(self.gun.motion.pos[0]);
        let z = Fx::from_int(target[2]).sub(self.gun.motion.pos[2]);
        fx::angle::atan2(z.raw(), x.raw())
    }
    /// Actual actuator direction, shared by muzzle, flight and presentation.
    pub fn heading(&self) -> [Fx; 3] {
        let x = fx::angle::cos(self.turret.angle);
        let z = fx::angle::sin(self.turret.angle);
        let length = x.mul(x).add(z.mul(z)).sqrt();
        if length == Fx::ZERO {
            return [Fx::ONE, Fx::ZERO, Fx::ZERO];
        }
        [x.div(length), Fx::ZERO, z.div(length)]
    }
    pub fn muzzle(&self) -> [Fx; 3] {
        let d = self.heading();
        [
            self.gun.motion.pos[0].add(d[0].mul(Fx::from_int(30))),
            self.gun.motion.pos[1].add(Fx::from_int(23)),
            self.gun.motion.pos[2].add(d[2].mul(Fx::from_int(30))),
        ]
    }
    pub fn new(config: ExplosionConfig) -> Self {
        Self::build(config, None)
    }
    pub fn with_layout(mut config: ExplosionConfig, layout: RangeLayout) -> Result<Self, String> {
        layout.validate()?;
        config.unit_ablation_resistance = layout.unit_resistance;
        let r = Self::build(config, Some(layout));
        for (i, u) in r.units.iter().enumerate() {
            for z in 0..u.body.vol.nz as usize {
                for y in 0..u.body.vol.ny as usize {
                    for x in 0..u.body.vol.nx as usize {
                        if !u.body.vol.get(x, y, z) {
                            continue;
                        }
                        let p = u.world_pos([x, y, z]);
                        if material(&r.world, p) != 0
                            || r.units[..i].iter().any(|other| other.solid(p))
                        {
                            return Err(
                                "a range unit intersects terrain, a block or another unit".into()
                            );
                        }
                    }
                }
            }
        }
        Ok(r)
    }
    fn build(config: ExplosionConfig, layout: Option<RangeLayout>) -> Self {
        let extended = layout.is_some() || config.motion == ProjectileMotion::Direct;
        let witness = config.unit_ablation_resistance > 0;
        Self::build_scene(config, layout, extended, witness)
    }
    fn build_scene(
        config: ExplosionConfig,
        layout: Option<RangeLayout>,
        extended: bool,
        witness: bool,
    ) -> Self {
        let world = World::new(
            SIZE[0],
            SIZE[1],
            SIZE[2],
            config.edits_per_tick,
            Box::new(RangeTerrain {
                extended,
                layout: layout.clone(),
            }),
        );
        let mut units: Vec<RangeUnit> = if let Some(r) = &layout {
            r.units
                .iter()
                .map(|u| RangeUnit::of_kind(u.kind, u.at))
                .collect()
        } else {
            witness
                .then(|| RangeUnit::new(WITNESS))
                .into_iter()
                .collect()
        };
        let mut gun = RangeUnit::new(GUN);
        if let Some(c) = layout.as_ref().and_then(|r| r.course.as_ref()) {
            for u in units.iter_mut().chain(std::iter::once(&mut gun)) {
                u.motion.speed = if u.kind == RangeUnitKind::Tank {
                    c.tank_speed
                } else {
                    c.walker_speed
                };
                u.motion.step = c.step_height;
                u.motion.arrival = c.arrival;
                u.motion.alignment = c.turn_alignment_pct;
                u.motion.replan = c.replan_ticks;
            }
            for p in &c.patrols {
                units[p.unit].motion.route = p.points.clone();
                units[p.unit].motion.enabled = true;
            }
        }
        let matter = matter::Matter::new(&units);
        let shield_tuning =
            modding::shield::ShieldTuning::from_ron(include_str!("../../../data/shield.ron"))
                .expect("shield tuning");
        let shields = layout.as_ref().map_or_else(Vec::new, |r| {
            r.shields
                .iter()
                .map(|d| RangeShield::new(d, shield_tuning.capacity))
                .collect()
        });
        let mut result = Self {
            matter,
            gun,
            player: false,
            patrols_running: true,
            attack: None,
            aim: None,
            world,
            fire_tick: config.fire_tick,
            config,
            tick: 0,
            shell: None,
            velocity: [Fx::ZERO; 3],
            impact: None,
            impact_tick: 0,
            explosion: None,
            removed: 0,
            units,
            layout,
            extended,
            witness,
            removed_materials: [0; 16],
            surface_work: SurfaceWork::new(),
            surface_changed: Vec::new(),
            turret: crate::turret::Traverse::new(0),
            turret_limits: crate::turret::Limits::of(
                modding::turret::Turret::from_ron(include_str!("../../../data/turret.ron"))
                    .expect("turret limits"),
                crate::TICKS_PER_SECOND,
            ),
            navigation: modding::navigation::Navigation::from_ron(include_str!(
                "../../../data/range-navigation.ron"
            ))
            .expect("navigation"),
            shields,
            shield_hits: Vec::new(),
            shield_tuning,
        };
        result.turret = crate::turret::Traverse::new(result.aim_angle());
        result
    }
    pub fn shield_tuning(&self) -> &modding::shield::ShieldTuning {
        &self.shield_tuning
    }
    /// The first dome whose surface lies between two swept points: a dome
    /// is a wall for a round crossing it either way (ADR-040).
    fn dome_crossed(&self, a: [i32; 3], b: [i32; 3]) -> Option<usize> {
        let (pa, pb) = (shot_point(a), shot_point(b));
        self.shields.iter().position(|s| {
            let d = s.dome();
            inside(&d, &pa) != inside(&d, &pb)
        })
    }
    /// The round is spent against the dome: its wave energy leaves the
    /// charge, and a dome drained to nothing collapses for the tuned outage.
    fn absorb(&mut self, i: usize, at: [i32; 3]) {
        let drain = self.config.energy;
        let overload = self.shield_tuning.overload_ticks;
        let s = &mut self.shields[i];
        let drained = drain.min(s.charge);
        let collapsed = s.charge <= drain;
        s.charge -= drained;
        s.hits += 1;
        if collapsed {
            s.down = overload;
        }
        self.shield_hits.push(ShieldHit {
            shield: i,
            at,
            drained,
            collapsed,
        });
    }

    /// Complete authoritative snapshot of this test range. Visual gas is absent.
    pub fn snapshot(&self) -> Vec<u8> {
        let mut b = fx::blob::Blob::new();
        b.u32(15);
        b.bool(self.extended);
        b.bool(self.witness);
        b.u32(self.fire_tick);
        b.bool(self.layout.is_some());
        if let Some(r) = &self.layout {
            b.u8s(r.to_ron().expect("validated layout").as_bytes());
        }
        self.turret.save(&mut b);
        b.bool(self.player);
        b.bool(self.patrols_running);
        b.usize(self.attack.unwrap_or(usize::MAX));
        b.bool(self.aim.is_some());
        if let Some(p) = self.aim {
            for v in p {
                b.i32(v);
            }
        }
        b.u32(self.tick);
        b.bool(self.shell.is_some());
        if let Some(p) = self.shell {
            for v in p {
                b.fx(v);
            }
        }
        for v in self.velocity {
            b.fx(v);
        }
        b.bool(self.impact.is_some());
        if let Some(at) = self.impact {
            for v in at {
                b.i32(v);
            }
        }
        b.u32(self.impact_tick);
        b.usize(self.removed);
        for n in self.removed_materials {
            b.usize(n);
        }
        b.u32(self.surface_work.len() as u32);
        for (p, work) in &self.surface_work {
            for v in p {
                b.i32(*v);
            }
            b.u32(*work);
        }
        b.u32(self.units.len() as u32 + 1);
        for u in self.units.iter().chain(std::iter::once(&self.gun)) {
            for v in u.at {
                b.i32(v);
            }
            u.motion.save(&mut b);
            b.u64(u.revision);
            b.u32(u.dents.len() as u32);
            for (p, work) in &u.dents {
                for v in p {
                    b.u8(*v);
                }
                b.u32(*work);
            }
            b.usize(u.removed.len());
            b.usize(u.detached);
            for p in &u.removed {
                for v in p {
                    b.u8(*v);
                }
            }
            b.bool(u.section.is_some());
            if let Some(j) = &u.section {
                j.save_cut(&mut b);
            }
        }
        self.world.save(&mut b);
        self.matter.save(&mut b);
        b.bool(self.explosion.is_some());
        if let Some(e) = &self.explosion {
            for v in e.at {
                b.i32(v);
            }
            for v in e.origin {
                b.i32(v);
            }
            b.i32(e.cell);
            b.usize(e.queued);
            e.wave.save(&mut b);
            b.usize(e.contact_cursor);
            b.usize(e.pending.len());
            for hit in &e.pending {
                for v in hit.cell {
                    b.i32(v);
                }
                b.u32(hit.pressure);
                b.bool(hit.broken);
            }
            b.u32(e.poses.len() as u32);
            for f in &e.poses {
                for v in [f.base, f.x, f.y, f.z] {
                    b.fx(v.x);
                    b.fx(v.y);
                    b.fx(v.z);
                }
                b.fx(f.scale);
                for v in f.centre {
                    b.fx(v);
                }
            }
        }
        b.u32(self.shields.len() as u32);
        for s in &self.shields {
            b.u32(s.charge);
            b.u32(s.down);
            b.u32(s.hits);
        }
        b.v
    }

    pub fn restore(config: ExplosionConfig, bytes: &[u8]) -> Option<Self> {
        let mut s = fx::blob::Scan::new(bytes);
        if s.u32() != 15 {
            return None;
        }
        let extended = s.bool();
        let witness = s.bool();
        let fire_tick = s.u32();
        let mut r = if s.bool() {
            let data = s.u8s();
            if data.len() > 65536 {
                return None;
            }
            Self::with_layout(
                config,
                RangeLayout::from_ron(std::str::from_utf8(&data).ok()?).ok()?,
            )
            .ok()?
        } else {
            Self::build_scene(config, None, extended, witness)
        };
        if r.extended != extended {
            return None;
        }
        r.witness = witness;
        r.turret = crate::turret::Traverse::load(&mut s, r.turret_limits)?;
        r.player = s.bool();
        r.patrols_running = s.bool();
        let attack = s.usize();
        if attack != usize::MAX {
            if attack >= r.units.len() {
                return None;
            }
            r.attack = Some(attack);
        }
        if s.bool() {
            r.aim = Some(std::array::from_fn(|_| s.i32()));
        }
        r.fire_tick = fire_tick;
        r.tick = s.u32();
        if s.bool() {
            r.shell = Some(std::array::from_fn(|_| s.fx()));
        }
        r.velocity = std::array::from_fn(|_| s.fx());
        if s.bool() {
            r.impact = Some(std::array::from_fn(|_| s.i32()));
        }
        r.impact_tick = s.u32();
        r.removed = s.usize();
        r.removed_materials = std::array::from_fn(|_| s.usize());
        let count = s.u32() as usize;
        if count > bytes.len() / 16 {
            return None;
        }
        for _ in 0..count {
            let p = std::array::from_fn(|_| s.i32());
            let work = s.u32();
            if work == 0
                || work > 1_000_000
                || (0..3).any(|a| p[a] < 0 || p[a] >= SIZE[a] as i32)
                || r.surface_work.insert(p, work).is_some()
            {
                return None;
            }
        }
        if s.u32() as usize != r.units.len() + 1 {
            return None;
        }
        for (id, u) in r
            .units
            .iter_mut()
            .chain(std::iter::once(&mut r.gun))
            .enumerate()
        {
            let at = std::array::from_fn(|_| s.i32());
            if !u.motion.load(&mut s) {
                return None;
            }
            u.update_pose();
            if u.at != at {
                return None;
            }
            let revision = s.u64();
            let count = s.u32() as usize;
            if count > u.body.solid_count() as usize {
                return None;
            }
            for _ in 0..count {
                let p = [s.u8(), s.u8(), s.u8()];
                let work = s.u32();
                if p[0] >= u.body.vol.nx
                    || p[1] >= u.body.vol.ny
                    || p[2] >= u.body.vol.nz
                    || !u.body.vol.get(p[0] as usize, p[1] as usize, p[2] as usize)
                    || work == 0
                    || work >= 1_000_000
                    || u.dents.insert(p, work).is_some()
                {
                    return None;
                }
            }
            let count = s.usize();
            u.detached = s.usize();
            if u.detached > count {
                return None;
            }
            if count > u.body.solid_count() as usize {
                return None;
            }
            for _ in 0..count {
                let p = [s.u8(), s.u8(), s.u8()];
                let [x, y, z] = p.map(usize::from);
                if x >= u.body.vol.nx as usize
                    || y >= u.body.vol.ny as usize
                    || z >= u.body.vol.nz as usize
                    || !u.body.vol.get(x, y, z)
                    || u.dents.contains_key(&p)
                {
                    return None;
                }
                u.body.remove(x, y, z);
                u.removed.push(p);
            }
            u.revision = revision;
            u.update_response();
            if s.bool() {
                u.section = Some(crate::damage::SectionJob::load_cut(
                    id as u32,
                    0,
                    &mut s,
                    &u.body,
                    &mut u.connectivity,
                )?);
            }
            if u.status.alive && u.status.detached > 0 && u.section.is_none() {
                return None;
            }
        }
        if !r.world.load(&mut s) {
            return None;
        }
        r.matter.load(&mut s, &r.world, &r.units)?;
        if r.surface_work.iter().any(|(p, work)| {
            let m = material(&r.world, *p);
            m == 0 || *work >= 1_000_000
        }) {
            return None;
        }
        if s.bool() {
            let at = std::array::from_fn(|_| s.i32());
            let origin = std::array::from_fn(|_| s.i32());
            let cell = s.i32();
            let queued = s.usize();
            let wave = Wave::load(&mut s)?;
            let contact_cursor = s.usize();
            let n = s.usize();
            if n > 8192 {
                return None;
            }
            let mut pending = std::collections::VecDeque::new();
            for _ in 0..n {
                let hit = phys::blast::Hit {
                    cell: std::array::from_fn(|_| s.i32()),
                    pressure: s.u32(),
                    broken: s.bool(),
                };
                if hit.cell.iter().any(|&v| v < 0 || v >= wave.size() as i32)
                    || hit.pressure > 1_000_000
                {
                    return None;
                }
                pending.push_back(hit);
            }
            if !(1..=4).contains(&cell)
                || contact_cursor >= (cell as usize).pow(3) * (r.units.len() + 1)
                || (pending.is_empty() && contact_cursor != 0)
                || (0..3).any(|a| {
                    at[a] < 0
                        || at[a] >= SIZE[a] as i32
                        || i64::from(origin[a])
                            != i64::from(at[a]) - (wave.size() / 2) as i64 * i64::from(cell)
                })
            {
                return None;
            }
            let n = s.u32() as usize;
            if n != r.units.len() {
                return None;
            }
            let mut poses = Vec::new();
            for u in &r.units {
                let mut v = || phys::V3::new(s.fx(), s.fx(), s.fx());
                let (base, x, y, z) = (v(), v(), v(), v());
                let scale = s.fx();
                let centre = std::array::from_fn(|_| s.fx());
                if scale != u.pose.scale
                    || centre != u.pose.centre
                    || [base.x, base.y, base.z]
                        .iter()
                        .any(|v| *v < Fx::from_int(-384) || *v > Fx::from_int(1024))
                    || [x, y, z].iter().any(|v| {
                        [v.x, v.y, v.z]
                            .iter()
                            .any(|c| *c < Fx::from_int(-2) || *c > Fx::from_int(2))
                    })
                {
                    return None;
                }
                poses.push(crate::damage::Frame {
                    base,
                    x,
                    y,
                    z,
                    scale,
                    centre,
                });
            }
            r.explosion = Some(Explosion {
                poses,
                pending,
                contact_cursor,
                at,
                origin,
                cell,
                queued,
                wave,
            });
        }
        if s.u32() as usize != r.shields.len() {
            return None;
        }
        for sh in &mut r.shields {
            sh.charge = s.u32();
            sh.down = s.u32();
            sh.hits = s.u32();
            if sh.charge > r.shield_tuning.capacity || sh.down > r.shield_tuning.overload_ticks {
                return None;
            }
        }
        (s.ok() && s.left() == 0).then_some(r)
    }

    pub fn state_hash(&self) -> u64 {
        self.snapshot()
            .into_iter()
            .fold(0xcbf29ce484222325, |h, v| {
                (h ^ u64::from(v)).wrapping_mul(0x100000001b3)
            })
    }

    /// Charge another round against the current, already damaged world.
    pub fn ready(&self) -> bool {
        self.gun.status.alive
            && self.gun.status.weapon
            && self.shell.is_none()
            && (self.fire_tick == u32::MAX || self.tick > self.fire_tick)
            && self
                .explosion
                .as_ref()
                .is_none_or(|e| e.wave.finished() && e.pending.is_empty())
            && self.world.queued() == 0
            && self.units.iter().all(|u| u.section.is_none())
    }
    pub fn refire(&mut self) -> bool {
        if !self.ready() {
            return false;
        }
        self.fire_tick = self.tick + self.config.charge_ticks + 1;
        self.impact = None;
        self.explosion = None;
        true
    }

    pub fn control(&mut self) {
        if !self.player && self.impact.is_none() && self.shell.is_none() {
            self.fire_tick = u32::MAX;
        }
        self.player = true;
    }
    pub fn order_move(&mut self, to: [i32; 2]) -> bool {
        self.control();
        self.attack = None;
        self.gun.motion.route.clear();
        self.gun.motion.move_to(
            &self.world,
            to,
            &self.gun.body,
            &self
                .units
                .iter()
                .map(|u| motion::Obstacle::of(&u.motion, &u.body))
                .collect::<Vec<_>>(),
        )
    }
    pub fn order_attack(&mut self, id: usize) -> bool {
        if id >= self.units.len() || !self.units[id].status.alive {
            return false;
        }
        self.control();
        self.attack = Some(id);
        self.gun.motion.stop();
        true
    }
    pub fn step(&mut self) {
        self.surface_changed.clear();
        self.shield_hits.clear();
        // Domes recover before this tick's round flies: a collapsed one
        // counts its outage down, a standing one refills to capacity.
        for s in &mut self.shields {
            if s.down > 0 {
                s.down -= 1;
            } else {
                s.charge = s
                    .charge
                    .saturating_add(self.shield_tuning.recharge_per_tick)
                    .min(self.shield_tuning.capacity);
            }
        }
        // This small range commits motion in stable slot order. Later actors
        // see accepted positions, so two approaching tanks cannot each consume
        // the same free gap between the beginning and end of the tick.
        let mut positions: Vec<_> = self
            .units
            .iter()
            .map(|u| motion::Obstacle::of(&u.motion, &u.body))
            .collect();
        for (id, u) in self.units.iter_mut().enumerate() {
            if self.patrols_running {
                let others: Vec<_> = positions
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != id)
                    .map(|(_, p)| *p)
                    .chain(std::iter::once(motion::Obstacle::of(
                        &self.gun.motion,
                        &self.gun.body,
                    )))
                    .collect();
                u.motion.step(
                    &self.world,
                    &u.body,
                    u.status.alive,
                    u.status.engine,
                    u.mobility,
                    &others,
                    self.tick,
                    self.config.gravity,
                );
                u.update_pose();
                positions[id] = motion::Obstacle::of(&u.motion, &u.body);
            }
        }
        self.gun.motion.step(
            &self.world,
            &self.gun.body,
            self.gun.status.alive,
            self.gun.status.engine,
            self.gun.mobility,
            &positions,
            self.tick,
            self.config.gravity,
        );
        self.gun.update_pose();
        // Tracked hulls cut the yielding matter they stand in; the crown that
        // hung from those cells follows over the next ticks.
        let cuts: Vec<(Vec<[u32; 3]>, [i32; 3])> = self
            .units
            .iter()
            .chain(std::iter::once(&self.gun))
            .filter(|u| u.kind == RangeUnitKind::Tank && u.status.alive && !u.motion.cut.is_empty())
            .map(|u| (u.motion.cut.clone(), u.at))
            .collect();
        for (cut, at) in cuts {
            self.matter
                .fell(&mut self.world, &cut, at, &self.navigation.fell);
        }
        self.matter
            .step_felling(&mut self.world, &self.navigation.fell);
        let target_angle = self.aim_angle();
        self.turret.step(target_angle, self.turret_limits);
        if let Some(id) = self.attack {
            if !self.units[id].status.alive {
                self.attack = None;
            } else {
                let reload = self
                    .layout
                    .as_ref()
                    .and_then(|r| r.course.as_ref())
                    .map_or(0, |c| c.reload_ticks);
                if self.fire_tick == u32::MAX
                    || (self.impact.is_some() && self.tick >= self.impact_tick + reload)
                {
                    self.refire();
                }
            }
        }
        let dt = Fx::from_ratio(1, crate::TICKS_PER_SECOND);
        let gravity = match self.config.motion {
            ProjectileMotion::Ballistic => self.config.gravity,
            ProjectileMotion::Direct => 0,
        };
        if self.tick == self.fire_tick && !self.turret.aligned(target_angle, self.turret_limits) {
            // Keep the charged round until the actual barrel has traversed.
            self.fire_tick = self.fire_tick.saturating_add(1);
        }
        if self.tick == self.fire_tick && self.gun.status.alive && self.gun.status.weapon {
            let from = self.muzzle();
            let target = self.target();
            let dx = Fx::from_int(target[0]).sub(from[0]);
            let dz = Fx::from_int(target[2]).sub(from[2]);
            let time = dx
                .mul(dx)
                .add(dz.mul(dz))
                .sqrt()
                .div(Fx::from_int(self.config.shell_speed))
                .max(Fx::from_ratio(1, 1000));
            self.velocity = [
                self.heading()[0].mul(Fx::from_int(self.config.shell_speed)),
                Fx::from_int(target[1])
                    .sub(from[1])
                    .div(time)
                    .add(Fx::from_int(gravity).mul(time).mul(Fx::from_ratio(1, 2))),
                self.heading()[2].mul(Fx::from_int(self.config.shell_speed)),
            ];
            self.shell = Some(from);
        }
        if let Some(p) = self.shell {
            self.velocity[1] = self.velocity[1].sub(Fx::from_int(gravity).mul(dt));
            let q = std::array::from_fn(|a| p[a].add(self.velocity[a].mul(dt)));
            // Sweep at sub-voxel intervals; never jump through a thin target.
            let span = (0..3)
                .map(|a| q[a].sub(p[a]).abs().to_int_trunc())
                .max()
                .unwrap_or(0)
                + 1;
            let mut impact = None;
            let mut caught = None;
            let mut prev = p.map(Fx::to_int_trunc);
            for s in 1..=span * 2 {
                let f = Fx::from_ratio(s, span * 2);
                let v = std::array::from_fn(|a| p[a].add(q[a].sub(p[a]).mul(f)).to_int_trunc());
                if let Some(i) = self.dome_crossed(prev, v) {
                    caught = Some((i, v));
                    break;
                }
                prev = v;
                if self
                    .layout
                    .as_ref()
                    .is_some_and(|r| !self.player && r.airburst_on_target && v[0] >= r.target[0])
                    || material(&self.world, v) != 0
                    || self.units.iter().any(|u| u.solid(v))
                {
                    impact = Some(v);
                    break;
                }
            }
            if let Some((i, at)) = caught {
                // Spent against the dome: no impact, no crater, another
                // order may follow at once.
                self.absorb(i, at);
                self.shell = None;
                self.fire_tick = u32::MAX;
            } else if let Some(at) = impact {
                self.impact = Some(at);
                self.impact_tick = self.tick;
                self.matter.begin(self.config.material_energy_j);
                self.explosion = Some(Explosion::with_damage(
                    &self.world,
                    at,
                    &self.config,
                    &self.units,
                    &self.surface_work,
                ));
                self.shell = None;
            } else if (0..3).any(|a| q[a] < Fx::ZERO || q[a] >= Fx::from_int(SIZE[a] as i32)) {
                // A miss retires at the map boundary, allowing another order.
                self.shell = None;
                self.fire_tick = u32::MAX;
            } else {
                self.shell = Some(q);
            }
        }
        if let Some(e) = self.explosion.as_mut() {
            let before: Vec<_> = self.units.iter().map(|u| u.removed.len()).collect();
            e.step(
                &mut self.world,
                self.config.wave_work_per_tick,
                &mut self.units,
                &mut self.surface_work,
                &mut self.surface_changed,
                &self.config,
                (self.config.material_energy_j > 0).then_some(&mut self.matter),
            );
            for (id, (u, n)) in self.units.iter_mut().zip(before).enumerate() {
                if u.removed.len() != n {
                    if self.config.material_energy_j == 0 {
                        self.matter.annihilate_body(id, u.removed.len() - n);
                    }
                    u.refresh_damage(id);
                }
            }
        }
        self.world.flush();
        self.matter.step_sections(&mut self.units);
        self.matter.flush(
            &self.world,
            self.config.material_energy_j > 0,
            self.config.gravity,
        );
        for e in self.world.edits() {
            if e.mat == 0 {
                self.surface_work
                    .remove(&[e.x as i32, e.y as i32, e.z as i32]);
            }
        }
        self.removed += self.world.edits().iter().filter(|e| e.mat == 0).count();
        for &(_, mat) in self.world.taken() {
            if let Some(n) = self.removed_materials.get_mut(mat as usize) {
                *n += 1;
            }
        }
        self.tick += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_engine_damage_uses_the_shared_physical_mobility_response() {
        let r = BlastRange::new(config());
        let mut intact = RangeUnit::new(GUN);
        let mut damaged = RangeUnit::new(GUN);
        for z in 0..30 {
            for y in 0..30 {
                for x in 0..16 {
                    if damaged.body.vol.get(x, y, z)
                        && damaged.body.part_at(x, y, z) == voxel::Part::Engine
                    {
                        damaged.body.remove(x, y, z);
                    }
                }
            }
        }
        damaged.update_response();
        assert!(damaged.status.alive && damaged.status.engine);
        assert!(
            damaged.mobility.engine > Fx::ZERO && damaged.mobility.engine < Fx::from_ratio(1, 2)
        );
        for u in [&mut intact, &mut damaged] {
            assert!(u.motion.move_to(&r.world, [205, 336], &u.body, &[]));
        }
        for tick in 0..30 {
            for u in [&mut intact, &mut damaged] {
                u.motion.step(
                    &r.world,
                    &u.body,
                    u.status.alive,
                    u.status.engine,
                    u.mobility,
                    &[],
                    tick,
                    r.config.gravity,
                );
            }
        }
        assert!(damaged.motion.drive.vel < intact.motion.drive.vel);
        assert!(damaged.motion.pos[0] < intact.motion.pos[0]);
        assert!(
            damaged.motion.pos[0] > Fx::from_int(GUN[0]),
            "damaged engine still has some thrust"
        );
    }
    #[test]
    fn a_severed_barrel_stops_an_already_charged_range_shot() {
        let c = config();
        let mut r = BlastRange::new(c);
        for z in 0..30 {
            for y in 0..30 {
                r.gun.body.remove(54, y, z);
            }
        }
        r.gun.status = r.gun.body.status(&mut r.gun.connectivity);
        assert!(r.gun.status.alive && !r.gun.status.weapon);
        r.fire_tick = 0;
        r.step();
        assert!(r.shell.is_none() && r.impact.is_none());
        assert!(!r.refire());
    }

    #[test]
    fn a_track_break_brakes_the_range_tank_and_resumes_without_powered_turning() {
        let c = config();
        let mut a = BlastRange::new(c.clone());
        a.control();
        a.gun.motion.drive.vel = Fx::from_int(30);
        for y in 0..10 {
            for z in 1..7 {
                if a.gun.body.remove(30, y, z) {
                    a.gun.removed.push([30, y as u8, z as u8]);
                }
            }
        }
        a.gun.refresh_damage(a.units.len());
        assert!(a.gun.status.alive && a.gun.status.engine && a.gun.status.weapon);
        assert_eq!(a.gun.mobility.traction, Fx::ZERO);
        a.order_move([205, 336]);
        let mut b = BlastRange::restore(c, &a.snapshot()).unwrap();
        let mut first = Fx::ZERO;
        for i in 0..120 {
            a.step();
            b.step();
            if i == 0 {
                first = a.gun.motion.drive.vel;
            }
            assert_eq!(a.state_hash(), b.state_hash());
            assert_eq!(a.gun.motion.drive.head, 0);
        }
        assert!(first > Fx::ZERO && first < Fx::from_int(30));
        assert_eq!(a.gun.motion.drive.vel, Fx::ZERO);
        assert!(a.gun.motion.pos[0] < Fx::from_int(205));
    }
    #[test]
    fn changing_aim_traverses_before_firing_and_resumes_mid_turn() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut a = BlastRange::new(c.clone());
        a.control();
        let start = a.muzzle();
        a.aim = Some([128, 30, 520]);
        assert_eq!(start, a.muzzle(), "new aim teleported the barrel");
        assert!(a.refire());
        for _ in 0..10 {
            a.step();
        }
        assert!(a.shell.is_none());
        assert!(a.turret.rate > 0 && a.turret.angle != 0);
        let mut b = BlastRange::restore(c, &a.snapshot()).unwrap();
        let mut fired = false;
        for _ in 0..200 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
            if a.shell.is_some() {
                assert!(a.turret.aligned(a.aim_angle(), a.turret_limits));
                let speed = a.velocity[0]
                    .mul(a.velocity[0])
                    .add(a.velocity[2].mul(a.velocity[2]))
                    .sqrt();
                assert!(
                    speed.sub(Fx::from_int(a.config.shell_speed)).abs() < Fx::from_ratio(1, 1000)
                );
                fired = true;
                break;
            }
        }
        assert!(fired);
    }

    #[test]
    fn range_sections_wait_atomically_resume_and_keep_mass_and_pose() {
        let c = config();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        a.control();
        a.patrols_running = false;
        let initial = a.units[0].body.solid_count();
        let u = &mut a.units[0];
        // Cut the protruding barrel while retaining the core and turret.
        for z in 0..30 {
            for y in 0..30 {
                if u.body.remove(54, y, z) {
                    u.removed.push([54, y as u8, z as u8]);
                }
            }
        }
        u.revision = u.removed.len() as u64;
        a.matter.annihilate_body(0, u.removed.len());
        u.refresh_damage(0);
        assert!(u.status.alive && u.status.detached > 8);
        let detached = u.status.detached;
        let after_cut = u.body.solid_count();
        let sink = a.matter.economy.ledger.sunk_total();
        for mut budget in [1, 40, 100] {
            let u = &mut a.units[0];
            u.section.as_mut().unwrap().advance(&u.body, &mut budget);
            let restored = BlastRange::restore(c.clone(), &a.snapshot()).unwrap();
            assert_eq!(a.state_hash(), restored.state_hash(), "partial extraction");
        }
        a.matter.tuning.max_flying = 0;
        for _ in 0..30 {
            a.matter.step_sections(&mut a.units);
        }
        assert_eq!(a.units[0].body.solid_count(), after_cut);
        assert!(a.matter.debris.fragments.is_empty());
        assert!(a.units[0].section.as_ref().unwrap().ready());
        let snapshot = a.snapshot();
        let mut b = BlastRange::restore(c, &snapshot).unwrap();
        a.matter.tuning.max_flying = b.matter.tuning.max_flying;
        assert_eq!(a.state_hash(), b.state_hash());
        for _ in 0..40 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        assert!(a.units[0].section.is_none());
        assert_eq!(a.units[0].body.solid_count(), after_cut - detached);
        assert_eq!(a.units[0].status.detached, 0);
        assert_eq!(a.matter.economy.ledger.sunk_total(), sink);
        assert!(a.matter.economy.ledger.balanced());
        let fragments = &a.matter.debris.fragments;
        assert!(
            fragments.iter().any(|f| f.count > 8),
            "barrel was reduced to crumbs"
        );
        assert_eq!(fragments.iter().map(|f| f.count).sum::<u32>(), detached);
        assert_eq!(
            fragments.iter().map(|f| f.mass_g).sum::<u64>(),
            u64::from(detached) * 7850
        );
        let mut cells = std::collections::BTreeSet::new();
        for f in fragments {
            let chunk = f.chunk.as_ref().unwrap();
            assert_eq!(chunk.cells.len(), f.count as usize);
            assert_eq!(chunk.frames[0].base, a.units[0].pose.base);
            for c in &chunk.cells {
                assert!(cells.insert(*c));
            }
        }
        assert_eq!(
            initial,
            a.units[0].body.solid_count() + a.units[0].removed.len() as u32
        );
        let restored = BlastRange::restore(b.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(
            a.state_hash(),
            restored.state_hash(),
            "saved whole fragment"
        );
    }
    /// The owner's report: craters came out square (ADR-273). A shell into
    /// flat soil must dig a crater whose rim reaches as far along the
    /// diagonals as along the axes.
    #[test]
    fn a_shell_crater_in_flat_soil_is_round() {
        let mut r = BlastRange::new(config());
        // Flat lane soil short of the concrete target and the steel revetment.
        r.aim = Some([200, FLOOR, 336]);
        for _ in 0..600 {
            r.step();
            if r.impact.is_some() && r.ready() {
                break;
            }
        }
        let at = r.impact.expect("the shell never landed");
        let column: Vec<u16> = (0..=FLOOR + 2)
            .map(|y| material(&r.world, [at[0], y, at[2]]))
            .collect();
        eprintln!("impact {at:?} removed {} column {column:?}", r.removed);
        let reach_at = |layer: i32, dx: i32, dz: i32| {
            (1..40)
                .take_while(|&k| material(&r.world, [at[0] + dx * k, layer, at[2] + dz * k]) == 0)
                .count() as i64
        };
        // The widest layer: the rim. Across the lane the rock-strewn shoulders
        // clip the bowl, so the lane axis and the four diagonals are compared;
        // the diagonals stay inside the lane at this radius.
        let layer = (at[1] - 8..=at[1].min(FLOOR - 1))
            .max_by_key(|&y| reach_at(y, 1, 0) + reach_at(y, -1, 0))
            .unwrap();
        let reach = |dx: i32, dz: i32| reach_at(layer, dx, dz);
        let axes = [(1, 0), (-1, 0)].map(|(dx, dz)| reach(dx, dz) * 1000);
        let diagonals = [(1, 1), (1, -1), (-1, 1), (-1, -1)].map(|(dx, dz)| reach(dx, dz) * 1414);
        let radii: Vec<i64> = axes.into_iter().chain(diagonals).collect();
        let (low, high) = (
            radii.iter().copied().min().unwrap(),
            radii.iter().copied().max().unwrap(),
        );
        eprintln!(
            "crater layer {layer} radii (thousandths): axes {axes:?} diagonals {diagonals:?}"
        );
        assert!(low >= 3000, "no crater to speak of: {radii:?}");
        // One diagonal cell is 1.41 units, so single rays quantise coarsely;
        // the means must agree closely, every ray roughly. Before ADR-273
        // the diagonals ran a quarter longer than the axes.
        let axis = axes.iter().sum::<i64>() / axes.len() as i64;
        let diagonal = diagonals.iter().sum::<i64>() / diagonals.len() as i64;
        assert!(
            (axis - diagonal).abs() * 100 <= axis * 12,
            "the crater is not round: axes {axis} diagonals {diagonal}"
        );
        assert!(high * 100 <= low * 125, "a ray is far off: {radii:?}");
    }
    /// Trees are matter a tracked hull pushes over (ADR-270): the cut is paid
    /// by the vehicle, the crown falls for free, and nothing is erased.
    /// The default range as the capture script drives it: the player's
    /// tank is sent from its start into the far woods with the patrols
    /// running. Its first turn happens beside two boulders that no planner
    /// sample sees at the heading the executor reaches; it must leave that
    /// spot on its own and arrive (ADR-274).
    #[test]
    fn the_player_tank_reaches_the_far_woods_from_the_start() {
        let c = config();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c, layout).unwrap();
        a.control();
        assert!(a.order_move([96, 540]));
        let mut still = 0;
        let mut longest_still = 0;
        let mut last = [a.gun.motion.pos[0], a.gun.motion.pos[2]];
        let mut arrived = None;
        for tick in 0..1500 {
            a.step();
            let m = &a.gun.motion;
            let here = [m.pos[0], m.pos[2]];
            if here == last {
                still += 1;
                longest_still = longest_still.max(still);
            } else {
                still = 0;
            }
            last = here;
            if m.destination.is_none() {
                arrived = Some(tick);
                break;
            }
        }
        eprintln!("arrived {arrived:?} longest stand {longest_still} ticks");
        assert!(
            arrived.is_some(),
            "stuck at {:?} for {longest_still} ticks",
            a.gun.motion.pos
        );
        assert!(longest_still <= 90, "stood {longest_still} ticks");
    }
    fn shielded_layout() -> RangeLayout {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.airburst_on_target = false;
        layout.shields.push(RangeShieldDef {
            at: [layout.target[0], FLOOR, layout.target[2]],
            radius: 40,
        });
        layout
    }
    fn caught(a: &mut BlastRange) -> Option<ShieldHit> {
        let hits = a.shields[0].hits;
        for _ in 0..300 {
            a.step();
            if let Some(h) = a.shield_hits.first() {
                return Some(*h);
            }
            if a.shields[0].hits > hits || a.impact.is_some() {
                break;
            }
        }
        None
    }
    /// A dome over the target catches the round at its surface: no impact,
    /// no crater, and the weapon's wave energy leaves the charge (ADR-275).
    #[test]
    fn a_dome_stops_the_round_at_its_surface_and_pays_with_charge() {
        let mut a = BlastRange::with_layout(config(), shielded_layout()).unwrap();
        let capacity = a.shields[0].charge;
        let h = caught(&mut a).expect("the round never met the dome");
        assert!(
            a.impact.is_none() && a.explosion.is_none(),
            "the round exploded"
        );
        assert_eq!((a.removed, a.shell), (0, None));
        let s = &a.shields[0];
        assert_eq!(
            (s.hits, s.charge, s.down, h.collapsed, h.drained),
            (1, capacity - a.config.energy, 0, false, a.config.energy)
        );
        let d2: i64 = (0..3).map(|k| i64::from(h.at[k] - s.at[k]).pow(2)).sum();
        let r = i64::from(s.radius);
        assert!(
            (d2 - r * r).abs() <= 4 * r,
            "caught at {:?}, not on a dome of radius {r}",
            h.at
        );
        assert!(a.ready(), "the spent round left the gun busy");
    }
    /// Both weapons drain one dome by their own wave energy: the HE round
    /// leaves less than the energy bolt needs, so the bolt collapses the
    /// dome, a third round passes while it is down, and the charge climbs
    /// back at the tuned rate once it stands again.
    #[test]
    fn two_weapons_on_one_dome_drain_it_and_the_second_collapses_it() {
        let mut a = BlastRange::with_layout(config(), shielded_layout()).unwrap();
        a.control();
        let tuning = *a.shield_tuning();
        let he = config().energy;
        let bolt =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        assert!(
            he < tuning.capacity && he + bolt.energy > tuning.capacity,
            "shield.ron no longer holds one round of each weapon and not two"
        );
        assert!(a.refire());
        let first = caught(&mut a).expect("the HE round passed the dome");
        assert!(!first.collapsed);
        assert_eq!(a.shields[0].charge, tuning.capacity - he);
        let bolt_energy = bolt.energy;
        a.config = bolt;
        assert!(a.refire());
        let second = caught(&mut a).expect("the bolt passed the dome");
        let s = &a.shields[0];
        assert!(second.collapsed && s.down > 0 && s.charge == 0, "{s:?}");
        // The dome recharged a little while the bolt was on its way, but
        // never enough to hold it.
        assert_eq!(s.hits, 2);
        assert!(
            second.drained >= tuning.capacity - he && second.drained < bolt_energy,
            "drained {} of a dome recharging from {}",
            second.drained,
            tuning.capacity - he
        );
        assert!(a.impact.is_none());
        assert!(a.refire());
        assert!(
            caught(&mut a).is_none(),
            "the collapsed dome caught a round"
        );
        assert!(
            a.impact.is_some(),
            "the round neither hit the dome nor the ground"
        );
        assert_eq!(a.shields[0].hits, 2);
        while a.shields[0].down > 0 {
            a.step();
        }
        for _ in 0..20 {
            a.step();
        }
        assert_eq!(a.shields[0].charge, 20 * tuning.recharge_per_tick);
    }
    /// Dome state is saved and hashed: a range restored after a catch
    /// matches tick for tick, and a spent point of charge changes the hash.
    #[test]
    fn dome_charge_and_outage_are_saved_and_hashed() {
        let mut a = BlastRange::with_layout(config(), shielded_layout()).unwrap();
        assert!(caught(&mut a).is_some());
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
        for _ in 0..60 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        assert_eq!(a.shields[0].charge, b.shields[0].charge);
        let mut c = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        c.shields[0].charge -= 1;
        assert_ne!(a.state_hash(), c.state_hash());
        let mut wide = shielded_layout();
        wide.shields[0].radius = 200;
        assert!(BlastRange::with_layout(config(), wide).is_err());
    }
    #[test]
    fn a_tank_pushes_through_the_forest_and_the_trees_fall_as_debris() {
        let c = config();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        a.control();
        a.patrols_running = false;
        assert!(
            a.order_move([128, 470]),
            "no route into the woods from {:?}",
            a.gun.motion.pos
        );
        let mut b: Option<BlastRange> = None;
        let mut cut_ticks = 0;
        let mut released = 0;
        let mut blocked_ticks = 0;
        let mut reversed = false;
        let mut travelled = Fx::ZERO;
        let mut arrived = None;
        for tick in 0..1200 {
            let before = a.gun.motion.pos;
            a.step();
            if let Some(r) = b.as_mut() {
                r.step();
                assert_eq!(a.state_hash(), r.state_hash(), "tick {tick}");
            }
            let m = &a.gun.motion;
            let dx = m.pos[0].sub(before[0]);
            let dz = m.pos[2].sub(before[2]);
            travelled = travelled.add(dx.mul(dx).add(dz.mul(dz)).sqrt());
            blocked_ticks += usize::from(m.blocked);
            reversed |= m.drive.vel < Fx::ZERO;
            if !m.cut.is_empty() {
                cut_ticks += 1;
                if b.is_none() {
                    b = Some(BlastRange::restore(c.clone(), &a.snapshot()).unwrap());
                }
            }
            released = released.max(a.matter.felling_pending());
            assert!(a.matter.economy.ledger.balanced());
            if m.destination.is_none() {
                arrived.get_or_insert(tick);
                if a.matter.felling_pending() == 0 {
                    break;
                }
            }
        }
        let wood = a.removed_materials[4] + a.removed_materials[5];
        eprintln!(
            "arrived {arrived:?} ticks {} travelled {} blocked {blocked_ticks} reversed {reversed} \
             cut_ticks {cut_ticks} frontier_max {released} foliage {} trunk {} spent_mj {} fragments {}",
            a.tick,
            travelled.to_int_trunc(),
            a.removed_materials[4],
            a.removed_materials[5],
            a.matter.spent_mj,
            a.matter.debris.fragments.len()
        );
        assert!(
            a.gun.motion.destination.is_none(),
            "stuck at {:?} blocked {}",
            a.gun.motion.pos,
            a.gun.motion.blocked
        );
        assert!(!reversed, "the hull backed off a tree");
        assert!(
            cut_ticks > 0 && released > 0,
            "cut {cut_ticks} released {released}"
        );
        assert!(wood > 200 && a.removed_materials[5] > 0, "felled {wood}");
        assert_eq!(a.matter.economy.ledger.sunk_total(), 0);
        let m = &a.matter;
        assert_eq!(
            m.debris
                .fragments
                .iter()
                .map(|f| u64::from(f.count))
                .sum::<u64>(),
            a.removed as u64
        );
        assert!(m.spent_mj <= m.supplied_mj && m.spent_mj > 0);
        // Nothing yielding is left standing inside the hull where it stopped.
        let shape = a.gun.motion.shape(&a.gun.body);
        let mut probe = nav::vehicle::Probe::with_yield(&a.world, &a.navigation.fell.materials);
        assert!(probe
            .crushed(
                [a.gun.motion.pos[0], a.gun.motion.pos[2]],
                a.gun.motion.drive.head,
                shape
            )
            .is_empty());
        assert_eq!(
            a.state_hash(),
            BlastRange::restore(c, &a.snapshot()).unwrap().state_hash()
        );
    }
    #[test]
    fn a_walker_still_treats_trees_as_walls() {
        let c = config();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c, layout).unwrap();
        a.patrols_running = false;
        let id = a
            .units
            .iter()
            .position(|u| u.kind == RangeUnitKind::Strider)
            .expect("a strider on the range");
        let body = a.units[id].body.clone();
        let others: Vec<_> = a
            .units
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != id)
            .map(|(_, u)| motion::Obstacle::of(&u.motion, &u.body))
            .collect();
        a.units[id].motion.route.clear();
        a.units[id]
            .motion
            .move_to(&a.world, [128, 470], &body, &others);
        for _ in 0..600 {
            a.step();
            let u = &a.units[id];
            let mut probe = nav::vehicle::Probe::with_yield(&a.world, &a.navigation.fell.materials);
            assert!(
                probe
                    .crushed(
                        [u.motion.pos[0], u.motion.pos[2]],
                        u.motion.drive.head,
                        u.motion.shape(&body)
                    )
                    .is_empty(),
                "the walker stands inside a tree at {:?}",
                u.motion.pos
            );
        }
        assert_eq!(a.removed_materials[4] + a.removed_materials[5], 0);
    }
    #[test]
    fn a_waiting_section_does_not_freeze_damage_elsewhere_on_the_body() {
        let c = config();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        a.control();
        a.patrols_running = false;
        let u = &mut a.units[0];
        for z in 0..30 {
            for y in 0..30 {
                if u.body.remove(54, y, z) {
                    u.removed.push([54, y as u8, z as u8]);
                }
            }
        }
        u.revision = u.removed.len() as u64;
        a.matter.annihilate_body(0, u.removed.len());
        u.refresh_damage(0);
        let detached = u.status.detached;
        assert!(u.status.alive && detached > 8);
        a.matter.tuning.max_flying = 0;
        a.matter.step_sections(&mut a.units);
        assert!(a.units[0].section.as_ref().unwrap().ready());
        // A hull cell outside the pending section still takes the impulse.
        let u = &a.units[0];
        let job = u.section.as_ref().unwrap();
        let hull = (0..u.body.vol.nx as usize)
            .flat_map(|x| (0..u.body.vol.ny as usize).map(move |y| [x, y, 15]))
            .find(|&[x, y, z]| {
                u.body.vol.get(x, y, z)
                    && !job.holds([x as u8, y as u8, z as u8])
                    && u.body.part_at(x, y, z) == voxel::Part::Armour
            })
            .expect("an armour cell that stays with the hull");
        let after_cut = u.body.solid_count();
        a.units[0].receive_cell(hull, 1_000_000, &c);
        a.matter.annihilate_body(0, 1);
        a.units[0].refresh_damage(0);
        assert_eq!(a.units[0].body.solid_count(), after_cut - 1);
        assert!(a.units[0].status.alive && a.units[0].section.is_some());
        let restored = BlastRange::restore(c.clone(), &a.snapshot()).unwrap();
        assert_eq!(a.state_hash(), restored.state_hash());
        a.matter.tuning.max_flying = restored.matter.tuning.max_flying;
        for _ in 0..40 {
            a.step();
            if a.units[0].section.is_none() {
                break;
            }
        }
        let u = &a.units[0];
        assert!(u.section.is_none());
        assert!(
            !u.body.vol.get(hull[0], hull[1], hull[2]),
            "commit resurrected the hull cell"
        );
        assert_eq!(u.body.solid_count(), after_cut - 1 - detached);
        assert_eq!(u.status.detached, 0);
        assert_eq!(
            a.matter
                .debris
                .fragments
                .iter()
                .map(|f| f.count)
                .sum::<u32>(),
            detached
        );
        assert!(a.matter.economy.ledger.balanced());
        assert_eq!(
            a.state_hash(),
            BlastRange::restore(c, &a.snapshot()).unwrap().state_hash()
        );
    }
    /// The owner's report: a directed-energy hit on a moving tank kept eating
    /// it for almost three seconds after the flash. The pieces cut loose were
    /// leaving one per tick while the wave waited on them (ADR-269).
    #[test]
    fn damage_arrives_with_the_wave_while_sections_are_still_leaving() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        a.control();
        let mut b: Option<BlastRange> = None;
        // The opening round lands on the default target; `hit` is the impact
        // of the round that actually struck the moving tank.
        let mut current = None;
        let mut hit = None;
        let mut first = None;
        let mut last = None;
        let mut death = None;
        let mut carried = 0;
        let mut stalled = 0;
        let mut previous_examined = 0;
        let mut last_removed = 0;
        for tick in 0..600u32 {
            if tick == 20 {
                a.order_move([235, 365]);
            }
            if tick == 220 {
                assert!(a.order_attack(2));
            }
            // One round only: the reload would otherwise land a second hit.
            if hit.is_some_and(|t| tick == t + 50) {
                break;
            }
            if hit.is_some() {
                carried += usize::from(a.units[2].section.is_some());
            }
            a.step();
            if let Some(r) = b.as_mut() {
                r.step();
                assert_eq!(a.state_hash(), r.state_hash(), "tick {tick}");
            }
            if a.impact.is_some() && current != Some(a.impact_tick) {
                current = Some(a.impact_tick);
                previous_examined = 0;
            }
            let u = &a.units[2];
            if u.removed.len() > last_removed {
                if hit.is_none() {
                    hit = current;
                    first = Some(tick);
                }
                last = Some(tick);
                last_removed = u.removed.len();
            }
            if !u.status.alive {
                death.get_or_insert(tick);
            }
            let examined = a.explosion.as_ref().map_or(0, |e| e.wave.examined);
            let finished = a.explosion.as_ref().is_none_or(|e| e.wave.finished());
            if hit.is_some() && !finished && examined == previous_examined {
                stalled += 1;
            }
            previous_examined = examined;
            if b.is_none() && hit.is_some_and(|t| tick == t + 2) {
                b = Some(BlastRange::restore(c.clone(), &a.snapshot()).unwrap());
            }
            assert!(a.matter.economy.ledger.balanced());
        }
        let (impact, first, last) = (hit.unwrap(), first.unwrap(), last.unwrap());
        let u = &a.units[2];
        eprintln!(
            "impact {impact} first {first} death {death:?} last {last} stalled {stalled} \
             carried {carried} removed {} detached {} finished {}",
            u.removed.len(),
            u.detached,
            a.explosion.as_ref().is_none_or(|e| e.wave.finished())
        );
        assert!(b.is_some(), "the replay never forked");
        assert_eq!(stalled, 0, "the wave paused for debris preparation");
        assert_eq!(first, impact);
        // Eight ticks is 0.27 s: the wave budget in energy-cannon.ron sweeps
        // the whole body within that, and nothing else may add to it. Where
        // the round lands on the moving hull depends on the drive model, so
        // the core is not always reached; when it is, it dies inside the same
        // window, and the hit is always a real one.
        assert!(
            last - impact <= 8 && death.is_none_or(|d| d - impact <= 8),
            "impact {impact} first {first} death {death:?} last {last}: destruction lagged the flash"
        );
        assert!(
            u.removed.len() > 2000,
            "a glancing hit: {} cells",
            u.removed.len()
        );
        assert!(u.detached > 0 && u.annihilated_mm3() > 0);
        // Pieces cut loose leave in the tick that frees them; nothing lingers.
        assert!(carried <= 2, "sections waited {carried} ticks");
    }
    #[test]
    fn material_shell_transfers_ground_and_armour_with_finite_energy_and_exact_resume() {
        let c = config();
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.target = [359, 28, 312];
        layout.airburst_on_target = false;
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        for _ in 0..300 {
            a.step();
            if a.units[0].detached > 0 {
                break;
            }
        }
        assert!(
            a.units[0].detached > 0,
            "body export path was not exercised"
        );
        let mut b = BlastRange::restore(c, &a.snapshot()).unwrap();
        for _ in 0..300 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
            let m = &a.matter;
            assert!(m.economy.ledger.balanced());
            assert_eq!(m.economy.ledger.sunk_total(), 0);
            assert!(m.spent_mj <= m.supplied_mj);
            assert!(
                m.debris.motion_mj + m.debris.body_motion_mj
                    <= m.spent_mj * u64::from(m.tuning.motion_pct) / 100
            );
            let body = a.units.iter().map(|u| u.detached as u64).sum::<u64>();
            assert_eq!(
                m.debris
                    .fragments
                    .iter()
                    .map(|f| u64::from(f.count))
                    .sum::<u64>(),
                a.removed as u64 + body
            );
            let ground_mass = a
                .removed_materials
                .iter()
                .enumerate()
                .map(|(i, &n)| {
                    m.economy.mats.get(i as u16).map_or(0, |d| {
                        n as u64
                            * u64::from(d.density)
                            * u64::from(m.tuning.density_kg_m3_per_density)
                    })
                })
                .sum::<u64>();
            assert_eq!(
                m.debris.fragments.iter().map(|f| f.mass_g).sum::<u64>(),
                ground_mass + body * 7850
            );
        }
        assert!(
            a.matter.debris.fragments.iter().any(|f| f.resting),
            "ground={} body={} first={:?}",
            a.removed,
            a.units[0].detached,
            a.matter
                .debris
                .fragments
                .first()
                .map(|f| (f.position, f.velocity))
        );
        assert_eq!(a.units[0].annihilated_mm3(), 0);
        assert!(
            a.units[1].removed.is_empty(),
            "the wall must still protect the rear tank"
        );
    }
    #[test]
    fn small_fragment_pool_defers_the_wave_without_discarding_matter() {
        let mut r = BlastRange::new(config());
        r.matter.tuning.max_flying = 16;
        let mut stalled = false;
        for _ in 0..6000 {
            let before = r.removed;
            r.step();
            let m = &r.matter;
            assert!(m.debris.fragments.iter().filter(|f| !f.resting).count() <= 16);
            assert_eq!(
                m.debris
                    .fragments
                    .iter()
                    .map(|f| u64::from(f.count))
                    .sum::<u64>(),
                r.removed as u64
            );
            assert!(m.economy.ledger.balanced());
            stalled |=
                r.explosion.as_ref().is_some_and(|e| !e.pending.is_empty()) && r.removed == before;
            if r.impact.is_some() && r.ready() {
                break;
            }
        }
        assert!(
            stalled && r.ready() && r.removed > 100,
            "stalled={stalled} ready={} removed={} moving={} first={:?}",
            r.ready(),
            r.removed,
            r.matter
                .debris
                .fragments
                .iter()
                .filter(|f| !f.resting)
                .count(),
            r.matter
                .debris
                .fragments
                .first()
                .map(|f| (f.position, f.velocity))
        );
        let mut ordinary = BlastRange::new(config());
        for _ in 0..500 {
            ordinary.step();
        }
        assert_eq!(r.removed_materials, ordinary.removed_materials);
        assert_eq!(r.matter.spent_mj, ordinary.matter.spent_mj);
    }
    #[test]
    fn switching_to_annihilation_keeps_existing_physical_debris() {
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut r = BlastRange::with_layout(config(), layout).unwrap();
        for _ in 0..400 {
            r.step();
        }
        let count = r
            .matter
            .debris
            .fragments
            .iter()
            .map(|f| u64::from(f.count))
            .sum::<u64>();
        let detached_before = r.units.iter().map(|u| u.detached as u64).sum::<u64>();
        r.config =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        r.aim = Some([359, 28, 312]);
        assert!(r.refire());
        for _ in 0..200 {
            r.step();
        }
        assert_eq!(
            r.matter
                .debris
                .fragments
                .iter()
                .map(|f| u64::from(f.count))
                .sum::<u64>(),
            count + r.units.iter().map(|u| u.detached as u64).sum::<u64>() - detached_before
        );
        assert!(r.matter.economy.ledger.sunk_total() > 0 && r.matter.economy.ledger.balanced());
        assert_eq!(
            r.state_hash(),
            BlastRange::restore(r.config.clone(), &r.snapshot())
                .unwrap()
                .state_hash()
        );
    }
    #[test]
    fn patrols_complete_two_laps_and_movement_resumes_exactly() {
        let mut a = BlastRange::with_layout(
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap(),
        )
        .unwrap();
        a.control();
        let mut highest = [16; 2];
        let mut tilted = false;
        for _ in 0..9000 {
            a.step();
            for (i, high) in highest.iter_mut().enumerate() {
                *high = (*high).max(a.units[i + 2].at[1]);
            }
            tilted |= a.units[2].motion.hull.up.x.abs() > Fx::from_ratio(1, 100);
            if a.units[2..].iter().all(|u| u.motion.laps >= 2) {
                break;
            }
        }
        for u in &a.units[2..] {
            assert!(
                u.motion.laps >= 2,
                "kind {:?} at {:?} laps {} blocked {} waypoint {} cursor {}/{} head {} path {:?}",
                u.kind,
                u.at,
                u.motion.laps,
                u.motion.blocked,
                u.motion.waypoint,
                u.motion.cursor,
                u.motion.path.len(),
                u.motion.drive.head,
                u.motion.path
            );
        }
        assert!(
            highest[0] > 20 && highest[1] > 19 && tilted,
            "ramps {highest:?}"
        );
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        for _ in 0..100 {
            a.step();
            b.step();
        }
        assert_eq!(a.state_hash(), b.state_hash());
        assert!(a.order_move([245, 340]));
        for _ in 0..300 {
            a.step();
        }
        assert!(
            (a.gun.at[0] - 245).abs() < 8,
            "gun {:?} motion {:?}",
            a.gun.at,
            a.gun.motion
        );
        assert!(a.order_attack(2));
        let removed = a.units[2].removed.len();
        let mut impacted = false;
        for _ in 0..240 {
            a.step();
            impacted |= a.impact.is_some();
        }
        assert!(impacted, "attack never produced an impact");
        assert!(
            a.units[2].removed.len() > removed,
            "moving target must take spatial damage"
        );
    }
    #[test]
    fn off_axis_aim_rotates_muzzle_without_increasing_horizontal_speed_and_resumes() {
        let mut c =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        c.fire_tick = 0;
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.blocks.clear();
        layout.units.clear();
        layout.course = None;
        layout.target = [450, 24, 380];
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        assert!(a.muzzle()[2] > Fx::from_int(MUZZLE[2]));
        assert!(a.muzzle()[0] < Fx::from_int(MUZZLE[0]));
        a.step();
        let speed = a.velocity[0]
            .mul(a.velocity[0])
            .add(a.velocity[2].mul(a.velocity[2]))
            .sqrt();
        assert!(speed.sub(Fx::from_int(c.shell_speed)).abs() < Fx::from_ratio(1, 1000));
        let bytes = a.snapshot();
        let mut b = BlastRange::restore(c, &bytes).unwrap();
        for _ in 0..150 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        assert!(a.impact.is_some());
    }
    #[test]
    fn steel_receives_work_and_repeated_rounds_erode_its_front_without_immunity() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut r = BlastRange::with_layout(c.clone(), layout).unwrap();
        for _ in 0..250 {
            r.step();
        }
        let steel: Vec<_> = r
            .surface_work
            .iter()
            .filter(|(p, _)| material(&r.world, **p) == 3)
            .map(|(p, w)| (*p, *w))
            .collect();
        assert!(steel.len() > 100, "steel did not receive surface work");
        assert_eq!(r.removed_materials[3], 0);
        assert!(!r.units[0].dents.is_empty());
        assert!(r.units[1].dents.is_empty() && r.units[1].removed.is_empty());
        // Work is spatial and remains part of the snapshot, not a global HP bar.
        for (p, _) in &steel {
            assert!(p[0] < 400);
        }
        for _ in 0..6 {
            assert!(r.refire());
            assert!(!r.refire(), "a charging shot was overwritten");
            let bytes = r.snapshot();
            let mut resumed = BlastRange::restore(c.clone(), &bytes).unwrap();
            for _ in 0..160 {
                r.step();
                resumed.step();
            }
            assert_eq!(r.state_hash(), resumed.state_hash());
        }
        assert!(
            r.removed_materials[3] > 0,
            "steel remained immune to repeated work"
        );
        assert!(
            r.units[1].removed.is_empty(),
            "small front scars punched through the thick core"
        );
    }
    #[test]
    fn stronger_round_breaks_steel_and_work_stays_local_on_the_body() {
        let mut c =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        c.energy *= 4;
        let mut r = BlastRange::with_layout(c.clone(), layout).unwrap();
        for _ in 0..300 {
            r.step();
        }
        assert!(r.removed_materials[3] > 0);
        let mut u = RangeUnit::new([100, 16, 100]);
        let mut chosen = None;
        for z in 0..u.body.vol.nz as usize {
            for y in 0..u.body.vol.ny as usize {
                for x in 0..u.body.vol.nx as usize {
                    if u.body.vol.get(x, y, z) && u.body.part_at(x, y, z) == voxel::Part::Armour {
                        chosen = Some([x, y, z]);
                    }
                }
            }
        }
        let p = chosen.unwrap();
        let q = u.world_pos(p);
        let cost = unit_work(&u.body, p, &c);
        u.receive(q, cost / 2, &c);
        assert!(u.solid(q));
        assert_eq!(u.dents.len(), 1);
        assert!(u.removed.is_empty());
        u.receive(q, cost - cost / 2, &c);
        assert!(!u.solid(q));
        assert_eq!(u.removed.len(), 1);
        assert!(u.dents.is_empty());
    }
    #[test]
    fn weapon_switch_restores_the_original_generator_and_roster() {
        let energy =
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        for (initial, current) in [(config(), energy.clone()), (energy, config())] {
            let mut a = BlastRange::new(initial);
            for _ in 0..200 {
                a.step();
            }
            a.config = current;
            let b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
            assert_eq!(a.state_hash(), b.state_hash());
            assert_eq!(a.units.len(), b.units.len());
            for x in [234, 258, 340, 394, 418] {
                assert_eq!(a.world.material(x, 30, 336), b.world.material(x, 30, 336));
            }
        }
    }

    #[test]
    fn a_single_fragment_slot_progresses_and_resumes_inside_a_contact() {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.target = [359, 28, 312];
        layout.airburst_on_target = false;
        let c = config();
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        a.matter.tuning.max_flying = 1;
        for _ in 0..400 {
            a.step();
            if a.explosion.as_ref().is_some_and(|e| e.contact_cursor > 0) {
                break;
            }
        }
        assert!(a.explosion.as_ref().is_some_and(|e| e.contact_cursor > 0));
        assert_eq!(a.matter.debris.fragments.len(), 1);
        let mut b = BlastRange::restore(c, &a.snapshot()).unwrap();
        b.matter.tuning.max_flying = 1;
        for _ in 0..300 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
            assert!(a.matter.economy.ledger.balanced());
            assert!(
                a.matter
                    .debris
                    .fragments
                    .iter()
                    .filter(|f| !f.resting)
                    .count()
                    <= 1
            );
        }
        assert!(
            a.matter
                .debris
                .fragments
                .iter()
                .map(|f| f.count)
                .sum::<u32>()
                > 1
        );
    }

    #[test]
    fn failed_move_retries_after_obstacle_leaves_and_stop_cancels_it() {
        let mut a = BlastRange::new(config());
        let goal = [180, 336];
        let obstacle = motion::Obstacle {
            pos: [
                Fx::from_int(goal[0]),
                Fx::from_int(FLOOR),
                Fx::from_int(goal[1]),
            ],
            head: 0,
            half: [Fx::from_int(32); 2],
        };
        assert!(!a
            .gun
            .motion
            .move_to(&a.world, goal, &a.gun.body, &[obstacle]));
        assert!(a.gun.motion.path.is_empty());
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        b.gun.motion.step(
            &b.world,
            &b.gun.body,
            true,
            true,
            b.gun.mobility,
            &[],
            b.gun.motion.replan,
            b.config.gravity,
        );
        assert!(!b.gun.motion.path.is_empty());
        a.gun.motion.stop();
        a.gun.motion.step(
            &a.world,
            &a.gun.body,
            true,
            true,
            a.gun.mobility,
            &[],
            a.gun.motion.replan,
            a.config.gravity,
        );
        assert!(a.gun.motion.path.is_empty());
        assert_eq!(a.gun.motion.destination, None);
    }

    fn config() -> ExplosionConfig {
        ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap()
    }
    #[test]
    fn shell_impacts_destroys_target_and_leaves_the_barrier() {
        let mut r = BlastRange::new(config());
        for _ in 0..150 {
            r.step();
        }
        assert!(r.impact.is_some());
        assert!(r.removed > 100, "no real destruction: {}", r.removed);
        assert_eq!(
            material(&r.world, [258, 30, 336]),
            3,
            "resistant wall destroyed"
        );
        let at = r.impact.unwrap();
        assert_eq!(material(&r.world, at), 0, "impact did not open the target");
    }
    #[test]
    fn different_work_budgets_produce_the_same_crater() {
        let mut a = BlastRange::new(config());
        let mut c = config();
        c.wave_work_per_tick = 257;
        let mut b = BlastRange::new(c);
        for _ in 0..300 {
            a.step();
            b.step();
        }
        assert_eq!(a.impact, b.impact);
        assert_eq!(a.removed, b.removed);
        for z in 316..358 {
            for y in 0..50 {
                for x in 205..270 {
                    assert_eq!(a.world.material(x, y, z), b.world.material(x, y, z));
                }
            }
        }
    }
    #[test]
    fn save_during_propagation_matches_uninterrupted_run() {
        let mut a = BlastRange::new(config());
        while a.impact.is_none() {
            a.step();
        }
        let mut b = BlastRange::restore(config(), &a.snapshot()).unwrap();
        for _ in 0..80 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
    }

    #[test]
    fn energy_bolt_flies_straight_hits_and_resumes() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut a = BlastRange::new(c.clone());
        for _ in 0..=c.fire_tick {
            a.step();
        }
        let velocity = a.velocity;
        let mut b = BlastRange::restore(c, &a.snapshot()).unwrap();
        for _ in 0..150 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
            if a.shell.is_some() {
                assert_eq!(a.velocity, velocity, "energy bolt fell under gravity");
            }
        }
        assert_eq!(a.impact.unwrap()[0], 392, "bolt skipped the target face");
        assert!(a.removed > 100 && a.world.queued() == 0);
        assert_eq!(material(&a.world, [418, 30, 336]), 3);
        assert!(
            a.impact_tick - a.config.fire_tick <= 6,
            "combat shot is still slowed down"
        );
        let u = a.units.first().unwrap();
        assert!(u.removed.len() > 1000 && u.body.solid_count() > 10000);
        assert!(
            u.status.alive,
            "the witness should survive with visible flank damage"
        );
        let mm3 = u64::from(u.body.mm).pow(3);
        assert_eq!(
            (u.body.solid_count() as u64 + u.detached as u64) * mm3 + u.annihilated_mm3(),
            voxel::unit::tank().solid_count() as u64 * mm3
        );
    }

    #[test]
    fn witness_damage_resumes_mid_wave_and_is_budget_independent() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut a = BlastRange::new(c.clone());
        while a.units.first().unwrap().removed.is_empty() {
            a.step();
        }
        let mut b = BlastRange::restore(c.clone(), &a.snapshot()).unwrap();
        for _ in 0..90 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        let mut slow = c;
        slow.wave_work_per_tick = 257;
        let mut other = BlastRange::new(slow);
        for _ in 0..400 {
            let before = other.units.first().unwrap().removed.len();
            other.step();
            assert!(other.units.first().unwrap().removed.len() - before <= 257 * 8);
        }
        let x = &a.units.first().unwrap().body.vol;
        let y = &other.units.first().unwrap().body.vol;
        assert_eq!(x.cols, y.cols, "work slicing changed armour damage");
        assert_eq!(a.removed, other.removed);
    }

    #[test]
    fn resistant_wall_shields_the_witness() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut r = BlastRange::new(c);
        // Body begins behind the steel revetment; its near side is inside the
        // wave domain, so this exercises occlusion, not simply out-of-range.
        r.units = vec![RangeUnit::new([449, FLOOR, 336])];
        for _ in 0..250 {
            r.step();
        }
        assert!(
            r.units.first().unwrap().removed.is_empty(),
            "ablation ignored steel shielding"
        );
    }

    #[test]
    fn cover_chips_but_protects_the_rear_unit_and_open_control_damages_it() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c.clone(), layout.clone()).unwrap();
        let mut open = layout;
        open.blocks.clear();
        let mut b = BlastRange::with_layout(c.clone(), open).unwrap();
        for _ in 0..300 {
            a.step();
            b.step();
        }
        assert_eq!(
            a.impact, b.impact,
            "cover control changed the point of detonation"
        );
        assert!(a.removed_materials[1] > 5000 && a.removed_materials[2] > 100);
        assert_eq!(
            a.removed_materials[3], 0,
            "standard shot destroyed the steel core"
        );
        assert!(a.units[0].removed.len() > 1000);
        assert!(a.units[1].removed.is_empty() && a.units[1].status.alive);
        assert!(
            b.units[1].removed.len() > 1000,
            "sheltered body was just outside the blast"
        );
        for y in 0..60 {
            for z in 282..390 {
                assert_eq!(material(&a.world, [400, y, z]), 3, "hole through the core");
            }
        }
    }

    #[test]
    fn cover_layout_and_every_body_resume_during_damage() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        let mut a = BlastRange::with_layout(c.clone(), layout.clone()).unwrap();
        while a.units[0].removed.is_empty() {
            a.step();
        }
        let mut b = BlastRange::restore(c.clone(), &a.snapshot()).unwrap();
        for _ in 0..50 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        let mut slow = c;
        slow.wave_work_per_tick = 257;
        let mut other = BlastRange::with_layout(slow, layout).unwrap();
        for _ in 0..400 {
            other.step();
        }
        assert_eq!(a.removed_materials, other.removed_materials);
        for (a, b) in a.units.iter().zip(&other.units) {
            assert_eq!(a.body.vol.cols, b.body.vol.cols);
        }
    }

    #[test]
    fn range_rejects_embedded_units_and_accepts_alternative_body() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        layout.units[0].at = [400, FLOOR, 336];
        assert!(BlastRange::with_layout(c.clone(), layout.clone()).is_err());
        layout.units[0].at = [359, FLOOR, 312];
        layout.units[0].kind = RangeUnitKind::Strider;
        let mut a = BlastRange::with_layout(c.clone(), layout).unwrap();
        for _ in 0..200 {
            a.step();
        }
        assert!(!a.units[0].removed.is_empty());
        assert_eq!(
            a.state_hash(),
            BlastRange::restore(c, &a.snapshot()).unwrap().state_hash()
        );
    }
}
