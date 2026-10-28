//! Presentation controls and a camera-local tool. Every effect still uses SIM commands.
use super::*;
use bevy::input::{
    keyboard::KeyboardInput,
    mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel},
    ButtonState,
};
use bevy::window::{CursorGrabMode, CursorOptions};

const KEYS: [KeyCode; 14] = [
    KeyCode::KeyW,
    KeyCode::KeyS,
    KeyCode::KeyA,
    KeyCode::KeyD,
    KeyCode::Space,
    KeyCode::ControlLeft,
    KeyCode::KeyX,
    KeyCode::KeyZ,
    KeyCode::ShiftLeft,
    KeyCode::ShiftRight,
    KeyCode::AltLeft,
    KeyCode::AltRight,
    KeyCode::ControlRight,
    KeyCode::KeyC,
];
pub struct Flight {
    pub preview: bool,
    pub profile: bool,
    pub captured: bool,
    pub request: bool,
    pub trigger_ready: bool,
    pub speed: f32,
    pub sensitivity: f32,
    pub fov: f32,
    held: [bool; 14],
}
impl Default for Flight {
    fn default() -> Self {
        Self {
            preview: false,
            profile: false,
            captured: false,
            request: false,
            trigger_ready: false,
            speed: 60.0,
            sensitivity: 0.0025,
            fov: 70.0,
            held: [false; 14],
        }
    }
}
pub fn immersive(lab: &Laboratory) -> bool {
    lab.active() && lab.tools.live && lab.tools.flying && lab.tools.flight.captured
}
fn turn(pose: &mut Transform, delta: Vec2, sensitivity: f32) {
    let (yaw, pitch, _) = pose.rotation.to_euler(EulerRot::YXZ);
    pose.rotation = Quat::from_euler(
        EulerRot::YXZ,
        (yaw - delta.x * sensitivity).rem_euclid(std::f32::consts::TAU),
        (pitch - delta.y * sensitivity).clamp(-1.50, 1.50),
        0.0,
    );
}
fn travel(pose: &mut Transform, h: [bool; 14], speed: f32, dt: f32, size: [u32; 3]) {
    // Horizontal WASD: looking down at a sample must not dive into it.
    let forward = Vec3::new(pose.forward().x, 0.0, pose.forward().z).normalize_or_zero();
    let right = Vec3::new(pose.right().x, 0.0, pose.right().z).normalize_or_zero();
    let up = h[4] || h[6];
    let down = h[5] || h[7] || h[12] || h[13];
    let direction = forward * (i32::from(h[0]) - i32::from(h[1])) as f32
        + right * (i32::from(h[3]) - i32::from(h[2])) as f32
        + Vec3::Y * (i32::from(up) - i32::from(down)) as f32;
    let gain = if h[10] || h[11] {
        0.2
    } else if h[8] || h[9] {
        3.0
    } else {
        1.0
    };
    pose.translation += direction.normalize_or_zero() * speed * gain * dt.clamp(0.0, 0.1);
    for axis in 0..3 {
        let margin: f32 = if axis == 1 { 1.5 } else { 34.0 };
        let lo = margin.min((size[axis] as f32 * 0.5 - 0.5).max(0.0));
        pose.translation[axis] =
            pose.translation[axis].clamp(lo, (size[axis] as f32 - lo - 0.5).max(lo));
    }
}
fn release(lab: &mut Laboratory) {
    lab.tools.flight.captured = false;
    lab.tools.flight.request = false;
    lab.tools.flight.trigger_ready = false;
    lab.tools.flight.held = [false; 14];
    if (lab.tools.emitting || lab.tools.live_target.is_some()) && !lab.constructor.pending.contains(&LabAction::LiveRelease) {
        lab.constructor.pending.push(LabAction::LiveRelease);
    }
}
#[allow(clippy::too_many_arguments)]
pub fn input(
    mut lab: ResMut<Laboratory>,
    scene: Res<Scene>,
    pilot: Option<Res<pilot::Pilot>>,
    mut events: MessageReader<KeyboardInput>,
    mut wheel: MessageReader<MouseWheel>,
    motion: Res<AccumulatedMouseMotion>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    time: Res<Time<Real>>,
    mut windows: Query<(&Window, &mut CursorOptions)>,
    roots: Query<&bevy::ui::RelativeCursorPosition, With<Root>>,
    mut owns_cursor: Local<bool>,
) {
    for e in events.read() {
        if let Some(i) = KEYS.iter().position(|k| *k == e.key_code) {
            lab.tools.flight.held[i] = e.state == ButtonState::Pressed;
        }
    }
    let Ok((window, mut cursor)) = windows.single_mut() else {
        wheel.clear();
        return;
    };
    if !lab.active() || !lab.tools.live || !lab.tools.flying {
        wheel.clear();
        if *owns_cursor {
            if !pilot.as_ref().is_some_and(|p| p.active()) {
                cursor.grab_mode = CursorGrabMode::None;
                cursor.visible = true;
            }
            *owns_cursor = false;
        }
        return;
    }
    if lab.tools.flight.preview && scene.out.is_some() {
        wheel.clear();
        return;
    }
    if !window.focused || lab.ui.editing.is_some() || lab.ui.help || lab.browser {
        release(&mut lab);
        wheel.clear();
    } else {
        let was_captured = lab.tools.flight.captured;
        if was_captured && (keys.just_pressed(KeyCode::Tab) || keys.just_pressed(KeyCode::Escape)) {
            release(&mut lab);
            keys.reset(KeyCode::Tab);
            keys.reset(KeyCode::Escape);
        } else {
            let over_ui = roots.iter().any(|r| r.cursor_over());
            if !was_captured
                && (lab.tools.flight.request
                    || keys.just_pressed(KeyCode::Tab)
                    || (!over_ui && mouse.just_pressed(MouseButton::Left)))
            {
                lab.tools.flight.captured = true;
                lab.tools.flight.request = false;
                lab.tools.flight.trigger_ready = !mouse.pressed(MouseButton::Left);
                keys.reset(KeyCode::Tab);
            }
            if lab.tools.flight.captured {
                let mut pose = lab.tools.camera.unwrap_or_default();
                if was_captured {
                    turn(
                        &mut pose,
                        motion.delta,
                        lab.tools.flight.sensitivity
                            * if mouse.pressed(MouseButton::Right) {
                                0.35
                            } else {
                                1.0
                            },
                    );
                }
                let flight = &lab.tools.flight;
                travel(
                    &mut pose,
                    flight.held,
                    flight.speed,
                    time.delta_secs(),
                    scene.sim.size(),
                );
                lab.tools.camera = Some(pose);
                lab.tools.flight.trigger_ready |= !mouse.pressed(MouseButton::Left);
                for (key, action) in [
                    (KeyCode::Digit1, LabAction::LiveTool(0)),
                    (KeyCode::Digit2, LabAction::LiveTool(1)),
                    (KeyCode::Digit3, LabAction::LiveTool(2)),
                    (KeyCode::Digit4, LabAction::LiveTool(3)),
                    (KeyCode::Digit5, LabAction::LiveTool(4)),
                    (KeyCode::Digit6, LabAction::LiveTool(5)),
                    (KeyCode::Digit7, LabAction::LiveTool(6)),
                    (KeyCode::Digit8, LabAction::LiveTool(7)),
                    (KeyCode::KeyP, LabAction::Pause),
                    (KeyCode::KeyR, LabAction::LivePoint),
                    (KeyCode::KeyQ, LabAction::BrushSize(-1)),
                    (KeyCode::KeyE, LabAction::BrushSize(1)),
                ] {
                    if keys.just_pressed(key) {
                        lab.constructor.pending.push(action);
                    }
                    keys.reset(key);
                }
                // Space belongs to flight; P pauses without making altitude jump.
                keys.reset(KeyCode::Space);
                for e in wheel.read() {
                    let dy = e.y
                        * if e.unit == MouseScrollUnit::Pixel {
                            0.01
                        } else {
                            1.0
                        };
                    if lab.tools.flight.held[8] || lab.tools.flight.held[9] {
                        lab.tools.flight.speed =
                            (lab.tools.flight.speed * (dy * 0.12).exp()).clamp(5.0, 600.0);
                    } else {
                        lab.tools.distance =
                            (lab.tools.distance * (dy * 0.15).exp()).clamp(1.0, 512.0);
                    }
                }
            } else {
                wheel.clear();
            }
        }
    }
    if lab.tools.flight.captured {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
        *owns_cursor = true;
    } else if *owns_cursor {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
        *owns_cursor = false;
    }
}

