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
    pub battle_turrets: Vec<i32>,
    occupied: Vec<usize>,
    /// Reuse the merged occupancy allocation from the previous tick.
    scratch: Vec<usize>,
    #[cfg(test)]
    occupancy_rebuilds: usize,
    cached: Vec<Occupied>,
    window: Option<TerrainWindow>,
    rigs: Vec<LegRig>,
}
/// Keep the previous shadow cells so replacement actors remove old silhouettes.
pub fn reset(v: &mut Visual) {
    *v = Visual {
        occupied: std::mem::take(&mut v.occupied),
        window: v.window,
        ..default()
    };
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
pub fn before(sim: &BlastRange, v: &mut Visual, window: &render::gas::TerrainWindow) {
    v.turret = Some(sim.turret.angle);
    v.battle_turrets = sim.battle.actors.iter().map(|a| a.turret.angle).collect();
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
        .chain((!sim.battle_mode()).then_some(&sim.gun))
        .map(pose)
        .collect();
    v.previous_feet = sim
        .units
        .iter()
        .chain((!sim.battle_mode()).then_some(&sim.gun))
        .map(feet_of)
        .collect();
    if v.occupied.is_empty() {
        v.occupied = cells(sim, window);
    }
}
/// World cells the bodies occupy, for the terrain's shadow rays: the root
/// in its pose and every segment in the frame the simulation posed it in
/// (ADR-282), so a walker's legs and a tank's plates shade the ground
/// under them rather than only the high root far downsun.
struct Occupied {
    turret_delta: i32,
    pose: sim_core::damage::Frame,
    segments: Vec<sim_core::damage::Frame>,
    revision: u64,
    root_bits: Vec<u64>,
    segment_bits: Vec<Vec<u64>>,
    gone: Vec<bool>,
    /// World cells survive camera shifts; only posed geometry changes
    /// require transforming the body's voxels again.
    world_cells: Vec<[i32; 3]>,
    bounds: Option<([i32; 3], [i32; 3])>,
    cells: Vec<usize>,
}
impl Occupied {
    fn same_clip(&mut self, old: TerrainWindow, new: TerrainWindow) -> bool {
        if old.dims != new.dims {
            return false;
        }
        // Bounds are a camera optimization. Avoid adding a reduction to every
        // simulation tick when moving actors never need their clip compared.
        let (lo, hi) = *self.bounds.get_or_insert_with(|| {
            self.world_cells
                .iter()
                .fold(([i32::MAX; 3], [i32::MIN; 3]), |(lo, hi), p| {
                    (
                        std::array::from_fn(|a| lo[a].min(p[a])),
                        std::array::from_fn(|a| hi[a].max(p[a])),
                    )
                })
        });
        let contains = |w: TerrainWindow| w.holds(lo) && w.holds(hi);
        let excludes =
            |w: TerrainWindow| (0..3).any(|a| hi[a] < w.origin[a] || lo[a] >= w.span(a).end);
        // Toroidal addresses depend on dimensions and world position, not
        // the origin. Fully contained or fully excluded bodies keep theirs.
        (contains(old) && contains(new)) || (excludes(old) && excludes(new))
    }
    fn same_geometry(&self, u: &RangeUnit) -> bool {
        self.root_bits == u.body.vol.cols
            && self.gone == u.seg_gone
            && self.segment_bits.len() == u.body.segments.len()
            && self
                .segment_bits
                .iter()
                .zip(&u.body.segments)
                .all(|(bits, segment)| *bits == segment.body.vol.cols)
    }
}
/// Factor the exact fixed-point cell transform by axis. Divisions and basis
/// products are shared by a whole row, without changing their rounding/order.
struct CellFrame {
    base: sim_core::damage::V3,
    axes: [Vec<sim_core::damage::V3>; 3],
}
impl CellFrame {
    fn new(frame: &sim_core::damage::Frame, dims: [usize; 3]) -> Self {
        let basis = [frame.x, frame.y, frame.z];
        Self {
            base: frame.base,
            axes: std::array::from_fn(|a| {
                (0..dims[a])
                    .map(|v| {
                        basis[a].scale(
                            fx::Fx::from_int(v as i32)
                                .add(fx::Fx::from_ratio(1, 2))
                                .sub(frame.centre[a])
                                .div(frame.scale),
                        )
                    })
                    .collect()
            }),
        }
    }
    fn world(&self, p: [usize; 3]) -> sim_core::damage::V3 {
        self.base
            .add(self.axes[0][p[0]])
            .add(self.axes[1][p[1]])
            .add(self.axes[2][p[2]])
    }
}
fn unit_cells(u: &RangeUnit, window: &TerrainWindow, out: &mut Vec<usize>) {
    unit_world_cells(u, |q| {
        if let Some(i) = window.index(q.to_array()) {
            out.push(i);
        }
    });
}
fn unit_world_cells(u: &RangeUnit, mut push: impl FnMut(IVec3)) {
    let p = pose(u);
    let rot = render::realtime::hull_rotation(p.up, p.yaw);
    let centre = Vec3::from(render::model::root_centre(&u.body));
    let turret = (u.turret_delta != 0).then(|| {
        CellFrame::new(
            &u.root_frame(usize::from(u.body.turret_y)),
            [
                u.body.vol.nx as usize,
                u.body.vol.ny as usize,
                u.body.vol.nz as usize,
            ],
        )
    });
    solid_cells(&u.body.vol, |[x, y, z]| {
        let q = if let Some(turret) = turret.as_ref().filter(|_| y >= u.body.turret_y as usize) {
            let w = turret.world([x, y, z]);
            IVec3::from([w.x, w.y, w.z].map(|v| v.to_int_trunc()))
        } else {
            (p.pos + rot * (Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5) - centre))
                .floor()
                .as_ivec3()
        };
        push(q);
    });
    for (i, (seg, frame)) in u.body.segments.iter().zip(&u.seg_frames).enumerate() {
        if u.seg_gone.get(i).copied().unwrap_or(false) {
            continue;
        }
        let b = &seg.body;
        let frame = CellFrame::new(
            frame,
            [b.vol.nx as usize, b.vol.ny as usize, b.vol.nz as usize],
        );
        solid_cells(&b.vol, |cell| {
            let w = frame.world(cell);
            push(IVec3::new(
                (w.x.raw() >> 32) as i32,
                (w.y.raw() >> 32) as i32,
                (w.z.raw() >> 32) as i32,
            ));
        });
    }
}
/// Enumerate occupied runs without visiting erased/empty body voxels.
fn solid_cells(vol: &voxel::topo::unit::UnitVol, mut visit: impl FnMut([usize; 3])) {
    for z in 0..vol.nz as usize {
        for y in 0..vol.ny as usize {
            for (word, &bits) in vol.col(y, z).iter().enumerate() {
                let mut bits = bits;
                while bits != 0 {
                    let x = word * 64 + bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    if x < vol.nx as usize {
                        visit([x, y, z]);
                    }
                }
            }
        }
    }
}

