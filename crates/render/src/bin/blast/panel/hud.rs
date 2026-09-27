//! RTS command deck. The simulation owns orders and loadouts; this is presentation.
use super::*;
#[derive(Component)]
pub struct Drawer;
#[derive(Component)]
pub struct DrawerSection(pub u8);
#[derive(Component)]
pub struct HudText(u8);
#[derive(Component)]
pub struct Roster(usize);
#[derive(Component)]
pub struct BattleLoadout;
#[derive(Component)]
pub struct SoloLoadout;
#[derive(Component)]
pub struct BattleButton;
#[derive(Component)]
pub struct BriefingCard;
#[derive(Component)]
pub struct RosterStrip;
pub fn scale(size: Vec2) -> f32 {
    (size.y / 860.0).min(size.x / 1280.0).clamp(0.5, 1.0)
}

pub fn section(editor: bool, a: Action) -> u8 {
    if editor {
        return 4;
    }
    match a {
        Action::Skirmish(_)
        | Action::BattleWorld
        | Action::BattlePreset
        | Action::BattleStart
        | Action::RangeMode => 1,
        Action::Overview
        | Action::WholeMap
        | Action::Own
        | Action::Target
        | Action::Zoom(_)
        | Action::Pan(..)
        | Action::Orbit(_)
        | Action::Pitch(_) => 3,
        Action::Play | Action::Edit | Action::Reset => 4,
        Action::ScenarioMenu
        | Action::Scenario(_)
        | Action::Chassis(_)
        | Action::Power(_)
        | Action::Patrol
        | Action::Routes
        | Action::Zone(_)
        | Action::RestartHere => 2,
        _ => 5,
    }
}
fn label(
    p: &mut ChildSpawnerCommands,
    font: &TextFont,
    text: &str,
    size: f32,
    color: Color,
) -> Entity {
    p.spawn((
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(size),
            ..font.clone()
        },
        TextColor(color),
    ))
    .id()
}
fn button(p: &mut ChildSpawnerCommands, font: &TextFont, text: &str, action: Action) -> Entity {
    let mut b = p.spawn((
        Button,
        action,
        Node {
            min_height: px(34),
            padding: UiRect::axes(px(12), px(7)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(3)),
            ..default()
        },
        BackgroundColor(Color::srgb(0.075, 0.13, 0.15)),
        BorderColor::all(Color::srgb(0.19, 0.29, 0.33)),
    ));
    if matches!(action, Action::BattleStart | Action::NextOwn) {
        b.insert(BattleButton);
    }
    b.with_children(|p| {
        let e = label(p, font, text, 13.0, Color::srgb(0.86, 0.93, 0.94));
        if let Action::SelectUnit(id) = action {
            p.commands().entity(e).insert(HudText(100 + id as u8));
        } else if action == Action::Pause {
            p.commands().entity(e).insert(HudText(4));
        }
    })
    .id()
}
fn row() -> Node {
    Node {
        column_gap: px(6),
        row_gap: px(6),
        flex_wrap: FlexWrap::Wrap,
        ..default()
    }
}
fn chrome(node: Node, camera: Entity) -> impl Bundle {
    (
        PanelRoot,
        bevy::ui::RelativeCursorPosition::default(),
        node,
        BackgroundColor(Color::srgba(0.025, 0.044, 0.055, 0.96)),
        BorderColor::all(Color::srgb(0.19, 0.31, 0.35)),
        UiTargetCamera(camera),
    )
}
pub fn build(commands: &mut Commands, camera: Entity, font: &TextFont) {
    commands
        .spawn(chrome(
            Node {
                position_type: PositionType::Absolute,
                left: px(16),
                right: px(16),
                top: px(12),
                height: px(52),
                padding: UiRect::axes(px(16), px(7)),
                border: UiRect::bottom(px(2)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                column_gap: px(12),
                ..default()
            },
            camera,
        ))
        .with_children(|p| {
            label(
                p,
                font,
                "RTS / ПОЛИГОН",
                19.0,
                Color::srgb(0.83, 0.94, 0.94),
            );
            p.spawn((
                Text::new(""),
                font.clone(),
                TextColor(Color::srgb(0.52, 0.77, 0.80)),
                HudText(0),
            ));
            p.spawn(row()).with_children(|p| {
                for (name, action) in [
                    ("Сражения", Action::Drawer(1)),
                    ("Испытания", Action::Drawer(2)),
                    ("Редактор", Action::Drawer(4)),
                    ("Камера", Action::Drawer(3)),
                    ("Настройки", Action::Settings),
                    ("Сброс", Action::Reset),
                    ("Пауза", Action::Pause),
                ] {
                    button(p, font, name, action);
                }
            });
        });
    commands
        .spawn(chrome(
            Node {
                position_type: PositionType::Absolute,
                left: px(16),
                right: px(16),
                bottom: px(16),
                height: px(188),
                padding: UiRect::all(px(14)),
                column_gap: px(20),
                border: UiRect::top(px(2)),
                ..default()
            },
            camera,
        ))
        .with_children(|p| {
            p.spawn(Node {
                width: percent(30),
                flex_direction: FlexDirection::Column,
                row_gap: px(9),
                ..default()
            })
            .with_children(|p| {
                label(
                    p,
                    font,
                    "ВЫБРАННЫЙ ЮНИТ",
                    11.0,
                    Color::srgb(0.43, 0.68, 0.72),
                );
                p.spawn((
                    Text::new(""),
                    TextFont {
                        font_size: FontSize::Px(16.0),
                        ..font.clone()
                    },
                    TextColor(Color::srgb(0.88, 0.94, 0.94)),
                    HudText(1),
                ));
                p.spawn(row()).with_children(|p| {
                    button(p, font, "Следующий свой", Action::NextOwn);
                    button(p, font, "К юниту", Action::Own);
                });
            });
            p.spawn(Node {
                width: percent(29),
                flex_direction: FlexDirection::Column,
                row_gap: px(9),
                ..default()
            })
            .with_children(|p| {
                label(p, font, "ПРИКАЗЫ", 11.0, Color::srgb(0.43, 0.68, 0.72));
                p.spawn(row()).with_children(|p| {
                    for (name, a) in [
                        ("Атаковать точку", Action::Fire),
                        ("Стоп", Action::Stop),
                        ("Автоатака", Action::AutoAttack),
                        ("Дальности", Action::Ranges),
                    ] {
                        button(p, font, name, a);
                    }
                });
                label(
                    p,
                    font,
                    "ЛКМ: выбрать  /  ПКМ: идти или атаковать",
                    12.0,
                    Color::srgb(0.56, 0.70, 0.73),
                );
                p.spawn(row()).with_children(|p| {
                    button(p, font, "Начать бой", Action::BattleStart);
                    button(p, font, "Сводка", Action::Drawer(5));
                });
            });
            p.spawn(Node {
                width: percent(41),
                flex_direction: FlexDirection::Column,
                row_gap: px(9),
                ..default()
            })
            .with_children(|p| {
                label(p, font, "ВООРУЖЕНИЕ", 11.0, Color::srgb(0.43, 0.68, 0.72));
                p.spawn((row(), BattleLoadout)).with_children(|p| {
                    for (name, w) in [
                        ("По цели", None),
                        ("Фугас", Some(BattleWeapon::Blast)),
                        ("Бронебойный", Some(BattleWeapon::Kinetic)),
                        ("Энергия", Some(BattleWeapon::Energy)),
                        ("Аннигиляция", Some(BattleWeapon::Annihilation)),
                    ] {
                        button(p, font, name, Action::BattleWeapon(w));
                    }
                });
                p.spawn((row(), SoloLoadout)).with_children(|p| {
                    for (name, w) in [
                        ("Фугас", 0),
                        ("Энергия", 1),
                        ("Аннигиляция", 2),
                        ("Бронебойный", 5),
                        ("Огнемёт", 3),
                        ("Пулемёт", 4),
                        ("Лазер", 6),
                    ] {
                        button(p, font, name, Action::Weapon(w));
                    }
                });
                p.spawn((
                    Text::new(""),
                    TextFont {
                        font_size: FontSize::Px(13.0),
                        ..font.clone()
                    },
                    TextColor(Color::srgb(0.78, 0.75, 0.57)),
                    HudText(2),
                ));
            });
        });
    commands
        .spawn(chrome(
            Node {
                position_type: PositionType::Absolute,
                left: px(182),
                right: px(16),
                bottom: px(212),
                padding: UiRect::all(px(6)),
                column_gap: px(5),
                max_width: percent(96),
                flex_wrap: FlexWrap::Wrap,
                ..default()
            },
            camera,
        ))
        .insert(RosterStrip)
        .with_children(|p| {
            for id in 0..modding::range::MAX_RANGE_UNITS {
                let e = button(p, font, &format!("Юнит {}", id + 1), Action::SelectUnit(id));
                p.commands().entity(e).insert(Roster(id));
            }
        });
    commands
        .spawn((
            BriefingCard,
            Node {
                position_type: PositionType::Absolute,
                left: px(16),
                top: px(76),
                width: percent(47),
                padding: UiRect::all(px(10)),
                border: UiRect::left(px(2)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.025, 0.044, 0.055, 0.85)),
            BorderColor::all(Color::srgb(0.46, 0.64, 0.61)),
            UiTargetCamera(camera),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(14.0),
                    ..font.clone()
                },
                TextColor(Color::srgb(0.94, 0.91, 0.73)),
                HudText(3),
            ));
        });
}
fn weapon_name(w: BattleWeapon) -> &'static str {
    match w {
        BattleWeapon::Blast => "Фугас",
        BattleWeapon::Kinetic => "Бронебойный",
        BattleWeapon::MachineGun => "Два пулемёта",
        BattleWeapon::Energy => "Энергопушка",
        BattleWeapon::Annihilation => "Аннигиляционная пушка",
        BattleWeapon::Laser => "Тяжёлый лазер",
    }
}

