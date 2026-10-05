//! RTS command deck. The simulation owns orders and loadouts; this is presentation.
use super::*;
use bevy::ui::FocusPolicy;
#[derive(Component)]
pub struct Drawer;
#[derive(Component)]
pub struct DrawerSection(pub u8);
#[derive(Component)]
pub struct HudText(pub(super) u8);
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
#[derive(Component)]
pub struct CommandDeck;
#[derive(Component)]
pub struct HeaderBrand;
#[derive(Component)]
pub struct BattleControls;
#[derive(Component)]
pub struct SelectionBox;
pub const ROSTER_PAGE_SIZE: usize = 8;
pub fn roster_page_size(window_width: f32) -> usize {
    if window_width < 1280.0 {
        5
    } else {
        ROSTER_PAGE_SIZE
    }
}
/// The compact layout handles the supported 960×540 window without shrinking
/// labels and click targets along with the rest of the interface.
pub fn scale(_size: Vec2) -> f32 {
    1.0
}

pub fn section(editor: bool, a: Action) -> u8 {
    if editor {
        return 4;
    }
    match a {
        Action::Skirmish(_)
        | Action::BattleWorld
        | Action::BattleWorldAuto
        | Action::BattlePreset
        | Action::BattleStart
        | Action::FreeControl
        | Action::FreeRange
        | Action::BattleReplay
        | Action::BattleChoose
        | Action::SelectAllOwn
        | Action::RangeMode => 1,
        Action::Overview
        | Action::WholeMap
        | Action::Own
        | Action::Target
        | Action::Zoom(_)
        | Action::Pan(..)
        | Action::Orbit(_)
        | Action::Pitch(_)
        | Action::ProjectileMarkers => 3,
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
            min_height: px(38),
            padding: UiRect::axes(px(8), px(7)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(1)),
            ..default()
        },
        BackgroundColor(theme::BUTTON),
        BorderColor::all(theme::FRAME),
    ));
    if matches!(action, Action::BattleStart | Action::NextOwn) {
        b.insert(BattleButton);
    }
    b.with_children(|p| {
        let e = label(p, font, text, 12.0, theme::TEXT);
        if let Action::SelectUnit(id) = action {
            p.commands().entity(e).insert(HudText(100 + id as u8));
        } else if action == Action::Pause {
            p.commands().entity(e).insert(HudText(4));
        } else if action == Action::Reset {
            p.commands().entity(e).insert(HudText(6));
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
        BackgroundColor(theme::PANEL),
        BorderColor::all(theme::FRAME),
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
                height: px(56),
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
            let brand = label(p, font, "RTS / ПОЛИГОН", 19.0, theme::TEXT);
            p.commands()
                .entity(brand)
                .insert((HeaderBrand, Visibility::Visible));
            p.spawn((
                Text::new(""),
                font.clone(),
                TextColor(theme::MUTED),
                HudText(0),
            ));
            let mut header_row = row();
            header_row.flex_wrap = FlexWrap::NoWrap;
            p.spawn(header_row).with_children(|p| {
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
                height: px(160),
                padding: UiRect::all(px(14)),
                column_gap: px(20),
                border: UiRect::top(px(2)),
                ..default()
            },
            camera,
        ))
        .insert(CommandDeck)
        .with_children(|p| {
            p.spawn(Node {
                width: percent(28),
                flex_direction: FlexDirection::Column,
                row_gap: px(9),
                ..default()
            })
            .with_children(|p| {
                label(p, font, "ВЫБРАННЫЙ ЮНИТ", 11.0, theme::MUTED);
                p.spawn((
                    Text::new(""),
                    TextFont {
                        font_size: FontSize::Px(16.0),
                        ..font.clone()
                    },
                    TextColor(theme::TEXT),
                    HudText(1),
                ));
                p.spawn(row()).with_children(|p| {
                    button(p, font, "Следующий свой", Action::NextOwn);
                    button(p, font, "К юниту", Action::Own);
                });
            });
            p.spawn(Node {
                width: percent(35),
                flex_direction: FlexDirection::Column,
                row_gap: px(9),
                ..default()
            })
            .with_children(|p| {
                label(p, font, "ПРИКАЗЫ", 11.0, theme::MUTED);
                p.spawn(row()).with_children(|p| {
                    for (name, a) in [
                        ("Атаковать точку", Action::Fire),
                        ("Стоп", Action::Stop),
                        ("Автоатака", Action::AutoAttack),
                        ("Радиусы выбранных", Action::Ranges),
                    ] {
                        button(p, font, name, a);
                    }
                });
                label(
                    p,
                    font,
                    "ЛКМ: выбрать  /  ПКМ: отдать приказ",
                    12.0,
                    theme::MUTED,
                );
            });
            p.spawn(Node {
                width: percent(31),
                flex_direction: FlexDirection::Column,
                row_gap: px(9),
                ..default()
            })
            .with_children(|p| {
                label(p, font, "ВООРУЖЕНИЕ", 11.0, theme::MUTED);
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
                    TextColor(theme::ACCENT),
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
                bottom: px(184),
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
            button(p, font, "Назад", Action::RosterPage(-1));
            let label = label(p, font, "", 13.0, theme::TEXT);
            p.commands().entity(label).insert(HudText(5));
            button(p, font, "Все", Action::SelectAllOwn);
            for id in 0..modding::range::MAX_RANGE_UNITS {
                let e = button(p, font, &format!("Юнит {}", id + 1), Action::SelectUnit(id));
                p.commands().entity(e).insert(Roster(id));
            }
            button(p, font, "Далее", Action::RosterPage(1));
        });
    commands
        .spawn(chrome(
            Node {
                position_type: PositionType::Absolute,
                left: px(16),
                top: px(76),
                width: percent(47),
                padding: UiRect::all(px(10)),
                border: UiRect::left(px(2)),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                ..default()
            },
            camera,
        ))
        .insert(BattleControls)
        .with_children(|p| {
            label(p, font, "ПОЛИГОН · РЕЖИМ И СЦЕНАРИЙ", 11.0, theme::MUTED);
            p.spawn(row()).with_children(|p| {
                button(p, font, "Свободный полигон", Action::FreeRange);
                button(p, font, "Управлять без боя", Action::FreeControl);
                button(p, font, "Начать бой", Action::BattleStart);
                button(p, font, "Повторить бой", Action::BattleReplay);
                button(p, font, "Другой бой", Action::BattleChoose);
                let summary = button(p, font, "Сводка", Action::Drawer(5));
                p.commands().entity(summary).insert(BattleButton);
            });
        });
    commands.spawn((
        SelectionBox,
        Node {
            position_type: PositionType::Absolute,
            display: Display::None,
            border: UiRect::all(px(1)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.24, 0.68, 0.9, 0.0)),
        BorderColor::all(Color::srgba(0.55, 0.9, 1.0, 0.9)),
        FocusPolicy::Pass,
        UiTargetCamera(camera),
    ));
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
            BackgroundColor(theme::PANEL),
            BorderColor::all(theme::ACCENT),
            UiTargetCamera(camera),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(14.0),
                    ..font.clone()
                },
                TextColor(theme::TEXT),
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
        BattleWeapon::Canister => "Картечь",
    }
}

pub fn refresh(
    scene: Res<Scene>,
    edit: Res<Workshop>,
    panel: Res<Panel>,
    windows: Query<&Window>,
    battle_bar: Query<&ComputedNode, With<BattleControls>>,
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
        Option<&CommandDeck>,
        Option<&HeaderBrand>,
        Option<&BattleControls>,
    )>,
) {
    let battle = scene.sim.battle_mode();
    let (compact, hide_brand) = windows.single().map_or((false, false), |w| {
        (w.width() < 1280.0 || w.height() < 760.0, w.width() < 1280.0)
    });
    let selected = if battle {
        edit.controlled
            .and_then(|i| scene.sim.units.get(i).map(|u| (i, u)))
    } else {
        Some((usize::MAX, &scene.sim.gun))
    };
    let selected_ids = if battle {
        edit.battle_selected_ids(&scene.sim)
    } else {
        Vec::new()
    };
    let loadout_count =
        selected_ids
            .iter()
            .filter(|&&i| {
                scene.sim.units.get(i).is_some_and(|u| {
                    matches!(u.kind, RangeUnitKind::Tank | RangeUnitKind::Artillery)
                })
            })
            .count();
    let roster: Vec<_> = scene
        .sim
        .units
        .iter()
        .enumerate()
        .filter(|(i, u)| scene.sim.allied(*i) && u.status.alive)
        .map(|(i, _)| i)
        .collect();
    let page_size = windows
        .single()
        .map_or(ROSTER_PAGE_SIZE, |window| roster_page_size(window.width()));
    let pages = roster.len().div_ceil(page_size).max(1);
    let page = panel.roster_page % pages;
    for (
        mut node,
        drawer,
        roster_button,
        loadout,
        solo,
        battle_button,
        action,
        briefing,
        strip,
        deck,
        brand,
        battle_controls,
    ) in &mut nodes
    {
        if deck.is_some() {
            let height = px(if compact { 210 } else { 160 });
            if node.height != height {
                node.height = height;
            }
        }
        if drawer.is_some() {
            let bottom = px(if compact { 16 } else { 236 });
            if node.bottom != bottom {
                node.bottom = bottom;
            }
        }
        if briefing.is_some() {
            let width = if compact { Val::Auto } else { percent(47) };
            if node.width != width {
                node.width = width;
            }
            node.left = px(if compact { 182 } else { 16 });
            node.right = if compact { px(16) } else { Val::Auto };
            node.top = px(84.0
                + battle_bar
                    .single()
                    .map_or(108.0, |n| n.size().y * n.inverse_scale_factor()));
        }
        if battle_controls.is_some() {
            // Minimap ends at x=170 in the compact layout. The drawer
            // occupies the rightmost 330px, so neither may cover a button.
            node.left = px(if compact { 182 } else { 16 });
            node.right = if battle && compact {
                px(if panel.drawer != 0 { 362 } else { 16 })
            } else {
                Val::Auto
            };
            node.width = if !battle {
                px(230)
            } else if compact {
                Val::Auto
            } else {
                percent(47)
            };
        }
        if strip.is_some() {
            let bottom = px(if compact { 234 } else { 184 });
            if node.bottom != bottom {
                node.bottom = bottom;
            }
        }
        let visible = if brand.is_some() {
            Some(!hide_brand)
        } else if battle_controls.is_some() {
            Some(true)
        } else if drawer.is_some() {
            Some(panel.drawer != 0)
        } else if let Some(Roster(i)) = roster_button {
            Some(
                battle
                    && roster
                        .iter()
                        .skip(page * page_size)
                        .take(page_size)
                        .any(|id| id == i),
            )
        } else if matches!(action, Some(Action::RosterPage(_))) {
            Some(battle && pages > 1)
        } else if loadout.is_some() {
            Some(battle && loadout_count > 0)
        } else if solo.is_some() {
            Some(!battle)
        } else if matches!(action, Some(Action::FreeRange)) {
            Some(!battle)
        } else if matches!(action, Some(Action::FreeControl)) {
            Some(
                battle
                    && edit.editing
                    && scene.sim.battle.phase == sim_core::blast::battle::Phase::Setup,
            )
        } else if matches!(action, Some(Action::Fire | Action::AutoAttack)) {
            Some(!battle || scene.sim.battle.phase == sim_core::blast::battle::Phase::Fighting)
        } else if matches!(action, Some(Action::BattleReplay)) {
            Some(battle && scene.sim.battle.phase != sim_core::blast::battle::Phase::Setup)
        } else if matches!(action, Some(Action::BattleChoose)) {
            Some(battle)
        } else if matches!(action, Some(Action::Pause)) {
            Some(!edit.editing)
        } else if battle_button.is_some() {
            Some(
                battle
                    && (action != Some(&Action::BattleStart)
                        || scene.sim.battle.phase == sim_core::blast::battle::Phase::Setup),
            )
        } else if briefing.is_some() {
            Some(
                !(compact && panel.drawer != 0)
                    && (!scene.notice.is_empty()
                        || (battle
                            && scene.sim.battle.phase == sim_core::blast::battle::Phase::Setup)
                        || (!battle && !panel.briefing.is_empty())),
            )
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
            5 => format!("{}/{}", page + 1, pages),
            7 => format!(
                "Метки снарядов: {}",
                if panel.projectile_markers {
                    "вкл"
                } else {
                    "выкл"
                }
            ),
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
                            } else if u.kind == RangeUnitKind::ArtilleryT2 {
                                "А2"
                            } else if u.kind == RangeUnitKind::Artillery {
                                "А"
                            } else if u.kind == RangeUnitKind::TurretT1 {
                                "1"
                            } else if u.kind == RangeUnitKind::TurretT2 {
                                "2"
                            } else if u.kind == RangeUnitKind::Base {
                                "Б"
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
            6 => {
                if battle {
                    "Расстановка".into()
                } else {
                    "Сброс".into()
                }
            }
            0 => {
                if battle {
                    let [a, b] = scene.sim.battle_counts();
                    let phase = match scene.sim.battle.phase {
                        sim_core::blast::battle::Phase::Setup if !edit.editing => "БЕЗ БОЯ",
                        sim_core::blast::battle::Phase::Setup => "РАССТАНОВКА",
                        sim_core::blast::battle::Phase::Fighting => "ИИ АКТИВЕН",
                        sim_core::blast::battle::Phase::Victory => "ПОБЕДА",
                        sim_core::blast::battle::Phase::Defeat => "ПОРАЖЕНИЕ",
                        sim_core::blast::battle::Phase::Draw => "НИЧЬЯ",
                    };
                    let name = if panel.battle_name.is_empty() {
                        "БОЙ"
                    } else {
                        &panel.battle_name
                    };
                    if compact {
                        format!("{name} · {phase}")
                    } else if let Some([own, enemy]) = scene.sim.battle_base_integrity() {
                        format!("{name} · БАЗЫ {own}% / {enemy}% · {phase}")
                    } else {
                        format!("{name} · СВОИ {a} / ВРАГИ {b} · {phase}")
                    }
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
                    if u.kind == RangeUnitKind::Base {
                        return format!(
                            "БАЗА {:02}  ·  {}\nЯдро {}%   ·   ЦЕЛЬ БОЯ",
                            id + 1,
                            if u.status.alive {
                                "В СТРОЮ"
                            } else {
                                "УНИЧТОЖЕНА"
                            },
                            u.integrity.core.mul(fx::Fx::from_int(100)).to_int_trunc()
                        );
                    }
                    format!(
                        "{} {}  ·  {}\nЯдро {}%   Ходовая {}%",
                        if u.kind == RangeUnitKind::Spider {
                            "ПАУК"
                        } else if u.kind == RangeUnitKind::Tank {
                            "ТАНК"
                        } else if u.kind == RangeUnitKind::ArtilleryT2 {
                            "АРТИЛЛЕРИЯ T2"
                        } else if u.kind == RangeUnitKind::Artillery {
                            "АРТИЛЛЕРИЯ T1"
                        } else if u.kind == RangeUnitKind::TurretT1 {
                            "ТУРЕЛЬ T1"
                        } else if u.kind == RangeUnitKind::TurretT2 {
                            "ТУРЕЛЬ T2"
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
                    if selected_ids.len() > 1 {
                        format!(
                            "В отряде {} танков/артиллерий\nСмена заряда — для всей группы",
                            loadout_count
                        )
                    } else {
                        selected.map_or_else(
                            || "Выберите танк или артиллерию для смены боеприпаса".into(),
                            |(i, u)| {
                                format!(
                                    "{} · дальность {} м\n{}",
                                    weapon_name(
                                        scene.sim.battle_weapon(i).unwrap_or(BattleWeapon::Blast)
                                    ),
                                    scene.sim.battle_attack_range(i).unwrap_or(0) / 10,
                                    if matches!(
                                        u.kind,
                                        RangeUnitKind::Tank | RangeUnitKind::Artillery
                                    ) {
                                        "Смена заряда сохраняет перезарядку"
                                    } else {
                                        "Ведёт огонь во время движения"
                                    }
                                )
                            },
                        )
                    }
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
                        sim_core::blast::battle::Phase::Setup if !edit.editing => {
                            "СВОБОДНОЕ УПРАВЛЕНИЕ · стрельба и ИИ выключены"
                        }
                        sim_core::blast::battle::Phase::Setup => {
                            "РАССТАНОВКА · выберите бой или управление без боя"
                        }
                        sim_core::blast::battle::Phase::Fighting => "БОЙ · уничтожьте противника",
                        sim_core::blast::battle::Phase::Victory => "ПОБЕДА",
                        sim_core::blast::battle::Phase::Defeat => "ПОРАЖЕНИЕ",
                        sim_core::blast::battle::Phase::Draw => "НИЧЬЯ",
                    }
                } else {
                    ""
                };
                let briefing = if compact && battle {
                    let mut chars = panel.briefing.chars();
                    let short: String = chars.by_ref().take(64).collect();
                    if chars.next().is_some() {
                        format!("{short}…")
                    } else {
                        short
                    }
                } else {
                    panel.briefing.clone()
                };
                format!(
                    "{phase}\n{}{}",
                    briefing,
                    if scene.notice.is_empty() {
                        String::new()
                    } else {
                        format!("\n{}", scene.notice)
                    }
                )
            }
        };
        let value = if id.0 == 1 {
            let count = selected_ids.len();
            if count > 1 {
                format!("ОТРЯД: {count}\n{value}")
            } else {
                value
            }
        } else {
            value
        };
        if **t != value {
            **t = value;
        }
    }
}

pub fn refresh_selection_box(
    edit: Res<Workshop>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    mut boxes: Query<&mut Node, With<SelectionBox>>,
) {
    let pointer_inside = windows
        .single()
        .map_or(true, |w| w.cursor_position().is_some());
    for mut node in &mut boxes {
        let drag = match (edit.drag_start, edit.drag_current) {
            (Some(start), Some(end))
                if mouse.pressed(MouseButton::Left)
                    && pointer_inside
                    && !edit.editing
                    && start.is_finite()
                    && end.is_finite()
                    && start.distance_squared(end) >= 64.0 =>
            {
                Some((start, end))
            }
            _ => None,
        };
        if let Some((start, end)) = drag {
            let (start, end) = if let Ok(window) = windows.single() {
                let size = Vec2::new(window.width(), window.height());
                (start.clamp(Vec2::ZERO, size), end.clamp(Vec2::ZERO, size))
            } else {
                (start, end)
            };
            let lo = start.min(end);
            let hi = start.max(end);
            node.left = px(lo.x);
            node.top = px(lo.y);
            node.width = px(hi.x - lo.x);
            node.height = px(hi.y - lo.y);
            node.display = Display::Flex;
        } else {
            node.display = Display::None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_outline_only_appears_during_a_real_drag() {
        let mut app = App::new();
        app.insert_resource(Workshop::new(None, false))
            .init_resource::<ButtonInput<MouseButton>>()
            .add_systems(Update, refresh_selection_box);
        let box_id = app.world_mut().spawn((SelectionBox, Node::default())).id();
        {
            let mut edit = app.world_mut().resource_mut::<Workshop>();
            edit.drag_start = Some(Vec2::new(20.0, 30.0));
            edit.drag_current = Some(Vec2::new(800.0, 600.0));
        }
        app.update();
        assert_eq!(
            app.world().entity(box_id).get::<Node>().unwrap().display,
            Display::None
        );
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.world_mut().resource_mut::<Workshop>().drag_current = Some(Vec2::new(24.0, 34.0));
        app.update();
        assert_eq!(
            app.world().entity(box_id).get::<Node>().unwrap().display,
            Display::None
        );
        app.world_mut().resource_mut::<Workshop>().drag_current = Some(Vec2::new(800.0, 600.0));
        app.update();
        assert_eq!(
            app.world().entity(box_id).get::<Node>().unwrap().display,
            Display::Flex
        );
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        app.update();
        assert_eq!(
            app.world().entity(box_id).get::<Node>().unwrap().display,
            Display::None
        );
    }
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
    fn rebuild_pending(app: &mut App) {
        let (config, layout) = app
            .world_mut()
            .resource_mut::<Workshop>()
            .pending
            .take()
            .expect("UI action queued a rebuild");
        app.world_mut().resource_mut::<Scene>().sim =
            BlastRange::with_layout(config, layout).unwrap();
        app.update();
    }
    #[test]
    fn free_range_entry_is_visible_in_solo_and_combat_controls_stay_separate() {
        let mut app = app();
        app.add_systems(Startup, |mut commands: Commands| {
            let camera = commands.spawn_empty().id();
            build(&mut commands, camera, &TextFont::default());
        });
        app.add_systems(PostUpdate, refresh);
        click(&mut app, Action::RangeMode);
        rebuild_pending(&mut app);
        let visible = |app: &mut App, action| {
            let mut query = app.world_mut().query::<(&Action, &Node)>();
            query
                .iter(app.world())
                .find(|(a, _)| **a == action)
                .unwrap()
                .1
                .display
                != Display::None
        };
        assert!(visible(&mut app, Action::FreeRange));
        assert!(!visible(&mut app, Action::BattleStart));
        assert!(!visible(&mut app, Action::FreeControl));
        assert!(!visible(&mut app, Action::BattleReplay));
        let mut bars = app
            .world_mut()
            .query_filtered::<&Node, With<BattleControls>>();
        let bar = bars.single(app.world()).unwrap();
        assert_ne!(bar.display, Display::None);
        assert_eq!(bar.width, px(230));
        click(&mut app, Action::FreeRange);
        rebuild_pending(&mut app);
        assert!(!visible(&mut app, Action::FreeRange));
        assert!(visible(&mut app, Action::BattleStart));
        assert!(!visible(&mut app, Action::Fire));
        assert!(!visible(&mut app, Action::AutoAttack));
        click(&mut app, Action::BattleStart);
        assert!(!visible(&mut app, Action::BattleStart));
        assert!(visible(&mut app, Action::BattleReplay));
        assert!(visible(&mut app, Action::Fire));
        assert!(visible(&mut app, Action::AutoAttack));
    }

    #[test]
    fn free_range_button_enters_control_from_solo_without_a_second_click() {
        let mut app = app();
        click(&mut app, Action::RangeMode);
        rebuild_pending(&mut app);
        assert!(!app.world().resource::<Scene>().sim.battle_mode());
        let map = app
            .world()
            .resource::<Scene>()
            .sim
            .layout
            .as_ref()
            .unwrap()
            .blocks
            .clone();
        click(&mut app, Action::FreeRange);
        assert!(app.world().resource::<Panel>().free_launch);
        rebuild_pending(&mut app);
        let scene = app.world().resource::<Scene>();
        assert!(scene.sim.battle_mode());
        assert_eq!(
            scene.sim.battle.phase,
            sim_core::blast::battle::Phase::Setup
        );
        assert!(!scene.sim.battle.autonomous);
        assert!(!scene.paused);
        assert_eq!(scene.sim.layout.as_ref().unwrap().blocks, map);
        assert_eq!(scene.sim.battle_counts(), [2, 2]);
        let workshop = app.world().resource::<Workshop>();
        assert!(!workshop.editing);
        let controlled = workshop.controlled.unwrap();
        assert!(scene.sim.allied(controlled));
        assert_eq!(workshop.selected_units, vec![controlled]);
        assert!(!app.world().resource::<Panel>().free_launch);
        // A quiet Setup keeps the same roster and never starts combat by itself.
        for _ in 0..30 {
            app.world_mut().resource_mut::<Scene>().sim.step();
        }
        assert!(app
            .world()
            .resource::<Scene>()
            .sim
            .battle
            .actors
            .iter()
            .all(|a| a.fired == 0));
        click(&mut app, Action::BattleStart);
        assert_eq!(
            app.world().resource::<Scene>().sim.battle.phase,
            sim_core::blast::battle::Phase::Fighting
        );
    }

    #[test]
    fn switching_scenarios_cancels_a_pending_free_range_launch() {
        let mut app = app();
        click(&mut app, Action::FreeRange);
        assert!(app.world().resource::<Panel>().free_launch);
        click(&mut app, Action::BattleWorldAuto);
        assert!(!app.world().resource::<Panel>().free_launch);
        rebuild_pending(&mut app);
        let scene = app.world().resource::<Scene>();
        assert!(scene.sim.battle.autonomous);
        assert_eq!(
            scene.sim.battle.phase,
            sim_core::blast::battle::Phase::Fighting
        );
        click(&mut app, Action::FreeRange);
        click(&mut app, Action::RangeMode);
        rebuild_pending(&mut app);
        assert!(!app.world().resource::<Scene>().sim.battle_mode());
        assert!(!app.world().resource::<Panel>().free_launch);
    }

    #[test]
    fn free_control_then_start_and_switch_battle_from_buttons() {
        let mut app = app();
        click(&mut app, Action::FreeControl);
        let scene = app.world().resource::<Scene>();
        assert_eq!(
            scene.sim.battle.phase,
            sim_core::blast::battle::Phase::Setup
        );
        assert!(!scene.paused);
        assert!(!app.world().resource::<Workshop>().editing);
        assert!(app.world().resource::<Workshop>().controlled.is_some());
        click(&mut app, Action::Fire);
        assert!(!app.world().resource::<Panel>().ground_fire);
        click(&mut app, Action::BattleStart);
        assert_eq!(
            app.world().resource::<Scene>().sim.battle.phase,
            sim_core::blast::battle::Phase::Fighting
        );
        click(&mut app, Action::BattleChoose);
        assert_eq!(app.world().resource::<Panel>().drawer, 1);
        click(&mut app, Action::BattlePreset);
        rebuild_pending(&mut app);
        click(&mut app, Action::FreeControl);
        assert_eq!(
            app.world().resource::<Scene>().sim.battle.phase,
            sim_core::blast::battle::Phase::Setup
        );
        assert!(!app.world().resource::<Workshop>().editing);
    }
    #[test]
    fn a_started_battle_can_be_replayed_and_switched_from_the_hud() {
        let mut app = app();
        assert!(app
            .world_mut()
            .resource_mut::<Scene>()
            .sim
            .set_ai_vs_ai(true));
        click(&mut app, Action::BattleStart);
        app.world_mut().resource_mut::<Scene>().sim.units[0]
            .status
            .alive = false;
        click(&mut app, Action::BattleChoose);
        assert_eq!(app.world().resource::<Panel>().drawer, 1);
        assert!(app.world().resource::<Workshop>().pending.is_none());
        assert!(!app.world().resource::<Scene>().sim.units[0].status.alive);

        click(&mut app, Action::BattleReplay);
        assert_eq!(app.world().resource::<Panel>().drawer, 0);
        assert!(app.world().resource::<Panel>().battle_launch);
        assert!(app.world().resource::<Panel>().battle_auto);
        assert!(app.world().resource::<Scene>().paused);
        rebuild_pending(&mut app);
        assert!(!app.world().resource::<Panel>().battle_launch);
        assert!(!app.world().resource::<Scene>().paused);
        assert!(app.world().resource::<Scene>().sim.battle.autonomous);
        assert_eq!(app.world().resource::<Scene>().sim.battle_counts(), [2, 2]);
        assert_eq!(
            app.world().resource::<Scene>().sim.battle.phase,
            sim_core::blast::battle::Phase::Fighting
        );

        click(&mut app, Action::BattleChoose);
        click(&mut app, Action::Skirmish(1));
        assert_eq!(
            app.world().resource::<Panel>().battle_name,
            "Встреча в окружении"
        );
        rebuild_pending(&mut app);
        assert_eq!(app.world().resource::<Scene>().sim.battle_counts(), [3, 4]);
        assert!(!app.world().resource::<Scene>().sim.battle.autonomous);
        assert_eq!(
            app.world().resource::<Scene>().sim.battle.phase,
            sim_core::blast::battle::Phase::Fighting
        );
    }
    #[test]
    fn hud_selects_tank_ammunition_and_issues_orders() {
        let mut app = app();
        {
            let mut panel = app.world_mut().resource_mut::<Panel>();
            panel.briefing = "Тактическая сводка".into();
            panel.drawer = 1;
        }
        click(&mut app, Action::BattleStart);
        assert!(app.world().resource::<Panel>().briefing.is_empty());
        assert_eq!(app.world().resource::<Panel>().drawer, 0);
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
    fn projectile_markers_toggle_without_changing_battle_or_selection() {
        let mut app = app();
        click(&mut app, Action::BattleStart);
        click(&mut app, Action::SelectUnit(0));
        let hash = app.world().resource::<Scene>().sim.state_hash();
        let controlled = app.world().resource::<Workshop>().controlled;
        assert_eq!(section(false, Action::ProjectileMarkers), 3);
        assert!(app.world().resource::<Panel>().projectile_markers);
        for enabled in [false, true] {
            click(&mut app, Action::ProjectileMarkers);
            assert_eq!(app.world().resource::<Panel>().projectile_markers, enabled);
            assert_eq!(app.world().resource::<Scene>().sim.state_hash(), hash);
            assert_eq!(app.world().resource::<Workshop>().controlled, controlled);
            assert!(app.world().resource::<Workshop>().pending.is_none());
        }
    }
    #[test]
    fn group_selection_and_commands_apply_to_every_selected_ally() {
        use sim_core::blast::battle::Order;
        let mut app = app();
        click(&mut app, Action::BattleStart);
        click(&mut app, Action::SelectAllOwn);
        assert_eq!(app.world().resource::<Workshop>().selected_units, [0, 1]);
        click(&mut app, Action::Stop);
        assert!(app.world().resource::<Scene>().sim.battle.actors[..2]
            .iter()
            .all(|actor| actor.order == Order::Hold));
        click(&mut app, Action::AutoAttack);
        assert!(app.world().resource::<Scene>().sim.battle.actors[..2]
            .iter()
            .all(|actor| actor.order == Order::Auto));
        click(&mut app, Action::BattleWeapon(Some(BattleWeapon::Energy)));
        assert_eq!(
            app.world().resource::<Scene>().sim.battle_weapon(0),
            Some(BattleWeapon::Energy)
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ShiftLeft);
        click(&mut app, Action::SelectUnit(0));
        assert_eq!(app.world().resource::<Workshop>().selected_units, [1]);
        *app.world_mut().resource_mut::<ButtonInput<KeyCode>>() = default();
        click(&mut app, Action::SelectAllOwn);
        let mut scene = app.world_mut().resource_mut::<Scene>();
        assert_eq!(
            workbench::issue_battle_group_order(&mut scene.sim, &[0, 1], Order::Attack(2), None),
            2
        );
        assert!(scene.sim.battle.actors[..2]
            .iter()
            .all(|actor| actor.order == Order::Attack(2)));
        assert_eq!(
            workbench::issue_battle_group_order(
                &mut scene.sim,
                &[0, 1],
                Order::Move,
                Some([3530, 2400])
            ),
            2
        );
        assert_ne!(
            scene.sim.units[0].motion.destination,
            scene.sim.units[1].motion.destination
        );
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
            assert!(app.world().resource::<Panel>().briefing.is_empty());
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

    #[test]
    fn world_battle_button_loads_bases_and_starts_both_armies() {
        let mut app = app();
        click(&mut app, Action::BattleWorld);
        assert_eq!(app.world().resource::<Scene>().view, 1);
        assert!(app.world().resource::<Panel>().battle_launch);
        assert!(app.world().resource::<Panel>().briefing.contains("баз"));
        let (config, layout) = app
            .world_mut()
            .resource_mut::<Workshop>()
            .pending
            .take()
            .expect("world battle rebuild");
        assert_eq!(layout.units.len(), 60);
        let block_count = layout.blocks.len();
        app.world_mut().resource_mut::<Scene>().sim =
            BlastRange::with_layout(config, layout).unwrap();
        app.update();
        let scene = app.world().resource::<Scene>();
        assert_eq!(scene.sim.battle_counts(), [30, 30]);
        assert_eq!(scene.sim.battle_base_integrity(), Some([100, 100]));
        assert_eq!(
            scene.sim.battle.phase,
            sim_core::blast::battle::Phase::Fighting
        );
        for (id, weapon) in [
            (4, BattleWeapon::Annihilation),
            (17, BattleWeapon::Energy),
            (16, BattleWeapon::Canister),
            (10, BattleWeapon::Annihilation),
            (41, BattleWeapon::Energy),
            (40, BattleWeapon::Canister),
        ] {
            assert_eq!(scene.sim.battle_weapon(id), Some(weapon));
        }
        assert!(!app.world().resource::<Workshop>().editing);
        assert_eq!(app.world().resource::<Workshop>().controlled, Some(1));
        assert!(!app.world().resource::<Panel>().battle_launch);
        click(&mut app, Action::RosterPage(1));
        assert_eq!(app.world().resource::<Panel>().roster_page, 1);
        click(&mut app, Action::SelectUnit(21));
        click(&mut app, Action::NextOwn);
        assert_eq!(app.world().resource::<Workshop>().controlled, Some(22));
        assert_eq!(app.world().resource::<Panel>().roster_page, 2);
        click(&mut app, Action::SelectUnit(1));
        let mut scene = app.world_mut().resource_mut::<Scene>();
        assert!(scene
            .sim
            .battle_order(1, sim_core::blast::battle::Order::Attack(6), None));
        scene.sim.step();
        assert_eq!(scene.sim.battle.actors[1].target, Some(6));
        assert!(scene.sim.units[1].motion.destination.is_some());
        drop(scene);
        click(&mut app, Action::SelectUnit(4));
        click(&mut app, Action::BattleWeapon(Some(BattleWeapon::Kinetic)));
        assert_eq!(
            app.world().resource::<Scene>().sim.battle_weapon(4),
            Some(BattleWeapon::Kinetic)
        );
        click(&mut app, Action::BattleWorldAuto);
        let (config, layout) = app
            .world_mut()
            .resource_mut::<Workshop>()
            .pending
            .take()
            .expect("world battle reset rebuild");
        assert_eq!(layout.units.len(), 60);
        assert_eq!(layout.blocks.len(), block_count);
        app.world_mut().resource_mut::<Scene>().sim =
            BlastRange::with_layout(config, layout).unwrap();
        app.update();
        let scene = app.world().resource::<Scene>();
        assert_eq!(scene.sim.battle_counts(), [30, 30]);
        assert_eq!(scene.sim.battle_base_integrity(), Some([100, 100]));
        assert_eq!(
            scene.sim.battle.phase,
            sim_core::blast::battle::Phase::Fighting
        );
        assert_eq!(scene.sim.battle.actors[1].target, None);
        assert!(scene.sim.battle.autonomous);
        assert_eq!(scene.sim.battle_weapon(4), Some(BattleWeapon::Annihilation));
        assert_eq!(scene.sim.battle_weapon(41), Some(BattleWeapon::Energy));
        assert_eq!(scene.sim.battle_weapon(16), Some(BattleWeapon::Canister));
    }

    #[test]
    fn range_mode_cancels_a_pending_world_battle_launch() {
        let mut app = app();
        let original =
            RangeLayout::from_ron(include_str!("../../../../../../data/range-km.ron")).unwrap();
        click(&mut app, Action::BattleWorld);
        assert!(app.world().resource::<Panel>().battle_launch);
        click(&mut app, Action::RangeMode);
        assert!(!app.world().resource::<Panel>().battle_launch);
        assert!(!app.world().resource::<Panel>().battle_auto);
        assert_eq!(app.world().resource::<Panel>().drawer, 0);
        assert!(app.world().resource::<Workshop>().controlled.is_none());
        rebuild_pending(&mut app);
        let scene = app.world().resource::<Scene>();
        assert_eq!(scene.sim.layout.as_ref(), Some(&original));
        assert!(scene.notice.is_empty(), "{}", scene.notice);
        assert!(app.world().resource::<Workshop>().pending.is_none());
    }
    #[test]
    fn editor_buttons_explain_missing_selection_and_reach_spider_tool() {
        let mut app = app();
        app.world_mut().resource_mut::<Workshop>().editing = true;
        click(&mut app, Action::Tool(Tool::Spider));
        assert_eq!(app.world().resource::<Workshop>().tool, Tool::Spider);
        click(&mut app, Action::Delete);
        assert!(app
            .world()
            .resource::<Scene>()
            .notice
            .contains("выберите объект"));
        click(&mut app, Action::AssignRoute);
        assert!(app
            .world()
            .resource::<Scene>()
            .notice
            .contains("выберите юнит"));
        click(&mut app, Action::Undo);
        assert!(app
            .world()
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::KeyZ));
    }
}
