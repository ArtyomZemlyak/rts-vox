//! Authoritative per-unit voxel bodies, shared until the first physical hit.
//! Unit slots are owned by World; this table never reorders or allocates IDs.
use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

use fx::Fx;
use phys::V3;
use voxel::impact::{Cell, Dents, Penetration};
use voxel::topo::unit::CoreBufs;
use voxel::{Status, UnitBody};
#[path = "detach.rs"]
mod detach;
pub use detach::Separated;

/// Orthonormal hull frame, computed once per unit/phase. Root origin and scale
/// match the renderer; collision uses the same suspension normal and heading.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub base: V3,
    pub x: V3,
    pub y: V3,
    pub z: V3,
    pub scale: Fx,
    pub centre: [Fx; 3],
}

impl Frame {
    pub fn new(body: &UnitBody, base: V3, head: i32, up: V3) -> Self {
        let normal = |v: V3, fallback| {
            let n = v.len_sq();
            if n <= Fx::ZERO {
                fallback
            } else {
                v.scale(Fx::ONE.div(n.sqrt()))
            }
        };
        let y = normal(up, V3::UP);
        let forward = V3::new(fx::angle::cos(head), Fx::ZERO, fx::angle::sin(head));
        let x = normal(
            forward.sub(y.scale(forward.dot(y))),
            V3::new(Fx::ONE, Fx::ZERO, Fx::ZERO),
        );
        Self {
            base,
            x,
            y,
            z: x.cross(y),
            scale: Fx::from_ratio(voxel::unit::WORLD_MM as i32, body.mm as i32),
            centre: [
                Fx::from_ratio(body.vol.nx as i32, 2),
                Fx::ZERO,
                Fx::from_ratio(body.vol.nz as i32, 2),
            ],
        }
    }
    pub fn local(&self, world: V3) -> [Fx; 3] {
        let v = world.sub(self.base);
        let q = [v.dot(self.x), v.dot(self.y), v.dot(self.z)];
        std::array::from_fn(|a| q[a].mul(self.scale).add(self.centre[a]))
    }
    pub fn world(&self, local: [Fx; 3]) -> V3 {
        let v = std::array::from_fn::<_, 3, _>(|a| local[a].sub(self.centre[a]).div(self.scale));
        self.base
            .add(self.x.scale(v[0]))
            .add(self.y.scale(v[1]))
            .add(self.z.scale(v[2]))
    }
}

/// First solid cell and the parameter at which the ray enters it.
pub fn first_hit(body: &UnitBody, from: [Fx; 3], to: [Fx; 3]) -> Option<(Fx, Cell)> {
    first_hit_layers(body, from, to, [0, body.vol.ny])
}
pub fn first_hit_layers(
    body: &UnitBody,
    from: [Fx; 3],
    to: [Fx; 3],
    layers: [u8; 2],
) -> Option<(Fx, Cell)> {
    for cell in voxel::impact::ray_cells([body.vol.nx, body.vol.ny, body.vol.nz], from, to) {
        if cell[1] < layers[0] || cell[1] >= layers[1] {
            continue;
        }
        let [x, y, z] = cell.map(|v| v as usize);
        if !body.vol.get(x, y, z) {
            continue;
        }
        let mut enter = Fx::ZERO;
        for a in 0..3 {
            let d = to[a].sub(from[a]);
            if d == Fx::ZERO {
                continue;
            }
            let edge = cell[a] as i32 + i32::from(d < Fx::ZERO);
            enter = enter.max(Fx::from_int(edge).sub(from[a]).div(d));
        }
        return Some((enter, cell));
    }
    None
}

