//! Connected convective silhouette and a separate low, broken dust front.
use super::{
    volume::{cell_noise, Volume},
    *,
};

fn roll_offset(phase: f32, radius: f32) -> Vec2 {
    // Across the top the roll travels outward; at the outer edge it turns
    // down. The previous signs pulled the top inward, like a closing iris.
    Vec2::new(-phase.cos(), phase.sin() * 0.85) * radius
}

/// Separable waves: sin(x+y) and cos(z+y) use short axis tables instead
/// of four transcendental calls at every exposed cloud cell. No world cache.
struct Folds {
    x: [[f32; 4]; 49],
    y: [[f32; 8]; 57],
    z: [[f32; 4]; 49],
}
fn terms(a: f32, b: f32) -> [f32; 4] {
    let (sa, ca) = a.sin_cos();
    let (sb, cb) = b.sin_cos();
    [sa, ca, sb, cb]
}
impl Folds {
    fn new(origin: Vec3, edge: f32, crown: Vec3, radius: f32, age: f32) -> Self {
        let local = |axis, q| (origin[axis] + (q as f32 + 0.5) * edge - crown[axis]) / radius;
        Self {
            x: std::array::from_fn(|q| {
                let v = local(0, q);
                terms(v * 6.0, v * 16.0)
            }),
            z: std::array::from_fn(|q| {
                let v = local(2, q);
                terms(v * 6.5, v * 14.0)
            }),
            y: std::array::from_fn(|q| {
                let v = local(1, q);
                let a = terms(v * 9.0 - age * 0.22, -v * 7.0 + age * 0.16);
                let b = terms(-v * 11.0 + age * 0.12, v * 13.0 - age * 0.10);
                [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]]
            }),
        }
    }
    fn at(&self, q: IVec3) -> (f32, f32) {
        let (x, y, z) = (
            self.x[q.x as usize],
            self.y[q.y as usize],
            self.z[q.z as usize],
        );
        (
            (x[0] * y[1] + x[1] * y[0]) * (z[1] * y[3] - z[0] * y[2]),
            (x[2] * y[5] + x[3] * y[4]) * (z[3] * y[7] - z[2] * y[6]),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crown_convection_moves_outward_over_the_top_and_down_at_the_rim() {
        let top = std::f32::consts::FRAC_PI_2;
        assert!(roll_offset(top + 0.01, 1.0).x > roll_offset(top - 0.01, 1.0).x);
        let rim = std::f32::consts::PI;
        assert!(roll_offset(rim + 0.01, 1.0).y < roll_offset(rim - 0.01, 1.0).y);
    }
    #[test]
    fn axis_fold_tables_preserve_the_advected_colour_field() {
        let origin = Vec3::new(-1234.0, 80.0, 4567.0);
        for age in [0.5, 3.0, 6.0, 18.0, 25.0] {
            let shape = Development::at(age);
            let crown = origin + Vec3::new(900.0, shape.height, 1200.0);
            let edge = 57.25;
            let folds = Folds::new(origin, edge, crown, shape.radius, age);
            for y in 0..57 {
                for z in (0..49).step_by(3) {
                    for x in (0..49).step_by(3) {
                        let q = IVec3::new(x, y, z);
                        let p = origin + (q.as_vec3() + Vec3::splat(0.5)) * edge;
                        let local = (p - crown) / shape.radius;
                        let expected = (
                            (local.x * 6.0 + local.y * 9.0 - age * 0.22).sin()
                                * (local.z * 6.5 - local.y * 7.0 + age * 0.16).cos(),
                            (local.x * 16.0 - local.y * 11.0 + age * 0.12).sin()
                                * (local.z * 14.0 + local.y * 13.0 - age * 0.10).cos(),
                        );
                        let actual = folds.at(q);
                        assert!(
                            (actual.0 - expected.0).abs() < 0.0001
                                && (actual.1 - expected.1).abs() < 0.0001
                        );
                    }
                }
            }
        }
    }
}

pub(super) fn append(
    buf: &mut MeshBuf,
    at: Vec3,
    age: f32,
    shape: Development,
    fade: f32,
    pixel: f32,
) {
    let Development {
        height,
        radius,
        grow,
        crown: rollout,
    } = shape;
    let opacity = smooth01((age - 0.45) / 1.6) * fade;
    if opacity <= 0.001 {
        return;
    }
    let full_edge = (radius * 3.2 / 48.0).max((height + radius * 0.65) / 56.0);
    // Keep every stage and billow in the silhouette; only the sampling grid
    // changes when individual cubes become smaller than two screen pixels.
    let (horizontal, vertical) = if pixel > full_edge {
        (24, 28)
    } else if pixel * 2.0 > full_edge {
        (32, 38)
    } else {
        (48, 56)
    };
    let edge = (radius * 3.2 / horizontal as f32).max((height + radius * 0.65) / vertical as f32);
    let origin = at
        + Vec3::new(
            -(horizontal + 1) as f32 * edge * 0.5,
            0.0,
            -(horizontal + 1) as f32 * edge * 0.5,
        );
    let mut cloud = Volume::new(
        origin,
        edge,
        IVec3::new(horizontal + 1, vertical + 1, horizontal + 1),
    );
    let lean = Vec3::new(height * 0.055, 0.0, height * 0.025) * grow;
    // Broad foot, constricted column, then a continuous flared shoulder.
    let root = smooth01((age - 0.55) / 1.8);
    if root > 0.01 {
        cloud.ellipsoid(
            at + Vec3::Y * height * 0.08,
            Vec3::new(radius * 0.32, height * 0.14, radius * 0.3) * root,
        );
    }
    for level in 0..14u32 {
        let t = (level as f32 + 0.5) / 14.0;
        let feed = smooth01((age - 0.65 - t * 1.3) / 1.8);
        if feed <= 0.01 {
            continue;
        }
        let width = (105.0 + 110.0 * t * t + 80.0 * (1.0 - t).powi(4)) * (0.35 + grow) * feed;
        let swirl = Vec3::new(
            (t * 9.0 - age * 0.16).sin(),
            0.0,
            (t * 7.0 - age * 0.12 + 1.0).cos(),
        ) * width
            * 0.24;
        cloud.ellipsoid(
            at + Vec3::Y * (height * t) + lean * t * t + swirl,
            Vec3::new(
                width * (0.86 + noise(level + 315) * 0.28),
                height / 13.0 + 35.0,
                width * 0.92,
            ),
        );
    }
    let crown = at + Vec3::Y * height + lean;
    let cap_radius = radius * (0.32 + rollout * 0.68);
    cloud.ellipsoid(
        crown + Vec3::Y * radius * 0.07,
        Vec3::new(
            cap_radius * 0.84,
            radius * (0.36 - rollout * 0.18),
            cap_radius * 0.80,
        ),
    );
    // Large interlocking rolls; uneven spacing/height removes necklace rings.
    for (ring, count) in [8u32, 14, 18].into_iter().enumerate() {
        let ring = ring as u32;
        for i in 0..count {
            let seed = ring * 71 + i;
            let forming = smooth01((age - 1.3 - ring as f32 * 0.7 - noise(seed + 851) * 1.0) / 2.6);
            if forming <= 0.01 {
                continue;
            }
            let angle = (i as f32 + noise(seed + 77) * 0.7) * std::f32::consts::TAU / count as f32
                + ring as f32 * 0.47
                + age * 0.018;
            let radial = Vec3::new(angle.cos(), 0.0, angle.sin());
            let spread = cap_radius * (0.23 + ring as f32 * 0.35 + (noise(seed + 51) - 0.5) * 0.18);
            let roll = age * 0.36 + i as f32 * 1.7;
            let orbit = roll_offset(
                roll,
                radius * (0.045 + ring as f32 * 0.035) * forming * rollout,
            );
            // A higher, broken crown feeds an overhanging rim. The rim rolls
            // out and down, rather than stacking three equally solid discs.
            let size = radius * (0.19 + noise(seed + 600) * 0.13) * forming;
            let lift = [0.20, 0.055, -0.13][ring as usize];
            let centre = crown
                + radial * (spread + orbit.x)
                + Vec3::Y * (radius * (lift + (noise(seed + 405) - 0.5) * 0.16) + orbit.y);
            cloud.ellipsoid(
                centre,
                Vec3::new(size * 1.10, size * (0.88 + noise(seed + 21) * 0.22), size),
            );
        }
    }
    // The young ball exposes too few cells to amortize the full axis tables.
    let folds = (rollout > 0.0).then(|| Folds::new(origin, edge, crown, radius, age));
    let cooling = smooth01(age / 13.0);
    cloud.append(buf, |p, q| {
        let t = ((p.y - at.y) / height).clamp(0.0, 1.4);
        let local = (p - crown) / radius;
        // Advected broad folds, restrained voxel grain: large soot pockets
        // should read before tiny blocks. The underside remains visibly dark.
        let (fold, ripple) = if let Some(folds) = &folds {
            folds.at(q)
        } else {
            (
                (local.x * 6.0 + local.y * 9.0 - age * 0.22).sin()
                    * (local.z * 6.5 - local.y * 7.0 + age * 0.16).cos(),
                (local.x * 16.0 - local.y * 11.0 + age * 0.12).sin()
                    * (local.z * 14.0 + local.y * 13.0 - age * 0.10).cos(),
            )
        };
        let cap = smooth01((t - 0.84) / 0.34);
        let foot = 1.0 - smooth01(t / 0.38);
        let column = Vec3::new(0.105, 0.095, 0.08).lerp(Vec3::new(0.19, 0.145, 0.09), foot);
        let ash = column.lerp(
            Vec3::new(0.30, 0.27, 0.22).lerp(Vec3::new(0.34, 0.335, 0.32), cooling),
            cap,
        );
        let ember = (1.0 - smooth01(age / 10.0))
            * (1.0 - ((t - 0.91) / 0.23).abs()).max(0.0)
            * (0.55 + fold * 0.45);
        let rgb = ash * (0.86 + fold * 0.22 + ripple * 0.055)
            + Vec3::splat((cell_noise(q / 3) - 0.5) * 0.012)
            + Vec3::new(1.8, 0.26, 0.015) * ember;
        let fringe = 1.0 - smooth01((Vec2::new(local.x, local.z).length() - 0.85) / 0.55);
        let density = (0.82 + fold * 0.13 + ripple * 0.05)
            * (1.0 - smooth01((age - 7.0) / 12.0) * (1.0 - cap) * 0.32);
        rgb.extend(opacity * (0.14 + fringe * 0.17) * density)
            .to_array()
    });
}
