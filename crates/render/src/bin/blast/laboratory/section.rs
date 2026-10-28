//! Camera-facing section view; this never changes simulation cells.
use super::*;

#[derive(Default)]
pub struct State {
    pub enabled: bool,
    pub anchor: [i32; 3],
    pub offset: i32,
}

fn anchor(lab: &Laboratory, scene: &Scene) -> [i32; 3] {
    lab.tools
        .selected
        .or_else(|| {
            lab.constructor
                .selected
                .and_then(|i| lab.constructor.placed.get(i).map(|p| p.at))
        })
        .or_else(|| scene.sim.layout.as_ref().map(|layout| layout.target))
        .unwrap_or_else(|| {
            let size = scene.sim.size();
            [
                size[0] as i32 / 2,
                scene.sim.ground_level(),
                size[2] as i32 / 2,
            ]
        })
}

pub fn toggle(lab: &mut Laboratory, scene: &Scene) {
    lab.tools.section.enabled = !lab.tools.section.enabled;
    if lab.tools.section.enabled {
        focus(lab, scene);
    }
}

pub fn focus(lab: &mut Laboratory, scene: &Scene) {
    let point = anchor(lab, scene);
    lab.tools.section.anchor = point;
    lab.tools.section.offset = 0;
}

pub fn shift(lab: &mut Laboratory, delta: i32) {
    lab.tools.section.offset = lab
        .tools
        .section
        .offset
        .saturating_add(delta)
        .clamp(-100_000, 100_000);
}

fn near_plane(state: &State, transform: &Transform, far: f32) -> f32 {
    let anchor = Vec3::from(state.anchor.map(|v| v as f32)) + Vec3::splat(0.5);
    let distance = (anchor - transform.translation).dot(*transform.forward());
    (distance + state.offset as f32).clamp(1.0, (far - 1.0).max(1.0))
}

/// Orthographic camera only: perspective/live tools retain their normal near plane.
pub fn camera(
    lab: Option<Res<Laboratory>>,
    mut cameras: Query<(&Transform, &mut Projection), With<GasCamera>>,
) {
    for (transform, mut projection) in &mut cameras {
        if let Projection::Orthographic(p) = &mut *projection {
            p.near = lab
                .as_ref()
                .filter(|l| l.active() && l.tools.section.enabled)
                .map_or(1.0, |l| near_plane(&l.tools.section, transform, p.far));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_tracks_camera_and_clamps_to_the_visible_range() {
        let transform = Transform::from_xyz(0.0, 0.0, 100.0).looking_at(Vec3::ZERO, Vec3::Y);
        let mut state = State {
            enabled: true,
            anchor: [0, 0, 0],
            offset: 0,
        };
        assert!((near_plane(&state, &transform, 1000.0) - 99.5).abs() < 0.001);
        state.offset = 10;
        assert!((near_plane(&state, &transform, 1000.0) - 109.5).abs() < 0.001);
        state.offset = i32::MAX;
        assert_eq!(near_plane(&state, &transform, 1000.0), 999.0);
        state.offset = i32::MIN;
        assert_eq!(near_plane(&state, &transform, 1000.0), 1.0);
    }
}
