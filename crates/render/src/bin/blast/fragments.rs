//! Read-only view of actual matter; no lifetime, random velocity or gravity here.
use super::*;
use bevy::platform::collections::HashMap;
use std::collections::BTreeMap;
#[path = "fragments/batches.rs"]
pub(super) mod batches;
#[path = "fragments/dense.rs"]
mod dense;
#[path = "fragments/history.rs"]
pub(super) mod history;
#[cfg(test)]
#[path = "fragments/lifecycle_tests.rs"]
pub(super) mod lifecycle_tests;
#[path = "fragments/picking.rs"]
pub(super) mod picking;
#[path = "fragments/slots.rs"]
mod slots;
#[derive(Component)]
pub struct Piece(u64, u64, bool);
#[derive(Component)]
pub struct Surface;
fn retained_mesh(buf: &render::geom::MeshBuf) -> Mesh {
    let mut m = super::mesh(buf); m.asset_usage |= RenderAssetUsages::MAIN_WORLD; m
}
#[derive(Default)]
pub struct Cache {
    meshes: BTreeMap<(render::model::FragmentMeshKey, u8), Handle<Mesh>>,
    materials: BTreeMap<u16, Handle<StandardMaterial>>,
    metric_materials: BTreeMap<[u8; 3], Handle<StandardMaterial>>,
    chunks: BTreeMap<u64, Vec<Handle<Mesh>>>,
    slots: dense::Slots,
    batches: batches::Batches,
}
/// The cube colour of a range material, shared with falling bodies.
pub fn material_colour(material: u16) -> [f32; 4] {
    look::to_linear(match material {
        1 | 8 => [0.25, 0.20, 0.13],
        10 => [0.64, 0.48, 0.27],
        12 => [0.18, 0.38, 0.30],
        2 => [0.42, 0.40, 0.34],
        3 => [0.29, 0.32, 0.33],
        13 => [1.0, 0.33, 0.055],
        14 => [1.0, 0.45, 0.11],
        16 => [0.95, 0.70, 0.22],
        4 => [0.17, 0.22, 0.10],
        5 => [0.27, 0.20, 0.12],
        7 => [0.23, 0.25, 0.12],
        _ => [0.33, 0.33, 0.29],
    })
}
pub fn pos(v: [fx::Fx; 3]) -> Vec3 {
    Vec3::from(v.map(|a| a.to_f64_lossy() as f32))
}
fn piece_mesh(shape: u64, side: u8, colour: [f32; 4]) -> Mesh {
    retained_mesh(&render::model::fragment_to_mesh(shape, side, colour))
}
fn rubble_variant(f: &sim_core::burst::Fragment) -> u8 {
    if f.chunk.is_some()
        || f.paint.is_some()
        || f.side != 4
        || f.shape != u64::MAX
        || !matches!(f.material, 1 | 2 | 6 | 7 | 8)
    {
        return 0;
    }
    // Stable per-piece variety, with just sixteen shared meshes per material.
    let mut hash = f.id.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    1 + ((hash ^ (hash >> 27)) & 15) as u8
}
fn piece_colour(f: &sim_core::burst::Fragment) -> [f32; 4] {
    if matches!(f.material, 13 | 14 | 16) { return material_colour(f.material); }
    if let Some(p) = f.paint {
        let part = match p[2] {
            1 => voxel::Part::Core,
            2 => voxel::Part::Engine,
            3 => voxel::Part::Weapon,
            4 => voxel::Part::Sensor,
            _ => voxel::Part::Armour,
        };
        look::to_linear(look::finish_srgb(
            look::finish_of(p[1], part),
            [0.22, 0.32, 0.27],
        ))
    } else {
        material_colour(f.material)
    }
}
fn piece_material(
    mat: u16,
    cached: &mut BTreeMap<u16, Handle<StandardMaterial>>,
    materials: &mut Assets<StandardMaterial>,
) -> Handle<StandardMaterial> {
    cached
        .entry(mat)
        .or_insert_with(|| {
            materials.add(StandardMaterial {
                emissive: if matches!(mat, 13 | 14 | 16) { LinearRgba::rgb(2.0, 0.3, 0.025) } else { LinearRgba::BLACK },
                metallic: if mat == 3 { 0.65 } else { 0.0 },
                perceptual_roughness: if mat == 3 { 0.63 } else { 0.95 },
                ..default()
            })
        })
        .clone()
}
#[allow(clippy::too_many_arguments)]
pub fn draw(
    mut commands: Commands,
    scene: Res<Scene>,
    visual: Res<moving::Visual>,
    art: Res<WitnessArt>,
    mut cache: Local<Cache>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut old: Query<(Entity, &Piece, &mut Transform)>,
    batching: Option<Res<batches::Enabled>>,
    lab: Option<Res<laboratory::Laboratory>>,
    batch_entities: Query<Entity, With<batches::RestingBatch>>,
    mut metric_log: Local<Option<(u64,u8,u32)>>,
) {
    let mode = lab.as_ref().map_or(0, |l| l.metric_mode());
    let diagnostic = mode > 0;
    let fragments = &scene.sim.matter.debris.fragments;
    let alpha = if scene.realtime && !scene.paused {
        (scene.acc * sim_core::TICKS_PER_SECOND as f32).clamp(0.0, 1.0)
    } else {
        1.0
    };
    // Preserve the former two-large-section ceiling, while allowing several
    // cheap panels through one bounded frame instead of delaying their birth.
    const SECTION_WORK: usize = 1024;
    let mut section_work = 0;
    let Cache {
        batches,
        materials: cached,
        slots,
        ..
    } = &mut *cache;
    let mut update = || {
        batches.update(
            batching.is_some(),
            mode,
            &scene,
            &visual,
            &mut commands,
            &mut meshes,
            &mut materials,
            cached,
            |e| batch_entities.contains(e),
        );
    };
    let mut align = || slots.align(fragments);
    if let Some(pool) = bevy::tasks::ComputeTaskPool::try_get().filter(|_| fragments.len() >= 2048)
    {
        pool.scope(|scope| {
            scope.spawn(async move { align() });
            update();
        });
    } else {
        update();
        align();
    }
    {
        let Cache { slots, chunks, .. } = &mut *cache;
        for slot in slots.retired.drain(..) {
            if old.contains(slot.entity) {
                commands.entity(slot.entity).despawn();
            }
            chunks.remove(&slot.id);
        }
    }
    let Cache { slots, batches, .. } = &mut *cache;
    slots.prepare(
        fragments,
        &scene.sim.units,
        &visual.fragments,
        alpha,
        &batches.flags,
        |entity| old.as_readonly().contains(entity),
        bevy::tasks::ComputeTaskPool::try_get().map(|pool| &**pool),
    );
    // Workers touch only their render slots and read-only physics/queries.
    // Asset and entity changes keep the original physical fragment order.
    let Cache {
        slots,
        chunks,
        meshes: cached_meshes,
        materials: cached_materials,
        metric_materials,
        ..
    } = &mut *cache;
    for (entry, f) in slots.entries.iter_mut().zip(fragments) {
        let colour_key = diagnostic.then(|| laboratory::metric_quantize(laboratory::fragment_metric_colour(f, &scene.sim, mode)));
        let diagnostic_material = colour_key.map(|key| laboratory::fragment_metric_material(key, metric_materials, &mut materials));
        // A view change must not wait behind physical section mesh work. These
        // meshes belong to this section: swap only their colour arrays and keep
        // positions/indices, clipping source and the real paint for restoration.
        if let Some(slot) = entry.slot.as_mut().filter(|slot| old.contains(slot.entity)) {
            if let Some(appearance) = &mut slot.appearance {
                if slot.diagnostic != diagnostic {
                    if diagnostic {
                        for handle in chunks.get(&f.id).into_iter().flatten() {
                            if let Some(mut mesh) = meshes.get_mut(handle) {
                                if let Some(bevy::mesh::VertexAttributeValues::Float32x4(col)) = mesh.attribute_mut(Mesh::ATTRIBUTE_COLOR) {
                                    let original = std::mem::replace(col, vec![[1.0; 4]; col.len()]);
                                    appearance.colours.push((handle.clone(), original));
                                }
                            }
                        }
                    } else {
                        for (handle, original) in appearance.colours.drain(..) {
                            if let Some(mut mesh) = meshes.get_mut(&handle) { mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, original); }
                        }
                    }
                }
                if slot.diagnostic != diagnostic || slot.colour != colour_key {
                    for (&child, original) in slot.children.iter().zip(&appearance.materials) {
                        commands.entity(child).insert(MeshMaterial3d(diagnostic_material.as_ref().unwrap_or(original).clone()));
                    }
                    slot.diagnostic = diagnostic;
                    slot.colour = colour_key;
                }
            }
        }
        if let Some(slot) = &entry.slot {
            if slot.diagnostic != diagnostic && matches!(entry.prepared, dense::Prepared::Existing(_)) {
                entry.prepared = dense::Prepared::Replace(slot.key, dense::transform(f));
            }
        }
        let (piece_key, current) = match entry.prepared {
            dense::Prepared::Existing(next) => {
                let slot = entry.slot.as_mut().unwrap();
                if slot.colour != colour_key {
                    if let Some(mat) = &diagnostic_material {
                        if slot.children.is_empty() { commands.entity(slot.entity).insert(MeshMaterial3d(mat.clone())); }
                        for &child in &slot.children { commands.entity(child).insert(MeshMaterial3d(mat.clone())); }
                    }
                    slot.colour = colour_key;
                }
                if let Some(transform) = next {
                    if let Ok((_, p, mut tr)) = old.get_mut(slot.entity) {
                        debug_assert_eq!((p.0, p.1, p.2), (f.id, slot.key.0, slot.key.1));
                        tr.set_if_neq(transform);
                    }
                }
                continue;
            }
            dense::Prepared::Batched => {
                if let Some(slot) = entry.slot.take() {
                    if old.contains(slot.entity) {
                        commands.entity(slot.entity).despawn();
                    }
                    chunks.remove(&f.id);
                }
                continue;
            }
            dense::Prepared::Replace(key, current) => (key, current),
        };
        // A resting exact join rebases the centre while retaining every world
        // cell. Interpolating from the old owner's centre would move the whole
        // new shape sideways for one tick after its donors disappeared.
        let previous = (!f.resting)
            .then(|| visual.fragments.get(&f.id).copied())
            .flatten();
        let transform = slots::interpolate(current, previous, alpha);
        let mesh_work = f.chunk.as_ref().map_or(0, |c| {
            if c.body.is_some() {
                512
            } else {
                c.cells.len().clamp(64, 512)
            }
        });
        // A pending charred/rebuilt section keeps its current mesh and follows
        // physics until the bounded mesh builder can replace it atomically.
        if f.chunk.is_some() && section_work + mesh_work > SECTION_WORK {
            if let Some(slot) = &entry.slot {
                if let Ok((_, _, mut tr)) = old.get_mut(slot.entity) {
                    tr.set_if_neq(transform);
                }
            }
            continue;
        }
        if let Some(slot) = entry.slot.take() {
            let entity = slot.entity;
            if old.contains(entity) {
                commands.entity(entity).despawn();
            }
            chunks.remove(&f.id);
        }
        if let Some(chunk) = &f.chunk {
            section_work += mesh_work;
            let unit = &scene.sim.units[chunk.template as usize];
            let dead = !unit.status.alive;
            let kind = unit.kind;
            let template = &art.pristine[kind.index()];
            let tint = witness::body_tint(
                scene
                    .sim
                    .battle_mode()
                    .then(|| scene.sim.allied(chunk.template as usize)),
                chunk.template as usize,
            );
            let mut handles = Vec::new();
            let mut children = Vec::new();
            let mut appearance = slots::Appearance { colours: Vec::new(), materials: Vec::new() };
            let entity = commands
                .spawn((
                    Piece(f.id, chunk.hash(), dead),
                    transform,
                    Visibility::default(),
                ))
                .with_children(|parent| {
                    for (finish, mut buf) in
                        render::model::section_to_mesh_tinted(template, chunk, tint)
                    {
                        if dead { wreck::charred(&mut buf); }
                        let original = diagnostic.then(|| std::mem::replace(&mut buf.col, vec![[1.0; 4]; buf.pos.len()]));
                        let handle = meshes.add(retained_mesh(&buf));
                        if let Some(original) = original { appearance.colours.push((handle.clone(), original)); }
                        handles.push(handle.clone());
                        let original_material = if dead { art.burned_materials[finish].clone() } else { art.materials[finish].clone() };
                        appearance.materials.push(original_material.clone());
                        let child = parent.spawn((
                            Surface,
                            Mesh3d(handle),
                            MeshMaterial3d(diagnostic_material.as_ref().unwrap_or(&original_material).clone()),
                            Transform::default(),
                        )).id();
                        children.push(child);
                    }
                })
                .id();
            let mut slot = slots::Slot::new(entity, piece_key, f, current, transform);
            slot.diagnostic = diagnostic; slot.colour = colour_key; slot.children = children;
            slot.appearance = Some(Box::new(appearance));
            entry.slot = Some(slot);
            chunks.insert(f.id, handles);
            continue;
        }
        let colour = if diagnostic { [1.0; 4] } else { piece_colour(f) };
        let variant = rubble_variant(f);
        let key = ((f.material, f.shape, f.side, f.paint), variant | if diagnostic { 128 } else { 0 });
        let mesh = cached_meshes
            .entry(key)
            .or_insert_with(|| {
                meshes.add(if variant == 0 {
                    piece_mesh(f.shape, f.side, colour)
                } else {
                    retained_mesh(&render::model::rubble_pile_to_mesh(variant - 1, colour))
                })
            })
            .clone();
        let material = diagnostic_material.unwrap_or_else(|| piece_material(f.material, cached_materials, &mut materials));
        let entity = commands
            .spawn((
                Piece(f.id, 0, false),
                Surface,
                Mesh3d(mesh),
                MeshMaterial3d(material),
                transform,
            ))
            .id();
        let mut slot = slots::Slot::new(entity, piece_key, f, current, transform);
        slot.diagnostic = diagnostic; slot.colour = colour_key;
        entry.slot = Some(slot);
    }
    let key=(scene.generation,mode,scene.sim.tick/5);
    if scene.out.is_some() && mode>0 && *metric_log!=Some(key) {
        let individual=slots.entries.iter().filter(|entry|entry.slot.as_ref().is_some_and(|slot|slot.diagnostic)).count();
        let batched=cache.batches.flags.iter().filter(|&&v|v).count();
        println!("METRIC_COVERAGE tick={} mode={mode} paused={} total={} individual={individual} batched={batched} pending={} units={} terrain_global=true",scene.sim.tick,scene.paused,fragments.len(),fragments.len().saturating_sub(individual+batched),scene.sim.units.len());
        *metric_log=Some(key);
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn irregular_rubble_has_closed_faces_fixed_bounds_and_the_same_triangle_budget() {
        let mut variants = Vec::new();
        for variant in 0..16 {
            let buf = render::model::rubble_pile_to_mesh(variant, [0.3, 0.2, 0.1, 1.0]);
            assert_eq!(buf.idx.len(), 44 * 3);
            assert!(!variants.contains(&buf.pos), "duplicate rubble variant");
            variants.push(buf.pos.clone());
            let mut low = Vec3::splat(f32::INFINITY);
            let mut high = Vec3::splat(f32::NEG_INFINITY);
            let mut edges = BTreeMap::new();
            for p in &buf.pos {
                low = low.min(Vec3::from(*p));
                high = high.max(Vec3::from(*p));
            }
            assert!((low - Vec3::splat(-1.99)).abs().max_element() < 0.0001);
            assert!((high - Vec3::splat(1.99)).abs().max_element() < 0.0001);
            for triangle in buf.idx.chunks_exact(3) {
                let p = [triangle[0], triangle[1], triangle[2]]
                    .map(|i| Vec3::from(buf.pos[i as usize]));
                let cross = (p[1] - p[0]).cross(p[2] - p[0]);
                assert!(cross.is_finite() && cross.length_squared() > 0.00001);
                let normal = Vec3::from(buf.nrm[triangle[0] as usize]);
                assert!(cross.normalize().dot(normal) > 0.999);
                for [a, b] in [[0, 1], [1, 2], [2, 0]] {
                    let a = p[a].to_array().map(f32::to_bits);
                    let b = p[b].to_array().map(f32::to_bits);
                    let (key, direction) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                    let entry = edges.entry(key).or_insert((0, 0));
                    entry.0 += 1;
                    entry.1 += direction;
                }
            }
            assert!(edges
                .values()
                .all(|&(count, direction)| count == 2 && direction == 0));
        }
    }
    #[test]
    fn rubble_variants_are_bounded_stable_and_leave_unit_fragments_intact() {
        let mut f = sim_core::burst::Fragment {
            id: 0,
            position: phys_v3_zero(),
            velocity: phys_v3_zero(),
            material: 1,
            paint: None,
            mm: 100,
            basis: [phys_v3(1, 0, 0), phys_v3(0, 1, 0), phys_v3(0, 0, 1)],
            mass_g: 1,
            count: 64,
            shape: u64::MAX,
            side: 4,
            chunk: None,
            held: 0,
            resting: true,
        };
        let mut seen = std::collections::BTreeSet::new();
        for id in 0..4096 {
            f.id = id;
            let variant = rubble_variant(&f);
            assert!((1..=16).contains(&variant));
            seen.insert(variant);
            f.count = 4096;
            assert_eq!(rubble_variant(&f), variant);
            f.resting = false;
            assert_eq!(rubble_variant(&f), variant);
            f.resting = true;
        }
        assert_eq!(seen.len(), 16);
        f.paint = Some([1, 0, 0]);
        assert_eq!(rubble_variant(&f), 0);
        f.paint = None;
        f.material = 3;
        assert_eq!(rubble_variant(&f), 0);
        f.material = 1;
        f.shape = 15;
        assert_eq!(rubble_variant(&f), 0);
    }
    #[test]
    fn solid_piles_and_single_voxels_have_cubic_surfaces() {
        let mesh = piece_mesh(u64::MAX, 4, [1.0; 4]);
        assert_eq!(mesh.indices().unwrap().len() / 3, 12);
        // Sixty-four independent cubes used to submit 768 triangles.
        let buf = render::model::fragment_to_mesh(u64::MAX, 4, [1.0; 4]);
        for tri in buf.idx.chunks_exact(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| Vec3::from(buf.pos[i as usize]));
            let normal = Vec3::from(buf.nrm[tri[0] as usize]);
            assert!((b - a).cross(c - a).dot(normal) > 0.0);
            let centre = (a + b + c) / 3.0;
            assert!(centre.dot(normal) > 1.0, "an internal face survived");
        }
        let shard = piece_mesh(1, 1, [1.0; 4]);
        assert_eq!(shard.indices().unwrap().len() / 3, 12);
        let normals = shard
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .unwrap()
            .as_float3()
            .unwrap();
        assert!(normals
            .iter()
            .all(|n| n.iter().filter(|v| v.abs() > 0.001).count() == 1));
    }
    #[test]
    fn saturated_pile_mesh_matches_physical_bounds() {
        for (shape, side, count) in [(1, 2, 1), (u64::MAX, 4, 64), (u64::MAX, 4, 4096)] {
            for mm in [100, 200] {
                let f = sim_core::burst::Fragment {
                    id: 1,
                    position: phys_v3_zero(),
                    velocity: phys_v3_zero(),
                    material: 3,
                    paint: None,
                    mm,
                    basis: [phys_v3(1, 0, 0), phys_v3(0, 1, 0), phys_v3(0, 0, 1)],
                    mass_g: 1,
                    count,
                    shape,
                    side,
                    chunk: None,
                    held: 0,
                    resting: true,
                };
                let mesh = piece_mesh(shape, side, [1.0; 4]);
                let points = mesh
                    .attribute(Mesh::ATTRIBUTE_POSITION)
                    .unwrap()
                    .as_float3()
                    .unwrap();
                let mut lo = Vec3::splat(f32::INFINITY);
                let mut hi = Vec3::splat(f32::NEG_INFINITY);
                for p in points {
                    let p = Vec3::from(*p) * render::model::fragment_scale(&f);
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
                let (low, high) = f.bounds();
                let v = |p: [fx::Fx; 3]| Vec3::from(p.map(|v| v.to_f64_lossy() as f32));
                assert!((lo - v(low)).abs().max_element() < 0.0001, "count={count}");
                assert!((hi - v(high)).abs().max_element() < 0.0001, "count={count}");
            }
        }
    }
    fn phys_v3(x: i32, y: i32, z: i32) -> sim_core::damage::V3 {
        sim_core::damage::V3::new(
            fx::Fx::from_int(x),
            fx::Fx::from_int(y),
            fx::Fx::from_int(z),
        )
    }
    fn phys_v3_zero() -> sim_core::damage::V3 {
        phys_v3(0, 0, 0)
    }
}
