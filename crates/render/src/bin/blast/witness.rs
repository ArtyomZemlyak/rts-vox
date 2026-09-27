//! Damage meshing runs off the frame thread; a bounded job per changed unit.
use super::*;
use bevy::tasks::{block_on, poll_once, AsyncComputeTaskPool, Task};
use sim_core::blast::RangeUnit;
use std::collections::BTreeMap;

pub(super) type Revision = (usize, u64, RangeUnitKind, bool);
pub(super) struct Pending {
    revision: Revision,
    task: Task<Vec<Built>>,
}
struct Built {
    mesh: Mesh,
    finish: usize,
    local: Transform,
    segment: Option<usize>,
    turret: bool,
}
struct Snapshot {
    dead: bool,
    body: voxel::unit::UnitBody,
    dents: BTreeMap<[u8; 3], u32>,
    segment_dents: BTreeMap<(u8, [u8; 3]), u32>,
    revision: u64,
    seg_gone: Vec<bool>,
    allied: Option<bool>,
}
impl From<&RangeUnit> for Snapshot {
    fn from(u: &RangeUnit) -> Self {
        Self {
            dead: !u.status.alive,
            body: u.body.clone(),
            allied: None,
            dents: u.dents.clone(),
            segment_dents: u.segment_dents().clone(),
            revision: u.revision,
            seg_gone: u.seg_gone.clone(),
        }
    }
}
fn build(
    unit: &Snapshot,
    id: usize,
    pristine: &voxel::unit::UnitBody,
    skins: &SkinBindings,
    config: &ExplosionConfig,
) -> Vec<Built> {
    let mut result = Vec::new();
    let centre = Vec3::from(render::model::root_centre(&unit.body));
    let lost = |v: Vec3| {
        let q = v.floor().as_ivec3();
        q.cmpge(IVec3::ZERO).all()
            && q.cmplt(IVec3::new(
                pristine.vol.nx as i32,
                pristine.vol.ny as i32,
                pristine.vol.nz as i32,
            ))
            .all()
            && pristine.vol.get(q.x as usize, q.y as usize, q.z as usize)
            && !unit.body.vol.get(q.x as usize, q.y as usize, q.z as usize)
    };
    let mut draw = |body: &voxel::unit::UnitBody,
                    pivot: [f32; 3],
                    pos: Vec3,
                    rot: Quat,
                    segment: Option<usize>,
                    rows: (usize, usize),
                    turret: bool| {
        let mut bufs = std::array::from_fn(|_| MeshBuf::default());
        render::model::body_to_mesh(
            body,
            if let Some(allied) = unit.allied {
                if allied {
                    [0.14, 0.30, 0.40]
                } else {
                    [0.44, 0.17, 0.12]
                }
            } else if id == 0 {
                [0.38, 0.34, 0.25]
            } else {
                [0.22, 0.32, 0.27]
            },
            1.0,
            rows,
            0.35,
            pivot,
            &mut bufs,
        );
        for (k, buf) in bufs.iter_mut().enumerate() {
            if buf.idx.is_empty() {
                continue;
            }
            weather(buf, k);
            if unit.revision > 0 {
                for j in 0..buf.pos.len() {
                    let normal = Vec3::from(buf.nrm[j]);
                    let local = Vec3::from(buf.pos[j]) + Vec3::from(pivot) - normal * 0.08;
                    let cell = local.floor().as_ivec3();
                    let local_cell = cell.cmpge(IVec3::ZERO).all().then_some([
                        cell.x as u8,
                        cell.y as u8,
                        cell.z as u8,
                    ]);
                    let owner = if let Some(seg) = segment {
                        local_cell.and_then(|cell| skins.owner(pristine, seg, cell))
                    } else {
                        local_cell
                    };
                    let mut severity = 0.0_f32;
                    if let Some(owner) = owner {
                        let work = unit.dents.get(&owner).copied().unwrap_or(0);
                        if work > 0 {
                            let cost = sim_core::blast::unit_work(
                                &unit.body,
                                owner.map(usize::from),
                                config,
                            );
                            severity = (work as f32 / cost as f32).clamp(0.0, 1.0);
                        }
                    }
                    if let (Some(seg), Some(cell)) = (segment, local_cell) {
                        let work = unit
                            .segment_dents
                            .get(&(seg as u8, cell))
                            .copied()
                            .unwrap_or(0);
                        if work > 0 {
                            let cost = sim_core::blast::unit_work(
                                &unit.body.segments[seg].body,
                                cell.map(usize::from),
                                config,
                            );
                            severity = severity.max((work as f32 / cost as f32).clamp(0.0, 1.0));
                        }
                    }
                    if severity > 0.0 {
                        let mark = owner.or(local_cell).unwrap();
                        let p = Vec3::from(mark.map(|v| v as f32)) + Vec3::splat(0.5);
                        let grain = (p.dot(Vec3::new(17.1, 7.7, 23.3)).sin() * 437.1)
                            .fract()
                            .abs();
                        let steel = Vec3::new(0.14, 0.155, 0.165) * (0.65 + grain * 0.7);
                        let burned = Vec3::new(0.026, 0.03, 0.035);
                        let col = steel.lerp(burned, severity * 0.85);
                        buf.col[j] = [col.x, col.y, col.z, 1.0];
                        // Sub-voxel deformation is presentation of the
                        // persisted work, never a second collision body.
                        buf.pos[j] = (Vec3::from(buf.pos[j]) - normal * severity * 0.13).to_array();
                    }
                    let Some(owner) = owner else {
                        continue;
                    };
                    let p = Vec3::from(owner.map(|v| v as f32)) + Vec3::splat(0.5);
                    let cut = lost(p + normal * 0.55);
                    let scorched = cut
                        || [
                            Vec3::X,
                            Vec3::NEG_X,
                            Vec3::Y,
                            Vec3::NEG_Y,
                            Vec3::Z,
                            Vec3::NEG_Z,
                        ]
                        .iter()
                        .any(|d| lost(p + *d * 1.4));
                    if scorched {
                        let grain = (p.dot(Vec3::new(17.1, 7.7, 23.3)).sin() * 437.1)
                            .fract()
                            .abs();
                        let c = if cut {
                            0.038 + grain * 0.065
                        } else {
                            0.025 + grain * 0.022
                        };
                        buf.col[j] = [c, c * 0.95, c * 0.87, 1.0];
                    }
                }
            }
            if unit.dead {
                wreck::charred(buf);
            }
            result.push(Built {
                mesh: mesh(buf),
                finish: k,
                local: Transform::from_translation(pos).with_rotation(rot),
                segment,
                turret,
            });
        }
    };
    draw(
        &unit.body,
        centre.to_array(),
        Vec3::ZERO,
        Quat::IDENTITY,
        None,
        (0, unit.body.turret_y as usize),
        false,
    );
    draw(
        &unit.body,
        centre.to_array(),
        Vec3::ZERO,
        Quat::IDENTITY,
        None,
        (unit.body.turret_y as usize, usize::MAX),
        true,
    );
    for (seg, (segment, frame)) in unit
        .body
        .segments
        .iter()
        .zip(render::model::segment_frames(&unit.body, 0.01))
        .enumerate()
    {
        if unit.seg_gone[seg] {
            continue;
        }
        let clipped = skins.clipped(pristine, &unit.body, seg);
        draw(
            &clipped,
            render::model::pivot_centre(&segment.attach),
            frame.pos,
            frame.rot,
            Some(seg),
            (0, usize::MAX),
            frame.turret,
        );
    }

    result
}

