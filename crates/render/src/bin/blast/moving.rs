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
/// World cells the bodies occupy, for the terrain's shadow rays: the root
/// in its pose and every segment in the frame the simulation posed it in
/// (ADR-282), so a walker's legs and a tank's plates shade the ground
/// under them rather than only the high root far downsun.
pub(super) fn cells(sim: &BlastRange) -> Vec<usize> {
    let mut out = Vec::new();
    let size = sim.size().map(|v| v as i32);
    let mut push = |q: IVec3| {
        if q.cmpge(IVec3::ZERO).all() && q.cmplt(IVec3::from_array(size)).all() {
            out.push((q.x + size[0] * (q.y + size[1] * q.z)) as usize);
        }
    };
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
                    push(q);
                }
            }
        }
        let half = fx::Fx::from_ratio(1, 2);
        for (i, (seg, frame)) in u.body.segments.iter().zip(&u.seg_frames).enumerate() {
            if u.seg_gone.get(i).copied().unwrap_or(false) {
                continue;
            }
            let b = &seg.body;
            for z in 0..b.vol.nz as usize {
                for y in 0..b.vol.ny as usize {
                    for x in 0..b.vol.nx as usize {
                        if !b.vol.get(x, y, z) {
                            continue;
                        }
                        let w =
                            frame.world([x, y, z].map(|v| fx::Fx::from_int(v as i32).add(half)));
                        push(IVec3::new(
                            (w.x.raw() >> 32) as i32,
                            (w.y.raw() >> 32) as i32,
                            (w.z.raw() >> 32) as i32,
                        ));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The shadow occupancy holds a walker's legs down to the ground, not
    /// only its root high above it (ADR-282).
    #[test]
    fn a_walker_shades_the_ground_with_its_legs() {
        let sim = BlastRange::with_layout(
            ExplosionConfig::from_ron(include_str!("../../../../../data/energy-cannon.ron"))
                .unwrap(),
            RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap(),
        )
        .unwrap();
        let (walker, u) = sim
            .units
            .iter()
            .enumerate()
            .find(|(_, u)| u.kind == RangeUnitKind::Strider)
            .expect("the cover range has a walker");
        let _ = walker;
        let low = u.at[1] + 12;
        let near = |i: usize| {
            let (x, y, z) = (
                (i % 640) as i32,
                (i / 640 % 192) as i32,
                (i / 640 / 192) as i32,
            );
            y < low && (x - u.at[0]).abs() <= 16 && (z - u.at[2]).abs() <= 16
        };
        let legs = cells(&sim).into_iter().filter(|&i| near(i)).count();
        assert!(legs > 500, "only {legs} occupied cells under the walker");
        // The root alone reaches no lower than its belly.
        let root_low = (0..u.body.vol.ny as usize)
            .find(|&y| {
                (0..u.body.vol.nz as usize)
                    .any(|z| (0..u.body.vol.nx as usize).any(|x| u.body.vol.get(x, y, z)))
            })
            .unwrap();
        assert!(
            root_low as i32 >= 12,
            "the root itself reaches y {root_low}"
        );
    }
}
