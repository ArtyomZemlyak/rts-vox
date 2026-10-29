//! Tool interaction is presentation only; mutations enter typed SIM commands.
use super::*;
use bevy::input::{keyboard::KeyboardInput, ButtonState};
use modding::lab_tools::DeviceKind;
pub struct State {
    pub section: section::State,
    pub metric: u8,
    pub metric_scale: u8,
    pub orbit: Option<orbit::View>,
    pub device_preview: Option<([i32;3],bool)>,
    pub flight: first_person::Flight,
    pub live: bool,
    pub live_tool: u8, // dig, material, water, flame, laser, MG, HE, kinetic
    pub point: bool,
    pub power_milli: u32,
    pub emitting: bool,
    pub live_tick: Option<u32>,
    pub live_target: Option<([i32;3],[i32;3])>,
    pub flying: bool,
    pub camera: Option<Transform>,
    held: [bool; 6],
    cursor: Option<Vec2>,
    pub edge: u32,
    pub distance: f32,
    pub probe: String,
    pub selected: Option<[i32; 3]>,
    pub probe_direction: Option<Vec3>,
    pub falling: Option<(u64, [u16; 3])>,
    pub fragment: Option<fragments::picking::Hit>,
    pub probe_stamp: Option<(u64, u32)>,
    pub last_tick: Option<u32>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            section: section::State::default(),
            metric: 0,
            metric_scale: 1,
            orbit: None,
            device_preview: None,
            flight: first_person::Flight::default(),
            live: false, live_tool: 0, point: false, power_milli: 1000,
            emitting: false, live_tick: None, live_target: None,
            flying: false,
            camera: None,
            held: [false; 6],
            cursor: None,
            edge: 4,
            distance: 32.0,
            probe: "Нажмите I или «Пипетка · 10 см», затем ЛКМ по кубику. Размер кисти и сетка размещения на выбор не влияют.".into(),
            selected: None,
            probe_direction: None,
            falling: None,
            fragment: None,
            probe_stamp: None,
            last_tick: None,
        }
    }
}
pub fn input(
    mut lab: ResMut<Laboratory>,
    mut events: MessageReader<KeyboardInput>,
    windows: Query<&Window>,
    cameras: Query<&Transform, With<GasCamera>>,
    mouse: Res<ButtonInput<MouseButton>>,
    time: Res<Time<Real>>,
    roots: Query<&bevy::ui::RelativeCursorPosition, With<Root>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Option<Res<bevy::input::mouse::AccumulatedMouseMotion>>,
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
) {
    lab.tools.device_preview=None;
    if lab.active() && lab.constructor.voxel_editing() {
        lab.tools.held=[false;6];lab.tools.cursor=None;events.clear();
        if let Ok(window)=windows.single() {
            orbit::input(&mut lab,window,roots.iter().any(|r|r.cursor_over()),&keys,&mouse,motion.as_ref().map(|m|m.delta),&mut wheel);
        } else {wheel.clear();}
        return;
    }
    orbit::leave(&mut lab);
    wheel.clear();
    if lab.tools.live {lab.tools.held=[false;6];lab.tools.cursor=None;events.clear();return;}
    for e in events.read() {
        if let Some(i) = [
            KeyCode::KeyW,
            KeyCode::KeyS,
            KeyCode::KeyA,
            KeyCode::KeyD,
            KeyCode::KeyZ,
            KeyCode::KeyX,
        ]
        .iter()
        .position(|k| *k == e.key_code)
        {
            lab.tools.held[i] = e.state == ButtonState::Pressed;
        }
    }
    if !lab.active() || !lab.tools.flying {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    if !window.focused {
        lab.tools.held = [false; 6];
        lab.tools.cursor = None;
        return;
    }
    if roots.iter().any(|r| r.cursor_over()) {
        lab.tools.cursor = None;
        return;
    }
    if lab.ui.editing.is_some() || lab.ui.help || lab.browser {
        return;
    }
    let inspecting = matches!(lab.constructor.mode, constructor::Mode::Inspect|constructor::Mode::Voxel);
    let t = &mut lab.tools;
    let Some(mut pose) = t.camera.or_else(|| cameras.single().ok().copied()) else {
        return;
    };
    if let Some(cursor) = window.cursor_position() {
        if mouse.pressed(MouseButton::Right) {
            if let Some(last) = t.cursor {
                let delta = (cursor - last) * 0.003;
                let (yaw, pitch, _) = pose.rotation.to_euler(EulerRot::YXZ);
                pose.rotation = Quat::from_euler(
                    EulerRot::YXZ,
                    yaw - delta.x,
                    (pitch - delta.y).clamp(-1.5, 1.5),
                    0.0,
                );
            }
        }
        t.cursor = Some(cursor);
    }
    let h = t.held;
    let direction = pose.forward() * (i32::from(h[0]) - i32::from(h[1])) as f32
        + pose.right() * (i32::from(h[3]) - i32::from(h[2])) as f32
        + Vec3::Y * (i32::from(h[5]) - i32::from(h[4])) as f32;
    pose.translation += direction.normalize_or_zero() * time.delta_secs().min(0.05) * if inspecting {10.0} else {60.0};
    t.camera = Some(pose);
}
impl Laboratory {
    pub fn editor_camera(&self) -> bool {
        self.active() && !self.tools.live && !self.constructor.voxel_editing() && !self.tools.flying
    }
    pub fn camera_ui_blocked(&self) -> bool {
        self.browser || self.ui.help || self.ui.editing.is_some()
    }
    pub fn flying(&self) -> bool {
        self.active() && (self.tools.flying || (self.constructor.voxel_editing() && self.tools.orbit.is_some()))
    }
    pub fn tool_camera(
        &self,
        transform: &mut Transform,
        projection: &mut Projection,
        aspect: f32,
    ) -> bool {
        if !self.flying() {
            return false;
        }
        let Some(camera) = self.tools.camera else {
            return false;
        };
        *transform = camera;
        if self.tools.flight.preview && std::env::var("RTS_LAB_CAPTURE_CASE").is_ok_and(|s|s.starts_with("metric-") && s!="metric-live") {
            *projection=Projection::Orthographic(OrthographicProjection {
                scaling_mode:bevy::camera::ScalingMode::FixedVertical{viewport_height:if std::env::var("RTS_LAB_CAPTURE_CASE").as_deref()==Ok("metric-fragment") {12.0}else{80.0}},
                ..OrthographicProjection::default_3d()
            });
            return true;
        }
        *projection = Projection::Perspective(PerspectiveProjection {
            fov: if self.constructor.voxel_editing() {orbit::FOV.to_radians()}else{self.tools.flight.fov.to_radians()},
            aspect_ratio: aspect,
            near: 0.1,
            far: 20000.0,
            ..default()
        });
        true
    }
}
pub(crate) fn entry(ray: Ray3d, lo: Vec3, hi: Vec3) -> Option<(f32, usize)> {
    let mut near = f32::NEG_INFINITY;
    let mut far = f32::INFINITY;
    let mut axis = 1;
    for a in 0..3 {
        let d = ray.direction[a];
        if d.abs() < 1e-8 {
            if ray.origin[a] < lo[a] || ray.origin[a] > hi[a] {
                return None;
            }
            continue;
        }
        let x = (lo[a] - ray.origin[a]) / d;
        let y = (hi[a] - ray.origin[a]) / d;
        if x.min(y) > near {
            near = x.min(y);
            axis = a;
        }
        far = far.min(x.max(y));
    }
    (far >= near.max(0.0)).then_some((near.max(0.0), axis))
}
pub fn surface_anchor(
    ray: Ray3d,
    hit: [i32; 3],
    lo: Vec3,
    hi: Vec3,
    step: i32,
    elevation: i32,
) -> [i32; 3] {
    let cell = Vec3::from(hit.map(|v| v as f32));
    let (distance, axis) = entry(ray, cell, cell + Vec3::ONE).unwrap_or((0.0, 1));
    let point = ray.get_point(distance);
    let centre = (lo + hi) * 0.5;
    let mut at =
        std::array::from_fn(|a| ((point[a] - centre[a]) / step as f32).round() as i32 * step);
    at[axis] = if ray.direction[axis] < 0.0 {
        hit[axis] + 1 - lo[axis].floor() as i32
    } else {
        hit[axis] - hi[axis].ceil() as i32
    };
    at[1] += elevation;
    at
}
pub(super) fn liquid_pick(scene: &Scene, ray: Ray3d) -> Option<[i32; 3]> {
    workbench::visit_pick_ray(scene.sim.size(), ray, |p| {
        let wet = p.iter().all(|v| *v >= 0) && scene.sim.water.amount(p.map(|v| v as u32)) > 0;
        (wet || scene.sim.units.iter().any(|u| u.solid(p))
            || (!scene.sim.point_gun() && scene.sim.gun.solid(p))
            || sim_core::blast::material(&scene.sim.world, p) != 0)
            .then_some(p)
    })
}
fn falling_probe(scene: &Scene, ray: Ray3d, limit: f32) -> Option<(f32, [i32; 3], u16, u64, [u16;3])> {
    let mut best = None;
    let mut end = limit;
    // Exact voxel query runs only on a click. There is no per-frame index
    // rebuild or full moving-debris scan while browsing the editor.
    for b in &scene.sim.bodies.list {
        let origin = Vec3::new(b.ox as f32, b.oy as f32, b.oz as f32);
        let padding = if b.tilt_angle == 0 {
            Vec3::ZERO
        } else {
            Vec3::splat(f32::from(b.sx.max(b.sz).max(b.height())) * 2.0)
        };
        if entry(
            ray,
            origin - padding,
            origin + Vec3::new(f32::from(b.sx), f32::from(b.height()), f32::from(b.sz)) + padding,
        )
        .is_none_or(|(t, _)| t > end)
        {
            continue;
        }
        for &v in &b.voxels {
            let (x, y, z) = b.world_of(v);
            let p = [x as i32, y as i32, z as i32];
            let lo = Vec3::from(p.map(|v| v as f32));
            if let Some((t, _)) = entry(ray, lo, lo + Vec3::ONE) {
                if t < end {
                    end = t;
                    best = Some((t, p, v.mat, b.id, [v.x,v.y,v.z]));
                }
            }
        }
    }
    best
}
pub fn pointer(
    lab: &mut Laboratory,
    scene: &Scene,
    ray: Ray3d,
    mouse: &ButtonInput<MouseButton>,
    gizmos: &mut Gizmos,
) -> bool {
    use constructor::Mode;
    let mode = lab.constructor.mode;
    if lab.tools.live {return live_tools::pointer(lab,scene,ray,mouse,gizmos);}
    if !matches!(
        mode,
        Mode::Brush | Mode::Erase | Mode::Inspect | Mode::Drill | Mode::Crawler | Mode::Collector
    ) {
        return false;
    }
    // Draw actual apparatus state, including the work spent in hard material.
    let hit = liquid_pick(scene, ray);
    let at = if lab.tools.flying && mode != Mode::Inspect
        || (!lab.constructor.workspace.surface_snap.unwrap_or(true) && mode != Mode::Inspect)
    {
        Some(
            ray.get_point(lab.tools.distance)
                .floor()
                .to_array()
                .map(|v| v as i32),
        )
    } else {
        hit
    };
    if mode == Mode::Inspect && mouse.just_pressed(MouseButton::Left) {
        let limit = hit
            .and_then(|p| {
                entry(
                    ray,
                    Vec3::from(p.map(|v| v as f32)),
                    Vec3::from(p.map(|v| v as f32)) + Vec3::ONE,
                )
            })
            .map_or(f32::INFINITY, |v| v.0 + 0.01);
        let falling=falling_probe(scene,ray,limit);
        let fragment=fragments::picking::pick(scene,ray,falling.as_ref().map_or(limit,|h|h.0));
        if let Some(hit)=fragment {
            let f=scene.sim.matter.debris.fragments.iter().find(|f|f.id==hit.id).unwrap();
            let p=hit.point(f).unwrap().floor().to_array().map(|v|v as i32);
            lab.tools.probe_direction=Some(*ray.direction);
            inspector::select(lab,scene,p);
            lab.tools.fragment=Some(hit);lab.tools.probe_stamp=None;
            inspector::refresh(lab,scene);
            return true;
        }
        if let Some((_, p, _, id, local)) = falling {
            lab.tools.probe_direction=Some(*ray.direction);
            inspector::select(lab, scene, p);
            lab.tools.falling = Some((id,local));
            lab.tools.probe_stamp = None;
            inspector::refresh(lab,scene);
            return true;
        }
    }
    let Some(mut at) = at else {
        return true;
    };
    if mode == Mode::Inspect {
        if mouse.just_pressed(MouseButton::Left) {
            lab.tools.probe_direction=Some(*ray.direction);
            inspector::select(lab, scene, at);
        }
        return true;
    }
    if mode == Mode::Brush && !lab.tools.flying {
        if let Some((_, axis)) = entry(
            ray,
            Vec3::from(at.map(|v| v as f32)),
            Vec3::from(at.map(|v| v as f32)) + Vec3::ONE,
        ) {
            at[axis] += if ray.direction[axis] < 0.0 {
                1
            } else {
                -(lab.tools.edge as i32)
            };
        }
    }
    if !lab.tools.flying && matches!(mode, Mode::Drill | Mode::Crawler | Mode::Collector) {
        at[1] += if mode == Mode::Crawler { 2 } else { 1 };
    }
    at[1] += lab.constructor.elevation;
    let edge = if matches!(mode, Mode::Drill | Mode::Crawler | Mode::Collector) {
        3
    } else {
        lab.tools.edge
    };
    let valid = if mode==Mode::Collector {
        let t=modding::lab_tools::Tuning::stock();
        let valid=at[0]>t.collector_half && at[2]>t.collector_half && at[1]>0
            && at[0]+t.collector_half<scene.sim.size()[0] as i32-1
            && at[2]+t.collector_half<scene.sim.size()[2] as i32-1
            && at[1]+t.collector_height<scene.sim.size()[1] as i32-1;
        lab.tools.device_preview=Some((at,valid));valid
    } else {(0..3).all(|a| at[a] > 0 && at[a] + (edge as i32) < scene.sim.size()[a] as i32)};
    let colour = if valid {
        if mode == Mode::Erase {
            Color::srgb(1.0, 0.35, 0.2)
        } else {
            Color::srgb(0.2, 0.95, 0.65)
        }
    } else {
        Color::srgb(1.0, 0.1, 0.1)
    };
    gizmos.cube(
        Transform::from_translation(
            Vec3::from(at.map(|v| v as f32))
                + Vec3::splat(if matches!(mode, Mode::Drill | Mode::Crawler | Mode::Collector) {
                    0.5
                } else {
                    edge as f32 * 0.5
                }),
        )
        .with_scale(Vec3::splat(edge as f32)),
        colour,
    );
    if valid {
        if matches!(mode, Mode::Brush | Mode::Erase)
            && mouse.pressed(MouseButton::Left)
            && lab.tools.last_tick != Some(scene.sim.tick)
        {
            lab.constructor
                .pending
                .push(LabAction::Sculpt(at, mode == Mode::Erase));
        } else if matches!(mode, Mode::Drill | Mode::Crawler | Mode::Collector)
            && mouse.just_pressed(MouseButton::Left)
        {
            lab.constructor.pending.push(LabAction::DeviceAt(
                if mode == Mode::Drill {
                    DeviceKind::Drill
                } else if mode==Mode::Collector {
                    DeviceKind::Collector
                } else {
                    DeviceKind::Crawler
                },
                at,
            ));
        }
    }
    true
}
pub fn draw(lab: &Laboratory, scene: &Scene, gizmos: &mut Gizmos) {
    if !lab.active() {
        return;
    }
    for d in scene
        .sim
        .layout
        .as_ref()
        .into_iter()
        .flat_map(|l| &l.lab_devices)
    {
        if let Some(c)=d.collector.as_ref() {
            if let Some(aim)=c.aim.filter(|_|c.spent>0&&!c.returning) {
                let p=Vec3::from(d.at.map(|v|v as f32));
                let rotation=Quat::from_rotation_y(-(d.turns as f32)*std::f32::consts::FRAC_PI_2);
                gizmos.line(p+rotation*Vec3::new(8.5,2.8,0.0),Vec3::from(aim.map(|v|v as f32))+Vec3::splat(0.5),theme::ACCENT);
            }
            continue;
        }
        let p = Vec3::from(d.at.map(|v| v as f32)) + Vec3::splat(0.5);
        let axis = if d.kind == DeviceKind::Drill {
            Vec3::NEG_Y
        } else {
            match d.turns {
                0 => Vec3::X,
                1 => Vec3::Z,
                2 => Vec3::NEG_X,
                _ => Vec3::NEG_Z,
            }
        };
        gizmos.cube(
            Transform::from_translation(p).with_scale(Vec3::new(3.0, 2.0, 3.0)),
            Color::srgb(0.95, 0.65, 0.1),
        );
        gizmos.arrow(p, p + axis * 5.0, Color::srgb(0.3, 1.0, 0.65));
        for i in 0..3 {
            let angle = scene.sim.tick as f32 * 0.3 + i as f32 * std::f32::consts::TAU / 3.0;
            gizmos.line(
                p + axis * 2.0,
                p + axis * 3.0 + Vec3::new(angle.cos(), 0.0, angle.sin()) * 1.4,
                theme::ACCENT,
            );
        }
    }
}

#[path = "inspector.rs"]
pub(super) mod inspector;
