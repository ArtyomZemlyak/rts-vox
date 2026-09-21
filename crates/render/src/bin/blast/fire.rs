//! Bounded cubic combustion: hot torn tongues, cooling embers and coarse smoke.
//! Pure presentation; body integrity and stream contacts come from sim-core.
use super::*;
use sim_core::blast::thermal::{temperature, Cell as HeatCell};
const CAP: usize = 1800;
#[derive(Component)]
pub(super) struct Layer(bool);
#[derive(Clone, Copy)]
struct Particle {
    at: Vec3,
    velocity: Vec3,
    born: f32,
    life: f32,
    edge: f32,
    seed: u32,
    smoke: bool,
}
#[derive(Resource, Default)]
pub struct Effects {
    particles: Vec<Particle>,
    fire: Option<Handle<Mesh>>,
    smoke: Option<Handle<Mesh>>,
    serial: u32,
    last_frame: Option<f32>,
    sources: Vec<(u64, Option<[usize; 3]>)>,
    heat_spots: Vec<(HeatCell, u32)>,
}
fn noise(n: u32) -> f32 {
    let n = (n ^ (n >> 16)).wrapping_mul(0x7feb352d);
    let n = (n ^ (n >> 15)).wrapping_mul(0x846ca68b);
    (n ^ (n >> 16)) as f32 / u32::MAX as f32
}
fn point(p: [fx::Fx; 3]) -> Vec3 {
    Vec3::from(p.map(|v| v.to_f64_lossy() as f32))
}
impl Effects {
    pub fn clear(&mut self) {
        self.particles.clear();
        self.sources.clear();
        self.heat_spots.clear();
        self.serial = 0;
        self.last_frame = None;
    }
    fn add(&mut self, at: Vec3, velocity: Vec3, now: f32, life: f32, edge: f32, smoke: bool) {
        if self.particles.len() >= CAP {
            return;
        }
        let seed = self.serial;
        self.serial = self.serial.wrapping_add(1);
        self.particles.push(Particle {
            at,
            velocity,
            born: now,
            life,
            edge,
            seed,
            smoke,
        });
    }
    fn plume(&mut self, at: Vec3, now: f32, fire: bool, scale: f32) {
        let seed = self.serial;
        let offset =
            Vec3::new(noise(seed) - 0.5, 0.0, noise(seed.wrapping_add(77)) - 0.5) * 6.0 * scale;
        if fire {
            self.add(
                at + offset,
                Vec3::new(0.6, 14.0 + noise(seed + 1) * 12.0, 1.1),
                now,
                0.5 + noise(seed + 2) * 0.65,
                (2.0 + noise(seed + 3) * 2.4) * scale,
                false,
            );
            if seed.is_multiple_of(5) {
                self.add(at + offset, Vec3::new(1.5, 30.0, 0.8), now, 1.4, 0.6, false);
            }
        }
        if seed.is_multiple_of(3) || !fire {
            self.add(
                at + offset + Vec3::Y * if fire { 11.0 } else { 1.5 },
                Vec3::new(2.4, 9.0, 1.2),
                now,
                3.5 + noise(seed + 8) * 2.5,
                (4.4 + noise(seed + 9) * 2.0) * scale,
                true,
            );
        }
    }
    pub fn capture(&mut self, sim: &BlastRange) {
        let now = sim.tick as f32 / 30.0;
        self.particles.retain(|p| now - p.born < p.life);
        let tuning = &sim.streams.tuning;
        self.sources.resize(sim.units.len() + 1, (u64::MAX, None));
        for (id, u) in sim
            .units
            .iter()
            .chain(std::iter::once(&sim.gun))
            .enumerate()
        {
            let dead = !u.status.alive;
            let removed = u.removed.len() + u.seg_removed.len();
            let remaining = u.body.solid_count() as usize
                + u.body
                    .segments
                    .iter()
                    .map(|s| s.body.solid_count() as usize)
                    .sum::<usize>();
            let damaged = u.integrity.engine < fx::Fx::from_ratio(tuning.smoke_engine_milli, 1000)
                || u.integrity.core < fx::Fx::from_ratio(tuning.smoke_core_milli, 1000)
                || removed * 1000 > (remaining + removed) * tuning.smoke_loss_milli as usize;
            if remaining == 0 || dead || !damaged || !sim.tick.is_multiple_of(3) {
                continue;
            }
            // Anchor to a surviving surface; erased bodies never leave a floating fire.
            if self.sources[id].0 != u.revision {
                let mut best = None;
                let mut nearest = i32::MAX;
                for z in 0..u.body.vol.nz as usize {
                    for x in 0..u.body.vol.nx as usize {
                        let dx = x as i32 - i32::from(u.body.core.0) + 5;
                        let dz = z as i32 - i32::from(u.body.core.2);
                        let distance = dx * dx + dz * dz;
                        if distance >= nearest {
                            continue;
                        }
                        if let Some(y) = (0..u.body.vol.ny as usize)
                            .rev()
                            .find(|&y| u.body.vol.get(x, y, z))
                        {
                            best = Some([x, y, z]);
                            nearest = distance;
                        }
                    }
                }
                self.sources[id] = (u.revision, best);
            }
            let surface = self.sources[id].1;
            if let Some(p) = surface {
                let at = Vec3::from(u.world_pos(p).map(|v| v as f32)) + Vec3::Y;
                self.plume(at, now, false, 0.85);
            }
        }
        for source in sim.combustion.sources(&sim.units, &sim.matter) {
            let mut at = point(source.at);
            if let Some(id) = source.unit {
                // Internal compartment fire vents at the roof above the actual
                // reservoir; released fuel burns at its physical spill position.
                let u = &sim.units[id];
                let [x, _, z] = sim.combustion.tuning.reservoir(u.kind).at.map(usize::from);
                if let Some(y) = (0..u.body.vol.ny as usize)
                    .rev()
                    .find(|&y| u.body.vol.get(x, y, z))
                {
                    let p = u.pose.world([
                        fx::Fx::from_ratio((x * 2 + 1) as i32, 2),
                        fx::Fx::from_int(y as i32 + 1),
                        fx::Fx::from_ratio((z * 2 + 1) as i32, 2),
                    ]);
                    at = point([p.x, p.y, p.z]);
                }
            }
            let scale = (source.intensity_milli as f32 / 1000.0).sqrt();
            self.plume(at, now, true, 1.6 * scale);
            if source.intensity_milli >= 500 {
                self.plume(at + Vec3::new(2.5, -0.5, 2.0), now, true, scale);
            }
        }
        self.heat_spots.clear();
        let mut warmest = vec![None; sim.units.len()];
        for (cell, heat) in sim.thermals.cells() {
            let Some(body) = cell.body(&sim.units) else {
                continue;
            };
            let temp = temperature(heat, body.mm);
            if temp >= sim.thermal_tuning.glow {
                self.heat_spots.push((cell, temp));
            }
            let hottest = &mut warmest[usize::from(cell.unit)];
            if temp >= sim.thermal_tuning.smoke && hottest.is_none_or(|(_, h)| temp > h) {
                *hottest = Some((cell, temp));
            }
        }
        self.heat_spots
            .sort_by_key(|(p, h)| (std::cmp::Reverse(*h), *p));
        self.heat_spots.truncate(128);
        if sim.tick.is_multiple_of(3) {
            for (cell, _) in warmest.into_iter().flatten() {
                if let Some(at) = cell.world(&sim.units) {
                    self.plume(point(at), now, false, 0.65);
                }
            }
        }
        // Spread terrain sources over the burning set, bounded independently of its size.
        for p in sim
            .heat
            .burning_cells()
            .skip((sim.tick as usize * 4) % sim.heat.burning().max(1))
            .take(4)
        {
            self.plume(Vec3::from(p.map(|v| v as f32)) + Vec3::Y, now, true, 0.9);
        }
        for trace in &sim.streams.traces {
            let (a, b) = (point(trace.from), point(trace.to));
            if sim.config.delivery == Delivery::Flame {
                let length = a.distance(b);
                let steps = (length / 5.0).ceil().clamp(1.0, 3.0) as usize;
                for i in 0..steps {
                    let t = (i as f32 + 0.5) / steps as f32;
                    // Wake curls upward after the packet, but cannot extend past its contact.
                    self.add(
                        a.lerp(b, t),
                        Vec3::new(0.5, 6.0, 0.3),
                        now,
                        0.28 + noise(trace.serial) * 0.25,
                        2.4 + noise(trace.serial.wrapping_add(i as u32)) * 2.6,
                        false,
                    );
                }
                if trace.hit && trace.serial.is_multiple_of(3) {
                    self.plume(b, now, true, 0.9);
                }
            } else {
                // Short luminous pieces show flight direction without drawing a permanent beam.
                for i in 0..4 {
                    self.add(
                        b.lerp(a, i as f32 * 0.06),
                        Vec3::ZERO,
                        now,
                        0.075,
                        0.65,
                        false,
                    );
                }
                if trace.hit {
                    for i in 0..5 {
                        let s = trace.serial.wrapping_add(i);
                        self.add(
                            b,
                            Vec3::new(
                                noise(s) * 22.0 - 11.0,
                                8.0 + noise(s + 1) * 18.0,
                                noise(s + 2) * 22.0 - 11.0,
                            ),
                            now,
                            0.25,
                            0.55,
                            false,
                        );
                    }
                }
            }
        }
        if sim.config.delivery == Delivery::MachineGun {
            for at in sim.stream_muzzles() {
                if !sim.streams.traces.iter().any(|t| t.from == at) {
                    continue;
                }
                self.add(point(at), Vec3::ZERO, now, 0.09, 3.8, false);
            }
        }
    }
}
fn cube(buf: &mut MeshBuf, at: Vec3, edge: f32, col: [f32; 4]) {
    for axis in 0..3 {
        for positive in [false, true] {
            let k = if positive { 1.0 } else { -1.0 };
            let mut normal = Vec3::ZERO;
            normal[axis] = k;
            let mut a = Vec3::ZERO;
            a[(axis + 1) % 3] = edge * k;
            let mut b = Vec3::ZERO;
            b[(axis + 2) % 3] = edge;
            let centre = at + normal * edge * 0.5;
            let start = buf.pos.len() as u32;
            for q in [
                centre - a * 0.5 - b * 0.5,
                centre + a * 0.5 - b * 0.5,
                centre + a * 0.5 + b * 0.5,
                centre - a * 0.5 + b * 0.5,
            ] {
                buf.pos.push(q.to_array());
                buf.nrm.push(normal.to_array());
                let shade = match axis {
                    1 if positive => 1.0,
                    0 => 0.82,
                    _ => 0.64,
                };
                buf.col
                    .push([col[0] * shade, col[1] * shade, col[2] * shade, col[3]]);
            }
            buf.idx
                .extend([start, start + 1, start + 2, start, start + 2, start + 3]);
        }
    }
}
#[allow(clippy::too_many_arguments)] // Independent Bevy resources and read-only pose queries.
pub fn draw(
    mut commands: Commands,
    scene: Res<Scene>,
    mut fx: ResMut<Effects>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    camera: Query<&Transform, With<Camera3d>>,
    bodies: Query<(&Witness, &Transform)>,
    mut layers: Query<(&Layer, &mut Visibility)>,
) {
    if fx.fire.is_none() {
        for smoke in [false, true] {
            let handle = meshes.add(fire_mesh(&MeshBuf::default()));
            let material = materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: !smoke,
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 1.0,
                cull_mode: None,
                ..default()
            });
            commands.spawn((
                Mesh3d(handle.clone()),
                MeshMaterial3d(material),
                Transform::IDENTITY,
                NoFrustumCulling,
                Layer(smoke),
                Visibility::Hidden,
                bevy::light::NotShadowCaster,
            ));
            if smoke {
                fx.smoke = Some(handle);
            } else {
                fx.fire = Some(handle);
            }
        }
    }
    let now = (scene.sim.tick as f32
        + if scene.paused {
            0.0
        } else {
            (scene.acc * 30.0).clamp(0.0, 1.0)
        })
        / 30.0;
    if fx.last_frame == Some(now) {
        return;
    }
    fx.last_frame = Some(now);
    let eye = camera.iter().next().map_or(Vec3::ZERO, |t| t.translation);
    let mut particles = Vec::with_capacity(fx.particles.len());
    for p in &fx.particles {
        let age = (now - p.born).max(0.0);
        let t = age / p.life;
        if t >= 1.0 {
            continue;
        }
        let n = noise(p.seed);
        let angle = age * 3.0 + n * 20.0;
        let wobble = Vec3::new(angle.sin(), 0.0, (angle * 0.83).cos())
            * age
            * if p.smoke { 2.5 } else { 1.6 };
        let at = p.at + p.velocity * age + wobble;
        particles.push((p, at, t));
    }
    particles.sort_by(|a, b| {
        eye.distance_squared(b.1)
            .total_cmp(&eye.distance_squared(a.1))
    });
    let mut hot = MeshBuf::default();
    let mut smoke = MeshBuf::default();
    for (p, at, t) in particles {
        if p.smoke {
            let fade = (t * 8.0).min(1.0) * (1.0 - t).powi(2);
            let shade = 0.035 + t * 0.09 + noise(p.seed) * 0.025;
            cube(
                &mut smoke,
                at,
                p.edge * (0.65 + t * 1.65),
                [shade * 0.94, shade * 0.96, shade, fade * 0.30],
            );
        } else {
            let flicker = 0.85 + noise(p.seed.wrapping_add((t * 12.0) as u32)) * 0.15;
            let col = if t < 0.23 {
                [5.8, 3.2, 0.55, 0.88]
            } else if t < 0.58 {
                [3.2, 0.75, 0.035, 0.82]
            } else {
                [1.3, 0.12, 0.009, (1.0 - t) * 1.8]
            };
            let edge = p.edge * (0.8 + (t * 3.0).sin() * 0.2) * (1.0 - t * 0.55) * flicker;
            cube(&mut hot, at, edge, col);
        }
    }
    // Use exactly the interpolated transforms already applied to visible meshes,
    // including solved legs. Current simulation poses can lead the picture by a tick.
    let mut poses = std::collections::BTreeMap::new();
    if !fx.heat_spots.is_empty() {
        for (w, tr) in &bodies {
            poses.entry((w.0, w.2)).or_insert(*tr);
        }
    }
    for &(cell, heat) in &fx.heat_spots {
        let id = usize::from(cell.unit);
        let seg = cell.part.checked_sub(1).map(usize::from);
        let Some(tr) = poses.get(&(id, seg)) else {
            continue;
        };
        let Some(body) = cell.body(&scene.sim.units) else {
            continue;
        };
        let level = (heat as f32 / scene.sim.thermal_tuning.glow as f32 - 1.0).clamp(0.0, 3.0);
        let start = hot.pos.len();
        let unit = &scene.sim.units[id];
        let pivot = if let Some(seg) = seg {
            render::model::pivot_centre(&unit.body.segments[seg].attach)
        } else {
            render::model::root_centre(body)
        };
        let local = (Vec3::from(cell.at.map(|v| v as f32)) + Vec3::splat(0.5) - Vec3::from(pivot))
            * (body.mm as f32 / voxel::unit::WORLD_MM as f32);
        let centre = tr.translation + tr.rotation * local;
        cube(
            &mut hot,
            centre,
            body.mm as f32 / voxel::unit::WORLD_MM as f32 * 1.06,
            [0.8 + level * 1.4, 0.025 + level * 0.12, 0.006, 0.65],
        );
        let rotation = tr.rotation;
        for p in &mut hot.pos[start..] {
            *p = (centre + rotation * (Vec3::from(*p) - centre)).to_array();
        }
        for n in &mut hot.nrm[start..] {
            *n = (rotation * Vec3::from(*n)).to_array();
        }
    }
    for (layer, mut visible) in &mut layers {
        *visible = if if layer.0 {
            smoke.is_empty()
        } else {
            hot.is_empty()
        } {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
    if !hot.is_empty() {
        if let Some(mut m) = fx.fire.as_ref().and_then(|h| meshes.get_mut(h)) {
            *m = fire_mesh(&hot);
        }
    }
    if !smoke.is_empty() {
        if let Some(mut m) = fx.smoke.as_ref().and_then(|h| meshes.get_mut(h)) {
            *m = fire_mesh(&smoke);
        }
    }
}

fn fire_mesh(buf: &MeshBuf) -> Mesh {
    // An empty Bevy 0.19 mesh has no allocator key. Keep the last nonempty
    // mesh hidden between bursts; the initial hidden mesh is a tiny cube.
    let mut placeholder = MeshBuf::default();
    if buf.is_empty() {
        cube(
            &mut placeholder,
            Vec3::new(0.0, -1000.0, 0.0),
            0.01,
            [0.0; 4],
        );
    }
    let mut m = mesh(if buf.is_empty() { &placeholder } else { buf });
    m.asset_usage = RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD;
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn machine_gun_scene_emits_both_flashes_at_its_own_launch_time() {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap();
        layout.gun_kind = RangeUnitKind::Strider;
        let cfg = weapon_config(false, false, 1000, Delivery::MachineGun);
        let fire = cfg.fire_tick;
        let mut r = range_sim(cfg, Some(&layout), false, None).unwrap();
        assert_eq!(r.fire_tick, fire);
        let mut effects = Effects::default();
        for _ in 0..fire {
            r.step();
            effects.capture(&r);
        }
        assert!(r.streams.traces.is_empty());
        r.step();
        effects.capture(&r);
        assert_eq!(r.streams.traces.len(), 2);
        for at in r.stream_muzzles() {
            assert!(effects
                .particles
                .iter()
                .any(|p| p.at == point(at) && p.edge == 3.8));
        }
    }
    #[test]
    fn hot_metal_glows_and_smokes_without_structural_damage() {
        let mut r = BlastRange::new(config(true, false));
        let body = &r.units[0].body;
        let mut at = [0; 3];
        'outer: for z in 0..body.vol.nz {
            for y in 0..body.vol.ny {
                for x in 0..body.vol.nx {
                    if body.vol.get(x as usize, y as usize, z as usize) {
                        at = [x, y, z];
                        break 'outer;
                    }
                }
            }
        }
        let cell = HeatCell {
            unit: 0,
            part: 0,
            at,
        };
        r.thermals.deposit(cell, 250, &r.thermal_tuning);
        r.tick = 3;
        let mut effects = Effects::default();
        effects.capture(&r);
        assert_eq!(effects.heat_spots, vec![(cell, 250)]);
        assert!(!effects.particles.is_empty());
        assert!(effects.particles.iter().all(|p| p.smoke));
        assert_eq!(r.units[0].revision, 0);
        effects.clear();
        assert!(effects.heat_spots.is_empty());
    }
    #[test]
    fn flame_cubes_have_outward_faces_and_keep_cpu_mesh_for_updates() {
        let mut b = MeshBuf::default();
        cube(&mut b, Vec3::ZERO, 4.0, [2.0, 0.5, 0.1, 0.7]);
        assert_eq!(b.pos.len(), 24);
        assert_eq!(b.idx.len(), 36);
        for t in b.idx.as_chunks::<3>().0 {
            let [a, c, d] = [t[0], t[1], t[2]].map(|i| Vec3::from(b.pos[i as usize]));
            assert!((c - a).cross(d - a).dot(Vec3::from(b.nrm[t[0] as usize])) > 0.0);
        }
        assert!(fire_mesh(&b)
            .asset_usage
            .contains(RenderAssetUsages::MAIN_WORLD));
    }
    #[test]
    fn healthy_damaged_and_destroyed_units_have_distinct_effects() {
        let mut r = BlastRange::new(config(true, false));
        let mut fx = Effects::default();
        fx.capture(&r);
        assert!(fx.particles.is_empty());
        r.units[0].integrity.engine = fx::Fx::from_ratio(1, 2);
        r.tick = 3;
        fx.capture(&r);
        assert!(!fx.particles.is_empty());
        assert!(fx.particles.iter().all(|p| p.smoke));
        fx.clear();
        r.units[0].status.alive = false;
        fx.capture(&r);
        assert!(
            fx.particles.is_empty(),
            "death alone must not create chemical fuel fire"
        );
        let at = r.combustion.tuning.reservoir(r.units[0].kind).at;
        r.thermals.deposit(
            HeatCell {
                unit: 0,
                part: 0,
                at,
            },
            200,
            &r.thermal_tuning,
        );
        r.control();
        for _ in 4..30 {
            r.step();
            fx.capture(&r);
        }
        assert!(fx.particles.iter().any(|p| !p.smoke));
        assert!(fx.particles.iter().any(|p| p.smoke && p.edge >= 4.0));
        assert!(fx.particles.len() < CAP);
        fx.clear();
        assert!(fx.particles.is_empty());
    }
}
