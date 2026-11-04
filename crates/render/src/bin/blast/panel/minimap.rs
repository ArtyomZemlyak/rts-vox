//! A map of actual terrain columns; clicking changes the existing camera pan.
use super::*;
use bevy::asset::RenderAssetUsages;
use bevy::math::Rot2;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui::FocusPolicy;
#[derive(Component, Default)]
pub struct MiniMap {
    generation: Option<u64>,
}
#[derive(Component)]
pub struct MiniRoot;
#[derive(Component)]
pub struct Marker(usize);
#[derive(Component)]
pub struct ViewEdge {
    index: usize,
    thickness: f32,
}
const SIDE: u32 = 96;
const MAP_PIXELS: f32 = 142.0;
pub fn build(commands: &mut Commands, camera: Entity, font: &TextFont) {
    commands
        .spawn((
            MiniRoot,
            PanelRoot,
            bevy::ui::RelativeCursorPosition::default(),
            Node {
                position_type: PositionType::Absolute,
                left: px(16),
                bottom: px(234),
                width: px(154),
                height: px(176),
                padding: UiRect::all(px(5)),
                row_gap: px(4),
                flex_direction: FlexDirection::Column,
                border: UiRect::all(px(1)),
                ..default()
            },
            BackgroundColor(theme::PANEL),
            BorderColor::all(theme::FRAME),
            UiTargetCamera(camera),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("ТАКТИЧЕСКАЯ КАРТА"),
                TextFont {
                    font_size: FontSize::Px(10.0),
                    ..font.clone()
                },
                TextColor(theme::MUTED),
            ));
            p.spawn((
                Button,
                ImageNode::default(),
                MiniMap::default(),
                bevy::ui::RelativeCursorPosition::default(),
                Node {
                    width: px(MAP_PIXELS),
                    height: px(MAP_PIXELS),
                    overflow: Overflow::clip(),
                    ..default()
                },
            ))
            .with_children(|p| {
                // Two strokes keep the view outline legible over both pale
                // roads and dark woods. They pass clicks through to the map.
                for index in 0..8 {
                    for (thickness, color) in [
                        (4.0, Color::srgba(0.015, 0.04, 0.05, 0.95)),
                        (2.0, Color::srgb(1.0, 0.87, 0.41)),
                    ] {
                        p.spawn((
                            ViewEdge { index, thickness },
                            FocusPolicy::Pass,
                            UiTransform::default(),
                            Node {
                                position_type: PositionType::Absolute,
                                display: Display::None,
                                height: px(thickness),
                                ..default()
                            },
                            BackgroundColor(color),
                        ));
                    }
                }
                for i in 0..=modding::range::MAX_RANGE_UNITS + 1 {
                    p.spawn((
                        Marker(i),
                        Node {
                            position_type: PositionType::Absolute,
                            width: px(5),
                            height: px(5),
                            border: UiRect::all(px(1)),
                            ..default()
                        },
                        BackgroundColor(Color::WHITE),
                        BorderColor::all(Color::BLACK),
                    ));
                }
            });
        });
}

/// Intersect the current orthographic frustum with the range's ground plane.
/// The four resulting corners preserve camera orbit and pitch on the map.
fn ground_footprint(
    transform: &Transform,
    projection: &Projection,
    viewport: Vec2,
    ground: f32,
) -> Option<[Vec2; 4]> {
    let Projection::Orthographic(ortho) = projection else {
        return None;
    };
    let bevy::camera::ScalingMode::FixedVertical { viewport_height } = ortho.scaling_mode else {
        return None;
    };
    if viewport.x <= 0.0 || viewport.y <= 0.0 {
        return None;
    }
    let height = viewport_height * ortho.scale;
    let width = height * viewport.x / viewport.y;
    let right = transform.rotation * Vec3::X;
    let up = transform.rotation * Vec3::Y;
    let forward = transform.rotation * Vec3::NEG_Z;
    if forward.y >= -0.01 {
        return None;
    }
    Some(std::array::from_fn(|i| {
        let (x, y) = [(-1.0, 1.0), (1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)][i];
        let origin = transform.translation + right * (x * width * 0.5) + up * (y * height * 0.5);
        let point = origin + forward * ((ground - origin.y) / forward.y);
        Vec2::new(point.x, point.z)
    }))
}

