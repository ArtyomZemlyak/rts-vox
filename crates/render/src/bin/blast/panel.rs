//! Mouse-first controls for the one forest range.
use super::*;
use workbench::{Selection, Tool};

#[derive(Resource)]
pub struct Panel {
    pub drive: bool,
    pub demo: bool,
    pub ground_fire: bool,
    pub blocked: bool,
    pub routes: bool,
    keys: Vec<KeyCode>,
    pending: Option<(bool, u32)>,
}
impl Panel {
    pub fn new(drive: bool, demo: bool) -> Self {
        Self {
            drive,
            demo,
            ground_fire: false,
            blocked: false,
            routes: true,
            keys: Vec::new(),
            pending: None,
        }
    }
}
#[derive(Component, Clone, Copy, PartialEq)]
pub enum Action {
    Play,
    Edit,
    Pause,
    Fire,
    Stop,
    Reset,
    Patrol,
    Routes,
    Weapon(bool),
    Power(u32),
    Tool(Tool),
    Delete,
    Rotate,
    Undo,
    Redo,
    Save,
    Load,
    AssignRoute,
    Overview,
    Own,
    Target,
    Zoom(f32),
    Pan(f32, f32),
}
#[derive(Component)]
pub struct Readout;
#[derive(Component)]
pub struct EditorOnly;

pub fn setup(mut commands: Commands, edit: Res<Workshop>, mut fonts: ResMut<Assets<Font>>) {
    if edit.layout.is_none() {
        return;
    }
    let camera = commands
        .spawn((
            Camera2d,
            Camera {
                order: 2,
                clear_color: ClearColorConfig::None,
                ..default()
            },
        ))
        .id();
    // Load an installed UI font in memory; no system font is copied into assets.
    let font = std::fs::read("C:/Windows/Fonts/segoeui.ttf")
        .ok()
        .map(Font::from_bytes)
        .map(|f| fonts.add(f));
    let text_font = TextFont {
        font: font.map(Into::into).unwrap_or_default(),
        font_size: FontSize::Px(14.0),
        ..default()
    };
    let mut root = commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(16),
            top: px(16),
            width: px(282),
            padding: UiRect::all(px(14)),
            row_gap: px(5),
            flex_direction: FlexDirection::Column,
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(10)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.025, 0.045, 0.06, 0.96)),
        BorderColor::all(Color::srgb(0.17, 0.29, 0.32)),
        UiTargetCamera(camera),
    ));
    root.with_children(|p| {
        p.spawn((
            Text::new("ПОЛИГОН / 01"),
            TextFont {
                font_size: FontSize::Px(23.0),
                ..text_font.clone()
            },
            TextColor(Color::srgb(0.83, 0.94, 0.93)),
        ));
        p.spawn((
            Text::new("Ходовые испытания и разрушения"),
            text_font.clone(),
            TextColor(Color::srgb(0.48, 0.65, 0.68)),
        ));
        let rows: Vec<(bool, Vec<(&str, Action)>)> = vec![
            (
                false,
                vec![
                    ("Управлять танком", Action::Play),
                    ("Расстановка", Action::Edit),
                ],
            ),
            (
                false,
                vec![
                    ("Пауза / пуск", Action::Pause),
                    ("Восстановить", Action::Reset),
                ],
            ),
            (
                false,
                vec![
                    ("Огонь по точке", Action::Fire),
                    ("Стоп танк", Action::Stop),
                ],
            ),
            (
                false,
                vec![
                    ("Ход мишеней", Action::Patrol),
                    ("Маршруты", Action::Routes),
                ],
            ),
            (
                false,
                vec![
                    ("Энергопушка", Action::Weapon(true)),
                    ("Фугас", Action::Weapon(false)),
                ],
            ),
            (
                false,
                vec![
                    ("0.5×", Action::Power(500)),
                    ("1×", Action::Power(1000)),
                    ("2×", Action::Power(2000)),
                    ("4×", Action::Power(4000)),
                ],
            ),
            (
                true,
                vec![
                    ("Выбрать", Action::Tool(Tool::Select)),
                    ("Точка прицела", Action::Tool(Tool::Aim)),
                ],
            ),
            (
                true,
                vec![
                    ("Добавить танк", Action::Tool(Tool::Tank)),
                    ("Шагоход", Action::Tool(Tool::Strider)),
                ],
            ),
            (
                true,
                vec![
                    ("Бетон", Action::Tool(Tool::Concrete)),
                    ("Сталь", Action::Tool(Tool::Steel)),
                    ("Грунт", Action::Tool(Tool::Earth)),
                ],
            ),
            (
                true,
                vec![
                    ("Удалить", Action::Delete),
                    ("Повернуть блок", Action::Rotate),
                ],
            ),
            (true, vec![("Маршрут / стоянка", Action::AssignRoute)]),
            (
                true,
                vec![("Отменить", Action::Undo), ("Повторить", Action::Redo)],
            ),
            (
                true,
                vec![("Сохранить", Action::Save), ("Загрузить", Action::Load)],
            ),
            (
                false,
                vec![
                    ("Обзор", Action::Overview),
                    ("Мой танк", Action::Own),
                    ("Мишень", Action::Target),
                ],
            ),
            (
                false,
                vec![
                    ("−", Action::Zoom(1.2)),
                    ("+", Action::Zoom(0.8)),
                    ("←", Action::Pan(-24.0, 0.0)),
                    ("→", Action::Pan(24.0, 0.0)),
                ],
            ),
            (
                false,
                vec![
                    ("Камера ↑", Action::Pan(0.0, -24.0)),
                    ("Камера ↓", Action::Pan(0.0, 24.0)),
                ],
            ),
        ];
        for (editor, row) in rows {
            let mut parent = p.spawn(Node {
                width: percent(100),
                column_gap: px(6),
                ..default()
            });
            if editor {
                parent.insert(EditorOnly);
            }
            let width = 100.0 / row.len() as f32;
            parent.with_children(|r| {
                for (label, action) in row {
                    r.spawn((
                        Button,
                        action,
                        Node {
                            width: percent(width),
                            height: px(30),
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::Center,
                            padding: UiRect::horizontal(px(3)),
                            border_radius: BorderRadius::all(px(5)),
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.075, 0.13, 0.15)),
                    ))
                    .with_children(|b| {
                        b.spawn((
                            Text::new(label),
                            text_font.clone(),
                            TextColor(Color::srgb(0.82, 0.9, 0.91)),
                        ));
                    });
                }
            });
        }
        p.spawn((
            Text::new(""),
            text_font,
            TextColor(Color::srgb(0.61, 0.77, 0.78)),
            Readout,
        ));
    });
}