pub const MUZZLE: Vec3 = Vec3::new(3.6, -2.7, -7.4);
/// A bounded voxel DDA prevents a side-mounted source emerging through a wall.
/// Unit tests cover a wall between the eye and nozzle, including a diagonal.
pub fn muzzle(scene: &Scene, pose: Transform) -> Option<(Vec3, f32)> {
    let eye = pose.translation;
    let delta = pose.rotation * MUZZLE;
    let mut cell = eye.floor().as_ivec3();
    let sign = delta.signum().as_ivec3();
    let mut next = Vec3::splat(f32::INFINITY);
    let mut stride = Vec3::splat(f32::INFINITY);
    for a in 0..3 {
        if delta[a].abs() > 1e-6 {
            stride[a] = 1.0 / delta[a].abs();
            let boundary = cell[a] as f32 + if delta[a] > 0.0 { 1.0 } else { 0.0 };
            next[a] = (boundary - eye[a]) / delta[a];
        }
    }
    let mut entry = 0.0f32;
    let end = eye + delta;
    let lo = eye.min(end).floor().as_ivec3() - IVec3::ONE;
    let hi = eye.max(end).ceil().as_ivec3() + IVec3::ONE;
    let units: Vec<_> = scene
        .sim
        .units
        .iter()
        .filter(|u| {
            let (a, b) = u.bounds();
            (0..3).all(|k| b[k] >= lo[k] && a[k] <= hi[k])
        })
        .collect();
    for _ in 0..32 {
        let p = cell.to_array();
        let blocked = (0..3).any(|a| p[a] < 1 || p[a] >= scene.sim.size()[a] as i32 - 1)
            || (0..3).any(|a| a != 1 && (p[a] < 32 || p[a] >= scene.sim.size()[a] as i32 - 32))
            || sim_core::blast::material(&scene.sim.world, p) != 0
            || units.iter().any(|u| u.solid(p));
        if blocked {
            let fraction = (entry - 0.08 / delta.length()).max(0.0);
            return (fraction >= 0.15).then_some((eye + delta * fraction, fraction));
        }
        let t = next.min_element();
        if t >= 1.0 {
            return Some((end, 1.0));
        }
        for a in 0..3 {
            if next[a] <= t + 1e-6 {
                cell[a] += sign[a];
                next[a] += stride[a];
            }
        }
        entry = t;
    }
    None
}

