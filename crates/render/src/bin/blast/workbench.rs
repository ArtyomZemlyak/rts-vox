//! Placement tools for the existing forest range. Edits replace a validated
//! pristine fixture; damage never leaks back into the authored layout.
use super::*;
use bevy::camera::visibility::RenderLayers;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use modding::range::{RangeBlock, RangeShieldDef, RangeUnitDef};

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum Tool {
    #[default]
    Select,
    Tank,
    Strider,
    Concrete,
    Steel,
    Earth,
    Aim,
    Shield,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Selection {
    Unit(usize),
    Block(usize),
    Shield(usize),
}
/// Rounds caught by domes, kept for a few frames after their tick.
#[derive(Resource, Default)]
pub struct ShieldFlashes {
    seen: u32,
    live: Vec<(Vec3, u8)>,
}
#[derive(Component)]
pub struct EditorCamera;

pub fn setup(mut commands: Commands, edit: Res<Workshop>, mut config: ResMut<GizmoConfigStore>) {
    if edit.layout.is_none() {
        return;
    }
    // The gas composite follows the world pass and would cover ordinary
    // gizmos. Draw interaction handles with a second, transparent camera.
    let (style, _) = config.config_mut::<DefaultGizmoConfigGroup>();
    style.render_layers = RenderLayers::layer(1);
    style.line.width = 2.0;
    commands.spawn((
        Camera3d::default(),
        Camera {
            order: 1,
            clear_color: ClearColorConfig::Custom(Color::NONE),
            output_mode: bevy::camera::CameraOutputMode::Write {
                blend_state: Some(
                    bevy::render::render_resource::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
                ),
                clear_color: ClearColorConfig::None,
            },
            is_active: true,
            ..default()
        },
        RenderLayers::layer(1),
        EditorCamera,
        Msaa::Off,
        Projection::Orthographic(OrthographicProjection::default_3d()),
        Transform::default(),
    ));
}
// Explicit disjoint filters let Bevy verify the mutable camera borrows.
#[allow(clippy::type_complexity)]
pub fn camera(
    edit: Res<Workshop>,
    main: Query<(&Transform, &Projection), (With<GasCamera>, Without<EditorCamera>)>,
    mut overlay: Query<
        (&mut Transform, &mut Projection, &mut Camera),
        (With<EditorCamera>, Without<GasCamera>),
    >,
) {
    let Ok((t, p)) = main.single() else { return };
    for (mut ot, mut op, mut c) in &mut overlay {
        *ot = *t;
        *op = p.clone();
        c.is_active = edit.layout.is_some();
    }
}

#[derive(Resource)]
pub struct Workshop {
    pub layout: Option<RangeLayout>,
    pub editing: bool,
    pub tool: Tool,
    pub selected: Option<Selection>,
    pub pan: Vec2,
    pub zoom: f32,
    rotate: bool,
    history: Vec<RangeLayout>,
    redo: Vec<RangeLayout>,
    hover: Option<[i32; 3]>,
    previous_cursor: Option<Vec2>,
    pub pending: Option<BlastRange>,
}
impl Workshop {
    pub fn new(layout: Option<RangeLayout>, editing: bool) -> Self {
        let selected = if editing && layout.as_ref().is_some_and(|l| !l.units.is_empty()) {
            Some(Selection::Unit(0))
        } else {
            None
        };
        Self {
            layout,
            editing,
            tool: Tool::Select,
            selected,
            pan: Vec2::ZERO,
            zoom: 1.0,
            rotate: false,
            history: Vec::new(),
            redo: Vec::new(),
            hover: None,
            previous_cursor: None,
            pending: None,
        }
    }
    /// Atomic edit: validate the real body occupancy before touching history.
    pub(super) fn commit(
        &mut self,
        next: RangeLayout,
        config: ExplosionConfig,
    ) -> Result<(), String> {
        let sim = BlastRange::with_layout(config, next.clone())?;
        if let Some(old) = self.layout.replace(next) {
            if self.history.len() == 32 {
                self.history.remove(0);
            }
            self.history.push(old);
        }
        self.redo.clear();
        self.pending = Some(sim);
        Ok(())
    }
    fn undo(&mut self, config: ExplosionConfig, redo: bool) -> Result<(), String> {
        let stack = if redo {
            &mut self.redo
        } else {
            &mut self.history
        };
        let Some(next) = stack.last().cloned() else {
            return Ok(());
        };
        let sim = BlastRange::with_layout(config, next.clone())?;
        stack.pop();
        if let Some(old) = self.layout.replace(next) {
            if redo {
                self.history.push(old);
            } else {
                self.redo.push(old);
            }
        }
        self.selected = None;
        self.pending = Some(sim);
        Ok(())
    }
}

fn snap(v: f32) -> i32 {
    (v / 4.0).round() as i32 * 4
}
fn block(tool: Tool, at: [i32; 3], rotate: bool) -> RangeBlock {
    let (material, mut size) = match tool {
        Tool::Concrete => (2, [12, 44, 60]),
        Tool::Steel => (3, [16, 44, 60]),
        _ => (1, [28, 28, 60]),
    };
    if rotate {
        size.swap(0, 2);
    }
    let min = [at[0] - size[0] / 2, 0, at[2] - size[2] / 2];
    RangeBlock {
        min,
        max: std::array::from_fn(|a| min[a] + size[a]),
        material,
    }
}
fn overlap(a: &RangeBlock, b: &RangeBlock) -> bool {
    (0..3).all(|i| a.min[i] < b.max[i] && b.min[i] < a.max[i])
}
/// Connected overlapping layers move/delete together (the stock wall's jacket
/// and steel core). Separate, touching test walls remain independent.
fn group(layout: &RangeLayout, id: usize) -> Vec<usize> {
    let mut ids = vec![id];
    loop {
        let before = ids.len();
        for j in 0..layout.blocks.len() {
            if !ids.contains(&j)
                && ids
                    .iter()
                    .any(|i| overlap(&layout.blocks[*i], &layout.blocks[j]))
            {
                ids.push(j);
            }
        }
        if before == ids.len() {
            break;
        }
    }
    ids
}
fn relocate(layout: &mut RangeLayout, selected: Selection, at: [i32; 3]) {
    match selected {
        Selection::Unit(i) => layout.units[i].at = at,
        Selection::Shield(i) => layout.shields[i].at = at,
        Selection::Block(i) => {
            let b = &layout.blocks[i];
            let dx = at[0] - (b.min[0] + b.max[0]) / 2;
            let dz = at[2] - (b.min[2] + b.max[2]) / 2;
            for j in group(layout, i) {
                let b = &mut layout.blocks[j];
                b.min[0] += dx;
                b.max[0] += dx;
                b.min[2] += dz;
                b.max[2] += dz;
            }
        }
    }
}
fn remove(layout: &mut RangeLayout, selected: Selection) {
    match selected {
        Selection::Unit(i) => {
            layout.units.remove(i);
            if let Some(c) = &mut layout.course {
                c.patrols.retain(|p| p.unit != i);
                for p in &mut c.patrols {
                    if p.unit > i {
                        p.unit -= 1;
                    }
                }
            }
        }
        Selection::Block(i) => {
            let mut ids = group(layout, i);
            ids.sort_unstable();
            for j in ids.into_iter().rev() {
                layout.blocks.remove(j);
            }
        }
        Selection::Shield(i) => {
            layout.shields.remove(i);
        }
    }
}

/// Presentation ray only. Half-cell stepping is bounded by the 640-cell map.
/// The authoritative shot uses its separate swept integer collision.
fn pick(sim: &BlastRange, ray: Ray3d) -> Option<([i32; 3], Option<Selection>)> {
    for step in 0..2600 {
        let v = ray.origin + *ray.direction * (step as f32 * 0.5);
        let p = v.floor().as_ivec3().to_array();
        if (0..3).any(|a| p[a] < 0 || p[a] >= SIZE[a] as i32) {
            continue;
        }
        if let Some(i) = sim.units.iter().position(|u| u.solid(p)) {
            return Some((p, Some(Selection::Unit(i))));
        }
        if sim_core::blast::material(&sim.world, p) != 0 {
            let b = sim
                .layout
                .as_ref()
                .and_then(|l| l.blocks.iter().rposition(|b| b.contains(p)))
                .map(Selection::Block);
            // Open ground under a dome picks the dome.
            let dome = sim
                .shields
                .iter()
                .position(|s| {
                    let (dx, dz) = (p[0] - s.at[0], p[2] - s.at[2]);
                    dx * dx + dz * dz <= s.radius * s.radius
                })
                .map(Selection::Shield);
            return Some((p, b.or(dome)));
        }
    }
    None
}

// Bevy injects each resource independently into this system.
#[allow(clippy::too_many_arguments)]
pub fn input(
    mut scene: ResMut<Scene>,
    mut edit: ResMut<Workshop>,
    panel: Option<Res<panel::Panel>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut wheel: MessageReader<MouseWheel>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<GasCamera>>,
) {
    if edit.layout.is_none() {
        wheel.clear();
        return;
    }
    if keys.just_pressed(KeyCode::KeyE) {
        edit.editing = !edit.editing;
        scene.paused = true;
        scene.repeat = false;
        scene.view = 1;
        scene.notice.clear();
        if edit.editing {
            edit.selected = None;
            match range_sim(scene.energy, scene.power, edit.layout.as_ref(), false, None) {
                Ok(sim) => edit.pending = Some(sim),
                Err(e) => scene.notice = e,
            }
        }
    }
    if keys.just_pressed(KeyCode::Home) {
        edit.pan = Vec2::ZERO;
        edit.zoom = 1.0;
        scene.view = 1;
    }
    let blocked = panel.as_ref().is_some_and(|p| p.blocked);
    for ev in wheel.read() {
        if blocked {
            continue;
        }
        let y = ev.y
            * if ev.unit == MouseScrollUnit::Pixel {
                0.01
            } else {
                1.0
            };
        edit.zoom = (edit.zoom * (-y * 0.08).exp()).clamp(0.3, 2.0);
    }
    let Ok(window) = windows.single() else { return };
    let cursor = window.cursor_position();
    let Ok((camera, gt)) = cameras.single() else {
        return;
    };
    if let (Some(a), Some(b)) = (edit.previous_cursor, cursor) {
        if !blocked && mouse.pressed(MouseButton::Middle) {
            if let (Ok(ra), Ok(rb)) = (
                camera.viewport_to_world(gt, a),
                camera.viewport_to_world(gt, b),
            ) {
                let d = ra.origin - rb.origin;
                edit.pan =
                    (edit.pan + Vec2::new(d.x, d.z)).clamp(Vec2::splat(-220.0), Vec2::splat(220.0));
                scene.view = 1;
            }
        }
    }
    edit.previous_cursor = cursor;
    if !edit.editing {
        edit.hover = None;
        if !blocked {
            if let Some(c) = cursor {
                if let Ok(ray) = camera.viewport_to_world(gt, c) {
                    if let Some((hit, selection)) = pick(&scene.sim, ray) {
                        if mouse.just_pressed(MouseButton::Left) {
                            edit.selected = selection;
                        }
                        let fire = panel.as_ref().is_some_and(|p| p.ground_fire);
                        if mouse.just_pressed(MouseButton::Right)
                            || (fire && mouse.just_pressed(MouseButton::Left))
                        {
                            scene.sim.control();
                            scene.paused = false;
                            if fire {
                                if !scene.sim.order_ground_fire(hit) {
                                    scene.notice = "Орудие ещё занято".into();
                                }
                            } else if let Some(Selection::Unit(id)) = selection {
                                scene.sim.order_attack(id);
                                edit.selected = selection;
                            } else if !scene.sim.order_move([hit[0], hit[2]]) {
                                scene.notice = "Нет проезда к этой точке".into();
                            }
                        }
                    }
                }
            }
        }
        return;
    }
    for (key, tool) in [
        (KeyCode::Digit1, Tool::Select),
        (KeyCode::Digit2, Tool::Tank),
        (KeyCode::Digit3, Tool::Strider),
        (KeyCode::Digit4, Tool::Concrete),
        (KeyCode::Digit5, Tool::Steel),
        (KeyCode::Digit6, Tool::Earth),
        (KeyCode::Digit7, Tool::Aim),
        (KeyCode::Digit8, Tool::Shield),
    ] {
        if keys.just_pressed(key) {
            edit.tool = tool;
            scene.notice.clear();
        }
    }
    if keys.just_pressed(KeyCode::KeyQ) {
        edit.rotate = !edit.rotate;
    }
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let cfg = scaled_config(scene.energy, scene.power);
    if ctrl && (keys.just_pressed(KeyCode::KeyZ) || keys.just_pressed(KeyCode::KeyY)) {
        let redo = keys.just_pressed(KeyCode::KeyY);
        if let Err(e) = edit.undo(cfg, redo) {
            scene.notice = e;
        }
        return;
    }
    if keys.just_pressed(KeyCode::F6) {
        let result = edit.layout.as_ref().unwrap().to_ron().and_then(|text| {
            std::fs::create_dir_all("out")
                .and_then(|_| std::fs::write("out/range-layout.ron", text))
                .map_err(|e| e.to_string())
        });
        scene.notice = match result {
            Ok(()) => {
                scene.layout_file = Some("out/range-layout.ron".into());
                "Saved: out/range-layout.ron".into()
            }
            Err(e) => format!("Save failed: {e}"),
        };
    }
    let mut next = edit.layout.as_ref().unwrap().clone();
    if keys.just_pressed(KeyCode::Delete) {
        if let Some(s) = edit.selected {
            remove(&mut next, s);
            edit.selected = None;
        } else {
            return;
        }
    } else {
        if blocked {
            edit.hover = None;
            return;
        }
        let Some(c) = cursor else {
            edit.hover = None;
            return;
        };
        let Ok(ray) = camera.viewport_to_world(gt, c) else {
            return;
        };
        // Placement uses the existing flat test pad, not the top of a wall.
        let t = (sim_core::blast::FLOOR as f32 - ray.origin.y) / ray.direction.y;
        let p = ray.origin + *ray.direction * t;
        let point = pick(&scene.sim, ray).map_or(p, |(hit, _)| Vec3::from(hit.map(|v| v as f32)));
        let x = snap(point.x);
        let z = snap(point.z);
        let at = [x, sim_core::blast::motion::floor(&scene.sim.world, x, z), z];
        edit.hover = Some(at);
        let left = mouse.just_pressed(MouseButton::Left);
        let right = mouse.just_pressed(MouseButton::Right);
        if !left && !right {
            return;
        }
        if (right && edit.selected.is_some())
            || (left && !matches!(edit.tool, Tool::Select | Tool::Aim))
        {
            let inside = |lo: [i32; 2], hi: [i32; 2]| {
                at[0] >= lo[0] && at[0] < hi[0] && at[2] >= lo[1] && at[2] < hi[1]
            };
            if !inside(next.clear_min, next.clear_max)
                && !next.course.as_ref().is_some_and(|c| inside(c.min, c.max))
            {
                scene.notice = "Place inside the prepared test pad; wheel zoom / MMB pan".into();
                return;
            }
        }
        if right {
            if edit.tool != Tool::Select {
                edit.tool = Tool::Select;
                return;
            }
            if let Some(s) = edit.selected {
                relocate(&mut next, s, at);
            } else {
                return;
            }
        } else {
            match edit.tool {
                Tool::Select => {
                    edit.selected = pick(&scene.sim, ray).and_then(|(_, s)| s);
                    return;
                }
                Tool::Tank | Tool::Strider => {
                    next.units.push(RangeUnitDef {
                        kind: if edit.tool == Tool::Tank {
                            RangeUnitKind::Tank
                        } else {
                            RangeUnitKind::Strider
                        },
                        at,
                    });
                }
                Tool::Aim => {
                    let Some((hit, _)) = pick(&scene.sim, ray) else {
                        return;
                    };
                    next.target = hit;
                    next.airburst_on_target = false;
                }
                Tool::Shield => next.shields.push(RangeShieldDef {
                    at,
                    radius: scene.sim.shield_tuning().default_radius,
                }),
                tool => next.blocks.push(block(tool, at, edit.rotate)),
            }
        }
    }
    // Keep additions in the prepared pad: outside terrain is intentionally
    // forested and uneven. Real unit-volume validation catches every overlap.
    let result = edit.commit(next, cfg);
    scene.notice = match result {
        Ok(()) => String::new(),
        Err(e) => format!("Placement rejected: {e}"),
    };
}

fn bounds(giz: &mut Gizmos, min: Vec3, max: Vec3, color: Color) {
    giz.cube(
        Transform::from_translation((min + max) * 0.5).with_scale(max - min),
        color,
    );
}
pub fn overlay(
    mut giz: Gizmos,
    scene: Res<Scene>,
    edit: Res<Workshop>,
    art: Res<WitnessArt>,
    panel: Res<panel::Panel>,
    mut flashes: ResMut<ShieldFlashes>,
) {
    let flat = Quat::from_rotation_arc(Vec3::Z, Vec3::Y);
    // Domes: a wire sphere fading with the charge, red and low while down.
    let capacity = scene.sim.shield_tuning().capacity.max(1) as f32;
    for s in &scene.sim.shields {
        let c = Vec3::from(s.at.map(|v| v as f32));
        let (colour, radius) = if s.down > 0 {
            (Color::srgba(0.9, 0.35, 0.25, 0.4), s.radius as f32 * 0.3)
        } else {
            let k = s.charge as f32 / capacity;
            (
                Color::srgba(0.35 + 0.5 * (1.0 - k), 0.8, 1.0, 0.2 + 0.6 * k),
                s.radius as f32,
            )
        };
        giz.sphere(Isometry3d::from_translation(c), radius, colour)
            .resolution(48);
        giz.circle(Isometry3d::new(c + Vec3::Y * 0.5, flat), radius, colour);
    }
    if flashes.seen != scene.sim.tick {
        flashes.seen = scene.sim.tick;
        for h in &scene.sim.shield_hits {
            let ttl = if h.collapsed { 40 } else { 18 };
            flashes.live.push((Vec3::from(h.at.map(|v| v as f32)), ttl));
        }
    }
    for (p, ttl) in &mut flashes.live {
        let t = (f32::from(*ttl) / 18.0).min(1.0);
        giz.sphere(
            Isometry3d::from_translation(*p),
            2.0 + (1.0 - t) * 6.0,
            Color::srgba(1.0, 0.95, 0.6, t),
        );
        *ttl -= 1;
    }
    flashes.live.retain(|(_, ttl)| *ttl > 0);
    if !edit.editing {
        let p = Vec3::from(scene.sim.gun.motion.pos.map(|v| v.to_f64_lossy() as f32));
        giz.circle(
            Isometry3d::new(p + Vec3::Y * 0.5, flat),
            23.0,
            Color::srgb(0.22, 0.85, 0.68),
        );
        if let Some(goal) = scene.sim.gun.motion.path.last() {
            let p = Vec3::new(
                goal[0] as f32,
                sim_core::blast::motion::floor(&scene.sim.world, goal[0], goal[1]) as f32 + 0.7,
                goal[1] as f32,
            );
            giz.circle(Isometry3d::new(p, flat), 5.0, Color::srgb(0.22, 0.85, 0.68));
        }
        if let Some(i) = scene.sim.attack {
            if let Some(u) = scene.sim.units.get(i) {
                let p = Vec3::from(u.at.map(|v| v as f32));
                giz.circle(
                    Isometry3d::new(p + Vec3::Y, flat),
                    24.0,
                    Color::srgb(1.0, 0.4, 0.22),
                );
            }
        }
    }
    if panel.routes {
        if let Some(c) = scene.sim.layout.as_ref().and_then(|l| l.course.as_ref()) {
            for (j, patrol) in c.patrols.iter().enumerate() {
                for i in 0..patrol.points.len() {
                    let a = patrol.points[i];
                    let b = patrol.points[(i + 1) % patrol.points.len()];
                    let n = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()) / 6).max(1);
                    for k in (0..n).step_by(2) {
                        let p = |t: i32| {
                            let x = a[0] + (b[0] - a[0]) * t / n;
                            let z = a[1] + (b[1] - a[1]) * t / n;
                            Vec3::new(
                                x as f32,
                                sim_core::blast::motion::floor(&scene.sim.world, x, z) as f32 + 0.5,
                                z as f32,
                            )
                        };
                        giz.line(
                            p(k),
                            p(k + 1),
                            if j == 0 {
                                Color::srgba(0.9, 0.68, 0.3, 0.65)
                            } else {
                                Color::srgba(0.36, 0.75, 0.9, 0.65)
                            },
                        );
                    }
                }
            }
        }
    }
    if !edit.editing {
        return;
    }

    let Some(layout) = &edit.layout else { return };
    let cyan = Color::srgb(0.3, 0.85, 0.9);
    let corners = [
        [layout.clear_min[0], layout.clear_min[1]],
        [layout.clear_max[0], layout.clear_min[1]],
        [layout.clear_max[0], layout.clear_max[1]],
        [layout.clear_min[0], layout.clear_max[1]],
    ];
    for i in 0..4 {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        giz.line(
            Vec3::new(a[0] as f32, 16.1, a[1] as f32),
            Vec3::new(b[0] as f32, 16.1, b[1] as f32),
            Color::srgba(0.3, 0.65, 0.7, 0.35),
        );
    }
    let flat = Quat::from_rotation_arc(Vec3::Z, Vec3::Y);
    let at = Vec3::from(layout.target.map(|v| v as f32));
    giz.circle(
        Isometry3d::new(at + Vec3::Y * 0.7, flat),
        5.0,
        Color::srgb(1.0, 0.63, 0.25),
    );
    giz.line(
        at - Vec3::X * 8.0,
        at + Vec3::X * 8.0,
        Color::srgb(1.0, 0.63, 0.25),
    );
    if let Some(s) = edit.selected {
        match s {
            Selection::Unit(i) => {
                if let Some(u) = scene.sim.units.get(i) {
                    let lo = Vec3::from(u.world_pos([0; 3]).map(|v| v as f32));
                    bounds(
                        &mut giz,
                        lo,
                        lo + Vec3::new(
                            u.body.vol.nx as f32,
                            u.body.vol.ny as f32,
                            u.body.vol.nz as f32,
                        ),
                        cyan,
                    );
                }
            }
            Selection::Block(i) => {
                if i < layout.blocks.len() {
                    for j in group(layout, i) {
                        let b = &layout.blocks[j];
                        bounds(
                            &mut giz,
                            Vec3::from(b.min.map(|v| v as f32)),
                            Vec3::from(b.max.map(|v| v as f32)),
                            cyan,
                        );
                    }
                }
            }
            Selection::Shield(i) => {
                if let Some(s) = layout.shields.get(i) {
                    giz.sphere(
                        Isometry3d::from_translation(Vec3::from(s.at.map(|v| v as f32))),
                        s.radius as f32,
                        cyan,
                    );
                }
            }
        }
    }
    if let Some(p) = edit.hover {
        let at = Vec3::from(p.map(|v| v as f32));
        let pad = p[0] >= layout.clear_min[0]
            && p[0] < layout.clear_max[0]
            && p[2] >= layout.clear_min[1]
            && p[2] < layout.clear_max[1];
        let c = if pad {
            cyan
        } else {
            Color::srgb(1.0, 0.4, 0.2)
        };
        match edit.tool {
            Tool::Concrete | Tool::Steel | Tool::Earth => {
                let b = block(edit.tool, p, edit.rotate);
                bounds(
                    &mut giz,
                    Vec3::from(b.min.map(|v| v as f32)),
                    Vec3::from(b.max.map(|v| v as f32)),
                    c,
                );
            }
            Tool::Tank | Tool::Strider => {
                let body = &art.pristine[if edit.tool == Tool::Tank { 0 } else { 1 }];
                let size = Vec3::new(body.vol.nx as f32, body.vol.ny as f32, body.vol.nz as f32);
                let lo = at - Vec3::new(size.x * 0.5, 0.0, size.z * 0.5);
                bounds(&mut giz, lo, lo + size, c);
            }
            Tool::Shield => {
                giz.sphere(
                    Isometry3d::from_translation(at),
                    scene.sim.shield_tuning().default_radius as f32,
                    c,
                );
            }
            _ => {}
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overview_keeps_the_shooter_when_aim_moves_beyond_the_wall() {
        use bevy::camera::{CameraProjection, RenderTargetInfo};
        let (w, c) = fixture();
        let mut layout = w.layout.unwrap();
        layout.target = [480, 24, 380];
        let sim = BlastRange::with_layout(c, layout).unwrap();
        let aim = Vec3::new(339.0, 34.0, 380.0);
        let offset = Vec3::new(-155.0, 250.0, 320.0);
        let (fitted, height) = overview_frame(&sim, aim, offset, 16.0 / 9.0);

        let half = Vec2::new(height * 16.0 / 9.0, height) * 0.5;
        let projection = OrthographicProjection {
            area: Rect::from_corners(-half, half),
            near: 1.0,
            far: 1200.0,
            ..OrthographicProjection::default_3d()
        };
        let mut camera = Camera::default();
        camera.computed.clip_from_view = projection.get_clip_from_view();
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: UVec2::new(1600, 900),
            scale_factor: 1.0,
        });
        let gt = GlobalTransform::from(
            Transform::from_translation(fitted + offset).looking_at(fitted, Vec3::Y),
        );
        for x in [83.0, 173.0] {
            for y in [14.0, 66.0] {
                for z in [306.0, 366.0] {
                    let p = camera.world_to_viewport(&gt, Vec3::new(x, y, z)).unwrap();
                    assert!(
                        p.x > 0.0 && p.x < 1600.0 && p.y > 0.0 && p.y < 900.0,
                        "cropped: {p}"
                    );
                }
            }
        }
    }
    fn fixture() -> (Workshop, ExplosionConfig) {
        (
            Workshop::new(
                Some(
                    RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron"))
                        .unwrap(),
                ),
                true,
            ),
            scaled_config(true, 1000),
        )
    }
    #[test]
    fn rejected_overlap_preserves_layout_history_and_pending() {
        let (mut w, c) = fixture();
        let before = w.layout.clone().unwrap();
        let mut next = before.clone();
        next.units[0].at = [400, 16, 336];
        assert!(w.commit(next, c).is_err());
        assert_eq!(w.layout.as_ref(), Some(&before));
        assert!(w.history.is_empty());
        assert!(w.pending.is_none());
    }
    #[test]
    fn moving_layered_cover_undo_redo_and_serializing_preserve_layers() {
        let (mut w, c) = fixture();
        let before = w.layout.clone().unwrap();
        let mut next = before.clone();
        relocate(&mut next, Selection::Block(0), [400, 16, 340]);
        assert_eq!(next.blocks[0].min[2], 286);
        assert_eq!(next.blocks[1].min[2], 286);
        w.commit(next.clone(), c.clone()).unwrap();
        w.undo(c.clone(), false).unwrap();
        assert_eq!(w.layout.as_ref(), Some(&before));
        w.undo(c, true).unwrap();
        assert_eq!(w.layout.as_ref(), Some(&next));
        assert_eq!(
            RangeLayout::from_ron(&next.to_ron().unwrap()).unwrap(),
            next
        );
        remove(&mut next, Selection::Block(1));
        assert!(next.blocks.is_empty());
    }
    #[test]
    fn bounded_placements_and_click_pick_use_actual_occupancy() {
        let (mut w, c) = fixture();
        let mut next = w.layout.clone().unwrap();
        next.units.push(RangeUnitDef {
            kind: RangeUnitKind::Strider,
            at: [448, 16, 385],
        });
        w.commit(next, c).unwrap();
        let sim = w.pending.take().unwrap();
        let hit = pick(
            &sim,
            Ray3d::new(Vec3::new(439.0, 180.0, 336.0), Dir3::NEG_Y),
        )
        .unwrap();
        assert_eq!(hit.1, Some(Selection::Unit(1)));
        let mut too_many = w.layout.clone().unwrap();
        while too_many.units.len() <= 8 {
            too_many.units.push(too_many.units[0].clone());
        }
        assert!(w.commit(too_many, scaled_config(true, 1000)).is_err());
    }

    #[test]
    fn mouse_and_keyboard_system_places_a_walker_and_undoes_without_a_window_backend() {
        use bevy::camera::{CameraProjection, RenderTargetInfo};
        let (workshop, c) = fixture();
        let original = workshop.layout.clone().unwrap();
        let sim = BlastRange::with_layout(c, original.clone()).unwrap();
        let scene = Scene {
            sim,
            energy: true,
            paused: true,
            frame: 0,
            finish: 420,
            out: None,
            all_frames: false,
            wait: 0,
            captured: 0,
            acc: 0.0,
            repeat: false,
            view: 1,
            patrol: 1,
            previous_shell: None,
            fps: 60.0,
            realtime: true,
            live_elapsed: 0.0,
            last_capture: 0,
            layout_file: Some("data/range-cover.ron".into()),
            power: 1000,
            uncovered: false,
            unit_override: None,
            skip_delta: true,
            notice: String::new(),
        };
        let mut app = App::new();
        app.insert_resource(scene)
            .insert_resource(workshop)
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_message::<MouseWheel>()
            .add_systems(Update, input);
        let projection = OrthographicProjection {
            area: Rect::from_corners(Vec2::new(-320.0, -240.0), Vec2::new(320.0, 240.0)),
            near: 1.0,
            far: 1200.0,
            ..OrthographicProjection::default_3d()
        };
        let mut camera = Camera::default();
        camera.computed.clip_from_view = projection.get_clip_from_view();
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: UVec2::new(1600, 900),
            scale_factor: 1.0,
        });
        let gt = GlobalTransform::from(
            Transform::from_xyz(320.0, 300.0, 336.0)
                .looking_at(Vec3::new(320.0, 0.0, 336.0), Vec3::NEG_Z),
        );
        let screen = camera
            .world_to_viewport(&gt, Vec3::new(448.0, 16.0, 384.0))
            .unwrap();
        let mut window = Window {
            resolution: bevy::window::WindowResolution::new(1600, 900)
                .with_scale_factor_override(1.0),
            ..default()
        };
        window.set_cursor_position(Some(screen));
        app.world_mut().spawn(window);
        app.world_mut().spawn((camera, gt, GasCamera));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Digit3);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        let w = app.world().resource::<Workshop>();
        assert_eq!(
            w.layout.as_ref().unwrap().units.len(),
            original.units.len() + 1,
            "{}",
            app.world().resource::<Scene>().notice
        );
        assert_eq!(
            w.layout.as_ref().unwrap().units[original.units.len()].kind,
            RangeUnitKind::Strider
        );
        assert_eq!(
            w.layout.as_ref().unwrap().units[original.units.len()].at,
            [448, 16, 384]
        );
        assert!(w.pending.is_some());
        *app.world_mut().resource_mut::<ButtonInput<MouseButton>>() = default();
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        *keys = default();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::KeyZ);
        app.update();
        assert_eq!(
            app.world().resource::<Workshop>().layout.as_ref(),
            Some(&original)
        );
        // The same input system accepts RTS commands with no keyboard keys.
        app.insert_resource(panel::Panel::new(true, false))
            .insert_resource(GasInput::default());
        app.add_systems(
            Update,
            (
                panel::actions.before(input),
                panel::release_keys.after(input),
            ),
        );
        *app.world_mut().resource_mut::<ButtonInput<KeyCode>>() = default();
        *app.world_mut().resource_mut::<ButtonInput<MouseButton>>() = default();
        app.world_mut().resource_mut::<Workshop>().pending = None;
        app.world_mut()
            .spawn((panel::Action::Play, Interaction::Pressed));
        app.update();
        assert!(!app.world().resource::<Workshop>().editing);
        assert!(app.world().resource::<Scene>().sim.player);
        assert_eq!(app.world().resource::<Scene>().sim.fire_tick, u32::MAX);
        let screen = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<(&Camera, &GlobalTransform), With<GasCamera>>();
            let (c, g) = q.single(world).unwrap();
            c.world_to_viewport(g, Vec3::new(245.0, 16.0, 340.0))
                .unwrap()
        };
        {
            let world = app.world_mut();
            let mut q = world.query::<&mut Window>();
            q.single_mut(world)
                .unwrap()
                .set_cursor_position(Some(screen));
        }
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        app.update();
        {
            let mut scene = app.world_mut().resource_mut::<Scene>();
            for _ in 0..350 {
                scene.sim.step();
            }
            assert!(
                (scene.sim.gun.at[0] - 245).abs() < 8,
                "mouse move {:?}",
                scene.sim.gun.at
            );
        }
        let screen = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<(&Camera, &GlobalTransform), With<GasCamera>>();
            let (c, g) = q.single(world).unwrap();
            c.world_to_viewport(g, Vec3::new(359.0, 40.0, 312.0))
                .unwrap()
        };
        {
            let world = app.world_mut();
            let mut q = world.query::<&mut Window>();
            q.single_mut(world)
                .unwrap()
                .set_cursor_position(Some(screen));
        }
        *app.world_mut().resource_mut::<ButtonInput<MouseButton>>() = default();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        app.update();
        assert_eq!(app.world().resource::<Scene>().sim.attack, Some(0));
        // A click on the panel executes its action and cannot issue a ground order.
        {
            let world = app.world_mut();
            let mut q = world.query::<&mut Window>();
            q.single_mut(world)
                .unwrap()
                .set_cursor_position(Some(Vec2::new(40.0, 40.0)));
        }
        app.world_mut()
            .spawn((panel::Action::Stop, Interaction::Pressed));
        app.update();
        assert!(app.world().resource::<Scene>().sim.attack.is_none());
        assert!(app
            .world()
            .resource::<Scene>()
            .sim
            .gun
            .motion
            .path
            .is_empty());
    }
}
