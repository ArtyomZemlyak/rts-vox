//! Exact presentation batches. Physics pieces, cells, IDs and material stay separate.
use super::*;
use sim_core::{burst::Fragment, damage::V3};
#[derive(Resource, Default)]
pub struct Enabled;
#[derive(Component)]
pub struct RestingBatch;
const PIECES: usize = 64;
const VERTICES: usize = 131072;
const BUILDS: usize = 8;
type Key = ([i32; 3], u16, usize);
#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    index: usize,
    id: u64,
    position: V3,
    basis: [V3; 3],
    mm: u16,
    shape: u64,
    side: u8,
    material: u16,
    paint: Option<[u8; 3]>,
}
impl Stamp {
    // Compaction changes source slots, not the already baked picture. Retain
    // unaffected GPU meshes and only refresh their source indices for wakeup.
    fn same_picture(&self, other: &Self) -> bool {
        self.id == other.id
            && self.position == other.position
            && self.basis == other.basis
            && self.mm == other.mm
            && self.shape == other.shape
            && self.side == other.side
            && self.material == other.material
            && self.paint == other.paint
    }
    fn of(index: usize, f: &Fragment) -> Self {
        Self {
            index,
            id: f.id,
            position: f.position,
            basis: f.basis,
            mm: f.mm,
            shape: f.shape,
            side: f.side,
            material: f.material,
            paint: f.paint,
        }
    }
}
struct Group {
    members: Vec<Stamp>,
    published: Option<(Entity, Handle<Mesh>)>,
}
#[derive(Default)]
pub struct Batches {
    input: Vec<Stamp>,
    scratch: Vec<Stamp>,
    groups: BTreeMap<Key, Group>,
    sources: BTreeMap<(render::model::FragmentMeshKey, u8), render::geom::MeshBuf>,
    pub flags: Vec<bool>,
    generation: Option<u64>,
}
impl Batches {
    fn source(&mut self, f: &Fragment) -> &render::geom::MeshBuf {
        let variant = rubble_variant(f);
        self.sources
            .entry(((f.material, f.shape, f.side, f.paint), variant))
            .or_insert_with(|| {
                let colour = piece_colour(f);
                if variant == 0 {
                    render::model::fragment_to_mesh(f.shape, f.side, colour)
                } else {
                    render::model::rubble_pile_to_mesh(variant - 1, colour)
                }
            })
    }
    fn retire(
        group: Group,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        exists: &impl Fn(Entity) -> bool,
    ) {
        if let Some((entity, mesh)) = group.published {
            if exists(entity) {
                commands.entity(entity).despawn();
            }
            meshes.remove(mesh.id());
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        enabled: bool,
        scene: &Scene,
        visual: &moving::Visual,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        cached: &mut BTreeMap<u16, Handle<StandardMaterial>>,
        exists: impl Fn(Entity) -> bool,
    ) {
        let fragments = &scene.sim.matter.debris.fragments;
        self.scratch.clear();
        let mut cursor = 0;
        if enabled && (fragments.len() >= 1024 || !self.groups.is_empty()) {
            for (index, f) in fragments.iter().enumerate() {
                if !f.resting || f.velocity != V3::ZERO || f.chunk.is_some() {
                    continue;
                }
                let stamp = Stamp::of(index, f);
                while self.input.get(cursor).is_some_and(|s| s.index < index) {
                    cursor += 1;
                }
                let was_stationary = self.input.get(cursor) == Some(&stamp);
                // Finish the landing interpolation before publishing static geometry.
                if !was_stationary
                    && !scene.paused
                    && visual
                        .fragments
                        .get(&f.id)
                        .is_some_and(|p| *p != pos([f.position.x, f.position.y, f.position.z]))
                {
                    continue;
                }
                self.scratch.push(stamp);
            }
        }
        let changed = self.generation != Some(scene.generation) || self.input != self.scratch;
        let mut flags_dirty = changed || self.flags.len() != fragments.len();
        if changed {
            let mut tiles: BTreeMap<([i32; 3], u16), Vec<Stamp>> = BTreeMap::new();
            for s in &self.scratch {
                let tile = [s.position.x, s.position.y, s.position.z]
                    .map(|v| v.to_int_trunc().div_euclid(32));
                tiles.entry((tile, s.material)).or_default().push(s.clone());
            }
            let mut old = std::mem::take(&mut self.groups);
            for ((tile, mat), members) in tiles {
                for (part, members) in members.chunks(PIECES).enumerate() {
                    if members.len() < 4 {
                        continue;
                    }
                    let key = (tile, mat, part);
                    let group = match old.remove(&key) {
                        Some(mut g)
                            if g.members.len() == members.len()
                                && g.members
                                    .iter()
                                    .zip(members)
                                    .all(|(a, b)| a.same_picture(b)) =>
                        {
                            g.members.clone_from_slice(members);
                            g
                        }
                        Some(g) => {
                            Self::retire(g, commands, meshes, &exists);
                            Group {
                                members: members.to_vec(),
                                published: None,
                            }
                        }
                        None => Group {
                            members: members.to_vec(),
                            published: None,
                        },
                    };
                    self.groups.insert(key, group);
                }
            }
            for (_, g) in old {
                Self::retire(g, commands, meshes, &exists);
            }
            std::mem::swap(&mut self.input, &mut self.scratch);
            self.generation = Some(scene.generation);
        }
        for g in self.groups.values_mut() {
            if g.published
                .as_ref()
                .is_some_and(|(entity, _)| !exists(*entity))
            {
                if let Some((_, mesh)) = g.published.take() {
                    meshes.remove(mesh.id());
                }
                flags_dirty = true;
            }
        }
        let pending: Vec<_> = self
            .groups
            .iter()
            .filter(|(_, g)| g.published.is_none())
            .map(|(&key, _)| key)
            .take(BUILDS)
            .collect();
        let mut used = 0;
        for key in pending {
            let members = self.groups[&key].members.clone();
            let cost: usize = members
                .iter()
                .map(|s| self.source(&fragments[s.index]).pos.len())
                .sum();
            if used > 0 && used + cost > VERTICES {
                break;
            }
            used += cost;
            let mut buf = render::geom::MeshBuf::default();
            buf.pos.reserve(cost);
            buf.nrm.reserve(cost);
            buf.col.reserve(cost);
            for s in &members {
                let f = &fragments[s.index];
                let transform = dense::transform(f);
                let source = self.source(f);
                let offset = buf.pos.len() as u32;
                buf.pos.extend(
                    source
                        .pos
                        .iter()
                        .map(|p| transform.transform_point(Vec3::from(*p)).to_array()),
                );
                buf.nrm.extend(
                    source
                        .nrm
                        .iter()
                        .map(|n| (transform.rotation * Vec3::from(*n)).to_array()),
                );
                buf.col.extend_from_slice(&source.col);
                buf.idx.extend(source.idx.iter().map(|i| i + offset));
            }
            let mesh = meshes.add(super::super::mesh(&buf));
            let material = piece_material(key.1, cached, materials);
            let entity = commands
                .spawn((
                    RestingBatch,
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material),
                    Transform::default(),
                ))
                .id();
            self.groups.get_mut(&key).unwrap().published = Some((entity, mesh));
            flags_dirty = true;
        }
        if flags_dirty {
            self.flags.clear();
            self.flags.resize(fragments.len(), false);
            for g in self.groups.values().filter(|g| g.published.is_some()) {
                for s in &g.members {
                    self.flags[s.index] = true;
                }
            }
        }
    }
}
