//! Bounded presentation of consolidated loose matter, within its collision box.
//! Four angular shards share the former solid pile's 44-triangle budget.
use super::{tri, MeshBuf, Vec3};

pub fn rubble_pile_to_mesh(variant: u8, colour: [f32; 4]) -> MeshBuf {
    let mut seed = u32::from(variant % 16)
        .wrapping_add(1)
        .wrapping_mul(0x9e37_79b9);
    let mut noise = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed & 0xffff) as f32 / 65535.0
    };
    // The .01 margin is the same as fragment_to_mesh's closed 4-cell box.
    const EDGE: f32 = 1.99;
    let split_x = -0.3 + noise() * 0.6;
    let split_z = -0.3 + noise() * 0.6;
    let boxes = [
        (
            [-EDGE, -EDGE, -EDGE],
            [split_x + 0.7, -0.2 + noise() * 0.6, split_z + 0.6],
        ),
        (
            [split_x + 0.25, -EDGE, -EDGE + 0.2 + noise() * 0.4],
            [EDGE, 0.2 + noise() * 0.6, EDGE],
        ),
        (
            [-EDGE, -EDGE, split_z + 0.15],
            [split_x + 0.5, 0.5 + noise() * 0.6, EDGE],
        ),
        (
            [-0.9 + noise() * 0.3, -0.7, -0.8 + noise() * 0.3],
            [0.7 + noise() * 0.3, EDGE, 0.9 + noise() * 0.3],
        ),
    ];
    let mut out = MeshBuf::default();
    for (piece, (lo, hi)) in boxes.into_iter().enumerate() {
        let wedge = piece == 3;
        let mut vertices = if wedge {
            vec![
                [lo[0], lo[1], lo[2]],
                [hi[0], lo[1], lo[2]],
                [lo[0], hi[1], lo[2]],
                [lo[0], lo[1], hi[2]],
                [hi[0], lo[1], hi[2]],
                [lo[0], hi[1], hi[2]],
            ]
        } else {
            let mut points: Vec<_> = (0..8)
                .map(|i| {
                    [
                        if i & 1 == 0 { lo[0] } else { hi[0] },
                        if i & 2 == 0 { lo[1] } else { hi[1] },
                        if i & 4 == 0 { lo[2] } else { hi[2] },
                    ]
                })
                .collect();
            // Fractured top planes; the bottom remains seated on its support.
            for i in [2, 3, 6, 7] {
                points[i][1] -= (hi[1] - lo[1]) * (0.08 + noise() * 0.3);
            }
            points
        };
        for p in &mut vertices {
            if variant & 4 != 0 {
                p[0] = -p[0];
            }
            for _ in 0..variant % 4 {
                *p = [-p[2], p[1], p[0]];
            }
        }
        let centre = vertices.iter().copied().map(Vec3::from).sum::<Vec3>() / vertices.len() as f32;
        let faces: &[[usize; 3]] = if wedge {
            &[
                [0, 1, 2],
                [3, 5, 4],
                [0, 4, 1],
                [0, 3, 4],
                [0, 2, 5],
                [0, 5, 3],
                [1, 4, 5],
                [1, 5, 2],
            ]
        } else {
            &[
                [0, 1, 3],
                [0, 3, 2],
                [4, 6, 7],
                [4, 7, 5],
                [0, 4, 5],
                [0, 5, 1],
                [2, 3, 7],
                [2, 7, 6],
                [0, 2, 6],
                [0, 6, 4],
                [1, 5, 7],
                [1, 7, 3],
            ]
        };
        let shade = 0.82 + noise() * 0.25;
        let tint = [
            colour[0] * shade,
            colour[1] * shade,
            colour[2] * shade,
            colour[3],
        ];
        for face in faces {
            let mut p = face.map(|i| vertices[i]);
            let [a, b, c] = p.map(Vec3::from);
            let mut normal = (b - a).cross(c - a).normalize();
            if normal.dot((a + b + c) / 3.0 - centre) < 0.0 {
                p.swap(1, 2);
                normal = -normal;
            }
            tri(p, normal.to_array(), tint, &mut out);
        }
    }
    debug_assert_eq!(out.idx.len(), 44 * 3);
    out
}
