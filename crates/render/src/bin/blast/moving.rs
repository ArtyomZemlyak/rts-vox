//! Interpolated actors and incremental shadow / gas occupancy.
use super::*;
use render::gait::{self, FootPlant, GaitParams, LegRig, Pose, Stance, StepMotion};
use sim_core::blast::RangeUnit;

#[derive(Resource, Default)]
pub struct Visual {
    pub previous: Vec<Pose>,
    pub fragments: std::collections::BTreeMap<u64, Vec3>,
    pub turret: Option<i32>,
    occupied: Vec<usize>,
    feet: Vec<Vec<FootPlant>>,
    rigs: Vec<LegRig>,
}
pub fn pose(u: &RangeUnit) -> Pose {
    let m = &u.motion;
    Pose {
        pos: Vec3::from(m.pos.map(|v| v.to_f64_lossy() as f32)),
        yaw: render::yaw_of(m.drive.head),
        up: Vec3::new(
            m.hull.up.x.to_f64_lossy() as f32,
            m.hull.up.y.to_f64_lossy() as f32,
            m.hull.up.z.to_f64_lossy() as f32,
        ),
        phase: gait::phase_of(m.phase),
        alive: u.status.alive,
    }
}
pub fn visible(u: &RangeUnit, previous: Option<&Pose>, scene: &Scene) -> Pose {
    let current = pose(u);
    let t = if scene.realtime && !scene.paused {
        (scene.acc * 30.0).clamp(0.0, 1.0)
    } else {
        1.0
    };
    previous.map_or(current, |p| gait::lerp(p, &current, t))
}
pub fn before(sim: &BlastRange, v: &mut Visual) {
    v.turret = Some(sim.turret.angle);
    v.fragments = sim
        .matter
        .debris
        .fragments
        .iter()
        .map(|f| {
            (
                f.id,
                super::fragments::pos([f.position.x, f.position.y, f.position.z]),
            )
        })
        .collect();
    v.previous = sim
        .units
        .iter()
        .chain(std::iter::once(&sim.gun))
        .map(pose)
        .collect();
    if v.occupied.is_empty() {
        v.occupied = cells(sim);
    }
}
fn cells(sim: &BlastRange) -> Vec<usize> {
    let mut out = Vec::new();
    for u in sim.units.iter().chain(std::iter::once(&sim.gun)) {
        let p = pose(u);
        let rot = render::realtime::hull_rotation(p.up, p.yaw);
        let centre = Vec3::from(render::model::root_centre(&u.body));
        for z in 0..u.body.vol.nz {
            for y in 0..u.body.vol.ny {
                for x in 0..u.body.vol.nx {
                    if !u.body.vol.get(x as usize, y as usize, z as usize) {
                        continue;
                    }
                    let q = (p.pos
                        + rot
                            * (Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5) - centre))
                        .floor()
                        .as_ivec3();
                    if q.cmpge(IVec3::ZERO).all() && q.cmplt(IVec3::new(640, 192, 640)).all() {
                        out.push((q.x + 640 * (q.y + 192 * q.z)) as usize);
                    }
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}
pub fn occupancy(sim: &BlastRange, gas: &mut GasInput, v: &mut Visual) {
    let next = cells(sim);
    if next == v.occupied {
        return;
    }
    let mut dirty = Vec::new();
    let words = Arc::make_mut(&mut gas.terrain_words);
    let mut set = |i: usize, m: u32| {
        let shift = (i % 8) * 4;
        let word = &mut words[i / 8];
        let new = (*word & !(15 << shift)) | (m << shift);
        if new == *word {
            return;
        }
        *word = new;
        dirty.push(i / 8);
    };
    for &i in &v.occupied {
        let p = [
            (i % 640) as i32,
            (i / 640 % 192) as i32,
            (i / 640 / 192) as i32,
        ];
        set(
            i,
            surface_material(sim, p, sim_core::blast::material(&sim.world, p)),
        );
    }
    for &i in &next {
        set(i, 15);
    }
    dirty.sort_unstable();
    dirty.dedup();
    let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
    for i in dirty {
        if let Some(r) = spans.last_mut() {
            if i <= r.end + 16 {
                r.end = i + 1;
                continue;
            }
        }
        spans.push(i..i + 1);
    }
    gas.terrain_dirty.extend(spans);
    gas.terrain_revision += 1;
    v.occupied = next;
}
pub fn animate(
    scene: Res<Scene>,
    mut v: ResMut<Visual>,
    time: Res<Time<Real>>,
    mut query: Query<(&Witness, &mut Transform)>,
) {
    if v.rigs.is_empty() {
        let b = voxel::unit::strider();
        v.rigs = gait::rigs_of(&b, 0.01, &render::model::segment_frames(&b, 0.01));
    }
    let rigs = v.rigs.clone();
    v.feet.resize_with(scene.sim.units.len(), Vec::new);
    let mut poses = Vec::new();
    let mut legs = Vec::new();
    for (i, u) in scene.sim.units.iter().enumerate() {
        let p = visible(u, v.previous.get(i), &scene);
        poses.push(p);
        let mut lp = Vec::new();
        if u.kind == RangeUnitKind::Strider && u.motion.enabled {
            let params = GaitParams::of(u.motion.chassis.gait.as_ref().expect("walking gait"), 0.1);
            let motion = v.previous.get(i).map_or(StepMotion::default(), |old| {
                StepMotion::between(old, &pose(u), 1.0 / 30.0)
            });
            let floor = |x: f32, z: f32| {
                sim_core::blast::motion::floor(&scene.sim.world, x.floor() as i32, z.floor() as i32)
                    as f32
            };
            let stance = Stance {
                grounded: u.motion.hull.load > fx::Fx::ZERO,
                dt: if scene.paused { 0.0 } else { time.delta_secs() },
                base: p.pos,
                rot: render::realtime::hull_rotation(p.up, p.yaw),
                phase: p.phase,
                lift_scale: (motion.cycles * params.stride / u.motion.speed.max(1) as f32 * 4.0)
                    .min(1.0),
                motion,
                params: &params,
                floor: &floor,
            };
            v.feet[i].resize(rigs.len(), FootPlant::default());
            lp.extend(
                rigs.iter()
                    .zip(&mut v.feet[i])
                    .map(|(r, f)| f.pose(r, &stance)),
            );
        }
        legs.push(lp);
    }
    for (w, mut tr) in &mut query {
        let Some(p) = poses.get(w.0) else {
            continue;
        };
        let rotation = render::realtime::hull_rotation(p.up, p.yaw);
        let mut local = w.1;
        if let Some(seg) = w.2 {
            for (i, r) in rigs.iter().enumerate() {
                if let (Some(role), Some(lp)) = (r.role(seg), legs[w.0].get(i)) {
                    let f = lp.part(role);
                    local.translation = f.pos;
                    local.rotation = f.rot;
                    break;
                }
            }
        }
        *tr = local;
        tr.translation = p.pos + rotation * local.translation;
        tr.rotation = rotation * local.rotation;
    }
}
