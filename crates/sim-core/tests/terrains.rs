//! Battle-damaged terrain series for the range vehicles (ADR-272): every
//! terrain kind from every approach heading, both hull kinds, with pose
//! continuity measured per tick. A run either arrives smoothly or the table
//! says exactly where it did not.
use fx::{angle, Fx};
use modding::range::RangeUnitKind;
use sim_core::blast::motion::Motion;
use sim_core::mobility::Mobility;
use voxel::{Generator, World};

const FLOOR: i32 = 16;
const CENTRE: [i32; 2] = [320, 320];
const REACH: i32 = 110;

#[derive(Clone, Copy, Debug)]
enum Terrain {
    Crater { depth: i32, radius: i32 },
    Berm { depth: i32, radius: i32, rim: i32 },
    Field,
    Trench { width: i32, depth: i32 },
    Rubble { size: i32 },
    Steps { rise: i32 },
    Ridge { height: i32, half: i32 },
}
const SERIES: [Terrain; 11] = [
    Terrain::Ridge {
        height: 8,
        half: 40,
    },
    Terrain::Crater {
        depth: 6,
        radius: 18,
    },
    Terrain::Crater {
        depth: 12,
        radius: 24,
    },
    Terrain::Berm {
        depth: 8,
        radius: 16,
        rim: 3,
    },
    Terrain::Field,
    Terrain::Trench { width: 6, depth: 3 },
    Terrain::Trench {
        width: 10,
        depth: 5,
    },
    Terrain::Rubble { size: 2 },
    Terrain::Rubble { size: 3 },
    Terrain::Steps { rise: 3 },
    Terrain::Ridge {
        height: 14,
        half: 40,
    },
];

fn bowl(dx: i32, dz: i32, depth: i32, radius: i32) -> i32 {
    let r2 = dx * dx + dz * dz;
    if r2 >= radius * radius {
        0
    } else {
        depth * (radius * radius - r2) / (radius * radius)
    }
}
fn hash(x: i32, z: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(73_856_093) ^ (z as u32).wrapping_mul(19_349_663);
    h ^= h >> 13;
    h.wrapping_mul(0x5bd1_e995) >> 7
}
struct Ground(Terrain);
impl Ground {
    fn height(&self, x: i32, z: i32) -> i32 {
        let dx = x - CENTRE[0];
        let dz = z - CENTRE[1];
        match self.0 {
            Terrain::Crater { depth, radius } => FLOOR - bowl(dx, dz, depth, radius),
            Terrain::Berm { depth, radius, rim } => {
                let r = Fx::from_int(dx * dx + dz * dz).sqrt().to_int_trunc();
                let ring = (rim - (r - radius - rim).abs()).max(0);
                FLOOR - bowl(dx, dz, depth, radius) + ring
            }
            Terrain::Field => {
                let craters = [
                    (0, 0, 8, 14),
                    (-26, 18, 5, 10),
                    (24, -16, 6, 12),
                    (-10, -30, 4, 9),
                    (30, 26, 7, 11),
                    (-34, -6, 3, 8),
                ];
                FLOOR
                    - craters
                        .iter()
                        .map(|&(ox, oz, d, r)| bowl(dx - ox, dz - oz, d, r))
                        .max()
                        .unwrap_or(0)
            }
            Terrain::Trench { width, depth } => {
                // Finite ditch across the x axis; oblique routes still cross it.
                if dx.abs() * 2 < width && dz.abs() < 150 {
                    FLOOR - depth
                } else {
                    FLOOR
                }
            }
            Terrain::Rubble { size } => {
                let h = hash(x.div_euclid(3), z.div_euclid(3));
                if h.is_multiple_of(5) && dx.abs() < 70 && dz.abs() < 70 {
                    FLOOR + 1 + (h >> 8) as i32 % size
                } else {
                    FLOOR
                }
            }
            Terrain::Steps { rise } => {
                let up = ((dx + 80) / 16).clamp(0, 5);
                let down = (5 - (dx / 16).max(0)).clamp(0, 5);
                if dx.abs() > 80 {
                    FLOOR
                } else {
                    FLOOR + rise * up.min(down)
                }
            }
            Terrain::Ridge { height, half } => {
                if dx.abs() >= half {
                    FLOOR
                } else {
                    FLOOR + height * (half * half - dx * dx) / (half * half)
                }
            }
        }
    }
}
impl Generator for Ground {
    fn material(&self, x: u32, y: u32, z: u32) -> u16 {
        if (y as i32) < self.height(x as i32, z as i32) {
            1
        } else {
            0
        }
    }
}
/// Whether a hull can be expected to cross this terrain at all. A vertical
/// ditch deeper than the climb step is a wall, and the route must say so;
/// a route running along the ditch never meets that wall. A hill steeper
/// than both tilt limits is a wall across the whole map.
fn passable(t: Terrain, kind: RangeUnitKind, eighth: i32) -> bool {
    // tan of the tilt limits in sim.ron: 91 % and 84 % of up.
    let grade_pct = match kind {
        RangeUnitKind::Tank
        | RangeUnitKind::Artillery
        | RangeUnitKind::TurretT1
        | RangeUnitKind::TurretT2 => 45,
        RangeUnitKind::Strider | RangeUnitKind::Spider => 65,
    };
    match t {
        Terrain::Trench { depth, .. } => depth <= 4 || eighth == 2,
        Terrain::Ridge { height, half } => 200 * height <= grade_pct * half || eighth == 2,
        _ => true,
    }
}
/// Ground height a hull settles to at `p`: the highest column under it.
fn settle(t: Terrain, p: [i32; 2]) -> i32 {
    let g = Ground(t);
    (-30..=30)
        .flat_map(|dx| (-15..=15).map(move |dz| (dx, dz)))
        .map(|(dx, dz)| g.height(p[0] + dx, p[1] + dz))
        .max()
        .unwrap_or(FLOOR)
}
fn milli(v: Fx) -> i64 {
    ((v.raw() as i128 * 1000) >> 32) as i64
}

