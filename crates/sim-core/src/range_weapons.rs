//! Finite flight packets and spatial contacts, independent of explosion waves.
use super::*;
use fx::blob::{Blob, Scan};
use modding::{
    explosion::Delivery,
    range_weapon::{Stream, Tuning},
};
const MAX_ROUNDS: usize = 128;

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
impl BlastRange {
    /// The single gun's round against what it met: [`Self::ricochet`].
    pub(super) fn kinetic_ricochet(&mut self, at: [i32; 3], from: [i32; 3]) -> bool {
        let config = self.config.clone();
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
        let Some(tuning) = config.ricochet.clone() else {
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
    /// budget before the ray can reach a hull behind it. No radial wave is
    /// created, so a blocked shot cannot damage nearby units.
    pub(super) fn kinetic_impact(&mut self, at: [i32; 3]) {
        let config = self.config.clone();
        self.kinetic_hit(at, self.velocity, &config);
    }

    /// [`Self::kinetic_impact`] for any solid round, with the weapon that
    /// fired it (ADR-456). The hit pays from a budget of its own: a burst
    /// in flight keeps what it has left.
    pub(super) fn kinetic_hit(
        &mut self,
        at: [i32; 3],
        velocity: [Fx; 3],
        config: &ExplosionConfig,
    ) {
        let length = velocity
            .iter()
            .fold(Fx::ZERO, |n, v| n.add(v.mul(*v)))
            .sqrt();
        if length <= Fx::ZERO {
            return;
        }
        let direction = velocity.map(|v| v.div(length));
        let horizontal_sq = velocity[0]
            .mul(velocity[0])
            .add(velocity[2].mul(velocity[2]));
        let launch_sq = Fx::from_int(config.shell_speed).mul(Fx::from_int(config.shell_speed));
        let remaining = horizontal_sq.div(launch_sq).min(Fx::ONE);
        let mut work =
            (u64::from(config.energy) * remaining.raw() as u64 / Fx::ONE.raw() as u64) as u32;
        if work == 0 {
            return;
        }
        let mut previous = None;
        let before: Vec<_> = self
            .units
            .iter()
            .map(|u| (u.removed.len(), u.seg_removed.len()))
            .collect();
        let burst = (self.matter.remaining_mj, self.matter.fragment_box());
        self.matter.begin(config.material_energy_j, 1);
        // Half-cell samples cannot skip a one-cell plate even at a diagonal.
        // The 32-unit bound also makes an empty ray a fixed, small cost.
        for i in 0..64 {
            let distance = Fx::from_ratio(i, 2);
            let p = std::array::from_fn(|a| {
                Fx::from_int(at[a])
                    .add(direction[a].mul(distance))
                    .to_int_trunc()
            });
            if previous == Some(p) {
                continue;
            }
            previous = Some(p);
            if let Some(id) = self.units.iter().position(|u| u.solid(p)) {
                let u = &mut self.units[id];
                let Some((part, cell)) = u.hit(p) else {
                    break;
                };
                if u.section_holds(part, cell.map(|v| v as u8)) {
                    break;
                }
                let need = u.resistance(p, config).max(1);
                let spent = work.min(need);
                if spent == need {
                    let result = if part == 0 {
                        self.matter.body(id, u, cell, &u.root_frame(cell[1]), at)
                    } else {
                        self.matter
                            .body_segment(id, u, part - 1, cell, &u.seg_frames[part - 1], at)
                    };
                    if !matches!(result, matter::ClaimResult::Applied) {
                        break;
                    }
                    if part == 0 {
                        u.detached += 1;
                    } else {
                        u.seg_detached += 1;
                    }
                }
                if part == 0 {
                    u.receive_cell(cell, spent, config);
                } else {
                    u.receive_seg_cell(part - 1, cell, spent, config);
                }
                work -= spent;
                if work == 0 {
                    break;
                }
            } else {
                let mat = material(&self.world, p);
                if mat == 0 {
                    continue;
                }
                let old = self.surface_work.get(&p).copied().unwrap_or(0);
                let need = material_work(mat, config).saturating_sub(old).max(1);
                let spent = work.min(need);
                if spent == need {
                    if !matches!(
                        self.matter.ground(&mut self.world, p, at),
                        matter::ClaimResult::Applied
                    ) {
                        break;
                    }
                    self.surface_work.remove(&p);
                } else {
                    self.surface_work.insert(p, old.saturating_add(spent));
                }
                self.surface_changed.push(p);
                work -= spent;
                if work == 0 {
                    break;
                }
            }
        }
        self.matter.remaining_mj = burst.0;
        self.matter.fragment_size(u32::from(burst.1));
        for (id, (u, old)) in self.units.iter_mut().zip(before).enumerate() {
            if (u.removed.len(), u.seg_removed.len()) != old {
                u.refresh_damage(id);
            }
        }
    }

    /// Authored barrel ends, with independent connectivity for the two guns.
    pub fn stream_muzzles(&self) -> Vec<[Fx; 3]> {
        muzzles(&self.gun, self.turret.angle)
    }
    fn emit_laser(&mut self, tuning: &Stream) {
        for from in self.stream_muzzles() {
            let target = self.target();
            let delta = std::array::from_fn::<_, 3, _>(|a| Fx::from_int(target[a]).sub(from[a]));
            let length = delta.iter().fold(Fx::ZERO, |n, v| n.add(v.mul(*v))).sqrt();
            if length <= Fx::ZERO {
                continue;
            }
            let direction = delta.map(|v| v.div(length));
            let serial = self.streams.serial;
            self.streams.serial = serial.wrapping_add(1);
            let mut previous = from.map(Fx::to_int_trunc);
            let mut end = from;
            let mut hit = false;
            // Half-cell samples cannot skip a thin plate at a diagonal. Stop
            // at the first surface, so a wall protects the hull behind it.
            for i in 1..=tuning.range * 2 {
                let distance = Fx::from_ratio(i, 2);
                let point = std::array::from_fn(|a| from[a].add(direction[a].mul(distance)));
                let p = point.map(Fx::to_int_trunc);
                if (0..3).any(|a| p[a] < 0 || p[a] >= self.size[a] as i32) {
                    break;
                }
                end = point;
                if p == previous {
                    continue;
                }
                if let Some(shield) = self.dome_crossed(previous, p) {
                    self.absorb_work(shield, p, tuning.work);
                    hit = true;
                    break;
                }
                if material(&self.world, p) != 0 || self.units.iter().any(|u| u.solid(p)) {
                    self.stream_contact(p, tuning, true, self.gun.at, &self.config.clone());
                    hit = true;
                    break;
                }
                previous = p;
            }
            self.streams.traces.push(Trace {
                from,
                to: end,
                hit,
                serial,
            });
        }
    }
    fn emit_stream(&mut self, tuning: &Stream) {
        let flame = self.config.delivery == Delivery::Flame;
        let muzzles = self.stream_muzzles();
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
        if let Some(id) = self.units.iter().position(|u| u.solid(p)) {
            let u = &mut self.units[id];
            let Some((part, cell)) = u.hit(p) else {
                return;
            };
            if u.section_holds(part, cell.map(|v| v as u8)) {
                return;
            }
            if flame {
                self.thermals.deposit(
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
                if self.config.delivery == Delivery::Laser {
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
                    if matches!(
                        self.matter.ground(&mut self.world, p, at),
                        matter::ClaimResult::Applied
                    ) {
                        self.surface_work.remove(&p);
                    }
                } else {
                    self.surface_work.insert(p, total);
                }
                self.surface_changed.push(p);
            }
        }
    }
    pub(super) fn step_streams(&mut self) {
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
        if self.tick == self.fire_tick && self.gun.status.alive && self.gun.status.weapon {
            self.streams.left = tuning.burst_ticks;
            self.streams.next = self.tick;
        }
        if !self.gun.status.alive || !self.gun.status.weapon {
            self.streams.left = 0;
        }
        if self.streams.left > 0 {
            if self.tick >= self.streams.next
                && self.turret.aligned(self.aim_angle(), self.turret_limits)
            {
                if self.config.delivery == Delivery::Laser {
                    self.emit_laser(&tuning);
                } else {
                    self.emit_stream(&tuning);
                }
                self.streams.next = self.tick + tuning.interval_ticks;
            }
            self.streams.left -= 1;
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
        let mut survivors = Vec::with_capacity(MAX_ROUNDS);
        let step = Fx::from_ratio(1, crate::TICKS_PER_SECOND);
        for mut round in rounds {
            let from = round.pos;
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
    let f = crate::damage::turned(&u.pose, angle, u.motion.drive.head);
    u.body
        .weapon_paths()
        .iter()
        .filter(|p| u.body.barrel_connected(p, u.connectivity.reached_cols()))
        .take(2)
        .map(|p| {
            let mut cell = std::array::from_fn(|a| {
                Fx::from_ratio(i32::from(p.min[a]) + i32::from(p.max[a]), 2)
            });
            cell[usize::from(p.axis)] = Fx::from_int(i32::from(p.max[usize::from(p.axis)]) + 1);
            let v = f.world(cell);
            [v.x, v.y, v.z]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn kinetic_range(blocks: Vec<modding::range::RangeBlock>) -> BlastRange {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/kinetic-shell.ron")).unwrap();
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
    fn laser_slowly_melts_a_steel_wall_without_hurting_the_sheltered_unit() {
        let mut r = laser_range(vec![modding::range::RangeBlock {
            min: [230, 0, 330],
            max: [246, 60, 342],
            material: 3,
        }]);
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
        assert_eq!(material(&r.world, first), 0, "front steel did not melt");
        assert_eq!(material(&r.world, [244, first[1], first[2]]), 3);
        assert!(r.heat.melted_total > 0);
        assert!(r.units[0].removed.is_empty(), "wall failed to shield unit");
        assert!(r.explosion.is_none());
        assert!(r.shell.is_none());
    }
    #[test]
    fn laser_softens_and_removes_armour_without_a_blast() {
        let mut r = laser_range(vec![]);
        r.control();
        r.aim = Some([285, 39, 336]);
        assert!(r.refire());
        for _ in 0..3 {
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
        let mut a = BlastRange::with_layout(c.clone(), l).unwrap();
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

        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
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
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
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
        let mut a = BlastRange::with_layout(c.clone(), l).unwrap();
        a.control();
        assert!(a.order_ground_fire([392, 24, 336]));
        for _ in 0..20 {
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
                    r.shell.unwrap()[2],
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
        for _ in 0..9 {
            a.step();
        }
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).expect("stream save");
        for _ in 0..50 {
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
        let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
        let mut faster = b.config.clone();
        faster.shell_speed = 10000;
        b.reconfigure(faster);
        for _ in 0..40 {
            a.step();
            b.step();
        }
        assert!(
            a.thermals.hot() > 0,
            "in-flight heat never reached the body"
        );
        assert_eq!(a.state_hash(), b.state_hash());
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
