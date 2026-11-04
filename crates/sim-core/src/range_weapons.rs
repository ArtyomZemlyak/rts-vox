//! Finite flight packets and spatial contacts, independent of explosion waves.
use super::*;
use crate::burst::PhysicsTask;
use fx::blob::{Blob, Scan};
use modding::{
    explosion::Delivery,
    range_weapon::{Stream, Tuning},
};
#[path = "range_laser.rs"]
mod laser;
#[path = "range_spall.rs"]
mod spall;

const MAX_ROUNDS: usize = 128;

#[cfg(test)]
fn laser_boxes(units: &[RangeUnit]) -> Vec<([i32; 3], [i32; 3])> {
    laser_boxes_observed(units, &|_, _| {})
}
fn laser_boxes_observed(
    units: &[RangeUnit],
    task: &(impl Fn(usize, &'static str) + Sync),
) -> Vec<([i32; 3], [i32; 3])> {
    if units.len() >= 16 {
        units
            .par_iter()
            .map_init(
                || PhysicsTask::new(task, "physics_laser_boxes_begin", "physics_laser_boxes_end"),
                |_, u| u.bounds(),
            )
            .collect()
    } else {
        let _task = PhysicsTask::new(task, "physics_laser_boxes_begin", "physics_laser_boxes_end");
        units.iter().map(|u| u.bounds()).collect()
    }
}

#[derive(Clone, Debug)]
pub struct Round {
    pub pos: [Fx; 3],
    pub velocity: [Fx; 3],
    pub distance: Fx,
    pub serial: u32,
}
#[derive(Clone, Copy, Debug)]
pub struct Trace {
    pub from: [Fx; 3],
    pub to: [Fx; 3],
    pub hit: bool,
    pub serial: u32,
}
#[derive(Clone, Copy, Debug)]
pub struct WaveTrace {
    pub trace: Trace,
    pub strength: Fx,
}
impl std::ops::Deref for WaveTrace {
    type Target = Trace;
    fn deref(&self) -> &Trace {
        &self.trace
    }
}
pub struct Streams {
    pub rounds: Vec<Round>,
    pub traces: Vec<Trace>,
    pub left: u32,
    next: u32,
    cooldown: u32,
    pub(super) serial: u32,
    pub tuning: Tuning,
}
impl Default for Streams {
    fn default() -> Self {
        Self {
            rounds: Vec::new(),
            traces: Vec::new(),
            left: 0,
            next: 0,
            cooldown: 0,
            serial: 0,
            tuning: Tuning::from_ron(include_str!("../../../data/range-weapons.ron"))
                .expect("range weapon tuning"),
        }
    }
}
impl Streams {
    /// Number of emitted packets since the range was reset (wrapping u32).
    pub fn emitted(&self) -> u32 {
        self.serial
    }
    pub fn ready(&self, tick: u32) -> bool {
        self.left == 0 && self.rounds.is_empty() && tick >= self.cooldown
    }
    pub fn save(&self, b: &mut Blob) {
        for v in [self.left, self.next, self.cooldown, self.serial] {
            b.u32(v);
        }
        b.usize(self.rounds.len());
        for r in &self.rounds {
            for v in r.pos.into_iter().chain(r.velocity) {
                b.fx(v);
            }
            b.fx(r.distance);
            b.u32(r.serial);
        }
    }
    pub fn load(&mut self, s: &mut Scan) -> Option<()> {
        self.left = s.u32();
        self.next = s.u32();
        self.cooldown = s.u32();
        self.serial = s.u32();
        let n = s.usize();
        if n > MAX_ROUNDS || self.left > 300 {
            return None;
        }
        self.rounds.clear();
        for _ in 0..n {
            let r = Round {
                pos: std::array::from_fn(|_| s.fx()),
                velocity: std::array::from_fn(|_| s.fx()),
                distance: s.fx(),
                serial: s.u32(),
            };
            if r.distance <= Fx::ZERO
                || r.distance > Fx::from_int(3000)
                || r.velocity.iter().any(|v| v.abs() > Fx::from_int(10000))
            {
                return None;
            }
            self.rounds.push(r);
        }
        s.ok().then_some(())
    }
}
/// Shared authored spread, seeded by shot order and never render time.
pub(super) fn spread(mut delta: [Fx; 3], heading: [Fx; 3], serial: u32, milli: i32) -> [Fx; 3] {
    let noise = |shift: u32| {
        ((serial.wrapping_mul(1664525).wrapping_add(1013904223) >> shift) & 255u32) as i32 - 128
    };
    let range = delta[0]
        .mul(delta[0])
        .add(delta[2].mul(delta[2]))
        .sqrt()
        .max(Fx::ONE);
    delta[1] = delta[1].add(range.mul(Fx::from_ratio(noise(8) * milli, 128000)));
    let side = range.mul(Fx::from_ratio(noise(16) * milli, 128000));
    delta[0] = delta[0].sub(heading[2].mul(side));
    delta[2] = delta[2].add(heading[0].mul(side));
    delta
}
/// Shared cell-centred ballistic launch for solid rounds in trials and battle.
/// Gravity is integrated before position, so the lift includes half a tick.
pub(super) fn kinetic_velocity(
    from: [Fx; 3],
    target: [i32; 3],
    config: &ExplosionConfig,
) -> [Fx; 3] {
    let delta: [Fx; 3] = std::array::from_fn(|a| {
        Fx::from_int(target[a])
            .add(Fx::from_ratio(1, 2))
            .sub(from[a])
    });
    let time = delta[0]
        .mul(delta[0])
        .add(delta[2].mul(delta[2]))
        .sqrt()
        .div(Fx::from_int(config.shell_speed))
        .max(Fx::from_ratio(1, crate::TICKS_PER_SECOND));
    let mut velocity = delta.map(|v| v.div(time));
    velocity[1] = velocity[1].add(
        Fx::from_int(config.gravity)
            .mul(time.add(Fx::from_ratio(1, crate::TICKS_PER_SECOND)))
            .mul(Fx::from_ratio(1, 2)),
    );
    velocity
}

impl BlastRange {
    /// A targeted burst belongs to that live target. Already flying rounds
    /// remain physical; the burst must not become untargeted ground fire.
    pub(super) fn cancel_target_stream(&mut self) {
        if self.streams.left > 0 {
            self.streams.left = 0;
            if let Some(tuning) = self.stream_tuning() {
                self.streams.cooldown = self.streams.cooldown.max(self.tick + tuning.reload_ticks);
            }
        }
        if self.shell.is_none() {
            self.fire_tick = u32::MAX;
        }
    }
    /// The single gun's round against what it met: [`Self::ricochet`].
    pub(super) fn kinetic_ricochet(&mut self, at: [i32; 3], from: [i32; 3]) -> bool {
        let config = self.config;
        let mut velocity = self.velocity;
        let turned = self.ricochet(at, from, &mut velocity, &config);
        self.velocity = velocity;
        turned
    }

    /// A shallow strike against intact steel leaves a dent and turns the
    /// round away from the exposed face. Its reduced speed also reduces the
    /// finite penetration work available at a later contact. Any solid
    /// round with the weapon that fired it: the single gun's or a battle
    /// tank's (ADR-456).
    pub(super) fn ricochet(
        &mut self,
        at: [i32; 3],
        from: [i32; 3],
        velocity: &mut [Fx; 3],
        config: &ExplosionConfig,
    ) -> bool {
        let Some(tuning) = config.ricochet else {
            return false;
        };
        if from == at {
            return false;
        }
        let unit = self.units.iter().position(|u| u.solid(at));
        if unit.is_none() && material(&self.world, at) != tuning.material {
            return false;
        }
        let speed_sq = velocity.iter().fold(Fx::ZERO, |sum, v| sum.add(v.mul(*v)));
        if speed_sq < Fx::from_int(tuning.min_speed).mul(Fx::from_int(tuning.min_speed)) {
            return false;
        }
        let speed = speed_sq.sqrt();
        if let Some(id) = unit {
            return self.armour_ricochet(id, at, &tuning, speed, velocity, config);
        }
        let need = material_work(tuning.material, config);
        let old = self.surface_work.get(&at).copied().unwrap_or(0);
        if u64::from(old) * 1000 >= u64::from(need) * u64::from(tuning.max_preexisting_wear_milli) {
            return false;
        }
        // A half-cell sweep can cross two voxel planes at a corner. Only
        // faces with air immediately behind the crossing are exposed; this
        // also keeps a tiny gravitational Y step from masquerading as a
        // horizontal wall's top face.
        let axis = (0..3)
            .filter(|&a| from[a] != at[a])
            .filter(|&a| {
                let mut neighbour = at;
                neighbour[a] = from[a];
                material(&self.world, neighbour) == 0
                    && !self.units.iter().any(|u| u.solid(neighbour))
            })
            .min_by_key(|&a| velocity[a].abs().raw());
        let Some(axis) = axis else { return false };
        if velocity[axis].abs() >= speed.mul(Fx::from_ratio(tuning.max_normal_milli as i32, 1000)) {
            return false;
        }
        let wear = (config.energy * tuning.wear_milli / 1000)
            .max(1)
            .min(need.saturating_sub(old).saturating_sub(1));
        if wear == 0 {
            return false;
        }
        self.surface_work.insert(at, old + wear);
        self.surface_changed.push(at);
        let keep = Fx::from_ratio(tuning.speed_retention_milli as i32, 1000);
        for (a, v) in velocity.iter_mut().enumerate() {
            let reflected = if a == axis { Fx::ZERO.sub(*v) } else { *v };
            *v = reflected.mul(keep);
        }
        true
    }

    /// The same shallow strike against a machine's intact armour (ADR-454).
    /// The surface is the armour's own, in the frame of the part it belongs
    /// to — hull, turned turret or segment — and its normal is taken from
    /// the armour around the struck cell, so a sloped plate turns the round
    /// by its slope, not by the world grid or by one voxel step. Other
    /// roles take the round by the ordinary finite work of `kinetic_impact`.
    fn armour_ricochet(
        &mut self,
        id: usize,
        at: [i32; 3],
        tuning: &modding::explosion::Ricochet,
        speed: Fx,
        velocity: &mut [Fx; 3],
        config: &ExplosionConfig,
    ) -> bool {
        let u = &self.units[id];
        let Some((part, cell)) = u.hit(at) else {
            return false;
        };
        let (body, frame) = if part == 0 {
            (&u.body, u.root_frame(cell[1]))
        } else {
            (&u.body.segments[part - 1].body, u.seg_frames[part - 1])
        };
        if body.part_at(cell[0], cell[1], cell[2]) != voxel::Part::Armour {
            return false;
        }
        let need = unit_work(body, cell, config);
        let old = if part == 0 {
            u.dents.get(&cell.map(|v| v as u8)).copied().unwrap_or(0)
        } else {
            u.seg_dent(part - 1, cell)
        };
        if u64::from(old) * 1000 >= u64::from(need) * u64::from(tuning.max_preexisting_wear_milli) {
            return false;
        }
        // The surface as the round meets it: four probes parallel to the
        // round, half a cell to each side of the struck cell's centre, in
        // the part's
        // own cells. Where they first meet the body gives the plane the
        // round strikes — a plate stepped in voxels by its slope, not by
        // the step one probe happened to meet, and never a hollow behind
        // the plate. A probe that meets nothing (the round clips an edge)
        // leaves the round to the ordinary hit.
        type V = crate::damage::V3;
        let size = [body.vol.nx, body.vol.ny, body.vol.nz].map(i64::from);
        let solid = |p: V| {
            let q = [p.x, p.y, p.z].map(|c| c.raw() >> 32);
            (0..3).all(|a| q[a] >= 0 && q[a] < size[a])
                && body.vol.get(q[0] as usize, q[1] as usize, q[2] as usize)
        };
        let unit_of = |d: V| {
            let n = d.len_sq().sqrt();
            (n > Fx::ZERO).then(|| d.scale(Fx::ONE.div(n)))
        };
        let v = V::new(velocity[0], velocity[1], velocity[2]);
        let Some(r) = unit_of(V::new(frame.x.dot(v), frame.y.dot(v), frame.z.dot(v))) else {
            return false;
        };
        let least = [r.x, r.y, r.z]
            .iter()
            .enumerate()
            .min_by_key(|(_, c)| c.abs().raw())
            .map_or(0, |(a, _)| a);
        let e = [
            V::new(Fx::ONE, Fx::ZERO, Fx::ZERO),
            V::new(Fx::ZERO, Fx::ONE, Fx::ZERO),
            V::new(Fx::ZERO, Fx::ZERO, Fx::ONE),
        ];
        let Some(u) = unit_of(r.cross(e[least])) else {
            return false;
        };
        let w = r.cross(u);
        let half = Fx::from_ratio(1, 2);
        let centre = V::new(
            Fx::from_int(cell[0] as i32).add(half),
            Fx::from_int(cell[1] as i32).add(half),
            Fx::from_int(cell[2] as i32).add(half),
        );
        let step = r.scale(Fx::from_ratio(1, 8));
        let depth = |side: V| -> Option<i32> {
            let mut p = centre
                .add(side.scale(Fx::from_ratio(1, 2)))
                .sub(r.scale(Fx::from_int(8)));
            (0..320).find(|_| {
                p = p.add(step);
                solid(p)
            })
        };
        let (Some(up), Some(un), Some(wp), Some(wn)) = (
            depth(u),
            depth(u.scale(Fx::from_int(-1))),
            depth(w),
            depth(w.scale(Fx::from_int(-1))),
        ) else {
            return false;
        };
        // Eighth-cell steps, a cell between the probes of a pair.
        let tangent =
            |along: V, far: i32, near: i32| along.add(r.scale(Fx::from_ratio(far - near, 8)));
        let n = tangent(u, up, un).cross(tangent(w, wp, wn));
        let n = if n.dot(r) > Fx::ZERO {
            n.scale(Fx::from_int(-1))
        } else {
            n
        };
        let n = frame
            .x
            .scale(n.x)
            .add(frame.y.scale(n.y))
            .add(frame.z.scale(n.z));
        let Some(normal) = unit_of(n) else {
            return false;
        };
        let into = normal.dot(v);
        if into >= Fx::ZERO
            || into.abs() >= speed.mul(Fx::from_ratio(tuning.max_normal_milli as i32, 1000))
        {
            return false;
        }
        let wear = (config.energy * tuning.wear_milli / 1000)
            .max(1)
            .min(need.saturating_sub(old).saturating_sub(1));
        if wear == 0 {
            return false;
        }
        let u = &mut self.units[id];
        if part == 0 {
            u.receive_cell(cell, wear, config);
        } else {
            u.receive_seg_cell(part - 1, cell, wear, config);
        }
        let reflected = v.sub(normal.scale(into.add(into)));
        let keep = Fx::from_ratio(tuning.speed_retention_milli as i32, 1000);
        *velocity = [reflected.x, reflected.y, reflected.z].map(|c| c.mul(keep));
        true
    }

    /// A solid shot spends finite work along its incoming ray. Crossing a
    /// thicker or more oblique face costs more cells; cover consumes the same
    /// budget before the ray can reach a hull behind it. A configured share
    /// fractures neighbours of the pierced channel in that hull; no blast
    /// wave is created and blocked shots cannot damage sheltered units.
    #[cfg(test)]
    pub(super) fn kinetic_impact(&mut self, at: [i32; 3]) {
        let config = self.config;
        self.kinetic_hit(at, self.velocity, &config);
    }

    /// Cell-aligned contact for isolated material tests.
    #[cfg(test)]
    pub(super) fn kinetic_hit(
        &mut self,
        at: [i32; 3],
        velocity: [Fx; 3],
        config: &ExplosionConfig,
    ) {
        self.kinetic_hit_at(at, at.map(Fx::from_int), velocity, config);
    }

    /// Authored barrel ends, with independent connectivity for the two guns.
    pub fn stream_muzzles(&self) -> Vec<[Fx; 3]> {
        if self.point_gun() {
            vec![self.muzzle()]
        } else {
            muzzles(&self.gun, self.turret.angle)
        }
    }
    fn emit_laser(&mut self, tuning: &Stream, physics: &(impl Fn(usize, &'static str) + Sync)) {
        for from in self.stream_muzzles() {
            let samples = if tuning.interval_ticks == 1 {
                modding::clock::stream_samples(self.tick).min(self.streams.left)
            } else {
                1
            };
            let mut last = None;
            for _ in 0..samples {
                last = Some(self.laser_ray(
                    from,
                    self.target(),
                    None,
                    tuning,
                    &self.config.clone(),
                    (&mut |_| {}, physics),
                ));
            }
            if let Some(trace) = last {
                self.streams.traces.push(trace);
            }
        }
    }
    /// One shared laser path for the test gun and battle actors. Ownership
    /// excludes the emitter from both the core ray and radial surface heat.
    pub(super) fn laser_ray(
        &mut self,
        from: [Fx; 3],
        target: [i32; 3],
        owner: Option<usize>,
        tuning: &Stream,
        config: &ExplosionConfig,
        observers: (
            &mut impl FnMut(&'static str),
            &(impl Fn(usize, &'static str) + Sync),
        ),
    ) -> Trace {
        self.laser_ray_mode(from, target, owner, tuning, config, true, observers)
    }
    pub(super) fn laser_ray_mode(
        &mut self,
        from: [Fx; 3],
        target: [i32; 3],
        owner: Option<usize>,
        tuning: &Stream,
        config: &ExplosionConfig,
        flame: bool,
        observers: (
            &mut impl FnMut(&'static str),
            &(impl Fn(usize, &'static str) + Sync),
        ),
    ) -> Trace {
        let (completed, physics) = observers;
        let task = |_, event| physics(2, event);
        let beam_task =
            PhysicsTask::new(&task, "physics_laser_beam_begin", "physics_laser_beam_end");
        let delta = std::array::from_fn::<_, 3, _>(|a| Fx::from_int(target[a]).sub(from[a]));
        let length = delta
            .iter()
            .fold(Fx::ZERO, |n, v| n.add(v.mul(*v)))
            .sqrt()
            .max(Fx::ONE);
        let direction = delta.map(|v| v.div(length));
        // A ray can only meet bodies whose enclosing boxes cross its full
        // segment. Build that short, slot-ordered list once; testing every
        // distant articulated body at every half-voxel was the hot path.
        let start = from.map(Fx::to_int_trunc);
        let far = std::array::from_fn::<_, 3, _>(|a| {
            from[a]
                .add(direction[a].mul(Fx::from_int(tuning.range)))
                .to_int_trunc()
        });
        let boxes = laser_boxes_observed(&self.units, &task);
        let near: Vec<_> = boxes
            .iter()
            .enumerate()
            .filter_map(|(id, (lo, hi))| {
                (Some(id) != owner
                    && (0..3)
                        .all(|a| start[a].min(far[a]) <= hi[a] && start[a].max(far[a]) >= lo[a]))
                .then_some(id)
            })
            .collect();
        let serial = self.streams.serial;
        self.streams.serial = serial.wrapping_add(1);
        let water_active = self
            .water
            .has_water_in_x_span(start[0].min(far[0]), start[0].max(far[0]));
        let mut effective = tuning.clone();
        completed("orders_laser_setup");
        if water_active
            && effective.heat > 0
            && (0..3).all(|a| start[a] >= 0 && start[a] < self.size[a] as i32)
            && self.water.amount(start.map(|v| v as u32)) > 0
            && material(&self.world, start) == 0
            && !near.iter().any(|&id| self.units[id].solid(start))
        {
            let absorbed =
                self.water
                    .absorb_heat(start.map(|v| v as u32), effective.heat, self.size);
            effective.heat = effective.heat.saturating_sub(absorbed);
            effective.work = (u64::from(tuning.work) * u64::from(effective.heat)
                / u64::from(tuning.heat.max(1))) as u32;
            if effective.heat == 0 && tuning.heat > 0 {
                completed("orders_laser_query");
                completed("orders_laser_contact_world");
                drop(beam_task);
                completed("orders_laser_beam");
                completed("orders_laser_halo");
                return Trace {
                    from,
                    to: from,
                    hit: true,
                    serial,
                };
            }
        }
        let mut previous = from.map(Fx::to_int_trunc);
        let (mut end, mut free_end, mut contact, mut hit) = (from, from, None, false);
        for i in 1..=tuning.range * 2 {
            let point =
                std::array::from_fn(|a| from[a].add(direction[a].mul(Fx::from_ratio(i, 2))));
            let p = point.map(Fx::to_int_trunc);
            if (0..3).any(|a| p[a] < 0 || p[a] >= self.size[a] as i32) {
                break;
            }
            end = point;
            if p == previous {
                continue;
            }
            if let Some(shield) = self.dome_crossed(previous, p) {
                completed("orders_laser_query");
                self.absorb_work(shield, p, tuning.work);
                completed("orders_laser_contact_shield");
                hit = true;
                break;
            }
            // Molten matter absorbs the beam until its remaining vaporisation
            // energy is paid. Only the surplus can reach the next cell.
            let m = material(&self.world, p);
            if let Some(flow) = self
                .heat_tuning
                .melt_flow
                .iter()
                .flatten()
                .find(|f| f.liquid == m)
                .copied()
                .filter(|_| effective.heat > 0)
                .filter(|_| !self.world.on_frame(p[0] as u32, p[1] as u32, p[2] as u32))
                .filter(|_| {
                    !near.iter().any(|&id| {
                        let (lo, hi) = boxes[id];
                        (0..3).all(|a| p[a] >= lo[a] && p[a] <= hi[a]) && self.units[id].solid(p)
                    })
                })
            {
                let q = p.map(|v| v as u32);
                let threshold = self
                    .matter
                    .economy
                    .mats
                    .get(flow.solid)
                    .map_or(flow.freeze, |m| m.melt.max(flow.freeze))
                    .saturating_mul(3)
                    .max(1);
                let needed = threshold.saturating_sub(self.heat.cell(q).heat);
                let absorbed = effective.heat.min(needed);
                self.heat.add(&self.world, q, absorbed);
                effective.heat -= absorbed;
                effective.work = (u64::from(tuning.work) * u64::from(effective.heat)
                    / u64::from(tuning.heat.max(1))) as u32;
                if self.heat.cell(q).heat < threshold
                    || self.world.has_pending_dig(q[0], q[1], q[2])
                    || self.matter.debris.claimed(q)
                {
                    completed("orders_laser_query");
                    completed("orders_laser_contact_world");
                    contact = Some(p);
                    hit = true;
                    break;
                }
                self.world
                    .queue_dig_by(q[0], q[1], q[2], crate::heat::VAPOR);
                if effective.heat == 0 {
                    completed("orders_laser_query");
                    completed("orders_laser_contact_world");
                    contact = Some(p);
                    hit = true;
                    break;
                }
                previous = p;
                free_end = point;
                continue;
            }
            if self.world.solid(p[0] as u32, p[1] as u32, p[2] as u32)
                || near.iter().any(|&id| {
                    let (lo, hi) = boxes[id];
                    (0..3).all(|a| p[a] >= lo[a] && p[a] <= hi[a]) && self.units[id].solid(p)
                })
            {
                completed("orders_laser_query");
                // Classify before heating/melting changes the endpoint. Body
                // precedence matches stream_contact_owned, including overlap.
                let body_hit = near.iter().any(|&id| {
                    let (lo, hi) = boxes[id];
                    (0..3).all(|a| p[a] >= lo[a] && p[a] <= hi[a]) && self.units[id].solid(p)
                });
                completed(if body_hit {
                    "orders_laser_contact_unit"
                } else {
                    "orders_laser_contact_world"
                });
                self.stream_contact_owned(
                    p,
                    &effective,
                    flame,
                    from.map(Fx::to_int_trunc),
                    config,
                    owner,
                );
                contact = Some(p);
                hit = true;
                break;
            }
            if water_active && effective.heat > 0 {
                let q = p.map(|v| v as u32);
                if self.water.amount(q) > 0 {
                    let absorbed = self.water.absorb_heat(q, effective.heat, self.size);
                    effective.heat = effective.heat.saturating_sub(absorbed);
                    effective.work = (u64::from(tuning.work) * u64::from(effective.heat)
                        / u64::from(tuning.heat.max(1)))
                        as u32;
                    if effective.heat == 0 {
                        completed("orders_laser_query");
                        completed("orders_laser_contact_world");
                        hit = true;
                        break;
                    }
                }
            }
            previous = p;
            free_end = point;
        }
        if !hit {
            completed("orders_laser_query");
            completed("orders_laser_contact_miss");
        }
        drop(beam_task);
        completed("orders_laser_beam");
        // Laser contact only deposits heat (or drains a shield); it never
        // edits body geometry. The halo therefore reads these same boxes.
        self.laser_halo_with_boxes(from, free_end, contact, &effective, owner, (&boxes, &task));
        completed("orders_laser_halo");
        Trace {
            from,
            to: end,
            hit,
            serial,
        }
    }
    fn emit_stream(&mut self, tuning: &Stream) {
        let flame = self.config.delivery == Delivery::Flame;
        let muzzles = self.stream_muzzles();
        if !flame && !self.solid_flight_room(&self.battle.rounds, muzzles.len()) {
            return;
        }
        for from in muzzles {
            for ray in 0..if flame { 3 } else { 1 } {
                if self.streams.rounds.len() == MAX_ROUNDS {
                    return;
                }
                let id = self.streams.serial;
                self.streams.serial = self.streams.serial.wrapping_add(1);
                let mut delta: [Fx; 3] =
                    std::array::from_fn(|a| Fx::from_int(self.target()[a]).sub(from[a]));
                // Spread is seeded by shot order, never frame time. Keep the first
                // flame packet on axis; the two others describe the ragged cone.
                if !flame || ray > 0 {
                    delta = spread(delta, self.heading(), id, tuning.spread_milli);
                }
                let velocity = flight_velocity(delta, self.config.shell_speed);
                self.streams.rounds.push(Round {
                    pos: from,
                    velocity,
                    distance: Fx::from_int(tuning.range),
                    serial: id,
                });
            }
        }
    }
    pub(super) fn stream_contact(
        &mut self,
        p: [i32; 3],
        tuning: &Stream,
        flame: bool,
        at: [i32; 3],
        config: &ExplosionConfig,
    ) {
        self.stream_contact_owned(p, tuning, flame, at, config, None);
    }
    pub(super) fn stream_contact_owned(
        &mut self,
        p: [i32; 3],
        tuning: &Stream,
        flame: bool,
        at: [i32; 3],
        config: &ExplosionConfig,
        owner: Option<usize>,
    ) {
        if let Some(id) = self
            .units
            .iter()
            .enumerate()
            .position(|(id, u)| Some(id) != owner && u.solid(p))
        {
            let u = &mut self.units[id];
            let Some((part, cell)) = u.hit(p) else {
                return;
            };
            if u.section_holds(part, cell.map(|v| v as u8)) {
                return;
            }
            if flame {
                self.thermals.deposit_direct(
                    thermal::Cell {
                        unit: id as u16,
                        part: part as u8,
                        at: cell.map(|v| v as u8),
                    },
                    tuning.heat,
                    &self.thermal_tuning,
                );
                return;
            }
            let pressure = tuning.work;
            let need = u.resistance(p, config);
            if pressure >= need {
                self.matter.begin(tuning.fracture_j, 1);
                let result = if part == 0 {
                    self.matter.body(id, u, cell, &u.root_frame(cell[1]), at)
                } else {
                    self.matter
                        .body_segment(id, u, part - 1, cell, &u.seg_frames[part - 1], at)
                };
                if !matches!(result, matter::ClaimResult::Applied) {
                    if matches!(result, matter::ClaimResult::Deferred) {
                        self.defer_stream_contact(Some((id, part, cell)), p, at, tuning, config);
                    }
                    return;
                }
                if part == 0 {
                    u.detached += 1;
                } else {
                    u.seg_detached += 1;
                }
            }
            if part == 0 {
                u.receive_cell(cell, pressure, config);
            } else {
                u.receive_seg_cell(part - 1, cell, pressure, config);
            }
        } else if material(&self.world, p) != 0 {
            if flame {
                if config.delivery == Delivery::Laser {
                    let mat = material(&self.world, p);
                    if self
                        .matter
                        .economy
                        .mats
                        .get(mat)
                        .is_some_and(|m| m.melt > 0 || m.kindling > 0)
                    {
                        self.heat.add(&self.world, p.map(|v| v as u32), tuning.heat);
                    }
                } else {
                    self.heat.deposit(
                        &self.world,
                        &self.matter.economy.mats,
                        p,
                        tuning.heat_radius,
                        tuning.heat,
                    );
                    self.water
                        .absorb_radial_heat(&self.world, p, tuning.heat_radius, tuning.heat);
                }
            } else {
                let total = self
                    .surface_work
                    .get(&p)
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(tuning.work);
                if total >= material_work(material(&self.world, p), config) {
                    self.matter.begin(tuning.fracture_j, 1);
                    match self.matter.ground(&mut self.world, p, at) {
                        matter::ClaimResult::Applied => {
                            self.surface_work.remove(&p);
                        }
                        matter::ClaimResult::Deferred => {
                            self.defer_stream_contact(None, p, at, tuning, config);
                        }
                        _ => {}
                    }
                } else {
                    self.surface_work.insert(p, total);
                }
                self.surface_changed.push(p);
            }
        }
    }
    pub(super) fn step_streams_observed(
        &mut self,
        physics: &(impl Fn(usize, &'static str) + Sync),
    ) {
        self.streams.traces.clear();
        if matches!(self.config.delivery, Delivery::Blast | Delivery::Kinetic) {
            return;
        }
        let flame = self.config.delivery == Delivery::Flame;
        let mut tuning = match self.config.delivery {
            Delivery::Flame => &self.streams.tuning.flame,
            Delivery::MachineGun => &self.streams.tuning.machine_gun,
            Delivery::Laser => &self.streams.tuning.laser,
            Delivery::Blast | Delivery::Kinetic => unreachable!(),
        }
        .clone();
        let scale = |v: u32| {
            (u64::from(v) * u64::from(self.config.energy) / 1000).min(u64::from(u32::MAX)) as u32
        };
        tuning.work = scale(tuning.work);
        tuning.heat = scale(tuning.heat);
        tuning.fracture_j = scale(tuning.fracture_j);
        if self.tick == self.fire_tick && (self.point_gun() || self.gun_armed()) {
            self.streams.left = tuning.burst_ticks;
            self.streams.next = modding::clock::stream_time(self.tick) as u32;
        }
        if !self.point_gun() && !self.gun_armed() {
            self.streams.left = 0;
        }
        if self.streams.left > 0 {
            if modding::clock::stream_time(self.tick) >= u64::from(self.streams.next)
                && (self.point_gun() || self.turret.aligned(self.aim_angle(), self.turret_limits))
            {
                if self.config.delivery == Delivery::Laser {
                    self.emit_laser(&tuning, physics);
                } else {
                    self.emit_stream(&tuning);
                }
                // Sample deadlines retain the fractional interval (2 authored
                // samples become alternating 1/2 simulation ticks at 20 Hz).
                self.streams.next = self
                    .streams
                    .next
                    .saturating_add(tuning.interval_ticks)
                    .max(modding::clock::stream_time(self.tick) as u32 + 1);
            }
            self.streams.left = self
                .streams
                .left
                .saturating_sub(modding::clock::stream_samples(self.tick));
            if self.streams.left == 0 {
                self.streams.cooldown = self.tick + tuning.reload_ticks;
                self.fire_tick = u32::MAX;
            }
        }
        let before: Vec<_> = self
            .units
            .iter()
            .map(|u| (u.removed.len(), u.seg_removed.len()))
            .collect();
        let rounds = std::mem::take(&mut self.streams.rounds);
        let flame_water = flame && self.water.has_free_water();
        let mut survivors = Vec::with_capacity(MAX_ROUNDS);
        let step = Fx::from_ratio(1, crate::TICKS_PER_SECOND);
        for mut round in rounds {
            let from = round.pos;
            let start = from.map(Fx::to_int_trunc);
            if flame_water
                && (0..3).all(|a| start[a] >= 0 && start[a] < self.size[a] as i32)
                && self.water.amount(start.map(|v| v as u32)) > 0
                && material(&self.world, start) == 0
                && !self.units.iter().any(|u| u.solid(start))
            {
                self.water
                    .absorb_heat(start.map(|v| v as u32), tuning.heat, self.size);
                self.streams.traces.push(Trace {
                    from,
                    to: from,
                    hit: true,
                    serial: round.serial,
                });
                continue;
            }
            // A live tuning change applies to new rounds. Existing packets
            // retain both their velocity and their remaining physical range.
            let delta = round.velocity.map(|v| v.mul(step));
            let length = delta.iter().fold(Fx::ZERO, |n, v| n.add(v.mul(*v))).sqrt();
            if length <= Fx::ZERO {
                continue;
            }
            let travel = length.min(round.distance);
            let fraction = travel.div(length);
            let q = std::array::from_fn::<_, 3, _>(|a| from[a].add(delta[a].mul(fraction)));
            let water_segment = flame_water
                && self.water.has_water_in_x_span(
                    from[0].min(q[0]).to_int_trunc(),
                    from[0].max(q[0]).to_int_trunc(),
                );
            let n = (travel.to_int_trunc() + 1) * 2;
            let mut end = q;
            let mut hit = false;
            let mut previous = from.map(Fx::to_int_trunc);
            for i in 1..=n {
                let point = std::array::from_fn(|a| {
                    from[a].add(q[a].sub(from[a]).mul(Fx::from_ratio(i, n)))
                });
                let p = point.map(Fx::to_int_trunc);
                if let Some(shield) = self.dome_crossed(previous, p) {
                    self.absorb_work(shield, p, tuning.work);
                    end = point;
                    hit = true;
                    break;
                }
                if material(&self.world, p) != 0 || self.units.iter().any(|u| u.solid(p)) {
                    self.stream_contact(p, &tuning, flame, self.gun.at, &self.config.clone());
                    end = point;
                    hit = true;
                    break;
                }
                if water_segment
                    && p != previous
                    && p.iter().all(|v| *v >= 0)
                    && (0..3).all(|a| p[a] < self.size[a] as i32)
                {
                    let q = p.map(|v| v as u32);
                    if self.water.amount(q) > 0 {
                        self.water.absorb_heat(q, tuning.heat, self.size);
                        end = point;
                        hit = true;
                        break;
                    }
                }
                previous = p;
            }
            self.streams.traces.push(Trace {
                from,
                to: end,
                hit,
                serial: round.serial,
            });
            round.distance = round.distance.sub(travel);
            round.pos = q;
            if !hit
                && round.distance > Fx::ZERO
                && (0..3).all(|a| q[a] >= Fx::ZERO && q[a] < Fx::from_int(self.size[a] as i32))
            {
                survivors.push(round);
            }
        }
        self.streams.rounds = survivors;
        for (id, (u, old)) in self.units.iter_mut().zip(before).enumerate() {
            if (u.removed.len(), u.seg_removed.len()) != old {
                u.refresh_damage(id);
            }
        }
    }
}

/// Shared finite-packet speed for trials and battle, including elevated targets.
pub(super) fn flight_velocity(delta: [Fx; 3], speed: i32) -> [Fx; 3] {
    let len = delta
        .iter()
        .fold(Fx::ZERO, |n, v| n.add(v.mul(*v)))
        .sqrt()
        .max(Fx::ONE);
    delta.map(|v| v.div(len).mul(Fx::from_int(speed)))
}

pub(super) fn muzzles(u: &RangeUnit, angle: i32) -> Vec<[Fx; 3]> {
    if u.kind == RangeUnitKind::Walker04 {
        return walker_muzzles(u)
            .into_iter()
            .map(|(_, point)| point)
            .collect();
    }
    let f = u.aimed_frame(
        &u.pose,
        fx::angle::diff(angle, u.motion.drive.head),
        u.turret_pitch,
    );
    u.body
        .weapon_paths()
        .iter()
        .filter(|p| u.body.barrel_connected(p, u.connectivity.reached_cols()))
        .take(2)
        .map(|p| {
            let v = f.world(muzzle_cell(p));
            [v.x, v.y, v.z]
        })
        .collect()
}

pub(super) fn walker_muzzles(u: &RangeUnit) -> Vec<(usize, [Fx; 3])> {
    u.body
        .weapon_paths()
        .iter()
        .enumerate()
        .take(4)
        .filter_map(|(arm, path)| {
            let first = 10 + arm * 4;
            let gun = first + 3;
            if u.seg_gone[first..=gun].iter().any(|gone| *gone)
                || u.body.segments[gun]
                    .body
                    .subsystem_count(voxel::unit::Part::Weapon)
                    == 0
                || !u.body.barrel_connected(path, u.connectivity.reached_cols())
            {
                return None;
            }
            let p = u.seg_frames[gun].world([Fx::from_int(4), Fx::from_int(2), Fx::from_int(2)]);
            Some((arm, [p.x, p.y, p.z]))
        })
        .collect()
}

pub(super) fn razlom_muzzles(u: &RangeUnit, angle: i32) -> Vec<[Fx; 3]> {
    let frame = u.aimed_frame(
        &u.pose,
        fx::angle::diff(angle, u.motion.drive.head),
        u.turret_pitch,
    );
    let Some(path) = u.body.weapon_paths().first() else {
        return Vec::new();
    };
    if !u.body.barrel_connected(path, u.connectivity.reached_cols()) {
        return Vec::new();
    }
    [11usize, 15, 19, 23, 27, 31, 35]
        .into_iter()
        .map(|z| {
            let cell = [
                Fx::from_int(i32::from(path.max[0]) + 1),
                Fx::from_ratio(i32::from(path.min[1]) + i32::from(path.max[1]), 2),
                Fx::from_int(z as i32),
            ];
            let v = frame.world(cell);
            [v.x, v.y, v.z]
        })
        .collect()
}

pub(super) fn muzzle_cell(p: &voxel::unit::WeaponPath) -> [Fx; 3] {
    let mut cell =
        std::array::from_fn(|a| Fx::from_ratio(i32::from(p.min[a]) + i32::from(p.max[a]), 2));
    cell[usize::from(p.axis)] = Fx::from_int(i32::from(p.max[usize::from(p.axis)]) + 1);
    cell
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn laser_starting_in_water_evaporates_it_and_stops() {
        let mut r = laser_range(vec![]);
        let from = [100, 100, 336];
        assert!(r.water.edit_volume(from.map(|v| v as u32), true));
        let tuning = r.streams.tuning.laser.clone();
        let config = r.config;
        let trace = r.laser_ray(
            from.map(Fx::from_int),
            [200, 100, 336],
            Some(0),
            &tuning,
            &config,
            (&mut |_| {}, &|_, _| {}),
        );
        assert!(trace.hit);
        assert_eq!(trace.to.map(Fx::to_int_trunc), from);
        assert!(r.water.stats().vapor_units > 0);
    }
    #[test]
    fn laser_spends_heat_on_water_before_reaching_a_solid_surface() {
        let mut r = laser_range(vec![modding::range::RangeBlock {
            min: [150, 99, 335],
            max: [151, 102, 338],
            material: 3,
        }]);
        let q = [120, 100, 336];
        assert!(r.water.edit_volume(q, true));
        let initial = r.water.stats().units;
        let mut tuning = r.streams.tuning.laser.clone();
        let needed = r.water.tuning.units_per_voxel * r.water.tuning.evaporation_heat;
        tuning.heat = needed + 1000;
        let config = r.config;
        let trace = r.laser_ray(
            [100, 100, 336].map(Fx::from_int),
            [200, 100, 336],
            Some(0),
            &tuning,
            &config,
            (&mut |_| {}, &|_, _| {}),
        );
        assert!(trace.hit);
        assert_eq!(trace.to[0].to_int_trunc(), 150);
        assert_eq!(r.water.amount(q), 0);
        assert_eq!(r.water.stats().vapor_units, initial);
        assert_eq!(r.water.stats().units, initial);
        assert!(r.heat.cell([150, 100, 336]).heat > 0);
        assert!(
            r.heat.cell([150, 100, 336]).heat <= 1000,
            "beam did not spend heat on water"
        );
    }
    fn kinetic_range(blocks: Vec<modding::range::RangeBlock>) -> BlastRange {
        let mut c =
            ExplosionConfig::from_ron(include_str!("../../../data/kinetic-shell.ron")).unwrap();
        // Fixed isolated penetration/ricochet fixture. Stock balance is tested separately.
        c.energy = 12000;
        c.material_energy_j = 1800000;
        c.shell_speed = 280;
        c.spall = None;
        c.unit_role_resistance_milli[1] = 1800;
        let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        l.size = [640, 128, 640];
        l.course = None;
        l.blocks = blocks;
        l.shields.clear();
        l.units.truncate(1);
        l.units[0].at = [285, 16, 336];
        BlastRange::with_layout(c, l).unwrap()
    }
    fn plate(x: i32, end: i32, material: u16) -> modding::range::RangeBlock {
        modding::range::RangeBlock {
            min: [x, 32, 330],
            max: [end, 60, 342],
            material,
        }
    }
    fn laser_range(blocks: Vec<modding::range::RangeBlock>) -> BlastRange {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/laser.ron")).unwrap();
        let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        l.course = None;
        l.blocks = blocks;
        l.shields.clear();
        l.units.truncate(1);
        l.units[0].at = [285, 16, 336];
        BlastRange::with_layout(c, l).unwrap()
    }

    #[test]
    fn laser_heats_melt_before_vaporising_it_and_passing_to_the_next_layer() {
        let mut r = laser_range(vec![modding::range::RangeBlock {
            min: [230, 100, 336],
            max: [232, 101, 337],
            material: 3,
        }]);
        for x in 230..232 {
            r.world.queue_transfer(voxel::Edit::put(x, 100, 336, 13));
        }
        r.world.flush_transfers();
        let mut tuning = r.streams.tuning.laser.clone();
        tuning.heat = 200;
        tuning.heat_radius = 0;
        let config = r.config;
        let from = [100, 100, 336].map(Fx::from_int);
        let q = [230, 100, 336];
        for _ in 0..5 {
            let trace = r.laser_ray(
                from,
                [250, 100, 336],
                None,
                &tuning,
                &config,
                (&mut |_| {}, &|_, _| {}),
            );
            assert!(trace.hit);
            assert_eq!(trace.to[0].to_int_trunc(), 230);
            assert!(!r.world.has_pending_dig(q[0], q[1], q[2]));
            assert_eq!(
                r.heat.cell([231, 100, 336]).heat,
                0,
                "beam heated through liquid"
            );
        }
        r.laser_ray(
            from,
            [250, 100, 336],
            None,
            &tuning,
            &config,
            (&mut |_| {}, &|_, _| {}),
        );
        assert!(r.world.has_pending_dig(q[0], q[1], q[2]));
        r.world.flush();
        assert_eq!(r.world.material(q[0], q[1], q[2]), 0);
        assert!(
            r.heat.cell([231, 100, 336]).heat > 0,
            "surplus did not reach the next layer"
        );
    }

    #[test]
    fn sustained_laser_opens_a_hole_and_accounts_for_vaporised_matter() {
        let mut r = laser_range(vec![modding::range::RangeBlock {
            min: [230, 0, 330],
            max: [233, 60, 342],
            material: 3,
        }]);
        r.heat_tuning.melt_moves_per_tick = 0;
        r.control();
        r.aim = Some([392, 40, 336]);
        assert!(r.refire());
        let mut first = None;
        let mut passed = false;
        for _ in 0..45 {
            r.step();
            if let Some(t) = r.streams.traces.first() {
                let p = t.to.map(Fx::to_int_trunc);
                first.get_or_insert(p);
                passed |= p[0] >= 233;
            }
        }
        assert!(passed, "laser never opened a passage");
        assert!(
            r.matter
                .economy
                .ledger
                .sunk(crate::matter::Sink::Pulverised)
                > 0,
            "vaporised matter was not accounted for"
        );
        assert!(r.matter.economy.ledger.balanced());
    }

    #[test]
    fn laser_contact_observation_distinguishes_world_unit_shield_and_miss_without_state_changes() {
        let seed = laser_range(vec![]);
        for (event, from, aim, owner) in [
            (
                "orders_laser_contact_world",
                [100, 100, 336],
                [200, 15, 336],
                Some(0),
            ),
            (
                "orders_laser_contact_unit",
                [100, 25, 336],
                battle::aim_point(&seed.units[0]),
                None,
            ),
            (
                "orders_laser_contact_shield",
                [100, 40, 336],
                [500, 40, 336],
                Some(0),
            ),
            (
                "orders_laser_contact_miss",
                [100, 100, 336],
                [500, 100, 336],
                Some(0),
            ),
        ] {
            let mut actual = if event == "orders_laser_contact_shield" {
                let mut layout = seed.layout.clone().unwrap();
                layout.shields.push(modding::range::RangeShieldDef {
                    at: [190, 40, 336],
                    radius: 30,
                });
                BlastRange::with_layout(seed.config, layout).unwrap()
            } else {
                BlastRange::restore(seed.config, &seed.snapshot()).unwrap()
            };
            let mut reference = BlastRange::restore(actual.config, &actual.snapshot()).unwrap();
            let tuning = actual.streams.tuning.laser.clone();
            let config = actual.config;
            let mut events = Vec::new();
            let observed = actual.laser_ray(
                from.map(Fx::from_int),
                aim,
                owner,
                &tuning,
                &config,
                (&mut |phase| events.push(phase), &|_, _| {}),
            );
            let expected = reference.laser_ray(
                from.map(Fx::from_int),
                aim,
                owner,
                &tuning,
                &config,
                (&mut |_| {}, &|_, _| {}),
            );
            assert_eq!(observed.hit, event != "orders_laser_contact_miss");
            assert_eq!(observed.to, expected.to);
            assert_eq!(
                events
                    .iter()
                    .copied()
                    .filter(|e| e.starts_with("orders_laser_contact_"))
                    .collect::<Vec<_>>(),
                vec![event]
            );
            assert_eq!(
                actual.state_hash(),
                reference.state_hash(),
                "observation changed {event}"
            );
        }
    }

    #[test]
    fn observed_laser_beam_preserves_state_and_pairs_all_scopes() {
        let seed = laser_range(vec![]);
        for workers in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            let (mut reference, mut actual) = (
                BlastRange::restore(seed.config, &seed.snapshot()).unwrap(),
                BlastRange::restore(seed.config, &seed.snapshot()).unwrap(),
            );
            let from = [100, 100, 336].map(Fx::from_int);
            let tuning = seed.streams.tuning.laser.clone();
            reference.laser_ray(
                from,
                [500, 100, 336],
                None,
                &tuning,
                &seed.config,
                (&mut |_| {}, &|_, _| {}),
            );
            let trace = crate::task_trace::Trace::default();
            pool.install(|| {
                actual.laser_ray(
                    from,
                    [500, 100, 336],
                    None,
                    &tuning,
                    &seed.config,
                    (&mut |_| {}, &|stage, event| {
                        assert_eq!(stage, 2);
                        trace.record(stage, event);
                    }),
                );
            });
            trace.balanced(&[
                "physics_laser_boxes",
                "physics_laser_beam",
                "physics_laser_halo",
                "physics_laser_halo_read",
                "physics_laser_halo_commit",
            ]);
            assert_eq!(
                actual.state_hash(),
                reference.state_hash(),
                "workers={workers}"
            );
        }
    }
    #[test]
    fn laser_boxes_preserve_slots_and_fresh_articulated_poses_across_workers() {
        let kinds = [
            RangeUnitKind::Tank,
            RangeUnitKind::Strider,
            RangeUnitKind::Spider,
            RangeUnitKind::Artillery,
            RangeUnitKind::TurretT1,
            RangeUnitKind::TurretT2,
            RangeUnitKind::Base,
        ];
        let mut units: Vec<_> = (0..17)
            .map(|id| RangeUnit::of_kind(kinds[id % kinds.len()], [100 + id as i32 * 250, 16, 336]))
            .collect();
        let pools: Vec<_> = [1, 2, 4, 8]
            .into_iter()
            .map(|workers| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(workers)
                    .build()
                    .unwrap()
            })
            .collect();
        let pristine: Vec<_> = units.iter().map(|unit| unit.bounds()).collect();
        for phase in 0..2 {
            if phase > 0 {
                for (id, unit) in units.iter_mut().enumerate() {
                    unit.motion.drive.head = (id as i32 + 1) * 13;
                    unit.motion.pos[1] = unit.motion.pos[1].add(Fx::from_int(id as i32 + 1));
                    unit.update_pose((id as i32 + 1) * 31);
                }
            }
            let expected: Vec<_> = units.iter().map(|unit| unit.bounds()).collect();
            if phase > 0 {
                assert_ne!(expected, pristine);
            }
            for count in [0, 1, 15, 16, 17] {
                for pool in &pools {
                    assert_eq!(
                        pool.install(|| laser_boxes(&units[..count])),
                        expected[..count],
                        "phase={phase} count={count} workers={}",
                        pool.current_num_threads()
                    );
                }
            }
        }
    }
    #[test]
    fn laser_halo_heats_exposed_surfaces_with_falloff_and_not_through_them() {
        let block = |min, max| modding::range::RangeBlock {
            min,
            max,
            material: 3,
        };
        let mut r = laser_range(vec![
            block([200, 100, 338], [201, 101, 340]),
            block([210, 100, 339], [211, 101, 340]),
            block([220, 100, 340], [221, 101, 341]),
            block([245, 100, 336], [246, 101, 337]),
        ]);
        let centre = |p: [i32; 3]| p.map(|v| Fx::from_int(v).add(Fx::from_ratio(1, 2)));
        let tuning = r.streams.tuning.laser.clone();
        r.laser_halo(
            centre([180, 100, 336]),
            centre([240, 100, 336]),
            None,
            &tuning,
        );
        assert_eq!(r.heat.cell([200, 100, 338]).heat, tuning.heat * 12 / 16);
        assert_eq!(r.heat.cell([210, 100, 339]).heat, tuning.heat * 7 / 16);
        assert_eq!(
            r.heat.cell([200, 100, 339]).heat,
            0,
            "steel must shade its far side"
        );
        assert_eq!(r.heat.cell([220, 100, 340]).heat, 0, "outside radius");
        assert_eq!(r.heat.cell([245, 100, 336]).heat, 0, "past segment end");
        // An immediately blocked muzzle has no free flight length, but its
        // surrounding exposed contact patch must still receive heat.
        r.laser_halo(
            centre([200, 100, 336]),
            centre([200, 100, 336]),
            None,
            &tuning,
        );
        assert_eq!(
            r.heat.cell([200, 100, 338]).heat,
            2 * (tuning.heat * 12 / 16)
        );
        let mut restored = BlastRange::restore(r.config, &r.snapshot()).unwrap();
        assert_eq!(r.state_hash(), restored.state_hash());
        for _ in 0..3 {
            r.step();
            restored.step();
            assert_eq!(
                r.state_hash(),
                restored.state_hash(),
                "halo heat lost on restore"
            );
        }
    }

    #[test]
    fn laser_halo_grazes_armour_but_never_heats_its_own_gun() {
        let mut r = laser_range(vec![]);
        let (at, _, cell) = hull_side(&r);
        let centre = |p: [i32; 3]| p.map(|v| Fx::from_int(v).add(Fx::from_ratio(1, 2)));
        let tuning = r.streams.tuning.laser.clone();
        r.laser_halo(
            centre([at[0] - 8, at[1], at[2] - 2]),
            centre([at[0] + 8, at[1], at[2] - 2]),
            None,
            &tuning,
        );
        let cell = thermal::Cell {
            unit: 0,
            part: 0,
            at: cell.map(|v| v as u8),
        };
        assert!(
            r.thermals.heat(cell) > 0,
            "grazing beam missed nearby armour"
        );
        assert!(
            r.thermals.heat(cell) <= tuning.heat,
            "pose samples stacked heat"
        );
        r.units.clear();
        r.thermals = Default::default();
        let (lo, hi) = r.gun.bounds();
        r.laser_halo(centre(lo), centre(hi), None, &tuning);
        assert_eq!(
            r.thermals.hot(),
            0,
            "firing unit entered its own thermal halo"
        );
        assert!(r.gun.removed.is_empty());
    }

    #[test]
    fn laser_halo_is_blocked_by_a_shield_even_off_axis() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/laser.ron")).unwrap();
        let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        l.course = None;
        l.units.clear();
        l.blocks = vec![modding::range::RangeBlock {
            min: [285, 100, 374],
            max: [286, 101, 375],
            material: 3,
        }];
        l.shields = vec![modding::range::RangeShieldDef {
            at: [285, 100, 336],
            radius: 40,
        }];
        let mut r = BlastRange::with_layout(c, l).unwrap();
        let tuning = r.streams.tuning.laser.clone();
        let centre = |p: [i32; 3]| p.map(|v| Fx::from_int(v).add(Fx::from_ratio(1, 2)));
        // Beam is outside the dome, the plate is inside its thermal radius.
        r.laser_halo(
            centre([275, 100, 377]),
            centre([295, 100, 377]),
            None,
            &tuning,
        );
        assert_eq!(r.heat.cell([285, 100, 374]).heat, 0);
        r.shields.clear();
        r.laser_halo(
            centre([275, 100, 377]),
            centre([295, 100, 377]),
            None,
            &tuning,
        );
        assert!(r.heat.cell([285, 100, 374]).heat > 0);
    }

    #[test]
    fn laser_slowly_melts_a_steel_wall_without_hurting_the_sheltered_unit() {
        let mut r = laser_range(vec![modding::range::RangeBlock {
            min: [230, 0, 330],
            max: [246, 60, 342],
            material: 3,
        }]);
        // Low-power beam: retain coverage of slow heating and sheltering.
        r.streams.tuning.laser.heat = 40;
        r.control();
        r.aim = Some([392, 40, 336]);
        assert!(r.refire());
        let mut first = None;
        for _ in 0..90 {
            r.step();
            if first.is_none() {
                first = r.streams.traces.first().map(|t| t.to.map(Fx::to_int_trunc));
            }
        }
        let first = first.expect("laser never fired");
        assert_eq!(first[0], 230);
        assert_ne!(material(&r.world, first), 3, "front steel did not melt");
        assert_eq!(material(&r.world, [244, first[1], first[2]]), 3);
        assert!(r.heat.melted_total > 0);
        assert!(r.units[0].removed.is_empty(), "wall failed to shield unit");
        assert!(r.explosion.is_none());
        assert!(r.shell.is_none());
    }
    #[test]
    fn spider_laser_burns_through_steel_in_half_a_second() {
        let mut r = laser_range(vec![modding::range::RangeBlock {
            min: [230, 0, 330],
            max: [238, 60, 342],
            material: 3,
        }]);
        r.streams.tuning.laser = super::super::motion::spider_tuning().laser.clone();
        r.control();
        r.aim = Some([392, 40, 336]);
        assert!(r.refire());
        let mut contacts = Vec::new();
        for _ in 0..15 {
            r.step();
            if let Some(trace) = r.streams.traces.first() {
                contacts.push(trace.to.map(Fx::to_int_trunc));
            }
        }
        let first = *contacts.first().expect("siege laser never fired");
        assert_eq!(first[0], 230);
        assert_ne!(
            material(&r.world, [237, first[1], first[2]]),
            3,
            "80 cm steel still blocks the siege beam: {contacts:?}"
        );
        assert!(
            contacts.iter().any(|p| p[0] > 238),
            "beam failed to pass the melt"
        );
        assert!(
            r.heat.melted_total >= 8,
            "steel did not melt layer by layer"
        );
        assert!(r.explosion.is_none());
        assert!(r.shell.is_none());
        eprintln!(
            "siege steel: ticks=15 melted={} contacts={contacts:?}",
            r.heat.melted_total
        );
    }
    #[test]
    fn spider_laser_delivers_ten_times_the_previous_heat_on_first_contact() {
        let block = modding::range::RangeBlock {
            min: [230, 100, 330],
            max: [231, 101, 342],
            material: 3,
        };
        let mut heat = [0; 2];
        for (i, old) in [true, false].into_iter().enumerate() {
            let mut r = laser_range(vec![block.clone()]);
            r.set_physics_workers(1);
            r.set_navigation_workers(1);
            let mut tuning = super::super::motion::spider_tuning().laser.clone();
            if old {
                tuning.work = 1200000;
                tuning.heat = 32000000;
                tuning.fracture_j = 2000;
            }
            tuning.heat_radius = 0;
            let config = r.config;
            let trace = r.laser_ray(
                [200, 100, 336].map(Fx::from_int),
                [280, 100, 336],
                None,
                &tuning,
                &config,
                (&mut |_| {}, &|_, _| {}),
            );
            assert!(trace.hit);
            heat[i] = r.heat.cell([230, 100, 336]).heat;
            assert!(r.matter.economy.ledger.balanced());
            if !old {
                let restored =
                    BlastRange::restore(r.config, &r.snapshot()).expect("high heat snapshot");
                assert_eq!(restored.state_hash(), r.state_hash());
            }
        }
        assert!(heat[0] > 0);
        assert_eq!(heat[1], heat[0] * 10, "first-contact heat: {heat:?}");
    }
    #[test]
    fn electric_laser_deposits_ten_times_previous_contact_heat() {
        let block = modding::range::RangeBlock {
            min: [230, 100, 330],
            max: [231, 101, 342],
            material: 3,
        };
        let mut heat = [0; 2];
        for (i, old) in [true, false].into_iter().enumerate() {
            let mut r = laser_range(vec![block.clone()]);
            r.set_physics_workers(1);
            r.set_navigation_workers(1);
            let mut tuning = r.streams.tuning.lightning.clone().unwrap();
            if old {
                tuning.work = 500;
                tuning.heat = 3000;
                tuning.fracture_j = 1;
            }
            tuning.heat_radius = 0;
            let config = r.config;
            assert!(
                r.laser_ray(
                    [200, 100, 336].map(Fx::from_int),
                    [280, 100, 336],
                    None,
                    &tuning,
                    &config,
                    (&mut |_| {}, &|_, _| {})
                )
                .hit
            );
            heat[i] = r.heat.cell([230, 100, 336]).heat;
        }
        assert!(heat[0] > 0);
        assert_eq!(heat[1], heat[0] * 10);
    }
    #[test]
    fn spider_laser_melts_armour_while_the_contact_sweeps() {
        let mut r = laser_range(vec![]);
        r.streams.tuning.laser = super::super::motion::spider_tuning().laser.clone();
        r.control();
        r.aim = Some([285, 39, 336]);
        assert!(r.refire());
        let own = r.gun.body.volume_mm3();
        for tick in 0..15 {
            // A moving contact must not need repeated hits on one voxel.
            r.aim = Some([285, 39, 332 + tick % 9]);
            r.step();
        }
        assert!(
            r.thermals.melted > 0,
            "sweeping siege beam left intact armour"
        );
        assert!(!r.units[0].removed.is_empty());
        assert_eq!(r.gun.body.volume_mm3(), own, "beam heated its emitter");
        assert!(r.explosion.is_none());
        eprintln!(
            "siege armour: ticks=15 melted={} removed={}",
            r.thermals.melted,
            r.units[0].removed.len()
        );
    }
    #[test]
    fn stock_tank_laser_finishes_a_tank_instead_of_drilling_forever() {
        let mut r = laser_range(vec![]);
        r.control();
        assert!(r.order_attack(0));
        let own = r.gun.body.volume_mm3();
        let mut death = None;
        for tick in 0..300 {
            r.step();
            if !r.units[0].status.alive {
                death = Some(tick);
                break;
            }
        }
        eprintln!(
            "tank laser death={death:?} core={:?} melted={} fragments={}",
            r.units[0].integrity.core,
            r.thermals.melted,
            r.matter.debris.fragments.len()
        );
        assert!(
            death.is_some(),
            "stock beam could not finish its target in ten seconds"
        );
        assert_eq!(r.gun.body.volume_mm3(), own);
        r.step();
        assert!(r.streams.traces.is_empty());
        assert!(r.explosion.is_none());
        assert!(r.matter.economy.ledger.balanced());
    }

    #[test]
    fn targeted_laser_cancels_its_burst_when_the_target_dies() {
        for automatic in [false, true] {
            let mut r = laser_range(vec![]);
            r.control();
            if automatic {
                r.set_auto_attack(true);
            } else {
                assert!(r.order_attack(0));
            }
            for _ in 0..60 {
                r.step();
                if !r.streams.traces.is_empty() {
                    break;
                }
            }
            assert!(!r.streams.traces.is_empty(), "laser never started");
            assert!(r.streams.left > 0);
            let u = &r.units[0];
            let [cx, cy, cz] = [u.body.core.0, u.body.core.1, u.body.core.2].map(usize::from);
            let size = usize::from(u.body.core_size);
            for z in cz..cz + size {
                for y in cy..cy + size {
                    for x in cx..cx + size {
                        r.thermals.deposit(
                            thermal::Cell {
                                unit: 0,
                                part: 0,
                                at: [x as u8, y as u8, z as u8],
                            },
                            10_000,
                            &r.thermal_tuning,
                        );
                    }
                }
            }
            r.step_unit_heat();
            assert!(!r.units[0].status.alive);
            r.step();
            assert!(
                r.streams.traces.is_empty(),
                "laser kept firing after target death (auto={automatic})"
            );
            assert_eq!(r.streams.left, 0);
            assert_eq!(r.attack, None);
            assert_eq!(r.fire_tick, u32::MAX);
            let mut restored = BlastRange::restore(r.config, &r.snapshot()).unwrap();
            for _ in 0..10 {
                r.step();
                restored.step();
                assert_eq!(r.state_hash(), restored.state_hash());
            }
        }
    }

    #[test]
    fn laser_softens_and_removes_armour_without_a_blast() {
        let mut r = laser_range(vec![]);
        r.control();
        r.aim = Some([285, 39, 336]);
        assert!(r.refire());
        for _ in 0..1 {
            r.step();
        }
        assert!(
            r.units[0].removed.is_empty(),
            "laser removed armour instantly"
        );
        for _ in 0..110 {
            r.step();
        }
        assert!(r.thermals.melted > 0, "armour did not heat through");
        assert!(!r.units[0].removed.is_empty());
        assert!(r.explosion.is_none());
        assert!(r.shell.is_none());
    }
    #[test]
    fn laser_takes_time_to_cut_a_concrete_face() {
        let mut r = laser_range(vec![modding::range::RangeBlock {
            min: [230, 0, 330],
            max: [231, 60, 342],
            material: 2,
        }]);
        r.streams.tuning.laser.heat = 40;
        r.control();
        r.aim = Some([392, 40, 336]);
        assert!(r.refire());
        for _ in 0..5 {
            r.step();
        }
        let face = r.streams.traces[0].to.map(Fx::to_int_trunc);
        assert_eq!(material(&r.world, face), 2);
        for _ in 0..100 {
            r.step();
        }
        assert_eq!(material(&r.world, face), 0);
        assert!(r.heat.melted_total > 0);
    }
    #[test]
    fn instantaneous_laser_aims_at_a_moving_units_current_position() {
        let mut r = laser_range(vec![]);
        r.attack = Some(0);
        r.units[0].motion.drive.vel = Fx::from_int(40);
        let u = &r.units[0];
        let core = [
            u.body.core.0 as usize,
            u.body.core.1 as usize,
            u.body.core.2 as usize,
        ];
        assert_eq!(r.target(), u.world_pos(core));
    }
    #[test]
    fn laser_does_not_heat_through_a_shield_and_replays_mid_burst() {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/laser.ron")).unwrap();
        let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        l.course = None;
        l.blocks.clear();
        l.units.truncate(1);
        l.units[0].at = [285, 16, 336];
        l.shields = vec![modding::range::RangeShieldDef {
            at: [285, 16, 336],
            radius: 40,
        }];
        let mut a = BlastRange::with_layout(c, l).unwrap();
        a.control();
        a.aim = Some([285, 39, 336]);
        assert!(a.refire());
        for _ in 0..20 {
            a.step();
        }
        assert!(a.shields[0].hits > 0);
        assert!(a.shields[0].charge < a.shield_tuning.capacity);
        assert_eq!(a.thermals.hot(), 0);
        assert!(a.units[0].removed.is_empty());
        let mut b = BlastRange::restore(c, &a.snapshot()).unwrap();
        for _ in 0..75 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
    }
    fn stock_kinetic_range() -> BlastRange {
        let mut r = kinetic_range(vec![]);
        r.reconfigure(
            ExplosionConfig::from_ron(include_str!("../../../data/kinetic-shell.ron")).unwrap(),
        );
        r.units[0].motion.drive.head = fx::angle::HALF;
        r.units[0].update_pose(fx::angle::HALF);
        r.control();
        r
    }

    #[test]
    fn stock_kinetic_penetrates_less_than_the_previous_round_but_still_damages_cover() {
        let current =
            ExplosionConfig::from_ron(include_str!("../../../data/kinetic-shell.ron")).unwrap();
        let mut previous = current;
        previous.energy = 48000;
        for mat in [1, 2, 3] {
            let depth = |config: ExplosionConfig| {
                let mut r = kinetic_range(vec![plate(218, 250, mat)]);
                r.kinetic_hit(
                    [218, 40, 336],
                    [Fx::from_int(config.shell_speed), Fx::ZERO, Fx::ZERO],
                    &config,
                );
                r.world.flush();
                (218..250)
                    .take_while(|&x| material(&r.world, [x, 40, 336]) == 0)
                    .count()
            };
            let old = depth(previous);
            let new = depth(current);
            eprintln!("material={mat} penetration: {old} -> {new} cells");
            assert!(new > 0 && new < old, "material {mat}: {old} -> {new}");
        }
    }

    #[test]
    fn stock_kinetic_auto_aim_kills_front_armour_and_survives_restore() {
        let mut r = stock_kinetic_range();
        assert_eq!(r.config.shell_speed, 560);
        assert!(r.order_attack(0));
        let mut hits = 0;
        let mut last = u32::MAX;
        let mut restored = None;
        for _ in 0..300 {
            r.step();
            if let Some(b) = &mut restored {
                let b: &mut BlastRange = b;
                b.step();
                assert_eq!(r.state_hash(), b.state_hash());
            }
            if r.impact.is_some() && r.impact_tick != last {
                last = r.impact_tick;
                hits += 1;
                if hits == 1 {
                    restored = Some(BlastRange::restore(r.config, &r.snapshot()).unwrap());
                }
            }
            if !r.units[0].status.alive {
                break;
            }
        }
        assert!(!r.units[0].status.alive);
        assert_eq!(hits, 4);
        assert_eq!(r.units[0].annihilated_mm3(), 0);
        assert!(r.matter.economy.ledger.balanced());
    }

    #[test]
    fn stock_kinetic_damages_front_armour_with_three_different_aim_points() {
        let mut r = stock_kinetic_range();
        let mut contacts = Vec::new();
        for (shot, offset) in [-3, 3, 0].into_iter().enumerate() {
            let removed_before = r.units[0].removed.len();
            let u = &r.units[0];
            let half = usize::from(u.body.core_size) / 2;
            let mut aim = u.world_pos([
                u.body.core.0 as usize + half,
                u.body.core.1 as usize + half,
                u.body.core.2 as usize + half,
            ]);
            aim[2] += offset;
            assert!(r.order_ground_fire(aim));
            let mut hit = false;
            for _ in 0..240 {
                r.step();
                if let Some(at) = r.impact.filter(|_| r.impact_tick == r.tick - 1) {
                    hit = true;
                    contacts.push(at);
                    break;
                }
            }
            assert!(hit);
            r.order_stop();
            for _ in 0..240 {
                if r.ready() {
                    break;
                }
                r.step();
            }
            assert!(r.units[0].status.alive, "shot {}", shot + 1);
            assert!(
                r.units[0].removed.len() > removed_before,
                "shot {} did no damage",
                shot + 1
            );
        }
        assert!(contacts.windows(2).all(|p| p[0] != p[1]));
        assert!(r.matter.economy.ledger.balanced());
        assert!(r.explosion.is_none());
    }

    #[test]
    fn stock_kinetic_spall_keeps_cover_and_a_finite_work_budget() {
        let mut r = stock_kinetic_range();
        let config = r.config;
        let (at, _, _) = hull_side(&r);
        r.kinetic_hit(
            at,
            [Fx::ZERO, Fx::ZERO, Fx::from_int(config.shell_speed)],
            &config,
        );
        let u = &r.units[0];
        assert!(u.removed.iter().any(|p| p[0] != 30), "no lateral fracture");
        let spent: u64 = u
            .removed
            .iter()
            .map(|p| u64::from(unit_work(&u.body, p.map(usize::from), &config)))
            .sum::<u64>()
            + u.dents.values().map(|v| u64::from(*v)).sum::<u64>()
            + u.seg_removed
                .iter()
                .map(|(part, p)| {
                    u64::from(unit_work(
                        &u.body.segments[*part as usize].body,
                        p.map(usize::from),
                        &config,
                    ))
                })
                .sum::<u64>()
            + u.seg_dents.values().map(|v| u64::from(*v)).sum::<u64>();
        assert!(
            spent <= u64::from(config.energy),
            "spall created free work: {spent}"
        );
        assert_eq!(u.annihilated_mm3(), 0);
        assert!(r.matter.economy.ledger.balanced());

        let mut covered = kinetic_range(vec![modding::range::RangeBlock {
            min: [230, 16, 320],
            max: [242, 60, 352],
            material: 3,
        }]);
        covered.reconfigure(config);
        covered.kinetic_hit(
            [230, 23, 336],
            [Fx::from_int(config.shell_speed), Fx::ZERO, Fx::ZERO],
            &config,
        );
        assert!(covered.units[0].removed.is_empty());
        assert!(covered.units[0].dents.is_empty());
    }

    #[test]
    fn solid_shell_spends_work_on_steel_without_creating_a_blast() {
        let mut r = kinetic_range(vec![plate(230, 231, 3), plate(233, 234, 3)]);
        r.velocity = [Fx::from_int(280), Fx::ZERO, Fx::ZERO];
        r.kinetic_impact([230, 40, 336]);
        r.world.flush();
        assert_eq!(material(&r.world, [230, 40, 336]), 0);
        assert_eq!(material(&r.world, [233, 40, 336]), 3);
        assert_eq!(r.surface_work.get(&[233, 40, 336]), Some(&5500));
        assert!(r.explosion.is_none());
    }
    #[test]
    fn grazing_a_steel_plate_spends_more_work_than_a_square_hit() {
        let mut square = kinetic_range(vec![plate(230, 231, 3)]);
        square.velocity = [Fx::from_int(280), Fx::ZERO, Fx::ZERO];
        square.kinetic_impact([230, 40, 330]);
        assert!(square.surface_work.is_empty());

        let mut grazing = kinetic_range(vec![plate(230, 231, 3)]);
        grazing.velocity = [Fx::from_int(70), Fx::ZERO, Fx::from_int(280)];
        grazing.kinetic_impact([230, 40, 330]);
        assert_eq!(grazing.surface_work.get(&[230, 40, 331]), Some(&5500));
        assert_eq!(grazing.units[0].removed.len(), 0);
    }
    #[test]
    fn shallow_steel_contact_deflects_a_live_round_and_replays_exactly() {
        let mut a = kinetic_range(vec![plate(230, 231, 3)]);
        a.control();
        a.shell = Some([
            Fx::from_ratio(2292, 10),
            Fx::from_int(40),
            Fx::from_int(331),
        ]);
        a.velocity = [Fx::from_int(70), Fx::ZERO, Fx::from_int(280)];
        a.step();
        let hit = a.impact.expect("grazing shot never touched the plate");
        assert_eq!(hit[0], 230);
        assert!(a.shell.is_some(), "the round stopped instead of bouncing");
        assert!(a.velocity[0] < Fx::ZERO && a.velocity[2] > Fx::ZERO);
        assert_eq!(a.surface_work.get(&hit), Some(&(a.config.energy / 8)));
        assert_eq!(
            material(&a.world, hit),
            3,
            "a shallow hit bored through steel"
        );
        assert!(a.explosion.is_none());

        let mut residual = kinetic_range(vec![plate(230, 231, 3)]);
        residual.velocity = a.velocity;
        residual.kinetic_impact(hit);
        let remaining_work = residual.surface_work.get(&hit).copied().unwrap_or(0);
        assert!(remaining_work > 0 && remaining_work < a.config.energy / 2);
        assert!(
            residual.world.queued() == 0,
            "the bounced round got full work"
        );

        let mut b = BlastRange::restore(a.config, &a.snapshot()).unwrap();
        for _ in 0..80 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        assert!(a.shell.is_none(), "the bounced round never landed");
    }
    /// The tank's side belt: armour two cells high along the whole hull
    /// on its `-z` face (local `y` 11 and 12). The struck cell on its
    /// lower row, the world cell and the free one outside it.
    fn hull_side(r: &BlastRange) -> ([i32; 3], [i32; 3], [usize; 3]) {
        let u = &r.units[0];
        let cell = [30, 11, 0];
        for x in cell[0] - 3..=cell[0] + 8 {
            for y in 11..=12 {
                assert!(u.body.vol.get(x, y, 0));
                assert_eq!(u.body.part_at(x, y, 0), voxel::Part::Armour);
            }
        }
        let p = u.world_pos(cell);
        assert_eq!(
            u.hit(p),
            Some((0, cell)),
            "the belt is not where the test looks"
        );
        (p, [p[0], p[1], p[2] - 1], cell)
    }

    /// **A shallow strike deflects off a machine's intact armour**
    /// (ADR-454), as it does off steel: the plate is dented, not bored,
    /// and the round leaves turned off the plate's own face. A square
    /// strike on the same face goes in.
    #[test]
    fn a_shallow_strike_deflects_off_hull_armour() {
        let mut r = kinetic_range(vec![]);
        let (at, from, cell) = hull_side(&r);
        r.velocity = [Fx::from_int(280), Fx::ZERO, Fx::from_int(40)];
        assert!(
            r.kinetic_ricochet(at, from),
            "the hull took a grazing round"
        );
        assert!(
            r.velocity[2] < Fx::ZERO,
            "the round was not turned off the plate"
        );
        assert!(r.velocity[0] > Fx::ZERO && r.velocity[0] < Fx::from_int(280));
        // The glance deposits an eighth of the round's work, and never the
        // whole cell's: a dent, not a hole.
        let key = cell.map(|v| v as u8);
        let need = unit_work(&r.units[0].body, cell, &r.config);
        assert_eq!(
            r.units[0].dents.get(&key),
            Some(&(r.config.energy / 8).min(need - 1))
        );
        assert!(r.units[0].removed.is_empty());

        let mut square = kinetic_range(vec![]);
        square.velocity = [Fx::ZERO, Fx::ZERO, Fx::from_int(280)];
        assert!(!square.kinetic_ricochet(at, from));
        assert!(square.units[0].dents.is_empty());
    }

    /// **A turned hull deflects by its own side** (ADR-454): the face is
    /// the hull's, in its frame, not the world grid's. A round grazing
    /// along a hull yawed by 30° leaves with its component across the
    /// side reversed and its component along it kept, both scaled by the
    /// retained speed.
    #[test]
    fn a_turned_hull_deflects_by_its_own_side() {
        let mut r = kinetic_range(vec![]);
        let yaw = fx::angle::FULL / 12;
        r.units[0].motion.drive.head = yaw;
        r.units[0].update_pose(yaw);
        let u = &r.units[0];
        // The side belt, as in `hull_side`.
        let cell = [30, 11, 0];
        let at = u.world_pos(cell);
        let frame = u.root_frame(cell[1]);
        let v = frame
            .x
            .scale(Fx::from_int(280))
            .add(frame.z.scale(Fx::from_int(40)));
        r.velocity = [v.x, v.y, v.z];
        assert!(r.kinetic_ricochet(at, [at[0], at[1], at[2] - 1]));
        let out = crate::damage::V3::new(r.velocity[0], r.velocity[1], r.velocity[2]);
        let near = |got: Fx, want: i32| (got.sub(Fx::from_int(want))).abs() < Fx::from_int(2);
        assert!(
            near(out.dot(frame.x), 168),
            "along the side: {:?}",
            out.dot(frame.x)
        );
        assert!(
            near(out.dot(frame.z), -24),
            "across the side: {:?}",
            out.dot(frame.z)
        );
    }

    /// **A live round glances off the hull and flies on** (ADR-454), and
    /// the run after the glance replays exactly from a snapshot.
    #[test]
    fn a_live_round_glances_off_the_hull_and_replays_exactly() {
        let mut a = kinetic_range(vec![]);
        a.control();
        let (at, _, _) = hull_side(&a);
        // A hair outside the plate, drifting into it at a shallow angle:
        // the side is not flat further on, and a round meeting the front
        // of a step there is a square hit.
        let start = [
            Fx::from_int(at[0]).add(Fx::from_ratio(1, 10)),
            Fx::from_int(at[1]).add(Fx::from_ratio(1, 2)),
            Fx::from_int(at[2]).sub(Fx::from_ratio(3, 20)),
        ];
        assert!(!a.units[0].solid(start.map(Fx::to_int_trunc)));
        a.shell = Some(start);
        a.velocity = [Fx::from_int(280), Fx::ZERO, Fx::from_int(40)];
        a.step();
        assert!(a.impact.is_some(), "the round never touched the hull");
        assert!(a.shell.is_some(), "the round stopped in the hull");
        assert!(
            a.velocity[2] < Fx::ZERO,
            "the round was not turned off the plate"
        );
        assert!(
            a.units[0].removed.is_empty(),
            "a glancing round bored the hull"
        );
        assert!(!a.units[0].dents.is_empty());
        let mut b = BlastRange::restore(a.config, &a.snapshot()).unwrap();
        for _ in 0..60 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
    }

    #[test]
    fn square_steel_and_grazing_concrete_contacts_do_not_ricochet() {
        let mut square = kinetic_range(vec![plate(230, 231, 3)]);
        square.control();
        square.shell = Some([Fx::from_int(225), Fx::from_int(40), Fx::from_int(336)]);
        square.velocity = [Fx::from_int(280), Fx::ZERO, Fx::ZERO];
        square.step();
        assert!(square.shell.is_none());
        square.world.flush();
        assert_eq!(material(&square.world, square.impact.unwrap()), 0);

        let mut concrete = kinetic_range(vec![plate(230, 231, 2)]);
        concrete.control();
        concrete.shell = Some([
            Fx::from_ratio(2292, 10),
            Fx::from_int(40),
            Fx::from_int(331),
        ]);
        concrete.velocity = [Fx::from_int(70), Fx::ZERO, Fx::from_int(280)];
        concrete.step();
        assert!(concrete.shell.is_none());
        assert!(concrete.velocity[0] > Fx::ZERO);
        assert!(concrete.explosion.is_none());

        let mut worn = kinetic_range(vec![plate(230, 231, 3)]);
        worn.velocity = [Fx::from_int(70), Fx::ZERO, Fx::from_int(280)];
        worn.surface_work
            .insert([230, 40, 336], material_work(3, &worn.config) / 2);
        assert!(!worn.kinetic_ricochet([230, 40, 336], [229, 40, 336]));
        worn.config.steel_resistance = 1;
        worn.surface_work.clear();
        assert!(!worn.kinetic_ricochet([230, 40, 336], [229, 40, 336]));
        assert!(worn.surface_work.is_empty());
    }
    #[test]
    fn solid_shell_can_cut_concrete_but_thick_cover_exhausts_it() {
        let mut r = kinetic_range(vec![plate(230, 239, 2)]);
        r.velocity = [Fx::from_int(280), Fx::ZERO, Fx::ZERO];
        r.kinetic_impact([230, 40, 336]);
        r.world.flush();
        assert_eq!(material(&r.world, [230, 40, 336]), 0);
        assert_eq!(material(&r.world, [237, 40, 336]), 2);
        assert_eq!(r.surface_work.get(&[237, 40, 336]), Some(&800));
        assert_eq!(r.units[0].removed.len(), 0);
    }
    #[test]
    fn solid_shell_damages_a_hull_only_when_cover_does_not_spend_its_work() {
        let mut open = kinetic_range(vec![]);
        let front = (230..320)
            .find_map(|x| {
                (28..65).find_map(|y| {
                    (320..350)
                        .find(|&z| open.units[0].solid([x, y, z]))
                        .map(|z| [x, y, z])
                })
            })
            .expect("front hull voxel");
        open.velocity = [Fx::from_int(280), Fx::ZERO, Fx::ZERO];
        open.kinetic_impact(front);
        assert!(!open.units[0].removed.is_empty());
        assert!(open.explosion.is_none());

        let mut covered = kinetic_range(vec![modding::range::RangeBlock {
            min: [front[0] - 5, front[1] - 5, front[2] - 5],
            max: [front[0] - 2, front[1] + 6, front[2] + 6],
            material: 3,
        }]);
        covered.velocity = open.velocity;
        covered.kinetic_impact([front[0] - 5, front[1], front[2]]);
        assert!(covered.units[0].removed.is_empty());
        assert!(covered.explosion.is_none());
    }
    #[test]
    fn ballistic_solid_shell_is_caught_by_a_shield_and_resumes_after_restore() {
        let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        l.course = None;
        l.blocks.clear();
        l.units.clear();
        l.airburst_on_target = false;
        l.shields = vec![modding::range::RangeShieldDef {
            at: [l.target[0], 16, l.target[2]],
            radius: 40,
        }];
        let c = ExplosionConfig::from_ron(include_str!("../../../data/kinetic-shell.ron")).unwrap();
        let mut a = BlastRange::with_layout(c, l).unwrap();
        a.control();
        assert!(a.order_ground_fire([392, 24, 336]));
        for _ in 0..modding::clock::stream_ticks(20) {
            a.step();
        }
        let mut b = BlastRange::restore(c, &a.snapshot()).unwrap();
        let mut caught = false;
        for _ in 0..180 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
            if !a.shield_hits.is_empty() {
                caught = true;
                break;
            }
        }
        assert!(caught, "solid round missed the dome");
        assert!(a.impact.is_none() && a.explosion.is_none());
        assert!(a.shields[0].hits > 0);
    }
    #[test]
    fn ballistic_solid_shell_hits_a_unit_without_a_fireball() {
        let mut r = kinetic_range(vec![]);
        r.control();
        assert!(r.order_attack(0));
        for _ in 0..240 {
            r.step();
            if r.impact.is_some() {
                break;
            }
        }
        assert!(r.impact.is_some(), "solid shot never landed");
        assert!(!r.units[0].removed.is_empty(), "solid shot missed the hull");
        assert!(r.explosion.is_none());
        assert_eq!(r.heat.hot(), 0);
    }
    fn range(delivery: Delivery, wall: bool, walker: bool) -> BlastRange {
        let mut c = ExplosionConfig::from_ron(match delivery {
            Delivery::Flame => include_str!("../../../data/flamethrower.ron"),
            Delivery::MachineGun => include_str!("../../../data/machine-gun.ron"),
            Delivery::Laser => include_str!("../../../data/laser.ron"),
            Delivery::Kinetic => include_str!("../../../data/kinetic-shell.ron"),
            Delivery::Blast => include_str!("../../../data/energy-cannon.ron"),
        })
        .unwrap();
        c.energy = 1000;
        let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
        l.size = [640, 128, 640];
        l.course = None;
        l.blocks.clear();
        l.shields.clear();
        l.clear_min = [64, 64];
        l.clear_max = [600, 600];
        l.gun_kind = if walker {
            RangeUnitKind::Strider
        } else {
            RangeUnitKind::Tank
        };
        l.units.truncate(1);
        l.units[0].at = [285, 16, 336];
        l.target = [285, 30, 336];
        if wall {
            l.blocks.push(modding::range::RangeBlock {
                min: [222, 16, 290],
                max: [235, 85, 380],
                material: 3,
            });
        }
        let mut r = BlastRange::with_layout(c, l).unwrap();
        r.control();
        r.order_ground_fire([285, 30, 336]);
        r
    }
    #[test]
    fn two_machine_gun_muzzles_and_severed_barrel() {
        let mut dual = range(Delivery::MachineGun, false, true);
        dual.step();
        dual.step();
        assert_eq!(dual.streams.serial, 2, "one round per intact barrel");
        assert_eq!(dual.streams.traces.len(), 2);
        assert_ne!(dual.streams.traces[0].from, dual.streams.traces[1].from);
        let mut r = range(Delivery::MachineGun, false, true);
        let p = r.stream_muzzles();
        assert_eq!(p.len(), 2);
        assert_ne!(p[0], p[1]);
        let path = r.gun.body.weapon_paths()[0];
        for y in path.min[1]..path.max[1] {
            for z in path.min[2]..path.max[2] {
                r.gun.body.remove(33, usize::from(y), usize::from(z));
            }
        }
        r.gun.update_response();
        assert!(r.gun.status.weapon);
        assert_eq!(r.stream_muzzles().len(), 1);
        for _ in 0..15 {
            r.step();
        }
        assert!(r.streams.serial > 0);
        assert!(
            r.units[0].revision > 0,
            "bullets did no spatial work: traces={:?} muzzle={:?} target={:?}",
            r.streams.traces,
            r.stream_muzzles(),
            r.units[0].bounds()
        );
    }
    #[test]
    fn fixed_clock_keeps_fifteen_paired_machine_gun_salvos_per_second() {
        let mut r = range(Delivery::MachineGun, false, true);
        r.fire_tick = 0;
        let barrels = r.stream_muzzles().len() as u32;
        for tick in 0..modding::clock::HZ {
            r.tick = tick;
            r.step_streams_observed(&|_, _| {});
        }
        assert_eq!(r.streams.serial, barrels * 15);
        assert_eq!(r.streams.left, 0);
    }
    #[test]
    fn walker_stream_weapons_use_both_muzzles() {
        for delivery in [Delivery::MachineGun, Delivery::Flame, Delivery::Laser] {
            let mut r = range(delivery, false, true);
            let mut paired = false;
            for _ in 0..40 {
                r.step();
                let muzzles = r.stream_muzzles();
                paired |= muzzles.len() == 2
                    && muzzles
                        .iter()
                        .all(|p| r.streams.traces.iter().any(|t| t.from == *p));
                if paired {
                    break;
                }
            }
            assert!(paired, "{delivery:?} did not emit from both barrels");
        }
    }
    #[test]
    fn walker_heavy_weapons_alternate_connected_barrels() {
        for delivery in [Delivery::Blast, Delivery::Kinetic] {
            let mut r = range(delivery, false, true);
            assert_eq!(r.stream_muzzles().len(), 2);
            for index in 0..2 {
                r.shell = None;
                r.impact = None;
                r.fire_tick = r.tick + 1;
                for _ in 0..20 {
                    r.step();
                    if r.shell.is_some() {
                        break;
                    }
                }
                assert!(r.shell.is_some(), "{delivery:?} barrel {index} never fired");
                assert_eq!(
                    r.muzzle(),
                    r.stream_muzzles()[index],
                    "flight retains the fired barrel"
                );
                assert_eq!(
                    r.shell.unwrap()[2]
                        .sub(r.velocity[2].mul(Fx::from_ratio(1, crate::TICKS_PER_SECOND))),
                    r.muzzle()[2],
                    "{delivery:?} fired from the selected side"
                );
            }
        }
        let mut damaged = range(Delivery::Blast, false, true);
        let path = damaged.gun.body.weapon_paths()[0];
        for y in path.min[1]..path.max[1] {
            for z in path.min[2]..path.max[2] {
                damaged.gun.body.remove(33, usize::from(y), usize::from(z));
            }
        }
        damaged.gun.update_response();
        assert!(!damaged.gun.barrel_connected(0));
        assert!(damaged.gun.barrel_connected(1));
        assert_eq!(damaged.muzzle(), damaged.stream_muzzles()[0]);
        damaged.fire_tick = damaged.tick + 1;
        for _ in 0..20 {
            damaged.step();
            if damaged.shell.is_some() {
                break;
            }
        }
        assert!(damaged.shell.is_some(), "surviving barrel did not fire");
        assert_eq!(damaged.muzzle(), damaged.stream_muzzles()[0]);
    }
    #[test]
    fn paired_machine_guns_keep_a_bounded_fifteen_salvo_burst_and_real_hits() {
        let mut r = range(Delivery::MachineGun, false, true);
        for _ in 0..32 {
            r.step();
        }
        assert_eq!(
            r.streams.emitted(),
            30,
            "fifteen salvos from two intact barrels"
        );
        assert!(
            r.units[0].revision > 0,
            "the slower burst missed the target"
        );
        assert!(r.matter.economy.ledger.balanced());
    }
    #[test]
    fn stock_flame_transfers_fifty_times_the_old_heat_to_materials_water_and_units() {
        let mut r = range(Delivery::Flame, false, false);
        let tuning = r.streams.tuning.flame.clone();
        assert_eq!(tuning.heat, 55 * 50);
        let config = r.config;
        for (x, mat) in [(190, 5), (200, 3), (210, 2)] {
            let p = [x, 100, 336];
            r.world.queue_edit(voxel::world::Edit::put(
                p[0] as u32,
                p[1] as u32,
                p[2] as u32,
                mat,
            ));
        }
        r.world.flush();
        let water = [201, 100, 336];
        assert!(r.water.edit_volume(water, true));
        for x in [190, 200, 210] {
            let p = [x, 100, 336];
            r.stream_contact(p, &tuning, true, r.gun.at, &config);
            assert_eq!(r.heat.cell(p.map(|v| v as u32)).heat, tuning.heat);
        }
        let expected_vapor = (tuning.heat * 3 / 4 / r.water.tuning.evaporation_heat) as u64;
        assert_eq!(r.water.stats().vapor_units, expected_vapor);
        let (point, _, cell) = hull_side(&r);
        r.stream_contact(point, &tuning, true, r.gun.at, &config);
        assert_eq!(
            r.thermals.heat(thermal::Cell {
                unit: 0,
                part: 0,
                at: cell.map(|v| v as u8)
            }),
            tuning.heat
        );
    }
    #[test]
    fn crawler_drill_does_not_turn_the_chassis_into_a_gun() {
        let mut r = range(Delivery::Flame, false, false);
        r.gun = RangeUnit::of_kind(RangeUnitKind::Crawler, r.gun.at);
        assert!(r.gun.status.weapon, "intact drill should be available");
        assert!(!r.ready());
        assert!(!r.refire());
    }
    #[test]
    fn flame_flight_cover_and_local_damage() {
        let mut open = range(Delivery::Flame, false, false);
        let mut wall = range(Delivery::Flame, true, false);
        for _ in 0..6 {
            open.step();
            wall.step();
        }
        assert_eq!(open.units[0].revision, 0, "flame hit before arriving");
        assert!(!open.streams.rounds.is_empty());
        for _ in 0..85 {
            open.step();
            wall.step();
        }
        assert!(open.units[0].revision > 0, "flame never reached target");
        assert_eq!(wall.units[0].revision, 0, "flame went through cover");
        assert_eq!(wall.thermals.hot(), 0, "heat teleported through cover");
        assert!(open.thermals.hot() > 0, "the exposed body retained no heat");
        assert!(open.explosion.is_none() && wall.explosion.is_none());
        assert!(
            open.units[0].detached > 0,
            "thermal ablation produced no conserved debris"
        );
    }
    #[test]
    fn stream_snapshot_replays_in_flight_and_stop_keeps_packets() {
        let mut a = range(Delivery::Flame, false, true);
        for _ in 0..modding::clock::stream_ticks(9) {
            a.step();
        }
        let mut b = BlastRange::restore(a.config, &a.snapshot()).expect("stream save");
        for _ in 0..modding::clock::stream_ticks(50) {
            a.step();
            b.step();
        }
        assert_eq!(a.state_hash(), b.state_hash());
        assert!(!a.streams.rounds.is_empty());
        let n = a.streams.serial;
        a.order_stop();
        for _ in 0..40 {
            a.step();
        }
        assert_eq!(a.streams.serial, n);
        assert!(a.streams.rounds.is_empty());
        assert!(a.ready());
    }
    #[test]
    fn retuning_launch_speed_does_not_shorten_rounds_already_in_flight() {
        let mut a = range(Delivery::Flame, false, false);
        for _ in 0..6 {
            a.step();
        }
        a.order_stop();
        let mut b = BlastRange::restore(a.config, &a.snapshot()).unwrap();
        let mut faster = b.config;
        faster.shell_speed = 10000;
        b.reconfigure(faster);
        // The live setting is now authoritative and included in the hash.
        // Restore must use it even when the caller still has the old preset.
        let mut copy = BlastRange::restore(a.config, &b.snapshot()).unwrap();
        for _ in 0..40 {
            a.step();
            b.step();
            copy.step();
            assert_eq!(b.snapshot(), copy.snapshot());
        }
        assert!(
            a.thermals.hot() > 0,
            "in-flight heat never reached the body"
        );
        assert_ne!(a.state_hash(), b.state_hash());
        // Compare the complete physical result with identical configuration
        // metadata, after all existing rounds have already travelled.
        a.reconfigure(faster);
        assert_eq!(a.snapshot(), b.snapshot());
    }
    #[test]
    fn flame_ignites_wood_and_shield_stops_packets() {
        let mut r = range(Delivery::Flame, false, false);
        for y in 16..55 {
            for z in 325..348 {
                r.world.queue_edit(voxel::world::Edit::put(220, y, z, 5));
            }
        }
        r.world.flush();
        for _ in 0..65 {
            r.step();
        }
        assert!(
            r.heat.burned_total > 0 || r.heat.burning() > 0,
            "wood did not catch fire"
        );
        let mut s = range(Delivery::MachineGun, false, true);
        // This tests interception, not depletion. Match the tuning to the
        // strong fixture: recharge otherwise clamps its charge to stock 4000.
        s.shield_tuning.capacity = 100000;
        s.shields.push(RangeShield::new(
            &modding::range::RangeShieldDef {
                at: [285, 16, 336],
                radius: 60,
            },
            100000,
        ));
        for _ in 0..20 {
            s.step();
        }
        assert!(s.shields[0].hits > 0);
        assert_eq!(s.units[0].revision, 0);
    }
}
