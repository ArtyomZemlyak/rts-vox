//! Lightweight native scrollbars for the editor docks. No scene or SIM work.
use super::*;
use bevy::ui::RelativeCursorPosition;

#[derive(Component)]
pub struct Area(pub u8);
#[derive(Component)]
pub struct Track(Entity);
#[derive(Component)]
pub struct Thumb(Entity);

pub fn area(
    p: &mut ChildSpawnerCommands,
    id: u8,
    children: impl FnOnce(&mut ChildSpawnerCommands),
) {
    p.spawn(Node {
        flex_grow: 1.0,
        min_height: px(0),
        column_gap: px(5),
        ..default()
    })
    .with_children(|p| {
        let viewport = p
            .spawn((
                Area(id),
                Node {
                    flex_grow: 1.0,
                    min_width: px(0),
                    min_height: px(0),
                    overflow: Overflow::scroll_y(),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(5),
                    ..default()
                },
                panel::ScrollPanel,
                ScrollPosition::default(),
                RelativeCursorPosition::default(),
            ))
            .with_children(children)
            .id();
        p.spawn((
            Track(viewport),
            RelativeCursorPosition::default(),
            Node {
                width: px(12),
                flex_shrink: 0.0,
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.10, 0.16, 0.22, 0.9)),
        ))
        .with_children(|p| {
            p.spawn((
                Thumb(viewport),
                Node {
                    position_type: PositionType::Absolute,
                    left: px(2),
                    right: px(2),
                    border_radius: BorderRadius::all(px(4)),
                    ..default()
                },
                BackgroundColor(theme::MUTED),
            ));
        });
    });
}

/// (maximum scroll, thumb height, thumb top), all in logical UI pixels.
fn geometry(view: f32, content: f32, track: f32, position: f32) -> (f32, f32, f32) {
    let max = (content - view).max(0.0);
    let thumb = if content > 0.0 {
        (track * view / content).clamp(24.0_f32.min(track), track)
    } else {
        track
    };
    let top = if max > 0.0 {
        position.clamp(0.0, max) / max * (track - thumb)
    } else {
        0.0
    };
    (max, thumb, top)
}

