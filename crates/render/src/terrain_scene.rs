//! Bounded terrain residency for the interactive game. The world remains authoritative.
use crate::geom::{chunk_at, chunk_origin, class_bufs, quads_to_mesh_painted, MeshBuf, Painter};
use crate::{look, stream, ChunkGrid, V3};
use bevy::asset::RenderAssetUsages;
use bevy::mesh::Indices;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use meshing::layer::{Fill, MeshLayer};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Component)]
pub struct TerrainChunk(pub usize);

#[derive(Default)]
struct ChunkArt {
    meshes: [Option<Handle<Mesh>>; look::CLASSES],
    entities: [Option<Entity>; look::CLASSES],
}

pub struct TerrainScene {
    pub grid: ChunkGrid,
    pub settings: stream::Stream,
    mesh: MeshLayer,
    chunks: BTreeMap<usize, ChunkArt>,
    state: Vec<u8>, // 0 unseen, 1 air, 2 solid, 3 queued, 4 resident
    probe: VecDeque<u32>,
    build: VecDeque<usize>,
    redraw: BTreeSet<usize>,
    centre: Option<(i32, i32)>,
    bufs: [MeshBuf; look::CLASSES],
    painter: Painter,
}

impl TerrainScene {
    pub fn new(world: &voxel::World, settings: stream::Stream) -> Self {
        let mesh = MeshLayer::new(world, meshing::Opts::default());
        let (nx, ny, nz) = mesh.dims();
        let grid = ChunkGrid {
            nx,
            ny,
            nz,
            cs: meshing::CS as u32,
        };
        Self {
            grid,
            settings,
            mesh,
            chunks: BTreeMap::new(),
            state: vec![0; grid.len()],
            probe: VecDeque::new(),
            build: VecDeque::new(),
            redraw: BTreeSet::new(),
            centre: None,
            bufs: class_bufs(),
            painter: Painter::default(),
        }
    }
    pub fn resident(&self) -> usize {
        self.chunks.len()
    }
    pub fn pending(&self) -> usize {
        self.probe.len() + self.build.len() + self.redraw.len()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        world: &voxel::World,
        edits: &[voxel::Edit],
        target: V3,
        materials: &[Handle<StandardMaterial>; look::CLASSES],
        meshes: &mut Assets<Mesh>,
        commands: &mut Commands,
    ) {
        let grid = self.grid;
        self.mesh
            .apply_where(world, edits, |i| self.chunks.contains_key(&i));
        for &id in self.mesh.dirty() {
            let i = id as usize;
            if self.chunks.contains_key(&i) {
                self.redraw.insert(i);
            } else if self.state[i] == 1 || self.state[i] == 2 {
                self.state[i] = 0;
                self.probe.push_back(stream::column(
                    &grid,
                    id % grid.nx,
                    id / (grid.nx * grid.ny),
                ));
            }
        }
        let centre = (
            (target.x / grid.cs as f32).floor() as i32,
            (target.z / grid.cs as f32).floor() as i32,
        );
        if self.centre != Some(centre) {
            self.centre = Some(centre);
            self.probe.clear();
            for i in self.build.drain(..) {
                if self.state[i] == 3 {
                    self.state[i] = 0;
                }
            }
            let mut columns = Vec::new();
            stream::ring(
                &grid,
                target,
                self.settings.build_radius as f32,
                &mut columns,
            );
            self.probe.extend(columns);
        }
        let mut probes = self.settings.probe_per_frame as i32;
        while probes > 0 {
            let Some(column) = self.probe.pop_front() else {
                break;
            };
            if !stream::within(&grid, column, target, self.settings.build_radius as f32) {
                continue;
            }
            let (cx, cz) = stream::column_at(&grid, column);
            for cy in (0..grid.ny).rev() {
                let i = grid.index(cx, cy, cz);
                if self.state[i] == 2 {
                    break;
                }
                if self.state[i] != 0 {
                    continue;
                }
                probes -= 1;
                match self.mesh.probe(world, i) {
                    Fill::Air => self.state[i] = 1,
                    Fill::Rock => {
                        self.state[i] = 2;
                        break;
                    }
                    Fill::Mixed => {
                        self.state[i] = 3;
                        self.build.push_back(i);
                    }
                }
            }
        }
        let mut budget = self.settings.build_per_frame;
        while budget > 0 {
            let (i, changed) = if let Some(i) = self.redraw.pop_first() {
                (i, true)
            } else if let Some(i) = self.build.pop_front() {
                (i, false)
            } else {
                break;
            };
            let column = stream::column(&grid, i as u32 % grid.nx, i as u32 / (grid.nx * grid.ny));
            if !stream::within(&grid, column, target, self.settings.keep_radius as f32) {
                if !changed {
                    self.state[i] = 0;
                }
                continue;
            }
            budget -= 1;
            let quads = if changed {
                self.mesh.quads(i)
            } else {
                self.mesh.scratch_quads(world, i).to_vec()
            };
            for b in &mut self.bufs {
                b.clear();
            }
            let mut cursor = voxel::World::mat_cursor();
            quads_to_mesh_painted(
                &quads,
                [0.0; 3],
                chunk_at(i, grid.nx, grid.ny),
                |x, y, z| world.material_cached(&mut cursor, x, y, z),
                &mut self.painter,
                &mut self.bufs,
            );
            let slot = self.chunks.entry(i).or_default();
            let origin = chunk_origin(i, grid.nx, grid.ny);
            for (class, b) in self.bufs.iter().enumerate() {
                if b.is_empty() {
                    if let Some(e) = slot.entities[class].take() {
                        commands.entity(e).despawn();
                    }
                    slot.meshes[class] = None;
                    continue;
                }
                let mesh = mesh_from(b);
                if let Some(h) = slot.meshes[class].as_ref() {
                    let _ = meshes.insert(h.id(), mesh);
                } else {
                    let h = meshes.add(mesh);
                    slot.entities[class] = Some(
                        commands
                            .spawn((
                                Mesh3d(h.clone()),
                                MeshMaterial3d(materials[class].clone()),
                                Transform::from_xyz(origin[0], origin[1], origin[2]),
                                TerrainChunk(i),
                            ))
                            .id(),
                    );
                    slot.meshes[class] = Some(h);
                }
            }
            self.state[i] = 4;
        }
        let stale: Vec<usize> = self
            .chunks
            .keys()
            .copied()
            .filter(|&i| {
                let column =
                    stream::column(&grid, i as u32 % grid.nx, i as u32 / (grid.nx * grid.ny));
                !stream::within(&grid, column, target, self.settings.keep_radius as f32)
            })
            .take(self.settings.free_per_frame as usize)
            .collect();
        for i in stale {
            if let Some(slot) = self.chunks.remove(&i) {
                for e in slot.entities.into_iter().flatten() {
                    commands.entity(e).despawn();
                }
            }
            self.mesh.forget(i);
            self.redraw.remove(&i);
            self.state[i] = 0;
        }
    }
}

