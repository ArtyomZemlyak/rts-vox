//! One cubic surface for overlapping billows; hidden internal faces never draw.
use super::*;

pub(super) struct Volume {
    origin: Vec3,
    edge: f32,
    dims: IVec3,
    rows: Vec<u64>,
}
impl Volume {
    pub(super) fn new(origin: Vec3, edge: f32, dims: IVec3) -> Self {
        assert!(dims.x > 0 && dims.x <= 64 && dims.y > 0 && dims.z > 0);
        Self {
            origin,
            edge,
            dims,
            rows: vec![0; (dims.y * dims.z) as usize],
        }
    }
    #[cfg(test)]
    fn contains(&self, p: IVec3) -> bool {
        p.cmpge(IVec3::ZERO).all()
            && p.cmplt(self.dims).all()
            && self.rows[(p.z * self.dims.y + p.y) as usize] & (1 << p.x) != 0
    }
    fn centre(&self, p: IVec3) -> Vec3 {
        self.origin + (p.as_vec3() + Vec3::splat(0.5)) * self.edge
    }
    pub(super) fn ellipsoid(&mut self, at: Vec3, size: Vec3) {
        let inverse = Vec3::ONE / size;
        let lo = ((at - size * 1.05 - self.origin) / self.edge)
            .floor()
            .as_ivec3()
            .max(IVec3::ZERO);
        let hi = ((at + size * 1.05 - self.origin) / self.edge)
            .ceil()
            .as_ivec3()
            .min(self.dims);
        if lo.cmpge(hi).any() {
            return;
        }
        for z in lo.z..hi.z {
            for y in lo.y..hi.y {
                let row = (z * self.dims.y + y) as usize;
                let mask = (u64::MAX << lo.x) & (u64::MAX >> (64 - hi.x));
                let mut candidates = mask & !self.rows[row];
                while candidates != 0 {
                    let x = candidates.trailing_zeros() as i32;
                    candidates &= candidates - 1;
                    let p = IVec3::new(x, y, z);
                    let distance = ((self.centre(p) - at) * inverse).length_squared();
                    if distance <= 0.95
                        || (distance <= 1.05 && distance <= 1.0 + (cell_noise(p / 2) - 0.5) * 0.1)
                    {
                        self.rows[row] |= 1 << x;
                    }
                }
            }
        }
    }
    pub(super) fn fill_columns(&mut self, surface: impl Fn(Vec3) -> f32) {
        self.rows.fill(0);
        for z in 0..self.dims.z {
            for x in 0..self.dims.x {
                let height = surface(self.centre(IVec3::new(x, 0, z)));
                for y in 0..self.dims.y {
                    let p = IVec3::new(x, y, z);
                    if self.centre(p).y <= height {
                        self.rows[(z * self.dims.y + y) as usize] |= 1 << x;
                    }
                }
            }
        }
    }
    pub(super) fn append(&self, buf: &mut MeshBuf, colour: impl Fn(Vec3, IVec3) -> [f32; 4]) {
        for z in 0..self.dims.z {
            for y in 0..self.dims.y {
                let row = (z * self.dims.y + y) as usize;
                let cells = self.rows[row];
                if cells == 0 {
                    continue;
                }
                let neighbour = |dy: i32, dz: i32| {
                    let (y, z) = (y + dy, z + dz);
                    if y >= 0 && y < self.dims.y && z >= 0 && z < self.dims.z {
                        self.rows[(z * self.dims.y + y) as usize]
                    } else {
                        0
                    }
                };
                // Each bit is an exposed face. Whole interior rows/cells
                // disappear before coordinate, colour or vertex work.
                let faces = [
                    cells & !(cells << 1),
                    cells & !(cells >> 1),
                    cells & !neighbour(-1, 0),
                    cells & !neighbour(1, 0),
                    cells & !neighbour(0, -1),
                    cells & !neighbour(0, 1),
                ];
                let mut boundary = faces.iter().fold(0, |all, f| all | f);
                while boundary != 0 {
                    let x = boundary.trailing_zeros() as i32;
                    boundary &= boundary - 1;
                    let p = IVec3::new(x, y, z);
                    let centre = self.centre(p);
                    let tint = colour(centre, p);
                    for axis in 0..3 {
                        for (side, sign) in [-1, 1].into_iter().enumerate() {
                            if faces[axis * 2 + side] & (1 << x) == 0 {
                                continue;
                            }
                            let mut n = Vec3::ZERO;
                            n[axis] = sign as f32;
                            let mut a = Vec3::ZERO;
                            let mut b = Vec3::ZERO;
                            a[(axis + 1) % 3] = self.edge * 0.5;
                            b[(axis + 2) % 3] = self.edge * 0.5 * sign as f32;
                            let face = centre + n * self.edge * 0.5;
                            let start = buf.pos.len() as u32;
                            let shade = if axis == 1 && sign > 0 {
                                1.0
                            } else if axis == 0 {
                                0.88
                            } else {
                                0.76
                            };
                            buf.pos.extend(
                                [face - a - b, face + a - b, face + a + b, face - a + b]
                                    .map(|v| v.to_array()),
                            );
                            buf.nrm.extend([n.to_array(); 4]);
                            let shaded =
                                [tint[0] * shade, tint[1] * shade, tint[2] * shade, tint[3]];
                            buf.col.extend([shaded; 4]);
                            buf.idx.extend([
                                start,
                                start + 1,
                                start + 2,
                                start,
                                start + 2,
                                start + 3,
                            ]);
                        }
                    }
                }
            }
        }
    }
}
pub(super) fn cell_noise(p: IVec3) -> f32 {
    noise(
        (p.x as u32).wrapping_mul(73856093)
            ^ (p.y as u32).wrapping_mul(19349663)
            ^ (p.z as u32).wrapping_mul(83492791),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packed_rows_match_scalar_occupancy_at_edges_and_overlapping_billows() {
        for width in [1, 12, 49, 57, 64] {
            let mut v = Volume::new(Vec3::new(-7.0, 2.0, 3.0), 1.7, IVec3::new(width, 9, 7));
            let shapes = [
                (Vec3::new(-8.0, 5.0, 7.0), Vec3::splat(4.0)),
                (Vec3::new(2.0, 8.0, 11.0), Vec3::new(9.0, 5.0, 3.0)),
                (v.centre(IVec3::new(width - 1, 5, 3)), Vec3::splat(6.0)),
                (Vec3::splat(-500.0), Vec3::splat(1.0)),
                (Vec3::splat(500.0), Vec3::splat(1.0)),
            ];
            for &(at, size) in &shapes {
                v.ellipsoid(at, size);
            }
            for z in 0..v.dims.z {
                for y in 0..v.dims.y {
                    for x in 0..v.dims.x {
                        let q = IVec3::new(x, y, z);
                        let expected = shapes.iter().any(|&(at, size)| {
                            let d = ((v.centre(q) - at) * (Vec3::ONE / size)).length_squared();
                            d <= 0.95 || (d <= 1.05 && d <= 1.0 + (cell_noise(q / 2) - 0.5) * 0.1)
                        });
                        assert_eq!(v.contains(q), expected, "width={width} cell={q}");
                    }
                }
            }
            v.fill_columns(|p| 7.0 + p.x.sin());
            for z in 0..v.dims.z {
                for y in 0..v.dims.y {
                    for x in 0..v.dims.x {
                        let q = IVec3::new(x, y, z);
                        let p = v.centre(q);
                        assert_eq!(v.contains(q), p.y <= 7.0 + p.x.sin());
                    }
                }
            }
            v.fill_columns(|_| -100.0);
            assert!(v.rows.iter().all(|&r| r == 0));
        }
    }
    #[test]
    fn overlapping_billows_have_one_closed_boundary_without_internal_faces() {
        let mut v = Volume::new(Vec3::ZERO, 1.0, IVec3::splat(12));
        v.ellipsoid(Vec3::splat(5.5), Vec3::splat(3.5));
        v.ellipsoid(Vec3::new(7.0, 5.5, 5.5), Vec3::splat(3.0));
        let mut b = MeshBuf::default();
        v.append(&mut b, |_, _| [1.0; 4]);
        let mut faces = std::collections::BTreeSet::new();
        for (points, normals) in b.pos.chunks_exact(4).zip(b.nrm.chunks_exact(4)) {
            let n = Vec3::from(normals[0]);
            let at = points.iter().copied().map(Vec3::from).sum::<Vec3>() * 0.25;
            let inside = ((at - n * 0.25 - v.origin) / v.edge).floor().as_ivec3();
            let outside = ((at + n * 0.25 - v.origin) / v.edge).floor().as_ivec3();
            assert!(v.contains(inside));
            assert!(!v.contains(outside));
            assert!(faces.insert((
                at.to_array().map(f32::to_bits),
                n.to_array().map(f32::to_bits)
            )));
            assert!(
                (Vec3::from(points[1]) - Vec3::from(points[0]))
                    .cross(Vec3::from(points[2]) - Vec3::from(points[0]))
                    .dot(n)
                    > 0.0
            );
        }
        assert!(!faces.is_empty());
    }
}
