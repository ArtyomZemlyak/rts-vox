//! Cached translucent template meshes. Only transform, visibility and material
//! change under the mouse; neither the world nor the unit mesh is rebuilt.
use super::*;

#[derive(Component)]
pub enum Ghost {
    Unit(RangeUnitKind),
    UnitCopy(RangeUnitKind, usize),
    Block(usize),
    Shield(usize),
}
#[derive(Resource)]
pub struct Colours([Handle<StandardMaterial>; 2]);
#[derive(Resource, Default)]
pub struct Templates(
    pub Vec<(RangeUnitKind, std::sync::Arc<MeshBuf>)>,
    Vec<(RangeUnitKind, Vec3, Vec3)>,
);
impl Templates {
    pub fn bounds(&self, kind: RangeUnitKind) -> (Vec3, Vec3) {
        self.1
            .iter()
            .find(|(k, _, _)| *k == kind)
            .map_or((Vec3::ZERO, Vec3::ZERO), |(_, lo, hi)| (*lo, *hi))
    }
}

fn template(body: &voxel::UnitBody) -> MeshBuf {
    let mut out = MeshBuf::default();
    let mut add = |b: &voxel::UnitBody, centre, pos: Vec3, rot: Quat| {
        let mut parts = std::array::from_fn(|_| MeshBuf::default());
        render::model::body_to_mesh(
            b,
            [1.0; 3],
            b.mm as f32 / voxel::unit::WORLD_MM as f32,
            (0, usize::MAX),
            0.0,
            centre,
            &mut parts,
        );
        for part in parts {
            let start = out.pos.len() as u32;
            out.idx.extend(part.idx.iter().map(|i| i + start));
            out.pos.extend(
                part.pos
                    .iter()
                    .map(|p| (pos + rot * Vec3::from(*p)).to_array()),
            );
            out.nrm
                .extend(part.nrm.iter().map(|n| (rot * Vec3::from(*n)).to_array()));
        }
    };
    add(
        body,
        render::model::root_centre(body),
        Vec3::ZERO,
        Quat::IDENTITY,
    );
    let frames = render::model::segment_frames(body, 1.0 / voxel::unit::WORLD_MM as f32);
    for (s, f) in body.segments.iter().zip(frames) {
        add(
            &s.body,
            render::model::pivot_centre(&s.attach),
            f.pos,
            f.rot,
        );
    }
    out.col = vec![[1.0; 4]; out.pos.len()];
    out
}

pub fn setup(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let colours = Colours(
        [
            Color::srgba(0.16, 1.0, 0.32, 0.32),
            Color::srgba(1.0, 0.12, 0.08, 0.38),
        ]
        .map(|base_color| {
            materials.add(StandardMaterial {
                base_color,
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                ..default()
            })
        }),
    );
    let mut templates = Templates::default();
    for kind in RangeUnitKind::ALL {
        let body = sim_core::blast::RangeUnit::of_kind(kind, [0; 3]).body;
        let geometry = std::sync::Arc::new(template(&body));
        let handle = meshes.add(mesh(&geometry));
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for p in &geometry.pos {
            lo = lo.min(Vec3::from(*p));
            hi = hi.max(Vec3::from(*p));
        }
        templates.1.push((kind, lo, hi));
        templates.0.push((kind, geometry));
        for slot in 0..4 {
            commands.spawn((
                Mesh3d(handle.clone()),
                MeshMaterial3d(colours.0[0].clone()),
                Transform::default(),
                Visibility::Hidden,
                RenderLayers::layer(1),
                if slot == 0 {
                    Ghost::Unit(kind)
                } else {
                    Ghost::UnitCopy(kind, slot)
                },
            ));
        }
    }
    commands.insert_resource(templates);
    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    for i in 0..modding::range::MAX_RANGE_BLOCKS {
        commands.spawn((
            Mesh3d(cube.clone()),
            MeshMaterial3d(colours.0[0].clone()),
            Transform::default(),
            Visibility::Hidden,
            RenderLayers::layer(1),
            Ghost::Block(i),
        ));
    }
    let sphere = meshes.add(Sphere::new(1.0));
    for i in 0..4 {
        commands.spawn((
            Mesh3d(sphere.clone()),
            MeshMaterial3d(colours.0[0].clone()),
            Transform::default(),
            Visibility::Hidden,
            RenderLayers::layer(1),
            Ghost::Shield(i),
        ));
    }
    commands.insert_resource(colours);
}