/// Clip a projected view against the unit square of the tactical map.
/// A four-corner view can yield up to eight edges after clipping.
fn clip_to_map(corners: [Vec2; 4]) -> Vec<Vec2> {
    let mut polygon = corners.to_vec();
    for (axis, limit, greater) in [
        (0, 0.0, true),
        (0, 1.0, false),
        (1, 0.0, true),
        (1, 1.0, false),
    ] {
        if polygon.is_empty() {
            break;
        }
        let mut next = Vec::with_capacity(8);
        let mut previous = *polygon.last().unwrap();
        let mut was_inside = if greater {
            previous[axis] >= limit
        } else {
            previous[axis] <= limit
        };
        for current in polygon.iter().copied() {
            let inside = if greater {
                current[axis] >= limit
            } else {
                current[axis] <= limit
            };
            if inside != was_inside {
                let t = (limit - previous[axis]) / (current[axis] - previous[axis]);
                next.push(previous.lerp(current, t));
            }
            if inside {
                next.push(current);
            }
            previous = current;
            was_inside = inside;
        }
        polygon = next;
    }
    polygon
        .into_iter()
        .map(|p| p.clamp(Vec2::ZERO, Vec2::ONE))
        .collect()
}

pub fn update_view(
    snapshot: Res<super::super::ui_snapshot::UiSnapshot>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &Transform, &Projection), With<GasCamera>>,
    mut edges: Query<(&ViewEdge, &mut Node, &mut UiTransform)>,
    mut previous: Local<Vec<Vec2>>,
) {
    let Some((camera, transform, projection)) = cameras.iter().next() else {
        return;
    };
    let viewport = camera
        .logical_viewport_size()
        .or_else(|| {
            windows
                .single()
                .ok()
                .map(|w| Vec2::new(w.width(), w.height()))
        })
        .unwrap_or(Vec2::new(1600.0, 900.0));
    let Some(view) = &snapshot.view else { return };
    let size = view.size;
    let polygon = ground_footprint(transform, projection, viewport, view.ground_level as f32)
        .map(|corners| {
            clip_to_map(corners.map(|p| Vec2::new(p.x / size[0] as f32, p.y / size[2] as f32)))
        })
        .unwrap_or_default();
    let pixels: Vec<Vec2> = polygon.into_iter().map(|p| p * MAP_PIXELS).collect();
    if pixels.len() == previous.len()
        && pixels
            .iter()
            .zip(previous.iter())
            .all(|(a, b)| a.distance_squared(*b) < 0.04)
    {
        return;
    }
    for (edge, mut node, mut transform) in &mut edges {
        if pixels.len() < 3 || edge.index >= pixels.len() {
            node.display = Display::None;
            continue;
        }
        let start = pixels[edge.index];
        let end = pixels[(edge.index + 1) % pixels.len()];
        let delta = end - start;
        let length = delta.length();
        let middle = (start + end) * 0.5;
        node.display = Display::Flex;
        node.left = px(middle.x - length * 0.5);
        node.top = px(middle.y - edge.thickness * 0.5);
        node.width = px(length);
        transform.rotation = Rot2::radians(delta.y.atan2(delta.x));
    }
    *previous = pixels;
}
pub fn update_terrain(
    scene: Res<Scene>,
    mut images: ResMut<Assets<Image>>,
    mut maps: Query<(&mut MiniMap, &mut ImageNode)>,
) {
    let size = scene.sim.size();
    for (mut map, mut image) in &mut maps {
        if map.generation != Some(scene.generation) {
            let mut pixels = Vec::with_capacity((SIDE * SIDE * 4) as usize);
            for z in 0..SIDE {
                for x in 0..SIDE {
                    let x = x * size[0] / SIDE;
                    let z = z * size[2] / SIDE;
                    let y = scene
                        .sim
                        .world
                        .column_ceiling(x, z)
                        .saturating_sub(1)
                        .min(size[1] - 1);
                    let m =
                        sim_core::blast::material(&scene.sim.world, [x as i32, y as i32, z as i32]);
                    let base: [u8; 3] = match m {
                        2 => [118, 125, 116],
                        3 => [82, 103, 112],
                        4 | 5 => [37, 67, 47],
                        7 => [118, 106, 76],
                        8 => [85, 105, 65],
                        _ => [65, 82, 54],
                    };
                    let shade = (y / 4).min(20) as u8;
                    pixels.extend(base.map(|v| v.saturating_add(shade)));
                    pixels.push(255);
                }
            }
            let texture = Image::new(
                Extent3d {
                    width: SIDE,
                    height: SIDE,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                pixels,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::default(),
            );
            if map.generation.is_some() && images.contains(&image.image) {
                *images.get_mut(&image.image).unwrap() = texture;
            } else {
                image.image = images.add(texture);
            }
            map.generation = Some(scene.generation);
        }
    }
}

pub fn input(
    mut scene: ResMut<Scene>,
    mut edit: ResMut<Workshop>,
    maps: Query<(&Interaction, &bevy::ui::RelativeCursorPosition), With<MiniMap>>,
) {
    let size = scene.sim.size();
    for (interaction, cursor) in &maps {
        if *interaction == Interaction::Pressed && cursor.cursor_over() {
            if let Some(uv) = cursor.normalized {
                let centre = edit.sector().map_or(
                    Vec2::new(scene.sim.gun.at[0] as f32, scene.sim.gun.at[2] as f32),
                    |z| {
                        Vec2::new(
                            (z.min[0] + z.max[0]) as f32 * 0.5,
                            (z.min[1] + z.max[1]) as f32 * 0.5,
                        )
                    },
                );
                edit.pan = destination(uv, size) - centre;
                edit.zoom = 1.0;
                scene.view = 1;
            }
        }
    }
}

pub fn refresh_markers(
    snapshot: Res<super::super::ui_snapshot::UiSnapshot>,
    edit: Res<Workshop>,
    panel: Res<Panel>,
    mut markers: Query<(&Marker, &mut Node, &mut BackgroundColor)>,
) {
    let Some(view) = &snapshot.view else { return };
    let size = view.size;
    for (Marker(i), mut node, mut color) in &mut markers {
        if *i == modding::range::MAX_RANGE_UNITS + 1 {
            let point = panel.mission.as_ref().and_then(MissionRun::waypoint);
            node.display = if point.is_some() {
                Display::Flex
            } else {
                Display::None
            };
            if let Some((p, _)) = point {
                node.width = px(7);
                node.height = px(7);
                node.left = percent(100.0 * p[0] as f32 / size[0] as f32);
                node.top = percent(100.0 * p[1] as f32 / size[2] as f32);
                *color = BackgroundColor(theme::ACCENT);
            }
            continue;
        }

        let unit = if *i == modding::range::MAX_RANGE_UNITS {
            (!view.battle).then_some(&view.gun)
        } else {
            view.units.get(*i)
        };
        node.display = if unit.is_some_and(|u| u.status.alive) {
            Display::Flex
        } else {
            Display::None
        };
        if let Some(u) = unit {
            let selected = edit.selected_units.contains(i)
                || (edit.selected_units.is_empty() && edit.controlled == Some(*i));
            let width = if u.kind == RangeUnitKind::Base {
                10.0
            } else {
                5.0
            } + if selected { 3.0 } else { 0.0 };
            node.width = px(width);
            node.height = px(width);
            node.left = percent(100.0 * u.position[0].to_f64_lossy() as f32 / size[0] as f32);
            node.top = percent(100.0 * u.position[2].to_f64_lossy() as f32 / size[2] as f32);
            color.0 = if selected {
                Color::srgb(0.99, 0.94, 0.56)
            } else if *i == modding::range::MAX_RANGE_UNITS || u.allied {
                Color::srgb(0.2, 0.85, 1.0)
            } else {
                Color::srgb(1.0, 0.32, 0.2)
            };
        }
    }
}

fn destination(cursor: Vec2, size: [u32; 3]) -> Vec2 {
    (cursor + Vec2::splat(0.5)).clamp(Vec2::ZERO, Vec2::ONE)
        * Vec2::new(size[0] as f32, size[2] as f32)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detached_markers_follow_publication_and_hide_removed_generation_slots() {
        use super::super::super::ui_snapshot::UiSnapshot;
        let layout =
            RangeLayout::from_ron(include_str!("../../../../../../data/range-cover.ron")).unwrap();
        let sim = BlastRange::with_layout(config(false, false), layout).unwrap();
        let view = sim_core::blast::read_model::View::new(&sim, 4);
        let pos = view.units[0].position;
        let size = view.size;
        drop(sim);
        let mut app = App::new();
        app.insert_resource(UiSnapshot {
            view: Some(view),
            ..default()
        })
        .insert_resource(Workshop::new(None, false))
        .insert_resource(Panel::new(false, false))
        .add_systems(Update, refresh_markers);
        let marker = app
            .world_mut()
            .spawn((Marker(0), Node::default(), BackgroundColor::default()))
            .id();
        let waypoint = app
            .world_mut()
            .spawn((
                Marker(modding::range::MAX_RANGE_UNITS + 1),
                Node::default(),
                BackgroundColor::default(),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Node>(waypoint).unwrap().display,
            Display::None
        );
        app.world_mut().resource_mut::<Panel>().mission = modding::range_skirmish::stock(6)
            .unwrap()
            .mission
            .map(MissionRun::new);
        app.update();
        assert_eq!(
            app.world().get::<Node>(waypoint).unwrap().display,
            Display::Flex
        );
        assert_eq!(
            app.world().get::<Node>(waypoint).unwrap().left,
            percent(100.0 * 4700.0 / size[0] as f32)
        );
        app.world_mut().resource_mut::<Panel>().mission = None;
        app.update();
        assert_eq!(
            app.world().get::<Node>(waypoint).unwrap().display,
            Display::None
        );
        let node = app.world().get::<Node>(marker).unwrap();
        assert_eq!(node.display, Display::Flex);
        assert_eq!(
            node.left,
            percent(100.0 * pos[0].to_f64_lossy() as f32 / size[0] as f32)
        );
        {
            let mut snapshot = app.world_mut().resource_mut::<UiSnapshot>();
            let view = snapshot.view.as_mut().unwrap();
            view.generation += 1;
            view.units.clear();
        }
        app.update();
        assert_eq!(
            app.world().get::<Node>(marker).unwrap().display,
            Display::None
        );
    }

    #[test]
    fn minimap_cursor_maps_centre_and_corners_to_world() {
        let size = [10000, 192, 8000];
        assert_eq!(destination(Vec2::ZERO, size), Vec2::new(5000.0, 4000.0));
        assert_eq!(destination(Vec2::splat(-0.5), size), Vec2::ZERO);
        assert_eq!(
            destination(Vec2::splat(0.5), size),
            Vec2::new(10000.0, 8000.0)
        );
        assert_eq!(
            destination(Vec2::new(2.0, -1.0), size),
            Vec2::new(10000.0, 0.0)
        );
    }

    #[test]
    fn camera_footprint_follows_orbit_and_clips_to_the_map() {
        let projection = Projection::Orthographic(OrthographicProjection {
            scaling_mode: bevy::camera::ScalingMode::FixedVertical {
                viewport_height: 100.0,
            },
            ..OrthographicProjection::default_3d()
        });
        let transform = Transform::from_xyz(500.0, 400.0, 800.0)
            .looking_at(Vec3::new(500.0, 16.0, 500.0), Vec3::Y);
        let corners =
            ground_footprint(&transform, &projection, Vec2::new(800.0, 600.0), 16.0).unwrap();
        let centre = corners.iter().copied().sum::<Vec2>() / 4.0;
        assert!((centre - Vec2::new(500.0, 500.0)).length() < 0.01);
        let rotated = Transform::from_xyz(800.0, 400.0, 500.0)
            .looking_at(Vec3::new(500.0, 16.0, 500.0), Vec3::Y);
        let rotated_corners =
            ground_footprint(&rotated, &projection, Vec2::new(800.0, 600.0), 16.0).unwrap();
        assert!((corners[0] - rotated_corners[0]).length() > 10.0);

        let clipped = clip_to_map([
            Vec2::new(-1.0, -1.0),
            Vec2::new(2.0, -1.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(-1.0, 2.0),
        ]);
        assert_eq!(clipped.len(), 4);
        assert!(clipped
            .iter()
            .all(|p| (0.0..=1.0).contains(&p.x) && (0.0..=1.0).contains(&p.y)));
    }
}
