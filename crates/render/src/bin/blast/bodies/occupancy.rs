//! Exact face exposure for a falling body's immutable local voxel set.
use std::collections::HashSet;
use voxel::bodies::BodyVoxel;

// A thin bridge slab takes kilobytes. Distant cells must not allocate their
// entire empty bounding volume: above 512 KiB, retain sparse membership.
const MAX_DENSE_CELLS: usize = 4 * 1024 * 1024;
const NEIGHBOURS: [[i32; 3]; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];

enum Occupancy {
    Dense { size: [usize; 3], bits: Vec<u64> },
    Sparse(HashSet<[u16; 3]>),
}

impl Occupancy {
    fn new(voxels: &[BodyVoxel]) -> Self {
        let mut size = [0; 3];
        for v in voxels {
            for (axis, value) in [v.x, v.y, v.z].into_iter().enumerate() {
                size[axis] = size[axis].max(usize::from(value) + 1);
            }
        }
        let volume = size.iter().try_fold(1usize, |n, &side| n.checked_mul(side));
        if let Some(volume) = volume.filter(|&n| n <= MAX_DENSE_CELLS) {
            let mut bits = vec![0u64; volume.div_ceil(64)];
            for v in voxels {
                let index =
                    (usize::from(v.y) * size[2] + usize::from(v.z)) * size[0] + usize::from(v.x);
                bits[index / 64] |= 1 << (index % 64);
            }
            Self::Dense { size, bits }
        } else {
            Self::Sparse(voxels.iter().map(|v| [v.x, v.y, v.z]).collect())
        }
    }

    fn contains(&self, p: [i32; 3]) -> bool {
        match self {
            Self::Dense { size, bits } => {
                if p.iter().zip(size).any(|(&v, &n)| v < 0 || v as usize >= n) {
                    return false;
                }
                let [x, y, z] = p.map(|v| v as usize);
                let index = (y * size[2] + z) * size[0] + x;
                bits[index / 64] & (1 << (index % 64)) != 0
            }
            Self::Sparse(cells) => {
                if p.iter().any(|&v| !(0..=i32::from(u16::MAX)).contains(&v)) {
                    return false;
                }
                cells.contains(&p.map(|v| v as u16))
            }
        }
    }
}

/// Bits +X, -X, +Y, -Y, +Z, -Z, in the same order as mesh face emission.
/// The output preserves input voxel order and material boundaries stay hidden.
pub(super) fn visible_faces(voxels: &[BodyVoxel]) -> Vec<u8> {
    let occupied = Occupancy::new(voxels);
    voxels
        .iter()
        .map(|v| {
            let p = [i32::from(v.x), i32::from(v.y), i32::from(v.z)];
            let mut mask = 0;
            for (face, d) in NEIGHBOURS.iter().enumerate() {
                if !occupied.contains([p[0] + d[0], p[1] + d[1], p[2] + d[2]]) {
                    mask |= 1 << face;
                }
            }
            mask
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(voxels: &[BodyVoxel]) {
        let masks = visible_faces(voxels);
        let oracle: HashSet<_> = voxels
            .iter()
            .map(|v| [i32::from(v.x), i32::from(v.y), i32::from(v.z)])
            .collect();
        for (v, mask) in voxels.iter().zip(masks) {
            for (face, d) in NEIGHBOURS.iter().enumerate() {
                let neighbour = [
                    i32::from(v.x) + d[0],
                    i32::from(v.y) + d[1],
                    i32::from(v.z) + d[2],
                ];
                assert_eq!(
                    mask & (1 << face) != 0,
                    !oracle.contains(&neighbour),
                    "{v:?}, face={face}"
                );
            }
        }
    }

    #[test]
    fn compact_mixed_material_body_preserves_holes_and_word_boundaries() {
        let mut voxels = Vec::new();
        for y in 0..9 {
            for z in 0..12 {
                for x in 0..71 {
                    if (x * 7 + y * 13 + z * 31) % 11 > 2 {
                        voxels.push(BodyVoxel {
                            x,
                            y,
                            z,
                            mat: 2 + (x + z) % 4,
                        });
                    }
                }
            }
        }
        assert!(matches!(Occupancy::new(&voxels), Occupancy::Dense { .. }));
        check(&voxels);
        voxels.reverse();
        check(&voxels);
    }

    #[test]
    fn sparse_extents_are_bounded_and_do_not_wrap_at_u16_edges() {
        let voxels = [
            BodyVoxel {
                x: 0,
                y: 0,
                z: 0,
                mat: 3,
            },
            BodyVoxel {
                x: u16::MAX,
                y: 0,
                z: 0,
                mat: 3,
            },
            BodyVoxel {
                x: 1,
                y: u16::MAX,
                z: u16::MAX,
                mat: 5,
            },
        ];
        assert!(matches!(Occupancy::new(&voxels), Occupancy::Sparse(_)));
        check(&voxels);
        assert_eq!(visible_faces(&voxels), vec![63; 3]);
    }

    #[test]
    fn empty_and_solid_cube_have_exact_surface_area() {
        assert!(visible_faces(&[]).is_empty());
        let mut voxels = Vec::new();
        for x in 0..16 {
            for y in 0..16 {
                for z in 0..16 {
                    voxels.push(BodyVoxel { x, y, z, mat: 2 });
                }
            }
        }
        assert_eq!(
            visible_faces(&voxels)
                .iter()
                .map(|m| m.count_ones())
                .sum::<u32>(),
            6 * 16 * 16
        );
        check(&voxels);
    }
}