pub struct BodyState {
    pub template: u16,
    pub generation: u32,
    own: Option<Arc<UnitBody>>,
    pub dents: Dents,
    /// Material removed by penetration. Kept until transferred to a physical
    /// debris container; never silently lost or granted to the player.
    pub removed_mm3: u64,
    /// Removed volume already transferred out of the body container.
    pub debris_mm3: u64,
    pub status: Status,
    pub shape: crate::burst::chunk::Shape,
    /// Derived at confirmed geometry changes and reconstructed on load.
    pub mobility: crate::mobility::Mobility,
    pub revision: u64,
    pub removed: Vec<Cell>,
    separating: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub unit: u32,
    pub generation: u32,
    pub ray: Penetration,
    pub direction: V3,
    pub motion_mj: u64,
}

#[derive(Default, Debug)]
pub struct Change {
    pub unit: u32,
    pub removed: Vec<Cell>,
    pub dented: Vec<Cell>,
    pub spent_mj: u64,
    pub direction: V3,
    pub motion_mj: u64,
}

pub struct Bodies {
    pub templates: Vec<Arc<UnitBody>>,
    template_status: Vec<Status>,
    template_shapes: Vec<crate::burst::chunk::Shape>,
    pub slots: Vec<Option<BodyState>>,
    pub generations: Vec<u32>,
    pub pending: VecDeque<Request>,
    core: CoreBufs,
    detach: VecDeque<detach::Job>,
}

