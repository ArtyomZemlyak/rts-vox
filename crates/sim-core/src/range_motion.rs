//! Mobile actors on the forest range. Shared drive and suspension, integer
//! route planning against the current destructible voxel world.
use crate::drive::{self, Drive, Limits};
use fx::{
    angle,
    blob::{Blob, Scan},
    Fx,
};
use phys::{Chassis, V3};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use voxel::{UnitBody, World};

#[derive(Clone, Copy)]
pub struct Obstacle {
    pub pos: [Fx; 3],
    pub head: i32,
    pub half: [Fx; 2],
}
impl Obstacle {
    pub fn of(m: &Motion, b: &UnitBody) -> Self {
        Self {
            pos: m.pos,
            head: m.drive.head,
            half: [
                Fx::from_ratio(b.vol.nx as i32, 2),
                Fx::from_ratio(b.vol.nz as i32, 2),
            ],
        }
    }
    fn overlaps(&self, other: &Self) -> bool {
        let axes = |head: i32| {
            let c = angle::cos(head);
            let s = angle::sin(head);
            [[c, s], [s.neg(), c]]
        };
        let a = axes(self.head);
        let b = axes(other.head);
        let dot = |u: [Fx; 2], v: [Fx; 2]| u[0].mul(v[0]).add(u[1].mul(v[1]));
        let delta = [other.pos[0].sub(self.pos[0]), other.pos[2].sub(self.pos[2])];
        a.iter().chain(&b).all(|n| {
            let ra = dot(*n, a[0])
                .abs()
                .mul(self.half[0])
                .add(dot(*n, a[1]).abs().mul(self.half[1]));
            let rb = dot(*n, b[0])
                .abs()
                .mul(other.half[0])
                .add(dot(*n, b[1]).abs().mul(other.half[1]));
            dot(delta, *n).abs() < ra.add(rb)
        })
    }
}
#[derive(Clone, Debug)]
pub struct Motion {
    pub pos: [Fx; 3],
    pub drive: Drive,
    pub hull: Chassis,
    pub phase: i32,
    pub path: Vec<[i32; 2]>,
    pub cursor: usize,
    pub destination: Option<[i32; 2]>,
    pub route: Vec<[i32; 2]>,
    pub waypoint: usize,
    pub laps: u32,
    pub enabled: bool,
    pub blocked: bool,
    pub chassis: modding::tuning::Chassis,
    pub speed: i32,
    pub step: i32,
    pub arrival: i32,
    pub alignment: i32,
    pub replan: u32,
}
impl Motion {
    pub fn new(at: [i32; 3], kind: modding::range::RangeUnitKind) -> Self {
        let t = crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).expect("tuning");
        let chassis = match kind {
            modding::range::RangeUnitKind::Tank => t.tracked,
            _ => t.walking,
        };
        let r = crate::ride::ride_of(&chassis);
        let c =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-cover.ron"))
                .expect("range tuning")
                .course
                .expect("course tuning");
        Self {
            pos: at.map(Fx::from_int),
            drive: Drive {
                head: 0,
                vel: Fx::ZERO,
                yaw_rate: 0,
            },
            hull: Chassis::resting(Fx::from_int(at[1]), &r),
            phase: 0,
            path: Vec::new(),
            cursor: 0,
            destination: None,
            route: Vec::new(),
            waypoint: 0,
            laps: 0,
            enabled: false,
            blocked: false,
            chassis,
            speed: if kind == modding::range::RangeUnitKind::Tank {
                c.tank_speed
            } else {
                c.walker_speed
            },
            step: c.step_height,
            arrival: c.arrival,
            alignment: c.turn_alignment_pct,
            replan: c.replan_ticks,
        }
    }
    pub fn frame(&self, b: &UnitBody) -> crate::damage::Frame {
        crate::damage::Frame::new(
            b,
            V3::new(self.pos[0], self.pos[1], self.pos[2]),
            self.drive.head,
            self.hull.up,
        )
    }
    pub fn move_to(&mut self, w: &World, to: [i32; 2], b: &UnitBody, others: &[Obstacle]) -> bool {
        let from = [self.pos[0].to_int_trunc(), self.pos[2].to_int_trunc()];
        let radius = (b.vol.nz as i32 / 2).max(10);
        let path = route(w, from, to, self.step, radius, others);
        self.destination = to.iter().all(|v| (40..600).contains(v)).then_some(to);
        self.blocked = path.is_empty();
        self.path = path;
        self.cursor = 0;
        self.enabled = true;
        !self.blocked
    }
    pub fn stop(&mut self) {
        self.path.clear();
        self.cursor = 0;
        self.destination = None;
        self.blocked = false;
    }
    // Drive phase takes explicit body, world, clock and contact inputs.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        w: &World,
        b: &UnitBody,
        alive: bool,
        engine: bool,
        others: &[Obstacle],
        tick: u32,
        gravity: i32,
    ) {
        if !self.enabled {
            return;
        }
        if (self.blocked || self.path.is_empty()) && tick.is_multiple_of(self.replan) {
            if let Some(goal) = self.destination {
                self.move_to(w, goal, b, others);
            }
        }
        if self.cursor == self.path.len()
            && !self.route.is_empty()
            && alive
            && engine
            && (!self.blocked || tick.is_multiple_of(self.replan))
        {
            let goal = self.route[self.waypoint];
            self.move_to(w, goal, b, others);
        }
        let mut want = drive::NO_WAY;
        let mut speed = Fx::ZERO;
        if alive && engine && self.cursor < self.path.len() {
            let goal = self.path[self.cursor];
            let dx = Fx::from_int(goal[0]).sub(self.pos[0]);
            let dz = Fx::from_int(goal[1]).sub(self.pos[2]);
            if dx.mul(dx).add(dz.mul(dz)) <= Fx::from_int(self.arrival * self.arrival) {
                self.cursor += 1;
                if self.cursor == self.path.len() {
                    self.destination = None;
                }
                if self.cursor == self.path.len() && !self.route.is_empty() {
                    self.waypoint = (self.waypoint + 1) % self.route.len();
                    if self.waypoint == 0 {
                        self.laps += 1;
                    }
                }
            } else {
                want = angle::atan2(dz.raw(), dx.raw());
                speed = Fx::from_int(self.speed).min(dx.abs().max(dz.abs()).mul(Fx::from_int(2)));
                let alignment = angle::cos(angle::diff(want, self.drive.head)).max(Fx::ZERO);
                speed = speed.mul(if alignment > Fx::from_ratio(self.alignment, 100) {
                    alignment
                } else {
                    Fx::ZERO
                });
            }
        }
        let lim = Limits::of(&self.chassis, 30);
        let mass =
            (b.volume_mm3() as u128 * self.chassis.density as u128 / 1_000_000_000).max(1) as i64;
        let factor = drive::drive_factor(self.chassis.mass as i64 * 1000, mass);
        let old = self.drive;
        let (next, delta) = drive::advance(
            old,
            want,
            speed,
            &lim,
            factor.mul(self.hull.load),
            Fx::from_ratio(1, 30),
        );
        self.drive = next;
        let nx = self.pos[0].add(delta.dx);
        let nz = self.pos[2].add(delta.dz);
        let (x, z) = (nx.to_int_trunc(), nz.to_int_trunc());
        let current = floor(w, self.pos[0].to_int_trunc(), self.pos[2].to_int_trunc());
        let target = floor(w, x, z);
        let mut candidate = Obstacle::of(self, b);
        candidate.pos[0] = nx;
        candidate.pos[2] = nz;
        let collision = others.iter().any(|p| candidate.overlaps(p));
        let (hc, hs) = crate::ride::heading_of(next.head);
        let support = crate::ride::supports_of(&self.chassis);
        let clear = support.iter().all(|s| {
            let px = nx.add(s.x.mul(hc)).sub(s.z.mul(hs)).to_int_trunc();
            let pz = nz.add(s.x.mul(hs)).add(s.z.mul(hc)).to_int_trunc();
            floor(w, px, pz) - target <= self.step + (b.vol.nx as i32 / 8)
        });
        let allowed = clear
            && x >= 40
            && z >= 40
            && x < 600
            && z < 600
            && target - current <= self.step
            && !collision;
        let path = if allowed {
            self.blocked = false;
            self.pos[0] = nx;
            self.pos[2] = nz;
            next.vel.div(Fx::from_int(30))
        } else {
            self.drive.vel = Fx::ZERO;
            if collision {
                self.drive.head = old.head;
                self.drive.yaw_rate = 0;
            }
            self.blocked = true;
            Fx::ZERO
        };
        if let Some(c) = drive::StepCycle::of(&self.chassis) {
            self.phase = angle::norm(
                self.phase
                    .wrapping_add(c.advance(path, angle::diff(self.drive.head, old.head))),
            );
        }
        let support = crate::ride::supports_of(&self.chassis);
        let (hc, hs) = crate::ride::heading_of(self.drive.head);
        let floors = support.map(|s| {
            let x = self.pos[0].add(s.x.mul(hc)).sub(s.z.mul(hs)).to_int_trunc();
            let z = self.pos[2].add(s.x.mul(hs)).add(s.z.mul(hc)).to_int_trunc();
            Fx::from_int(floor(w, x, z))
        });
        let mut r = crate::ride::ride_of(&self.chassis);
        r.gravity = Fx::from_int(gravity);
        r.preload = r.gravity.div(Fx::from_int(4));
        let sway = crate::ride::sway_of(
            next.vel.sub(old.vel).mul(Fx::from_int(30)),
            next.vel,
            angle::diff(next.head, old.head),
            next.head,
        );
        phys::step(
            &mut self.hull,
            &support,
            &floors,
            (hc, hs),
            sway,
            &r,
            Fx::from_ratio(1, 30),
        );
        self.pos[1] = self.hull.y.sub(r.clearance);
    }
    pub fn save(&self, b: &mut Blob) {
        for v in self.pos {
            b.fx(v);
        }
        b.i32(self.drive.head);
        b.fx(self.drive.vel);
        b.i32(self.drive.yaw_rate);
        for v in [
            self.hull.y,
            self.hull.vy,
            self.hull.up.x,
            self.hull.up.y,
            self.hull.up.z,
            self.hull.w.x,
            self.hull.w.y,
            self.hull.w.z,
            self.hull.load,
        ] {
            b.fx(v);
        }
        b.i32(self.phase);
        b.usize(self.cursor);
        b.bool(self.destination.is_some());
        if let Some(p) = self.destination {
            for v in p {
                b.i32(v);
            }
        }
        b.usize(self.waypoint);
        b.u32(self.laps);
        b.bool(self.enabled);
        b.bool(self.blocked);
        for list in [&self.path, &self.route] {
            b.u32(list.len() as u32);
            for p in list {
                b.i32(p[0]);
                b.i32(p[1]);
            }
        }
    }
    pub fn load(&mut self, s: &mut Scan) -> bool {
        self.pos = std::array::from_fn(|_| s.fx());
        self.drive = Drive {
            head: s.i32(),
            vel: s.fx(),
            yaw_rate: s.i32(),
        };
        self.hull = Chassis {
            y: s.fx(),
            vy: s.fx(),
            up: V3::new(s.fx(), s.fx(), s.fx()),
            w: V3::new(s.fx(), s.fx(), s.fx()),
            load: s.fx(),
        };
        self.phase = s.i32();
        self.cursor = s.usize();
        self.destination = s.bool().then(|| [s.i32(), s.i32()]);
        self.waypoint = s.usize();
        self.laps = s.u32();
        self.enabled = s.bool();
        self.blocked = s.bool();
        for list in [&mut self.path, &mut self.route] {
            list.clear();
            let n = s.u32();
            if n > 6400 {
                return false;
            }
            for _ in 0..n {
                list.push([s.i32(), s.i32()]);
            }
        }
        s.ok()
            && self.pos[0] >= Fx::ZERO
            && self.pos[0] < Fx::from_int(640)
            && self.pos[2] >= Fx::ZERO
            && self.pos[2] < Fx::from_int(640)
            && self.pos[1].abs() < Fx::from_int(384)
            && self.drive.vel.abs() <= Fx::from_int(200)
            && [self.hull.up.x, self.hull.up.y, self.hull.up.z]
                .iter()
                .all(|v| v.abs() <= Fx::ONE)
            && self.hull.load >= Fx::ZERO
            && self.hull.load <= Fx::ONE
            && self
                .path
                .iter()
                .chain(&self.route)
                .all(|p| p.iter().all(|v| *v >= 0 && *v < 640))
            && self
                .destination
                .is_none_or(|p| p.iter().all(|v| (40..600).contains(v)))
            && self.cursor <= self.path.len()
            && (self.route.is_empty() || self.waypoint < self.route.len())
    }
}
pub fn floor(w: &World, x: i32, z: i32) -> i32 {
    if x < 0 || z < 0 || x >= w.nx as i32 || z >= w.nz as i32 {
        return 192;
    }
    for y in (0..w.ny).rev() {
        if w.material(x as u32, y, z as u32) != 0 {
            return y as i32 + 1;
        }
    }
    0
}
fn route(
    w: &World,
    from: [i32; 2],
    to: [i32; 2],
    step: i32,
    radius: i32,
    others: &[Obstacle],
) -> Vec<[i32; 2]> {
    const N: usize = 80;
    const EDGE: i32 = 8;
    if to.iter().any(|v| *v < 40 || *v >= 600) {
        return Vec::new();
    }
    let start = from[0].div_euclid(EDGE) as usize + N * from[1].div_euclid(EDGE) as usize;
    let end = to[0].div_euclid(EDGE) as usize + N * to[1].div_euclid(EDGE) as usize;
    let point = |i: usize| {
        [
            (i % N) as i32 * EDGE + EDGE / 2,
            (i / N) as i32 * EDGE + EDGE / 2,
        ]
    };
    let mut height = BTreeMap::new();
    let mut sample = |i: usize| -> (i32, bool) {
        *height.entry(i).or_insert_with(|| {
            let p = point(i);
            let h = floor(w, p[0], p[1]);
            let ok = [[-radius, 0], [radius, 0], [0, -radius], [0, radius]]
                .iter()
                .all(|d| (floor(w, p[0] + d[0], p[1] + d[1]) - h).abs() <= step + radius / 4);
            let candidate = Obstacle {
                pos: [Fx::from_int(p[0]), Fx::from_int(h), Fx::from_int(p[1])],
                head: 0,
                half: [Fx::from_int(radius); 2],
            };
            (h, ok && !others.iter().any(|o| candidate.overlaps(o)))
        })
    };
    let mut dist = vec![u32::MAX; N * N];
    let mut previous = vec![usize::MAX; N * N];
    let mut open = BinaryHeap::new();
    dist[start] = 0;
    open.push(Reverse((0u32, start)));
    for _ in 0..N * N {
        let Some(Reverse((_, i))) = open.pop() else {
            break;
        };
        if i == end {
            break;
        }
        let (h, _) = sample(i);
        for (dx, dz) in [
            (-1, 0),
            (1, 0),
            (0, -1),
            (0, 1),
            (-1, -1),
            (-1, 1),
            (1, -1),
            (1, 1),
        ] {
            let x = i as i32 % N as i32 + dx;
            let z = i as i32 / N as i32 + dz;
            if x < 5 || z < 5 || x >= 75 || z >= 75 {
                continue;
            }
            let j = x as usize + N * z as usize;
            let (nh, ok) = sample(j);
            if !ok || (nh - h).abs() > step {
                continue;
            }
            if dx != 0
                && dz != 0
                && (!sample((x as usize) + N * (i / N)).1 || !sample(i % N + N * z as usize).1)
            {
                continue;
            }
            let cost = dist[i].saturating_add(if dx == 0 || dz == 0 { 10 } else { 14 });
            if cost < dist[j] {
                dist[j] = cost;
                previous[j] = i;
                let q = point(j);
                let heuristic = ((q[0] - to[0]).abs().max((q[1] - to[1]).abs()) / EDGE * 10) as u32;
                open.push(Reverse((cost + heuristic, j)));
            }
        }
    }
    if dist[end] == u32::MAX {
        return Vec::new();
    }
    let mut ids = vec![end];
    while *ids.last().unwrap() != start {
        let i = previous[*ids.last().unwrap()];
        if i == usize::MAX {
            return Vec::new();
        }
        ids.push(i);
    }
    ids.reverse();
    let all: Vec<_> = ids.into_iter().map(point).collect();
    let mut out = Vec::new();
    for i in 1..all.len() {
        if i + 1 == all.len()
            || [all[i][0] - all[i - 1][0], all[i][1] - all[i - 1][1]]
                != [all[i + 1][0] - all[i][0], all[i + 1][1] - all[i][1]]
        {
            out.push(all[i]);
        }
    }
    if out.is_empty() {
        out.push(to);
    } else {
        *out.last_mut().unwrap() = to;
    }
    out
}
