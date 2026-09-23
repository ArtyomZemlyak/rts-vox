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
    pending: Option<(u8, u32)>,
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
    BattlePreset,
    BattleWorld,
    RangeMode,
    BattleStart,
    Side(bool),
    NextOwn,
    Edit,
    Pause,
    Fire,
    AutoAttack,
    Stop,
    Reset,
    Patrol,
    Routes,
    /// Shell 0, energy bolt 1, annihilation bolt 2 (ADR-285).
    Weapon(u8),
    Chassis(RangeUnitKind),
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
    Zone(i32),
    RestartHere,
    Orbit(f32),
    Own,
    Target,
    Zoom(f32),
    Pan(f32, f32),
    /// The live tuning menu (ADR-298), handled in `knobs`.
    Settings,
    Group(u8),
    Knob(u8, bool),
    EditKnob(u8),
    SaveKnobs,
    LoadKnobs,
    ResetKnobs,
}
#[derive(Component)]
pub struct Readout;
#[derive(Component)]
pub struct SectorLabel;
#[derive(Component)]
pub struct ScrollPanel;
#[derive(Component)]
pub struct EditorOnly;
#[derive(Component)]
pub struct RangeOnly;

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
    knobs::build(&mut commands, camera, &text_font);
    let mut root = commands.spawn((
        PanelRoot,
        ScrollPanel,
        ScrollPosition::default(),
        bevy::ui::RelativeCursorPosition::default(),
        Node {
            position_type: PositionType::Absolute,
            left: px(16),
            top: px(16),
            width: px(282),
            max_height: percent(96),
            overflow: Overflow::scroll_y(),
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
        p.spawn((
            Text::new(""),
            text_font.clone(),
            TextColor(Color::srgb(0.85, 0.76, 0.48)),
            SectorLabel,
        ));
        let rows: Vec<(bool, Vec<(&str, Action)>)> = vec![
            (
                false,
                vec![
                    ("Бой на всей карте", Action::BattleWorld),
                    ("Бой 2×2 / расстановка", Action::BattlePreset),
                ],
            ),
            (false, vec![("Режим испытаний / сброс", Action::RangeMode)]),
            (
                false,
                vec![
                    ("Начать бой", Action::BattleStart),
                    ("Следующий свой", Action::NextOwn),
                ],
            ),
            (
                true,
                vec![
                    ("Свои / синие", Action::Side(true)),
                    ("Враги / красные", Action::Side(false)),
                ],
            ),
            (
                false,
                vec![
                    ("‹ Участок", Action::Zone(-1)),
                    ("Участок ›", Action::Zone(1)),
                ],
            ),
            (
                false,
                vec![("Новый опыт здесь (сброс)", Action::RestartHere)],
            ),
            (
                false,
                vec![
                    ("Управлять юнитом", Action::Play),
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
                    ("Стоп юнит", Action::Stop),
                ],
            ),
            (false, vec![("Автоатака", Action::AutoAttack)]),
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
                    ("Энергопушка", Action::Weapon(1)),
                    ("Фугас", Action::Weapon(0)),
                    ("Анипушка", Action::Weapon(2)),
                ],
            ),
            (
                false,
                vec![
                    ("Огнемёт", Action::Weapon(3)),
                    ("Пулемёт", Action::Weapon(4)),
                ],
            ),
            (
                false,
                vec![
                    ("Танк / сброс", Action::Chassis(RangeUnitKind::Tank)),
                    ("Шагоход / сброс", Action::Chassis(RangeUnitKind::Strider)),
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
                    ("Расчистить", Action::Tool(Tool::Clear)),
                    ("Щит", Action::Tool(Tool::Shield)),
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
                    ("Мой юнит", Action::Own),
                    ("Мишень", Action::Target),
                ],
            ),
            (false, vec![("Настройки", Action::Settings)]),
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
                    ("↶", Action::Orbit(-0.25)),
                    ("↷", Action::Orbit(0.25)),
                ],
            ),
        ];
        for (editor, row) in rows {
            let mut parent = p.spawn(Node {
                width: percent(100),
                flex_shrink: 0.0,
                column_gap: px(6),
                ..default()
            });
            if editor {
                parent.insert(EditorOnly);
            }
            if row.iter().any(|(_, a)| {
                matches!(
                    a,
                    Action::Weapon(_) | Action::Chassis(_) | Action::Power(_) | Action::Patrol
                )
            }) {
                parent.insert(RangeOnly);
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

/// A panel's root node: the cursor over it belongs to the panel, not the
/// map (P-24).
#[derive(Component)]
pub struct PanelRoot;

// Bevy injects each resource independently into this system.
#[allow(clippy::too_many_arguments)]
pub fn actions(
    mut panel: ResMut<Panel>,
    mut scene: ResMut<Scene>,
    mut edit: ResMut<Workshop>,
    interactions: Query<(&Action, &Interaction), Changed<Interaction>>,
    windows: Query<&Window>,
    roots: Query<&bevy::ui::RelativeCursorPosition, With<PanelRoot>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut gas: ResMut<GasInput>,
    scale: Option<ResMut<UiScale>>,
    overrides: Option<Res<knobs::Overrides>>,
) {
    let stock = knobs::Overrides::default();
    let overrides = overrides.as_deref().unwrap_or(&stock);
    let factor = windows
        .single()
        .map_or(1.0, |w| (w.height() / 860.0).clamp(0.65, 1.0));
    if let Some(mut scale) = scale {
        scale.0 = factor;
    }
    // The cursor over any panel — the main one or the settings — keeps
    // its clicks off the map (P-24): a nudge of a knob fired the gun.
    panel.blocked = roots.iter().any(|r| r.cursor_over());
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
            Action::BattleWorld => match edit.battle_world(scene.sim.config.clone()) {
                Ok(()) => {
                    scene.view = 1;
                    scene.repeat = false;
                    panel.demo = false;
                    panel.ground_fire = false;
                }
                Err(e) => scene.notice = e,
            },
            Action::BattlePreset => match edit.battle_preset(scene.sim.config.clone()) {
                Ok(()) => {
                    scene.view = 1;
                    scene.repeat = false;
                    panel.demo = false;
                    panel.ground_fire = false;
                }
                Err(e) => scene.notice = e,
            },
            Action::RangeMode => match edit.range_mode(scene.sim.config.clone()) {
                Ok(()) => {
                    panel.drive = true;
                    panel.demo = false;
                    panel.ground_fire = false;
                    panel.pending = None;
                    scene.repeat = false;
                    scene.paused = false;
                }
                Err(e) => scene.notice = e,
            },
            Action::BattleStart => match scene.sim.start_battle() {
                Ok(()) => {
                    edit.editing = false;
                    scene.paused = false;
                    panel.drive = true;
                    panel.demo = false;
                    panel.ground_fire = false;
                    scene.repeat = false;
                    edit.controlled = scene
                        .sim
                        .units
                        .iter()
                        .enumerate()
                        .find(|(i, u)| scene.sim.allied(*i) && u.status.alive)
                        .map(|(i, _)| i);
                }
                Err(e) => scene.notice = e.into(),
            },
            Action::Side(allied) => {
                if edit.editing {
                    if let Err(e) = edit.side(allied, scene.sim.config.clone()) {
                        scene.notice = e;
                    }
                }
            }
            Action::NextOwn => {
                let n = scene.sim.units.len();
                let start = edit.controlled.map_or(0, |i| i + 1);
                if n > 0 {
                    edit.controlled = (0..n)
                        .map(|i| (start + i) % n)
                        .find(|&i| scene.sim.allied(i) && scene.sim.units[i].status.alive);
                    edit.selected = edit.controlled.map(Selection::Unit);
                }
            }
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
            Action::Reset => {
                if scene.sim.battle_mode() {
                    edit.editing = true;
                }
                key(KeyCode::KeyR, &mut panel);
            }
            Action::Fire => {
                scene.sim.set_auto_attack(false);
                panel.ground_fire = !panel.ground_fire;
                panel.drive = true;
                edit.editing = false;
                scene.sim.control();
                scene.paused = false;
            }
            Action::AutoAttack => {
                if scene.sim.battle_mode() {
                    if let Some(id) = edit.controlled {
                        scene
                            .sim
                            .battle_order(id, sim_core::blast::battle::Order::Auto, None);
                    }
                    panel.ground_fire = false;
                    continue;
                }
                let enabled = !scene.sim.auto_attack;
                scene.sim.set_auto_attack(enabled);
                panel.ground_fire = false;
                panel.drive = true;
                edit.editing = false;
                scene.paused = false;
            }
            Action::Stop => {
                if scene.sim.battle_mode() {
                    if let Some(id) = edit.controlled {
                        scene
                            .sim
                            .battle_order(id, sim_core::blast::battle::Order::Hold, None);
                    }
                } else {
                    scene.sim.order_stop();
                }
                panel.ground_fire = false;
            }
            Action::Patrol => scene.sim.toggle_patrols(),
            Action::Routes => panel.routes = !panel.routes,
            Action::Chassis(kind) => {
                panel.pending = None;
                if let Some(mut layout) = edit.layout.clone() {
                    layout.gun_kind = kind;
                    let w = if kind == RangeUnitKind::Strider {
                        4
                    } else {
                        weapon_of(&scene)
                    };
                    let delivery = match w {
                        3 => Delivery::Flame,
                        4 => Delivery::MachineGun,
                        _ => Delivery::Blast,
                    };
                    let cfg = knobs::effective_config(
                        scene.energy,
                        scene.anni,
                        scene.power,
                        delivery,
                        overrides,
                    );
                    match edit.commit(layout, cfg) {
                        Ok(()) => {
                            if delivery != Delivery::Blast {
                                scene.energy = false;
                                scene.anni = false;
                            }
                            scene.notice = "Шасси сменится со сбросом опыта".into();
                        }
                        Err(e) => scene.notice = e.to_string(),
                    }
                }
            }
            Action::Weapon(e) => {
                let p = panel.pending.map_or(scene.power, |v| v.1);
                panel.pending = Some((e, p));
            }
            Action::Power(p) => {
                let e = panel.pending.map_or(weapon_of(&scene), |v| v.0);
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
                    let sector_route = workbench::sector_patrol(&layout, id);
                    if let Some(c) = &mut layout.course {
                        if c.patrols.iter().any(|p| p.unit == id) {
                            c.patrols.retain(|p| p.unit != id);
                        } else {
                            let stock = RangeLayout::from_ron(include_str!(
                                "../../../../../data/range-cover.ron"
                            ))
                            .expect("stock course");
                            let template = sector_route
                                .or_else(|| {
                                    c.patrols
                                        .iter()
                                        .find(|p| {
                                            layout.units[p.unit].kind == layout.units[id].kind
                                        })
                                        .map(|p| p.points.clone())
                                })
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
                    if let Err(e) = edit.commit(
                        layout,
                        knobs::effective_config(
                            scene.energy,
                            scene.anni,
                            scene.power,
                            scene.sim.config.delivery,
                            overrides,
                        ),
                    ) {
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
            Action::Zone(step) => {
                edit.change_zone(step);
                scene.view = 1;
            }
            Action::RestartHere => {
                match edit.restart_here(knobs::effective_config(
                    scene.energy,
                    scene.anni,
                    scene.power,
                    scene.sim.config.delivery,
                    overrides,
                )) {
                    Ok(()) => {
                        panel.drive = true;
                        panel.demo = false;
                        panel.ground_fire = false;
                        scene.repeat = false;
                        scene.view = 1;
                    }
                    Err(e) => scene.notice = e,
                }
            }
            Action::Orbit(delta) => {
                edit.orbit = (edit.orbit + delta).rem_euclid(std::f32::consts::TAU)
            }
            Action::Overview => {
                scene.view = 1;
                edit.pan = Vec2::ZERO;
                edit.zoom = 1.0;
            }
            Action::Own => {
                scene.view = if scene.sim.battle_mode() {
                    edit.selected = edit.controlled.map(Selection::Unit);
                    5
                } else {
                    4
                };
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
                edit.pan_by(Vec2::new(x, z));
            }
            Action::Settings
            | Action::Group(_)
            | Action::Knob(..)
            | Action::EditKnob(_)
            | Action::SaveKnobs
            | Action::LoadKnobs
            | Action::ResetKnobs => {}
        }
    }
    if (edit.editing || scene.sim.ready()) && panel.pending.is_some() {
        let (w, p) = panel.pending.take().unwrap();
        scene.energy = w == 1 || w == 2;
        let delivery = match w {
            3 => Delivery::Flame,
            4 => Delivery::MachineGun,
            _ => Delivery::Blast,
        };
        scene.anni = w == 2;
        scene.power = p;
        let cfg = knobs::effective_config(scene.energy, scene.anni, p, delivery, overrides);
        scene.sim.reconfigure(cfg);
        let origin = gas.params.origin;
        configure_gas(&scene.sim, scene.energy, scene.anni, &mut gas);
        gas.params.origin = origin;
    }
}
/// The scene's weapon as the panel counts them: shell 0, energy bolt 1,
/// annihilation bolt 2.
fn weapon_of(scene: &Scene) -> u8 {
    match scene.sim.config.delivery {
        Delivery::Flame => return 3,
        Delivery::MachineGun => return 4,
        Delivery::Blast => {}
    }
    if scene.anni {
        2
    } else if scene.energy {
        1
    } else {
        0
    }
}
pub fn release_keys(mut panel: ResMut<Panel>, mut keys: ResMut<ButtonInput<KeyCode>>) {
    for k in panel.keys.drain(..) {
        keys.reset(k);
    }
}
// Explicit ECS resources and disjoint UI row markers.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn refresh(
    scene: Res<Scene>,
    edit: Res<Workshop>,
    panel: Res<Panel>,
    streaming: Option<Res<streaming::Streaming>>,
    mut rows: Query<
        (&mut Node, Option<&EditorOnly>, Option<&RangeOnly>),
        Or<(With<EditorOnly>, With<RangeOnly>)>,
    >,
    mut buttons: Query<(&Action, &Interaction, &mut BackgroundColor)>,
    mut text: Query<&mut Text, (With<Readout>, Without<SectorLabel>)>,
    mut sectors: Query<&mut Text, (With<SectorLabel>, Without<Readout>)>,
) {
    for mut t in &mut sectors {
        let sector = edit.sector().map_or_else(
            || "Испытательная площадка".into(),
            |z| {
                format!(
                    "{} / {} · {}",
                    edit.zone + 1,
                    edit.layout.as_ref().unwrap().zones.len(),
                    z.name
                )
            },
        );
        **t = if scene.sim.battle_mode() {
            format!("Бой · {sector} · расстановка по всей карте")
        } else {
            sector
        };
    }
    for (mut n, editor, range) in &mut rows {
        n.display = if (editor.is_none() || edit.editing)
            && (range.is_none() || !scene.sim.battle_mode())
        {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (a, interaction, mut color) in &mut buttons {
        let active = match *a {
            Action::Side(own) => edit.allied == own,
            Action::BattleStart => {
                scene.sim.battle.phase == sim_core::blast::battle::Phase::Fighting
                    && scene.sim.battle_mode()
            }
            Action::Play => !edit.editing,
            Action::Edit => edit.editing,
            Action::Pause => scene.paused,
            Action::Fire => panel.ground_fire,
            Action::AutoAttack => scene.sim.auto_attack,
            Action::Chassis(k) => k == scene.sim.gun.kind,
            Action::Weapon(w) => w == panel.pending.map_or(weapon_of(&scene), |v| v.0),
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
    } else if scene.sim.battle_mode() {
        "ЛКМ — выбрать своего
ПКМ — движение / атака врага"
    } else if panel.ground_fire {
        "Укажите точку выстрела на карте"
    } else if scene.sim.auto_attack {
        "Автоатака — живые мишени в радиусе\nПКМ — перейти к ручному приказу"
    } else {
        "ПКМ по земле — ехать\nПКМ по юниту — атаковать\nЛКМ — осмотреть мишень"
    };
    let selected = scene.sim.attack.or_else(|| match edit.selected {
        Some(Selection::Unit(i)) => Some(i),
        _ => None,
    });
    let mut target = selected
        .and_then(|i| {
            scene.sim.units.get(i).map(|u| {
                format!(
                    "Мишень {} · {}\nЯдро {}% · двигатель {}\nХодовая {} · орудие {}",
                    i + 1,
                    if u.status.alive {
                        "активна"
                    } else {
                        "уничтожена"
                    },
                    u.integrity.core.mul(fx::Fx::from_int(100)).to_int_trunc(),
                    if u.status.engine {
                        format!(
                            "{}%",
                            u.integrity.engine.mul(fx::Fx::from_int(100)).to_int_trunc()
                        )
                    } else {
                        "отказ".into()
                    },
                    if u.status.alive && u.integrity.drive > fx::Fx::ZERO {
                        format!(
                            "{}%",
                            u.integrity.drive.mul(fx::Fx::from_int(100)).to_int_trunc()
                        )
                    } else {
                        "обездвижена".into()
                    },
                    if u.status.weapon {
                        "готово"
                    } else {
                        "отказ"
                    },
                )
            })
        })
        .unwrap_or_else(|| {
            if scene.sim.battle_mode() {
                return String::new();
            }
            let laps: Vec<_> = scene
                .sim
                .units
                .iter()
                .filter(|u| !u.motion.route.is_empty())
                .map(|u| u.motion.laps.to_string())
                .collect();
            format!("Круги мишеней: {}", laps.join(" / "))
        });
    if scene.sim.battle_mode() {
        let [own, enemy] = scene.sim.battle_counts();
        let phase = match scene.sim.battle.phase {
            sim_core::blast::battle::Phase::Setup => "Расставьте отряды и нажмите «Начать бой»",
            sim_core::blast::battle::Phase::Fighting => "БОЙ",
            sim_core::blast::battle::Phase::Victory => "ПОБЕДА",
            sim_core::blast::battle::Phase::Defeat => "ПОРАЖЕНИЕ",
            sim_core::blast::battle::Phase::Draw => "НИЧЬЯ",
        };
        let controlled = edit
            .controlled
            .map_or_else(|| "не выбран".into(), |i| format!("№{}", i + 1));
        target = format!(
            "{phase}
Свои: {own} · Враги: {enemy}
Управление: {controlled}
Танки — фугас · Шагоходы — два пулемёта
{target}"
        );
    }
    let capacity = scene.sim.shield_tuning().capacity.max(1);
    if let Some(Selection::Shield(i)) = edit.selected {
        if let Some(s) = scene.sim.shields.get(i) {
            target = format!(
                "Щит {} · радиус {} · заряд {}%{}",
                i + 1,
                s.radius,
                s.charge * 100 / capacity,
                if s.down > 0 {
                    " · перегружен"
                } else {
                    ""
                }
            );
        }
    }
    let shields = if scene.sim.shields.is_empty() || scene.sim.battle_mode() {
        String::new()
    } else {
        let list: Vec<String> = scene
            .sim
            .shields
            .iter()
            .map(|s| {
                if s.down > 0 {
                    format!("перегружен {:.1} с", s.down as f32 / 30.0)
                } else {
                    format!("{}%", s.charge * 100 / capacity)
                }
            })
            .collect();
        format!("\nЩиты: {}", list.join(" · "))
    };
    let state = if edit.editing {
        "РАССТАНОВКА"
    } else if scene.paused {
        "ПАУЗА"
    } else {
        "РЕАЛЬНОЕ ВРЕМЯ · 1×"
    };
    let weapon_hint = if scene.sim.battle_mode() {
        String::new()
    } else {
        match scene.sim.config.delivery {
            Delivery::Flame => format!(
                "Огнемёт · дальность {} м",
                scene.sim.streams.tuning.flame.range / 10
            ),
            Delivery::MachineGun => format!(
                "Пулемёт · {} ствола · дальность {} м",
                scene.sim.stream_muzzles().len(),
                scene.sim.streams.tuning.machine_gun.range / 10
            ),
            Delivery::Blast => String::new(),
        }
    };
    let readiness = if scene.sim.battle_mode() {
        ""
    } else if scene.sim.ready() {
        "Готов к выстрелу"
    } else {
        "Заряд / выстрел / перезарядка"
    };
    for mut t in &mut text {
        let pieces = &scene.sim.matter.debris.fragments;
        let bodies = scene.sim.bodies.list.len();
        let debris = if pieces.is_empty() && bodies == 0 {
            String::new()
        } else {
            let settled = pieces.iter().filter(|f| f.resting).count();
            let falling = if bodies > 0 {
                format!(" · падает тел: {bodies}")
            } else {
                String::new()
            };
            format!(
                "\nОбломки: {} летят / {settled} на земле{falling}\n",
                pieces.len() - settled
            )
        };
        let loading = streaming
            .as_ref()
            .and_then(|s| s.progress())
            .map_or_else(String::new, |p| format!("Подготовка участка: {p}%\n"));
        *t = Text::new(format!(
            "{state} · {:.0} FPS · тик {:.1} мс\n\n{instructions}\n\n{readiness}\n{weapon_hint}\n{target}{shields}\n{}{debris}{}",
            scene.fps,
            scene.step_us as f32 / 1000.0,
            if panel.pending.is_some() {
                "Новый заряд после выстрела\n"
            } else {
                ""
            },
            format_args!("{loading}{}\n{}", edit.placement_notice(), scene.notice)
        ));
    }
}

/// Scroll the sidebar at small window sizes; map input already ignores its hover.
pub fn scroll(
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    mut panels: Query<
        (
            &bevy::ui::RelativeCursorPosition,
            &ComputedNode,
            &mut ScrollPosition,
        ),
        With<ScrollPanel>,
    >,
) {
    use bevy::input::mouse::MouseScrollUnit;
    let delta: f32 = wheel
        .read()
        .map(|e| {
            -e.y * if e.unit == MouseScrollUnit::Line {
                32.0
            } else {
                1.0
            }
        })
        .sum();
    for (cursor, node, mut pos) in &mut panels {
        if cursor.cursor_over() {
            let max =
                ((node.content_size().y - node.size().y) * node.inverse_scale_factor()).max(0.0);
            pos.y = (pos.y + delta).clamp(0.0, max);
        }
    }
}
