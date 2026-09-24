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
/// Farthest a burst pushes a hull, world units from its centre (ADR-286).
const IMPULSE_REACH: i32 = 64;
/// How far from the edge of the world an order may still be given, world
/// units (ADR-315). A hull needs room to turn at the far end of a route,
/// and the planner's goal ring reaches four cells.
const EDGE: i32 = 40;
/// The widest world these checks accept when the world itself is not at
/// hand: they guard a restored snapshot against nonsense, and the exact
/// bound is the world's own, where the world is known.
const MAX_SIDE: i32 = 16384;
#[derive(Clone, Debug)]
pub struct Motion {
    pub pos: [Fx; 3],
    pub drive: Drive,
    pub hull: Chassis,
    pub phase: i32,
    pub path: Vec<[i32; 2]>,
    pub cursor: usize,
    pub destination: Option<[i32; 2]>,
    /// The current route ends at a checked intermediate point, not the order.
    pub partial_path: bool,
    pub route: Vec<[i32; 2]>,
    pub waypoint: usize,
    pub laps: u32,
    pub enabled: bool,
    pub blocked: bool,
    pub chassis: modding::tuning::Chassis,
    pub speed: i32,
    pub step: i32,
    /// Terrain acceptance, distinct from the damaged hull's roll limit.
    terrain_tilt_pct: i32,
    terrain_approach_pct: i32,
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
    /// Sideways velocity a burst gave the hull, world units per second in
    /// the horizontal plane (ADR-286); the brakes and the terrain spend it.
    /// Saved.
    pub shove: V3,
    /// Contact generated during this motion tick: index in `others` and
    /// incoming velocity along the line to that hull. Consumed by the
    /// range after all actors have committed their positions, never saved.
    contact: Option<(usize, V3)>,
    /// Legs computed once from the body.
    rigs: Vec<crate::gait::LegRig>,
    /// Share of a buckled leg's rest reach its foot keeps, thousandths
    /// (ADR-288), from the integrity data.
    fold_milli: u32,
    /// Clearance of a walker with fewer than two legs carrying, per cent
    /// of the ride's (ADR-304), from the integrity data.
    collapse_pct: i32,
    // Installed engine sizing is based on the pristine machine. Losing armour
    // must not silently resize the motor each movement tick.
    drive_scale: Fx,
    navigation: modding::navigation::Navigation,
    /// Nodes expanded and poses probed by the last plan: diagnostics, not
    /// state.
    pub last_plan: [u32; 2],
    /// Columns scanned and surface samples taken by it (P-29).
    pub last_probe: [u64; 2],
    /// A route search still walking its frontier (ADR-306). While one is
    /// in flight the hull stands and the orders wait for it.
    hunt: Option<Box<nav::vehicle::Hunt>>,
    /// The last search ran its open set dry after expanding nodes. Read in
    /// the same tick it is written, never across one: not state.
    dry: bool,
    /// Yielding vegetation this hull fells instead of treating as a wall.
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
    recovery_origin: [Fx; 2],
    next_plan: u32,
    /// The range hands out permission to begin a search each tick
    /// (ADR-312); `started` reports back whether this hull used it. Both
    /// are scratch for the tick, not state: the range sets them before
    /// every step, and a hull stepped by anything else may always begin.
    may_start: bool,
    started: bool,
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
        let yielding = navigation.fell.materials.to_vec();
        let rigs = crate::gait::rigs_of(body);
        let feet = vec![crate::gait::Foot::default(); rigs.len()];
        let integrity = modding::integrity::Integrity::from_ron(include_str!(
            "../../../data/unit-integrity.ron"
        ))
        .expect("unit integrity tuning");
        let fold_milli = integrity.leg_fold_pct.clamp(0, 100) as u32 * 10;
        let collapse_pct = integrity.collapse_clearance_pct.clamp(0, 100);
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
            partial_path: false,
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
            step: if kind == modding::range::RangeUnitKind::Tank {
                c.step_height
            } else {
                c.walker_step_height
            },
            terrain_tilt_pct: if kind == modding::range::RangeUnitKind::Tank {
                c.tank_terrain_tilt_pct
            } else {
                c.walker_terrain_tilt_pct
            },
            terrain_approach_pct: if kind == modding::range::RangeUnitKind::Tank {
                c.tank_terrain_approach_pct
            } else {
                c.walker_terrain_approach_pct
            },
            arrival: c.arrival,
            alignment: c.turn_alignment_pct,
            replan: c.replan_ticks,
            cut: Vec::new(),
            floors: [Fx::ZERO; 4],
            feet,
            legs_lost,
            legs_buckled,
            shove: V3::ZERO,
            contact: None,
            rigs,
            fold_milli,
            collapse_pct,
            navigation,
            yielding,
            blocked_ticks: 0,
            escape_left: 0,
            escape: 0,
            stranded: false,
            recovery_head: 0,
            recovery_origin: [Fx::from_int(at[0]), Fx::from_int(at[2])],
            next_plan: 0,
            may_start: true,
            started: false,
            last_plan: [0; 2],
            last_probe: [0; 2],
            hunt: None,
            dry: false,
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
    /// A burst's push on this hull (ADR-286, ADR-297, ADR-302): `newton_s`
    /// newton-seconds of the wave's pressure the body absorbed, landing on
    /// the point of the hull's box nearest the burst, at the burst's height
    /// within the hull's — the face the wave met. A burst inside the box,
    /// or just below its footprint, pushes through the centre of mass.
    /// Returns the point.
    pub fn impulse(&mut self, b: &UnitBody, at: [i32; 3], newton_s: u64) -> [i32; 3] {
        let shape = self.shape(b);
        let (c, s) = (
            fx::angle::cos(self.drive.head),
            fx::angle::sin(self.drive.head),
        );
        let burst = [
            Fx::from_int(at[0]),
            Fx::from_int(at[1]),
            Fx::from_int(at[2]),
        ];
        let (dx, dz) = (burst[0].sub(self.pos[0]), burst[2].sub(self.pos[2]));
        let half = shape.half.map(Fx::from_int);
        let local_x = dx.mul(c).add(dz.mul(s));
        let local_z = dz.mul(c).sub(dx.mul(s));
        let u = local_x.max(half[0].neg()).min(half[0]);
        let v = local_z.max(half[1].neg()).min(half[1]);
        let bottom = self.pos[1];
        let top = bottom.add(Fx::from_int(shape.height));
        let mut through = [
            self.pos[0].add(u.mul(c)).sub(v.mul(s)),
            burst[1].max(bottom).min(top),
            self.pos[2].add(u.mul(s)).add(v.mul(c)),
        ]
        .map(Fx::to_int_trunc);
        // A ground burst can be below the belly yet inside the hull's
        // horizontal footprint. The closest point is then straight above
        // the burst: treating its whole pressure as vertical launches an
        // off-centre tank in place. The wave loads more of the underside
        // toward the centre of mass, giving that push a horizontal part.
        if through == at
            || (burst[1] < bottom && local_x.abs() < half[0] && local_z.abs() < half[1])
        {
            let r = crate::ride::ride_of(&self.chassis);
            through =
                [self.pos[0], self.pos[1].add(r.clearance), self.pos[2]].map(Fx::to_int_trunc);
        }
        self.impulse_through(b, at, through, newton_s);
        through
    }
    /// The push of `newton_s` newton-seconds landing at `through`, the
    /// point the wave's pressure on this body centres on (ADR-302). It acts
    /// along the line from the burst to that point — the wave's own
    /// direction where it met the body — so a blast beside a hull shoves
    /// it sideways, one under its belly lifts it, and a tall body is not
    /// thrown up merely for carrying its mass high; its moment about the
    /// centre of mass rolls or pitches the hull through the ride's radius
    /// of gyration. The range derives the newton-seconds from the pressure
    /// the body absorbed, so the falling off with distance and the shadow
    /// of cover are the wave's. The mass is the chassis' own, not the
    /// volume left: a hull that lost its plates is not thrown twenty times
    /// as far. The horizontal part becomes a shove the brakes and the
    /// terrain spend, the vertical part a hop through the suspension.
    /// Bursts farther than the reach do nothing.
    pub fn impulse_through(
        &mut self,
        b: &UnitBody,
        at: [i32; 3],
        through: [i32; 3],
        newton_s: u64,
    ) {
        if newton_s == 0 {
            return;
        }
        let r = crate::ride::ride_of(&self.chassis);
        let centre = V3::new(self.pos[0], self.pos[1].add(r.clearance), self.pos[2]);
        let burst = V3::new(
            Fx::from_int(at[0]),
            Fx::from_int(at[1]),
            Fx::from_int(at[2]),
        );
        let reach = Fx::from_int(IMPULSE_REACH);
        if centre.sub(burst).len_sq() > reach.mul(reach) {
            return;
        }
        let hit = V3::new(
            Fx::from_int(through[0]),
            Fx::from_int(through[1]),
            Fx::from_int(through[2]),
        );
        let d = hit.sub(burst);
        let dist2 = d.len_sq();
        if dist2 == Fx::ZERO {
            return;
        }
        let dist = dist2.sqrt().max(Fx::ONE);
        let dir = d.scale(Fx::ONE.div(dist));
        let newton_s = newton_s.min(i32::MAX as u64);
        let mass_kg = if self.chassis.mass > 0 {
            self.chassis.mass.saturating_mul(1000)
        } else {
            (u128::from(b.volume_mm3()) * self.chassis.density.max(1) as u128 / 1_000_000_000)
                .clamp(1, i32::MAX as u128) as i32
        };
        let units_per_metre = (1000 / i32::from(voxel::unit::WORLD_MM)).max(1);
        // No single push throws a hull faster than the chassis allows: a
        // bolt in the body presses on every cell at once, and the sum has
        // no ceiling of its own (ADR-304).
        let dv = Fx::from_int(newton_s as i32)
            .div(Fx::from_int(mass_kg))
            .mul(Fx::from_int(units_per_metre))
            .min(Fx::from_int(
                self.chassis.push_max_mps.max(1) * units_per_metre,
            ));
        let push = dir.scale(dv);
        self.shove = self.shove.add(V3::new(push.x, Fx::ZERO, push.z));
        self.hull.vy = self.hull.vy.add(push.y);
        // The push lands where the pressure did: its moment about the
        // centre of mass rolls or pitches the hull, no faster than the
        // chassis allows.
        let torque = hit.sub(centre).cross(push);
        let k2 = r.inertia.max(Fx::ONE);
        let w = self
            .hull
            .w
            .add(V3::new(torque.x.div(k2), Fx::ZERO, torque.z.div(k2)));
        let spin = w.x.mul(w.x).add(w.z.mul(w.z)).sqrt();
        let spin_max = Fx::from_int(self.chassis.spin_max_deg_s.max(1))
            .mul(Fx::from_ratio(314159, 100000))
            .div(Fx::from_int(180));
        self.hull.w = if spin > spin_max {
            w.scale(spin_max.div(spin))
        } else {
            w
        };
    }
    /// Spend the shove against the brakes for one tick; the terrain spends
    /// the rest when it refuses the slide.
    fn brake_shove(&mut self, lim: &Limits) {
        let len2 = self.shove.len_sq();
        if len2 == Fx::ZERO {
            return;
        }
        let len = len2.sqrt();
        let left = len.sub(lim.brake).max(Fx::ZERO);
        self.shove = if left == Fx::ZERO {
            V3::ZERO
        } else {
            self.shove.scale(left.div(len))
        };
    }
    /// Separate a refused shove into its contact-normal and sliding parts.
    /// A driven hull also offers the speed it had at the refused pose: its
    /// drive is stopped by the collision, but the other hull receives part
    /// of that normal speed in the range's stable second phase.
    fn record_contact(&mut self, index: usize, other: &Obstacle, attempted: V3) {
        let line = V3::new(
            other.pos[0].sub(self.pos[0]),
            Fx::ZERO,
            other.pos[2].sub(self.pos[2]),
        );
        let distance2 = line.len_sq();
        if distance2 == Fx::ZERO {
            self.shove = V3::ZERO;
            return;
        }
        let toward = attempted.dot(line);
        if toward <= Fx::ZERO {
            self.shove = V3::ZERO;
            return;
        }
        let normal = line.scale(toward.div(distance2));
        let shove_toward = self.shove.dot(line).max(Fx::ZERO);
        self.shove = self.shove.sub(line.scale(shove_toward.div(distance2)));
        self.contact = Some((index, normal));
    }
    pub(super) fn take_contact(&mut self) -> Option<(usize, V3)> {
        self.contact.take()
    }
    pub(super) fn receive_contact(&mut self, velocity: V3) {
        self.shove = self.shove.add(velocity);
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
        // A body with fewer legs than nominal points stands on long feet:
        // each leg carries heel and toe, every point on its own side
        // (ADR-288), so a two-legged body stands whole and falls onto a
        // buckled leg.
        let long_feet = self.rigs.len() < 4;
        // Support order, not leg order: the suspension sums in index order,
        // and a whole walker must add up exactly as the nominal four do.
        for i in 0..4 {
            let carried = self
                .rigs
                .iter()
                .zip(self.legs_lost.iter().zip(&self.legs_buckled))
                .any(|(rig, (lost, buckled))| {
                    let s = rig.support.min(3);
                    (s == i || (long_feet && s % 2 == i % 2)) && !*lost && !*buckled
                });
            if carried {
                support.push(nominal[i]);
                ground.push(floors[i]);
            }
        }
        (support.len() >= 2).then_some((support, ground))
    }
    /// A parked hull's slide under a burst's shove (ADR-286): the same
    /// acceptance as a driven move. Terrain spends a refused shove; a hull
    /// contact passes part of it to that hull (ADR-359). Returns the accepted
    /// displacement.
    fn slide(
        &mut self,
        w: &World,
        rubble: &nav::vehicle::Rubble,
        b: &UnitBody,
        others: &[Obstacle],
        response: crate::mobility::Mobility,
    ) -> V3 {
        if self.shove.len_sq() == Fx::ZERO {
            return V3::ZERO;
        }
        let lim = response.limits(&Limits::of(&self.chassis, 30), true);
        let mut ground =
            nav::vehicle::Probe::around(w, &self.yielding, [self.pos[0], self.pos[2]], 96)
                .with_rubble(rubble);
        let shape = self.shape(b);
        let current = ground.pose([self.pos[0], self.pos[2]], self.drive.head, shape);
        let nx = self.pos[0].add(self.shove.x.div(Fx::from_int(30)));
        let nz = self.pos[2].add(self.shove.z.div(Fx::from_int(30)));
        let target = ground.pose([nx, nz], self.drive.head, shape);
        let previous = Obstacle::of(self, b);
        let mut candidate = previous;
        candidate.pos[0] = nx;
        candidate.pos[2] = nz;
        let collision = others.iter().position(|p| {
            let penetration = candidate.penetration(p);
            penetration > Fx::ZERO && penetration >= previous.penetration(p)
        });
        let (x, z) = (nx.to_int_trunc(), nz.to_int_trunc());
        let terrain_allowed = (target.blocked == 0 || target.blocked < current.blocked)
            && !target.tilt
            && x >= EDGE
            && z >= EDGE
            && x < w.nx as i32 - EDGE
            && z < w.nz as i32 - EDGE
            && target.height.sub(current.height) <= Fx::from_int(self.step);
        let allowed = terrain_allowed && collision.is_none();
        let moved = if allowed {
            let d = V3::new(nx.sub(self.pos[0]), Fx::ZERO, nz.sub(self.pos[2]));
            self.pos[0] = nx;
            self.pos[2] = nz;
            self.floors = {
                let f = target.floors;
                [f[3], f[1], f[2], f[0]]
            };
            d
        } else {
            if terrain_allowed {
                if let Some(index) = collision {
                    self.record_contact(index, &others[index], self.shove);
                }
            } else {
                self.shove = V3::ZERO;
            }
            V3::ZERO
        };
        self.brake_shove(&lim);
        moved
    }
    /// Whether the legs carry the hull: a tracked hull always, a walker
    /// with at least two legs neither lost nor buckled (ADR-280). A walker
    /// with fewer sits at the collapse clearance on its folded legs
    /// (ADR-304): dead, or with every leg gone or buckled.
    fn carried(&self) -> bool {
        self.rigs.is_empty()
            || self
                .legs_lost
                .iter()
                .zip(&self.legs_buckled)
                .filter(|(lost, buckled)| !**lost && !**buckled)
                .count()
                >= 2
    }
    /// The ride of this hull now: the chassis' ride at the range's gravity
    /// and the body's own mobility (ADR-305), low on its legs when none
    /// carries them (ADR-304).
    fn ride_now(
        &self,
        b: &UnitBody,
        gravity: i32,
        response: crate::mobility::Mobility,
    ) -> phys::Ride {
        let mut r = crate::ride::ride_of(&self.chassis);
        r.gravity = Fx::from_int(gravity);
        r.preload = r.gravity.div(Fx::from_int(4));
        if !self.carried() {
            // The surviving root lands on its belly. Empty space where
            // the legs used to be is not a contact surface. Keep the
            // original body origin / suspension anchor relationship.
            let low = (0..b.vol.ny as usize)
                .find(|&y| {
                    (0..b.vol.nz as usize).any(|z| {
                        let start = (z * b.vol.ny as usize + y) * b.vol.words as usize;
                        b.vol.cols[start..start + b.vol.words as usize]
                            .iter()
                            .any(|v| *v != 0)
                    })
                })
                .unwrap_or(0);
            let belly = Fx::from_ratio(low as i32 * b.mm as i32, voxel::unit::WORLD_MM as i32);
            let gap = r.clearance.mul(Fx::from_ratio(self.collapse_pct, 100));
            r.clearance = r.clearance.sub(belly).add(gap);
        }
        response.ride(r)
    }
    /// Passive physics remains active without a drive order. Zero velocity
    /// does not prove support: terrain may disappear or a hop reach its apex.
    fn settle(
        &mut self,
        w: &World,
        rubble: &nav::vehicle::Rubble,
        b: &UnitBody,
        gravity: i32,
        response: crate::mobility::Mobility,
    ) {
        let (hc, hs) = crate::ride::heading_of(self.drive.head);
        let mut ground =
            nav::vehicle::Probe::around(w, &self.yielding, [self.pos[0], self.pos[2]], 96)
                .with_rubble(rubble);
        let shape = self.shape(b);
        let f = ground.support_floors([self.pos[0], self.pos[2]], self.drive.head, shape);
        let floors = [f[3], f[1], f[2], f[0]];
        self.floors = floors;
        let nominal = crate::ride::supports_of(&self.chassis);
        let (support, ground) = self
            .foot_supports(&nominal, &floors)
            .unwrap_or_else(|| (nominal.to_vec(), floors.to_vec()));
        let r = self.ride_now(b, gravity, response);
        phys::step(
            &mut self.hull,
            &support,
            &ground,
            (hc, hs),
            V3::ZERO,
            &r,
            Fx::from_ratio(1, 30),
        );
        self.pos[1] = self
            .hull
            .y
            .sub(crate::ride::ride_of(&self.chassis).clearance);
    }
    pub(super) fn passive(
        &mut self,
        w: &World,
        rubble: &nav::vehicle::Rubble,
        b: &UnitBody,
        gravity: i32,
        response: crate::mobility::Mobility,
        others: &[Obstacle],
    ) {
        self.cut.clear();
        self.contact = None;
        let moved = self.slide(w, rubble, b, others, response);
        self.settle(w, rubble, b, gravity, response);
        self.step_feet(w, rubble, b, moved, 0, Fx::ZERO);
    }
    /// Advance a walker's feet one tick from the motion just accepted
    /// (ADR-279): the displacement, yaw step and phase advance of this
    /// tick, the terrain under each sole, and whether the hull carries
    /// load. A lost leg keeps its last state.
    fn step_feet(
        &mut self,
        w: &World,
        rubble: &nav::vehicle::Rubble,
        b: &UnitBody,
        linear: V3,
        angular: i32,
        cycles: Fx,
    ) {
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
        // A resting heap of debris is floor for a foot too (ADR-292).
        let floor = |x: Fx, z: Fx| {
            let (x, z) = (x.to_int_trunc(), z.to_int_trunc());
            let top = rubble.get(&[x, z]).map_or(0, |t| i32::from(*t));
            Fx::from_int(floor(w, x, z).max(top))
        };
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
                foot.fold(rig, &root, self.drive.head, self.fold_milli, &floor);
            } else {
                foot.step(rig, &stance);
            }
        }
    }
    /// Ask for a route to `to` and walk one tick's worth of the search
    /// (ADR-306). `false` once the answer is known to be no route; while
    /// the search goes on the order stands and this is `true`.
    pub fn move_to(
        &mut self,
        w: &World,
        to: [i32; 2],
        b: &UnitBody,
        others: &[Obstacle],
        rubble: &nav::vehicle::Rubble,
    ) -> bool {
        let from = [self.pos[0], self.pos[2]];
        let shape = self.shape(b);
        let half = shape.half.map(Fx::from_int);
        let clear = |p: [Fx; 2], head: i32| {
            !others.iter().any(|o| {
                o.overlaps(&Obstacle {
                    pos: [p[0], Fx::ZERO, p[1]],
                    head,
                    half,
                })
            })
        };
        let far = [w.nx as i32 - EDGE, w.nz as i32 - EDGE];
        // A fresh destination supersedes the recovery cycle of the old
        // order. Replanning the same destination preserves its attempt count.
        if self.destination != Some(to) {
            self.escape = 0;
            self.blocked_ticks = 0;
        }
        self.escape_left = 0;
        self.destination = (0..2)
            .all(|a| (EDGE..far[a]).contains(&to[a]))
            .then_some(to);
        self.enabled = true;
        self.hunt = None;
        self.path.clear();
        self.partial_path = false;
        self.cursor = 0;
        let mut ground = nav::vehicle::Probe::for_search(w, &self.yielding, Default::default())
            .with_rubble(rubble);
        match nav::vehicle::Hunt::start_bounded(
            &mut ground,
            shape,
            from,
            self.drive.head,
            to,
            self.search(w),
            &clear,
            self.navigation.search_nodes_per_tick.max(1),
        ) {
            nav::vehicle::Hunting::Done(plan) => self.take_plan(plan),
            nav::vehicle::Hunting::Open(hunt) => {
                // Starting consumed this tick's bounded share. Goal probes,
                // the direct sweep and frontier resume on the next tick.
                self.hunt = Some(hunt);
                self.blocked = false;
                self.stranded = false;
                self.dry = false;
            }
        }
        !self.blocked
    }
    /// The planner's budget and rules for this hull. Its bounds are the
    /// world's, less a margin at the edge (ADR-315).
    fn search(&self, w: &World) -> nav::vehicle::Search {
        nav::vehicle::Search {
            cell: self.navigation.cell_units,
            min: [EDGE; 2],
            max: [w.nx as i32 - EDGE, w.nz as i32 - EDGE],
            nodes: self.navigation.search_nodes,
            crush_cost: self.navigation.fell.cost_per_column,
            crush_cap: self.navigation.fell.cost_cap,
            margin: self.navigation.plan_margin,
            heuristic_pct: self.navigation.heuristic_pct,
        }
    }
    /// A search's answer becomes the hull's route.
    fn take_plan(&mut self, plan: nav::vehicle::Route) {
        // A search that ran the open set dry found the goal cut off in the
        // world as it is; the goal stays, since the world changes — a hull
        // moves, a tree falls, a crater fills — but the next try waits four
        // replan periods, not one (P-17). A goal with no cell to stand on
        // near it is refused without a search and retried as before.
        self.dry = plan.exhausted && plan.nodes > 0;
        self.blocked = plan.path.is_empty();
        self.stranded = plan.stranded;
        self.last_plan = [plan.nodes, plan.poses];
        self.last_probe = [plan.scans, plan.samples];
        self.partial_path = plan.partial;
        self.path = plan.path;
        self.cursor = 0;
    }
    /// Whether a route search is still walking its frontier (ADR-306).
    pub fn searching(&self) -> bool {
        self.hunt.is_some()
    }
    /// Cells the search in flight has expanded so far; zero when none is
    /// in flight or when it has not walked yet (ADR-314).
    pub fn frontier_nodes(&self) -> u32 {
        self.hunt.as_ref().map_or(0, |h| h.nodes())
    }
    /// One tick's worth of the search in flight, if there is one. A world
    /// that changed under it drops what it scanned, not the search.
    fn hunt_step(
        &mut self,
        w: &World,
        b: &UnitBody,
        others: &[Obstacle],
        rubble: &nav::vehicle::Rubble,
        tick: u32,
        steady: bool,
    ) {
        let Some(mut hunt) = self.hunt.take() else {
            return;
        };
        if !hunt.starts_at([self.pos[0], self.pos[2]], self.drive.head) {
            // Braking, contact impulses and escape manoeuvres can move a
            // hull while its search is pending. Its checked first edge no
            // longer starts here. Retain the order; restart through the
            // normal shared allowance after any active escape ends.
            self.next_plan = tick;
            return;
        }
        if !steady {
            hunt.forget_world();
        }
        let half = self.shape(b).half.map(Fx::from_int);
        let clear = |p: [Fx; 2], head: i32| {
            !others.iter().any(|o| {
                o.overlaps(&Obstacle {
                    pos: [p[0], Fx::ZERO, p[1]],
                    head,
                    half,
                })
            })
        };
        let slice = self.navigation.search_nodes_per_tick.max(1);
        match hunt.advance_in(w, rubble, &clear, slice) {
            Some(plan) => {
                // The cadence of the next try counts from the answer, not
                // from the question (ADR-306).
                self.take_plan(plan);
                self.next_plan = tick.saturating_add(self.replan_after_search());
            }
            None => self.hunt = Some(hunt),
        }
    }
    /// Ticks until the next search after the last one: four replan periods
    /// when it ran the open set dry, one otherwise (P-17).
    fn replan_after_search(&self) -> u32 {
        if self.dry {
            self.replan.saturating_mul(4)
        } else {
            self.replan
        }
    }
    /// May this hull begin a search this tick, and did it. The range
    /// hands the permission out under a ceiling (ADR-312); a hull refused
    /// keeps its order and asks again next tick.
    pub fn allow_start(&mut self, may: bool) {
        self.may_start = may;
        self.started = false;
    }
    pub fn started_search(&self) -> bool {
        self.started
    }
    /// Planner budget and weight, for benches and tests; the data sets them.
    pub fn set_search(&mut self, nodes: usize, heuristic_pct: u32) {
        self.navigation.search_nodes = nodes;
        self.navigation.heuristic_pct = heuristic_pct;
    }
    pub fn stop(&mut self) {
        self.hunt = None;
        self.path.clear();
        self.partial_path = false;
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
        let terrain_tilt = if self.legs_lost.iter().any(|lost| *lost)
            || self.legs_buckled.iter().any(|buckled| *buckled)
        {
            self.chassis.max_tilt_pct
        } else {
            self.terrain_tilt_pct
        };
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
            min_up: Fx::from_ratio(terrain_tilt, 100),
            height: b.vol.ny as i32 * b.mm as i32 / units,
            span: self.chassis.contact_cm * 10 / units,
            ramp: Fx::from_ratio(self.terrain_approach_pct, 100),
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
        rubble: &nav::vehicle::Rubble,
        steady: bool,
    ) {
        self.cut.clear();
        self.contact = None;
        if !self.enabled {
            // A parked hull that a burst pushed slides and rocks (ADR-286);
            // a parked walker still stands on its feet, and its hull on them.
            self.passive(w, rubble, b, gravity, response, others);
            return;
        }
        let engine = engine && response.traction > Fx::ZERO && self.carried();
        // A search begun on an earlier tick walks on before anything asks
        // for another (ADR-306).
        self.hunt_step(w, b, others, rubble, tick, steady);
        if self.hunt.is_none()
            && self.escape_left == 0
            && (self.blocked || self.path.is_empty())
            && tick >= self.next_plan
            && self.may_start
        {
            if let Some(goal) = self.destination {
                self.started = true;
                self.move_to(w, goal, b, others, rubble);
                self.next_plan = tick.saturating_add(self.replan_after_search());
            }
        }
        if self.cursor == self.path.len()
            && self.hunt.is_none()
            && self.escape_left == 0
            && !self.route.is_empty()
            && !self.partial_path
            && alive
            && engine
            && (!self.blocked || (tick.is_multiple_of(self.replan) && tick >= self.next_plan))
            && self.may_start
        {
            let goal = self.route[self.waypoint];
            self.started = true;
            self.move_to(w, goal, b, others, rubble);
            if self.dry {
                self.next_plan = tick.saturating_add(self.replan_after_search());
            }
        }
        let mut want = drive::NO_WAY;
        let mut speed = Fx::ZERO;
        let base_limits = Limits::of(&self.chassis, 30);
        let brake = response
            .limits(&base_limits, false)
            .brake
            .mul(self.drive_scale)
            .mul(self.hull.load);
        if alive && engine && self.escape_left == 0 && self.cursor < self.path.len() {
            let goal = self.path[self.cursor];
            let dx = Fx::from_int(goal[0]).sub(self.pos[0]);
            let dz = Fx::from_int(goal[1]).sub(self.pos[2]);
            let arrival = if self.partial_path || self.cursor + 1 < self.path.len() {
                Fx::from_ratio(
                    self.navigation.corner_arrival_mm,
                    voxel::unit::WORLD_MM as i32,
                )
                .min(Fx::from_int(self.arrival))
            } else {
                Fx::from_int(self.arrival)
            };
            if dx.mul(dx).add(dz.mul(dz)) <= arrival.mul(arrival) {
                self.cursor += 1;
                self.escape = 0;
                let reached_end = self.cursor == self.path.len();
                if reached_end && self.partial_path {
                    // Keep the player's goal and patrol waypoint. The next tick
                    // requests a continuation under the normal start allowance.
                    self.path.clear();
                    self.cursor = 0;
                    self.next_plan = tick.saturating_add(1);
                } else if reached_end {
                    self.destination = None;
                }
                if reached_end && !self.partial_path && !self.route.is_empty() {
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
                // Reserve the actual stopping distance before a checked turn
                // centre. Merely asking for 2*distance can brake too late.
                let distance = dx.abs().max(dz.abs());
                let stopping = brake.mul(Fx::from_int(60)).mul(distance);
                if self.drive.vel.mul(self.drive.vel) > stopping {
                    speed = Fx::ZERO;
                }
                if alignment > Fx::from_ratio(self.alignment, 100)
                    && (self.drive.vel > Fx::ZERO
                        || angle::diff(want, self.drive.head).abs() <= base_limits.turn)
                {
                    speed = speed.mul(alignment);
                } else {
                    speed = Fx::ZERO;
                    // The planner reserves a pivot, not a moving curve. Finish
                    // braking along the checked segment before rotating.
                    if self.drive.vel.abs() > brake {
                        want = self.drive.head;
                    }
                }
            }
        }
        // Repeat recovery only after a net move of at least half a navigation
        // cell. Returning to the same spot cannot renew the cycle forever.
        if self.escape >= ESCAPES && self.escape_left == 0 {
            let dx = self.pos[0].sub(self.recovery_origin[0]);
            let dz = self.pos[2].sub(self.recovery_origin[1]);
            let cell = Fx::from_ratio(self.navigation.cell_units, 2);
            if dx.mul(dx).add(dz.mul(dz)) >= cell.mul(cell) {
                self.escape = 0;
            }
        }
        if alive
            && engine
            && self.blocked_ticks >= self.navigation.recovery_ticks
            && self.escape_left == 0
            && self.escape < ESCAPES
        {
            if self.escape == 0 {
                self.recovery_origin = [self.pos[0], self.pos[2]];
            }
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
        let lim = response.limits(&base_limits, want == drive::NO_WAY);
        let old = self.drive;
        let old_pos = self.pos;
        // Two poses and the cut lie within a hull length of the vehicle; a
        // whole-map cache would cost more to clear than the poses to compute.
        let mut ground =
            nav::vehicle::Probe::around(w, &self.yielding, [self.pos[0], self.pos[2]], 96)
                .with_rubble(rubble);
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
        // A burst's shove rides along with the drive's own step (ADR-286)
        // and is spent by the brakes, or by the terrain if it refuses.
        let nx = self.pos[0]
            .add(delta.dx)
            .add(self.shove.x.div(Fx::from_int(30)));
        let nz = self.pos[2]
            .add(delta.dz)
            .add(self.shove.z.div(Fx::from_int(30)));
        let (x, z) = (nx.to_int_trunc(), nz.to_int_trunc());
        let target = ground.pose([nx, nz], next.head, shape);
        let mut candidate = Obstacle::of(self, b);
        candidate.pos[0] = nx;
        candidate.pos[2] = nz;
        let collision = others.iter().position(|p| {
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
        let terrain_allowed = clear
            && x >= EDGE
            && z >= EDGE
            && x < w.nx as i32 - EDGE
            && z < w.nz as i32 - EDGE
            && target.height.sub(current.height) <= Fx::from_int(self.step);
        let allowed = terrain_allowed && collision.is_none();
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
            if terrain_allowed {
                if let Some(index) = collision {
                    // The route was valid when planned, but another hull
                    // now occupies it. Ask for one new route promptly;
                    // repeated refusals keep the normal replan cadence.
                    if self.blocked_ticks == 0 && self.destination.is_some() {
                        self.next_plan = self
                            .next_plan
                            .min(tick.saturating_add(self.navigation.contact_replan_ticks));
                    }
                    let drive = V3::new(
                        delta.dx.mul(Fx::from_int(30)),
                        Fx::ZERO,
                        delta.dz.mul(Fx::from_int(30)),
                    );
                    self.record_contact(index, &others[index], drive.add(self.shove));
                }
            } else {
                self.shove = V3::ZERO;
            }
            Fx::ZERO
        };
        self.brake_shove(&lim);
        if (allowed && target.crush > 0) || (!allowed && current.crush > 0) {
            self.cut = ground.crushed([self.pos[0], self.pos[2]], self.drive.head, shape);
        }
        // A dry search is a block even if its tiny reachable pocket contains
        // several cells. Only recovery can change the next search start.
        // A search still walking is not a block: the hull is waiting.
        if (self.stranded || self.dry)
            && self.hunt.is_none()
            && self.destination.is_some()
            && self.path.is_empty()
        {
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
        let f = ground.support_floors([self.pos[0], self.pos[2]], self.drive.head, shape);
        let floors = [f[3], f[1], f[2], f[0]];
        self.floors = floors;
        // A walker stands on the legs it still has (ADR-280); a tracked
        // hull on all four nominal points over the probe's floors.
        let (support, ground) = self
            .foot_supports(&nominal, &floors)
            .unwrap_or_else(|| (nominal.to_vec(), floors.to_vec()));
        let r = self.ride_now(b, gravity, response);
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
        self.pos[1] = self
            .hull
            .y
            .sub(crate::ride::ride_of(&self.chassis).clearance);
        let linear = V3::new(
            self.pos[0].sub(old_pos[0]),
            Fx::ZERO,
            self.pos[2].sub(old_pos[2]),
        );
        self.step_feet(
            w,
            rubble,
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
        b.fx(self.shove.x);
        b.fx(self.shove.z);
        // A search still walking its frontier (ADR-306). Without it a
        // restored hull would answer the same order a tick later than the
        // hull it was restored from.
        b.bool(self.hunt.is_some());
        if let Some(hunt) = &self.hunt {
            hunt.save(b);
        }
        b.bool(self.partial_path);
        for v in self.recovery_origin {
            b.fx(v);
        }
    }
    pub fn load(&mut self, s: &mut Scan) -> bool {
        self.load_version(s, true)
    }
    pub(crate) fn load_version(&mut self, s: &mut Scan, continuation: bool) -> bool {
        self.contact = None;
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
        self.shove = V3::new(s.fx(), Fx::ZERO, s.fx());
        if self.shove.len_sq() > Fx::from_int(1_000_000) {
            return false;
        }
        self.hunt = if s.bool() {
            match nav::vehicle::Hunt::load(s, &self.yielding) {
                Some(hunt) => Some(Box::new(hunt)),
                None => return false,
            }
        } else {
            None
        };
        self.partial_path = continuation && s.bool();
        self.recovery_origin = if continuation {
            [s.fx(), s.fx()]
        } else {
            [self.pos[0], self.pos[2]]
        };
        s.ok()
            && (!self.partial_path || self.destination.is_some())
            && self
                .recovery_origin
                .iter()
                .all(|p| *p >= Fx::ZERO && *p < Fx::from_int(MAX_SIDE))
            && self.escape_left <= self.navigation.escape_ticks
            && (0..angle::FULL).contains(&self.recovery_head)
            && self.pos[0] >= Fx::ZERO
            && self.pos[0] < Fx::from_int(MAX_SIDE)
            && self.pos[2] >= Fx::ZERO
            && self.pos[2] < Fx::from_int(MAX_SIDE)
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
                .all(|p| p.iter().all(|v| *v >= 0 && *v < MAX_SIDE))
            && self
                .destination
                .is_none_or(|p| p.iter().all(|v| (EDGE..MAX_SIDE).contains(v)))
            && self.cursor <= self.path.len()
            && (self.route.is_empty() || self.waypoint < self.route.len())
    }
}
pub fn floor(w: &World, x: i32, z: i32) -> i32 {
    if x < 0 || z < 0 || x >= w.nx as i32 || z >= w.nz as i32 {
        return 192;
    }
    // The world updates this ceiling on every applied edit; probing the
    // height voxel by voxel here repeats that work for every foot contact.
    w.column_ceiling(x as u32, z as u32) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use modding::range::RangeUnitKind;
    struct Ground {
        depth: i32,
    }
    #[test]
    fn displaced_search_restarts_from_the_actual_pose_and_replays() {
        let world = ground(0);
        let body = voxel::unit::tank();
        let goal = [430, 100];
        let rubble = nav::vehicle::Rubble::new();
        for displacement in 0..3 {
            let mut motion = Motion::new([60, 16, 100], RangeUnitKind::Tank, &body);
            motion.navigation.search_nodes_per_tick = 8;
            assert!(motion.move_to(&world, goal, &body, &[], &rubble));
            assert!(motion.searching(), "fixture needs an unfinished search");
            match displacement {
                0 => motion.pos[2] = Fx::from_int(180),
                1 => motion.drive.head = angle::FULL / 4,
                _ => {
                    // A real contact impulse displaces the hull during the
                    // drive phase, after this tick's search slice ran.
                    let old_pos = motion.pos;
                    motion.receive_contact(V3::new(Fx::ZERO, Fx::ZERO, Fx::from_int(20)));
                    step(&mut motion, &world, &body, &[], 0);
                    assert_ne!(motion.pos, old_pos);
                    assert!(motion.searching());
                }
            }
            let mut saved = Blob::new();
            motion.save(&mut saved);
            let mut restored = motion.clone();
            assert!(restored.load(&mut Scan::new(&saved.v)));
            for m in [&mut motion, &mut restored] {
                m.allow_start(false);
                step(m, &world, &body, &[], 1);
                assert!(!m.searching(), "search still uses the previous pose");
                assert!(m.path.is_empty());
                assert_eq!(m.destination, Some(goal));
            }
            for tick in 2..1200 {
                for m in [&mut motion, &mut restored] {
                    m.allow_start(true);
                    step(m, &world, &body, &[], tick);
                }
                let mut a = Blob::new();
                let mut b = Blob::new();
                motion.save(&mut a);
                restored.save(&mut b);
                assert_eq!(a.v, b.v, "replay at tick {tick}");
                if motion.destination.is_none() {
                    break;
                }
            }
            assert!(motion.destination.is_none(), "restart did not reach goal");
            for (axis, coordinate) in [0, 2].into_iter().zip(goal) {
                assert!(
                    motion.pos[axis].sub(Fx::from_int(coordinate)).abs()
                        <= Fx::from_int(motion.arrival)
                );
            }
        }
    }
    #[test]
    fn partial_routes_keep_the_order_and_patrol_waypoint_until_arrival() {
        struct Trees;
        impl voxel::Generator for Trees {
            fn material(&self, x: u32, y: u32, _z: u32) -> u16 {
                if y < 16 || ((240..248).contains(&x) && (60..140).contains(&_z) && y < 50) {
                    1
                } else if x % 64 < 8 && y < 40 {
                    4
                } else {
                    0
                }
            }
        }
        let world = World::new(640, 64, 256, 100, Box::new(Trees));
        let body = voxel::unit::tank();
        let goal = [430, 100];
        let mut motion = Motion::new([60, 16, 100], RangeUnitKind::Tank, &body);
        motion.route = vec![goal, [60, 100]];
        motion.destination = Some(goal);
        motion.enabled = true;
        motion.take_plan(nav::vehicle::Route {
            path: vec![[190, 100]],
            partial: true,
            stranded: false,
            nodes: 1,
            poses: 1,
            exhausted: false,
            scans: 0,
            samples: 0,
        });
        let mut saw_partial = false;
        let mut reached = false;
        for tick in 0..2400 {
            if motion.partial_path {
                saw_partial = true;
                assert_eq!(motion.destination, Some(goal));
                assert_eq!(motion.waypoint, 0);
                let mut bytes = Blob::new();
                motion.save(&mut bytes);
                let mut restored = Motion::new([60, 16, 100], RangeUnitKind::Tank, &body);

                assert!(restored.load(&mut Scan::new(&bytes.v)));
                let mut check = Blob::new();
                restored.save(&mut check);
                assert_eq!(bytes.v, check.v);
                motion = restored;
            }
            motion.allow_start(true);
            step(&mut motion, &world, &body, &[], tick);
            if motion.destination.is_none() && motion.waypoint == 1 {
                assert!((motion.pos[0].to_int_trunc() - goal[0]).abs() <= motion.arrival);
                reached = true;
                break;
            }
        }
        assert!(saw_partial, "fixture must exercise continuation");
        assert!(
            reached,
            "partial route never reached its real goal: {:?}",
            motion.pos
        );
        motion.stop();
        assert!(!motion.partial_path);
    }
    #[test]
    fn old_motion_without_continuation_flag_still_loads() {
        let body = voxel::unit::tank();
        let motion = Motion::new([60, 16, 100], RangeUnitKind::Tank, &body);
        let mut bytes = Blob::new();
        motion.save(&mut bytes);
        let extra = bytes.v.split_off(bytes.v.len() - 17);
        assert_eq!(extra[0], 0);
        let mut restored = Motion::new([60, 16, 100], RangeUnitKind::Tank, &body);
        assert!(restored.load_version(&mut Scan::new(&bytes.v), false));
        assert!(!restored.partial_path);
        let mut check = Blob::new();
        restored.save(&mut check);
        bytes.v.extend(extra);
        assert_eq!(bytes.v, check.v);
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

    #[test]
    fn walker_plans_and_cuts_a_tree_belt() {
        struct TreeBelt;
        impl voxel::Generator for TreeBelt {
            fn material(&self, x: u32, y: u32, _z: u32) -> u16 {
                if y < 16 {
                    1
                } else if (120..128).contains(&x) && y < 42 {
                    4
                } else {
                    0
                }
            }
        }
        let mut w = World::new(256, 64, 256, 100, Box::new(TreeBelt));
        let body = voxel::unit::strider();
        let mut m = Motion::new([60, 16, 128], RangeUnitKind::Strider, &body);
        let goal = [190, 128];
        assert!(m.move_to(&w, goal, &body, &[], &nav::vehicle::Rubble::new()));
        let mut cut_trunk = false;
        for tick in 0..600 {
            step(&mut m, &w, &body, &[], tick);
            for [x, y, z] in m.cut.iter().copied() {
                cut_trunk |= w.material(x, y, z) == 4;
                w.queue_dig(x, y, z);
            }
            w.flush();
            if m.destination.is_none() {
                break;
            }
        }
        assert!(
            cut_trunk,
            "walker crossed the forest without felling a trunk"
        );
        assert!(m.destination.is_none(), "walker stopped at {:?}", m.pos);
    }
    #[test]
    fn walker_climbs_a_ridge_that_refuses_a_tank() {
        struct Ridge;
        impl voxel::Generator for Ridge {
            fn material(&self, x: u32, y: u32, _z: u32) -> u16 {
                let rise = (x as i32 - 220).clamp(0, 80) * 3 / 4;
                u16::from(y < (16 + rise) as u32)
            }
        }
        let w = World::new(640, 128, 640, 100, Box::new(Ridge));
        let walker_body = voxel::unit::strider();
        let tank_body = voxel::unit::tank();
        let walker = Motion::new([160, 16, 320], RangeUnitKind::Strider, &walker_body);
        let tank = Motion::new([160, 16, 320], RangeUnitKind::Tank, &tank_body);
        let mut probe = nav::vehicle::Probe::new(&w);
        let at = [Fx::from_int(260), Fx::from_int(320)];
        assert!(probe.pose(at, 0, tank.shape(&tank_body)).refused());
        assert!(!probe.pose(at, 0, walker.shape(&walker_body)).refused());
        let mut m = walker;
        assert!(m.move_to(
            &w,
            [365, 320],
            &walker_body,
            &[],
            &nav::vehicle::Rubble::new()
        ));
        for tick in 0..1000 {
            step(&mut m, &w, &walker_body, &[], tick);
            if m.destination.is_none() {
                break;
            }
        }
        assert!(
            m.destination.is_none(),
            "walker stuck on ridge at {:?}",
            m.pos
        );
        assert!(m.pos[1] > Fx::from_int(60));
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
            &nav::vehicle::Rubble::new(),
            true,
        );
    }
    #[test]
    fn a_dry_reachable_pocket_starts_recovery_without_declaring_start_stranded() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let mut m = Motion::new([200, 16, 300], RangeUnitKind::Tank, &body);
        m.enabled = true;
        m.destination = Some([430, 300]);
        m.dry = true;
        m.stranded = false;
        m.next_plan = 1000;
        let from = m.pos;
        for tick in 0..80 {
            step(&mut m, &w, &body, &[], tick);
        }
        assert!(m.escape > 0);
        assert_ne!(m.pos, from, "dry pocket stayed idle forever");
    }

    #[test]
    fn driving_and_blast_shoves_cross_the_old_range_boundary() {
        let w = World::new(1024, 64, 1024, 100, Box::new(Ground { depth: 0 }));
        for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
            let body = match kind {
                RangeUnitKind::Tank => voxel::unit::tank(),
                RangeUnitKind::Strider => voxel::unit::strider(),
            };
            for axis in [0, 1] {
                let mut m = Motion::new([590, 16, 590], kind, &body);
                let mut goal = [590, 590];
                goal[axis] = 720;
                assert!(m.move_to(&w, goal, &body, &[], &nav::vehicle::Rubble::new()));
                for tick in 0..900 {
                    step(&mut m, &w, &body, &[], tick);
                }
                assert!(
                    m.pos[axis * 2] > Fx::from_int(700),
                    "stopped at {:?}",
                    m.pos
                );
            }
            let mut m = Motion::new([700, 16, 700], kind, &body);
            m.shove = V3::new(Fx::from_int(30), Fx::ZERO, Fx::from_int(30));
            let moved = m.slide(
                &w,
                &nav::vehicle::Rubble::new(),
                &body,
                &[],
                crate::mobility::Mobility::intact(true),
            );
            assert!(moved.x > Fx::ZERO && moved.z > Fx::ZERO);
        }
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
    /// A buckled leg carries nothing and folds on its foot (ADR-288): the
    /// foot stays on the ground drawn in under the hip, the walker sinks
    /// and leans over it and still walks.
    #[test]
    fn a_buckled_leg_carries_nothing_and_folds_on_its_foot() {
        let w = ground(0);
        let body = voxel::unit::strider();
        let mut m = Motion::new([200, 16, 300], RangeUnitKind::Strider, &body);
        let nominal = crate::ride::supports_of(&m.chassis);
        m.legs_buckled[1] = true;
        m.enabled = true;
        m.destination = Some([430, 300]);
        m.path = vec![[430, 300]];
        let gone = nominal[usize::from(body.legs[1].support)];
        let leg = body.legs[1];
        for tick in 0..300 {
            step(&mut m, &w, &body, &[], tick);
            let (support, _) = m.foot_supports(&nominal, &m.floors).unwrap();
            assert_eq!(support.len(), 3);
            assert!(!support.contains(&gone));
            let c = m.feet[1].contact.expect("the folded leg has a contact");
            // On the ground: the sole's rest height over the floor.
            assert!(
                (c.pos
                    .y
                    .sub(Fx::from_int(16))
                    .sub(Fx::from_int(i32::from(leg.ankle.1))))
                .abs()
                    < Fx::from_ratio(1, 2),
                "foot off the ground at tick {tick}: {c:?}"
            );
            // Drawn in under the hip: nearer than the rest ankle, measured
            // from the hip's projection at ankle height so the hull's lean
            // moves both alike.
            let frame = m.frame(&body);
            let hip = frame.world([
                Fx::from_int(i32::from(leg.hip.0)),
                Fx::from_int(i32::from(leg.ankle.1)),
                Fx::from_int(i32::from(leg.hip.2)),
            ]);
            let rest = frame.world([
                Fx::from_int(i32::from(leg.ankle.0)),
                Fx::from_int(i32::from(leg.ankle.1)),
                Fx::from_int(i32::from(leg.ankle.2)),
            ]);
            let flat = |p: V3| V3::new(p.x.sub(hip.x), Fx::ZERO, p.z.sub(hip.z)).len_sq();
            assert!(
                flat(c.pos) < flat(rest).div(Fx::from_int(4)),
                "not drawn in at tick {tick}"
            );
            assert!(m.feet[1].planted());
        }
        assert!(
            m.pos[1] < Fx::from_int(16).sub(Fx::from_ratio(1, 10)),
            "did not sink"
        );
        assert!(m.pos[0] > Fx::from_int(300), "did not walk: {:?}", m.pos);
    }
    #[test]
    fn a_burst_beside_a_parked_tank_shoves_and_rocks_it() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let mut m = Motion::new([300, 16, 300], RangeUnitKind::Tank, &body);
        for tick in 0..5 {
            step(&mut m, &w, &body, &[], tick);
        }
        let before = m.pos;
        let level = m.hull;
        // A hundred and twenty thousand newton-seconds a hull's width
        // beyond the near face, at the ground: what eighteen megajoules of
        // a shell delivered to a tank at that distance before ADR-297. The
        // tank's box reaches thirty units from its centre.
        m.impulse(&body, [254, 16, 300], 120_000);
        assert!(m.shove.x > Fx::from_int(5), "shove {:?}", m.shove);
        assert!(m.hull.w.len_sq() > Fx::ZERO, "no spin");
        // A push on the flank at ground height is level (ADR-302): the
        // hop belongs to a push under the belly.
        assert_eq!(m.hull.vy, Fx::ZERO, "a flank push lifted the hull");
        for tick in 5..125 {
            step(&mut m, &w, &body, &[], tick);
        }
        assert!(
            m.pos[0].sub(before[0]) > Fx::ONE,
            "did not move: {:?}",
            m.pos
        );
        assert_eq!(m.shove, V3::ZERO, "the brakes did not spend the shove");
        assert!(
            m.hull.up.y > Fx::from_ratio(999, 1000),
            "did not settle: {:?}",
            m.hull.up
        );
        assert!((m.hull.y.sub(level.y)).abs() < Fx::from_ratio(1, 10));
        // The same newton-seconds from twice as far push no less: the
        // falling off with distance is the wave's, which delivers less
        // pressure there, not the push's (ADR-297).
        let mut far = Motion::new([300, 16, 300], RangeUnitKind::Tank, &body);
        far.impulse(&body, [238, 16, 300], 120_000);
        let mut near = Motion::new([300, 16, 300], RangeUnitKind::Tank, &body);
        near.impulse(&body, [254, 16, 300], 120_000);
        assert!(
            far.shove.x >= near.shove.x.sub(Fx::from_ratio(1, 100)),
            "far {:?} near {:?}",
            far.shove,
            near.shove
        );
        // Out of reach: nothing.
        let mut away = Motion::new([300, 16, 300], RangeUnitKind::Tank, &body);
        away.impulse(&body, [200, 16, 300], 120_000);
        assert_eq!(away.shove, V3::ZERO);
    }
    /// A push landing under the belly lifts the hull (ADR-302): the same
    /// newton-seconds from below hop it and it comes back down, where the
    /// flank push only shoves and rocks.
    #[test]
    fn a_push_under_the_belly_lifts_the_hull_and_it_comes_down() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let mut m = Motion::new([300, 16, 300], RangeUnitKind::Tank, &body);
        for tick in 0..5 {
            step(&mut m, &w, &body, &[], tick);
        }
        let level = m.hull.y;
        m.impulse_through(&body, [300, 12, 300], [300, 17, 300], 120_000);
        assert!(m.hull.vy > Fx::ONE, "no lift: {:?}", m.hull.vy);
        assert_eq!(m.shove, V3::ZERO, "a push from below shoved sideways");
        let mut hopped = Fx::ZERO;
        for tick in 5..125 {
            step(&mut m, &w, &body, &[], tick);
            hopped = hopped.max(m.hull.y.sub(level));
        }
        assert!(hopped > Fx::from_ratio(3, 10), "did not hop: {hopped:?}");
        assert!(
            m.hull.y.sub(level).abs() < Fx::from_ratio(1, 10),
            "did not come down: {:?}",
            m.hull.y.sub(level)
        );
    }
    #[test]
    fn an_off_centre_burst_under_a_tank_shoves_as_well_as_lifts() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let mut m = Motion::new([300, 16, 300], RangeUnitKind::Tank, &body);
        for tick in 0..5 {
            step(&mut m, &w, &body, &[], tick);
        }
        let start = m.pos;
        let level = m.hull.y;
        // Under the front of the box, but well away from the centre.
        // Previously the nearest point was directly above the burst and
        // all of this impulse went into an implausible vertical hop.
        let through = m.impulse(&body, [274, 15, 300], 120_000);
        assert!(through[0] > 274, "no pressure span: {through:?}");
        assert!(m.shove.x > Fx::ONE, "no lateral impulse: {:?}", m.shove);
        assert!(m.hull.vy > Fx::ZERO, "no lift: {:?}", m.hull.vy);
        let mut peak = Fx::ZERO;
        for tick in 5..125 {
            step(&mut m, &w, &body, &[], tick);
            peak = peak.max(m.hull.y.sub(level));
        }
        assert!(m.pos[0] > start[0], "tank did not shift: {:?}", m.pos);
        assert!(peak > Fx::ZERO, "tank did not hop");
        assert!(m.hull.y.sub(level).abs() < Fx::from_ratio(1, 10));
        assert_eq!(m.shove, V3::ZERO);
    }
    #[test]
    fn a_damaged_tank_shoved_into_a_crater_lands_without_entering_another_hull() {
        let w = ground(12);
        let mut body = voxel::unit::tank();
        let original = crate::burst::chunk::Shape::from_body(&body);
        let mut m = Motion::new([290, 16, 300], RangeUnitKind::Tank, &body);
        for tick in 0..90 {
            step(&mut m, &w, &body, &[], tick);
        }
        for z in 0..body.vol.nz as usize {
            for y in 0..body.vol.ny as usize {
                for x in 0..body.vol.nx as usize / 4 {
                    if body.vol.get(x, y, z) && body.part_at(x, y, z) == voxel::Part::Armour {
                        body.remove(x, y, z);
                    }
                }
            }
        }
        let response = crate::mobility::Mobility::from_shapes(
            &original,
            &crate::burst::chunk::Shape::from_body(&body),
            [body.vol.nx, body.vol.ny, body.vol.nz],
            Fx::ONE,
        );
        assert!(response.inverse_mass > Fx::ONE);
        let half = Obstacle::of(&m, &body).half;
        let neighbour = Obstacle {
            pos: [
                Fx::from_int(290)
                    .add(half[0].mul(Fx::from_int(2)))
                    .add(Fx::from_int(8)),
                Fx::from_int(16),
                Fx::from_int(300),
            ],
            head: 0,
            half,
        };
        let origin = m.pos;
        m.impulse(&body, [264, 8, 300], 240_000);
        let mut peak = m.pos[1];
        for tick in 90..300 {
            m.passive(
                &w,
                &nav::vehicle::Rubble::new(),
                &body,
                98,
                response,
                &[neighbour],
            );
            peak = peak.max(m.pos[1]);
            assert_eq!(Obstacle::of(&m, &body).penetration(&neighbour), Fx::ZERO);
            assert!(m.pos[1].abs() < Fx::from_int(64), "unbounded hop at {tick}");
        }
        assert!(m.pos[0] > origin[0], "no shove: {:?}", m.pos);
        let gap = neighbour.pos[0]
            .sub(m.pos[0])
            .sub(half[0].mul(Fx::from_int(2)));
        assert!(gap >= Fx::ZERO && gap < Fx::ONE, "missed contact: {gap:?}");
        assert!(peak > origin[1], "no hop: {peak:?}");
        assert!(m.pos[1] < origin[1].sub(Fx::ONE), "missed crater slope");
        assert!(m.hull.load > Fx::ZERO, "no landing: {:?}", m.hull);
        assert!(m.hull.vy.abs() < Fx::ONE, "still falling: {:?}", m.hull);
        assert!(m.hull.up.y > Fx::from_ratio(95, 100), "did not settle");
    }
    #[test]
    fn a_concrete_wall_spends_the_shove_without_a_hull_contact() {
        struct Wall;
        impl voxel::Generator for Wall {
            fn material(&self, x: u32, y: u32, z: u32) -> u16 {
                if y < 16 {
                    1
                } else if (335..340).contains(&x) && (250..350).contains(&z) && y < 50 {
                    2
                } else {
                    0
                }
            }
        }
        let w = World::new(640, 64, 640, 100, Box::new(Wall));
        let body = voxel::unit::tank();
        let mut m = Motion::new([300, 16, 300], RangeUnitKind::Tank, &body);
        for tick in 0..20 {
            step(&mut m, &w, &body, &[], tick);
        }
        m.impulse(&body, [254, 16, 300], 120_000);
        for tick in 20..100 {
            step(&mut m, &w, &body, &[], tick);
            assert!(m.take_contact().is_none(), "wall became a moving hull");
        }
        assert!(
            m.pos[0] < Fx::from_int(305),
            "crossed the wall: {:?}",
            m.pos
        );
        assert_eq!(m.shove, V3::ZERO, "wall did not spend the shove");
    }
    /// A wide two-legged body stands on both legs, each foot carrying
    /// heel and toe; a buckled leg leaves one side, and the body falls onto
    /// the broken leg: the hull leans to its limit and comes down, and the
    /// leg folds under it (ADR-288, the owner's two-legged check).
    #[test]
    fn a_wide_two_legged_body_falls_onto_a_buckled_leg() {
        let w = ground(0);
        let body = voxel::unit::biped();
        assert_eq!(body.legs.len(), 2);
        let mut m = Motion::new([300, 16, 300], RangeUnitKind::Strider, &body);
        let nominal = crate::ride::supports_of(&m.chassis);
        for tick in 0..60 {
            step(&mut m, &w, &body, &[], tick);
        }
        let (support, _) = m.foot_supports(&nominal, &m.floors).unwrap();
        assert_eq!(support.len(), 4, "each foot carries heel and toe");
        assert!(
            m.hull.up.y > Fx::from_ratio(99, 100),
            "did not stand: {:?}",
            m.hull.up
        );
        let level = m.hull.y;
        let leg = body.legs[0];
        let hip_of = |m: &Motion| {
            m.frame(&body).world([
                Fx::from_int(i32::from(leg.hip.0)),
                Fx::from_int(i32::from(leg.hip.1)),
                Fx::from_int(i32::from(leg.hip.2)),
            ])
        };
        let ankle_of = |m: &Motion| {
            m.frame(&body).world([
                Fx::from_int(i32::from(leg.ankle.0)),
                Fx::from_int(i32::from(leg.ankle.1)),
                Fx::from_int(i32::from(leg.ankle.2)),
            ])
        };
        let rest_length = hip_of(&m).sub(ankle_of(&m)).len_sq().sqrt();
        // The -z leg buckles: only the +z foot carries.
        m.legs_buckled[0] = true;
        for tick in 60..240 {
            step(&mut m, &w, &body, &[], tick);
        }
        let (support, _) = m.foot_supports(&nominal, &m.floors).unwrap();
        assert_eq!(support.len(), 2);
        assert!(support.iter().all(|s| s.z > Fx::ZERO));
        assert!(
            m.hull.up.z < Fx::from_ratio(-1, 10),
            "did not lean onto the broken leg: {:?}",
            m.hull.up
        );
        assert!(
            m.hull.y < level.sub(Fx::ONE),
            "did not come down: {:?} from {level:?}",
            m.hull.y
        );
        let c = m.feet[0].contact.expect("the folded leg keeps its foot");
        let length = hip_of(&m).sub(c.pos).len_sq().sqrt();
        assert!(
            length < rest_length.mul(Fx::from_ratio(9, 10)),
            "the leg did not fold under the body: {length:?} of {rest_length:?}"
        );
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
            "a {:?} path {:?} search {:?} stranded {} escape {} blocked {} dry {} plan {:?}; b {:?} path {:?} search {:?} stranded {} escape {} blocked {} dry {} plan {:?}",
            a.pos, a.path, a.hunt, a.stranded,a.escape,a.blocked,a.dry,a.last_plan,
            b.pos, b.path, b.hunt,b.stranded,b.escape,b.blocked,b.dry,b.last_plan
        );
    }
    #[test]
    fn a_new_hull_blocker_prompts_a_route_around_it_without_waiting_for_recovery() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let mut mover = Motion::new([200, 16, 300], RangeUnitKind::Tank, &body);
        let blocker = Motion::new([280, 16, 300], RangeUnitKind::Tank, &body);
        let other = Obstacle::of(&blocker, &body);
        mover.enabled = true;
        mover.destination = Some([430, 300]);
        mover.path = vec![[430, 300]];
        mover.next_plan = 1000;
        let mut contact_tick = None;
        for tick in 0..300 {
            step(&mut mover, &w, &body, &[other], tick);
            if mover.blocked {
                contact_tick = Some(tick);
                assert!(
                    mover.next_plan <= tick + 2,
                    "contact postponed replanning to {}",
                    mover.next_plan
                );
                break;
            }
        }
        let first = contact_tick.expect("fixture did not meet the new blocker");
        let mut snapshot = Blob::new();
        mover.save(&mut snapshot);
        let mut restored = Motion::new([200, 16, 300], RangeUnitKind::Tank, &body);
        assert!(restored.load(&mut Scan::new(&snapshot.v)));
        let mut arrived = false;
        for tick in first + 1..1200 {
            step(&mut mover, &w, &body, &[other], tick);
            step(&mut restored, &w, &body, &[other], tick);
            let mut a = Blob::new();
            let mut b = Blob::new();
            mover.save(&mut a);
            restored.save(&mut b);
            assert_eq!(a.v, b.v, "route diverged after loading at {tick}");
            assert_eq!(Obstacle::of(&mover, &body).penetration(&other), Fx::ZERO);
            if mover.destination.is_none() {
                arrived = true;
                break;
            }
        }
        assert!(arrived, "mover never passed the blocker: {:?}", mover.pos);
    }
    #[test]
    fn a_new_move_order_interrupts_the_old_escape_manoeuvre() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let mut mover = Motion::new([200, 16, 300], RangeUnitKind::Tank, &body);
        mover.enabled = true;
        mover.destination = Some([70, 300]);
        mover.escape_left = mover.navigation.escape_ticks;
        mover.escape = 2;
        assert!(mover.move_to(&w, [430, 300], &body, &[], &nav::vehicle::Rubble::new()));
        assert_eq!(mover.escape_left, 0, "old escape delayed the new order");
        assert_eq!(mover.escape, 0, "old recovery cycle survived retargeting");
        for tick in 0..30 {
            step(&mut mover, &w, &body, &[], tick);
        }
        assert!(mover.pos[0] > Fx::from_int(201));
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
                        m.move_to(&w, [end, 300], &body, &[], &nav::vehicle::Rubble::new()),
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
