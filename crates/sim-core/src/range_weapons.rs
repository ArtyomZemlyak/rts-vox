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
    serial: u32,
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
    /// Authored barrel ends, with independent connectivity for the two guns.
    pub fn stream_muzzles(&self) -> Vec<[Fx; 3]> {
        muzzles(&self.gun, self.turret.angle)
    }
    fn emit_stream(&mut self, tuning: &Stream) {
        let flame = self.config.delivery == Delivery::Flame;
        let muzzles = self.stream_muzzles();
        for (barrel, from) in muzzles.into_iter().enumerate() {
            if flame && barrel > 0 {
                break;
            }
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
                let len = delta
                    .iter()
                    .fold(Fx::ZERO, |n, v| n.add(v.mul(*v)))
                    .sqrt()
                    .max(Fx::ONE);
                let velocity = delta.map(|v| v.div(len).mul(Fx::from_int(self.config.shell_speed)));
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
                self.heat.deposit(
                    &self.world,
                    &self.matter.economy.mats,
                    p,
                    tuning.heat_radius,
                    tuning.heat,
                );
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
        if self.config.delivery == Delivery::Blast {
            return;
        }
        let flame = self.config.delivery == Delivery::Flame;
        let mut tuning = if flame {
            &self.streams.tuning.flame
        } else {
            &self.streams.tuning.machine_gun
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
                self.emit_stream(&tuning);
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
                    let charge = self.shields[shield].charge;
                    let drained = charge.min(tuning.work);
                    self.shields[shield].charge -= drained;
                    self.shields[shield].hits += 1;
                    if drained == charge {
                        self.shields[shield].down = self.shield_tuning.overload_ticks;
                    }
                    self.shield_hits.push(ShieldHit {
                        shield,
                        at: p,
                        drained,
                        collapsed: drained == charge,
                    });
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
    fn range(delivery: Delivery, wall: bool, walker: bool) -> BlastRange {
        let mut c = ExplosionConfig::from_ron(if delivery == Delivery::Flame {
            include_str!("../../../data/flamethrower.ron")
        } else {
            include_str!("../../../data/machine-gun.ron")
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