pub(super) fn cells(sim: &BlastRange, window: &TerrainWindow) -> Vec<usize> {
    let mut out = Vec::new();
    for u in sim
        .units
        .iter()
        .chain((!sim.battle_mode()).then_some(&sim.gun))
    {
        unit_cells(u, window, &mut out);
    }
    out.sort_unstable();
    out.dedup();
    out
}
fn cached_cells(sim: &BlastRange, window: &TerrainWindow, visual: &mut Visual) -> Vec<usize> {
    let previous_window = visual.window;
    let shifted = previous_window != Some(*window);
    if visual.cached.len() != sim.units.len() + usize::from(!sim.battle_mode()) {
        visual.cached.clear();
    }
    visual.window = Some(*window);
    let mut next = std::mem::take(&mut visual.scratch);
    next.clear();
    next.reserve(visual.occupied.len());
    for (id, u) in sim
        .units
        .iter()
        .chain((!sim.battle_mode()).then_some(&sim.gun))
        .enumerate()
    {
        let changed = visual.cached.get(id).is_none_or(|old| {
            old.turret_delta != u.turret_delta
                || old.pose != u.pose
                || old.segments != u.seg_frames
                // Dents/heat update appearance but keep occupancy. Compare
                // the small packed geometry only when its revision changed.
                || (old.revision != u.revision && !old.same_geometry(u))
        });
        if changed {
            #[cfg(test)]
            {
                visual.occupancy_rebuilds += 1;
            }
            let mut world_cells = visual
                .cached
                .get_mut(id)
                .map_or_else(Vec::new, |old| std::mem::take(&mut old.world_cells));
            world_cells.clear();
            unit_world_cells(u, |p| world_cells.push(p.to_array()));
            if id == visual.cached.len() {
                visual.cached.push(Occupied {
                    turret_delta: u.turret_delta,
                    pose: u.pose,
                    segments: u.seg_frames.clone(),
                    revision: u.revision,
                    root_bits: u.body.vol.cols.clone(),
                    segment_bits: u
                        .body
                        .segments
                        .iter()
                        .map(|s| s.body.vol.cols.clone())
                        .collect(),
                    gone: u.seg_gone.clone(),
                    world_cells,
                    bounds: None,
                    cells: Vec::new(),
                });
            } else {
                let old = &mut visual.cached[id];
                old.turret_delta = u.turret_delta;
                old.pose = u.pose;
                old.segments.clone_from(&u.seg_frames);
                // Turning or driving changes the shadow cells, but leaves
                // packed body geometry intact. Preserve its cached vectors
                // until a damage revision actually changes.
                if old.revision != u.revision {
                    old.root_bits.clone_from(&u.body.vol.cols);
                    if old.segment_bits.len() != u.body.segments.len() {
                        old.segment_bits
                            .resize_with(u.body.segments.len(), Vec::new);
                    }
                    for (bits, segment) in old.segment_bits.iter_mut().zip(&u.body.segments) {
                        bits.clone_from(&segment.body.vol.cols);
                    }
                    old.gone.clone_from(&u.seg_gone);
                }
                old.revision = u.revision;
                old.world_cells = world_cells;
                old.bounds = None;
            }
        }
        if changed
            || (shifted
                && previous_window.is_none_or(|old| !visual.cached[id].same_clip(old, *window)))
        {
            let cached = &mut visual.cached[id];
            cached.cells.clear();
            cached
                .cells
                .extend(cached.world_cells.iter().filter_map(|&p| window.index(p)));
        }
        visual.cached[id].revision = u.revision;
        next.extend_from_slice(&visual.cached[id].cells);
    }
    next.sort_unstable();
    next.dedup();
    next
}
pub fn occupancy(sim: &BlastRange, gas: &mut GasInput, v: &mut Visual) {
    let window = gas.window;
    gas.terrain_column_top
        .resize((window.dims[0] * window.dims[2]) as usize, 0);
    // The moving bodies share the terrain volume. Keep its conservative top
    // above their highest occupied cell so a later toroidal window shift can
    // clear old shadows without scanning the rest of the empty sky.
    for unit in &sim.units {
        let (_, hi) = unit.bounds();
        gas.params.terrain_origin.w = gas
            .params
            .terrain_origin
            .w
            .max((hi[1] + 1).min(window.span(1).end) as f32);
    }
    let shifted = v.window != Some(window);
    let next = cached_cells(sim, &window, v);
    if !shifted && next == v.occupied {
        v.scratch = next;
        return;
    }
    let mut dirty = Vec::new();
    let words = Arc::make_mut(&mut gas.terrain_words);
    let tops = &mut gas.terrain_column_top;
    let mut set = |i: usize, m: u32| {
        if m == 15 {
            let x = i % window.dims[0] as usize;
            let z = i / (window.dims[0] * window.dims[1]) as usize;
            let slot = x + z * window.dims[0] as usize;
            tops[slot] = tops[slot].max((window.position(i)[1] + 1) as u16);
        }
        let shift = (i % 8) * 4;
        let word = &mut words[i / 8];
        let new = (*word & !(15 << shift)) | (m << shift);
        if new == *word {
            return;
        }
        *word = new;
        dirty.push(i / 8);
    };
    // Both lists are sorted sets. Interior cells shared by the old and new
    // pose need neither a material lookup nor a GPU upload.
    let (mut old, mut new) = (0, 0);
    while old < v.occupied.len() || new < next.len() {
        let a = v.occupied.get(old).copied().unwrap_or(usize::MAX);
        let b = next.get(new).copied().unwrap_or(usize::MAX);
        if a == b {
            if shifted {
                set(b, 15);
            }
            old += 1;
            new += 1;
        } else if a < b {
            let p = window.position(a);
            set(
                a,
                surface_material(sim, p, sim_core::blast::material(&sim.world, p)),
            );
            old += 1;
        } else {
            set(b, 15);
            new += 1;
        }
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
    v.scratch = std::mem::replace(&mut v.occupied, next);
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
        let yaw = if w.3 && scene.sim.battle_mode() {
            {
                let angle = scene.sim.battle.actors[w.0].turret.angle;
                let was = v.battle_turrets.get(w.0).copied().unwrap_or(angle);
                -(was as f32 + fx::angle::diff(angle, was) as f32 * t) * std::f32::consts::TAU
                    / fx::angle::FULL as f32
            }
        } else {
            p.yaw
        };
        let rotation = render::realtime::hull_rotation(p.up, yaw);
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

/// Player links use the same foot interpolation as target walkers.
pub fn gun_segments(scene: &Scene, v: &Visual, p: Pose) -> Vec<Transform> {
    let u = &scene.sim.gun;
    if u.kind != RangeUnitKind::Strider {
        return Vec::new();
    }
    let root = render::realtime::hull_rotation(p.up, p.yaw);
    let frames = render::model::segment_frames(&u.body, 0.01);
    let rigs = gait::rigs_of(&u.body, 0.01, &frames);
    let mut out: Vec<_> = frames
        .iter()
        .map(|f| Transform::from_translation(p.pos + root * f.pos).with_rotation(root * f.rot))
        .collect();
    for (i, rig) in rigs.iter().enumerate() {
        let Some(now) = u
            .motion
            .feet
            .get(i)
            .and_then(|f| f.contact.as_ref())
            .map(foot_of)
        else {
            continue;
        };
        let old = v
            .previous_feet
            .last()
            .and_then(|f| f.get(i))
            .copied()
            .flatten();
        let c = old.map_or(now, |was| was.lerp(now, fraction(scene)));
        let mut solved = rig.solve(root.inverse() * (c.pos - p.pos), root);
        solved.foot.rot = root.inverse() * c.rot;
        for (seg, tr) in out.iter_mut().enumerate() {
            if let Some(role) = rig.role(seg) {
                let f = solved.part(role);
                *tr = Transform::from_translation(p.pos + root * f.pos).with_rotation(root * f.rot);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_shadow_cells_match_multiword_volume_after_damage() {
        let mut v = voxel::topo::unit::UnitVol::new(139, 7, 5);
        let mut expected = Vec::new();
        for z in 0..5 {
            for y in 0..7 {
                for x in 0..139 {
                    if (x + 7 * y + 11 * z) % 5 == 0 || [0, 63, 64, 127, 128, 138].contains(&x) {
                        v.set(x, y, z, true);
                        expected.push([x, y, z]);
                    }
                }
            }
        }
        let mut actual = Vec::new();
        solid_cells(&v, |p| actual.push(p));
        assert_eq!(actual, expected);
        v.cols.fill(0);
        actual.clear();
        solid_cells(&v, |p| actual.push(p));
        assert!(actual.is_empty());
    }

    #[test]
    fn factored_cell_frames_match_fixed_point_transforms_exactly() {
        use fx::Fx;
        for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
            let mut u = RangeUnit::of_kind(kind, [-73, 19, -21]);
            u.motion.drive.head = fx::angle::FULL / 7;
            u.motion.hull.up =
                sim_core::damage::V3::new(Fx::from_ratio(1, 7), Fx::ONE, Fx::from_ratio(-1, 5));
            u.update_pose(-fx::angle::FULL / 9);
            for (body, source) in std::iter::once((&u.body, u.root_frame(u.body.turret_y as usize)))
                .chain(
                    u.body
                        .segments
                        .iter()
                        .zip(&u.seg_frames)
                        .map(|(s, f)| (&s.body, *f)),
                )
            {
                let dims = [
                    body.vol.nx as usize,
                    body.vol.ny as usize,
                    body.vol.nz as usize,
                ];
                let factored = CellFrame::new(&source, dims);
                for z in 0..dims[2] {
                    for y in 0..dims[1] {
                        for x in 0..dims[0] {
                            let p = [x, y, z];
                            assert_eq!(
                                factored.world(p),
                                source.world(
                                    p.map(|v| Fx::from_int(v as i32).add(Fx::from_ratio(1, 2)))
                                )
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn battle_occupancy_reuses_unchanged_units_and_invalidates_damage_and_aim() {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap();
        layout.battle = true;
        let mut sim = BlastRange::with_layout(config(false, false), layout).unwrap();
        let window = TerrainWindow::around(sim.size(), [320, 96, 320], [640, 192, 640], 64);
        let mut v = Visual::default();
        assert_eq!(cached_cells(&sim, &window, &mut v), cells(&sim, &window));
        let initial = v.occupancy_rebuilds;
        assert_eq!(initial, sim.units.len());
        assert_eq!(cached_cells(&sim, &window, &mut v), cells(&sim, &window));
        assert_eq!(
            v.occupancy_rebuilds, initial,
            "unchanged battle re-created every shadow"
        );
        sim.units[0].revision += 1; // appearance-only dent, no removed voxel
        assert_eq!(cached_cells(&sim, &window, &mut v), cells(&sim, &window));
        assert_eq!(
            v.occupancy_rebuilds, initial,
            "a dent rebuilt unchanged occupancy"
        );
        sim.units[0].update_pose(fx::angle::FULL / 4);
        assert_eq!(cached_cells(&sim, &window, &mut v), cells(&sim, &window));
        assert_eq!(v.occupancy_rebuilds, initial + 1);
        sim.units[0].body.vol.cols.fill(0);
        sim.units[0].revision += 1;
        assert_eq!(cached_cells(&sim, &window, &mut v), cells(&sim, &window));
        assert_eq!(v.occupancy_rebuilds, initial + 2);
    }

    #[test]
    fn camera_shifts_reclip_shadows_without_reposing_stationary_bodies() {
        let layout =
            RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap();
        let mut sim = BlastRange::with_layout(config(false, false), layout).unwrap();
        let mut visual = Visual::default();
        let windows = [
            TerrainWindow {
                origin: [96, 0, 256],
                dims: [256, 192, 256],
            },
            TerrainWindow {
                origin: [104, 0, 264],
                dims: [256, 192, 256],
            },
            TerrainWindow {
                origin: [360, 0, 264],
                dims: [256, 192, 256],
            },
            TerrainWindow {
                origin: [0, 0, 0],
                dims: [64, 64, 64],
            },
            TerrainWindow {
                origin: [96, 0, 256],
                dims: [256, 192, 256],
            },
        ];
        for window in &windows {
            assert_eq!(cached_cells(&sim, window, &mut visual), cells(&sim, window));
            assert_eq!(visual.occupancy_rebuilds, sim.units.len() + 1);
        }
        // A body damaged while out of view must not return with old shadows.
        let outside = windows[3];
        sim.units[0].body.vol.cols.fill(0);
        sim.units[0].seg_gone.fill(true);
        sim.units[0].revision += 1;
        assert_eq!(
            cached_cells(&sim, &outside, &mut visual),
            cells(&sim, &outside)
        );
        assert_eq!(
            cached_cells(&sim, &windows[0], &mut visual),
            cells(&sim, &windows[0])
        );
        assert_eq!(visual.occupancy_rebuilds, sim.units.len() + 2);
    }

    #[test]
    fn moving_shadows_equal_a_fresh_volume_in_a_shifted_window() {
        let layout =
            RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap();
        let mut sim =
            range_sim(scaled_config(true, false, 1000), Some(&layout), false, None).unwrap();
        sim.control();
        sim.order_move([235, 365]);
        let window = TerrainWindow {
            origin: [96, 0, 256],
            dims: [256, 192, 256],
        };
        let mut gas = GasInput::default();
        gas.terrain_words = Arc::new(vec![0; window.words()]);
        write_window(&sim, &mut gas, window, None);
        let mut visual = Visual {
            occupied: cells(&sim, &window),
            ..default()
        };
        for _ in 0..8 {
            sim.step();
            occupancy(&sim, &mut gas, &mut visual);
        }
        // Cached stationary units must invalidate on damage as well as pose.
        sim.units[0].body.vol.cols.fill(0);
        sim.units[0].seg_gone.fill(true);
        sim.units[0].revision += 1;
        occupancy(&sim, &mut gas, &mut visual);
        let mut reference = GasInput::default();
        reference.terrain_words = Arc::new(vec![0; window.words()]);
        write_window(&sim, &mut reference, window, None);
        assert_eq!(gas.terrain_words, reference.terrain_words);
        for origin in [[104, 0, 264], [360, 0, 264], [96, 0, 256], [96, 0, 256]] {
            let had = gas.window;
            let next = TerrainWindow { origin, ..window };
            gas.terrain_dirty.clear();
            let uploaded = gas.terrain_words.as_ref().clone();
            write_window(&sim, &mut gas, next, Some(had));
            occupancy(&sim, &mut gas, &mut visual);
            let mut delivered = uploaded;
            for span in &gas.terrain_dirty {
                delivered[span.clone()].copy_from_slice(&gas.terrain_words[span.clone()]);
            }
            assert_eq!(
                delivered, *gas.terrain_words,
                "missing GPU patch at {origin:?}"
            );
            if origin == [104, 0, 264] {
                let changed: usize = gas.terrain_dirty.iter().map(|r| r.len()).sum();
                assert!(
                    changed < next.words() / 4,
                    "a short shift uploaded the full window"
                );
            }
            let mut fresh = GasInput::default();
            fresh.terrain_words = Arc::new(vec![0; next.words()]);
            write_window(&sim, &mut fresh, next, None);
            assert_eq!(
                gas.terrain_words, fresh.terrain_words,
                "shift to {origin:?}"
            );
        }
    }
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
        let window =
            render::gas::TerrainWindow::around(sim.size(), [320, 96, 320], [640, 192, 640], 64);
        let legs = cells(&sim, &window)
            .into_iter()
            .filter(|&i| near(i))
            .count();
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
