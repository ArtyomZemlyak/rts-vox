//! Read-only chunk surfaces of authoritative finite-volume water.
use super::*;
use sim_core::blast::water::{Water, MESH_SIDE};
use std::collections::BTreeMap;
type Chunk = [u32; 3];
#[derive(Component)]
pub struct Surface(Chunk);
#[derive(Default)]
pub struct Cache {
    generation: u64,
    cursor: Option<Chunk>,
    versions: BTreeMap<Chunk, u64>,
    material: Option<Handle<StandardMaterial>>,
}
fn quad(b: &mut MeshBuf, normal: [f32; 3], corners: [[f32; 3]; 4], colour: [f32; 4]) {
    coloured_quad(b, normal, corners, [colour; 4]);
}
fn coloured_quad(b: &mut MeshBuf, normal: [f32; 3], corners: [[f32; 3]; 4], colours: [[f32; 4]; 4]) {
    let first = b.pos.len() as u32;
    b.pos.extend(corners);
    b.nrm.extend([normal; 4]);
    b.col.extend(colours);
    b.idx
        .extend([first, first + 1, first + 2, first, first + 2, first + 3]);
}
// A continuous, world-space tint avoids a noisy checkerboard and matches at
// chunk boundaries. These are vertex colours on the existing surface mesh.
fn top_colour(x: f32, z: f32, height: f32, edge: f32) -> [f32; 4] {
    let glint = (((x * 0.095 + z * 0.061).sin() + (z * 0.11 - x * 0.043).sin()) * 0.5 + 1.0) * 0.5;
    let shallow = (1.0 - height).clamp(0.0, 1.0);
    let foam = edge.clamp(0.0, 1.0) * 0.28;
    let mix = |a: f32, b: f32| a + (b - a) * foam;
    [mix(0.085 + glint * 0.055 + shallow * 0.035, 0.50),
     mix(0.32 + glint * 0.08 + shallow * 0.055, 0.73),
     mix(0.43 + glint * 0.09 + shallow * 0.055, 0.78),
     0.74 + height * 0.10]
}
fn height(water: &Water, p: Chunk) -> f32 {
    if water.amount(p) == 0 {
        return 0.0;
    }
    if water.amount([p[0], p[1] + 1, p[2]]) > 0 {
        1.0
    } else {
        (water.amount(p) as f32 / water.tuning.units_per_voxel as f32).min(1.0)
    }
}
fn surface(water: &Water, chunk: Chunk) -> MeshBuf {
    let mut b = MeshBuf::default();
    let origin = chunk.map(|v| v * MESH_SIDE);
    for z in origin[2]..origin[2] + MESH_SIDE {
        for y in origin[1]..origin[1] + MESH_SIDE {
            for x in origin[0]..origin[0] + MESH_SIDE {
                let p = [x, y, z];
                if water.amount(p) == 0 {
                    continue;
                }
                let h = height(water, p);
                let (x, y, z) = (x as f32, y as f32, z as f32);
                let top = y + h;
                let colour = [0.08, 0.29, 0.39, 0.82];
                if water.amount([p[0], p[1] + 1, p[2]]) == 0 {
                    let edge = |dx: i32, dz: i32| {
                        let qx = p[0] as i32 + dx;
                        let qz = p[2] as i32 + dz;
                        if qx < 0 || qz < 0 { return 1.0; }
                        ((h - height(water, [qx as u32, p[1], qz as u32])) * 1.5).clamp(0.0, 1.0)
                    };
                    let (left, right, back, front) = (edge(-1, 0), edge(1, 0), edge(0, -1), edge(0, 1));
                    coloured_quad(
                        &mut b,
                        [0.0, 1.0, 0.0],
                        [
                            [x, top, z],
                            [x, top, z + 1.0],
                            [x + 1.0, top, z + 1.0],
                            [x + 1.0, top, z],
                        ],
                        [top_colour(x, z, h, left.max(back)),
                         top_colour(x, z + 1.0, h, left.max(front)),
                         top_colour(x + 1.0, z + 1.0, h, right.max(front)),
                         top_colour(x + 1.0, z, h, right.max(back))],
                    );
                }
                if p[1] > 0 && water.amount([p[0], p[1] - 1, p[2]]) == 0 {
                    quad(
                        &mut b,
                        [0.0, -1.0, 0.0],
                        [
                            [x, y, z],
                            [x + 1.0, y, z],
                            [x + 1.0, y, z + 1.0],
                            [x, y, z + 1.0],
                        ],
                        colour,
                    );
                }
                for (axis, delta) in [(0, -1), (0, 1), (2, -1), (2, 1)] {
                    let mut q = p;
                    q[axis] = (p[axis] as i32 + delta).max(0) as u32;
                    let neighbour = height(water, q);
                    if neighbour >= h {
                        continue;
                    }
                    let low = y + neighbour;
                    let mut normal = [0.0; 3];
                    normal[axis] = delta as f32;
                    let corners = if axis == 0 {
                        let xx = x + if delta > 0 { 1.0 } else { 0.0 };
                        [
                            [xx, low, z],
                            [xx, top, z],
                            [xx, top, z + 1.0],
                            [xx, low, z + 1.0],
                        ]
                    } else {
                        let zz = z + if delta > 0 { 1.0 } else { 0.0 };
                        [
                            [x, low, zz],
                            [x + 1.0, low, zz],
                            [x + 1.0, top, zz],
                            [x, top, zz],
                        ]
                    };
                    let upper = [0.13, 0.40, 0.51, 0.82];
                    let lower = [0.055, 0.23, 0.32, 0.86];
                    let colours = if axis == 0 {
                        [lower, upper, upper, lower]
                    } else {
                        [lower, lower, upper, upper]
                    };
                    coloured_quad(&mut b, normal, corners, colours);
                }
            }
        }
    }
    b
}
pub fn draw(
    mut commands: Commands,
    scene: Res<Scene>,
    mut cache: Local<Cache>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    old: Query<(Entity, &Surface, &Mesh3d)>,
) {
    let versions: BTreeMap<_, _> = scene.sim.water.chunks().collect();
    let reset = cache.generation != scene.generation;
    let mut entities = BTreeMap::new();
    for (e, s, m) in &old {
        if reset || !versions.contains_key(&s.0) {
            commands.entity(e).despawn();
            meshes.remove(m.0.id());
        } else {
            entities.insert(s.0, (e, m.0.clone()));
        }
    }
    if reset {
        cache.versions.clear();
        cache.generation = scene.generation;
    }
    cache.versions.retain(|p, _| versions.contains_key(p));
    if versions.is_empty() {
        return;
    }
    let material = cache
        .material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgba(0.90, 0.98, 1.0, 1.0),
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 0.18,
                reflectance: 0.65,
                cull_mode: None,
                ..default()
            })
        })
        .clone();
    // Frame budget, independent of the number of wet voxels. One entity per
    // surface chunk; dormant water does not remesh while the camera moves.
    let pending: Vec<_> = versions
        .iter()
        .filter(|(p, v)| cache.versions.get(*p) != Some(*v))
        .map(|(&p, &v)| (p, v))
        .collect();
    let start = pending.partition_point(|(p, _)| cache.cursor.is_some_and(|c| *p <= c));
    for i in 0..pending.len().min(4) {
        let (chunk, version) = pending[(start + i) % pending.len()];
        cache.cursor = Some(chunk);
        let b = surface(&scene.sim.water, chunk);
        if let Some((_, handle)) = entities.get(&chunk) {
            meshes
                .insert(handle.id(), mesh(&b))
                .expect("existing water mesh");
        } else if !b.pos.is_empty() {
            commands.spawn((
                Surface(chunk),
                Mesh3d(meshes.add(mesh(&b))),
                MeshMaterial3d(material.clone()),
                Transform::default(),
            ));
        }
        cache.versions.insert(chunk, version);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_water_has_no_surface_or_draw_chunks() {
        let w = Water::new();
        assert!(w.chunks().next().is_none());
        assert!(surface(&w, [0; 3]).pos.is_empty());
    }
    #[test]
    fn surface_tints_are_continuous_across_chunk_edges_without_extra_faces() {
        let mut w = Water::new();
        let s = MESH_SIDE;
        for x in s-2..=s+1 {
            for z in 2..=4 { assert!(w.edit_volume([x, 2, z], true)); }
        }
        let left = surface(&w, [0, 0, 0]);
        let right = surface(&w, [1, 0, 0]);
        let vertex = |b: &MeshBuf, p: [f32; 3]| {
            b.pos.iter().zip(&b.nrm).zip(&b.col)
                .find_map(|((pos, normal), color)| (*pos == p && *normal == [0.0, 1.0, 0.0]).then_some(*color))
                .expect("top vertex")
        };
        let p = [s as f32, 3.0, 3.0];
        assert_eq!(vertex(&left, p), vertex(&right, p));
        let alone = { let mut water=Water::new(); water.edit_volume([2,2,2],true); surface(&water,[0;3]) };
        assert_eq!(alone.pos.len(), 24, "one cell should still use only six faces");
        assert_eq!(alone.idx.len(), 36);
    }
    #[test]
    fn exposed_edge_is_subtle_and_does_not_change_opacity() {
        let middle = top_colour(20.0, 30.0, 1.0, 0.0);
        let edge = top_colour(20.0, 30.0, 1.0, 1.0);
        assert!(edge[0] > middle[0] && edge[1] > middle[1]);
        assert!(edge[1] < 0.6, "edge must not turn a whole tile white");
        assert_eq!(edge[3], middle[3]);
    }
}