#[derive(Component)]
pub struct Hud;
#[derive(Component)]
pub struct Readout;
#[derive(Component)]
pub struct ToolModel;
#[derive(Default)]
pub struct Visual {
    model: Option<Entity>,
}
pub fn build_ui(commands: &mut Commands, camera: Entity, font: &TextFont) {
    commands.spawn((Hud,UiTargetCamera(camera),ZIndex(120),Node {
        position_type:PositionType::Absolute,width:percent(100),height:percent(100),
        display:Display::None,..default()
    })).with_children(|p| {
        p.spawn((Node {position_type:PositionType::Absolute,left:percent(50),top:percent(50),
            width:px(4),height:px(4),margin:UiRect::all(px(-2)),border_radius:BorderRadius::all(px(2)),..default()},BackgroundColor(Color::srgb(0.90,0.98,1.0))));
        p.spawn((Node {position_type:PositionType::Absolute,left:px(24),top:px(20),
            padding:UiRect::all(px(12)),border_radius:BorderRadius::all(px(6)),..default()},BackgroundColor(theme::PANEL)))
            .with_children(|p|{p.spawn((Readout,Text::new(""),font.clone(),TextColor(theme::TEXT)));});
        p.spawn((Node {position_type:PositionType::Absolute,left:px(24),bottom:px(22),
            padding:UiRect::all(px(12)),border_radius:BorderRadius::all(px(6)),..default()},BackgroundColor(theme::PANEL)))
            .with_children(|p|{p.spawn((Text::new("1 Копатель   2 Материал   3 Вода   4 Огонь   5 Лазер\n6 Пулемёт   7 Фугасная пушка   8 Бронебойная пушка\nWASD · Space / Ctrl ↑↓ · Shift быстрее · Alt точно · ПКМ точный обзор\nЛКМ воздействие · Колесо дальность · Q/E кисть · R точка/поверхность · P пауза · Tab меню"),font.clone(),TextColor(theme::MUTED)));});
    });
}
pub fn draw(
    mut commands: Commands,
    lab: Res<Laboratory>,
    scene: Res<Scene>,
    mut cache: Local<Visual>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut models: Query<(&mut Transform, &mut Visibility), With<ToolModel>>,
    mut hud: Query<&mut Node, With<Hud>>,
    mut texts: Query<&mut Text, With<Readout>>,
) {
    let show = immersive(&lab);
    for mut n in &mut hud {
        let d = if show { Display::Flex } else { Display::None };
        if n.display != d {
            n.display = d;
        }
    }
    if show {
        let phase=if scene.paused {"ПАУЗА"}
            else if lab.tools.live_tool>=6 && lab.tools.live_target.is_some() && scene.sim.shell.is_none()
                && scene.sim.fire_tick!=u32::MAX && scene.sim.fire_tick>=scene.sim.tick {"ЗАРЯДКА"}
            else if lab.tools.live_tool>=6 && lab.tools.live_target.is_some() && !scene.sim.ready() {"ПЕРЕЗАРЯДКА"}
            else {"СИМУЛЯЦИЯ"};
        let value=format!("ЛАБОРАТОРИЯ / {}  ·  {}\nКисть {} см · Дальность {:.1} м · Полёт {:.1} м/с\nМощность {:.2}× · Tab — настройки и курсор",live_tools::name(lab.tools.live_tool),
            phase,lab.tools.edge*10,lab.tools.distance*0.1,lab.tools.flight.speed*0.1,lab.tools.power_milli as f32/1000.0);
        for mut t in &mut texts {
            if t.0 != value {
                t.0 = value.clone();
            }
        }
    }
    if !show {
        for (_, mut v) in &mut models {
            *v = Visibility::Hidden;
        }
        return;
    }
    let Some(pose) = lab.tools.camera else {
        return;
    };
    let Some((_, scale)) = muzzle(&scene, pose) else {
        for (_, mut v) in &mut models {
            *v = Visibility::Hidden;
        }
        return;
    };
    let transform = Transform {
        translation: pose.translation,
        rotation: pose.rotation,
        scale: Vec3::splat(scale),
    };
    if cache.model.is_none() {
        let mut b = MeshBuf::default();
        for (lo, hi, col) in [
            (
                [4.3, -6.5, -4.3],
                [6.0, -4.0, -1.5],
                [0.055, 0.085, 0.11, 1.0],
            ),
            (
                [3.7, -4.2, -6.7],
                [5.4, -3.0, -3.5],
                [0.12, 0.17, 0.20, 1.0],
            ),
            (
                [3.45, -3.45, -7.25],
                [4.45, -2.85, -6.2],
                [0.025, 0.05, 0.07, 1.0],
            ),
            (
                [3.2, -3.0, -7.4],
                [4.0, -2.4, -7.2],
                [0.05, 0.55, 0.7, 1.0],
            ),
            (
                [3.68, -3.8, -5.9],
                [3.72, -3.2, -4.7],
                [0.75, 0.4, 0.1, 1.0],
            ),
        ] {
            super::super::surround::block(&mut b, Vec3::from(lo), Vec3::from(hi), col);
        }
        let e = commands
            .spawn((
                ToolModel,
                Mesh3d(meshes.add(mesh(&b))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::WHITE,
                    unlit: true,
                    ..default()
                })),
                transform,
                Visibility::Inherited,
                bevy::light::NotShadowCaster,
                bevy::light::NotShadowReceiver,
            ))
            .id();
        cache.model = Some(e);
    } else {
        for (mut t, mut v) in &mut models {
            *t = transform;
            *v = Visibility::Inherited;
        }
    }
}

#[cfg(test)]
#[path = "first_person_tests.rs"]
mod tests;

/// Flush only pointer commands so this frame's aim reaches the next fixed tick.
pub fn flush(mut lab: ResMut<Laboratory>, mut scene: ResMut<Scene>) {
    if !lab.active() {
        return;
    }
    if lab.tools.live && lab.tools.flight.preview && scene.out.is_some() && !scene.paused {
        if let Some(pose) = lab.tools.camera {
            if let Some((at, origin)) =
                live_tools::target(&lab, &scene, Ray3d::new(pose.translation, pose.forward()))
            {
                live_tools::action(&mut lab, &mut scene, LabAction::LiveApply(at, origin));
            }
        }
    }
    let pending = std::mem::take(&mut lab.constructor.pending);
    for a in pending {
        if matches!(a, LabAction::LiveApply(_, _) | LabAction::LiveRelease) {
            live_tools::action(&mut lab, &mut scene, a);
        } else {
            lab.constructor.pending.push(a);
        }
    }
}

pub fn beam_origin(lab: &Laboratory, scene: &Scene) -> Option<Vec3> {
    (immersive(lab) && lab.tools.live_tool == 4).then_some(())?;
    muzzle(scene, lab.tools.camera?).map(|v| v.0)
}