pub fn refresh(
    scene: Res<Scene>,
    edit: Res<Workshop>,
    panel: Res<Panel>,
    mut text: Query<(&HudText, &mut Text)>,
    mut nodes: Query<(
        &mut Node,
        Option<&Drawer>,
        Option<&Roster>,
        Option<&BattleLoadout>,
        Option<&SoloLoadout>,
        Option<&BattleButton>,
        Option<&Action>,
        Option<&BriefingCard>,
        Option<&RosterStrip>,
    )>,
) {
    let battle = scene.sim.battle_mode();
    let selected = if battle {
        edit.controlled
            .and_then(|i| scene.sim.units.get(i).map(|u| (i, u)))
    } else {
        Some((usize::MAX, &scene.sim.gun))
    };
    for (mut node, drawer, roster, loadout, solo, battle_button, action, briefing, strip) in
        &mut nodes
    {
        let visible = if drawer.is_some() {
            Some(panel.drawer != 0)
        } else if let Some(Roster(i)) = roster {
            Some(
                battle
                    && scene.sim.allied(*i)
                    && scene.sim.units.get(*i).is_some_and(|u| u.status.alive),
            )
        } else if loadout.is_some() {
            Some(
                battle
                    && selected
                        .is_some_and(|(_, u)| u.kind == RangeUnitKind::Tank && u.status.alive),
            )
        } else if solo.is_some() {
            Some(!battle)
        } else if battle_button.is_some() {
            Some(
                battle
                    && (action != Some(&Action::BattleStart)
                        || scene.sim.battle.phase == sim_core::blast::battle::Phase::Setup),
            )
        } else if briefing.is_some() {
            Some(battle || !scene.notice.is_empty() || !panel.briefing.is_empty())
        } else if strip.is_some() {
            Some(battle && scene.sim.battle_counts()[0] > 0)
        } else {
            None
        };
        if let Some(show) = visible {
            node.display = if show { Display::Flex } else { Display::None };
        }
    }
    for (id, mut t) in &mut text {
        let value = match id.0 {
            n if n >= 100 => {
                scene
                    .sim
                    .units
                    .get((n - 100) as usize)
                    .map_or_else(String::new, |u| {
                        format!(
                            "{} {:02} · {}%",
                            if u.kind == RangeUnitKind::Spider {
                                "П"
                            } else if u.kind == RangeUnitKind::Tank {
                                "Т"
                            } else {
                                "Ш"
                            },
                            n - 99,
                            u.integrity.core.mul(fx::Fx::from_int(100)).to_int_trunc()
                        )
                    })
            }
            4 => {
                if scene.paused {
                    "Продолжить".into()
                } else {
                    "Пауза".into()
                }
            }
            0 => {
                if battle {
                    let [a, b] = scene.sim.battle_counts();
                    format!("СВОИ {a}  /  ВРАГИ {b}  ·  ИИ АКТИВЕН")
                } else {
                    format!(
                        "1 КМ  ·  {:.0} FPS  ·  {}",
                        scene.fps,
                        if scene.paused {
                            "ПАУЗА"
                        } else {
                            "ИСПЫТАНИЯ"
                        }
                    )
                }
            }
            1 => selected.map_or_else(
                || "Выберите свой юнит".into(),
                |(id, u)| {
                    format!(
                        "{} {}  ·  {}\nЯдро {}%   Ходовая {}%",
                        if u.kind == RangeUnitKind::Spider {
                            "ПАУК"
                        } else if u.kind == RangeUnitKind::Tank {
                            "ТАНК"
                        } else {
                            "ШАГОХОД"
                        },
                        if id == usize::MAX {
                            String::new()
                        } else {
                            format!("{:02}", id + 1)
                        },
                        if u.status.alive {
                            "В СТРОЮ"
                        } else {
                            "УНИЧТОЖЕН"
                        },
                        u.integrity.core.mul(fx::Fx::from_int(100)).to_int_trunc(),
                        u.integrity.drive.mul(fx::Fx::from_int(100)).to_int_trunc()
                    )
                },
            ),
            2 => {
                if battle {
                    selected.map_or_else(
                        || "Выберите танк для смены боеприпаса".into(),
                        |(i, _)| {
                            format!(
                                "{} · дальность {} м\nСмена заряда сохраняет перезарядку",
                                weapon_name(
                                    scene.sim.battle_weapon(i).unwrap_or(BattleWeapon::Blast)
                                ),
                                scene.sim.battle_attack_range(i).unwrap_or(0) / 10
                            )
                        },
                    )
                } else {
                    format!(
                        "Мощность {:.1}×  ·  {}",
                        scene.power as f32 / 1000.0,
                        if scene.sim.ready() {
                            "Готов к выстрелу"
                        } else {
                            "Заряд / выстрел"
                        }
                    )
                }
            }
            _ => {
                let phase = if battle {
                    match scene.sim.battle.phase {
                        sim_core::blast::battle::Phase::Setup => {
                            "РАССТАНОВКА · нажмите «Начать бой»"
                        }
                        sim_core::blast::battle::Phase::Fighting => "БОЙ · уничтожьте противника",
                        sim_core::blast::battle::Phase::Victory => "ПОБЕДА",
                        sim_core::blast::battle::Phase::Defeat => "ПОРАЖЕНИЕ",
                        sim_core::blast::battle::Phase::Draw => "НИЧЬЯ",
                    }
                } else {
                    ""
                };
                format!(
                    "{phase}\n{}{}",
                    panel.briefing,
                    if scene.notice.is_empty() {
                        String::new()
                    } else {
                        format!("\n{}", scene.notice)
                    }
                )
            }
        };
        if **t != value {
            **t = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> App {
        let layout =
            RangeLayout::from_ron(include_str!("../../../../../../data/range-km.ron")).unwrap();
        let mut workshop = Workshop::new(Some(layout), false);
        workshop.battle_preset(config(false, false)).unwrap();
        let (c, layout) = workshop.pending.take().unwrap();
        let sim = BlastRange::with_layout(c, layout).unwrap();
        let scene = Scene {
            generation: 0,
            sim,
            energy: true,
            anni: false,
            paused: true,
            frame: 0,
            finish: 420,
            out: None,
            all_frames: false,
            wait: 0,
            captured: 0,
            acc: 0.0,
            repeat: false,
            view: 1,
            camera_sweep: false,
            patrol: 1,
            previous_shell: None,
            fps: 60.0,
            step_us: 0,
            flame: 0,
            realtime: true,
            live_elapsed: 0.0,
            last_capture: 0,
            layout_file: Some("data/range-cover.ron".into()),
            power: 1000,
            uncovered: false,
            unit_override: None,
            skip_delta: true,
            notice: String::new(),
        };

        let mut app = App::new();
        app.insert_resource(scene)
            .insert_resource(workshop)
            .insert_resource(Panel::new(true, false))
            .insert_resource(GasInput::default())
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(
                Update,
                (super::super::actions, super::super::scenario_progress).chain(),
            );
        app
    }
    fn click(app: &mut App, action: Action) {
        let e = app.world_mut().spawn((action, Interaction::Pressed)).id();
        app.update();
        app.world_mut().despawn(e);
    }
    #[test]
    fn hud_selects_tank_ammunition_and_issues_orders() {
        let mut app = app();
        click(&mut app, Action::BattleStart);
        click(&mut app, Action::SelectUnit(0));
        click(&mut app, Action::BattleWeapon(Some(BattleWeapon::Kinetic)));
        assert_eq!(
            app.world().resource::<Scene>().sim.battle.actors[0].weapon,
            Some(BattleWeapon::Kinetic)
        );
        for weapon in [BattleWeapon::Energy, BattleWeapon::Annihilation] {
            click(&mut app, Action::BattleWeapon(Some(weapon)));
            assert_eq!(
                app.world().resource::<Scene>().sim.battle_weapon(0),
                Some(weapon)
            );
        }
        click(&mut app, Action::Stop);
        assert_eq!(
            app.world().resource::<Scene>().sim.battle.actors[0].order,
            sim_core::blast::battle::Order::Hold
        );
        click(&mut app, Action::AutoAttack);
        assert_eq!(
            app.world().resource::<Scene>().sim.battle.actors[0].order,
            sim_core::blast::battle::Order::Auto
        );
        click(&mut app, Action::Drawer(3));
        assert_eq!(app.world().resource::<Panel>().drawer, 3);
        click(&mut app, Action::Ranges);
        assert!(!app.world().resource::<Panel>().ranges);
    }
    #[test]
    fn authored_battles_load_through_hud_and_enemy_ai_acquires_targets() {
        let mut app = app();
        for (id, expected) in [(0, [3, 3]), (1, [3, 4])] {
            click(&mut app, Action::Skirmish(id));
            assert!(app.world().resource::<Panel>().battle_launch);
            let (cfg, layout) = app
                .world_mut()
                .resource_mut::<Workshop>()
                .pending
                .take()
                .unwrap();
            app.world_mut().resource_mut::<Scene>().sim =
                BlastRange::with_layout(cfg, layout).unwrap();
            app.update();
            assert!(!app.world().resource::<Panel>().battle_launch);
            assert!(!app.world().resource::<Workshop>().editing);
            let mut scene = app.world_mut().resource_mut::<Scene>();
            assert_eq!(scene.sim.battle_counts(), expected);
            assert_eq!(
                scene.sim.battle.phase,
                sim_core::blast::battle::Phase::Fighting
            );
            scene.sim.step();
            assert!(scene
                .sim
                .battle
                .actors
                .iter()
                .enumerate()
                .any(|(i, a)| !scene.sim.allied(i) && a.target.is_some()));
        }
    }
}
