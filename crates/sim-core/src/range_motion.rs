//! Mobile actors on the forest range. Shared drive and suspension, integer
//! route planning against the current destructible voxel world.
use crate::drive::{self, Drive, Limits};
use crate::TICKS_PER_SECOND as TICKS;
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

/// A local continuation computed against the pre-commit obstacle snapshot.
/// The original motion/search stays untouched until its blockers are validated.
pub(super) struct PreparedLocalHunt {
    origin: [Fx; 2],
    shape: nav::vehicle::Shape,
    blockers: Vec<Obstacle>,
    checked: Vec<HuntClearance>,
    result: HuntAdvance,
}
struct HuntClearance {
    position: [Fx; 2],
    head: i32,
    clear: bool,
}
/// Exact current-pose query plus its warm probe. The source borrows prevent
/// edits to terrain/rubble until the enclosing motion phase consumes it.
pub(super) struct PreparedGround<'a> {
    world: &'a World,
    rubble: &'a nav::vehicle::Rubble,
    position: [Fx; 2],
    head: i32,
    shape: nav::vehicle::Shape,
    current: nav::vehicle::Pose,
    probe: nav::vehicle::Probe<'a>,
}
/// A whole-map search in flight (ADR-680) and the ticks it has waited
/// for its answer: the range serves the oldest first, ties by slot.
#[derive(Clone, Debug)]
pub(super) struct CoarsePending {
    search: nav::vehicle::coarse::Search,
    waited: u32,
}
enum CoarseAdvance {
    Pending,
    Planned(nav::vehicle::Route, [Fx; 2]),
    Refused,
}
enum HuntAdvance {
    Pending(Box<nav::vehicle::Hunt>),
    Restart,
    Planned(nav::vehicle::Route, [Fx; 2]),
}
/// Inputs from accepted hull motion. Feet use the previous tick for suspension,
/// then advance against the tick's immutable terrain before contacts or felling.
#[derive(Clone, Copy, Debug)]
struct FeetStep {
    linear: V3,
    angular: i32,
    cycles: Fx,
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
        let mut depth = Fx::MAX;
        for n in a.iter().chain(&b) {
            let ra = dot(*n, a[0])
                .abs()
                .mul(self.half[0])
                .add(dot(*n, a[1]).abs().mul(self.half[1]));
            let rb = dot(*n, b[0])
                .abs()
                .mul(other.half[0])
                .add(dot(*n, b[1]).abs().mul(other.half[1]));
            let overlap = ra.add(rb).sub(dot(delta, *n).abs());
            // Zero is the minimum possible answer. Most pairs are far
            // apart; a separating axis makes the other three unnecessary.
            if overlap <= Fx::ZERO {
                return Fx::ZERO;
            }
            depth = depth.min(overlap);
        }
        depth
    }
    fn overlaps(&self, other: &Self) -> bool {
        self.penetration(other) > Fx::ZERO
    }
    fn within(&self, from: [Fx; 2], radius: Fx) -> bool {
        let reach = radius.add(self.half[0].max(self.half[1]));
        self.pos[0].sub(from[0]).abs() <= reach && self.pos[2].sub(from[1]).abs() <= reach
    }
}
/// Manoeuvres in one escape cycle: astern, ahead, then each turning both ways.
const ESCAPES: u32 = 6;
// One spider edge probes a much wider footprint than a tank edge. The shared
// node allowance is therefore not a shared time allowance in a large battle.
const SPIDER_SEARCH_SLICE: usize = 32;
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
    /// Maximum graph work per tick for this hull's footprint, derived from
    /// its kind and therefore restored with the body rather than serialized.
    search_slice_limit: usize,
    /// Reconstructed from the unit kind, never a mutable search state.
    coarse_local_grade: bool,
    /// Nodes expanded and poses probed by the last plan: diagnostics, not
    /// state.
    pub last_plan: [u32; 2],
    /// Columns scanned and surface samples taken by it (P-29).
    pub last_probe: [u64; 2],
    /// A route search still walking its frontier (ADR-306). An existing
    /// route remains usable while its replacement is pending; every step
    /// still checks the current terrain and hulls.
    hunt: Option<Box<nav::vehicle::Hunt>>,
    /// A whole-map search still walking under the range's shared work
    /// allowance (ADR-680). Like `hunt`, it keeps the current route usable
    /// while it runs. Saved by the range in its own snapshot section.
    coarse: Option<Box<CoarsePending>>,
    /// Coarse work this hull may spend in the current call; `None` runs a
    /// coarse search to its end at once (a hull stepped outside a range).
    /// Scratch set by the range before each call; never saved.
    coarse_budget: Option<u64>,
    /// Coarse work spent since the range last set the allowance. Scratch.
    coarse_spent: u64,
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
    /// A completed refusal needs recovery: either no start exit, or an
    /// exhausted reachable pocket. Saved in the former stranded flag.
    needs_recovery: bool,
    recovery_head: i32,
    recovery_origin: [Fx; 2],
    next_plan: u32,
    /// The range hands out permission to begin a search each tick
    /// (ADR-312); `started` reports back whether this hull used it. Both
    /// are scratch for the tick, not state: the range sets them before
    /// every step, and a hull stepped by anything else may always begin.
    may_start: bool,
    /// Per-tick continuation allowance assigned by the range. Scratch,
    /// recomputed before motion after loading; standalone callers use tuning.
    resume_budget: Option<usize>,
    /// This tick's validated continuation was run before spatially ordered
    /// motion. Never serialized; the enclosing tick is atomic.
    prepared_hunt: bool,
    /// Pure terrain answers carried between ticks; changed pages are dropped
    /// before each use. Scratch only, never serialized.
    terrain_columns: nav::vehicle::Columns,
    /// Transient cap supplied by the range's fragment room before each tick.
    /// Standalone motion retains the complete cut list.
    pub(crate) cut_limit: usize,
    /// Scratch inside one atomic range tick; never saved or hashed.
    defer_feet: bool,
    pending_feet: Option<FeetStep>,
    started: bool,
}
/// Fair continuation shares in stable slot order. Rotate the remainder so
/// even a total smaller than the number of hunts cannot starve a slot.
pub(super) fn resume_share(
    total: usize,
    cap: usize,
    count: usize,
    rank: usize,
    tick: u32,
) -> usize {
    if count == 0 {
        return 0;
    }
    let offset = (rank + count - tick as usize % count) % count;
    (total / count + usize::from(offset < total % count)).min(cap)
}
pub(super) fn spider_tuning() -> &'static modding::spider::Tuning {
    static T: std::sync::OnceLock<modding::spider::Tuning> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        modding::spider::Tuning::from_ron(include_str!("../../../data/spider.ron"))
            .expect("spider tuning")
    })
}
/// The stock packs every hull is built from, parsed once per process
/// (analysis 240, B2): four RON files per unit made a 60-unit layout parse
/// 240 files. Each is a pure function of a compiled-in constant, so the
/// cache is that constant, not state.
struct StockPacks {
    sim: crate::Tuning,
    course: modding::range::RangeCourse,
    navigation: modding::navigation::Navigation,
    integrity: modding::integrity::Integrity,
}
fn stock_packs() -> &'static StockPacks {
    static P: std::sync::OnceLock<StockPacks> = std::sync::OnceLock::new();
    P.get_or_init(|| StockPacks {
        sim: crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).expect("tuning"),
        course: modding::range::RangeLayout::from_ron(include_str!(
            "../../../data/range-cover.ron"
        ))
        .expect("range tuning")
        .course
        .expect("course tuning"),
        navigation: modding::navigation::Navigation::from_ron(include_str!(
            "../../../data/range-navigation.ron"
        ))
        .expect("navigation"),
        integrity: modding::integrity::Integrity::from_ron(include_str!(
            "../../../data/unit-integrity.ron"
        ))
        .expect("unit integrity tuning"),
    })
}
impl Motion {
    pub fn new(at: [i32; 3], kind: modding::range::RangeUnitKind, body: &UnitBody) -> Self {
        let packs = stock_packs();
        let t = &packs.sim;
        let chassis = match kind {
            modding::range::RangeUnitKind::Tank
            | modding::range::RangeUnitKind::Artillery
            | modding::range::RangeUnitKind::ArtilleryT2
            | modding::range::RangeUnitKind::ArtilleryTurretT2
            | modding::range::RangeUnitKind::TurretT1
            | modding::range::RangeUnitKind::TurretT2
            | modding::range::RangeUnitKind::Base => t.tracked,
            modding::range::RangeUnitKind::Spider => spider_tuning().chassis,
            _ => t.walking,
        };
        let r = crate::ride::ride_of(&chassis);
        let c = &packs.course;
        let navigation = packs.navigation;
        let yielding = navigation.fell.materials.to_vec();
        let rigs = crate::gait::rigs_of(body);
        let feet = vec![crate::gait::Foot::default(); rigs.len()];
        let integrity = &packs.integrity;
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
            speed: if kind.emplacement() {
                0
            } else if kind == modding::range::RangeUnitKind::Spider {
                spider_tuning().speed
            } else if matches!(
                kind,
                modding::range::RangeUnitKind::Tank
                    | modding::range::RangeUnitKind::Artillery
                    | modding::range::RangeUnitKind::ArtilleryT2
            ) {
                c.tank_speed
            } else {
                c.walker_speed
            },
            step: if kind.emplacement() {
                0
            } else if kind == modding::range::RangeUnitKind::Spider {
                spider_tuning().step
            } else if matches!(
                kind,
                modding::range::RangeUnitKind::Tank
                    | modding::range::RangeUnitKind::Artillery
                    | modding::range::RangeUnitKind::ArtilleryT2
            ) {
                c.step_height
            } else {
                c.walker_step_height
            },
            terrain_tilt_pct: if matches!(
                kind,
                modding::range::RangeUnitKind::Tank
                    | modding::range::RangeUnitKind::Artillery
                    | modding::range::RangeUnitKind::ArtilleryT2
            ) {
                c.tank_terrain_tilt_pct
            } else {
                c.walker_terrain_tilt_pct
            },
            terrain_approach_pct: if matches!(
                kind,
                modding::range::RangeUnitKind::Tank
                    | modding::range::RangeUnitKind::Artillery
                    | modding::range::RangeUnitKind::ArtilleryT2
            ) {
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
            search_slice_limit: if kind == modding::range::RangeUnitKind::Spider {
                SPIDER_SEARCH_SLICE
            } else {
                usize::MAX
            },
            coarse_local_grade: kind == modding::range::RangeUnitKind::Spider,
            yielding,
            blocked_ticks: 0,
            escape_left: 0,
            escape: 0,
            needs_recovery: false,
            recovery_head: 0,
            recovery_origin: [Fx::from_int(at[0]), Fx::from_int(at[2])],
            next_plan: 0,
            may_start: true,
            resume_budget: None,
            prepared_hunt: false,
            terrain_columns: nav::vehicle::Columns::default(),
            cut_limit: usize::MAX,
            defer_feet: false,
            pending_feet: None,
            started: false,
            last_plan: [0; 2],
            last_probe: [0; 2],
            hunt: None,
            coarse: None,
            coarse_budget: None,
            coarse_spent: 0,
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
        // Emplacements are anchored to the ground; damage still applies to
        // their voxels, but a shell must not shove the whole command base.
        if newton_s == 0 || self.speed == 0 {
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
        if self.speed != 0 {
            self.shove = self.shove.add(velocity);
        }
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
        let lim = response.limits(&Limits::of(&self.chassis, TICKS), true);
        let mut ground =
            nav::vehicle::Probe::around(w, &self.yielding, [self.pos[0], self.pos[2]], 96)
                .with_rubble(rubble);
        let shape = self.shape(b);
        let current = ground.pose([self.pos[0], self.pos[2]], self.drive.head, shape);
        let nx = self.pos[0].add(self.shove.x.div(Fx::from_int(TICKS)));
        let nz = self.pos[2].add(self.shove.z.div(Fx::from_int(TICKS)));
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
        // What yields to a shoved hull falls as it does to a driven one
        // (ADR-452): slid into standing trunks, the hull stood in them
        // until it next drove.
        if self.rigs.is_empty()
            && ((allowed && target.crush > 0) || (!allowed && current.crush > 0))
        {
            self.cut = ground.crushed_limit(
                [self.pos[0], self.pos[2]],
                self.drive.head,
                shape,
                self.cut_limit,
            );
        }
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
    pub(super) fn can_drive(&self, response: crate::mobility::Mobility) -> bool {
        response.engine > Fx::ZERO && response.traction > Fx::ZERO && self.carried()
    }
    /// Installed propulsion can try to recover a stance. Buckled legs do not
    /// permanently remove this ability; lost legs and severed machinery do.
    pub(super) fn can_reposition(&self, response: crate::mobility::Mobility) -> bool {
        response.engine > Fx::ZERO
            && response.traction > Fx::ZERO
            && (self.rigs.is_empty() || self.legs_lost.iter().filter(|lost| !**lost).count() >= 2)
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
            Fx::from_ratio(1, TICKS),
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
        // Passive motion does not receive the per-tick changed-page set.
        // Its last driven answers must not survive edits made while parked.
        self.terrain_columns = nav::vehicle::Columns::default();
        // Nothing drives a parked hull (ADR-434): its position already
        // stops advancing, and a speed kept from the last driven tick
        // led the aim at a standing target, went into contacts and
        // spilled pieces, and came back at once when driving resumed.
        self.drive.vel = Fx::ZERO;
        self.drive.yaw_rate = 0;
        let moved = self.slide(w, rubble, b, others, response);
        self.settle(w, rubble, b, gravity, response);
        self.step_feet(w, rubble, b, moved, 0, Fx::ZERO);
    }
    /// Only the enclosing range can defer feet: it joins every job before
    /// changing terrain, applying contacts, observing poses or saving state.
    pub(super) fn defer_feet(&mut self) {
        debug_assert!(!self.defer_feet && self.pending_feet.is_none());
        self.defer_feet = true;
    }
    pub(super) fn finish_feet(&mut self, w: &World, rubble: &nav::vehicle::Rubble, b: &UnitBody) {
        self.defer_feet = false;
        if let Some(step) = self.pending_feet.take() {
            self.step_feet(w, rubble, b, step.linear, step.angular, step.cycles);
        }
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
        if self.defer_feet {
            debug_assert!(self.pending_feet.is_none());
            self.pending_feet = Some(FeetStep {
                linear,
                angular,
                cycles,
            });
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
        let ground = std::cell::RefCell::new(nav::vehicle::Probe::for_search(
            w,
            &self.yielding,
            Default::default(),
        ));
        let floor = |x: Fx, z: Fx| {
            let (x, z) = (x.to_int_trunc(), z.to_int_trunc());
            let top = rubble.get(&[x, z]).map_or(0, |t| i32::from(*t));
            Fx::from_int(ground.borrow_mut().floor(x, z).max(top))
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
        self.cut_feet(w, b);
    }
    /// Yielding vegetation touches a walker's actual feet, including a
    /// swinging foot, rather than the empty rectangle between its legs.
    fn cut_feet(&mut self, w: &World, b: &UnitBody) {
        if self.rigs.is_empty() {
            return;
        }
        self.cut.clear();
        let root = self.frame(b);
        let mut frames = crate::damage::segment_frames(&root, b, self.drive.head, self.drive.head);
        let mut cells = std::collections::BTreeSet::new();
        for (leg, rig) in self.rigs.iter().enumerate() {
            if self.legs_lost[leg] {
                continue;
            }
            let (Some(part), Some(contact)) = (rig.foot, self.feet[leg].contact.as_ref()) else {
                continue;
            };
            crate::gait::leg_frames(rig, &root, contact, &mut frames);
            let foot = &b.segments[part].body;
            let frame = &frames[part];
            let dims = [foot.vol.nx, foot.vol.ny, foot.vol.nz].map(i32::from);
            let mut lo = [i32::MAX; 3];
            let mut hi = [i32::MIN; 3];
            for corner in 0..8 {
                let p = frame.world(
                    [0, 1, 2]
                        .map(|a| Fx::from_int(if corner & (1 << a) == 0 { 0 } else { dims[a] })),
                );
                for (a, v) in [p.x, p.y, p.z].into_iter().enumerate() {
                    lo[a] = lo[a].min((v.raw() >> 32) as i32);
                    hi[a] = hi[a].max((v.raw() >> 32) as i32);
                }
            }
            for z in lo[2].max(0)..=hi[2].min(w.nz as i32 - 1) {
                for x in lo[0].max(0)..=hi[0].min(w.nx as i32 - 1) {
                    // Above the column's ceiling there is only air
                    // (ADR-313): most of a foot's box asks nothing.
                    let top = w.column_ceiling(x as u32, z as u32) as i32 - 1;
                    for y in lo[1].max(0)..=hi[1].min(w.ny as i32 - 1).min(top) {
                        let p = [x as u32, y as u32, z as u32];
                        if !self.yielding.contains(&w.material(p[0], p[1], p[2])) {
                            continue;
                        }
                        let local = frame.local(V3::new(
                            Fx::from_int(x).add(Fx::from_ratio(1, 2)),
                            Fx::from_int(y).add(Fx::from_ratio(1, 2)),
                            Fx::from_int(z).add(Fx::from_ratio(1, 2)),
                        ));
                        if (0..3).all(|a| local[a] >= Fx::ZERO && local[a] < Fx::from_int(dims[a]))
                        {
                            let q = local.map(|v| v.to_int_trunc() as usize);
                            if foot.vol.get(q[0], q[1], q[2]) {
                                cells.insert(p);
                            }
                        }
                    }
                }
            }
        }
        // Same column order and claim quota as the tracked hull's cut.
        let mut cells: Vec<_> = cells.into_iter().collect();
        cells.sort_unstable_by_key(|p| (p[2], p[0], p[1]));
        let mut claimed = 0;
        for p in cells {
            if !w.on_frame(p[0], p[1], p[2]) {
                if claimed == self.cut_limit {
                    break;
                }
                claimed += 1;
            }
            self.cut.push(p);
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
        self.move_to_observed(w, to, b, others, rubble, None, |_| {})
    }
    #[allow(clippy::too_many_arguments)]
    fn move_to_observed(
        &mut self,
        w: &World,
        to: [i32; 2],
        b: &UnitBody,
        others: &[Obstacle],
        rubble: &nav::vehicle::Rubble,
        prepared: Option<&PreparedGround<'_>>,
        completed: impl FnMut(&'static str),
    ) -> bool {
        // An explicit order supersedes the old route, even to the same point.
        self.path.clear();
        self.partial_path = false;
        self.cursor = 0;
        self.plan_to_observed(w, to, b, others, rubble, prepared, completed)
    }
    /// Accept a group order without starting every hull's route search in the
    /// input frame. The normal motion scheduler starts these searches under
    /// `searches_started_per_tick`, using the world as it is on that tick.
    pub fn queue_move(&mut self, w: &World, to: [i32; 2]) -> bool {
        let far = [w.nx as i32 - EDGE, w.nz as i32 - EDGE];
        if !(0..2).all(|a| (EDGE..far[a]).contains(&to[a])) {
            return false;
        }
        self.route.clear();
        self.path.clear();
        self.partial_path = false;
        self.cursor = 0;
        self.hunt = None;
        self.coarse = None;
        self.destination = Some(to);
        self.enabled = true;
        self.blocked = false;
        self.needs_recovery = false;
        self.blocked_ticks = 0;
        self.escape = 0;
        self.escape_left = 0;
        self.next_plan = 0;
        true
    }
    /// Only nearby hulls matter to a contact detour. A distant actor is
    /// handled if and when the moving hull reaches it, not by this search.
    fn local_blockers(
        &self,
        from: [Fx; 2],
        shape: nav::vehicle::Shape,
        others: &[Obstacle],
    ) -> Vec<Obstacle> {
        if self.blocked_ticks == 0 && self.escape == 0 {
            return Vec::new();
        }
        let radius =
            Fx::from_int(4 * (shape.half[0].max(shape.half[1]) + self.navigation.cell_units));
        others
            .iter()
            .copied()
            .filter(|o| o.within(from, radius))
            .collect()
    }
    /// Replanning retains the current route while a replacement is pending.
    /// A temporary blocker can leave before the search finishes; execution
    /// may then resume under the same live collision checks as any move.
    #[cfg(test)]
    fn plan_to(
        &mut self,
        w: &World,
        to: [i32; 2],
        b: &UnitBody,
        others: &[Obstacle],
        rubble: &nav::vehicle::Rubble,
    ) -> bool {
        self.plan_to_observed(w, to, b, others, rubble, None, |_| {})
    }
    #[allow(clippy::too_many_arguments)]
    fn plan_to_observed(
        &mut self,
        w: &World,
        to: [i32; 2],
        b: &UnitBody,
        others: &[Obstacle],
        rubble: &nav::vehicle::Rubble,
        prepared: Option<&PreparedGround<'_>>,
        mut completed: impl FnMut(&'static str),
    ) -> bool {
        let from = [self.pos[0], self.pos[2]];
        let shape = self.shape(b);
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
        // A new request supersedes any search of the old one, fine or
        // whole-map (ADR-680): the hull owns at most one of each.
        self.hunt = None;
        self.coarse = None;
        // Global routing is terrain-only. A hull enters dynamic avoidance
        // only after physical contact or an escape attempt on this order.
        let avoid = self.blocked_ticks > 0 || self.escape > 0;
        if !self.blocked
            && !self.needs_recovery
            && !avoid
            && self.route.is_empty()
            // Finish a checked local bypass before returning to the global map.
            && (!self.partial_path || self.cursor >= self.path.len())
            && self.destination.is_some()
            && (0..2).any(|a| {
                from[a].sub(Fx::from_int(to[a])).abs()
                    >= Fx::from_int(self.navigation.global_min_distance)
            })
        {
            // The whole-map search walks under the range's shared work
            // allowance (ADR-680). An answer within this call's share is
            // taken at once; otherwise the order stands, the current route
            // stays usable, and the range continues the search next tick.
            if let Ok(search) = nav::vehicle::coarse::Search::new(
                [w.nx, w.nz],
                shape,
                from,
                to,
                self.coarse_config(),
            ) {
                let pending = Box::new(CoarsePending { search, waited: 0 });
                match self.walk_coarse(pending, w, rubble) {
                    CoarseAdvance::Pending => {
                        completed("plan_coarse");
                        self.blocked = false;
                        self.needs_recovery = false;
                        self.dry = false;
                        return true;
                    }
                    CoarseAdvance::Planned(plan, _) => {
                        completed("plan_coarse");
                        self.take_plan(plan);
                        return true;
                    }
                    CoarseAdvance::Refused => {}
                }
            }
            completed("plan_coarse");
        }
        self.start_fine(w, to, b, others, rubble, prepared, completed)
    }
    /// The configuration of this hull's whole-map searches.
    pub(crate) fn coarse_config(&self) -> nav::vehicle::coarse::Config {
        nav::vehicle::coarse::Config {
            cell: self.navigation.global_cell_units,
            nodes: self.navigation.global_nodes,
            yielding: self.navigation.fell.materials,
            local_grade: self.coarse_local_grade,
            restarts: self.navigation.global_restarts,
        }
    }
    /// Walk a whole-map search with what is left of this call's allowance.
    fn walk_coarse(
        &mut self,
        mut pending: Box<CoarsePending>,
        w: &World,
        rubble: &nav::vehicle::Rubble,
    ) -> CoarseAdvance {
        let budget = self.coarse_budget.unwrap_or(u64::MAX);
        let left = budget.saturating_sub(self.coarse_spent);
        if left == 0 {
            self.coarse = Some(pending);
            return CoarseAdvance::Pending;
        }
        let (answer, spent) = pending.search.advance(w, rubble, left);
        self.coarse_spent = self.coarse_spent.saturating_add(spent);
        match answer {
            None => {
                self.coarse = Some(pending);
                CoarseAdvance::Pending
            }
            Some(Ok(plan)) => CoarseAdvance::Planned(plan, pending.search.origin()),
            Some(Err(_)) => CoarseAdvance::Refused,
        }
    }
    /// Begin the oriented fine search toward `to` from where the hull
    /// stands: the start of every short order, and of a long one the
    /// whole-map planner refused.
    #[allow(clippy::too_many_arguments)]
    fn start_fine(
        &mut self,
        w: &World,
        to: [i32; 2],
        b: &UnitBody,
        others: &[Obstacle],
        rubble: &nav::vehicle::Rubble,
        prepared: Option<&PreparedGround<'_>>,
        mut completed: impl FnMut(&'static str),
    ) -> bool {
        let from = [self.pos[0], self.pos[2]];
        let shape = self.shape(b);
        let half = shape.half.map(Fx::from_int);
        let blockers = self.local_blockers(from, shape, others);
        let clear = |p: [Fx; 2], head: i32| {
            !blockers.iter().any(|o| {
                o.overlaps(&Obstacle {
                    pos: [p[0], Fx::ZERO, p[1]],
                    head,
                    half,
                })
            })
        };
        // Prepared motion geometry already invalidated changed pages against
        // this tick's immutable sources. Share its exact column answers, not
        // its pose: route contacts can use a different approach margin.
        let columns = prepared
            .filter(|p| {
                std::ptr::eq(p.world, w)
                    && std::ptr::eq(p.rubble, rubble)
                    && p.probe.matches_yielding(&self.yielding)
            })
            .map(|p| p.probe.fork_route().take_columns())
            .unwrap_or_default();
        let mut ground =
            nav::vehicle::Probe::for_search(w, &self.yielding, columns).with_rubble(rubble);
        let budget = self
            .navigation
            .search_nodes_per_tick
            .min(self.search_slice_limit)
            .max(1);
        let mut prepare = |request: nav::vehicle::StartSweep,
                           ground: &mut nav::vehicle::Probe<'_>| {
            use rayon::prelude::*;
            let shape = request.shape;
            let samples = &request.samples;
            let prefix = ground.route_prefix(shape, &samples[..4], |p| request.prefix_open(p));
            if !request.prefix_open(&prefix) {
                return nav::vehicle::PreparedSweep {
                    poses: samples[..4].iter().copied().zip(prefix).collect(),
                    ..Default::default()
                };
            }
            let batches: Vec<_> = samples[4..]
                .par_chunks(4)
                .with_max_len(1)
                .map(|batch| {
                    let mut probe = ground.fork_route();
                    let answers = probe.route_samples(shape, batch);
                    let reads = [probe.scans, probe.samples];
                    (
                        batch.iter().copied().zip(answers).collect::<Vec<_>>(),
                        probe.take_columns(),
                        reads,
                    )
                })
                .collect();
            let mut poses: Vec<_> = samples[..4].iter().copied().zip(prefix).collect();
            let mut columns = None;
            let mut reads = [0u64; 2];
            for (answers, cache, work) in batches {
                poses.extend(answers);
                columns = Some(cache);
                for a in 0..2 {
                    reads[a] = reads[a].wrapping_add(work[a]);
                }
            }
            nav::vehicle::PreparedSweep {
                poses,
                columns,
                reads,
            }
        };
        let prepare: Option<&mut nav::vehicle::PrepareStart<'_>> =
            (shape.half.iter().all(|&n| n >= 40) && rayon::current_num_threads() >= 4)
                .then_some(&mut prepare);
        match nav::vehicle::Hunt::start_bounded_sweep_observed(
            &mut ground,
            shape,
            from,
            self.drive.head,
            to,
            self.search(w),
            &clear,
            budget,
            prepare,
            &mut completed,
        ) {
            nav::vehicle::Hunting::Done(plan) => self.take_plan(plan),
            nav::vehicle::Hunting::Open(hunt) => {
                // Starting consumed this tick's bounded share. Goal probes,
                // the direct sweep and frontier resume on the next tick.
                self.hunt = Some(hunt);
                self.blocked = false;
                self.needs_recovery = false;
                self.dry = false;
            }
        }
        completed("plan_fine");
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
        self.needs_recovery = plan.stranded || self.dry;
        self.last_plan = [plan.nodes, plan.poses];
        self.last_probe = [plan.scans, plan.samples];
        self.partial_path = plan.partial;
        self.path = plan.path;
        self.cursor = 0;
    }
    /// A route searched from `from` taken up by a hull that moved on while
    /// the search ran (ADR-462): a turn point near the search's start that
    /// the hull has already passed, being past it along the leg that
    /// leaves it, is skipped, so the hull does not turn back for it. Only
    /// those within the reach a search holds for: farther on, and the
    /// route's end, are followed as planned.
    fn take_up(&mut self, from: [Fx; 2]) {
        let here = [self.pos[0], self.pos[2]];
        if here == from {
            return;
        }
        let reach = Fx::from_int(self.navigation.retarget_units);
        let point = |p: [i32; 2]| p.map(Fx::from_int);
        while self.cursor + 1 < self.path.len() {
            let (turn, next) = (
                point(self.path[self.cursor]),
                point(self.path[self.cursor + 1]),
            );
            let near = [turn[0].sub(from[0]), turn[1].sub(from[1])];
            let passed = [here[0].sub(turn[0]), here[1].sub(turn[1])];
            let leg = [next[0].sub(turn[0]), next[1].sub(turn[1])];
            if near[0].mul(near[0]).add(near[1].mul(near[1])) > reach.mul(reach)
                || passed[0].mul(leg[0]).add(passed[1].mul(leg[1])) <= Fx::ZERO
            {
                break;
            }
            self.cursor += 1;
        }
    }
    /// A new goal for a hull already under way (ADR-462): it keeps driving
    /// its route while the new search runs, and takes the new route up
    /// from where it has got to. A hull with no route left, blocked or
    /// escaping is ordered as by [`Motion::move_to`].
    pub fn retarget(
        &mut self,
        w: &World,
        to: [i32; 2],
        b: &UnitBody,
        others: &[Obstacle],
        rubble: &nav::vehicle::Rubble,
    ) -> bool {
        self.retarget_observed(w, to, b, others, rubble, |_| {})
    }
    /// The same immediate pursuit order, with observations of route startup.
    /// Callbacks never enter the search inputs or determine its allowance.
    pub(super) fn retarget_observed(
        &mut self,
        w: &World,
        to: [i32; 2],
        b: &UnitBody,
        others: &[Obstacle],
        rubble: &nav::vehicle::Rubble,
        completed: impl FnMut(&'static str),
    ) -> bool {
        if self.cursor >= self.path.len() || self.blocked || self.escape_left > 0 {
            return self.move_to_observed(w, to, b, others, rubble, None, completed);
        }
        self.plan_to_observed(w, to, b, others, rubble, None, completed)
    }
    /// Whether a route search is still walking its frontier (ADR-306),
    /// fine or whole-map (ADR-680).
    pub fn searching(&self) -> bool {
        self.hunt.is_some() || self.coarse.is_some()
    }
    /// Whether a fine search is in flight: what continuation shares count.
    pub(super) fn hunting(&self) -> bool {
        self.hunt.is_some()
    }
    /// Ticks a whole-map search in flight has waited, if there is one.
    pub fn coarse_waited(&self) -> Option<u32> {
        self.coarse.as_ref().map(|c| c.waited)
    }
    /// The most one corridor test of the search in flight can cost.
    pub(super) fn coarse_max_test(&self) -> u64 {
        self.coarse.as_ref().map_or(0, |c| c.search.max_test())
    }
    /// Work the whole-map search in flight has spent, if there is one.
    pub fn coarse_work(&self) -> Option<u64> {
        self.coarse.as_ref().map(|c| c.search.spent())
    }
    /// The coarse work this hull may spend in the coming call (ADR-680);
    /// `None` runs a whole-map search to its end at once.
    pub fn allow_coarse(&mut self, budget: Option<u64>) {
        self.coarse_budget = budget;
        self.coarse_spent = 0;
    }
    /// Coarse work spent since the allowance was last set.
    pub fn coarse_spent(&self) -> u64 {
        self.coarse_spent
    }
    /// The range's continuation of a whole-map search in flight (ADR-680):
    /// note what changed in the world since the last tick, walk on with
    /// `budget`, and take the answer up. Returns the work spent. The
    /// first call of a tick counts the tick as waited; a second round of
    /// the range's leftover passes no changes and counts nothing.
    /// The terrain-only planner needs hull obstacles only on a fine fallback.
    /// Build them at most once from this phase's frozen positions.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn continue_coarse(
        &mut self,
        w: &World,
        b: &UnitBody,
        others: impl FnOnce() -> Vec<Obstacle>,
        rubble: &nav::vehicle::Rubble,
        tick: u32,
        changed: &nav::vehicle::Changed,
        budget: u64,
        first: bool,
        full_tick_budget: bool,
    ) -> u64 {
        let Some(mut pending) = self.coarse.take() else {
            return 0;
        };
        pending.search.note(changed);
        // As for the fine search (ADR-462): the search holds while the hull
        // drives on or brakes within `retarget_units` of where it began. An
        // escape, or a hull carried farther, drops it; the order stands and
        // asks again under the start allowance.
        let from = pending.search.origin();
        let (dx, dz) = (self.pos[0].sub(from[0]), self.pos[2].sub(from[1]));
        let reach = Fx::from_int(self.navigation.retarget_units);
        if self.escape_left > 0 || dx.mul(dx).add(dz.mul(dz)) > reach.mul(reach) {
            self.next_plan = tick;
            return 0;
        }
        let to = pending.search.goal();
        self.allow_coarse(Some(budget));
        let result = self.walk_coarse(pending, w, rubble);
        let spent = self.coarse_spent;
        self.allow_coarse(None);
        match result {
            CoarseAdvance::Pending if spent == 0 && budget > 0 && full_tick_budget => {
                // Its next atomic corridor test cannot fit even in an
                // otherwise empty tick. Keep the order and hand it to the
                // existing fine planner instead of overspending or waiting
                // forever. The range supplies this flag: its active budget
                // can differ from this motion's cloned navigation settings.
                self.coarse = None;
                self.start_fine(w, to, b, &others(), rubble, None, |_| {});
            }
            CoarseAdvance::Pending if first => {
                if let Some(c) = &mut self.coarse {
                    c.waited = c.waited.saturating_add(1);
                }
            }
            CoarseAdvance::Pending => {}
            // The cadence of the next try is the one the request set: a
            // start in the hull's own step counted it from the start, an
            // order left it as it was, as with the one-call planner.
            CoarseAdvance::Planned(plan, from) => {
                self.take_plan(plan);
                self.take_up(from);
            }
            CoarseAdvance::Refused => {
                // A refusal hands the order to the fine planner, exactly as
                // the one-call planner did.
                self.start_fine(w, to, b, &others(), rubble, None, |_| {});
            }
        }
        spent
    }
    /// The whole-map search in flight as bytes (ADR-680), for the range's
    /// snapshot section; nothing when there is none.
    pub(super) fn save_coarse(&self, b: &mut Blob) {
        if let Some(c) = &self.coarse {
            b.u32(c.waited);
            c.search.save(b);
        }
    }
    /// Read a whole-map search in flight back; `false` when the bytes do
    /// not describe one this hull could own.
    pub(super) fn load_coarse(&mut self, s: &mut Scan, size: [u32; 2]) -> bool {
        let waited = s.u32();
        let Some(search) = nav::vehicle::coarse::Search::load(s, size) else {
            return false;
        };
        if !self.enabled || self.destination != Some(search.goal()) {
            return false;
        }
        self.coarse = Some(Box::new(CoarsePending { search, waited }));
        true
    }
    /// A refused route owns the hull until its bounded escape sequence ends.
    /// Changing the goal meanwhile resets the attempt counter and prevents
    /// the unit from ever finishing a manoeuvre on difficult terrain.
    pub fn recovering(&self) -> bool {
        self.escape_left > 0 || (self.blocked && self.escape < ESCAPES)
    }
    /// Diagnostic state for movement traces: attempts begun and ticks left
    /// in the current manoeuvre.
    pub fn recovery_state(&self) -> [u32; 2] {
        [self.escape, self.escape_left]
    }
    /// Cells the search in flight has expanded so far; zero when none is
    /// in flight or when it has not walked yet (ADR-314).
    pub fn frontier_nodes(&self) -> u32 {
        self.hunt.as_ref().map_or(0, |h| h.nodes())
    }
    /// One tick's worth of the search in flight, if there is one. A world
    /// that changed under it drops what it scanned, not the search.
    #[allow(clippy::too_many_arguments)]
    fn hunt_step(
        &mut self,
        w: &World,
        b: &UnitBody,
        others: &[Obstacle],
        rubble: &nav::vehicle::Rubble,
        tick: u32,
        changed: &nav::vehicle::Changed,
        detail: impl Fn(&'static str),
        sweep_detail: impl Fn(usize, bool) + Sync,
    ) {
        let Some(hunt) = self.hunt.take() else {
            return;
        };
        let blockers = self.local_blockers(hunt.origin(), self.shape(b), others);
        let result = self.advance_hunt_mode::<true>(
            hunt,
            w,
            b,
            &blockers,
            rubble,
            changed,
            None,
            detail,
            sweep_detail,
        );
        self.apply_hunt(result, tick);
    }
    /// Pure continuation over owned search scratch and immutable surroundings.
    /// Both speculative and serial paths use the same calculation and commit.
    #[allow(clippy::too_many_arguments)]
    fn advance_hunt_mode<const LOCAL: bool>(
        &self,
        mut hunt: Box<nav::vehicle::Hunt>,
        w: &World,
        b: &UnitBody,
        blockers: &[Obstacle],
        rubble: &nav::vehicle::Rubble,
        changed: &nav::vehicle::Changed,
        checked: Option<&std::cell::RefCell<Vec<HuntClearance>>>,
        detail: impl Fn(&'static str),
        sweep_detail: impl Fn(usize, bool) + Sync,
    ) -> HuntAdvance {
        let slice = self
            .resume_budget
            .unwrap_or(self.navigation.search_nodes_per_tick);
        if slice == 0 {
            // Changes must still invalidate cached terrain during a skipped slice.
            detail("route_forget_begin");
            hunt.forget(changed);
            detail("route_forget_end");
            return HuntAdvance::Pending(hunt);
        }
        if !hunt.starts_at([self.pos[0], self.pos[2]], self.drive.head) {
            // A hull driving on its route or braking while the new search
            // runs moves by design: the search holds while the hull is
            // within `retarget_units` of where it began, and the new route
            // is taken up from where it has got to (ADR-462). Restarting at
            // every displacement dropped a search sliced over ticks on
            // each tick of a braking hull: a start on every tick of the
            // braking and a full stop for every new goal of a pursuit. An
            // escape manoeuvre, or a hull carried further, still starts
            // over: retain the order and restart through the normal shared
            // allowance after the escape.
            let from = hunt.origin();
            let (dx, dz) = (self.pos[0].sub(from[0]), self.pos[2].sub(from[1]));
            let reach = Fx::from_int(self.navigation.retarget_units);
            if self.escape_left > 0 || dx.mul(dx).add(dz.mul(dz)) > reach.mul(reach) {
                return HuntAdvance::Restart;
            }
        }
        detail("route_forget_begin");
        hunt.forget(changed);
        detail("route_forget_end");
        let half = self.shape(b).half.map(Fx::from_int);
        let is_clear = |p: [Fx; 2], head: i32| {
            !blockers.iter().any(|o| {
                o.overlaps(&Obstacle {
                    pos: [p[0], Fx::ZERO, p[1]],
                    head,
                    half,
                })
            })
        };
        let clear = |p: [Fx; 2], head: i32| {
            let answer = is_clear(p, head);
            if let Some(checked) = checked {
                checked.borrow_mut().push(HuntClearance {
                    position: p,
                    head,
                    clear: answer,
                });
            }
            answer
        };
        detail("route_query_begin");
        let prepared = if (LOCAL || checked.is_none() && blockers.is_empty())
            && self.shape(b).half.iter().all(|&n| n >= 40)
            && rayon::current_thread_index().is_some()
            && rayon::current_num_threads() >= 4
        {
            hunt.sweep_samples(slice)
                // Nearby hulls are immutable for this speculative continuation.
                // A blocked prefix would discard the wide terrain preparation;
                // test it without recording callbacks ahead of the actual search.
                .filter(|(_, samples)| {
                    blockers.is_empty() || samples[..4].iter().all(|&(p, head)| is_clear(p, head))
                })
                .map(|(shape, samples)| {
                    use rayon::prelude::*;
                    let mut seed = hunt.sweep_probe(w, rubble);
                    let prefix =
                        seed.route_prefix(shape, &samples[..4], |p| hunt.sweep_prefix_open(p));
                    if !hunt.sweep_prefix_open(&prefix) {
                        return nav::vehicle::PreparedSweep {
                            poses: samples[..4].iter().copied().zip(prefix).collect(),
                            reads: [seed.scans, seed.samples],
                            columns: Some(seed.take_columns()),
                        };
                    }
                    let poses: Vec<_> = samples[4..]
                        .par_chunks(8)
                        .enumerate()
                        .with_max_len(1)
                        .map(|(id, batch)| {
                            sweep_detail(id, true);
                            let mut ground = seed.fork_route();
                            let poses = ground.route_samples(shape, batch);
                            let reads = [ground.scans, ground.samples];
                            let columns = ground.take_columns();
                            sweep_detail(id, false);
                            (poses, columns, reads)
                        })
                        .collect();
                    let mut answers = Vec::with_capacity(samples.len());
                    answers.extend(prefix);
                    let mut reads = [seed.scans, seed.samples];
                    let mut columns = None;
                    for (poses, cache, work) in poses {
                        answers.extend(poses);
                        columns = Some(cache);
                        for a in 0..2 {
                            reads[a] = reads[a].wrapping_add(work[a]);
                        }
                    }
                    nav::vehicle::PreparedSweep {
                        poses: samples.into_iter().zip(answers).collect(),
                        columns,
                        reads,
                    }
                })
                .unwrap_or_default()
        } else {
            nav::vehicle::PreparedSweep::default()
        };
        let plan = hunt.advance_in_prepared(w, rubble, &clear, slice, prepared);
        detail("route_query_end");
        match plan {
            Some(plan) => HuntAdvance::Planned(plan, hunt.origin()),
            None => HuntAdvance::Pending(hunt),
        }
    }
    fn apply_hunt(&mut self, result: HuntAdvance, tick: u32) {
        match result {
            HuntAdvance::Planned(plan, from) => {
                self.hunt = None;
                // The cadence of the next try counts from the answer, not
                // from the question (ADR-306).
                self.take_plan(plan);
                self.take_up(from);
                self.next_plan = tick.saturating_add(self.replan_after_search());
            }
            HuntAdvance::Pending(hunt) => self.hunt = Some(hunt),
            HuntAdvance::Restart => {
                self.hunt = None;
                self.next_plan = tick;
            }
        }
    }
    /// Continue a search that cannot read live blockers independently of the
    /// spatial motion commit. The caller may run these on a fixed worker pool.
    pub(super) fn prepare_terrain_hunt(
        &mut self,
        w: &World,
        b: &UnitBody,
        rubble: &nav::vehicle::Rubble,
        tick: u32,
        changed: &nav::vehicle::Changed,
        detail: impl Fn(&'static str) + Sync,
    ) {
        if self.enabled && self.hunt.is_some() && self.blocked_ticks == 0 && self.escape == 0 {
            detail("terrain_advance_begin");
            self.hunt_step(w, b, &[], rubble, tick, changed, &detail, |batch, begin| {
                detail(sweep_event(batch, begin));
            });
            self.prepared_hunt = true;
            detail("terrain_advance_end");
        }
    }
    #[cfg(test)]
    pub(super) fn prepare_local_hunt(
        &self,
        w: &World,
        b: &UnitBody,
        others: &[Obstacle],
        rubble: &nav::vehicle::Rubble,
        changed: &nav::vehicle::Changed,
    ) -> Option<PreparedLocalHunt> {
        self.prepare_local_hunt_observed(w, b, others, rubble, changed, |_| {})
    }
    /// Test after terrain continuation: it may already have completed the
    /// search. Inactive actors need no neighbour snapshot allocation.
    pub(super) fn needs_local_hunt(&self) -> bool {
        self.enabled
            && !self.prepared_hunt
            && self.resume_budget != Some(0)
            && (self.blocked_ticks != 0 || self.escape != 0)
            && self.hunt.is_some()
    }
    #[cfg(test)]
    pub(super) fn prepare_local_hunt_observed(
        &self,
        w: &World,
        b: &UnitBody,
        others: &[Obstacle],
        rubble: &nav::vehicle::Rubble,
        changed: &nav::vehicle::Changed,
        detail: impl Fn(&'static str),
    ) -> Option<PreparedLocalHunt> {
        self.prepare_local_hunt_tasks(w, b, others, rubble, changed, detail, |_, _| {})
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_local_hunt_tasks(
        &self,
        w: &World,
        b: &UnitBody,
        others: &[Obstacle],
        rubble: &nav::vehicle::Rubble,
        changed: &nav::vehicle::Changed,
        detail: impl Fn(&'static str),
        sweep_detail: impl Fn(usize, bool) + Sync,
    ) -> Option<PreparedLocalHunt> {
        if !self.needs_local_hunt() {
            return None;
        }
        let hunt = self.hunt.as_ref()?;
        let origin = hunt.origin();
        let shape = self.shape(b);
        let blockers = self.local_blockers(origin, shape, others);
        let checked = std::cell::RefCell::new(Vec::new());
        // Only the budgeted continuation's callback reads are retained. The
        // recorder is local to this worker; serial searches do not allocate it.
        detail("local_clone_begin");
        let candidate = hunt.clone();
        detail("local_clone_end");
        detail("local_advance_begin");
        let result = self.advance_hunt_mode::<true>(
            candidate,
            w,
            b,
            &blockers,
            rubble,
            changed,
            Some(&checked),
            &detail,
            sweep_detail,
        );
        detail("local_advance_end");
        Some(PreparedLocalHunt {
            origin,
            shape,
            blockers,
            checked: checked.into_inner(),
            result,
        })
    }
    /// Later actors observe earlier accepted positions. A moved blocker is
    /// harmless if every queried pose still has the same clearance answer.
    pub(super) fn accept_local_hunt(
        &mut self,
        prepared: PreparedLocalHunt,
        b: &UnitBody,
        others: &[Obstacle],
        tick: u32,
    ) -> bool {
        let shape = self.shape(b);
        if shape != prepared.shape {
            return false;
        }
        let blockers = self.local_blockers(prepared.origin, shape, others);
        let unchanged = blockers.len() == prepared.blockers.len()
            && blockers.iter().zip(&prepared.blockers).all(|(a, b)| {
                a.pos[0] == b.pos[0] && a.pos[2] == b.pos[2] && a.head == b.head && a.half == b.half
            });
        if !unchanged
            && !prepared.checked.iter().all(|q| {
                let clear = !blockers.iter().any(|o| {
                    o.overlaps(&Obstacle {
                        pos: [q.position[0], Fx::ZERO, q.position[1]],
                        head: q.head,
                        half: shape.half.map(Fx::from_int),
                    })
                });
                clear == q.clear
            })
        {
            return false;
        }
        // Identical callback answers reproduce every branch of the original
        // continuation. Terrain/rubble and this actor remain fixed in the phase;
        // neighbors can change without invalidating unread or harmless poses.
        self.apply_hunt(prepared.result, tick);
        self.prepared_hunt = true;
        true
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
    pub(super) fn allow_resume(&mut self, work: usize) {
        self.resume_budget = Some(work.min(self.search_slice_limit));
    }
    pub fn allow_start(&mut self, may: bool) {
        self.may_start = may;
        self.started = false;
    }
    pub fn started_search(&self) -> bool {
        self.started
    }
    /// Whether this tick's step may begin a new route search, read before
    /// the step. A superset of the start conditions in `step_observed_ground`:
    /// the range only uses it to choose who is offered the shared start
    /// allowance (analysis 240, C3), and an offer that is not taken is lost
    /// for this tick, never mis-spent.
    pub(super) fn wants_start(&self, tick: u32) -> bool {
        self.enabled
            && !self.searching()
            && self.escape_left == 0
            && ((self.destination.is_some()
                && (self.blocked || self.path.is_empty())
                && tick >= self.next_plan)
                || (self.cursor == self.path.len() && !self.route.is_empty() && !self.partial_path))
    }
    /// Override the global graph allowance (zero selects the oriented planner).
    pub fn set_global_search_limit(&mut self, nodes: usize) {
        self.navigation.global_nodes = nodes;
    }
    /// Fine planner budget and weight, for benches and tests; the data sets them.
    pub fn set_search(&mut self, nodes: usize, heuristic_pct: u32) {
        // Explicit fine-search diagnostics bypass the whole-map planner.
        self.navigation.global_nodes = 0;
        self.navigation.search_nodes = nodes;
        self.navigation.heuristic_pct = heuristic_pct;
    }
    pub fn stop(&mut self) {
        self.hunt = None;
        self.coarse = None;
        self.path.clear();
        self.partial_path = false;
        self.cursor = 0;
        self.destination = None;
        self.blocked = false;
        self.blocked_ticks = 0;
        self.escape_left = 0;
        self.escape = 0;
        self.needs_recovery = false;
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
        changed: &nav::vehicle::Changed,
    ) {
        self.step_observed(
            w,
            b,
            alive,
            engine,
            response,
            others,
            tick,
            gravity,
            rubble,
            changed,
            |_| {},
        );
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn step_observed(
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
        changed: &nav::vehicle::Changed,
        completed: impl FnMut(&'static str),
    ) {
        self.step_observed_ground(
            w, b, alive, engine, response, others, tick, gravity, rubble, changed, None, completed,
        );
    }
    pub(super) fn prepare_ground<'a>(
        &mut self,
        w: &'a World,
        b: &UnitBody,
        rubble: &'a nav::vehicle::Rubble,
        changed: &nav::vehicle::Changed,
    ) -> Option<PreparedGround<'a>> {
        if !self.enabled {
            return None;
        }
        let mut probe = nav::vehicle::Probe::for_local(
            w,
            &self.yielding,
            std::mem::take(&mut self.terrain_columns),
            changed,
        )
        .with_rubble(rubble);
        let position = [self.pos[0], self.pos[2]];
        let shape = self.shape(b);
        let current = probe.pose(position, self.drive.head, shape);
        Some(PreparedGround {
            world: w,
            rubble,
            position,
            head: self.drive.head,
            shape,
            current,
            probe,
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn step_observed_ground<'a>(
        &mut self,
        w: &'a World,
        b: &UnitBody,
        alive: bool,
        engine: bool,
        response: crate::mobility::Mobility,
        others: &[Obstacle],
        tick: u32,
        gravity: i32,
        rubble: &'a nav::vehicle::Rubble,
        changed: &nav::vehicle::Changed,
        prepared: Option<PreparedGround<'a>>,
        mut completed: impl FnMut(&'static str),
    ) {
        self.cut.clear();
        self.contact = None;
        if !self.enabled {
            // A parked hull that a burst pushed slides and rocks (ADR-286);
            // a parked walker still stands on its feet, and its hull on them.
            self.passive(w, rubble, b, gravity, response, others);
            return;
        }
        let engine = engine && self.can_drive(response);
        // A search begun on an earlier tick walks on before anything asks
        // for another (ADR-306).
        if !std::mem::take(&mut self.prepared_hunt) {
            self.hunt_step(w, b, others, rubble, tick, changed, |_| {}, |_, _| {});
        }
        completed("hunt");
        // Finish a checked prefix before searching its continuation. Replacing
        // it on the cadence from a moving source can lead back behind the hull
        // into the pocket it just left. A block or a new pursuit goal still
        // requests a replacement; real steps always check fresh geometry.
        if !self.searching()
            && self.escape_left == 0
            && (self.blocked || self.path.is_empty())
            && tick >= self.next_plan
            && self.may_start
        {
            if let Some(goal) = self.destination {
                self.started = true;
                self.plan_to_observed(
                    w,
                    goal,
                    b,
                    others,
                    rubble,
                    prepared.as_ref(),
                    &mut completed,
                );
                self.next_plan = tick.saturating_add(self.replan_after_search());
            }
        }
        if self.cursor == self.path.len()
            && !self.searching()
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
            self.move_to_observed(
                w,
                goal,
                b,
                others,
                rubble,
                prepared.as_ref(),
                &mut completed,
            );
            if self.dry {
                self.next_plan = tick.saturating_add(self.replan_after_search());
            }
        }
        completed("plan");
        let mut want = drive::NO_WAY;
        let mut speed = Fx::ZERO;
        let base_limits = Limits::of(&self.chassis, TICKS);
        let drive_factor = self.drive_scale.mul(self.hull.load);
        let brake = response.limits(&base_limits, false).brake.mul(drive_factor);
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
                if reached_end {
                    // A pending replacement must not revive a completed
                    // order or overwrite the next prefix/patrol request.
                    self.hunt = None;
                    self.coarse = None;
                }
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
                    .mul(response.traction);
                let alignment = angle::cos(angle::diff(want, self.drive.head)).max(Fx::ZERO);
                // Keep the existing conservative axis-distance bound on
                // diagonals. Braking includes this tick and the full tail.
                let distance = dx.abs().max(dz.abs());
                speed = drive::stopping_speed(distance, brake, crate::TICKS_PER_SECOND, speed);
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
            && !self.searching()
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
        // Retain bounded column pages across nearby ticks. The world's edits
        // and rubble changes invalidate precisely the pages they touched.
        let shape = self.shape(b);
        let (mut ground, current) = match prepared {
            Some(p)
                if std::ptr::eq(p.world, w)
                    && std::ptr::eq(p.rubble, rubble)
                    && p.position == [self.pos[0], self.pos[2]]
                    && p.head == old.head
                    && p.shape == shape
                    && p.probe.matches_yielding(&self.yielding) =>
            {
                completed("ground_accepted");
                (p.probe, p.current)
            }
            other => {
                let columns = match other {
                    Some(mut p) => {
                        completed("ground_rejected");
                        // Pages are still exact source-world answers. Only a
                        // matching source can seed the ordinary current query.
                        if std::ptr::eq(p.world, w)
                            && std::ptr::eq(p.rubble, rubble)
                            && p.probe.matches_yielding(&self.yielding)
                        {
                            p.probe.take_columns()
                        } else {
                            Default::default()
                        }
                    }
                    None => std::mem::take(&mut self.terrain_columns),
                };
                let mut ground =
                    nav::vehicle::Probe::for_local(w, &self.yielding, columns, changed)
                        .with_rubble(rubble);
                let current = ground.pose([self.pos[0], self.pos[2]], old.head, shape);
                (ground, current)
            }
        };
        // Pushing through a tree is slower than open ground.
        if current.crush > 0 {
            speed = speed.mul(Fx::from_ratio(self.navigation.fell.speed_pct, 100));
        }
        let (next, delta) = drive::advance(
            old,
            want,
            speed,
            &lim,
            drive_factor,
            Fx::from_ratio(1, TICKS),
        );
        let previous = Obstacle::of(self, b);
        self.drive = next;
        // A burst's shove rides along with the drive's own step (ADR-286)
        // and is spent by the brakes, or by the terrain if it refuses.
        let nx = self.pos[0]
            .add(delta.dx)
            .add(self.shove.x.div(Fx::from_int(TICKS)));
        let nz = self.pos[2]
            .add(delta.dz)
            .add(self.shove.z.div(Fx::from_int(TICKS)));
        let (x, z) = (nx.to_int_trunc(), nz.to_int_trunc());
        // This probe only lives for the current tick. A stationary hull
        // has the same clearance answer, but still gets fresh support,
        // gravity and contact processing below.
        let target = if nx == self.pos[0] && nz == self.pos[2] && next.head == old.head {
            current
        } else {
            ground.pose([nx, nz], next.head, shape)
        };
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
        let belly_clear = target.blocked == 0
            || target.blocked < current.blocked
            || (self.escape_left > 0
                && target.blocked == current.blocked
                && ground.keeps_belly_contacts(
                    ([self.pos[0], self.pos[2]], old.head, current),
                    ([nx, nz], next.head, target),
                    shape,
                ));
        let clear =
            belly_clear && !target.tilt && (self.escape_left > 0 || !target.steep || current.steep);
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
            next.vel.div(Fx::from_int(TICKS))
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
                        delta.dx.mul(Fx::from_int(TICKS)),
                        Fx::ZERO,
                        delta.dz.mul(Fx::from_int(TICKS)),
                    );
                    self.record_contact(index, &others[index], drive.add(self.shove));
                }
            } else {
                self.shove = V3::ZERO;
            }
            Fx::ZERO
        };
        self.brake_shove(&lim);
        if self.rigs.is_empty()
            && ((allowed && target.crush > 0) || (!allowed && current.crush > 0))
        {
            self.cut = ground.crushed_limit(
                [self.pos[0], self.pos[2]],
                self.drive.head,
                shape,
                self.cut_limit,
            );
        }
        // A dry search is a block even if its tiny reachable pocket contains
        // several cells. Only recovery can change the next search start.
        // A search still walking is not a block: the hull is waiting.
        if self.needs_recovery
            && !self.searching()
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
        completed("terrain");
        let mut advanced = 0;
        if let Some(c) = drive::StepCycle::of(&self.chassis) {
            advanced = c.advance(path, angle::diff(self.drive.head, old.head));
            self.phase = angle::norm(self.phase.wrapping_add(advanced));
        }
        let nominal = crate::ride::supports_of(&self.chassis);
        let (hc, hs) = crate::ride::heading_of(self.drive.head);
        let f = ground.support_floors([self.pos[0], self.pos[2]], self.drive.head, shape);
        self.terrain_columns = ground.take_columns();
        let floors = [f[3], f[1], f[2], f[0]];
        self.floors = floors;
        // A walker stands on the legs it still has (ADR-280); a tracked
        // hull on all four nominal points over the probe's floors.
        let (support, ground) = self
            .foot_supports(&nominal, &floors)
            .unwrap_or_else(|| (nominal.to_vec(), floors.to_vec()));
        let r = self.ride_now(b, gravity, response);
        let sway = crate::ride::sway_of(
            self.drive.vel.sub(old.vel).mul(Fx::from_int(TICKS)),
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
            Fx::from_ratio(1, TICKS),
        );
        self.pos[1] = self
            .hull
            .y
            .sub(crate::ride::ride_of(&self.chassis).clearance);
        completed("ride");
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
        completed("feet");
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
        b.bool(self.needs_recovery);
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
        self.dry = false;
        self.prepared_hunt = false;
        // A whole-map search in flight is the range's own snapshot section
        // (ADR-680), read after every hull.
        self.coarse = None;
        self.allow_coarse(None);
        self.terrain_columns = nav::vehicle::Columns::default();
        self.cut_limit = usize::MAX;
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
        self.needs_recovery = s.bool();
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
#[path = "range_coarse_budget_tests.rs"]
mod coarse_budget_tests;

#[cfg(test)]
#[path = "range_coarse_obstacle_tests.rs"]
mod coarse_obstacle_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use modding::range::RangeUnitKind;
    #[test]
    fn command_base_is_anchored_under_contact_and_blast_impulse() {
        let body = voxel::unit::command_base();
        let mut motion = Motion::new([100, 16, 100], RangeUnitKind::Base, &body);
        assert_eq!(motion.speed, 0);
        motion.receive_contact(V3::new(Fx::from_int(50), Fx::ZERO, Fx::from_int(40)));
        motion.impulse_through(&body, [90, 16, 100], [95, 16, 100], 1_000_000);
        assert_eq!(motion.shove, V3::ZERO);
        assert_eq!(motion.hull.vy, Fx::ZERO);
        assert_eq!(motion.pos.map(Fx::to_int_trunc), [100, 16, 100]);
    }
    struct Ground {
        depth: i32,
    }
    /// A search holds while its hull stays within `retarget_units` of
    /// where it began, turned in place or shoved by a contact, and starts
    /// over from the actual pose once the hull is carried farther (ADR-399,
    /// ADR-462). Either way the order is kept and reached, and a restored
    /// copy replays byte for byte.
    #[test]
    fn a_displaced_search_holds_within_reach_and_restarts_past_it() {
        let world = ground(0);
        let body = voxel::unit::tank();
        let goal = [430, 100];
        let rubble = nav::vehicle::Rubble::new();
        for displacement in 0..3 {
            let mut motion = Motion::new([60, 16, 100], RangeUnitKind::Tank, &body);
            motion.navigation.global_nodes = 0; // Exercise the resumable fine planner.
            motion.navigation.search_nodes_per_tick = 8;
            assert!(motion.move_to(&world, goal, &body, &[], &rubble));
            assert!(motion.searching(), "fixture needs an unfinished search");
            let reach = motion.navigation.retarget_units;
            match displacement {
                0 => motion.pos[2] = Fx::from_int(100 + reach + 1),
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
                if displacement == 0 {
                    assert!(!m.searching(), "search still uses the previous pose");
                } else {
                    assert!(m.searching(), "a search within reach started over");
                }
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
            let following = motion.partial_path
                && motion.cursor < motion.path.len()
                && !motion.blocked
                && !motion.searching()
                && motion.escape_left == 0;
            motion.allow_start(true);
            step(&mut motion, &world, &body, &[], tick);
            assert!(
                !following || !motion.started_search(),
                "usable prefix started a speculative replacement at tick {tick}"
            );
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
    #[test]
    fn search_shares_are_bounded_fair_and_borrow_idle_capacity() {
        assert_eq!(resume_share(768, 192, 1, 0, 0), 192);
        assert_eq!(resume_share(768, 192, 16, 0, 0), 48);
        for count in 1..=17 {
            for total in [1, 7, 96, 768] {
                let mut granted = vec![0; count];
                for tick in 0..count as u32 {
                    let shares: Vec<_> = (0..count)
                        .map(|rank| resume_share(total, 192, count, rank, tick))
                        .collect();
                    assert!(shares.iter().sum::<usize>() <= total);
                    assert!(shares.iter().all(|&work| work <= 192));
                    for (sum, work) in granted.iter_mut().zip(shares) {
                        *sum += work;
                    }
                }
                assert!(granted.iter().all(|&work| work > 0), "starved: {granted:?}");
                assert!(granted.windows(2).all(|pair| pair[0] == pair[1]));
            }
        }
    }

    fn ground(depth: i32) -> World {
        World::new(640, 64, 640, 100, Box::new(Ground { depth }))
    }

    #[test]
    fn distant_route_ignores_units_until_a_live_contact() {
        let w = ground(0);
        w.prepare_navigation([4, 5]);
        let body = voxel::unit::tank();
        let goal = [500, 300];
        let blocker = Motion::new([280, 16, 300], RangeUnitKind::Tank, &body);
        let mut empty = Motion::new([90, 16, 300], RangeUnitKind::Tank, &body);
        let mut occupied = empty.clone();
        let rubble = nav::vehicle::Rubble::new();
        assert!(empty.move_to(&w, goal, &body, &[], &rubble));
        assert!(occupied.move_to(&w, goal, &body, &[Obstacle::of(&blocker, &body)], &rubble));
        assert!(!occupied.searching());
        assert_eq!(occupied.path, empty.path);
        assert_eq!(occupied.path.last(), Some(&goal));
    }

    #[test]
    fn walkers_cut_only_occupied_feet_and_keep_trees_between_legs() {
        struct Forest;
        impl voxel::Generator for Forest {
            fn material(&self, _x: u32, y: u32, _z: u32) -> u16 {
                if y < 16 {
                    1
                } else if y < 42 {
                    4
                } else {
                    0
                }
            }
        }
        let w = World::new(320, 96, 320, 100, Box::new(Forest));
        for kind in [RangeUnitKind::Strider, RangeUnitKind::Spider] {
            for head in [0, 730] {
                let intact_body = match kind {
                    RangeUnitKind::Strider => voxel::unit::strider(),
                    _ => voxel::unit::spider(),
                };
                let mut body = intact_body.clone();
                let mut m = Motion::new([160, 16, 160], kind, &body);
                m.drive.head = head;
                m.enabled = false;
                m.passive(
                    &w,
                    &nav::vehicle::Rubble::new(),
                    &body,
                    98,
                    crate::mobility::Mobility::intact(true),
                    &[],
                );
                assert!(!m.cut.is_empty(), "feet did not touch any trees");
                // Strider feet return under the hull; Spider feet stand much
                // farther out. Check the central gap in hull coordinates,
                // not an axis-aligned world square after turning the hull.
                let empty = Fx::from_int(if kind == RangeUnitKind::Spider { 24 } else { 4 });
                let (c, s) = (angle::cos(head), angle::sin(head));
                assert!(
                    m.cut.iter().all(|p| {
                        let dx = Fx::from_int(p[0] as i32 - 160).add(Fx::from_ratio(1, 2));
                        let dz = Fx::from_int(p[2] as i32 - 160).add(Fx::from_ratio(1, 2));
                        dx.mul(c).add(dz.mul(s)).abs() > empty
                            || dz.mul(c).sub(dx.mul(s)).abs() > empty
                    }),
                    "{kind:?} head={head}: empty space below the hull cut trees"
                );
                assert!(m.cut.iter().all(|p| p[1] < 24), "feet stood on tree crowns");
                let all = m.cut.clone();
                m.cut_limit = 7;
                m.cut_feet(&w, &body);
                assert_eq!(
                    m.cut,
                    all[..7],
                    "{kind:?} head={head}: contact prefix order"
                );
                m.cut_limit = 0;
                m.cut_feet(&w, &body);
                assert!(m.cut.is_empty());
                m.cut_limit = usize::MAX;
                m.legs_lost[0] = true;
                m.cut_feet(&w, &body);
                assert!(
                    !m.cut.is_empty(),
                    "{kind:?}: remaining feet stopped cutting"
                );
                assert!(m.cut.len() < all.len(), "{kind:?}: lost foot still cuts");
                m.legs_lost[0] = false;
                let foot_parts: Vec<_> = m.rigs.iter().filter_map(|r| r.foot).collect();
                for i in foot_parts {
                    let foot = &mut body.segments[i].body;
                    for z in 0..foot.vol.nz as usize {
                        for y in 0..foot.vol.ny as usize {
                            for x in 0..foot.vol.nx as usize {
                                foot.remove(x, y, z);
                            }
                        }
                    }
                }
                m.cut_feet(&w, &body);
                assert!(m.cut.is_empty(), "destroyed feet still fell trees");
                m.legs_lost.fill(true);
                m.cut_feet(&w, &intact_body);
                assert!(m.cut.is_empty(), "missing legs still fell trees");
            }
        }
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
    /// **A shoved hull fells what it slides into** (ADR-452), as a driven
    /// one does; slid into standing trunks, it stood in them until it
    /// next drove.
    #[test]
    fn a_shoved_tank_fells_the_trees_it_slides_into() {
        struct Grove;
        impl voxel::Generator for Grove {
            fn material(&self, x: u32, y: u32, _z: u32) -> u16 {
                if y < 16 {
                    1
                } else if (100..106).contains(&x) && y < 42 {
                    4
                } else {
                    0
                }
            }
        }
        let w = World::new(256, 64, 256, 100, Box::new(Grove));
        let body = voxel::unit::tank();
        let mut m = Motion::new([60, 16, 128], RangeUnitKind::Tank, &body);
        let mut felled = false;
        for _ in 0..60 {
            m.shove = V3::new(Fx::from_int(120), Fx::ZERO, Fx::ZERO);
            m.cut.clear();
            m.slide(
                &w,
                &nav::vehicle::Rubble::new(),
                &body,
                &[],
                crate::mobility::Mobility::intact(true),
            );
            felled |= m.cut.iter().any(|&[x, y, z]| w.material(x, y, z) == 4);
            if felled {
                break;
            }
        }
        assert!(
            felled,
            "slid into the grove without felling it: {:?}",
            m.pos
        );
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
            &nav::vehicle::Changed::default(),
        );
    }
    fn local_hunter(w: &World, b: &UnitBody, others: &[Obstacle]) -> Motion {
        let mut m = Motion::new([200, 16, 300], RangeUnitKind::Tank, b);
        m.navigation.global_nodes = 0;
        m.navigation.search_nodes_per_tick = 1;
        m.blocked_ticks = 1;
        assert!(m.move_to(w, [430, 300], b, others, &nav::vehicle::Rubble::new()));
        m.blocked_ticks = 1;
        assert!(m.searching());
        m
    }
    fn motion_bytes(m: &Motion) -> Vec<u8> {
        let mut blob = Blob::new();
        m.save(&mut blob);
        blob.v
    }
    #[test]
    fn fresh_spider_search_matches_serial_across_workers_and_live_blockers() {
        let body = voxel::unit::spider();
        let serial_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        for workers in [2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            for depth in [0, 9] {
                let w = ground(depth);
                let rubble = nav::vehicle::Rubble::from_sorted(vec![([215, 300], 24)]);
                for goal in [[500, 300], [200, 530], [80, 80]] {
                    for blocked in [false, true] {
                        let mut serial = Motion::new([180, 16, 300], RangeUnitKind::Spider, &body);
                        serial.navigation.global_nodes = 0;
                        serial.navigation.search_nodes_per_tick = 32;
                        serial.blocked_ticks = u32::from(blocked);
                        let mut prepared = serial.clone();
                        let others = [Obstacle {
                            pos: [Fx::from_int(240), Fx::from_int(16), Fx::from_int(300)],
                            head: angle::FULL / 7,
                            half: [Fx::from_int(30); 2],
                        }];
                        let a = serial_pool
                            .install(|| serial.move_to(&w, goal, &body, &others, &rubble));
                        let b =
                            pool.install(|| prepared.move_to(&w, goal, &body, &others, &rubble));
                        assert_eq!(a, b);
                        assert_eq!(
                            motion_bytes(&serial),
                            motion_bytes(&prepared),
                            "workers={workers} depth={depth} goal={goal:?} blocked={blocked}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn fresh_search_reuses_only_matching_prepared_sources_across_workers() {
        let w = ground(9);
        let other_world = ground(0);
        let rubble = nav::vehicle::Rubble::new();
        let other_rubble = nav::vehicle::Rubble::from([([330, 300], 80)]);
        let changed = nav::vehicle::Changed::default();
        let body = voxel::unit::spider();
        let mut reused = false;
        for workers in [1, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            for source in 0..4 {
                for goal in [[323, 300], [450, 300], [320, 480], [90, 80]] {
                    let mut seeded = Motion::new([320, 16, 300], RangeUnitKind::Spider, &body);
                    seeded.enabled = true;
                    seeded.navigation.global_nodes = 0;
                    seeded.navigation.search_nodes_per_tick = 32;
                    let prepared = seeded
                        .prepare_ground(
                            if source == 1 { &other_world } else { &w },
                            &body,
                            if source == 2 { &other_rubble } else { &rubble },
                            &changed,
                        )
                        .unwrap();
                    if source == 3 {
                        seeded.yielding.clear();
                    }
                    let mut cold = seeded.clone();
                    let a = pool.install(|| cold.move_to(&w, goal, &body, &[], &rubble));
                    let b = pool.install(|| {
                        seeded.move_to_observed(
                            &w,
                            goal,
                            &body,
                            &[],
                            &rubble,
                            Some(&prepared),
                            |_| {},
                        )
                    });
                    assert_eq!(a, b);
                    assert_eq!(
                        motion_bytes(&cold),
                        motion_bytes(&seeded),
                        "workers={workers} source={source} goal={goal:?}"
                    );
                    if source == 0 {
                        reused |= seeded.last_probe[0] < cold.last_probe[0];
                    } else {
                        assert_eq!(
                            cold.last_probe, seeded.last_probe,
                            "wrong source seeded search"
                        );
                    }
                }
            }
        }
        assert!(reused, "matching source did not reduce terrain reads");
    }
    #[test]
    fn prepared_ground_and_deferred_feet_match_serial_motion_across_workers() {
        use rayon::prelude::*;
        let w = ground(0);
        let bodies = [
            voxel::unit::tank(),
            voxel::unit::strider(),
            voxel::unit::spider(),
            voxel::unit::artillery(),
        ];
        let kinds = [
            RangeUnitKind::Tank,
            RangeUnitKind::Strider,
            RangeUnitKind::Spider,
            RangeUnitKind::Artillery,
        ];
        let changed = nav::vehicle::Changed::default();
        for workers in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            let mut rubble = nav::vehicle::Rubble::new();
            let mut serial: Vec<_> = bodies
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    let mut m = Motion::new([160, 16, 180 + i as i32 * 100], kinds[i], b);
                    m.enabled = true;
                    m.path = vec![[500, 180 + i as i32 * 100]];
                    m.destination = m.path.last().copied();
                    m.next_plan = u32::MAX;
                    m
                })
                .collect();
            let mut prepared = serial.clone();
            let mut contacts = 0;
            for tick in 0..60 {
                let mut changed = changed.clone();
                for group in [&mut serial, &mut prepared] {
                    if tick == 15 {
                        group[1].legs_buckled[1] = true;
                        group[2].legs_buckled[1] = true;
                    }
                    if tick == 25 {
                        group[1].legs_lost[0] = true;
                        group[2].legs_lost[0] = true;
                    }
                    if tick == 45 {
                        for m in group.iter_mut() {
                            m.enabled = false;
                            m.shove = V3::new(Fx::from_int(12), Fx::ZERO, Fx::ZERO);
                        }
                    }
                }
                if tick >= 30 {
                    rubble = nav::vehicle::Rubble::from_sorted(vec![([170, 280], 18)]);
                    changed.column(170, 280);
                }
                let mut answers: Vec<_> = pool.install(|| {
                    prepared
                        .par_iter_mut()
                        .enumerate()
                        .map(|(i, m)| m.prepare_ground(&w, &bodies[i], &rubble, &changed))
                        .collect()
                });
                // Live blockers are supplied after preparation, just as spatial
                // acceptance sees earlier actors' moves in the enclosing tick.
                let others = [Obstacle {
                    pos: [
                        Fx::from_int(190 + (tick / 20) as i32),
                        Fx::from_int(16),
                        Fx::from_int(180),
                    ],
                    head: 0,
                    half: [Fx::from_int(12), Fx::from_int(18)],
                }];
                for i in 0..bodies.len() {
                    serial[i].allow_start(false);
                    prepared[i].allow_start(false);
                    prepared[i].defer_feet();
                    serial[i].step(
                        &w,
                        &bodies[i],
                        true,
                        true,
                        crate::mobility::Mobility::intact(true),
                        &others,
                        tick,
                        98,
                        &rubble,
                        &changed,
                    );
                    let mut accepted = false;
                    prepared[i].step_observed_ground(
                        &w,
                        &bodies[i],
                        true,
                        true,
                        crate::mobility::Mobility::intact(true),
                        &others,
                        tick,
                        98,
                        &rubble,
                        &changed,
                        answers[i].take(),
                        |stage| accepted |= stage == "ground_accepted",
                    );
                    assert!(accepted || !prepared[i].enabled);
                    // Articulation does not change the blocker later hulls see.
                    let before = Obstacle::of(&prepared[i], &bodies[i]);
                    let expected = Obstacle::of(&serial[i], &bodies[i]);
                    assert_eq!(before.pos, expected.pos);
                    assert_eq!(before.head, expected.head);
                    assert_eq!(before.half, expected.half);
                }
                drop(answers);
                pool.install(|| {
                    prepared.par_iter_mut().enumerate().for_each(|(i, m)| {
                        m.finish_feet(&w, &rubble, &bodies[i]);
                    });
                });
                for i in 0..bodies.len() {
                    assert!(!prepared[i].defer_feet && prepared[i].pending_feet.is_none());
                    assert_eq!(
                        motion_bytes(&prepared[i]),
                        motion_bytes(&serial[i]),
                        "workers={workers} tick={tick} unit={i}"
                    );
                    assert_eq!(prepared[i].floors, serial[i].floors);
                    assert_eq!(prepared[i].cut, serial[i].cut);
                    assert_eq!(prepared[i].contact, serial[i].contact);
                    contacts += usize::from(prepared[i].contact.is_some());
                }
            }
            assert!(contacts > 0, "fixture must exercise live contacts");
        }
    }
    #[test]
    fn changed_ground_key_or_source_falls_back_to_exact_probe() {
        let w = ground(0);
        let other_world = ground(5);
        let b = voxel::unit::tank();
        let rubble = nav::vehicle::Rubble::new();
        let other_rubble = nav::vehicle::Rubble::from_sorted(vec![([200, 300], 18)]);
        let changed = nav::vehicle::Changed::default();
        for case in 0..6 {
            let mut m = Motion::new([200, 16, 300], RangeUnitKind::Tank, &b);
            m.enabled = true;
            let answer = m.prepare_ground(&w, &b, &rubble, &changed);
            match case {
                0 => m.pos[0] = m.pos[0].add(Fx::ONE),
                1 => m.drive.head = angle::FULL / 8,
                2 => m.step += 1,
                5 => m.yielding = vec![1, 2],
                _ => {}
            }
            let mut serial = m.clone();
            let world = if case == 3 { &other_world } else { &w };
            let rubble = if case == 4 { &other_rubble } else { &rubble };
            let mut rejected = false;
            m.step_observed_ground(
                world,
                &b,
                true,
                true,
                crate::mobility::Mobility::intact(true),
                &[],
                0,
                98,
                rubble,
                &changed,
                answer,
                |stage| rejected |= stage == "ground_rejected",
            );
            serial.step(
                world,
                &b,
                true,
                true,
                crate::mobility::Mobility::intact(true),
                &[],
                0,
                98,
                rubble,
                &changed,
            );
            assert!(rejected, "case={case}");
            assert_eq!(motion_bytes(&m), motion_bytes(&serial), "case={case}");
            assert_eq!(m.cut, serial.cut);
        }
    }
    #[test]
    fn prepared_local_hunt_matches_serial_continuation() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let rubble = nav::vehicle::Rubble::new();
        let changed = nav::vehicle::Changed::default();
        let obstacle = Obstacle {
            pos: [Fx::from_int(240), Fx::from_int(16), Fx::from_int(300)],
            head: 0,
            half: [Fx::from_int(6), Fx::from_int(10)],
        };
        let mut serial = local_hunter(&w, &body, &[obstacle]);
        let mut prepared = serial.clone();
        let mut accepted = 0;
        for tick in 0..200 {
            let events = std::cell::RefCell::new(Vec::new());
            if let Some(result) = prepared.prepare_local_hunt_observed(
                &w,
                &body,
                &[obstacle],
                &rubble,
                &changed,
                |event| events.borrow_mut().push(event),
            ) {
                assert_eq!(
                    *events.borrow(),
                    [
                        "local_clone_begin",
                        "local_clone_end",
                        "local_advance_begin",
                        "route_forget_begin",
                        "route_forget_end",
                        "route_query_begin",
                        "route_query_end",
                        "local_advance_end"
                    ]
                );
                prepared.allow_start(false);
                prepared.cut_limit = 7;
                assert!(prepared.accept_local_hunt(result, &body, &[obstacle], tick));
                assert!(!prepared.may_start);
                assert_eq!(prepared.cut_limit, 7);
                accepted += 1;
            } else {
                assert!(events.borrow().is_empty());
            }
            serial.allow_start(false);
            prepared.allow_start(false);
            step(&mut serial, &w, &body, &[obstacle], tick);
            step(&mut prepared, &w, &body, &[obstacle], tick);
            assert_eq!(
                motion_bytes(&serial),
                motion_bytes(&prepared),
                "tick={tick}"
            );
        }
        assert!(accepted > 0);
    }
    #[test]
    fn changed_local_blockers_preserve_exact_serial_continuation() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let rubble = nav::vehicle::Rubble::new();
        let changed = nav::vehicle::Changed::default();
        let obstacle = Obstacle {
            pos: [Fx::from_int(240), Fx::from_int(16), Fx::from_int(300)],
            head: 0,
            half: [Fx::from_int(6), Fx::from_int(10)],
        };
        let mut moved = obstacle;
        moved.pos[0] = moved.pos[0].add(Fx::ONE);
        let mut rotated = obstacle;
        rotated.head = angle::FULL / 8;
        let mut larger = obstacle;
        larger.half[0] = larger.half[0].add(Fx::ONE);
        for (before, after) in [
            (vec![obstacle], vec![moved]),
            (vec![obstacle], vec![rotated]),
            (vec![obstacle], vec![larger]),
            (vec![obstacle], vec![]),
            (vec![], vec![obstacle]),
        ] {
            let mut m = local_hunter(&w, &body, &before);
            let mut serial = m.clone();
            let original = motion_bytes(&m);
            let prepared = m
                .prepare_local_hunt(&w, &body, &before, &rubble, &changed)
                .unwrap();
            if !m.accept_local_hunt(prepared, &body, &after, 0) {
                assert_eq!(motion_bytes(&m), original);
            }
            step(&mut m, &w, &body, &after, 0);
            step(&mut serial, &w, &body, &after, 0);
            assert_eq!(motion_bytes(&m), motion_bytes(&serial));
        }
    }
    #[test]
    fn local_hunt_rejects_both_clearance_changes_without_mutating_search() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let rubble = nav::vehicle::Rubble::new();
        let changed = nav::vehicle::Changed::default();
        let mut source = local_hunter(&w, &body, &[]);
        source.resume_budget = Some(64);
        let free = source
            .prepare_local_hunt(&w, &body, &[], &rubble, &changed)
            .unwrap();
        let query = free.checked.iter().find(|q| q.clear).unwrap();
        let obstacle = Obstacle {
            pos: [query.position[0], Fx::from_int(16), query.position[1]],
            head: query.head,
            half: source.shape(&body).half.map(Fx::from_int),
        };
        assert_eq!(
            source
                .local_blockers(free.origin, free.shape, &[obstacle])
                .len(),
            1
        );
        let blocked = source
            .prepare_local_hunt(&w, &body, &[obstacle], &rubble, &changed)
            .unwrap();
        assert!(blocked.checked.iter().any(|q| !q.clear));
        for (prepared, after) in [(free, vec![obstacle]), (blocked, vec![])] {
            let mut m = source.clone();
            let mut serial = source.clone();
            let original = motion_bytes(&m);
            assert!(!m.accept_local_hunt(prepared, &body, &after, 0));
            assert_eq!(motion_bytes(&m), original);
            m.allow_start(false);
            serial.allow_start(false);
            step(&mut m, &w, &body, &after, 0);
            step(&mut serial, &w, &body, &after, 0);
            assert_eq!(motion_bytes(&m), motion_bytes(&serial));
        }
    }
    #[test]
    fn local_hunt_accepts_changed_neighbors_that_do_not_change_queries() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let rubble = nav::vehicle::Rubble::new();
        let changed = nav::vehicle::Changed::default();
        let obstacle = Obstacle {
            pos: [Fx::from_int(160), Fx::from_int(16), Fx::from_int(350)],
            head: 0,
            half: [Fx::from_int(6), Fx::from_int(10)],
        };
        let mut moved = obstacle;
        moved.pos[0] = moved.pos[0].sub(Fx::ONE);
        let mut rotated = obstacle;
        rotated.head = angle::FULL / 8;
        let mut larger = obstacle;
        larger.half[0] = larger.half[0].add(Fx::ONE);
        for (before, after) in [
            (vec![obstacle], vec![moved]),
            (vec![obstacle], vec![rotated]),
            (vec![obstacle], vec![larger]),
            (vec![obstacle], vec![]),
            (vec![], vec![obstacle]),
            (vec![obstacle, moved], vec![moved, obstacle]),
        ] {
            let mut m = local_hunter(&w, &body, &before);
            m.resume_budget = Some(64);
            let mut serial = m.clone();
            let prepared = m
                .prepare_local_hunt(&w, &body, &before, &rubble, &changed)
                .unwrap();
            assert!(!prepared.checked.is_empty());
            assert!(prepared.checked.iter().all(|q| q.clear));
            assert_eq!(
                m.local_blockers(prepared.origin, prepared.shape, &before)
                    .len(),
                before.len()
            );
            assert_eq!(
                m.local_blockers(prepared.origin, prepared.shape, &after)
                    .len(),
                after.len()
            );
            assert!(m.accept_local_hunt(prepared, &body, &after, 0));
            m.allow_start(false);
            serial.allow_start(false);
            step(&mut m, &w, &body, &after, 0);
            step(&mut serial, &w, &body, &after, 0);
            assert_eq!(motion_bytes(&m), motion_bytes(&serial));
            assert_eq!(m.cut, serial.cut);
            assert_eq!(m.contact, serial.contact);
        }
    }
    #[test]
    fn local_hunt_rejects_changed_own_shape() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let mut m = local_hunter(&w, &body, &[]);
        let prepared = m
            .prepare_local_hunt(
                &w,
                &body,
                &[],
                &nav::vehicle::Rubble::new(),
                &nav::vehicle::Changed::default(),
            )
            .unwrap();
        m.step += 1;
        let original = motion_bytes(&m);
        assert!(!m.accept_local_hunt(prepared, &body, &[], 0));
        assert_eq!(motion_bytes(&m), original);
    }
    #[test]
    fn pooled_local_hunts_validate_live_queries_across_worker_counts() {
        use rayon::prelude::*;
        let w = ground(0);
        let body = voxel::unit::tank();
        let rubble = nav::vehicle::Rubble::new();
        let changed = nav::vehicle::Changed::default();
        let obstacle = Obstacle {
            pos: [Fx::from_int(160), Fx::from_int(16), Fx::from_int(350)],
            head: 0,
            half: [Fx::from_int(6), Fx::from_int(10)],
        };
        for workers in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            let mut prepared: Vec<_> = (0..12)
                .map(|_| {
                    let mut m = local_hunter(&w, &body, &[obstacle]);
                    m.resume_budget = Some(64);
                    m
                })
                .collect();
            let mut serial = prepared.clone();
            let answers: Vec<_> = pool.install(|| {
                prepared
                    .par_iter()
                    .map(|m| {
                        m.prepare_local_hunt(&w, &body, &[obstacle], &rubble, &changed)
                            .unwrap()
                    })
                    .collect()
            });
            let mut accepted = 0;
            let mut rejected = 0;
            for (i, answer) in answers.into_iter().enumerate() {
                let mut moved = obstacle;
                moved.pos[0] = moved.pos[0].sub(Fx::from_int(i as i32 + 1));
                moved.head = angle::FULL / 8;
                let others = match i % 3 {
                    0 => vec![moved],
                    1 => {
                        let q = answer.checked.iter().find(|q| q.clear).unwrap();
                        vec![
                            moved,
                            Obstacle {
                                pos: [q.position[0], Fx::from_int(16), q.position[1]],
                                head: q.head,
                                half: answer.shape.half.map(Fx::from_int),
                            },
                        ]
                    }
                    _ => vec![],
                };
                if prepared[i].accept_local_hunt(answer, &body, &others, 0) {
                    assert_ne!(i % 3, 1);
                    accepted += 1;
                } else {
                    assert_eq!(i % 3, 1);
                    assert_eq!(motion_bytes(&prepared[i]), motion_bytes(&serial[i]));
                    rejected += 1;
                }
                prepared[i].allow_start(false);
                serial[i].allow_start(false);
                step(&mut prepared[i], &w, &body, &others, 0);
                step(&mut serial[i], &w, &body, &others, 0);
                assert_eq!(
                    motion_bytes(&prepared[i]),
                    motion_bytes(&serial[i]),
                    "workers={workers} unit={i}"
                );
                assert_eq!(prepared[i].cut, serial[i].cut);
                assert_eq!(prepared[i].contact, serial[i].contact);
            }
            assert_eq!((accepted, rejected), (8, 4));
        }
    }
    #[test]
    fn blocker_height_and_distant_motion_do_not_invalidate_local_hunt() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let obstacle = Obstacle {
            pos: [Fx::from_int(240), Fx::from_int(16), Fx::from_int(300)],
            head: 0,
            half: [Fx::from_int(6), Fx::from_int(10)],
        };
        let mut distant = obstacle;
        distant.pos[0] = Fx::from_int(490);
        let mut m = local_hunter(&w, &body, &[obstacle, distant]);
        let prepared = m
            .prepare_local_hunt(
                &w,
                &body,
                &[obstacle, distant],
                &nav::vehicle::Rubble::new(),
                &nav::vehicle::Changed::default(),
            )
            .unwrap();
        let mut raised = obstacle;
        raised.pos[1] = raised.pos[1].add(Fx::ONE);
        distant.pos[0] = distant.pos[0].sub(Fx::ONE);
        assert!(m.accept_local_hunt(prepared, &body, &[raised, distant], 0));
    }
    #[test]
    fn a_whole_map_order_is_immediate_drives_to_the_end_and_replays() {
        let world = World::new(10000, 32, 256, 100, Box::new(Ground { depth: 0 }));
        world.prepare_navigation([4, 5]);
        for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
            let body = if kind == RangeUnitKind::Tank {
                voxel::unit::tank()
            } else {
                voxel::unit::strider()
            };
            let mut motion = Motion::new([64, 16, 128], kind, &body);
            let goal = [9800, 128];
            assert!(motion.move_to(&world, goal, &body, &[], &nav::vehicle::Rubble::new()));
            assert!(!motion.searching());
            assert!(!motion.partial_path);
            assert_eq!(motion.path.last(), Some(&goal));
            let mut saved = Blob::new();
            motion.save(&mut saved);
            let mut restored = Motion::new([64, 16, 128], kind, &body);
            assert!(restored.load(&mut Scan::new(&saved.v)));
            let mut arrived = false;
            for tick in 0..22000 {
                step(&mut motion, &world, &body, &[], tick);
                step(&mut restored, &world, &body, &[], tick);
                assert_eq!(motion.pos, restored.pos);
                if motion.destination.is_none() {
                    arrived = true;
                    break;
                }
            }
            assert!(arrived, "{kind:?} stopped at {:?}", motion.pos);
            let mut a = Blob::new();
            let mut b = Blob::new();
            motion.save(&mut a);
            restored.save(&mut b);
            assert_eq!(a.v, b.v);
        }
    }
    #[test]
    fn stationary_clearance_still_observes_destroyed_support_next_tick() {
        for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
            let mut w = ground(0);
            let body = match kind {
                RangeUnitKind::Tank => voxel::unit::tank(),
                RangeUnitKind::Strider => voxel::unit::strider(),
                RangeUnitKind::Spider => voxel::unit::spider(),
                RangeUnitKind::Artillery => voxel::unit::artillery(),
                RangeUnitKind::ArtilleryT2 => voxel::unit::artillery_t2(),
                RangeUnitKind::ArtilleryTurretT2 => voxel::unit::artillery_turret_t2(),
                RangeUnitKind::TurretT1 => voxel::unit::turret_t1(),
                RangeUnitKind::TurretT2 => voxel::unit::turret_t2(),
                RangeUnitKind::Base => voxel::unit::command_base(),
            };
            let mut m = Motion::new([300, 16, 300], kind, &body);
            m.enabled = true;
            for tick in 0..20 {
                step(&mut m, &w, &body, &[], tick);
            }
            let before = m.pos;
            for x in 240..360 {
                for z in 240..360 {
                    for y in 8..16 {
                        w.queue_dig(x, y, z);
                    }
                }
            }
            while w.queued() > 0 {
                w.flush();
            }
            // The standalone helper has no world-change tracker; pass the
            // dug region explicitly, as BlastRange does on a live tick.
            m.step(
                &w,
                &body,
                true,
                true,
                crate::mobility::Mobility::intact(true),
                &[],
                20,
                98,
                &nav::vehicle::Rubble::new(),
                &nav::vehicle::Changed::everywhere(),
            );
            assert!(m.floors.iter().all(|h| *h == Fx::from_int(8)));
            assert!(
                m.hull.vy < Fx::ZERO,
                "unsupported hull did not begin falling"
            );
            for tick in 21..140 {
                step(&mut m, &w, &body, &[], tick);
            }
            assert_eq!([m.pos[0], m.pos[2]], [before[0], before[2]]);
            assert!(m.pos[1] < before[1].sub(Fx::from_int(7)));
            assert!(m.pos[1].sub(Fx::from_int(8)).abs() < Fx::from_ratio(1, 10));
        }
    }
    #[test]
    fn early_separation_matches_all_four_contact_axes() {
        // Keep the former all-axis calculation as the bitwise oracle. This
        // covers both separated pairs and the exact positive overlap used
        // when already intersecting hulls try to move apart.
        fn reference(a: Obstacle, b: Obstacle) -> Fx {
            let axes = |head| {
                let (c, s) = (angle::cos(head), angle::sin(head));
                [[c, s], [s.neg(), c]]
            };
            let aa = axes(a.head);
            let bb = axes(b.head);
            let dot = |u: [Fx; 2], v: [Fx; 2]| u[0].mul(v[0]).add(u[1].mul(v[1]));
            let delta = [b.pos[0].sub(a.pos[0]), b.pos[2].sub(a.pos[2])];
            aa.iter()
                .chain(&bb)
                .map(|axis| {
                    let radius = |basis: [[Fx; 2]; 2], half: [Fx; 2]| {
                        dot(*axis, basis[0])
                            .abs()
                            .mul(half[0])
                            .add(dot(*axis, basis[1]).abs().mul(half[1]))
                    };
                    radius(aa, a.half)
                        .add(radius(bb, b.half))
                        .sub(dot(delta, *axis).abs())
                        .max(Fx::ZERO)
                })
                .min()
                .unwrap()
        }
        let offsets = [-801, -600, -599, -300, -1, 0, 1, 300, 599, 600, 801];
        let mut touching = 0;
        let mut apart = 0;
        for heading in (0..angle::FULL).step_by(4096) {
            for other_heading in (0..angle::FULL).step_by(4096) {
                for x in offsets {
                    for z in offsets {
                        let a = Obstacle {
                            pos: [Fx::from_ratio(1, 3), Fx::from_int(16), Fx::ZERO],
                            head: heading,
                            half: [Fx::from_int(30), Fx::from_ratio(151, 10)],
                        };
                        let b = Obstacle {
                            pos: [
                                a.pos[0].add(Fx::from_ratio(x, 10)),
                                Fx::ZERO,
                                Fx::from_ratio(z, 10),
                            ],
                            head: other_heading,
                            half: [Fx::from_int(30), Fx::from_ratio(99, 10)],
                        };
                        let expected = reference(a, b);
                        touching += usize::from(expected > Fx::ZERO);
                        apart += usize::from(expected == Fx::ZERO);
                        assert_eq!(a.penetration(&b), expected);
                        assert_eq!(b.penetration(&a), reference(b, a));
                    }
                }
            }
        }
        assert!(touching > 1000 && apart > 1000);
    }
    #[test]
    fn a_dry_reachable_pocket_resumes_recovery_after_loading() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let mut m = Motion::new([200, 16, 300], RangeUnitKind::Tank, &body);
        m.enabled = true;
        m.destination = Some([430, 300]);
        let plan = nav::vehicle::plan(
            nav::vehicle::Probe::new(&w),
            m.shape(&body),
            [m.pos[0], m.pos[2]],
            m.drive.head,
            m.destination.unwrap(),
            m.search(&w),
            |p, _| {
                (p[0].sub(Fx::from_int(200)).abs() <= Fx::from_int(8)
                    && p[1].sub(Fx::from_int(300)).abs() <= Fx::from_int(8))
                    || p[0] >= Fx::from_int(350)
            },
        );
        assert!(plan.exhausted && plan.nodes > 1 && plan.path.is_empty());
        assert!(!plan.stranded, "the search must leave its initial cell");
        m.take_plan(plan);
        m.next_plan = m.replan_after_search();
        let from = m.pos;
        let mut replicas = Vec::new();
        let mut escaped = false;
        for tick in 0..600 {
            if [
                0,
                1,
                m.navigation.recovery_ticks - 1,
                m.navigation.recovery_ticks + 4,
                90,
            ]
            .contains(&tick)
            {
                let mut bytes = Blob::new();
                m.save(&mut bytes);
                for stale_dry in [false, true] {
                    let mut restored = Motion::new([200, 16, 300], RangeUnitKind::Tank, &body);
                    restored.dry = stale_dry;
                    assert!(restored.load(&mut Scan::new(&bytes.v)));
                    assert!(!restored.dry, "load retained an old scratch result");
                    replicas.push(restored);
                }
            }
            step(&mut m, &w, &body, &[], tick);
            escaped |= m.escape_left > 0;
            let mut expected = Blob::new();
            m.save(&mut expected);
            for restored in &mut replicas {
                step(restored, &w, &body, &[], tick);
                let mut actual = Blob::new();
                restored.save(&mut actual);
                assert!(
                    expected.v == actual.v,
                    "recovery diverged after loading at tick {tick}"
                );
            }
            if m.destination.is_none() {
                break;
            }
        }
        assert!(escaped, "dry pocket never started recovery");
        assert_ne!(m.pos, from, "dry pocket stayed idle forever");
        assert!(
            m.destination.is_none(),
            "recovered hull never reached its goal"
        );
    }

    #[test]
    fn driving_and_blast_shoves_cross_the_old_range_boundary() {
        let w = World::new(1024, 64, 1024, 100, Box::new(Ground { depth: 0 }));
        for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
            let body = match kind {
                RangeUnitKind::Tank => voxel::unit::tank(),
                RangeUnitKind::Strider => voxel::unit::strider(),
                RangeUnitKind::Spider => voxel::unit::spider(),
                RangeUnitKind::Artillery => voxel::unit::artillery(),
                RangeUnitKind::ArtilleryT2 => voxel::unit::artillery_t2(),
                RangeUnitKind::ArtilleryTurretT2 => voxel::unit::artillery_turret_t2(),
                RangeUnitKind::TurretT1 => voxel::unit::turret_t1(),
                RangeUnitKind::TurretT2 => voxel::unit::turret_t2(),
                RangeUnitKind::Base => voxel::unit::command_base(),
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
            "a {:?} path {:?} search {:?} needs_recovery {} escape {} blocked {} dry {} plan {:?}; b {:?} path {:?} search {:?} needs_recovery {} escape {} blocked {} dry {} plan {:?}",
            a.pos, a.path, a.hunt, a.needs_recovery,a.escape,a.blocked,a.dry,a.last_plan,
            b.pos, b.path, b.hunt,b.needs_recovery,b.escape,b.blocked,b.dry,b.last_plan
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
    fn a_cleared_crossing_resumes_the_route_while_its_replacement_is_pending() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let goal = [430, 300];
        let mut mover = Motion::new([200, 16, 300], RangeUnitKind::Tank, &body);
        let blocker = Motion::new([280, 16, 300], RangeUnitKind::Tank, &body);
        let other = Obstacle::of(&blocker, &body);
        mover.enabled = true;
        mover.destination = Some(goal);
        mover.path = vec![goal];
        mover.next_plan = 1000;
        mover.navigation.search_nodes_per_tick = 1;
        let mut tick = 0;
        while !mover.searching() && tick < 100 {
            step(&mut mover, &w, &body, &[other], tick);
            assert_eq!(Obstacle::of(&mover, &body).penetration(&other), Fx::ZERO);
            tick += 1;
        }
        assert!(
            mover.searching(),
            "contact did not start a replacement search"
        );
        assert_eq!(
            mover.path,
            vec![goal],
            "pending search discarded the usable route"
        );
        // A refused full-speed step may still leave room for a slower one.
        // Let that settle so this fixture isolates waiting at the crossing.
        for _ in 0..100 {
            let before = mover.pos;
            step(&mut mover, &w, &body, &[other], tick);
            tick += 1;
            assert_eq!(Obstacle::of(&mover, &body).penetration(&other), Fx::ZERO);
            if mover.searching() && mover.pos[0] == before[0] && mover.pos[2] == before[2] {
                break;
            }
        }
        assert!(mover.searching());
        mover.blocked_ticks = mover.navigation.recovery_ticks;
        let stopped = mover.pos;
        let mut saved = Blob::new();
        mover.save(&mut saved);
        let mut restored = Motion::new([200, 16, 300], RangeUnitKind::Tank, &body);
        restored.navigation = mover.navigation;
        assert!(restored.load(&mut Scan::new(&saved.v)));
        let release = tick + 1;
        for current in tick..release {
            for m in [&mut mover, &mut restored] {
                step(m, &w, &body, &[other], current);
                assert_eq!(Obstacle::of(m, &body).penetration(&other), Fx::ZERO);
                assert!(m.searching());
                assert_eq!(m.escape_left, 0, "recovery interrupted the pending search");
            }
            let mut a = Blob::new();
            let mut b = Blob::new();
            mover.save(&mut a);
            restored.save(&mut b);
            assert_eq!(a.v, b.v, "blocked replacement diverged after load");
        }
        // The crossing hull leaves. Resume immediately, without waiting for
        // the distant replacement route or running an escape manoeuvre.
        for current in release..release + 2 {
            for m in [&mut mover, &mut restored] {
                step(m, &w, &body, &[], current);
                assert!(
                    m.pos[0] > stopped[0],
                    "free crossing still waits for planning"
                );
                assert_eq!(m.escape_left, 0);
                assert_eq!(m.destination, Some(goal));
            }
            let mut a = Blob::new();
            let mut b = Blob::new();
            mover.save(&mut a);
            restored.save(&mut b);
            assert_eq!(a.v, b.v, "pending route diverged after load");
        }
        // The hull is still within the reach of the search it waited on:
        // the search holds, and its route is taken up from where the hull
        // has got to (ADR-462).
        assert!(
            mover.searching(),
            "a hull within reach dropped its pending search"
        );
        for current in release + 2..1000 {
            step(&mut mover, &w, &body, &[], current);
            if mover.destination.is_none() {
                break;
            }
        }
        assert!(
            mover.destination.is_none(),
            "retained route never reached the goal"
        );
    }
    /// **A retargeted hull drives on and takes the new route up where it
    /// has got to** (ADR-462). A tank at full speed given a new goal keeps
    /// its route while the new one is searched over several ticks, never
    /// slows for the search, takes the new route up at speed and does not
    /// turn back for the turn points it has passed.
    #[test]
    fn a_retargeted_hull_drives_on_and_takes_up_the_new_route() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let rubble = nav::vehicle::Rubble::new();
        let mut m = Motion::new([60, 16, 200], RangeUnitKind::Tank, &body);
        assert!(m.move_to(&w, [580, 200], &body, &[], &rubble));
        let full = Fx::from_int(m.speed);
        let mut tick = 0;
        while m.drive.vel < full && tick < 300 {
            m.allow_start(true);
            step(&mut m, &w, &body, &[], tick);
            tick += 1;
        }
        assert_eq!(m.drive.vel, full, "the fixture needs a hull at full speed");
        let (route, goal) = (m.path.clone(), [580, 420]);
        m.navigation.global_nodes = 0; // Exercise the resumable fine planner.
        m.navigation.search_nodes_per_tick = 8;
        assert!(m.retarget(&w, goal, &body, &[], &rubble));
        assert!(
            m.searching(),
            "the fixture needs a search over several ticks"
        );
        assert_eq!(m.path, route, "the route driven was dropped for the search");
        let mut previous = m.pos;
        let mut ticks = 0;
        while m.searching() && ticks < 200 {
            m.allow_start(true);
            step(&mut m, &w, &body, &[], tick);
            // On the tick the answer comes the hull turns onto it.
            if m.searching() {
                assert_eq!(m.drive.vel, full, "the hull slowed for the search");
            }
            assert!(m.pos[0] > previous[0]);
            previous = m.pos;
            tick += 1;
            ticks += 1;
        }
        assert!(ticks > 1 && !m.searching(), "searched for {ticks} ticks");
        assert_eq!(m.path.last(), Some(&goal), "the new route was not taken up");
        assert_eq!(m.destination, Some(goal));
        // Taken up where the hull has got to: it drives on towards the
        // goal and never back past where it took the route up.
        let taken = m.pos;
        for _ in 0..600 {
            if m.destination.is_none() {
                break;
            }
            m.allow_start(true);
            step(&mut m, &w, &body, &[], tick);
            assert!(m.pos[0] >= taken[0], "turned back to {:?}", m.pos);
            tick += 1;
        }
        assert!(m.destination.is_none(), "the new goal was not reached");
        for (axis, coordinate) in [0, 2].into_iter().zip(goal) {
            assert!(m.pos[axis].sub(Fx::from_int(coordinate)).abs() <= Fx::from_int(m.arrival));
        }
    }
    /// A route taken up away from where its search began skips only the
    /// turn points near that start which the hull has passed; a turn
    /// still ahead, one beyond the reach of the search, and the route's
    /// end are kept (ADR-462).
    #[test]
    fn a_route_taken_up_skips_only_the_turns_passed_near_its_start() {
        let body = voxel::unit::tank();
        let from = [Fx::from_int(90), Fx::from_int(200)];
        let reach = Motion::new([90, 16, 200], RangeUnitKind::Tank, &body)
            .navigation
            .retarget_units;
        let cursor = |here: [i32; 2], path: Vec<[i32; 2]>| {
            let mut m = Motion::new([here[0], 16, here[1]], RangeUnitKind::Tank, &body);
            m.path = path;
            m.take_up(from);
            m.cursor
        };
        // Passed on the way: skipped, and so is the next one passed.
        assert_eq!(cursor([150, 200], vec![[110, 200], [300, 400]]), 1);
        assert_eq!(
            cursor([150, 200], vec![[110, 200], [130, 210], [300, 400]]),
            2
        );
        // Still ahead of the hull.
        assert_eq!(cursor([100, 200], vec![[110, 200], [300, 400]]), 0);
        // Beyond the reach of the search from its start.
        let far = 90 + reach + 1;
        assert_eq!(
            cursor([far + 40, 200], vec![[far, 200], [far + 200, 400]]),
            0
        );
        // The end of the route, however far the hull has driven past it.
        assert_eq!(cursor([150, 200], vec![[110, 200]]), 0);
        // A hull that has not moved takes the route whole.
        assert_eq!(cursor([90, 200], vec![[90, 200], [300, 400]]), 0);
    }
    #[test]
    fn arrival_and_explicit_orders_cancel_a_pending_route_replacement() {
        let w = ground(0);
        let body = voxel::unit::tank();
        let rubble = nav::vehicle::Rubble::new();
        let goal = [202, 300];
        let mut mover = Motion::new([200, 16, 300], RangeUnitKind::Tank, &body);
        mover.enabled = true;
        mover.destination = Some(goal);
        mover.path = vec![goal];
        mover.navigation.global_nodes = 0; // Exercise the resumable fine planner.
        mover.navigation.search_nodes_per_tick = 1;
        assert!(mover.plan_to(&w, goal, &body, &[], &rubble));
        assert!(mover.searching());
        let mut retargeted = mover.clone();
        step(&mut mover, &w, &body, &[], 0);
        assert!(mover.destination.is_none());
        assert!(!mover.searching(), "arrival kept the superseded search");
        for tick in 1..10 {
            step(&mut mover, &w, &body, &[], tick);
            assert!(mover.destination.is_none());
            assert_eq!(mover.cursor, mover.path.len());
        }
        // Even another explicit order to the same point discards the fallback.
        for next in [goal, [200, 430]] {
            let mut ordered = retargeted.clone();
            assert!(ordered.move_to(&w, next, &body, &[], &rubble));
            assert!(
                ordered.path.is_empty(),
                "explicit order kept the previous route"
            );
            assert_eq!(ordered.destination, Some(next));
            assert!(ordered.searching());
        }
        retargeted.stop();
        assert!(retargeted.path.is_empty());
        assert!(!retargeted.searching());
        assert!(retargeted.destination.is_none());
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
                        RangeUnitKind::Spider => voxel::unit::spider(),
                        RangeUnitKind::Artillery => voxel::unit::artillery(),
                        RangeUnitKind::ArtilleryT2 => voxel::unit::artillery_t2(),
                        RangeUnitKind::ArtilleryTurretT2 => voxel::unit::artillery_turret_t2(),
                        RangeUnitKind::TurretT1 => voxel::unit::turret_t1(),
                        RangeUnitKind::TurretT2 => voxel::unit::turret_t2(),
                        RangeUnitKind::Base => voxel::unit::command_base(),
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

#[cfg(test)]
#[path = "range_local_sweep_tests.rs"]
mod local_sweep_tests;

/// Navigation observers pair each independent leaf by unit and sweep batch.
pub(super) fn sweep_event(batch: usize, begin: bool) -> &'static str {
    const BEGIN: [&str; 4] = [
        "sweep_0_begin",
        "sweep_1_begin",
        "sweep_2_begin",
        "sweep_3_begin",
    ];
    const END: [&str; 4] = ["sweep_0_end", "sweep_1_end", "sweep_2_end", "sweep_3_end"];
    if begin {
        BEGIN[batch]
    } else {
        END[batch]
    }
}