pub fn update(
    lab: Res<Laboratory>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    mut areas: Query<(&Area, &ComputedNode, &mut ScrollPosition)>,
    mut tracks: Query<(
        &Track,
        &ComputedNode,
        &RelativeCursorPosition,
        &mut Visibility,
    )>,
    mut thumbs: Query<(&Thumb, &mut Node, &mut BackgroundColor)>,
    mut drag: Local<Option<(Entity, f32)>>,
    mut context: Local<Option<(u8, Option<usize>, usize)>>,
) {
    let input = !windows.single().is_ok_and(|w| !w.focused) && !lab.ui.help && !lab.browser;
    if !lab.active() || !input {
        *drag = None;
    }
    if !lab.active() {
        return;
    }
    let current = (
        lab.ui.tab,
        (matches!(lab.ui.tab, 0 | 1))
            .then_some(lab.constructor.selected)
            .flatten(),
        match lab.ui.tab {
            2 | 10 => lab.constructor.workspace.selected,
            1 => lab.constructor.material(&lab.definition, lab.recipe) as usize,
            _ => 0,
        },
    );
    let changed_context = *context != Some(current);
    *context = Some(current);
    if !mouse.pressed(MouseButton::Left) {
        *drag = None;
    }
    for (Track(viewport), track, cursor, mut visibility) in &mut tracks {
        let Ok((area, node, mut pos)) = areas.get_mut(*viewport) else {
            continue;
        };
        if area.0 == 1 && changed_context && pos.y != 0.0 {
            pos.y = 0.0;
        }
        let factor = node.inverse_scale_factor();
        let height = (track.size().y * track.inverse_scale_factor()).max(0.0);
        let (max, thumb, top) = geometry(
            node.size().y * factor,
            node.content_size().y * factor,
            height,
            pos.y,
        );
        let show = if max > 0.5 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != show {
            *visibility = show;
        }
        let y = cursor.normalized.map(|p| p.y * height);
        if max > 0.5 && mouse.just_pressed(MouseButton::Left) && cursor.cursor_over() && input {
            if let Some(y) = y {
                let grab = if (top..=top + thumb).contains(&y) {
                    y - top
                } else {
                    thumb / 2.0
                };
                *drag = Some((*viewport, grab));
            }
        }
        if let Some((target, grab)) = *drag {
            if target == *viewport {
                if let Some(y) = y {
                    let value = if height > thumb {
                        ((y - grab) / (height - thumb)).clamp(0.0, 1.0) * max
                    } else {
                        0.0
                    };
                    if pos.y != value {
                        pos.y = value;
                    }
                }
            }
        }
        let value = pos.y.clamp(0.0, max);
        if pos.y != value {
            pos.y = value;
        }
        let (_, thumb, top) = geometry(
            node.size().y * factor,
            node.content_size().y * factor,
            height,
            pos.y,
        );
        for (Thumb(target), mut style, mut colour) in &mut thumbs {
            if target != viewport {
                continue;
            }
            if style.top != px(top) {
                style.top = px(top);
            }
            if style.height != px(thumb) {
                style.height = px(thumb);
            }
            let value = if drag.is_some_and(|(e, _)| e == *viewport) || cursor.cursor_over() {
                theme::TEXT
            } else {
                theme::MUTED
            };
            if colour.0 != value {
                colour.0 = value;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scrollbar_geometry_handles_fit_and_limits() {
        assert_eq!(geometry(100.0, 80.0, 100.0, 90.0), (0.0, 100.0, 0.0));
        assert_eq!(geometry(100.0, 400.0, 100.0, 150.0), (300.0, 25.0, 37.5));
        assert_eq!(geometry(100.0, 400.0, 100.0, 999.0), (300.0, 25.0, 75.0));
        assert_eq!(geometry(0.0, 0.0, 0.0, 0.0), (0.0, 0.0, 0.0));
    }

    #[test]
    fn scrollbar_drag_track_click_and_tab_reset_are_functional() {
        let mut app = super::super::integration_tests::app();
        super::super::integration_tests::enter(&mut app);
        app.add_systems(Update, update.after(super::super::actions));
        let viewport = app
            .world_mut()
            .spawn((
                Area(1),
                ComputedNode {
                    size: Vec2::new(200.0, 100.0),
                    content_size: Vec2::new(200.0, 400.0),
                    inverse_scale_factor: 1.0,
                    ..default()
                },
                ScrollPosition::default(),
            ))
            .id();
        let track = app
            .world_mut()
            .spawn((
                Track(viewport),
                ComputedNode {
                    size: Vec2::new(12.0, 100.0),
                    inverse_scale_factor: 1.0,
                    ..default()
                },
                RelativeCursorPosition {
                    cursor_over: true,
                    normalized: Some(Vec2::new(0.5, 0.5)),
                },
                Visibility::Inherited,
            ))
            .id();
        app.world_mut().spawn((
            Thumb(viewport),
            Node::default(),
            BackgroundColor(theme::MUTED),
        ));
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        assert_eq!(
            app.world().get::<ScrollPosition>(viewport).unwrap().y,
            150.0,
            "track click centers thumb"
        );
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear_just_pressed(MouseButton::Left);
        app.world_mut()
            .get_mut::<RelativeCursorPosition>(track)
            .unwrap()
            .normalized = Some(Vec2::new(0.5, 1.5));
        app.update();
        assert_eq!(
            app.world().get::<ScrollPosition>(viewport).unwrap().y,
            300.0,
            "drag continues beyond track and clamps"
        );
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        app.world_mut().resource_mut::<Laboratory>().ui.tab = 2;
        app.update();
        assert_eq!(
            app.world().get::<ScrollPosition>(viewport).unwrap().y,
            0.0,
            "new tab starts at top"
        );
        app.world_mut()
            .get_mut::<ScrollPosition>(viewport)
            .unwrap()
            .y = 150.0;
        app.world_mut()
            .resource_mut::<Laboratory>()
            .constructor
            .workspace
            .selected = 1;
        app.update();
        assert_eq!(
            app.world().get::<ScrollPosition>(viewport).unwrap().y,
            0.0,
            "switching weapon source starts at top"
        );
        app.world_mut()
            .get_mut::<ScrollPosition>(viewport)
            .unwrap()
            .y = 300.0;
        app.world_mut()
            .get_mut::<ComputedNode>(viewport)
            .unwrap()
            .content_size
            .y = 80.0;
        app.update();
        assert_eq!(app.world().get::<ScrollPosition>(viewport).unwrap().y, 0.0);
        assert_eq!(
            *app.world().get::<Visibility>(track).unwrap(),
            Visibility::Hidden
        );
    }
}
