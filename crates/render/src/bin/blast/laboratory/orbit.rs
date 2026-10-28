//! Editor view: orbit a fixed selection pivot, without rotating authored matter.
use super::*;

pub const FOV: f32 = 50.0;
pub struct View {
    pub pivot: Vec3,
    pub distance: f32,
    yaw: f32,
    pitch: f32,
    dragging: bool,
    previous: Option<Vec2>,
    saved_camera: Option<Transform>,
    saved_flying: bool,
}
impl View {
    pub fn pose(&self) -> Transform {
        let offset = Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.cos() * self.pitch.cos(),
        ) * self.distance;
        Transform::from_translation(self.pivot + offset).looking_at(self.pivot, Vec3::Y)
    }
    fn rotate(&mut self, delta: Vec2) {
        self.yaw = (self.yaw - delta.x * 0.006).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + delta.y * 0.006).clamp(-1.52, 1.52);
    }
    fn pan(&mut self, delta: Vec2, height: f32) {
        let pose = self.pose();
        let scale = 2.0 * self.distance * (FOV.to_radians() * 0.5).tan() / height.max(1.0);
        self.pivot += (-*pose.right() * delta.x + *pose.up() * delta.y) * scale;
    }
    fn zoom(&mut self, delta: f32) {
        self.distance = (self.distance * (-delta * 0.12).exp()).clamp(1.2, 4096.0);
    }
}
fn bounds(lab: &Laboratory) -> Option<(Vec3, Vec3)> {
    let item = lab
        .constructor
        .selected
        .and_then(|i| lab.constructor.placed.get(i))?;
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for b in item.element.blocks.iter().filter(|b| b.material != 0) {
        for x in [b.min[0], b.max[0] - 1] {
            for y in [b.min[1], b.max[1] - 1] {
                for z in [b.min[2], b.max[2] - 1] {
                    let p = Vec3::from(
                        modding::laboratory::catalog::voxels::world_cell(item, [x, y, z])
                            .map(|v| v as f32),
                    );
                    lo = lo.min(p);
                    hi = hi.max(p + Vec3::ONE);
                }
            }
        }
    }
    lo.is_finite().then_some((lo, hi))
}
pub fn frame(lab: &mut Laboratory) {
    let Some((lo, hi)) = bounds(lab) else {
        return;
    };
    let pivot = (lo + hi) * 0.5;
    // Fit the complete bounding sphere with room for the editor's panels.
    let distance =
        ((hi - lo).length() * 0.5 / (FOV.to_radians() * 0.5).sin() * 2.0).clamp(3.0, 4096.0);
    if let Some(view) = lab.tools.orbit.as_mut() {
        view.pivot = pivot;
        view.distance = distance;
        view.dragging = false;
        view.previous = None;
    } else {
        lab.tools.orbit = Some(View {
            pivot,
            distance,
            yaw: -std::f32::consts::FRAC_PI_4,
            pitch: 0.48,
            dragging: false,
            previous: None,
            saved_camera: lab.tools.camera,
            saved_flying: lab.tools.flying,
        });
    }
    lab.tools.flying = false;
    lab.tools.camera = lab.tools.orbit.as_ref().map(View::pose);
}
pub fn leave(lab: &mut Laboratory) {
    if let Some(view) = lab.tools.orbit.take() {
        lab.tools.camera = view.saved_camera;
        lab.tools.flying = view.saved_flying;
    }
}
pub fn input(
    lab: &mut Laboratory,
    window: &Window,
    over_ui: bool,
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    delta: Option<Vec2>,
    wheel: &mut MessageReader<bevy::input::mouse::MouseWheel>,
) {
    if lab.tools.orbit.is_none() {
        frame(lab);
    }
    let blocked = !window.focused || lab.browser || lab.ui.help || lab.ui.editing.is_some();
    let Some(view) = lab.tools.orbit.as_mut() else {
        wheel.clear();
        return;
    };
    let cursor = window.cursor_position();
    if blocked {
        view.dragging = false;
        view.previous = None;
        wheel.clear();
        return;
    }
    if !mouse.pressed(MouseButton::Middle) {
        view.dragging = false;
    }
    let start = mouse.just_pressed(MouseButton::Middle) && !over_ui;
    if start {
        view.dragging = true;
    }
    if view.dragging && !start {
        let motion = delta
            .or_else(|| view.previous.zip(cursor).map(|(a, b)| b - a))
            .unwrap_or_default();
        if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
            view.pan(motion, window.height());
        } else if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) {
            view.zoom(-motion.y * 0.04);
        } else {
            view.rotate(motion);
        }
    }
    for e in wheel.read() {
        if !over_ui {
            view.zoom(
                e.y * if e.unit == bevy::input::mouse::MouseScrollUnit::Pixel {
                    0.01
                } else {
                    1.0
                },
            );
        }
    }
    view.previous = cursor;
    lab.tools.camera = Some(view.pose());
}

#[cfg(test)]
#[path = "orbit_tests.rs"]
mod tests;