#[derive(Debug, Default)]
struct Run {
    arrived: Option<u32>,
    routed: bool,
    dy: Fx,
    /// Largest change of the per-tick height change: the jerk a viewer sees.
    ddy: Fx,
    tilt: Fx,
    dv: Fx,
    blocked: u32,
    reversed: bool,
    low: i32,
    high: i32,
    /// Tick of the largest height jump, with the hull height and support
    /// floors just before and after it.
    worst: (u32, [i64; 2], [[i64; 4]; 2]),
}
fn run(t: Terrain, kind: RangeUnitKind, heading: i32, back: bool) -> Run {
    let w = World::new(640, 64, 640, 100, Box::new(Ground(t)));
    let body = match kind {
        RangeUnitKind::Tank => voxel::unit::tank(),
        RangeUnitKind::Strider => voxel::unit::strider(),
        RangeUnitKind::Spider => voxel::unit::spider(),
        RangeUnitKind::Artillery => voxel::unit::artillery(),
        RangeUnitKind::TurretT1 => voxel::unit::turret_t1(),
        RangeUnitKind::TurretT2 => voxel::unit::turret_t2(),
    };
    let yaw = angle::norm(heading + if back { angle::FULL / 2 } else { 0 });
    let dir = [angle::cos(yaw), angle::sin(yaw)];
    let along = |k: i32| {
        [
            CENTRE[0] + dir[0].mul(Fx::from_int(k)).to_int_trunc(),
            CENTRE[1] + dir[1].mul(Fx::from_int(k)).to_int_trunc(),
        ]
    };
    let start = along(-REACH);
    let goal = along(REACH);
    let mut m = Motion::new([start[0], settle(t, start), start[1]], kind, &body);
    m.drive.head = yaw;
    let mut out = Run {
        routed: m.move_to(&w, goal, &body, &[], &nav::vehicle::Rubble::new()),
        low: i32::MAX,
        high: i32::MIN,
        ..Default::default()
    };
    let mut y = m.pos[1];
    let mut up = m.hull.up;
    let mut vel = m.drive.vel;
    let mut floors = [0i64; 4];
    let mut rate = Fx::ZERO;
    let mut old_path = Vec::new();
    let trace = std::env::var_os("RTS_TRACE_TERRAIN").is_some();
    for tick in 0..1500u32 {
        m.step(
            &w,
            &body,
            true,
            true,
            Mobility::intact(true),
            &[],
            tick,
            98,
            &nav::vehicle::Rubble::new(),
            &nav::vehicle::Changed::default(),
        );
        if trace && (old_path != m.path || (m.blocked && out.blocked < 3)) {
            eprintln!("tick={tick} {kind:?} pos={:?} head={} vel={:?} blocked={} path={:?} cursor={} partial={}", m.pos, m.drive.head, m.drive.vel, m.blocked, m.path, m.cursor, m.partial_path);
            old_path = m.path.clone();
        }
        let dup = m.hull.up.sub(up);
        let step = m.pos[1].sub(y);
        let jump = step.abs();
        let jerk = step.sub(rate).abs();
        rate = step;
        let now = m.floors.map(milli);
        if jerk > out.ddy {
            out.worst = (tick, [milli(y), milli(m.pos[1])], [floors, now]);
        }
        floors = now;
        out.dy = out.dy.max(jump);
        out.ddy = out.ddy.max(jerk);
        out.tilt = out.tilt.max(dup.len_sq().sqrt());
        out.dv = out.dv.max(m.drive.vel.sub(vel).abs());
        out.blocked += u32::from(m.blocked);
        out.reversed |= m.drive.vel < Fx::ZERO;
        let h = m.pos[1].to_int_trunc();
        out.low = out.low.min(h);
        out.high = out.high.max(h);
        y = m.pos[1];
        up = m.hull.up;
        vel = m.drive.vel;
        if m.destination.is_none() {
            out.arrived = Some(tick);
            break;
        }
    }
    out
}