// Bevy injects each resource independently into this system.
#[allow(clippy::too_many_arguments)]
pub fn actions(
    mut panel: ResMut<Panel>,
    mut scene: ResMut<Scene>,
    mut edit: ResMut<Workshop>,
    interactions: Query<(&Action, &Interaction), Changed<Interaction>>,
    windows: Query<&Window>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut gas: ResMut<GasInput>,
    scale: Option<ResMut<UiScale>>,
) {
    let factor = windows
        .single()
        .map_or(1.0, |w| (w.height() / 860.0).clamp(0.65, 1.0));
    if let Some(mut scale) = scale {
        scale.0 = factor;
    }
    panel.blocked = windows
        .single()
        .ok()
        .and_then(|w| w.cursor_position())
        .is_some_and(|p| p.x < 310.0 * factor);
    if let Some(id) = scene.sim.attack {
        if edit.selected.is_none() {
            edit.selected = Some(Selection::Unit(id));
        }
    }
    let mut key = |k: KeyCode, p: &mut Panel| {
        keys.press(k);
        p.keys.push(k);
    };
    for (&action, interaction) in &interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        scene.notice.clear();
        match action {
            Action::Play => {
                panel.drive = true;
                panel.ground_fire = false;
                edit.editing = false;
                scene.sim.control();
                scene.paused = false;
                scene.view = 1;
            }
            Action::Edit => {
                if !edit.editing {
                    key(KeyCode::KeyE, &mut panel);
                }
            }
            Action::Pause => {
                if !edit.editing {
                    scene.paused = !scene.paused;
                }
            }
            Action::Reset => key(KeyCode::KeyR, &mut panel),
            Action::Fire => {
                panel.ground_fire = !panel.ground_fire;
                panel.drive = true;
                edit.editing = false;
                scene.sim.control();
                scene.paused = false;
            }
            Action::Stop => {
                scene.sim.attack = None;
                scene.sim.gun.motion.stop();
                panel.ground_fire = false;
            }
            Action::Patrol => {
                scene.sim.patrols_running = !scene.sim.patrols_running;
            }
            Action::Routes => panel.routes = !panel.routes,
            Action::Weapon(e) => {
                let p = panel.pending.map_or(scene.power, |v| v.1);
                panel.pending = Some((e, p));
            }
            Action::Power(p) => {
                let e = panel.pending.map_or(scene.energy, |v| v.0);
                panel.pending = Some((e, p));
            }
            Action::Tool(t) => {
                edit.tool = t;
            }
            Action::Delete => key(KeyCode::Delete, &mut panel),
            Action::Rotate => key(KeyCode::KeyQ, &mut panel),
            Action::Undo | Action::Redo => {
                key(KeyCode::ControlLeft, &mut panel);
                key(
                    if action == Action::Undo {
                        KeyCode::KeyZ
                    } else {
                        KeyCode::KeyY
                    },
                    &mut panel,
                );
            }
            Action::AssignRoute => {
                if let (Some(Selection::Unit(id)), Some(mut layout)) =
                    (edit.selected, edit.layout.clone())
                {
                    if let Some(c) = &mut layout.course {
                        if c.patrols.iter().any(|p| p.unit == id) {
                            c.patrols.retain(|p| p.unit != id);
                        } else {
                            let stock = RangeLayout::from_ron(include_str!(
                                "../../../../../data/range-cover.ron"
                            ))
                            .expect("stock course");
                            let template = c
                                .patrols
                                .iter()
                                .find(|p| layout.units[p.unit].kind == layout.units[id].kind)
                                .map(|p| p.points.clone())
                                .or_else(|| {
                                    stock
                                        .course
                                        .as_ref()
                                        .and_then(|c| {
                                            c.patrols.iter().find(|p| {
                                                stock.units[p.unit].kind == layout.units[id].kind
                                            })
                                        })
                                        .map(|p| p.points.clone())
                                });
                            if let Some(points) = template {
                                c.patrols.push(modding::range::Patrol { unit: id, points });
                            }
                        }
                    }
                    if let Err(e) = edit.commit(layout, scaled_config(scene.energy, scene.power)) {
                        scene.notice = e;
                    }
                }
            }
            Action::Save => key(KeyCode::F6, &mut panel),
            Action::Load => {
                if std::path::Path::new("out/range-layout.ron").is_file() {
                    scene.layout_file = Some("out/range-layout.ron".into());
                }
                key(KeyCode::F5, &mut panel);
            }
            Action::Overview => {
                scene.view = 1;
                edit.pan = Vec2::ZERO;
                edit.zoom = 1.0;
            }
            Action::Own => {
                scene.view = 4;
                edit.pan = Vec2::ZERO;
                edit.zoom = 1.0;
            }
            Action::Target => {
                scene.view = 5;
                edit.pan = Vec2::ZERO;
                edit.zoom = 1.0;
            }
            Action::Zoom(z) => edit.zoom = (edit.zoom * z).clamp(0.3, 2.0),
            Action::Pan(x, z) => {
                edit.pan =
                    (edit.pan + Vec2::new(x, z)).clamp(Vec2::splat(-220.0), Vec2::splat(220.0));
            }
        }
    }
    if (edit.editing || scene.sim.ready()) && panel.pending.is_some() {
        let (e, p) = panel.pending.take().unwrap();
        scene.energy = e;
        scene.power = p;
        scene.sim.config = scaled_config(e, p);
        if let Some(r) = &scene.sim.layout {
            scene.sim.config.unit_ablation_resistance = r.unit_resistance;
        }
        let origin = gas.params.origin;
        configure_gas(&scene.sim, e, &mut gas);
        gas.params.origin = origin;
        gas.generation += 1;
    }
}
pub fn release_keys(mut panel: ResMut<Panel>, mut keys: ResMut<ButtonInput<KeyCode>>) {
    for k in panel.keys.drain(..) {
        keys.reset(k);
    }
}
pub fn refresh(
    scene: Res<Scene>,
    edit: Res<Workshop>,
    panel: Res<Panel>,
    mut rows: Query<&mut Node, With<EditorOnly>>,
    mut buttons: Query<(&Action, &Interaction, &mut BackgroundColor)>,
    mut text: Query<&mut Text, With<Readout>>,
) {
    for mut n in &mut rows {
        n.display = if edit.editing {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (a, interaction, mut color) in &mut buttons {
        let active = match *a {
            Action::Play => !edit.editing,
            Action::Edit => edit.editing,
            Action::Pause => scene.paused,
            Action::Fire => panel.ground_fire,
            Action::Weapon(e) => e == panel.pending.map_or(scene.energy, |v| v.0),
            Action::Power(p) => p == panel.pending.map_or(scene.power, |v| v.1),
            Action::Tool(t) => edit.tool == t,
            Action::Patrol => scene.sim.patrols_running,
            Action::Routes => panel.routes,
            _ => false,
        };
        color.0 = if *interaction == Interaction::Hovered {
            Color::srgb(0.16, 0.28, 0.3)
        } else if active {
            Color::srgb(0.09, 0.31, 0.32)
        } else {
            Color::srgb(0.075, 0.13, 0.15)
        };
    }
    let instructions = if edit.editing {
        "ЛКМ — поставить / выбрать\nПКМ — переместить выбранное"
    } else if panel.ground_fire {
        "Укажите точку выстрела на карте"
    } else {
        "ПКМ по земле — ехать\nПКМ по юниту — атаковать\nЛКМ — осмотреть мишень"
    };
    let selected = scene.sim.attack.or_else(|| match edit.selected {
        Some(Selection::Unit(i)) => Some(i),
        _ => None,
    });
    let target = selected
        .and_then(|i| {
            scene.sim.units.get(i).map(|u| {
                format!(
                    "Мишень {} · срезано {}\nЯдро {} · привод {}",
                    i + 1,
                    u.removed.len(),
                    if u.status.alive {
                        "цел"
                    } else {
                        "разрушено"
                    },
                    if u.status.engine {
                        "работает"
                    } else {
                        "отказ"
                    }
                )
            })
        })
        .unwrap_or_else(|| {
            let laps: Vec<_> = scene
                .sim
                .units
                .iter()
                .filter(|u| !u.motion.route.is_empty())
                .map(|u| u.motion.laps.to_string())
                .collect();
            format!("Круги мишеней: {}", laps.join(" / "))
        });
    let state = if edit.editing {
        "РАССТАНОВКА"
    } else if scene.paused {
        "ПАУЗА"
    } else {
        "РЕАЛЬНОЕ ВРЕМЯ · 1×"
    };
    let readiness = if scene.sim.ready() {
        "Готов к выстрелу"
    } else {
        "Заряд / выстрел / перезарядка"
    };
    for mut t in &mut text {
        let pieces = &scene.sim.matter.debris.fragments;
        let debris = if pieces.is_empty() {
            String::new()
        } else {
            let settled = pieces.iter().filter(|f| f.resting).count();
            format!(
                "\nОбломки: {} летят / {settled} на земле\n",
                pieces.len() - settled
            )
        };
        *t = Text::new(format!(
            "{state} · {:.0} FPS\n\n{instructions}\n\n{readiness}\n{target}\n{}{debris}{}",
            scene.fps,
            if panel.pending.is_some() {
                "Новый заряд после выстрела\n"
            } else {
                ""
            },
            scene.notice
        ));
    }
}
