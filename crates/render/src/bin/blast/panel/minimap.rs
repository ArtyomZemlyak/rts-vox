//! A map of actual terrain columns; clicking changes the existing camera pan.
use super::*;
use bevy::asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
#[derive(Component, Default)]
pub struct MiniMap {
    generation: Option<u64>,
}
#[derive(Component)]
pub struct Marker(usize);
const SIDE: u32 = 96;
pub fn build(commands: &mut Commands, camera: Entity, font: &TextFont) {
    commands
        .spawn((
            PanelRoot,
            bevy::ui::RelativeCursorPosition::default(),
            Node {
                position_type: PositionType::Absolute,
                left: px(16),
                bottom: px(212),
                width: px(154),
                height: px(176),
                padding: UiRect::all(px(5)),
                row_gap: px(4),
                flex_direction: FlexDirection::Column,
                border: UiRect::all(px(1)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.025, 0.044, 0.055, 0.96)),
            BorderColor::all(Color::srgb(0.19, 0.31, 0.35)),
            UiTargetCamera(camera),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("ТАКТИЧЕСКАЯ КАРТА"),
                TextFont {
                    font_size: FontSize::Px(10.0),
                    ..font.clone()
                },
                TextColor(Color::srgb(0.55, 0.78, 0.80)),
            ));
            p.spawn((
                Button,
                ImageNode::default(),
                MiniMap::default(),
                bevy::ui::RelativeCursorPosition::default(),
                Node {
                    width: px(142),
                    height: px(142),
                    ..default()
                },
            ))
            .with_children(|p| {
                for i in 0..=modding::range::MAX_RANGE_UNITS {
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
pub fn update(
    mut scene: ResMut<Scene>,
    mut edit: ResMut<Workshop>,
    mut images: ResMut<Assets<Image>>,
    mut maps: Query<(
        &mut MiniMap,
        &mut ImageNode,
        &Interaction,
        &bevy::ui::RelativeCursorPosition,
    )>,
    mut markers: Query<(&Marker, &mut Node, &mut BackgroundColor)>,
) {
    let size = scene.sim.size();
    for (mut map, mut image, interaction, cursor) in &mut maps {
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
    for (Marker(i), mut node, mut color) in &mut markers {
        let unit = if *i == modding::range::MAX_RANGE_UNITS {
            (!scene.sim.battle_mode()).then_some(&scene.sim.gun)
        } else {
            scene.sim.units.get(*i)
        };
        node.display = if unit.is_some_and(|u| u.status.alive) {
            Display::Flex
        } else {
            Display::None
        };
        if let Some(u) = unit {
            node.left = percent(100.0 * u.motion.pos[0].to_f64_lossy() as f32 / size[0] as f32);
            node.top = percent(100.0 * u.motion.pos[2].to_f64_lossy() as f32 / size[2] as f32);
            color.0 = if *i == modding::range::MAX_RANGE_UNITS || scene.sim.allied(*i) {
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
}