fn mesh_from(b: &MeshBuf) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, b.pos.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, b.nrm.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, b.col.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, b.uv.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, b.tan.clone())
    .with_inserted_indices(Indices::U32(b.idx.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Flat;
    impl voxel::Generator for Flat {
        fn material(&self, _x: u32, y: u32, _z: u32) -> u16 {
            if y < 24 {
                1
            } else {
                0
            }
        }
    }
    #[test]
    fn streaming_defers_work_and_rebuilds_edits_after_returning() {
        let mut world = voxel::World::new(384, 128, 384, 4096, Box::new(Flat));
        let settings = stream::Stream {
            build_radius: 80,
            keep_radius: 100,
            probe_per_frame: 20,
            build_per_frame: 2,
            free_per_frame: 2,
        };
        let mut scene = TerrainScene::new(&world, settings);
        let mut ecs = bevy::prelude::World::new();
        let mut meshes = Assets::<Mesh>::default();
        let materials = std::array::from_fn(|_| Handle::<StandardMaterial>::default());
        let frame = |s: &mut TerrainScene,
                     w: &voxel::World,
                     p: V3,
                     edits: &[voxel::Edit],
                     ecs: &mut bevy::prelude::World,
                     meshes: &mut Assets<Mesh>| {
            let mut queue = bevy::ecs::world::CommandQueue::default();
            {
                let mut commands = Commands::new(&mut queue, ecs);
                s.update(w, edits, p, &materials, meshes, &mut commands);
            }
            queue.apply(ecs);
        };
        let a = V3::new(40.0, 30.0, 40.0);
        frame(&mut scene, &world, a, &[], &mut ecs, &mut meshes);
        assert!(scene.resident() <= 2);
        assert!(scene.pending() > 0);
        for _ in 0..40 {
            frame(&mut scene, &world, a, &[], &mut ecs, &mut meshes);
        }
        assert_eq!(scene.pending(), 0);
        let first: Vec<_> = scene.chunks.keys().copied().collect();
        assert!(first.len() > 2);
        let b = V3::new(340.0, 30.0, 340.0);
        for _ in 0..40 {
            frame(&mut scene, &world, b, &[], &mut ecs, &mut meshes);
        }
        assert!(first.iter().all(|i| !scene.chunks.contains_key(i)));
        world.queue_dig(35, 23, 35);
        world.flush();
        frame(&mut scene, &world, b, world.edits(), &mut ecs, &mut meshes);
        assert_eq!(
            scene.mesh.live_chunks(),
            0,
            "off-screen edit allocated a resident mesher"
        );
        for _ in 0..40 {
            frame(&mut scene, &world, a, &[], &mut ecs, &mut meshes);
        }
        let i = scene.grid.index(0, 0, 0);
        assert!(scene.chunks.contains_key(&i));
        let expected = scene.mesh.scratch_quads(&world, i).to_vec();
        assert!(expected.len() > 6, "the returned chunk lost the crater");
        let uploaded: usize = scene.chunks[&i]
            .meshes
            .iter()
            .flatten()
            .map(|h| meshes.get(h).unwrap().count_vertices())
            .sum();
        assert!(uploaded > 24);
    }
}