/// Trace a crater detour against the same physical controller as the series.
#[test]
#[ignore]
fn trace_crater_detour() {
    let t = Terrain::Crater {
        depth: 12,
        radius: 24,
    };
    for (kind, heading) in [
        (RangeUnitKind::Tank, 3 * angle::FULL / 8),
        (RangeUnitKind::Strider, angle::FULL / 8),
    ] {
        let r = run(t, kind, heading, false);
        eprintln!(
            "trace {kind:?}: arrived={:?} blocked={}",
            r.arrived, r.blocked
        );
    }
}

/// Poses along the straight run, for reading why a route is refused.
#[test]
#[ignore]
fn probe_poses_along_the_run() {
    for (t, kind) in [
        (Terrain::Steps { rise: 3 }, RangeUnitKind::Tank),
        (
            Terrain::Crater {
                depth: 12,
                radius: 24,
            },
            RangeUnitKind::Strider,
        ),
    ] {
        let w = World::new(640, 64, 640, 100, Box::new(Ground(t)));
        let body = match kind {
            RangeUnitKind::Tank => voxel::unit::tank(),
            RangeUnitKind::Strider => voxel::unit::strider(),
            RangeUnitKind::Spider => voxel::unit::spider(),
            RangeUnitKind::Artillery => voxel::unit::artillery(),
            RangeUnitKind::TurretT1 => voxel::unit::turret_t1(),
            RangeUnitKind::TurretT2 => voxel::unit::turret_t2(),
        };
        let m = Motion::new([210, settle(t, [210, 320]), 320], kind, &body);
        let shape = m.probe_shape(&body);
        let mut probe = nav::vehicle::Probe::new(&w);
        eprintln!("== {t:?} {kind:?}");
        for x in (210..=430).step_by(2) {
            let pose = probe.pose([Fx::from_int(x), Fx::from_int(320)], 0, shape);
            if pose.refused() || x % 16 == 0 {
                eprintln!(
                    "x={x} ground={} blocked={} height={} floors={:?}",
                    Ground(t).height(x, 320),
                    pose.blocked,
                    milli(pose.height),
                    pose.floors.map(milli)
                );
            }
        }
        // The executed run: every tick the hull was refused a move.
        let mut m = Motion::new([210, settle(t, [210, 320]), 320], kind, &body);
        eprintln!(
            "routed={}",
            m.move_to(&w, [430, 320], &body, &[], &nav::vehicle::Rubble::new())
        );
        let mut shown = 0;
        for tick in 0..900u32 {
            m.step(
                &w,
                &body,
                true,
                true,
                Mobility::intact(true),
                &[],
                tick,
                98,
                &nav::vehicle::Rubble::new(),
                &nav::vehicle::Changed::default(),
            );
            if m.blocked && shown < 12 {
                shown += 1;
                let mut p = nav::vehicle::Probe::new(&w);
                let here = p.pose([m.pos[0], m.pos[2]], m.drive.head, shape);
                eprintln!(
                    "tick={tick} pos=({},{},{}) head={} blocked_cols={} steep={} tilt={} height={} floors={:?} cursor={}/{}",
                    milli(m.pos[0]),
                    milli(m.pos[1]),
                    milli(m.pos[2]),
                    m.drive.head,
                    here.blocked,
                    here.steep,
                    here.tilt,
                    milli(here.height),
                    here.floors.map(milli),
                    m.cursor,
                    m.path.len()
                );
            }
            if m.destination.is_none() {
                eprintln!("arrived tick={tick}");
                break;
            }
        }
    }
}

