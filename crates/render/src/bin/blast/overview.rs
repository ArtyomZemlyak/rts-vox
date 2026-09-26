//! A cheap distant surface for the kilometre map. The detailed voxel window
//! remains authoritative near the camera; this mesh replaces it at map scale.
use super::*;

const GRID: u32 = 40;

fn patch(x: u32, z: u32) -> f32 {
    let (gx, gz) = (x / 280, z / 280);
    let (u, v) = ((x % 280) as f32 / 280.0, (z % 280) as f32 / 280.0);
    let (u, v) = (u * u * (3.0 - 2.0 * u), v * v * (3.0 - 2.0 * v));
    let sample = |dx: u32, dz: u32| {
        let k = (gx + dx).wrapping_mul(1_664_525) ^ (gz + dz).wrapping_mul(1_013_904_223);
        (k.wrapping_mul(747_796_405) >> 16) as f32 / 65_535.0
    };
    let a = sample(0, 0) * (1.0 - u) + sample(1, 0) * u;
    let b = sample(0, 1) * (1.0 - u) + sample(1, 1) * u;
    a * (1.0 - v) + b * v
}

#[derive(Component)]
pub struct FarTerrain {
    size: [u32; 3],
}

fn surface(size: [u32; 3]) -> Mesh {
    let (nx, nz) = (size[0].div_ceil(GRID), size[2].div_ceil(GRID));
    let width = (nx + 1) as usize;
    let mut positions = Vec::with_capacity(width * (nz + 1) as usize);
    let mut normals = Vec::with_capacity(positions.capacity());
    let mut colours = Vec::with_capacity(positions.capacity());
    let mut indices = Vec::with_capacity((nx * nz * 6) as usize);
    for iz in 0..=nz {
        let z = (iz * GRID).min(size[2] - 1);
        for ix in 0..=nx {
            let x = (ix * GRID).min(size[0] - 1);
            let h = sim_core::blast::range_height(x, z).min(size[1] - 1) as f32;
            let xm = x.saturating_sub(GRID).max(1);
            let zm = z.saturating_sub(GRID).max(1);
            let xp = (x + GRID).min(size[0] - 1);
            let zp = (z + GRID).min(size[2] - 1);
            let dx = sim_core::blast::range_height(xp, z) as f32
                - sim_core::blast::range_height(xm, z) as f32;
            let dz = sim_core::blast::range_height(x, zp) as f32
                - sim_core::blast::range_height(x, zm) as f32;
            let n = Vec3::new(
                -dx / (xp - xm).max(1) as f32,
                1.0,
                -dz / (zp - zm).max(1) as f32,
            )
            .normalize();
            // Smooth macro variation keeps the terrain legible from a whole-
            // map view, without a grid of hard-edged coloured squares.
            let t = ((patch(x, z) - 0.37) * 2.6).clamp(0.0, 1.0);
            let grass = [0.08, 0.18, 0.07];
            let earth = [0.19, 0.15, 0.09];
            let rock = [0.18, 0.18, 0.15];
            let shade = (0.78 + 0.22 * n.dot(Vec3::new(-0.4, 0.8, -0.4))).clamp(0.55, 1.0);
            let c = if h > 37.0 {
                rock
            } else {
                std::array::from_fn(|i| grass[i] * (1.0 - t) + earth[i] * t)
            };
            positions.push([x as f32, h - 1.0, z as f32]);
            normals.push(n.to_array());
            colours.push([c[0] * shade, c[1] * shade, c[2] * shade, 1.0]);
        }
    }
    for iz in 0..nz {
        for ix in 0..nx {
            let a = iz * (nx + 1) + ix;
            let b = a + 1;
            let c = a + nx + 1;
            let d = c + 1;
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colours)
    .with_inserted_indices(Indices::U32(indices))
}

pub fn sync(
    mut commands: Commands,
    scene: Res<Scene>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    far: Query<(Entity, &FarTerrain)>,
) {
    let size = scene.sim.size();
    if far.iter().any(|(_, f)| f.size == size) {
        return;
    }
    for (entity, _) in &far {
        commands.entity(entity).despawn();
    }
    if size[0] <= TERRAIN_WINDOW[0] && size[2] <= TERRAIN_WINDOW[2] {
        return;
    }
    let mesh = meshes.add(surface(size));
    let material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        unlit: true,
        fog_enabled: false,
        perceptual_roughness: 1.0,
        cull_mode: None,
        ..default()
    });
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(material),
        FarTerrain { size },
        Visibility::Hidden,
    ));
}

pub fn visibility(
    stream: Res<streaming::Streaming>,
    mut far: Query<&mut Visibility, With<FarTerrain>>,
) {
    // Never draw both the coarse surface and the detailed raymarched volume.
    // They intersect on slopes, and their depth ordering changes with camera
    // movement. The streaming job keeps the old detailed view visible until
    // the next window is ready; it is not a reason to show the overview.
    let show = stream.overview;
    for mut visibility in &mut far {
        *visibility = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_mesh_covers_all_four_map_corners() {
        let size = [10_000, 192, 10_000];
        let mesh = surface(size);
        let positions = mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap();
        let bevy::mesh::VertexAttributeValues::Float32x3(positions) = positions else {
            panic!("positions")
        };
        assert_eq!(positions.first().unwrap()[0], 0.0);
        assert_eq!(positions.first().unwrap()[2], 0.0);
        assert_eq!(positions.last().unwrap()[0], 9999.0);
        assert_eq!(positions.last().unwrap()[2], 9999.0);
    }

    #[test]
    fn full_map_fits_at_each_orbit_and_pitch() {
        let size = [10_000, 192, 10_000];
        let aspect = 16.0 / 9.0;
        for yaw in [0.0, 0.8, 2.1, 4.3] {
            for pitch in [-0.5, 0.0, 0.5] {
                let offset = orbit_offset(Vec3::new(-155.0, 420.0, 320.0), yaw, pitch);
                let (aim, height) = whole_map_frame(size, offset, aspect);
                let rotation = Transform::from_translation(offset)
                    .looking_at(Vec3::ZERO, Vec3::Y)
                    .rotation;
                for x in [0.0, size[0] as f32] {
                    for y in [0.0, size[1] as f32] {
                        for z in [0.0, size[2] as f32] {
                            let relative = Vec3::new(x, y, z) - aim;
                            assert!(
                                (rotation * Vec3::X).dot(relative).abs() <= height * aspect / 2.0
                            );
                            assert!((rotation * Vec3::Y).dot(relative).abs() <= height / 2.0);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn prepared_sector_starts_in_detailed_mode() {
        let layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron"))
                .unwrap();
        let offset = orbit_offset(Vec3::new(-155.0, 420.0, 320.0), 0.0, 0.0);
        let (_, fit) = sector_frame(&layout.zones[0], offset, 16.0 / 9.0);
        let fitted_height = fit / 0.8;
        let limit = window_height_limit(offset, 16.0 / 9.0, layout.size);
        let height = fitted_height.min(limit);
        assert!(
            !streaming::uses_overview(layout.size, height, false),
            "prepared sector height {height}, local window limit {limit} unexpectedly uses the coarse map"
        );
        assert!(streaming::uses_overview(layout.size, limit * 1.2, false));
    }
}