impl Bodies {
    pub fn save(&self, b: &mut fx::blob::Blob) {
        b.u32s(&self.generations);
        b.u32(self.slots.len() as u32);
        for slot in &self.slots {
            b.bool(slot.is_some());
            let Some(s) = slot else {
                continue;
            };
            b.u16(s.template);
            b.u32(s.generation);
            b.bool(s.own.is_some());
            b.u64(s.revision);
            b.u64(s.debris_mm3);
            b.u32(s.removed.len() as u32);
            for c in &s.removed {
                for v in c {
                    b.u8(*v);
                }
            }
            b.u32(s.dents.len() as u32);
            for (c, energy) in &s.dents {
                for v in c {
                    b.u8(*v);
                }
                b.u64(*energy);
            }
        }
        self.save_detach(b);
        b.u32(self.pending.len() as u32);
        for r in &self.pending {
            b.u32(r.unit);
            b.u32(r.generation);
            for v in [r.direction.x, r.direction.y, r.direction.z] {
                b.i64(v.raw());
            }
            b.u64(r.motion_mj);
            b.usize(r.ray.cursor);
            b.u64(r.ray.remaining_mj);
            b.u64(r.ray.initial_mj);
            b.u32(r.ray.cells.len() as u32);
            for c in &r.ray.cells {
                for v in c {
                    b.u8(*v);
                }
            }
        }
    }
    pub fn load(&mut self, s: &mut fx::blob::Scan, units: usize, max_pending: u32) -> bool {
        let generations = s.u32s();
        let n = s.list(1);
        if !s.ok() || n > units || generations.len() != n {
            return false;
        }
        self.slots.clear();
        self.generations = generations;
        for i in 0..n {
            if !s.bool() {
                self.slots.push(None);
                continue;
            }
            let template = s.u16();
            let generation = s.u32();
            let own = s.bool();
            let revision = s.u64();
            let debris_mm3 = s.u64();
            let Some(t) = self.templates.get(template as usize) else {
                return false;
            };
            if generation != self.generations[i] {
                return false;
            }
            let mut body = own.then(|| Arc::clone(t));
            let removed_n = s.list(3);
            if removed_n > t.solid_count() as usize || (removed_n > 0 && !own) {
                return false;
            }
            let mut removed = Vec::new();
            let mut shape = self.template_shapes[template as usize].clone();
            let valid = |c: Cell| c[0] < t.vol.nx && c[1] < t.vol.ny && c[2] < t.vol.nz;
            for _ in 0..removed_n {
                let c = [s.u8(), s.u8(), s.u8()];
                if !s.ok() || !valid(c) {
                    return false;
                }
                let [x, y, z] = c.map(|v| v as usize);
                if !Arc::make_mut(body.as_mut().unwrap()).remove(x, y, z) {
                    return false;
                }
                shape.remove(c, t.turret_y);
                removed.push(c);
            }
            let dent_n = s.list(11);
            if dent_n > t.solid_count() as usize || (dent_n > 0 && !own) {
                return false;
            }
            let mut dents = Dents::new();
            for _ in 0..dent_n {
                let c = [s.u8(), s.u8(), s.u8()];
                let energy = s.u64();
                if !s.ok() || !valid(c) || energy == 0 || dents.insert(c, energy).is_some() {
                    return false;
                }
                let [x, y, z] = c.map(|v| v as usize);
                if !body.as_ref().unwrap().vol.get(x, y, z) {
                    return false;
                }
            }
            let status = body
                .as_ref()
                .map_or(self.template_status[template as usize], |b| {
                    b.status(&mut self.core)
                });
            if debris_mm3 > removed_n as u64 * (t.mm as u64).pow(3) {
                return false;
            }
            let mobility =
                body.as_ref()
                    .map_or(crate::mobility::Mobility::intact(status.engine), |body| {
                        crate::mobility::Mobility::after_damage(
                            &self.template_shapes[template as usize],
                            &shape,
                            t,
                            body,
                            &self.core,
                            status,
                        )
                    });
            self.slots.push(Some(BodyState {
                template,
                generation,
                own: body,
                dents,
                removed_mm3: removed_n as u64 * (t.mm as u64).pow(3),
                debris_mm3,
                status,
                shape,
                mobility,
                revision,
                removed,
                separating: false,
            }));
        }
        if !self.load_detach(s) {
            return false;
        }
        let n = s.list(36);
        if n > max_pending as usize {
            return false;
        }
        self.pending.clear();
        for _ in 0..n {
            let unit = s.u32();
            let generation = s.u32();
            let direction = V3::new(Fx(s.i64()), Fx(s.i64()), Fx(s.i64()));
            let motion_mj = s.u64();
            let cursor = s.usize();
            let remaining_mj = s.u64();
            let initial_mj = s.u64();
            let count = s.list(3);
            if count > 768 || cursor > count || remaining_mj > initial_mj || unit as usize >= units
            {
                return false;
            }
            let cells: Vec<Cell> = (0..count).map(|_| [s.u8(), s.u8(), s.u8()]).collect();
            if let Some(state) = self.state(unit).filter(|b| b.generation == generation) {
                let b = &self.templates[state.template as usize];
                if cells
                    .iter()
                    .any(|c| c[0] >= b.vol.nx || c[1] >= b.vol.ny || c[2] >= b.vol.nz)
                {
                    return false;
                }
            }
            self.pending.push_back(Request {
                unit,
                generation,
                direction,
                motion_mj,
                ray: Penetration {
                    cells,
                    cursor,
                    remaining_mj,
                    initial_mj,
                },
            });
        }
        s.ok()
    }
    pub fn hash(&self) -> u64 {
        // Sparse damage journal, rather than re-hashing every pristine volume.
        let mut h = 0u64;
        let add = |h: &mut u64, v: u64| *h = h.wrapping_mul(0x1000_0000_01b3).wrapping_add(v);
        add(&mut h, self.generations.len() as u64);
        for v in &self.generations {
            add(&mut h, *v as u64);
        }
        for (id, slot) in self.slots.iter().enumerate() {
            let Some(s) = slot else {
                continue;
            };
            add(&mut h, id as u64);
            add(&mut h, s.template as u64);
            add(&mut h, s.revision);
            add(&mut h, s.debris_mm3);
            add(&mut h, s.removed.len() as u64);
            for c in &s.removed {
                for v in c {
                    add(&mut h, *v as u64);
                }
            }
            add(&mut h, s.dents.len() as u64);
            for (c, e) in &s.dents {
                for v in c {
                    add(&mut h, *v as u64);
                }
                add(&mut h, *e);
            }
        }
        if !self.detach.is_empty() {
            add(&mut h, self.detach_hash());
        }
        add(&mut h, self.pending.len() as u64);
        for r in &self.pending {
            for v in [
                r.unit as u64,
                r.generation as u64,
                r.ray.cursor as u64,
                r.ray.remaining_mj,
                r.ray.initial_mj,
                r.direction.x.raw() as u64,
                r.direction.y.raw() as u64,
                r.direction.z.raw() as u64,
                r.motion_mj,
            ] {
                add(&mut h, v);
            }
            add(&mut h, r.ray.cells.len() as u64);
            for c in &r.ray.cells {
                for v in c {
                    add(&mut h, *v as u64);
                }
            }
        }
        h
    }
    pub fn frames(&self, units: &crate::Units) -> Vec<Option<[Frame; 2]>> {
        (0..units.len())
            .map(|i| {
                let body = self.body(i as u32)?;
                if !units.alive[i] {
                    return None;
                }
                let y = units.body_y(i);
                let base = V3::new(
                    Fx::from_int(units.x[i]).add(units.fx[i]),
                    y,
                    Fx::from_int(units.z[i]).add(units.fz[i]),
                );
                let hull = Frame::new(body, base, units.head[i], units.up[i]);
                let target = units.target[i] as usize;
                let head = if target < units.len() && units.alive[target] {
                    let dx = Fx::from_int(units.x[target])
                        .add(units.fx[target])
                        .sub(base.x);
                    let dz = Fx::from_int(units.z[target])
                        .add(units.fz[target])
                        .sub(base.z);
                    if dx == Fx::ZERO && dz == Fx::ZERO {
                        units.head[i]
                    } else {
                        fx::angle::atan2(dz.raw(), dx.raw())
                    }
                } else {
                    units.head[i]
                };
                Some([hull, Frame::new(body, base, head, units.up[i])])
            })
            .collect()
    }
    pub fn max_radius(&self) -> i32 {
        self.templates
            .iter()
            .map(|b| {
                (b.vol.nx as i32 + b.vol.ny as i32 + b.vol.nz as i32) * b.mm as i32
                    / voxel::unit::WORLD_MM as i32
            })
            .max()
            .unwrap_or(0)
    }
    pub fn new(templates: Vec<UnitBody>) -> Self {
        let mut core = CoreBufs::new();
        let template_status = templates.iter().map(|b| b.status(&mut core)).collect();
        let template_shapes = templates
            .iter()
            .map(crate::burst::chunk::Shape::from_body)
            .collect();
        Self {
            templates: templates.into_iter().map(Arc::new).collect(),
            template_status,
            template_shapes,
            slots: Vec::new(),
            generations: Vec::new(),
            pending: VecDeque::new(),
            core,
            detach: VecDeque::new(),
        }
    }
    pub fn spawn_at(&mut self, id: u32, template: u16) {
        let i = id as usize;
        self.slots.resize_with(self.slots.len().max(i + 1), || None);
        self.generations.resize(self.slots.len(), 0);
        self.generations[i] = self.generations[i].wrapping_add(1);
        self.detach.retain(|j| j.unit != id);
        self.slots[i] = Some(BodyState {
            template,
            generation: self.generations[i],
            own: None,
            dents: Dents::new(),
            removed_mm3: 0,
            debris_mm3: 0,
            status: self.template_status[template as usize],
            shape: self.template_shapes[template as usize].clone(),
            mobility: crate::mobility::Mobility::intact(
                self.template_status[template as usize].engine,
            ),
            revision: 0,
            removed: Vec::new(),
            separating: false,
        });
    }
    pub fn despawn(&mut self, id: u32) {
        self.detach.retain(|j| j.unit != id);
        if let Some(s) = self.slots.get_mut(id as usize) {
            *s = None;
        }
    }
    pub fn state(&self, id: u32) -> Option<&BodyState> {
        self.slots.get(id as usize)?.as_ref()
    }
    pub fn body(&self, id: u32) -> Option<&UnitBody> {
        let s = self.state(id)?;
        Some(
            s.own
                .as_deref()
                .unwrap_or(&self.templates[s.template as usize]),
        )
    }
    pub fn remaining_matter(&self, id: u32, cost: u64) -> u64 {
        let Some(s) = self.state(id) else {
            return cost;
        };
        let volume = self.templates[s.template as usize].volume_mm3().max(1);
        cost.saturating_sub((cost as u128 * s.debris_mm3 as u128 / volume as u128) as u64)
    }
    pub fn shared_body(&self, id: u32) -> Option<Arc<UnitBody>> {
        let state = self.state(id)?;
        Some(Arc::clone(
            state
                .own
                .as_ref()
                .unwrap_or(&self.templates[state.template as usize]),
        ))
    }
    pub fn queue(
        &mut self,
        id: u32,
        from: [Fx; 3],
        to: [Fx; 3],
        energy_mj: u64,
        max_pending: usize,
    ) -> bool {
        if self.pending.len() >= max_pending {
            return false;
        }
        let Some(state) = self.state(id) else {
            return false;
        };
        let generation = state.generation;
        let ray = Penetration::new(self.body(id).unwrap(), from, to, energy_mj);
        self.pending.push_back(Request {
            unit: id,
            generation,
            ray,
            direction: V3::ZERO,
            motion_mj: 0,
        });
        true
    }
    /// A resolved field cell uses the same fracture/deformation queue as a ray.
    /// The captured generation prevents a delayed wave striking a replacement.
    // The queued hit carries independent identity, work and momentum fields.
    #[allow(clippy::too_many_arguments)]
    pub fn queue_cells(
        &mut self,
        id: u32,
        generation: u32,
        cells: Vec<Cell>,
        fracture_mj: u64,
        motion_mj: u64,
        direction: V3,
        max_pending: u32,
    ) -> bool {
        let Some(state) = self.state(id) else {
            return true;
        };
        if state.generation != generation {
            return true;
        }
        let b = self.body(id).unwrap();
        if cells.len() > 768
            || cells
                .iter()
                .any(|c| c[0] >= b.vol.nx || c[1] >= b.vol.ny || c[2] >= b.vol.nz)
        {
            return false;
        }
        if let Some(previous) = self.pending.back_mut() {
            if previous.unit == id
                && previous.generation == generation
                && previous.ray.cursor == 0
                && previous.direction == direction
                && previous.ray.cells.len() + cells.len() <= 768
            {
                previous.ray.cells.extend(cells);
                previous.ray.initial_mj = previous.ray.initial_mj.saturating_add(fracture_mj);
                previous.ray.remaining_mj = previous.ray.remaining_mj.saturating_add(fracture_mj);
                previous.motion_mj = previous.motion_mj.saturating_add(motion_mj);
                return true;
            }
        }
        if self.pending.len() >= max_pending as usize {
            return false;
        }
        self.pending.push_back(Request {
            unit: id,
            generation,
            ray: Penetration {
                cells,
                cursor: 0,
                initial_mj: fracture_mj,
                remaining_mj: fracture_mj,
            },
            direction,
            motion_mj,
        });
        true
    }
    /// Empty and stale requests spend a job token too. Geometry consumes the
    /// cell budget. The unfinished front request resumes on the next tick.
    pub fn step(&mut self, work_kj_m3: &[u32; 5], mut cells: u32, jobs: u32) -> Vec<Change> {
        let mut out = Vec::new();
        let mut changed = BTreeSet::new();
        for _ in 0..jobs {
            if cells == 0 {
                break;
            }
            let Some(mut request) = self.pending.pop_front() else {
                break;
            };
            let Some(state) = self
                .slots
                .get_mut(request.unit as usize)
                .and_then(Option::as_mut)
            else {
                continue;
            };
            if state.generation != request.generation || request.ray.done() {
                continue;
            }
            if state.separating {
                self.pending.push_back(request);
                continue;
            }
            // A ray through an already empty passage need not make a COW copy.
            let source = state
                .own
                .as_deref()
                .unwrap_or(&self.templates[state.template as usize]);
            while !request.ray.done() && cells > 0 {
                let [x, y, z] = request.ray.cells[request.ray.cursor].map(|v| v as usize);
                if source.vol.get(x, y, z) {
                    break;
                }
                request.ray.cursor += 1;
                cells -= 1;
            }
            if request.ray.done() {
                continue;
            }
            if cells == 0 {
                self.pending.push_front(request);
                break;
            }
            let body = state
                .own
                .get_or_insert_with(|| Arc::clone(&self.templates[state.template as usize]));
            let body = Arc::make_mut(body);
            let before_energy = request.ray.remaining_mj;
            let result = request.ray.step(body, &mut state.dents, work_kj_m3, cells);
            let motion_mj = ((request.motion_mj as u128 * result.spent_mj as u128)
                / before_energy.max(1) as u128) as u64;
            request.motion_mj -= motion_mj;
            cells -= result.visited;
            if result.spent_mj > 0 {
                state.revision = state.revision.wrapping_add(result.spent_mj);
                state.removed_mm3 += result.removed.len() as u64 * (body.mm as u64).pow(3);
                for c in &result.removed {
                    state.shape.remove(*c, body.turret_y);
                }
                state.removed.extend_from_slice(&result.removed);
                if !result.removed.is_empty() {
                    changed.insert(request.unit);
                }
                out.push(Change {
                    unit: request.unit,
                    removed: result.removed,
                    dented: result.dented.into_iter().collect(),
                    spent_mj: result.spent_mj,
                    direction: request.direction,
                    motion_mj,
                });
            }
            if !request.ray.done() {
                self.pending.push_front(request);
            }
        }
        for id in changed {
            let state = self.slots[id as usize].as_mut().unwrap();
            let body = state.own.as_ref().unwrap();
            state.status = body.status(&mut self.core);
            state.mobility = crate::mobility::Mobility::after_damage(
                &self.template_shapes[state.template as usize],
                &state.shape,
                &self.templates[state.template as usize],
                body,
                &self.core,
                state.status,
            );
            if state.status.alive && state.status.detached > 0 && !state.separating {
                let mut mask = body.vol.clone();
                for (word, reached) in mask.cols.iter_mut().zip(self.core.reached_cols()) {
                    *word &= !reached;
                }
                self.detach
                    .push_back(detach::Job::new(id, state.generation, mask));
                state.separating = true;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel::Part;
    fn body() -> UnitBody {
        let mut b = UnitBody::new_scaled(8, 3, 3, 1, 100);
        for x in 0..8 {
            b.set(x, 1, 1, Part::Armour);
        }
        b.set(0, 1, 1, Part::Engine);
        b.set(7, 1, 1, Part::Core);
        assert!(b.seat_core());
        b
    }
    fn p(x: i32) -> [Fx; 3] {
        [Fx::from_int(x), Fx::from_ratio(3, 2), Fx::from_ratio(3, 2)]
    }
    #[test]
    fn a_penetration_disables_the_real_engine_without_hurting_the_template() {
        let mut b = Bodies::new(vec![body()]);
        b.spawn_at(0, 0);
        b.spawn_at(1, 0);
        assert!(b.queue(0, p(-1), p(10), 1000, 16));
        let changes = b.step(&[1; 5], 32, 8);
        assert_eq!(changes[0].removed, [[0, 1, 1]]);
        let status = b.state(0).unwrap().status;
        assert!(status.alive && !status.engine);
        assert!(b.state(1).unwrap().status.engine);
        assert!(b.slots[1].as_ref().unwrap().own.is_none());
        assert_eq!(b.state(0).unwrap().removed_mm3, 1_000_000);
    }
    #[test]
    fn shared_explosion_input_stays_immutable_after_a_live_hit() {
        let mut b = Bodies::new(vec![body()]);
        b.spawn_at(0, 0);
        let snapshot = b.shared_body(0).unwrap();
        let before = snapshot.vol.cols.clone();
        b.queue(0, p(-1), p(10), 4500, 16);
        b.step(&[1; 5], 100, 1);
        assert_eq!(snapshot.vol.cols, before);
        assert_ne!(b.body(0).unwrap().vol.cols, before);
    }
    #[test]
    fn an_impact_waiting_in_the_queue_cannot_hit_a_reused_unit_slot() {
        let mut b = Bodies::new(vec![body()]);
        b.spawn_at(9, 0);
        b.queue(9, p(-1), p(10), 10000, 16);
        b.despawn(9);
        b.spawn_at(9, 0);
        assert!(b.step(&[1; 5], 32, 8).is_empty());
        assert!(b.state(9).unwrap().status.alive);
        assert!(b.state(9).unwrap().status.engine);
    }
    #[test]
    fn a_small_budget_defers_damage_and_preserves_its_final_geometry() {
        let run = |budget| {
            let mut b = Bodies::new(vec![body()]);
            b.spawn_at(0, 0);
            b.queue(0, p(-1), p(10), 4500, 16);
            while !b.pending.is_empty() {
                b.step(&[1; 5], budget, 1);
            }
            let state = b.state(0).unwrap();
            (
                b.body(0).unwrap().vol.cols.clone(),
                state.dents.clone(),
                state.removed_mm3,
                state.revision,
            )
        };
        assert_eq!(run(1), run(100));
    }
    #[test]
    fn a_snapshot_resumes_partial_penetration_and_dents_exactly() {
        let mut a = Bodies::new(vec![body()]);
        a.spawn_at(0, 0);
        a.queue(0, p(-1), p(10), 4500, 16);
        a.step(&[1; 5], 2, 1);
        let mut blob = fx::blob::Blob::new();
        a.save(&mut blob);
        let mut b = Bodies::new(vec![body()]);
        assert!(b.load(&mut fx::blob::Scan::new(&blob.v), 1, 16));
        assert_eq!(a.hash(), b.hash());
        while !a.pending.is_empty() {
            a.step(&[1; 5], 1, 1);
            b.step(&[1; 5], 1, 1);
            assert_eq!(a.hash(), b.hash());
            assert_eq!(a.body(0).unwrap().vol.cols, b.body(0).unwrap().vol.cols);
        }
        assert!(!b.state(0).unwrap().dents.is_empty());
    }
    #[test]
    fn a_snapshot_rejects_an_out_of_bounds_pending_cell_and_truncation() {
        let mut a = Bodies::new(vec![body()]);
        a.spawn_at(0, 0);
        a.queue(0, p(-1), p(10), 4500, 16);
        let mut blob = fx::blob::Blob::new();
        a.save(&mut blob);
        for end in 0..blob.v.len() {
            let mut b = Bodies::new(vec![body()]);
            assert!(!b.load(&mut fx::blob::Scan::new(&blob.v[..end]), 1, 16));
        }
        *blob.v.last_mut().unwrap() = 255;
        let mut b = Bodies::new(vec![body()]);
        assert!(!b.load(&mut fx::blob::Scan::new(&blob.v), 1, 16));
    }
    #[test]
    fn tilted_body_frames_round_trip_in_physical_units() {
        let mut body = body();
        body.mm = 250;
        let frame = Frame::new(
            &body,
            V3::new(Fx::from_int(4000), Fx::from_int(27), Fx::from_int(4800)),
            fx::angle::FULL / 7,
            V3::new(Fx::from_ratio(1, 5), Fx::ONE, Fx::from_ratio(-1, 7)),
        );
        let a = [Fx::from_ratio(9, 2), Fx::ONE, Fx::from_ratio(1, 2)];
        let b = frame.local(frame.world(a));
        for i in 0..3 {
            assert!(a[i].sub(b[i]).abs() < Fx::from_ratio(1, 10000));
        }
    }
}