#[test]
fn every_terrain_is_crossed_from_every_heading_without_pose_jumps() {
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for t in SERIES {
        for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
            for eighth in 0..4 {
                for back in [false, true] {
                    let heading = eighth * angle::FULL / 8;
                    let r = run(t, kind, heading, back);
                    let deg = eighth * 45 + if back { 180 } else { 0 };
                    let row = format!(
                        "{t:?} {kind:?} {deg:>3}deg routed={} arrived={:?} dy_milli={} ddy_milli={} tilt_milli={} dv_milli={} blocked={} reversed={} y={}..{} worst={:?}",
                        r.routed,
                        r.arrived,
                        milli(r.dy),
                        milli(r.ddy),
                        milli(r.tilt),
                        milli(r.dv),
                        r.blocked,
                        r.reversed,
                        r.low,
                        r.high,
                        r.worst
                    );
                    // A walker skirting a 45-degree crater rim on a diagonal
                    // approach can still meet a refusal between two planner
                    // samples and back off once; it must still arrive.
                    let recovery_allowed = matches!(
                        (t, kind),
                        (Terrain::Crater { depth: 12, .. }, RangeUnitKind::Strider)
                    );
                    let ok = if passable(t, kind, eighth) {
                        r.routed
                            && r.arrived.is_some()
                            && (!r.reversed || recovery_allowed)
                            && r.ddy < Fx::from_ratio(7, 10)
                            && r.tilt < Fx::from_ratio(1, 4)
                            && r.blocked <= if recovery_allowed { 60 } else { 12 }
                    } else {
                        // The ditch is a wall: no route through it, and the
                        // hull never claims to have crossed it.
                        !r.routed || r.arrived.is_none() || r.low >= FLOOR - 1
                    };
                    if !ok {
                        failures.push(row.clone());
                    }
                    rows.push(row);
                }
            }
        }
    }
    for row in &rows {
        eprintln!("{row}");
    }
    assert!(
        failures.is_empty(),
        "{} of {} runs failed:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}
