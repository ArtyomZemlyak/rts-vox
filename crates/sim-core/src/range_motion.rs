//! Mobile actors on the forest range. Shared drive and suspension, integer
//! route planning against the current destructible voxel world.
use crate::drive::{self, Drive, Limits};
use fx::{
    angle,
    blob::{Blob, Scan},
    Fx,
};
use phys::{Chassis, V3};
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
                Fx::from_ratio(
                    b.vol.nx as i32 * b.mm as i32,
                    2 * voxel::unit::WORLD_MM as i32,
                ),
                Fx::from_ratio(
                    b.vol.nz as i32 * b.mm as i32,
                    2 * voxel::unit::WORLD_MM as i32,
                ),
            ],
        }
    }
    fn penetration(&self, other: &Self) -> Fx {
        let axes = |head: i32| {
            let c = angle::cos(head);
            let s = angle::sin(head);
            [[c, s], [s.neg(), c]]
        };
        let a = axes(self.head);
        let b = axes(other.head);
        let dot = |u: [Fx; 2], v: [Fx; 2]| u[0].mul(v[0]).add(u[1].mul(v[1]));
        let delta = [other.pos[0].sub(self.pos[0]), other.pos[2].sub(self.pos[2])];
        a.iter()
            .chain(&b)
            .map(|n| {
                let ra = dot(*n, a[0])
                    .abs()
                    .mul(self.half[0])
                    .add(dot(*n, a[1]).abs().mul(self.half[1]));
                let rb = dot(*n, b[0])
                    .abs()
                    .mul(other.half[0])
                    .add(dot(*n, b[1]).abs().mul(other.half[1]));
                ra.add(rb).sub(dot(delta, *n).abs()).max(Fx::ZERO)
            })
            .min()
            .unwrap_or(Fx::ZERO)
    }
    fn overlaps(&self, other: &Self) -> bool {
        self.penetration(other) > Fx::ZERO
    }
}
/// Manoeuvres in one escape cycle: astern, ahead, then each turning both ways.
const ESCAPES: u32 = 6;
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
    /// Yielding cells the hull stood in after this tick's move: the cut the
    /// range turns into debris. Transient; recomputed every step.
    pub cut: Vec<[u32; 3]>,
    /// Support floors used by the last suspension step, for diagnostics.
    pub floors: [Fx; 4],
    /// Planting state of each leg, in leg order (ADR-279); empty for a
    /// tracked hull. Saved.
    pub feet: Vec<crate::gait::Foot>,
    /// Legs that left the body whole (ADR-278): they neither step nor
    /// carry (ADR-280). Saved.
    pub legs_lost: Vec<bool>,
    /// Legs that lost more than the buckling share of their links
    /// (ADR-281): folded under the hip, carrying nothing. Saved.
    pub legs_buckled: Vec<bool>,
    /// Legs computed once from the body.
    rigs: Vec<crate::gait::LegRig>,
    // Installed engine sizing is based on the pristine machine. Losing armour
    // must not silently resize the motor each movement tick.
    drive_scale: Fx,
    navigation: modding::navigation::Navigation,
    /// Materials this hull crushes instead of avoiding; empty for walkers.
    yielding: Vec<voxel::store::Mat>,
    blocked_ticks: u32,
    /// Ticks left in the current escape manoeuvre; zero while following
    /// the plan.
    escape_left: u32,
    /// Escape manoeuvres begun since the last waypoint. The next one to
    /// try cycles astern, ahead, then astern and ahead turning each way
    /// (ADR-275).
    escape: u32,
    /// The last search could not leave the start pose (ADR-275).
    stranded: bool,
    recovery_head: i32,
    next_plan: u32,
}
impl Motion {
    pub fn new(at: [i32; 3], kind: modding::range::RangeUnitKind, body: &UnitBody) -> Self {
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
        let navigation = modding::navigation::Navigation::from_ron(include_str!(
            "../../../data/range-navigation.ron"
        ))
        .expect("navigation");
        let yielding = if kind == modding::range::RangeUnitKind::Tank {
            navigation.fell.materials.to_vec()
        } else {
            Vec::new()
        };
        let rigs = crate::gait::rigs_of(body);
        let feet = vec![crate::gait::Foot::default(); rigs.len()];
        let legs_lost = vec![false; rigs.len()];
        let legs_buckled = vec![false; rigs.len()];
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
            cut: Vec::new(),
            floors: [Fx::ZERO; 4],
            feet,
            legs_lost,
            legs_buckled,
            rigs,
            navigation,
            yielding,
            blocked_ticks: 0,
            escape_left: 0,
            escape: 0,
            stranded: false,
            recovery_head: 0,
            next_plan: 0,
            drive_scale: drive::drive_factor(
                i64::from(chassis.mass) * 1000,
                (u128::from(body.volume_mm3()) * chassis.density as u128 / 1_000_000_000).max(1)
                    as i64,
            ),
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
    /// The legs of this hull, in template order; empty for a tracked one.
    pub fn rigs(&self) -> &[crate::gait::LegRig] {
        &self.rigs
    }
    /// Supports of a walker (ADR-280): the chassis' nominal point of every
    /// leg it still has, over the probed floor of that point. A lost leg
    /// carries nothing, and neither does a buckled one (ADR-281). A foot in the air keeps its leg's support: the
    /// gait stabilises the hull actively, and a passive spring that let go
    /// of the swinging leg rocked the hull to fourteen degrees on flat
    /// ground. The point of application stays nominal and the floor the
    /// probe's: a stance foot keeps its world point while the hull moves,
    /// and both the sole's own height and the foot's own position under a
    /// spring with four units of travel threw the hull up by whole units
    /// on crater and trench runs; the probe under the nominal point is
    /// what the planner accepted (ADR-272). `None` for a tracked hull and
    /// with fewer than two legs left.
    fn foot_supports(
        &self,
        nominal: &[phys::Support; 4],
        floors: &[Fx; 4],
    ) -> Option<(Vec<phys::Support>, Vec<Fx>)> {
        if self.rigs.is_empty() {
            return None;
        }
        let mut support = Vec::with_capacity(4);
        let mut ground = Vec::with_capacity(4);
        // Support order, not leg order: the suspension sums in index order,
        // and a whole walker must add up exactly as the nominal four do.
        for i in 0..4 {
            let carried = self
                .rigs
                .iter()
                .zip(self.legs_lost.iter().zip(&self.legs_buckled))
                .any(|(rig, (lost, buckled))| rig.support.min(3) == i && !*lost && !*buckled);
            if carried {
                support.push(nominal[i]);
                ground.push(floors[i]);
            }
        }
        (support.len() >= 2).then_some((support, ground))
    }
    /// A parked walker's hull rests on its legs (ADR-280): a lost leg lets
    /// it sink and lean where it stands. A tracked hull keeps the resting
    /// pose.
    fn settle(
        &mut self,
        w: &World,
        b: &UnitBody,
        gravity: i32,
        response: crate::mobility::Mobility,
    ) {
        if self.rigs.is_empty() {
            return;
        }
        let (hc, hs) = crate::ride::heading_of(self.drive.head);
        let mut ground =
            nav::vehicle::Probe::around(w, &self.yielding, [self.pos[0], self.pos[2]], 96);
        let shape = self.shape(b);
        let f = ground
            .pose([self.pos[0], self.pos[2]], self.drive.head, shape)
            .floors;
        let floors = [f[3], f[1], f[2], f[0]];
        self.floors = floors;
        let nominal = crate::ride::supports_of(&self.chassis);
        let Some((support, ground)) = self.foot_supports(&nominal, &floors) else {
            return;
        };
        let mut r = crate::ride::ride_of(&self.chassis);
        r.gravity = Fx::from_int(gravity);
        r.preload = r.gravity.div(Fx::from_int(4));
        let r = response.ride(r);
        phys::step(
            &mut self.hull,
            &support,
            &ground,
            (hc, hs),
            V3::ZERO,
            &r,
            Fx::from_ratio(1, 30),
        );
        self.pos[1] = self.hull.y.sub(r.clearance);
    }
    /// Advance a walker's feet one tick from the motion just accepted
    /// (ADR-279): the displacement, yaw step and phase advance of this
    /// tick, the terrain under each sole, and whether the hull carries
    /// load. A lost leg keeps its last state.
    fn step_feet(&mut self, w: &World, b: &UnitBody, linear: V3, angular: i32, cycles: Fx) {
        let Some(g) = self.chassis.gait.as_ref() else {
            return;
        };
        if self.rigs.is_empty() {
            return;
        }
        let gait = crate::gait::Gait::of(g);
        let root = self.frame(b);
        // A slow walk lifts less: the picture's rule, in per-second terms.
        let lift_scale = cycles
            .mul(Fx::from_int(crate::TICKS_PER_SECOND))
            .mul(gait.stride)
            .mul(Fx::from_int(4))
            .div(Fx::from_int(self.speed.max(1)))
            .min(Fx::ONE);
        let floor = |x: Fx, z: Fx| Fx::from_int(floor(w, x.to_int_trunc(), z.to_int_trunc()));
        let stance = crate::gait::Stance {
            grounded: self.hull.grounded(),
            root: &root,
            head: self.drive.head,
            phase: self.phase,
            lift_scale,
            linear,
            angular,
            cycles,
            gait: &gait,
            floor: &floor,
        };
        for (((rig, foot), lost), buckled) in self
            .rigs
            .iter()
            .zip(&mut self.feet)
            .zip(&self.legs_lost)
            .zip(&self.legs_buckled)
        {
            if *lost {
                continue;
            }
            if *buckled {
                foot.fold(rig, &root, self.drive.head);
            } else {
                foot.step(rig, &stance);
            }
        }
    }
    pub fn move_to(&mut self, w: &World, to: [i32; 2], b: &UnitBody, others: &[Obstacle]) -> bool {
        let from = [self.pos[0], self.pos[2]];
        let shape = self.shape(b);
        let half = shape.half.map(Fx::from_int);
        let plan = nav::vehicle::plan(
            nav::vehicle::Probe::with_yield(w, &self.yielding),
            shape,
            from,
            self.drive.head,
            to,
            nav::vehicle::Search {
                cell: self.navigation.cell_units,
                min: [40; 2],
                max: [600; 2],
                nodes: self.navigation.search_nodes,
                crush_cost: self.navigation.fell.cost_per_column,
                crush_cap: self.navigation.fell.cost_cap,
                margin: self.navigation.plan_margin,
            },
            |p, head| {
                !others.iter().any(|o| {
                    o.overlaps(&Obstacle {
                        pos: [p[0], Fx::ZERO, p[1]],
                        head,
                        half,
                    })
                })
            },
        );
        self.destination = to.iter().all(|v| (40..600).contains(v)).then_some(to);
        self.blocked = plan.path.is_empty();
        self.stranded = plan.stranded;
        self.path = plan.path;
        self.cursor = 0;
        self.enabled = true;
        !self.blocked
    }
    pub fn stop(&mut self) {
        self.path.clear();
        self.cursor = 0;
        self.destination = None;
        self.blocked = false;
        self.blocked_ticks = 0;
        self.escape_left = 0;
        self.escape = 0;
        self.stranded = false;
        self.next_plan = 0;
    }
    /// The footprint the range probes with, for diagnostics and tests.
    pub fn probe_shape(&self, b: &UnitBody) -> nav::vehicle::Shape {
        self.shape(b)
    }
    pub(crate) fn shape(&self, b: &UnitBody) -> nav::vehicle::Shape {
        let units = voxel::unit::WORLD_MM as i32;
        nav::vehicle::Shape {
            half: [
                b.vol.nx as i32 * b.mm as i32 / units / 2,
                b.vol.nz as i32 * b.mm as i32 / units / 2,
            ],
            support: [
                self.chassis.support_x_cm * 10 / units,
                self.chassis.support_z_cm * 10 / units,
            ],
            clearance: Fx::from_ratio(self.chassis.clearance_cm * 10, units),
            step: self.step,
            min_up: Fx::from_ratio(self.chassis.max_tilt_pct, 100),
            height: b.vol.ny as i32 * b.mm as i32 / units,
            span: self.chassis.contact_cm * 10 / units,
            ramp: Fx::from_ratio(self.chassis.approach_pct, 100),
        }
    }
    // Drive phase takes explicit body, world, clock and contact inputs.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        w: &World,
        b: &UnitBody,
        alive: bool,
        engine: bool,
        response: crate::mobility::Mobility,
        others: &[Obstacle],
        tick: u32,
        gravity: i32,
    ) {
        self.cut.clear();
        if !self.enabled {
            // A parked walker still stands on its feet, and its hull on them.
            self.step_feet(w, b, V3::ZERO, 0, Fx::ZERO);
            self.settle(w, b, gravity, response);
            return;
        }
        let engine = engine && response.traction > Fx::ZERO;
        if self.escape_left == 0 && (self.blocked || self.path.is_empty()) && tick >= self.next_plan
        {
            if let Some(goal) = self.destination {
                self.move_to(w, goal, b, others);
                self.next_plan = tick.saturating_add(self.replan);
            }
        }
        if self.cursor == self.path.len()
            && self.escape_left == 0
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
        if alive && engine && self.escape_left == 0 && self.cursor < self.path.len() {
            let goal = self.path[self.cursor];
            let dx = Fx::from_int(goal[0]).sub(self.pos[0]);
            let dz = Fx::from_int(goal[1]).sub(self.pos[2]);
            if dx.mul(dx).add(dz.mul(dz)) <= Fx::from_int(self.arrival * self.arrival) {
                self.cursor += 1;
                self.escape = 0;
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
                speed = Fx::from_int(self.speed)
                    .mul(response.engine)
                    .mul(response.traction)
                    .min(dx.abs().max(dz.abs()).mul(Fx::from_int(2)));
                let alignment = angle::cos(angle::diff(want, self.drive.head)).max(Fx::ZERO);
                speed = speed.mul(if alignment > Fx::from_ratio(self.alignment, 100) {
                    alignment
                } else {
                    Fx::ZERO
                });
            }
        }
        // One cycle of manoeuvres per waypoint: a hull that none of them
        // freed waits for the world to change rather than pacing forever.
        if alive
            && engine
            && self.blocked_ticks >= self.navigation.recovery_ticks
            && self.escape_left == 0
            && self.escape < ESCAPES
        {
            self.escape_left = self.navigation.escape_ticks;
            self.recovery_head = self.drive.head;
            self.blocked_ticks = 0;
            self.escape = self.escape.wrapping_add(1);
        }
        if self.escape_left > 0 && alive && engine {
            // Astern, ahead, then astern and ahead turning each way, the
            // turn from the first tick: a hull wedged on one side leaves by
            // the other, and one whose every turn is refused where it
            // stands turns while it moves.
            let attempt = self.escape.wrapping_sub(1) % ESCAPES;
            let astern = attempt.is_multiple_of(2);
            let sign = match attempt {
                0 | 1 => 0,
                2 | 5 => 1,
                _ => -1,
            };
            let turn = sign * self.navigation.recovery_turn_deg * angle::FULL / 360;
            want = angle::norm(self.recovery_head + turn);
            let pct = self.navigation.escape_speed_pct * if astern { -1 } else { 1 };
            speed = Fx::from_int(self.speed)
                .mul(response.engine)
                .mul(response.traction)
                .mul(Fx::from_ratio(pct, 100));
        }
        let lim = response.limits(&Limits::of(&self.chassis, 30), want == drive::NO_WAY);
        let old = self.drive;
        let old_pos = self.pos;
        // Two poses and the cut lie within a hull length of the vehicle; a
        // whole-map cache would cost more to clear than the poses to compute.
        let mut ground =
            nav::vehicle::Probe::around(w, &self.yielding, [self.pos[0], self.pos[2]], 96);
        let shape = self.shape(b);
        let current = ground.pose([self.pos[0], self.pos[2]], old.head, shape);
        // Pushing through a tree is slower than open ground.
        if current.crush > 0 {
            speed = speed.mul(Fx::from_ratio(self.navigation.fell.speed_pct, 100));
        }
        let (next, delta) = drive::advance(
            old,
            want,
            speed,
            &lim,
            self.drive_scale.mul(self.hull.load),
            Fx::from_ratio(1, 30),
        );
        let previous = Obstacle::of(self, b);
        self.drive = next;
        let nx = self.pos[0].add(delta.dx);
        let nz = self.pos[2].add(delta.dz);
        let (x, z) = (nx.to_int_trunc(), nz.to_int_trunc());
        let target = ground.pose([nx, nz], next.head, shape);
        let mut candidate = Obstacle::of(self, b);
        candidate.pos[0] = nx;
        candidate.pos[2] = nz;
        let collision = others.iter().any(|p| {
            let penetration = candidate.penetration(p);
            penetration > Fx::ZERO && penetration >= previous.penetration(p)
        });
        // A hull never enters a pose that leans it past its limit or
        // grounds more of its belly. The contact grade is a softer rule: a
        // rise the hull already stands in may be left through another such
        // pose, and an escape manoeuvre disregards it altogether, since it
        // exists to leave a spot where every planned move met exactly that
        // refusal. Belly, tilt, bounds and collisions still hold.
        let clear = (target.blocked == 0 || target.blocked < current.blocked)
            && !target.tilt
            && (self.escape_left > 0 || !target.steep || current.steep);
        let allowed = clear
            && x >= 40
            && z >= 40
            && x < 600
            && z < 600
            && target.height.sub(current.height) <= Fx::from_int(self.step)
            && !collision;
        let path = if allowed {
            self.blocked = false;
            self.pos[0] = nx;
            self.pos[2] = nz;
            next.vel.div(Fx::from_int(30))
        } else {
            self.drive.vel = Fx::ZERO;
            self.drive.head = old.head;
            self.drive.yaw_rate = 0;
            self.blocked = true;
            Fx::ZERO
        };
        if (allowed && target.crush > 0) || (!allowed && current.crush > 0) {
            self.cut = ground.crushed([self.pos[0], self.pos[2]], self.drive.head, shape);
        }
        // A search that could not leave this pose is a block like any
        // other: only a manoeuvre will change what the next search sees.
        if self.stranded && self.destination.is_some() && self.path.is_empty() {
            self.blocked = true;
        }
        if self.escape_left > 0 {
            // A refused manoeuvre is over: the next one begins at once
            // instead of counting down against the same wall.
            self.escape_left = if allowed { self.escape_left - 1 } else { 0 };
            if self.escape_left == 0 {
                self.path.clear();
                self.cursor = 0;
                self.next_plan = tick.saturating_add(1);
                if !allowed {
                    self.blocked_ticks = self.navigation.recovery_ticks;
                }
            }
        } else if self.blocked {
            self.blocked_ticks = self.blocked_ticks.saturating_add(1);
        } else {
            self.blocked_ticks = 0;
        }
        let mut advanced = 0;
        if let Some(c) = drive::StepCycle::of(&self.chassis) {
            advanced = c.advance(path, angle::diff(self.drive.head, old.head));
            self.phase = angle::norm(self.phase.wrapping_add(advanced));
        }
        let nominal = crate::ride::supports_of(&self.chassis);
        let (hc, hs) = crate::ride::heading_of(self.drive.head);
        let f = if allowed {
            target.floors
        } else {
            current.floors
        };
        let floors = [f[3], f[1], f[2], f[0]];
        self.floors = floors;
        // A walker stands on the legs it still has (ADR-280); a tracked
        // hull on all four nominal points over the probe's floors.
        let (support, ground) = self
            .foot_supports(&nominal, &floors)
            .unwrap_or_else(|| (nominal.to_vec(), floors.to_vec()));
        let mut r = crate::ride::ride_of(&self.chassis);
        r.gravity = Fx::from_int(gravity);
        r.preload = r.gravity.div(Fx::from_int(4));
        let r = response.ride(r);
        let sway = crate::ride::sway_of(
            self.drive.vel.sub(old.vel).mul(Fx::from_int(30)),
            self.drive.vel,
            angle::diff(self.drive.head, old.head),
            self.drive.head,
        );
        phys::step(
            &mut self.hull,
            &support,
            &ground,
            (hc, hs),
            sway,
            &r,
            Fx::from_ratio(1, 30),
        );
        self.pos[1] = self.hull.y.sub(r.clearance);
        let linear = V3::new(
            self.pos[0].sub(old_pos[0]),
            Fx::ZERO,
            self.pos[2].sub(old_pos[2]),
        );
        self.step_feet(
            w,
            b,
            linear,
            angle::diff(self.drive.head, old.head),
            Fx::from_ratio(advanced, angle::FULL),
        );
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
        b.u32(self.blocked_ticks);
        b.u32(self.escape_left);
        b.u32(self.escape);
        b.bool(self.stranded);
        b.i32(self.recovery_head);
        b.u32(self.next_plan);
        for list in [&self.path, &self.route] {
            b.u32(list.len() as u32);
            for p in list {
                b.i32(p[0]);
                b.i32(p[1]);
            }
        }
        b.u32(self.feet.len() as u32);
        for f in &self.feet {
            f.save(b);
        }
        for lost in &self.legs_lost {
            b.bool(*lost);
        }
        for buckled in &self.legs_buckled {
            b.bool(*buckled);
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
        self.blocked_ticks = s.u32();
        self.escape_left = s.u32();
        self.escape = s.u32();
        self.stranded = s.bool();
        self.recovery_head = s.i32();
        self.next_plan = s.u32();
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
        if s.u32() as usize != self.rigs.len() {
            return false;
        }
        for f in &mut self.feet {
            match crate::gait::Foot::load(s) {
                Some(v) => *f = v,
                None => return false,
            }
        }
        for lost in &mut self.legs_lost {
            *lost = s.bool();
        }
        for buckled in &mut self.legs_buckled {
            *buckled = s.bool();
        }
        s.ok()
            && self.escape_left <= self.navigation.escape_ticks
            && (0..angle::FULL).contains(&self.recovery_head)
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
        if w.solid(x as u32, y, z as u32) {
            return y as i32 + 1;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use modding::range::RangeUnitKind;
    struct Ground {
        depth: i32,
    }
    impl voxel::Generator for Ground {
        fn material(&self, x: u32, y: u32, _z: u32) -> u16 {
            let height = 16 - (self.depth - (x as i32 - 320).abs() / 3).max(0);
            if y < height as u32 {
                1
            } else {
                0
            }
        }
    }
    fn ground(depth: i32) -> World {
        World::new(640, 64, 640, 100, Box::new(Ground { depth }))
    }
    fn step(m: &mut Motion, w: &World, b: &UnitBody, others: &[Obstacle], tick: u32) {
        m.step(
            w,
            b,
            true,
            true,
            crate::mobility::Mobility::intact(true),
            others,
            tick,
            98,
        );
    }
    /// A whole walker carries its hull on all four legs, feet in the air or
    /// not (ADR-280): the gait stabilises the hull, and the run stays level.
    #[test]
    fn a_whole_walker_carries_its_hull_on_all_four_legs() {
        let w = ground(0);
        let body = voxel::unit::strider();
        let mut m = Motion::new([200, 16, 300], RangeUnitKind::Strider, &body);
        let nominal = crate::ride::supports_of(&m.chassis);
        assert_eq!(m.feet.len(), 4);
        m.enabled = true;
        m.destination = Some([430, 300]);
        m.path = vec![[430, 300]];
        let mut lifted = 0;
        for tick in 0..300 {
            step(&mut m, &w, &body, &[], tick);
            let (support, ground) = m
                .foot_supports(&nominal, &m.floors)
                .expect("a walker has legs");
            assert_eq!(support, nominal.to_vec());
            assert_eq!(ground, m.floors.to_vec());
            lifted += usize::from(m.feet.iter().any(|f| !f.planted()));
            assert_eq!(m.hull.load, Fx::ONE);
            assert!(
                m.hull.up.y > Fx::from_ratio(99, 100),
                "tilted at tick {tick}: {:?}",
                m.hull.up
            );
        }
        assert!(lifted > 200, "feet in the air on {lifted} ticks");
        assert!(
            m.pos[0] > Fx::from_int(300),
            "the walker did not walk: {:?}",
            m.pos
        );
    }
    /// A walker that lost a leg carries its hull on the other three
    /// (ADR-280): it sinks, leans over the missing corner, and still walks.
    #[test]
    fn a_walker_that_lost_a_leg_sinks_leans_and_walks_on_three() {
        let w = ground(0);
        let body = voxel::unit::strider();
        let mut m = Motion::new([200, 16, 300], RangeUnitKind::Strider, &body);
        let nominal = crate::ride::supports_of(&m.chassis);
        m.legs_lost[0] = true;
        m.enabled = true;
        m.destination = Some([430, 300]);
        m.path = vec![[430, 300]];
        let gone = nominal[usize::from(body.legs[0].support)];
        for tick in 0..300 {
            step(&mut m, &w, &body, &[], tick);
            let (support, _) = m.foot_supports(&nominal, &m.floors).unwrap();
            assert_eq!(support.len(), 3);
            assert!(!support.contains(&gone));
            // The lean is bounded by the chassis' tilt limit, nothing else:
            // the missing corner has no leg and no belly near the ground.
            assert!(
                m.hull.up.y >= Fx::from_ratio(84, 100),
                "fell over at tick {tick}: {:?}",
                m.hull.up
            );
        }
        // Leg 0 hangs at the `-x, -z` corner; heading `+x`, the hull sinks
        // and its normal leans towards the sunk corner.
        assert_eq!(m.drive.head, 0);
        assert!(
            m.pos[1] < Fx::from_int(16).sub(Fx::from_ratio(1, 10)),
            "did not sink: {:?}",
            m.pos
        );
        assert!(
            m.hull.up.x < Fx::ZERO && m.hull.up.z < Fx::ZERO,
            "did not lean over the lost leg: {:?}",
            m.hull.up
        );
        assert!(m.pos[0] > Fx::from_int(300), "did not walk: {:?}", m.pos);
    }
    /// A buckled leg carries nothing and folds under its hip (ADR-281): the
    /// walker sinks and leans over it and still walks.
    #[test]
    fn a_buckled_leg_carries_nothing_and_folds_under_the_hip() {
        let w = ground(0);
        let body = voxel::unit::strider();
        let mut m = Motion::new([200, 16, 300], RangeUnitKind::Strider, &body);
        let nominal = crate::ride::supports_of(&m.chassis);
        m.legs_buckled[1] = true;
        m.enabled = true;
        m.destination = Some([430, 300]);
        m.path = vec![[430, 300]];
        let gone = nominal[usize::from(body.legs[1].support)];
        for tick in 0..300 {
            step(&mut m, &w, &body, &[], tick);
            let (support, _) = m.foot_supports(&nominal, &m.floors).unwrap();
            assert_eq!(support.len(), 3);
            assert!(!support.contains(&gone));
            let c = m.feet[1].contact.expect("the folded leg has a contact");
            let hip = m.frame(&body).world([
                Fx::from_int(i32::from(body.legs[1].hip.0)),
                Fx::from_int(i32::from(body.legs[1].hip.1)),
                Fx::from_int(i32::from(body.legs[1].hip.2)),
            ]);
            assert!(
                hip.y.sub(c.pos.y) < Fx::from_int(4),
                "not folded at tick {tick}: {c:?}"
            );
            assert!(!m.feet[1].planted());
        }
        assert!(
            m.pos[1] < Fx::from_int(16).sub(Fx::from_ratio(1, 10)),
            "did not sink"
        );
        assert!(m.pos[0] > Fx::from_int(300), "did not walk: {:?}", m.pos);
    }
    #[test]
    fn opposing_tanks_recover_and_do_not_reenter_each_other() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let mut a = Motion::new([200, 16, 300], RangeUnitKind::Tank, &body);
        let mut b = Motion::new([250, 16, 300], RangeUnitKind::Tank, &body);
        a.enabled = true;
        a.destination = Some([430, 300]);
        a.path = vec![[430, 300]];
        b.enabled = true;
        b.drive.head = angle::FULL / 2;
        b.destination = Some([70, 300]);
        b.path = vec![[70, 300]];
        let mut old = Obstacle::of(&a, &body).penetration(&Obstacle::of(&b, &body));
        let mut reversed = false;
        for tick in 0..1600 {
            step(&mut a, &w, &body, &[Obstacle::of(&b, &body)], tick);
            step(&mut b, &w, &body, &[Obstacle::of(&a, &body)], tick);
            reversed |= a.drive.vel < Fx::ZERO || b.drive.vel < Fx::ZERO;
            let overlap = Obstacle::of(&a, &body).penetration(&Obstacle::of(&b, &body));
            assert!(overlap <= old, "penetration increased at {tick}");
            old = overlap;
            if a.destination.is_none() && b.destination.is_none() {
                break;
            }
        }
        assert!(reversed, "recovery never reversed");
        assert_eq!(old, Fx::ZERO);
        assert!(
            a.destination.is_none() && b.destination.is_none(),
            "a {:?} path {:?}; b {:?} path {:?}",
            a.pos,
            a.path,
            b.pos,
            b.path
        );
    }
    #[test]
    fn recovery_resumes_from_a_save_without_a_velocity_flip() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let mut a = Motion::new([200, 16, 300], RangeUnitKind::Tank, &body);
        a.enabled = true;
        a.destination = Some([430, 300]);
        a.path = vec![[430, 300]];
        let obstacle = Obstacle {
            pos: [Fx::from_int(250), Fx::from_int(16), Fx::from_int(300)],
            head: 0,
            half: [Fx::from_int(30), Fx::from_int(15)],
        };
        let mut tick = 0;
        while a.escape_left == 0 && tick < 100 {
            step(&mut a, &w, &body, &[obstacle], tick);
            tick += 1;
        }
        assert!(a.escape_left > 0);
        let mut blob = Blob::new();
        a.save(&mut blob);
        let mut b = Motion::new([200, 16, 300], RangeUnitKind::Tank, &body);
        assert!(b.load(&mut Scan::new(&blob.v)));
        for tick in tick..tick + 180 {
            step(&mut a, &w, &body, &[obstacle], tick);
            step(&mut b, &w, &body, &[obstacle], tick);
            let mut x = Blob::new();
            let mut y = Blob::new();
            a.save(&mut x);
            b.save(&mut y);
            assert_eq!(x.v, y.v);
        }
    }
    /// Flat ground with a post either side of the stern of a hull parked
    /// at (200, 300) facing +x: the posts stand at x = 160..176 beyond the
    /// hull's own width.
    struct Posts;
    impl voxel::Generator for Posts {
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            let post = (160..=176).contains(&x) && !(284..=316).contains(&z) && y < 46;
            u16::from(y < 16 || post)
        }
    }
    #[test]
    fn a_hull_refused_astern_escapes_ahead() {
        let w = World::new(640, 64, 640, 100, Box::new(Posts));
        let body = voxel::unit::tank();
        let mut a = Motion::new([200, 16, 300], RangeUnitKind::Tank, &body);
        // Between the posts with another hull one unit astern and a
        // waypoint straight behind: any turn grounds a stern corner on a
        // post, backing off meets the neighbour within a few ticks. The
        // escape astern is abandoned the tick it is refused and the next
        // goes ahead, clear of both. No destination, so no search
        // intervenes: this is the manoeuvre cycle alone.
        let neighbour = Obstacle {
            pos: [Fx::from_int(139), Fx::from_int(16), Fx::from_int(300)],
            head: 0,
            half: [Fx::from_int(30), Fx::from_int(15)],
        };
        a.enabled = true;
        a.path = vec![[50, 300]];
        let mut ahead = false;
        let mut astern = false;
        let mut attempts = 0;
        for tick in 0..300 {
            step(&mut a, &w, &body, &[neighbour], tick);
            if a.escape_left > 0 {
                ahead |= a.drive.vel > Fx::ZERO;
                astern |= a.drive.vel < Fx::ZERO;
            }
            attempts = attempts.max(a.escape);
            assert_eq!(
                Obstacle::of(&a, &body).penetration(&neighbour),
                Fx::ZERO,
                "entered the neighbour at {tick}"
            );
            if attempts >= 2 && a.escape_left == 0 {
                break;
            }
        }
        assert!(
            astern && ahead,
            "escapes: {attempts}, astern {astern}, ahead {ahead}"
        );
        assert_eq!(attempts, 2);
        assert!(
            a.pos[0] > Fx::from_int(210),
            "never drove out ahead: {:?}",
            a.pos
        );
        assert!(a.path.is_empty(), "the refused plan survived the escape");
    }
    #[test]
    fn tanks_and_walkers_cross_several_craters_in_both_directions() {
        for depth in [4, 8, 12] {
            for reverse in [false, true] {
                for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
                    let w = ground(depth);
                    let body = match kind {
                        RangeUnitKind::Tank => voxel::unit::tank(),
                        RangeUnitKind::Strider => voxel::unit::strider(),
                    };
                    let (start, end) = if reverse { (410, 230) } else { (230, 410) };
                    let mut m = Motion::new([start, 16, 300], kind, &body);
                    m.drive.head = if reverse { angle::FULL / 2 } else { 0 };
                    assert!(
                        m.move_to(&w, [end, 300], &body, &[]),
                        "no path {depth} {kind:?}"
                    );
                    let mut low = Fx::from_int(16);
                    let mut old = m.pos[1];
                    for tick in 0..750 {
                        step(&mut m, &w, &body, &[], tick);
                        assert!(
                            m.pos[1].sub(old).abs() < Fx::ONE,
                            "height jump {depth} {kind:?}"
                        );
                        old = m.pos[1];
                        low = low.min(old);
                        if m.destination.is_none() {
                            break;
                        }
                    }
                    assert!(
                        m.destination.is_none(),
                        "stuck {kind:?} depth {depth} at {:?}",
                        m.pos
                    );
                    if depth == 12 {
                        assert!(low < Fx::from_int(15), "never entered crater");
                    }
                }
            }
        }
    }
}