pub fn update(
    edit: Res<Workshop>,
    laboratory: Option<Res<laboratory::Laboratory>>,
    colours: Option<Res<Colours>>,
    catalog: Option<Res<construction::Catalog>>,
    mut ghosts: Query<(
        &Ghost,
        &mut Transform,
        &mut Visibility,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
) {
    let Some(colours) = colours else { return };
    let preview = edit
        .preview
        .as_ref()
        .filter(|_| edit.editing && edit.hover.is_some());
    let blocks = preview
        .and_then(|p| match p.selected {
            Selection::Block(i) => Some(if edit.tool == Tool::Select {
                group(edit.layout.as_ref().unwrap(), i)
            } else {
                vec![i]
            }),
            _ => None,
        })
        .unwrap_or_default();
    for (ghost, mut transform, mut visible, mut material) in &mut ghosts {
        if let Some(lab) = laboratory.as_ref().filter(|l| l.active()) {
            if let Some((pose, invalid)) = lab.ghost_pose(ghost) {
                if *transform != pose {
                    *transform = pose;
                }
                let colour = &colours.0[usize::from(invalid)];
                if material.0 != *colour {
                    material.0 = colour.clone();
                }
                if *visible != Visibility::Visible {
                    *visible = Visibility::Visible;
                }
            } else if *visible != Visibility::Hidden {
                *visible = Visibility::Hidden;
            }
            continue;
        }
        if catalog.as_ref().is_some_and(|c| c.placing()) {
            let pose = match ghost { Ghost::Unit(kind) => catalog.as_ref().and_then(|c| c.ghost(*kind)), _ => None };
            if let Some((pose, invalid)) = pose {
                *transform = pose;
                material.0 = colours.0[usize::from(invalid)].clone();
                *visible = Visibility::Visible;
            } else { *visible = Visibility::Hidden; }
            continue;
        }
        let Some(p) = preview else {
            if *visible != Visibility::Hidden {
                *visible = Visibility::Hidden;
            }
            continue;
        };
        let pose = match (ghost, p.selected) {
            (Ghost::Unit(kind), Selection::Unit(i)) if *kind == p.next.units[i].kind => Some(
                Transform::from_translation(Vec3::from(p.next.units[i].at.map(|v| v as f32)))
                    .with_rotation(Quat::from_rotation_y(
                        -(p.next.unit_yaw(i) as f32) * std::f32::consts::FRAC_PI_2,
                    )),
            ),
            (Ghost::Block(slot), Selection::Block(_)) => blocks.get(*slot).map(|i| {
                let b = &p.next.blocks[*i];
                let lo = Vec3::from(b.min.map(|v| v as f32));
                let hi = Vec3::from(b.max.map(|v| v as f32));
                Transform::from_translation((lo + hi) * 0.5).with_scale(hi - lo)
            }),
            _ => None,
        };
        if let Some(pose) = pose {
            if *transform != pose {
                *transform = pose;
            }
            let colour = &colours.0[usize::from(p.issue.is_some())];
            if &material.0 != colour {
                *material = MeshMaterial3d(colour.clone());
            }
            if *visible != Visibility::Visible {
                *visible = Visibility::Visible;
            }
        } else if *visible != Visibility::Hidden {
            *visible = Visibility::Hidden;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ghost_follows_checked_colour_and_hides_outside_placement() {
        let layout =
            RangeLayout::from_ron(include_str!("../../../../../../data/range-cover.ron")).unwrap();
        let mut edit = Workshop::new(Some(layout), true);
        edit.tool = Tool::Strider;
        edit.hover = Some([448, 16, 384]);
        edit.preview([448, 16, 384], 60);
        let mut materials = Assets::<StandardMaterial>::default();
        let green = materials.add(StandardMaterial::default());
        let red = materials.add(StandardMaterial::default());
        let mut app = App::new();
        app.insert_resource(edit)
            .insert_resource(Colours([green.clone(), red.clone()]))
            .add_systems(Update, update);
        let entity = app
            .world_mut()
            .spawn((
                Ghost::Unit(RangeUnitKind::Strider),
                Transform::default(),
                Visibility::Hidden,
                MeshMaterial3d(green.clone()),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(entity),
            Some(&Visibility::Visible)
        );
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            Vec3::new(448.0, 16.0, 384.0)
        );
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(entity)
                .unwrap()
                .0,
            green
        );
        app.world_mut()
            .resource_mut::<Workshop>()
            .preview([400, 16, 336], 60);
        app.update();
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(entity)
                .unwrap()
                .0,
            red
        );
        app.world_mut().resource_mut::<Workshop>().hover = None;
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(entity),
            Some(&Visibility::Hidden)
        );
    }
    #[test]
    fn template_preview_keeps_root_and_articulated_geometry() {
        for body in [voxel::unit::tank(), voxel::unit::strider()] {
            let mut root_body = body.clone();
            root_body.segments.clear();
            let root = template(&root_body);
            let full = template(&body);
            assert!(full.idx.len() > root.idx.len());
            assert_eq!(full.pos.len(), full.nrm.len());
            assert_eq!(full.pos.len(), full.col.len());
            assert!(full.idx.iter().all(|i| (*i as usize) < full.pos.len()));
            assert!(full.pos.iter().flatten().all(|v| v.is_finite()));
        }
    }
}
