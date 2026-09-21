//! Cached translucent template meshes. Only transform, visibility and material
//! change under the mouse; neither the world nor the unit mesh is rebuilt.
use super::*;

#[derive(Component)]
pub enum Ghost {
    Unit(RangeUnitKind),
    Block(usize),
}
#[derive(Resource)]
pub struct Colours([Handle<StandardMaterial>; 2]);

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
    for (kind, body) in [
        (RangeUnitKind::Tank, voxel::unit::tank()),
        (RangeUnitKind::Strider, voxel::unit::strider()),
    ] {
        commands.spawn((
            Mesh3d(meshes.add(mesh(&template(&body)))),
            MeshMaterial3d(colours.0[0].clone()),
            Transform::default(),
            Visibility::Hidden,
            RenderLayers::layer(1),
            Ghost::Unit(kind),
        ));
    }
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
    commands.insert_resource(colours);
}

pub fn update(
    edit: Res<Workshop>,
    colours: Option<Res<Colours>>,
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
        let Some(p) = preview else {
            if *visible != Visibility::Hidden {
                *visible = Visibility::Hidden;
            }
            continue;
        };
        let pose = match (ghost, p.selected) {
            (Ghost::Unit(kind), Selection::Unit(i)) if *kind == p.next.units[i].kind => Some(
                Transform::from_translation(Vec3::from(p.next.units[i].at.map(|v| v as f32))),
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
