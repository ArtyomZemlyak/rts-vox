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
    stream: bool,
    dust_material: u16,
}
#[derive(Resource, Default)]
pub struct Effects {
    particles: Vec<Particle>,
    fire: Option<Handle<Mesh>>,
    smoke: Option<Handle<Mesh>>,
    serial: u32,
    last_frame: Option<f32>,
    last_dust_tick: Option<u32>,
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
        self.last_dust_tick = None;
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
            stream: false,
            dust_material: 0,
        });
    }
    fn add_dust(&mut self, at: Vec3, velocity: Vec3, now: f32, life: f32, edge: f32, mat: u16) {
        // Track dust has its own budget and never fills the slots reserved
        // for impacts, flame and damage smoke.
        if self.particles.len() >= CAP - 100 {
            return;
        }
        let before = self.particles.len();
        self.add(at, velocity, now, life, edge, true);
        if self.particles.len() > before {
            self.particles[before].dust_material = mat;
        }
    }
    fn add_stream(&mut self, at: Vec3, velocity: Vec3, now: f32, life: f32, edge: f32) {
        let before = self.particles.len();
        self.add(at, velocity, now, life, edge, false);
        if self.particles.len() > before {
            self.particles[before].stream = true;
        }
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
    fn shell_burst(&mut self, at: Vec3, now: f32, energy: u32) {
        // A full pool must not silently swallow a new impact. Particles are
        // stored in birth order, so recycle the oldest visual remnants first.
        let needed = self.particles.len().saturating_add(35).saturating_sub(CAP);
        if needed > 0 {
            self.particles.drain(..needed);
        }
        let scale = (energy as f32 / 2300.0).sqrt().clamp(0.55, 1.8);
        self.add(at, Vec3::ZERO, now, 0.16, 9.0 * scale, false);
        // Fast, short-lived flame tongues bridge the impact flash to the
        // slower advected gas. These are light and soot, not solid debris.
        for i in 0..24u32 {
            let seed = self.serial.wrapping_add(i * 13);
            let angle = std::f32::consts::TAU * (i as f32 + noise(seed) * 0.6) / 24.0;
            let outward = Vec3::new(angle.cos(), 0.0, angle.sin());
            let speed = (28.0 + noise(seed.wrapping_add(3)) * 34.0) * scale;
            self.add(
                at + outward * (1.0 + noise(seed.wrapping_add(5)) * 2.0),
                outward * speed + Vec3::Y * (9.0 + noise(seed.wrapping_add(7)) * 28.0),
                now,
                0.18 + noise(seed.wrapping_add(11)) * 0.17,
                (1.8 + noise(seed.wrapping_add(17)) * 2.3) * scale,
                false,
            );
        }
        for i in 0..10u32 {
            let seed = self.serial.wrapping_add(i * 19);
            let angle = std::f32::consts::TAU * (i as f32 + noise(seed) * 0.7) / 10.0;
            let outward = Vec3::new(angle.cos(), 0.0, angle.sin());
            self.add(
                at + outward * 2.0,
                outward * (12.0 + noise(seed.wrapping_add(2)) * 16.0) * scale
                    + Vec3::Y * (7.0 + noise(seed.wrapping_add(4)) * 10.0),
                now,
                1.0 + noise(seed.wrapping_add(6)) * 0.9,
                (3.0 + noise(seed.wrapping_add(8)) * 2.5) * scale,
                true,
            );
        }
    }
    pub fn capture(&mut self, sim: &BlastRange) {
        let now = sim.tick as f32 / 30.0;
        self.particles.retain(|p| now - p.born < p.life);
        for i in &sim.battle.impacts {
            let at = Vec3::from(i.at.map(|v| v as f32));
            if i.shell {
                self.shell_burst(at, now, sim.battle.shell_config.energy);
            } else {
                self.add(at, Vec3::Y * 3.0, now, 0.16, 1.8, false);
            }
        }
        if !sim.battle_mode()
            && sim.config.delivery == Delivery::Blast
            && sim.config.motion != modding::explosion::ProjectileMotion::Direct
            && sim.tick == sim.impact_tick.saturating_add(1)
        {
            if let Some(at) = sim.impact {
                self.shell_burst(Vec3::from(at.map(|v| v as f32)), now, sim.config.energy);
            }
        }
        if !sim.battle_mode()
            && sim.config.delivery == Delivery::Kinetic
            && sim.tick == sim.impact_tick.saturating_add(1)
        {
            if let Some(at) = sim.impact {
                let centre = Vec3::from(at.map(|v| v as f32));
                for i in 0..10u32 {
                    let seed = self.serial.wrapping_add(i * 17);
                    self.add(
                        centre,
                        Vec3::new(
                            noise(seed) * 26.0 - 13.0,
                            noise(seed + 1) * 18.0 + 3.0,
                            noise(seed + 2) * 26.0 - 13.0,
                        ),
                        now,
                        0.18 + noise(seed + 3) * 0.12,
                        0.55 + noise(seed + 4) * 0.8,
                        false,
                    );
                }
            }
        }
        let tuning = &sim.streams.tuning;
        self.sources.resize(sim.units.len() + 1, (u64::MAX, None));
        for (id, u) in sim
            .units
            .iter()
            .chain((!sim.battle_mode()).then_some(&sim.gun))
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
                let axis = trace.serial.is_multiple_of(3);
                let steps = if axis {
                    (length / 2.2).ceil().clamp(3.0, 7.0) as usize
                } else {
                    2
                };
                let direction = (b - a).normalize_or_zero();
                let side = Vec3::Y.cross(direction).normalize_or_zero();
                let up = (Vec3::Y - direction * direction.y).normalize_or_zero();
                let forward = if trace.hit {
                    Vec3::ZERO
                } else {
                    direction * 9.0
                };
                for i in 0..steps {
                    let seed = trace.serial.wrapping_add(i as u32 * 17);
                    let t = (i as f32 + 0.5) / steps as f32;
                    // The three physical rays are one visual jet: one ray
                    // carries its body; the others roughen the silhouette.
                    // Offsets stay across the travelled segment, so a hit
                    // never draws a new flame beyond the contact plane.
                    let curl = side * ((noise(seed) - 0.5) * 10.0)
                        + up * ((noise(seed.wrapping_add(3)) - 0.5) * 7.0);
                    self.add_stream(
                        a.lerp(b, t) + curl,
                        forward + side * 0.5 + up * 4.5,
                        now,
                        if axis { 0.23 } else { 0.11 } + noise(seed.wrapping_add(11)) * 0.16,
                        if axis { 2.9 } else { 1.25 }
                            + noise(seed.wrapping_add(13)) * if axis { 3.1 } else { 1.5 },
                    );
                    if axis && i.is_multiple_of(2) {
                        self.add_stream(
                            a.lerp(b, t) - curl * 0.45,
                            forward + up * 2.0,
                            now,
                            0.12 + noise(seed.wrapping_add(23)) * 0.11,
                            1.0 + noise(seed.wrapping_add(29)) * 1.2,
                        );
                    }
                }
                if trace.hit && axis {
                    self.plume(b, now, true, 0.9);
                }
            } else {
                // A brief broken tracer shows the faster, heavier round at
                // ordinary camera scale without turning its path into a beam.
                for i in 0..6 {
                    self.add(
                        b.lerp(a, i as f32 * 0.035),
                        Vec3::ZERO,
                        now,
                        0.11,
                        1.05,
                        false,
                    );
                }
                if trace.hit {
                    for i in 0..7 {
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
                            0.9,
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
                self.add(point(at), Vec3::ZERO, now, 0.11, 4.7, false);
            }
        }
        self.capture_dust(sim, now);
    }
    fn capture_dust(&mut self, sim: &BlastRange, now: f32) {
        let d = &sim.streams.tuning.dust;
        if sim.tick == 0
            || !sim.tick.is_multiple_of(d.interval_ticks)
            || self.last_dust_tick == Some(sim.tick)
        {
            return;
        }
        self.last_dust_tick = Some(sim.tick);
        let mut active = self
            .particles
            .iter()
            .filter(|p| p.dust_material != 0)
            .count();
        for u in sim
            .units
            .iter()
            .chain((!sim.battle_mode()).then_some(&sim.gun))
        {
            if u.kind != RangeUnitKind::Tank || !u.status.alive || !u.motion.hull.grounded() {
                continue;
            }
            let speed = u.motion.drive.vel.abs().to_f64_lossy() as f32;
            let turn = u.motion.drive.yaw_rate.abs() as f32;
            if speed < d.min_speed as f32 && turn < d.min_turn_rate as f32 {
                continue;
            }
            let motion = (speed / d.full_speed as f32)
                .max(turn / d.full_turn_rate as f32)
                .clamp(0.0, 1.0);
            let nx = i32::from(u.body.vol.nx);
            let nz = i32::from(u.body.vol.nz);
            let rear = fx::Fx::from_ratio(nx * d.rear_milli, 1000);
            let offset = fx::Fx::from_ratio(nz * d.lateral_milli, 1000);
            for sign in [-1.0f32, 1.0] {
                if active >= d.max_particles || self.particles.len() >= CAP - 100 {
                    return;
                }
                let z = fx::Fx::from_ratio(nz, 2).add(offset.mul(fx::Fx::from_int(sign as i32)));
                let p = u.pose.world([rear, fx::Fx::ZERO, z]);
                let [x, y, z] = [p.x, p.y, p.z].map(fx::Fx::to_int_trunc);
                let Some((mat, floor)) = (1..=d.floor_probe).find_map(|drop| {
                    let floor = y - drop;
                    let mat = sim_core::blast::material(&sim.world, [x, floor, z]);
                    (mat != 0).then_some((mat, floor))
                }) else {
                    continue;
                };
                let strength = dust_strength(mat);
                if strength == 0.0 {
                    continue;
                }
                let scale = (0.4 + 0.6 * motion) * strength;
                let forward = point([u.pose.x.x, u.pose.x.y, u.pose.x.z]);
                let side = point([u.pose.z.x, u.pose.z.y, u.pose.z.z]);
                let drift = -forward * (d.trail_milli as f32 / 1000.0 * motion)
                    + Vec3::Y * (d.rise_milli as f32 / 1000.0 * scale)
                    + side * (sign * turn / d.full_turn_rate as f32 * 2.0);
                self.add_dust(
                    Vec3::new(
                        p.x.to_f64_lossy() as f32,
                        floor as f32 + 2.0,
                        p.z.to_f64_lossy() as f32,
                    ),
                    drift,
                    now,
                    d.life_milli as f32 / 1000.0 * (0.8 + 0.2 * motion),
                    d.edge_milli as f32 / 1000.0
                        * scale
                        * (0.78 + 0.44 * noise(self.serial.wrapping_add(31))),
                    mat,
                );
                active += 1;
            }
        }
    }
}
fn dust_strength(mat: u16) -> f32 {
    match mat {
        1 | 7 => 1.0,
        8 => 0.8,
        6 => 0.6,
        2 => 0.42,
        _ => 0.0,
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
fn beam(buf: &mut MeshBuf, from: Vec3, to: Vec3, width: f32, col: [f32; 4]) {
    let delta = to - from;
    let length = delta.length();
    if length < 0.01 {
        return;
    }
    let centre = (from + to) * 0.5;
    let rotation = Quat::from_rotation_arc(Vec3::Y, delta / length);
    let start = buf.pos.len();
    cube(buf, centre, 1.0, col);
    for p in &mut buf.pos[start..] {
        let local = Vec3::from(*p) - centre;
        *p = (centre + rotation * Vec3::new(local.x * width, local.y * length, local.z * width))
            .to_array();
    }
    for n in &mut buf.nrm[start..] {
        *n = (rotation * Vec3::from(*n)).to_array();
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
    if scene.sim.config.delivery == Delivery::Laser && !scene.sim.battle_mode() {
        for trace in &scene.sim.streams.traces {
            let from = point(trace.from);
            let to = point(trace.to);
            beam(&mut hot, from, to, 2.4, [2.8, 0.06, 0.025, 0.18]);
            beam(&mut hot, from, to, 0.65, [7.0, 0.36, 0.25, 0.92]);
            if trace.hit {
                cube(&mut hot, to, 2.5, [5.0, 0.65, 0.18, 0.75]);
            }
        }
    }
    if scene.sim.battle_mode() {
        let point = |p: [fx::Fx; 3]| Vec3::from(p.map(|v| v.to_f64_lossy() as f32));
        for r in &scene.sim.battle.rounds {
            let t = if scene.realtime {
                (scene.acc * 30.0).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let at = point(r.previous).lerp(point(r.pos), t);
            cube(
                &mut hot,
                at,
                if r.shell { 2.0 } else { 0.8 },
                [6.0, 3.2, 0.55, 1.0],
            );
        }
        for trace in &scene.sim.battle.traces {
            let from = point(trace.from);
            let to = point(trace.to);
            let n = ((to - from).length() / 2.0).ceil().clamp(1.0, 48.0) as usize;
            for i in 0..n {
                cube(
                    &mut hot,
                    from.lerp(to, i as f32 / n as f32),
                    0.65,
                    [4.0, 2.1, 0.25, 0.85],
                );
            }
        }
    }
    let mut smoke = MeshBuf::default();
    for (p, at, t) in particles {
        if p.smoke {
            if p.dust_material != 0 {
                let rgb = match p.dust_material {
                    1 => [0.38, 0.31, 0.23],
                    7 => [0.36, 0.34, 0.25],
                    8 => [0.39, 0.36, 0.31],
                    6 => [0.34, 0.34, 0.32],
                    _ => [0.38, 0.36, 0.33],
                };
                let fade = (t * 9.0).min(1.0) * (1.0 - t).powi(2);
                cube(
                    &mut smoke,
                    at,
                    p.edge * (0.75 + t * 0.9),
                    [rgb[0], rgb[1], rgb[2], fade * 0.34],
                );
            } else {
                let fade = (t * 8.0).min(1.0) * (1.0 - t).powi(2);
                let shade = 0.085 + t * 0.09 + noise(p.seed) * 0.025;
                cube(
                    &mut smoke,
                    at,
                    p.edge * (0.65 + t * 1.65),
                    [shade * 0.94, shade * 0.96, shade, fade * 0.25],
                );
            }
        } else {
            let flicker = 0.85 + noise(p.seed.wrapping_add((t * 12.0) as u32)) * 0.15;
            let col = if p.stream && t < 0.25 {
                [3.2, 1.15, 0.12, 0.74]
            } else if p.stream && t < 0.65 {
                [2.3, 0.46, 0.015, 0.7]
            } else if p.stream {
                [0.95, 0.11, 0.007, (1.0 - t) * 1.3]
            } else if t < 0.23 {
                [5.8, 3.2, 0.55, 0.88]
            } else if t < 0.58 {
                [3.2, 0.75, 0.035, 0.82]
            } else {
                [1.3, 0.12, 0.009, (1.0 - t) * 1.8]
            };
            let edge = p.edge * (0.8 + (t * 3.0).sin() * 0.2) * (1.0 - t * 0.55) * flicker;
            if p.stream {
                // A flame packet travels as a ragged tongue. Extend its
                // cuboid behind the current position so the ribbon joins up
                // without painting beyond the latest contact point.
                let axis = p.velocity.normalize_or_zero();
                beam(
                    &mut hot,
                    at - axis * edge * (2.1 - t * 0.7),
                    at,
                    edge * 0.72,
                    col,
                );
            } else {
                cube(&mut hot, at, edge, col);
            }
        }
    }
    // Use exactly the interpolated transforms already applied to visible meshes,
    // including solved legs. Current simulation poses can lead the picture by a tick.
    let mut poses = std::collections::BTreeMap::new();
    if !fx.heat_spots.is_empty() {
        for (w, tr) in &bodies {
            poses.entry((w.0, w.2, w.3)).or_insert(*tr);
        }
    }
    for &(cell, heat) in &fx.heat_spots {
        let id = usize::from(cell.unit);
        let seg = cell.part.checked_sub(1).map(usize::from);
        let turret = seg.map_or(cell.at[1] >= scene.sim.units[id].body.turret_y, |i| {
            scene.sim.units[id].body.segments[i].attach.turret
        });
        let Some(tr) = poses.get(&(id, seg, turret)) else {
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
    fn moving_tracks_raise_bounded_ground_coloured_dust_once_per_tick() {
        let mut r = BlastRange::new(config(false, false));
        let mut effects = Effects::default();
        r.tick = r.streams.tuning.dust.interval_ticks;
        r.gun.motion.drive.vel = fx::Fx::from_int(30);
        r.gun.update_pose(r.gun.motion.drive.head);
        effects.capture(&r);
        let dust: Vec<_> = effects
            .particles
            .iter()
            .filter(|p| p.dust_material != 0)
            .collect();
        assert_eq!(dust.len(), 2);
        assert!(dust.iter().all(|p| p.smoke && p.edge > 1.0));
        effects.capture(&r);
        assert_eq!(effects.particles.len(), 2, "one tick emitted twice");

        r.gun.motion.drive.vel = fx::Fx::ZERO;
        r.tick += r.streams.tuning.dust.interval_ticks;
        effects.capture(&r);
        assert_eq!(effects.particles.len(), 2, "stopped tracks made dust");
        r.tick += 30;
        effects.capture(&r);
        assert!(effects.particles.is_empty(), "old dust did not clear");
    }
    #[test]
    fn pivoting_tracks_raise_dust_and_keep_a_separate_particle_budget() {
        let mut r = BlastRange::new(config(false, false));
        r.streams.tuning.dust.max_particles = 4;
        let mut effects = Effects::default();
        r.tick = 3;
        r.gun.motion.drive.yaw_rate = 90;
        effects.capture(&r);
        assert_eq!(effects.particles.len(), 2);
        r.tick = 6;
        effects.capture(&r);
        r.tick = 9;
        effects.capture(&r);
        assert_eq!(effects.particles.len(), 4);
        assert_eq!(dust_strength(3), 0.0, "steel should not shed soil");
        assert!(dust_strength(7) > dust_strength(2));
    }
    #[test]
    fn a_steel_floor_hides_the_soil_below_from_track_dust() {
        let mut r = BlastRange::new(config(false, false));
        let mut effects = Effects::default();
        r.tick = 3;
        r.gun.motion.drive.vel = fx::Fx::from_int(30);
        effects.capture(&r);
        let contacts: Vec<_> = effects
            .particles
            .iter()
            .filter(|p| p.dust_material != 0)
            .map(|p| {
                [
                    p.at.x.floor() as u32,
                    (p.at.y - 2.0) as u32,
                    p.at.z.floor() as u32,
                ]
            })
            .collect();
        assert_eq!(contacts.len(), 2);
        for [x, y, z] in contacts {
            r.world.queue_edit(voxel::world::Edit::put(x, y, z, 3));
        }
        r.world.flush();
        effects.clear();
        r.tick = 6;
        effects.capture(&r);
        assert!(effects.particles.is_empty());
    }
    #[test]
    fn an_airborne_tank_does_not_raise_track_dust() {
        let mut r = BlastRange::new(config(false, false));
        r.tick = 3;
        r.gun.motion.drive.vel = fx::Fx::from_int(30);
        r.gun.motion.hull.load = fx::Fx::ZERO;
        let mut effects = Effects::default();
        effects.capture(&r);
        assert!(effects.particles.is_empty());
    }
    #[test]
    fn a_real_range_move_emits_track_dust() {
        let layout =
            RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap();
        let mut r = BlastRange::with_layout(config(false, false), layout).unwrap();
        r.control();
        assert!(r.order_move([250, 336]));
        let mut effects = Effects::default();
        let mut emitted = false;
        for _ in 0..90 {
            r.step();
            effects.capture(&r);
            emitted |= effects.particles.iter().any(|p| p.dust_material != 0);
        }
        assert!(emitted, "moving range tank raised no dust");
    }
    #[test]
    fn shell_impact_starts_a_bounded_flash_before_the_slow_plume() {
        let mut r = BlastRange::new(config(false, false));
        let mut effects = Effects::default();
        for _ in 0..200 {
            r.step();
            effects.capture(&r);
            if r.impact.is_some() {
                break;
            }
        }
        assert!(r.impact.is_some(), "the shell did not land");
        let now = r.tick as f32 / 30.0;
        let hot = effects
            .particles
            .iter()
            .filter(|p| p.born == now && !p.smoke)
            .count();
        let smoke = effects
            .particles
            .iter()
            .filter(|p| p.born == now && p.smoke)
            .count();
        assert!(hot >= 25 && smoke >= 10, "flash={hot} smoke={smoke}");
        assert!(effects.particles.len() <= CAP);
    }
    #[test]
    fn solid_shell_contact_has_sparks_without_the_explosive_fireball() {
        let mut r = BlastRange::new(weapon_config(false, false, 1000, Delivery::Kinetic));
        let mut effects = Effects::default();
        for _ in 0..240 {
            r.step();
            effects.capture(&r);
            if r.impact.is_some() {
                break;
            }
        }
        assert!(r.impact.is_some(), "solid shell never landed");
        let now = r.tick as f32 / 30.0;
        let sparks = effects
            .particles
            .iter()
            .filter(|p| p.born == now && !p.smoke)
            .count();
        assert_eq!(sparks, 10);
        assert!(r.explosion.is_none());
    }
    #[test]
    fn a_new_impact_replaces_old_remnants_when_the_pool_is_full() {
        let mut effects = Effects::default();
        for _ in 0..CAP {
            effects.add(Vec3::ZERO, Vec3::ZERO, 0.0, 10.0, 1.0, true);
        }
        effects.shell_burst(Vec3::new(20.0, 0.0, 0.0), 1.0, 2300);
        assert_eq!(effects.particles.len(), CAP);
        assert_eq!(
            effects.particles.iter().filter(|p| p.born == 1.0).count(),
            35
        );
        assert_eq!(
            effects.particles.iter().filter(|p| p.born == 0.0).count(),
            CAP - 35
        );
    }
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
                .any(|p| p.at == point(at) && p.edge == 4.7));
        }
    }
    #[test]
    fn flame_fills_the_flown_segment_without_crossing_a_hit() {
        let cfg = weapon_config(false, false, 1000, Delivery::Flame);
        let mut r = BlastRange::new(cfg);
        let mut effects = Effects::default();
        for _ in 0..r.fire_tick + 8 {
            r.step();
            if !r.streams.traces.is_empty() {
                effects.capture(&r);
                break;
            }
        }
        let trace = r.streams.traces.first().expect("flame packet did not fly");
        let (a, b) = (point(trace.from), point(trace.to));
        let direction = (b - a).normalize();
        let length = a.distance(b);
        let now = r.tick as f32 / 30.0;
        let along: Vec<_> = effects
            .particles
            .iter()
            .filter(|p| p.born == now && !p.smoke)
            .map(|p| (p.at - a).dot(direction))
            .collect();
        assert!(along.len() >= 4, "flame must fill each packet path");
        assert!(along.iter().any(|&v| v < length * 0.35));
        assert!(along.iter().any(|&v| v > length * 0.65));
        if trace.hit {
            assert!(along.iter().all(|&v| v <= length + 0.7));
        }
    }
    #[test]
    fn flame_contact_visuals_drift_across_the_wall_instead_of_through_it() {
        let cfg = weapon_config(false, false, 1000, Delivery::Flame);
        let mut r = BlastRange::new(cfg);
        r.tick = 1;
        r.streams.traces.push(sim_core::blast::weapons::Trace {
            from: [fx::Fx::ZERO, fx::Fx::from_int(20), fx::Fx::ZERO],
            to: [fx::Fx::from_int(12), fx::Fx::from_int(23), fx::Fx::ZERO],
            hit: true,
            serial: 0,
        });
        let mut effects = Effects::default();
        effects.capture(&r);
        let trace = r.streams.traces[0];
        let (a, b) = (point(trace.from), point(trace.to));
        let direction = (b - a).normalize();
        let length = a.distance(b);
        let mut count = 0;
        for p in effects.particles.iter().filter(|p| p.stream) {
            count += 1;
            assert!((p.at - a).dot(direction) < length);
            assert!(p.velocity.dot(direction).abs() < 0.01);
        }
        assert!(count >= 6);
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
