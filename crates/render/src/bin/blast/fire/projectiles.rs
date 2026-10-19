//! Screen-sized, depth-tested strategic ammunition dots. Presentation only.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct View {
    eye: Vec3,
    rotation: Quat,
    height: f32,
    aspect: f32,
    pixel: f32,
    near: f32,
    far: f32,
}
impl View {
    pub fn camera(transform: &Transform, camera: &Camera, projection: &Projection) -> Option<Self> {
        let Projection::Orthographic(p) = projection else {
            return None;
        };
        let size = camera.logical_viewport_size()?;
        if size.x <= 0.0 || size.y <= 0.0 {
            return None;
        }
        // Framing changes the mode before Bevy refreshes projection.area.
        let height = match p.scaling_mode {
            bevy::camera::ScalingMode::FixedVertical { viewport_height } => {
                viewport_height * p.scale
            }
            _ => p.area.height(),
        };
        if !height.is_finite() || height <= 0.0 {
            return None;
        }
        Some(Self {
            eye: transform.translation,
            rotation: transform.rotation,
            height,
            aspect: size.x / size.y,
            pixel: height / size.y,
            near: p.near,
            far: p.far,
        })
    }
    fn alpha(self, edge: f32) -> f32 {
        let projected = edge / self.pixel;
        // Fade both ends: no enlarged dots at close range, and no sea of
        // tiny machine-gun bullets at the furthest strategic scales.
        smooth01((projected - 0.08) / 0.12) * (1.0 - smooth01((projected - 0.7) / 1.8))
    }
    fn visible(self, at: Vec3) -> bool {
        let p = self.rotation.inverse() * (at - self.eye);
        let margin = self.pixel * 3.0;
        -p.z >= self.near
            && -p.z <= self.far
            && p.x.abs() <= self.height * self.aspect * 0.5 + margin
            && p.y.abs() <= self.height * 0.5 + margin
    }
    fn dot(self, mesh: &mut MeshBuf, at: Vec3, edge: f32) {
        let alpha = self.alpha(edge);
        if alpha <= 0.001 || !self.visible(at) {
            return;
        }
        let right = self.rotation * Vec3::X;
        let up = self.rotation * Vec3::Y;
        let normal = self.rotation * Vec3::Z;
        for (diameter, colour) in [
            (4.2, [1.1, 0.55, 0.025, alpha * 0.25]),
            (2.8, [2.0, 1.45, 0.055, alpha * 0.95]),
        ] {
            let r = right * (self.pixel * diameter * 0.5);
            let u = up * (self.pixel * diameter * 0.5);
            let start = mesh.pos.len() as u32;
            for p in [at - r - u, at + r - u, at + r + u, at - r + u] {
                mesh.pos.push(p.to_array());
                mesh.nrm.push(normal.to_array());
                mesh.col.push(colour);
            }
            mesh.idx
                .extend([start, start + 1, start + 2, start, start + 2, start + 3]);
        }
    }
}
pub(super) fn append(
    mesh: &mut MeshBuf,
    sim: &BlastRange,
    view: View,
    fraction: f32,
    single: (Option<Vec3>, f32),
) {
    let fraction = fraction.clamp(0.0, 1.0);
    if sim.battle_mode() {
        for round in &sim.battle.rounds {
            let edge = match round.weapon {
                BattleWeapon::MachineGun => 0.7,
                BattleWeapon::Canister => 2.1,
                BattleWeapon::Siege => 3.2,
                BattleWeapon::Nuclear => 8.0,
                BattleWeapon::Blast | BattleWeapon::Kinetic => 2.0,
                BattleWeapon::Energy => 4.0,
                BattleWeapon::Annihilation => 6.0,
                BattleWeapon::Laser => continue,
            };
            view.dot(
                mesh,
                point(round.previous).lerp(point(round.pos), fraction),
                edge,
            );
        }
    } else if sim.config.delivery == Delivery::MachineGun {
        for round in &sim.streams.rounds {
            let head = point(round.pos);
            // Straight bullet interpolation stays behind the surviving head.
            let tail = head - point(round.velocity) / 30.0;
            view.dot(mesh, tail.lerp(head, fraction), 0.7);
        }
    } else if matches!(sim.config.delivery, Delivery::Blast | Delivery::Kinetic) {
        if let Some(shell) = sim.shell {
            let head = point(shell);
            view.dot(
                mesh,
                single.0.unwrap_or(head).lerp(head, fraction),
                single.1,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn view(pixel: f32) -> View {
        View {
            eye: Vec3::new(0.0, 0.0, 1000.0),
            rotation: Quat::IDENTITY,
            height: pixel * 720.0,
            aspect: 16.0 / 9.0,
            pixel,
            near: 1.0,
            far: 4000.0,
        }
    }
    #[test]
    fn live_shell_dot_interpolates_without_changing_sim_and_disappears_on_contact() {
        let mut sim = BlastRange::new(weapon_config(false, false, 1000, Delivery::Kinetic));
        for _ in 0..120 {
            sim.step();
            if sim.shell.is_some() {
                break;
            }
        }
        let head = point(sim.shell.expect("live solid round"));
        let tail = head - Vec3::new(12.0, 0.0, 0.0);
        let mut camera = view(5.0);
        camera.eye = head + Vec3::Z * 100.0;
        let hash = sim.state_hash();
        let mut mesh = MeshBuf::default();
        append(&mut mesh, &sim, camera, 0.25, (Some(tail), 2.0));
        assert_eq!(mesh.pos.len(), 8);
        let centre = mesh.pos[..4].iter().map(|&p| Vec3::from(p)).sum::<Vec3>() / 4.0;
        assert!(centre.distance(tail.lerp(head, 0.25)) < 0.001);
        assert_eq!(sim.state_hash(), hash);
        for _ in 0..240 {
            sim.step();
            if sim.impact.is_some() {
                break;
            }
        }
        assert!(sim.impact.is_some());
        let mut after = MeshBuf::default();
        append(&mut after, &sim, camera, 0.5, (Some(tail), 2.0));
        assert!(
            after.is_empty(),
            "contact must not leave a strategic dot behind"
        );
    }
    #[test]
    fn frozen_frame_key_changes_on_camera_resize_pan_or_toggle() {
        let v = view(5.0);
        let frame = (10.0, Some(v), true);
        assert_ne!(frame, (10.0, Some(v), false));
        let mut resized = v;
        resized.pixel *= 2.0;
        assert_ne!(frame, (10.0, Some(resized), true));
        let mut moved = v;
        moved.eye.x += 1.0;
        assert_ne!(frame, (10.0, Some(moved), true));
    }
    #[test]
    fn zoomed_small_layout_keeps_the_projectile_inside_the_camera_far_plane() {
        assert_eq!(framing_far(1200.0, Vec3::Z * 600.0, 300.0, 192), 1200.0);
        let distance = 2400.0;
        let mut camera = view(5.0);
        camera.eye = Vec3::Z * distance;
        camera.far = 1200.0;
        assert!(!camera.visible(Vec3::ZERO));
        camera.far = framing_far(1200.0, camera.eye, camera.height, 192);
        assert!(camera.visible(Vec3::ZERO));
        assert_eq!(
            framing_far(30_000.0, Vec3::Z * 9000.0, 11000.0, 192),
            30_000.0
        );
    }
    #[test]
    fn strategic_dots_keep_screen_size_and_filter_small_ammunition_first() {
        assert_eq!(view(0.1).alpha(2.0), 0.0);
        assert!(view(1.0).alpha(2.0) > 0.1);
        assert_eq!(view(10.0).alpha(0.7), 0.0);
        assert!(view(10.0).alpha(2.0) > 0.99);
        assert_eq!(view(100.0).alpha(6.0), 0.0);
        for pixel in [1.0, 5.0, 10.0] {
            let mut buf = MeshBuf::default();
            view(pixel).dot(&mut buf, Vec3::ZERO, 2.0);
            assert_eq!(buf.pos.len(), 8);
            assert!(((buf.pos[1][0] - buf.pos[0][0]) / pixel - 4.2).abs() < 0.001);
        }
        assert!(!view(1.0).visible(Vec3::new(0.0, 0.0, 1001.0)));
        assert!(!view(1.0).visible(Vec3::new(3000.0, 0.0, 0.0)));
    }
}
