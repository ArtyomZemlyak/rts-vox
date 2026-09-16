//! Interpolated actors and incremental shadow / gas occupancy.
use super::*;
use render::footing::Contact;
use render::gait::{self, LegRig, Pose};
use sim_core::blast::RangeUnit;

#[derive(Resource, Default)]
pub struct Visual {
    pub previous: Vec<Pose>,
    /// Foot contacts of the previous tick, per unit and leg (ADR-279).
    previous_feet: Vec<Vec<Option<Contact>>>,
    pub fragments: std::collections::BTreeMap<u64, Vec3>,
    pub turret: Option<i32>,
    occupied: Vec<usize>,
    rigs: Vec<LegRig>,
}
/// A range contact as the picture uses it: the ankle point and the sole's
/// rotation, heading then tilt, the same composition as `footing::Sole`.
fn foot_of(c: &sim_core::gait::Contact) -> Contact {
    let f = |v: fx::Fx| v.to_f64_lossy() as f32;
    let normal = Vec3::new(f(c.normal.x), f(c.normal.y), f(c.normal.z))
        .try_normalize()
        .unwrap_or(Vec3::Y);
    Contact {
        pos: Vec3::new(f(c.pos.x), f(c.pos.y), f(c.pos.z)),
        rot: Quat::from_rotation_y(render::yaw_of(c.head))
            * Quat::from_rotation_arc(Vec3::Y, normal),
    }
}
fn feet_of(u: &RangeUnit) -> Vec<Option<Contact>> {
    u.motion
        .feet
        .iter()
        .map(|f| f.contact.as_ref().map(foot_of))
        .collect()
}
/// Share of the tick elapsed on this frame; a whole tick outside realtime.
fn fraction(scene: &Scene) -> f32 {
    if scene.realtime && !scene.paused {
        (scene.acc * 30.0).clamp(0.0, 1.0)
    } else {
        1.0
    }
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
    let t = fraction(scene);
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
    v.previous_feet = sim.units.iter().map(feet_of).collect();
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
    mut query: Query<(&Witness, &mut Transform)>,
) {
    if v.rigs.is_empty() {
        let b = voxel::unit::strider();
        v.rigs = gait::rigs_of(&b, 0.01, &render::model::segment_frames(&b, 0.01));
    }
    let rigs = &v.rigs;
    let t = fraction(&scene);
    let mut poses = Vec::new();
    let mut legs = Vec::new();
    for (i, u) in scene.sim.units.iter().enumerate() {
        let p = visible(u, v.previous.get(i), &scene);
        poses.push(p);
        let mut lp = Vec::new();
        // The range plants the feet (ADR-279); the picture interpolates
        // their contacts between ticks and poses the links with the same
        // two-link solve the simulation uses.
        if u.kind == RangeUnitKind::Strider {
            let rot = render::realtime::hull_rotation(p.up, p.yaw);
            for (j, rig) in rigs.iter().enumerate() {
                let Some(now) = u
                    .motion
                    .feet
                    .get(j)
                    .and_then(|f| f.contact.as_ref())
                    .map(foot_of)
                else {
                    break;
                };
                let was = v
                    .previous_feet
                    .get(i)
                    .and_then(|f| f.get(j))
                    .copied()
                    .flatten();
                let c = match was {
                    Some(w) if t < 1.0 => w.lerp(now, t),
                    _ => now,
                };
                let mut pose = rig.solve(rot.inverse() * (c.pos - p.pos), rot);
                pose.foot.rot = rot.inverse() * c.rot;
                lp.push(pose);
            }
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