pub(super) fn update(
    mut commands: Commands,
    scene: Res<Scene>,
    mut art: ResMut<WitnessArt>,
    debris_art: Res<Art>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut old: Query<(Entity, &Witness, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    if art.generation != scene.generation {
        art.pending.clear();
        art.seen.clear();
        art.emitted.clear();
        art.generation = scene.generation;
    }
    for (entity, w, mut material) in &mut old {
        if let Some(unit) = scene.sim.units.get(w.0) {
            if !unit.status.alive {
                // Darken immediately while the bounded mesh worker bakes soot.
                if let Some(finish) = art.materials.iter().position(|m| *m == material.0) {
                    material.0 = art.burned_materials[finish].clone();
                }
            }
        } else {
            commands.entity(entity).despawn();
        }
    }
    let count = scene.sim.units.len();
    art.seen.resize(count, None);
    art.emitted.resize(count, 0);
    art.pending.resize_with(count, || None);
    // Limit concurrent snapshots and CPU work, regardless of the unit count.
    let mut active = art.pending.iter().filter(|p| p.is_some()).count();
    for (id, unit) in scene.sim.units.iter().enumerate() {
        let removed = unit.removed.len();
        let from = art.emitted[id].min(removed);
        if let Some(at) = scene
            .sim
            .impact
            .filter(|_| scene.sim.config.material_energy_j == 0)
        {
            let hit = Vec3::from(at.map(|v| v as f32));
            for p in unit.removed[from..].iter().step_by(97).take(32) {
                let pos = Vec3::from(unit.world_pos(p.map(usize::from)).map(|v| v as f32));
                let seed = f32::from(p[0] ^ p[1] ^ p[2]) / 63.0;
                commands.spawn((
                    Mesh3d(debris_art.cube.clone()),
                    MeshMaterial3d(debris_art.rock.clone()),
                    Transform::from_translation(pos).with_scale(Vec3::new(1.0 + seed, 0.35, 1.5)),
                    Ejecta {
                        velocity: (pos - hit).normalize_or_zero() * (20.0 + seed * 24.0)
                            + Vec3::Y * 24.0,
                        life: 4.0,
                    },
                ));
            }
        }

        art.emitted[id] = removed;
        if let Some(pending) = art.pending[id].as_mut() {
            if let Some(parts) = block_on(poll_once(&mut pending.task)) {
                let built_revision = pending.revision;
                art.pending[id] = None;
                active -= 1;
                // A reset invalidates all jobs above. Damage may have advanced
                // while this job ran: show this completed revision, then build
                // the latest one; never restart an in-flight job every tick.
                for (entity, w, _) in &old {
                    if w.0 == id {
                        commands.entity(entity).despawn();
                    }
                }
                let base = Vec3::from(unit.at.map(|v| v as f32));
                for b in parts {
                    let mut transform = b.local;
                    transform.translation += base;
                    commands.spawn((
                        Mesh3d(meshes.add(b.mesh)),
                        MeshMaterial3d(if unit.status.alive {
                            art.materials[b.finish].clone()
                        } else {
                            art.burned_materials[b.finish].clone()
                        }),
                        transform,
                        Witness(id, b.local, b.segment, b.turret),
                    ));
                }
                art.seen[id] = Some(built_revision);
            }
        }
    }
    let first = art.next_mesh;
    for offset in 0..count {
        if active >= 2 {
            break;
        }
        let id = (first + offset) % count;
        let unit = &scene.sim.units[id];
        let revision = (
            unit.removed.len(),
            unit.revision,
            unit.kind,
            !unit.status.alive,
        );
        if art.seen[id] == Some(revision) || art.pending[id].is_some() {
            continue;
        }
        art.next_mesh = (id + 1) % count;
        let mut snapshot = Snapshot::from(unit);
        if scene.sim.battle_mode() {
            snapshot.allied = Some(scene.sim.allied(id));
        }
        let kind = match unit.kind {
            RangeUnitKind::Tank => 0,
            RangeUnitKind::Strider => 1,
        };
        let pristine = art.pristine.clone();
        let skins = art.skins.clone();
        let config = scene.sim.config.clone();
        art.pending[id] = Some(Pending {
            revision,
            task: AsyncComputeTaskPool::get()
                .spawn(async move { build(&snapshot, id, &pristine[kind], &skins[kind], &config) }),
        });
        active += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn destruction_chars_every_root_and_segment_without_erasing_geometry() {
        for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider] {
            let unit = RangeUnit::of_kind(kind, [300, 16, 300]);
            let pristine = unit.body.clone();
            let skins = SkinBindings::new(&pristine);
            let clean = Snapshot::from(&unit);
            let mut dead = Snapshot::from(&unit);
            dead.dead = true; // No dent revision: alive state itself invalidates the finish.
            let config = scaled_config(true, false, 1000);
            let clean = build(&clean, 0, &pristine, &skins, &config);
            let dead = build(&dead, 0, &pristine, &skins, &config);
            assert_eq!(clean.len(), dead.len());
            assert!(dead.iter().any(|p| p.segment.is_some()));
            for (a, b) in clean.iter().zip(&dead) {
                assert_eq!(
                    a.mesh.attribute(Mesh::ATTRIBUTE_POSITION),
                    b.mesh.attribute(Mesh::ATTRIBUTE_POSITION)
                );
                assert_eq!(
                    a.mesh.attribute(Mesh::ATTRIBUTE_UV_0),
                    b.mesh.attribute(Mesh::ATTRIBUTE_UV_0)
                );
                let bevy::mesh::VertexAttributeValues::Float32x4(colours) =
                    b.mesh.attribute(Mesh::ATTRIBUTE_COLOR).unwrap()
                else {
                    panic!("colours");
                };
                assert!(colours
                    .iter()
                    .all(|c| c[0] <= 0.14 && c[1] <= c[0] && c[2] <= c[1] && c[3] == 1.0));
                assert_ne!(
                    a.mesh.attribute(Mesh::ATTRIBUTE_COLOR),
                    b.mesh.attribute(Mesh::ATTRIBUTE_COLOR)
                );
            }
        }
    }
    #[test]
    fn damaged_tank_plate_changes_its_own_visible_mesh() {
        let pristine = voxel::unit::tank();
        let skins = SkinBindings::new(&pristine);
        let config = scaled_config(true, false, 1000);
        let unit = RangeUnit::of_kind(RangeUnitKind::Tank, [300, 16, 300]);
        let clean = Snapshot::from(&unit);
        let mut marked = Snapshot::from(&unit);
        let cell = [0, 0, 0];
        assert!(marked.body.segments[0].body.vol.get(0, 0, 0));
        let cost = sim_core::blast::unit_work(&marked.body.segments[0].body, [0, 0, 0], &config);
        marked.segment_dents.insert((0, cell), cost / 2);
        marked.revision = 1;
        let clean = build(&clean, 0, &pristine, &skins, &config);
        let marked = build(&marked, 0, &pristine, &skins, &config);
        let colors = |meshes: &Vec<Built>| {
            meshes
                .iter()
                .filter(|mesh| mesh.segment == Some(0))
                .map(|mesh| mesh.mesh.attribute(Mesh::ATTRIBUTE_COLOR).unwrap().clone())
                .collect::<Vec<_>>()
        };
        assert_ne!(colors(&clean), colors(&marked));
    }
    #[test]
    fn worker_owns_a_snapshot_while_the_authoritative_body_keeps_changing() {
        let layout =
            RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap();
        let mut sim =
            range_sim(scaled_config(true, false, 1000), Some(&layout), false, None).unwrap();
        for _ in 0..250 {
            sim.step();
        }
        let unit = &mut sim.units[0];
        assert!(!unit.removed.is_empty());
        let pristine = Arc::new(voxel::unit::tank());
        let skins = Arc::new(SkinBindings::new(&pristine));
        let captured = Snapshot::from(&*unit);
        let reference = build(&captured, 0, &pristine, &skins, &sim.config);
        assert!(!reference.is_empty());
        unit.body.vol.cols.fill(0);
        unit.seg_gone.fill(true);
        unit.dents.clear();
        let config = sim.config.clone();
        let pool = bevy::tasks::TaskPoolBuilder::new().num_threads(1).build();
        let work = pool.spawn(async move { build(&captured, 0, &pristine, &skins, &config) });
        let result = block_on(work);
        assert_eq!(result.len(), reference.len());
        for (actual, expected) in result.iter().zip(&reference) {
            assert_eq!(actual.finish, expected.finish);
            assert_eq!(actual.local, expected.local);
            assert_eq!(actual.segment, expected.segment);
            for attribute in [
                Mesh::ATTRIBUTE_POSITION,
                Mesh::ATTRIBUTE_NORMAL,
                Mesh::ATTRIBUTE_COLOR,
                Mesh::ATTRIBUTE_UV_0,
                Mesh::ATTRIBUTE_TANGENT,
            ] {
                assert_eq!(
                    actual.mesh.attribute(attribute),
                    expected.mesh.attribute(attribute)
                );
            }
        }
    }
}
