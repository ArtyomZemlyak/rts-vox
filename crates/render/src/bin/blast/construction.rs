//! One catalogue for sandbox placement, engineering and real production queues.
use super::*;
use modding::laboratory::catalog::{Category, Element};
use panel::theme;
use sim_core::blast::battle::{commands::Command, Phase};
use std::collections::BTreeMap;
use workbench::ghost::Templates;
use workbench::Selection;

const SLOTS: usize = 16;
#[cfg(test)]
#[path = "construction/tests.rs"]
mod tests;
pub const HEIGHT: f32 = 272.0;
/// Both the button and keyboard use the same permission; opening the editor
/// rebuilds a layout and must never reset damage in an ongoing standard battle.
pub fn editing_allowed(sim: &BlastRange, panel: Option<&panel::Panel>) -> bool {
    !sim.battle_mode() || sim.battle.phase == Phase::Setup || panel.is_some_and(|p| p.sandbox)
}
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum Context {
    #[default]
    Hidden,
    Sandbox,
    Engineer(usize),
    Factory(usize),
}
#[derive(Resource, Default)]
pub struct Catalog {
    context: Context,
    generation: u64,
    tier: u8,
    category: u8,
    page: usize,
    per_page: usize,
    pending: Option<RangeUnitKind>,
    allied: bool,
    yaw: u8,
    pub hover: Option<([i32; 3], bool)>,
    hover_tick: Option<u32>,
    icons: BTreeMap<usize, Handle<Image>>,
    costs: BTreeMap<usize, u64>,
    capture_initialized: bool,
    image_ready_frames: u8,
}
impl Catalog {
    pub fn capture_ready(&self) -> bool {
        self.context == Context::Hidden || self.image_ready_frames >= 3
    }
    pub fn placing(&self) -> bool {
        self.pending.is_some() && self.context != Context::Hidden
    }
    pub fn ghost(&self, kind: RangeUnitKind) -> Option<(Transform, bool)> {
        if self.pending != Some(kind) {
            return None;
        }
        let (at, invalid) = self.hover?;
        Some((
            Transform::from_translation(Vec3::from(at.map(|v| v as f32))).with_rotation(
                Quat::from_rotation_y(-f32::from(self.yaw) * std::f32::consts::FRAC_PI_2),
            ),
            invalid,
        ))
    }
}
pub fn engineer_selected(sim: &BlastRange, edit: &Workshop) -> bool {
    edit.controlled.is_some_and(|id| {
        sim.units.get(id).is_some_and(|u| {
            sim.allied(id) && u.status.alive && u.kind == RangeUnitKind::SkobaDrone
        })
    })
}
pub(super) fn context(sim: &BlastRange, panel: &panel::Panel, edit: &Workshop) -> Context {
    if edit.layout.is_none() || edit.pending.is_some() {
        return Context::Hidden;
    }
    if panel.sandbox || (edit.editing && sim.battle.phase == Phase::Setup) {
        return Context::Sandbox;
    }
    if edit.editing || !sim.battle_mode() || sim.battle.phase != Phase::Fighting {
        return Context::Hidden;
    }
    let Some(id) = edit.controlled else {
        return Context::Hidden;
    };
    if sim.unit_under_construction(id) {
        return Context::Hidden;
    }
    let Some(u) = sim
        .units
        .get(id)
        .filter(|u| u.status.alive && sim.allied(id))
    else {
        return Context::Hidden;
    };
    if u.kind == RangeUnitKind::SkobaDrone {
        return Context::Engineer(id);
    }
    if sim.mining().is_some_and(|m| {
        m.data().production.as_ref().is_some_and(|p| {
            p.producers
                .iter()
                .any(|p| p.kind == u.kind && !p.produces.is_empty())
        })
    }) {
        return Context::Factory(id);
    }
    Context::Hidden
}
pub fn tier(kind: RangeUnitKind) -> u8 {
    use RangeUnitKind::*;
    match kind {
        Strider | TurretT2 | ArtilleryT2 | ArtilleryTurretT2 | LightningTank | KilFactoryT2 => 2,
        Spider | Walker04 | ArtilleryTurretT3 | NuclearSilo | RazlomT3 | KilFactoryT3 => 3,
        KilFactoryT4 => 4,
        _ => 1,
    }
}
pub fn name(kind: RangeUnitKind) -> &'static str {
    use RangeUnitKind::*;
    match kind {
        Tank => "Танк",
        Strider => "Шагоход",
        Spider => "Паук",
        Artillery => "Артиллерия",
        TurretT1 => "Турель",
        TurretT2 => "Энерготурель",
        Base => "Командная база",
        ArtilleryT2 => "Картечная артиллерия",
        ArtilleryTurretT2 => "Артиллерийская башня",
        ArtilleryTurretT3 => "Осадная башня",
        NuclearSilo => "Ядерная шахта",
        Crawler => "Краулер",
        LightningTank => "Электротанк",
        EidolonTank => "Скат",
        Walker04 => "Волкер",
        RazlomT3 => "Разлом",
        GrebenStorage => "Гребень · склад",
        SkobaDrone => "Скоба · инженер",
        KilFactoryT1 | KilFactoryT2 | KilFactoryT3 | KilFactoryT4 => "Киль · завод",
    }
}
fn products(sim: &BlastRange, actor: usize) -> Vec<RangeUnitKind> {
    let Some(u) = sim.units.get(actor) else {
        return vec![];
    };
    sim.mining()
        .and_then(|m| m.data().production.as_ref())
        .and_then(|p| p.producers.iter().find(|p| p.kind == u.kind))
        .map_or_else(Vec::new, |p| p.produces.clone())
}
fn choices(c: &Catalog, sim: &BlastRange) -> Vec<RangeUnitKind> {
    let kinds = match c.context {
        Context::Hidden => vec![],
        Context::Sandbox => RangeUnitKind::ALL.to_vec(),
        Context::Engineer(_) => RangeUnitKind::ALL
            .into_iter()
            .filter(|k| k.emplacement())
            .collect(),
        Context::Factory(id) => products(sim, id),
    };
    kinds
        .into_iter()
        .filter(|k| {
            (c.tier == 0 || tier(*k) == c.tier)
                && (c.category == 0 || (*k).emplacement() == (c.category == 2))
        })
        .collect()
}
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Tier(u8),
    Category(u8),
    Page(i32),
    Choose(usize),
    Side(bool),
    Cancel,
    Rotate,
    Upgrade,
    Focus(bool),
}
#[derive(Component)]
pub(super) struct Root;
#[derive(Component)]
pub(super) struct Slot(usize);
#[derive(Component)]
pub(super) struct Picture(usize);
#[derive(Component)]
pub(super) enum Caption {
    Heading,
    Hint,
    Item(usize),
}
#[derive(Component)]
pub(super) struct SandboxControl(bool);

pub fn input(
    mut c: ResMut<Catalog>,
    mut scene: ResMut<Scene>,
    mut panel: ResMut<panel::Panel>,
    mut edit: ResMut<Workshop>,
    lab: Res<laboratory::Laboratory>,
    pilot: Res<pilot::Pilot>,
    buttons: Query<(&Action, &Interaction), Changed<Interaction>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<GasCamera>>,
    roots: Query<&bevy::ui::RelativeCursorPosition, With<panel::PanelRoot>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    streaming: Option<Res<streaming::Streaming>>,
) {
    if !lab.active() && !pilot.active() {
        for (action, interaction) in &buttons {
            if let (Action::Focus(engineer), Interaction::Pressed) = (*action, *interaction) {
                if let Some(id) = scene
                    .sim
                    .units
                    .iter()
                    .enumerate()
                    .find(|(id, u)| {
                        scene.sim.allied(*id)
                            && u.status.alive
                            && if engineer {
                                u.kind == RangeUnitKind::SkobaDrone
                            } else {
                                matches!(
                                    u.kind,
                                    RangeUnitKind::KilFactoryT1
                                        | RangeUnitKind::KilFactoryT2
                                        | RangeUnitKind::KilFactoryT3
                                        | RangeUnitKind::KilFactoryT4
                                )
                            }
                    })
                    .map(|(id, _)| id)
                {
                    edit.select_battle_unit(&scene.sim, id, false);
                } else {
                    scene.notice = if engineer {
                        "Нет доступных инженеров"
                    } else {
                        "Нет доступных заводов"
                    }
                    .into();
                }
            }
        }
    }
    if scene.out.is_some() && !c.capture_initialized {
        c.capture_initialized = true;
        let case = std::env::var("RTS_BUILD_CAPTURE_CASE").unwrap_or_default();
        let want = match case.as_str() {
            "factory" => Some(RangeUnitKind::KilFactoryT1),
            "engineer" => Some(RangeUnitKind::SkobaDrone),
            _ => None,
        };
        if let Some(kind) = want {
            if let Some(actor) = scene
                .sim
                .units
                .iter()
                .enumerate()
                .find(|(i, u)| u.kind == kind && scene.sim.allied(*i))
                .map(|(i, _)| i)
            {
                if scene.sim.battle.phase == Phase::Setup {
                    let _ = scene.sim.start_battle();
                }
                panel.sandbox = false;
                edit.editing = false;
                edit.controlled = Some(actor);
                edit.selected_units = vec![actor];
            }
        }
    }
    let next = if lab.active() || pilot.active() {
        Context::Hidden
    } else {
        context(&scene.sim, &panel, &edit)
    };
    if next != c.context || c.generation != scene.generation {
        c.context = next;
        c.generation = scene.generation;
        c.pending = None;
        c.hover = None;
        c.hover_tick = None;
        c.tier = 0;
        c.category = 0;
        c.page = 0;
        c.yaw = 0;
        c.allied = true;
        panel.construction_kind = None;
        c.costs.clear();
        // A card may still depict the previously selected producer this frame.
        // Never interpret that old click using a different actor's recipes.
        return;
    }
    if next == Context::Hidden {
        return;
    }
    let mut selected = false;
    for (action, interaction) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if !matches!(action, Action::Focus(_)) {
            scene.notice.clear();
        }
        match *action {
            Action::Tier(t) => {
                c.tier = t;
                c.page = 0;
            }
            Action::Category(t) => {
                c.category = t;
                c.page = 0;
            }
            Action::Page(delta) => {
                c.page = (c.page as i32 + delta).max(0) as usize;
            }
            Action::Side(allied) if next == Context::Sandbox => {
                c.allied = allied;
                c.hover = None;
            }
            Action::Rotate if next == Context::Sandbox => {
                c.yaw = (c.yaw + 1) % 4;
                c.hover = None;
            }
            Action::Upgrade => {
                if let Context::Factory(actor) = next {
                    scene.notice = if scene.sim.upgrade_kil(actor) {
                        "Завод улучшен, очередь сохранена"
                    } else {
                        "Улучшение недоступно: проверьте материал, тир и состояние завода"
                    }
                    .into();
                }
            }
            Action::Cancel => {
                if let Context::Factory(actor) = next {
                    let outcome = scene.sim.execute_command(Command::Cancel { actor });
                    scene.notice = if outcome.accepted() {
                        "Последний заказ отменён"
                    } else {
                        "Очередь пуста"
                    }
                    .into();
                } else {
                    c.pending = None;
                    c.hover = None;
                    panel.construction_kind = None;
                }
            }
            Action::Choose(slot) => {
                let list = choices(&c, &scene.sim);
                let Some(kind) = list.get(c.page * c.per_page.max(1) + slot).copied() else {
                    continue;
                };
                if let Context::Factory(actor) = next {
                    let outcome = scene.sim.execute_command(Command::Produce { actor, kind });
                    scene.notice = if outcome.accepted() {
                        format!("{} добавлен в очередь", name(kind))
                    } else {
                        "Заказ не принят: проверьте очередь и состояние завода".into()
                    };
                } else {
                    c.pending = Some(kind);
                    c.hover = None;
                    c.yaw = 0;
                    selected = true;
                    panel.ground_fire = false;
                    panel.construction_kind = None;
                    edit.drag_start = None;
                    edit.drag_current = None;
                }
            }
            _ => {}
        }
    }
    if c.pending.is_none() {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) || mouse.just_pressed(MouseButton::Right) {
        c.pending = None;
        c.hover = None;
        panel.construction_kind = None;
        keys.reset(KeyCode::Escape);
        mouse.reset(MouseButton::Right);
        return;
    }
    if next == Context::Sandbox {
        for (key, step) in [(KeyCode::KeyQ, 3), (KeyCode::KeyE, 1)] {
            if keys.just_pressed(key) {
                c.yaw = (c.yaw + step) % 4;
                c.hover = None;
                keys.reset(key);
            }
        }
    }
    if selected || roots.iter().any(|r| r.cursor_over()) {
        c.hover = None;
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    if !window.focused || streaming.as_ref().is_some_and(|s| s.loading()) {
        c.hover = None;
        return;
    }
    let Some(cursor) = window.cursor_position() else {
        c.hover = None;
        return;
    };
    let Ok((camera, transform)) = cameras.single() else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(transform, cursor) else {
        return;
    };
    if ray.direction.y.abs() < 0.001 {
        c.hover = None;
        return;
    }
    let distance = (scene.sim.ground_level() as f32 - ray.origin.y) / ray.direction.y;
    if distance < 0.0 {
        c.hover = None;
        return;
    }
    let p = ray.origin + ray.direction * distance;
    let x = (p.x / 4.0).round() as i32 * 4;
    let z = (p.z / 4.0).round() as i32 * 4;
    let at = [x, sim_core::blast::motion::floor(&scene.sim.world, x, z), z];
    let kind = c.pending.expect("placement");
    if c.hover.is_none_or(|(old, _)| old != at)
        || c.hover_tick != Some(scene.sim.tick)
        || mouse.just_pressed(MouseButton::Left)
    {
        let issue = if next == Context::Sandbox {
            edit.validator.runtime(&scene.sim, kind, at, c.yaw).is_err()
        } else {
            !scene.sim.can_construct(kind, true, at)
        };
        c.hover = Some((at, issue));
        c.hover_tick = Some(scene.sim.tick);
    }
    if mouse.just_pressed(MouseButton::Left) {
        let result = if c.hover.is_some_and(|(_, invalid)| invalid) {
            Err("Здесь разместить нельзя: препятствие или край карты")
        } else if next == Context::Sandbox {
            scene
                .sim
                .place_sandbox_unit(kind, c.allied, at, c.yaw)
                .map(|id| {
                    edit.layout = scene.sim.layout.clone();
                    edit.selected = Some(Selection::Unit(id));
                    if c.allied {
                        edit.controlled = Some(id);
                    }
                })
        } else if engineer_selected(&scene.sim, &edit) {
            scene
                .sim
                .create_construction_site(kind, true, at)
                .map(|_| ())
                .ok_or("Стройплощадка недоступна или место уже занято")
        } else {
            Err("Выберите живого союзного инженера")
        };
        match result {
            Ok(()) => {
                scene.notice = if next == Context::Sandbox {
                    format!("Размещён {} · без сброса симуляции", name(kind))
                } else {
                    "Стройплощадка создана. Дроны доставляют материал со склада".into()
                };
                if !keys.pressed(KeyCode::ShiftLeft) && !keys.pressed(KeyCode::ShiftRight) {
                    c.pending = None;
                }
                c.hover = None;
            }
            Err(e) => scene.notice = e.into(),
        }
        mouse.reset(MouseButton::Left);
    }
}

fn label(p: &mut ChildSpawnerCommands, font: &TextFont, text: &str, size: f32) -> Entity {
    p.spawn((
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(size),
            ..font.clone()
        },
        TextColor(theme::TEXT),
    ))
    .id()
}
fn button(p: &mut ChildSpawnerCommands, font: &TextFont, text: &str, action: Action) -> Entity {
    let mut e = p.spawn((
        Button,
        action,
        Node {
            padding: UiRect::axes(px(10), px(6)),
            align_items: AlignItems::Center,
            border: UiRect::all(px(1)),
            ..default()
        },
        BackgroundColor(theme::BUTTON),
        BorderColor::all(theme::FRAME),
    ));
    e.with_children(|p| {
        label(p, font, text, 14.0);
    });
    e.id()
}
pub fn shortcuts(p: &mut ChildSpawnerCommands, font: &TextFont) {
    button(p, font, "Инженер", Action::Focus(true));
    button(p, font, "Завод", Action::Focus(false));
}
pub fn build(commands: &mut Commands, camera: Entity, font: &TextFont) {
    commands
        .spawn((
            Root,
            panel::PanelRoot,
            bevy::ui::RelativeCursorPosition::default(),
            UiTargetCamera(camera),
            GlobalZIndex(10),
            Node {
                position_type: PositionType::Absolute,
                left: px(16),
                right: px(16),
                bottom: px(16),
                height: px(HEIGHT),
                padding: UiRect::all(px(12)),
                row_gap: px(8),
                flex_direction: FlexDirection::Column,
                border: UiRect::top(px(2)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(theme::PANEL),
            BorderColor::all(theme::FRAME),
        ))
        .with_children(|p| {
            p.spawn(Node {
                column_gap: px(8),
                row_gap: px(6),
                flex_wrap: FlexWrap::Wrap,
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|p| {
                label(p, font, "СТРОИТЕЛЬСТВО", 15.0);
                for (t, n) in [(0, "Все тиры"), (1, "T1"), (2, "T2"), (3, "T3"), (4, "T4")] {
                    button(p, font, n, Action::Tier(t));
                }
                for (t, n) in [(0, "Всё"), (1, "Юниты"), (2, "Постройки")] {
                    button(p, font, n, Action::Category(t));
                }
                for (side, n) in [(true, "Свои"), (false, "Враги")] {
                    let id = button(p, font, n, Action::Side(side));
                    p.commands().entity(id).insert(SandboxControl(true));
                }
                let id = button(p, font, "Повернуть · Q/E", Action::Rotate);
                p.commands().entity(id).insert(SandboxControl(true));
                let id = button(p, font, "Улучшить завод", Action::Upgrade);
                p.commands().entity(id).insert(SandboxControl(false));
                button(p, font, "Отмена", Action::Cancel);
                button(p, font, "←", Action::Page(-1));
                button(p, font, "→", Action::Page(1));
            });
            let id = label(p, font, "", 14.0);
            p.commands().entity(id).insert(Caption::Heading);
            p.spawn(Node {
                column_gap: px(10),
                width: percent(100),
                height: px(144),
                flex_grow: 0.0,
                flex_shrink: 0.0,
                ..default()
            })
            .with_children(|p| {
                for slot in 0..SLOTS {
                    p.spawn((
                        Button,
                        Action::Choose(slot),
                        Slot(slot),
                        Node {
                            flex_grow: 0.0,
                            flex_shrink: 0.0,
                            flex_basis: px(180),
                            min_width: px(180),
                            height: percent(100),
                            min_height: px(0),
                            flex_direction: FlexDirection::Column,
                            padding: UiRect::all(px(6)),
                            border: UiRect::all(px(1)),
                            align_items: AlignItems::Center,
                            row_gap: px(4),
                            ..default()
                        },
                        BackgroundColor(theme::BUTTON),
                        BorderColor::all(theme::FRAME),
                    ))
                    .with_children(|p| {
                        p.spawn((
                            Picture(slot),
                            ImageNode::default(),
                            Node {
                                height: px(72),
                                width: px(104),
                                ..default()
                            },
                        ));
                        let id = label(p, font, "", 14.0);
                        p.commands().entity(id).insert(Caption::Item(slot));
                    });
                }
            });
            let id = label(p, font, "", 13.0);
            p.commands().entity(id).insert(Caption::Hint);
        });
}

pub fn refresh(
    mut c: ResMut<Catalog>,
    scene: Res<Scene>,
    panel: Res<panel::Panel>,
    edit: Res<Workshop>,
    lab: Res<laboratory::Laboratory>,
    pilot: Res<pilot::Pilot>,
    menu: Res<frontend::Menu>,
    templates: Res<Templates>,
    mut images: ResMut<Assets<Image>>,
    (windows, mut legacy): (
        Query<&Window>,
        Query<
            (
                &mut Node,
                Option<&panel::hud::CommandDeck>,
                Option<&panel::hud::RosterStrip>,
                Option<&panel::minimap::MiniRoot>,
                Option<&panel::hud::BriefingCard>,
            ),
            (
                Or<(
                    With<panel::hud::CommandDeck>,
                    With<panel::hud::RosterStrip>,
                    With<panel::minimap::MiniRoot>,
                    With<panel::hud::BriefingCard>,
                )>,
                Without<Root>,
                Without<Slot>,
                Without<SandboxControl>,
            ),
        >,
    ),
    mut root: Query<&mut Node, (With<Root>, Without<Slot>, Without<SandboxControl>)>,
    mut cards: Query<(
        &Slot,
        &mut Node,
        &mut BackgroundColor,
        &mut BorderColor,
        &Interaction,
    )>,
    mut controls: Query<(&Action, &Interaction, &mut BackgroundColor), Without<Slot>>,
    mut sides: Query<(&SandboxControl, &mut Node), (Without<Slot>, Without<Root>)>,
    mut text: Query<(&Caption, &mut Text)>,
    mut pictures: Query<(&Picture, &mut ImageNode)>,
) {
    let context = if lab.active() || pilot.active() || menu.shown() {
        Context::Hidden
    } else {
        context(&scene.sim, &panel, &edit)
    };
    let compact = windows.single().is_ok_and(|w| w.width() < 1280.0);
    for mut root in &mut root {
        root.display = if context == Context::Hidden {
            Display::None
        } else {
            Display::Flex
        };
        root.height = px(if compact { HEIGHT + 52.0 } else { HEIGHT });
    }
    let height = if compact { HEIGHT + 52.0 } else { HEIGHT };
    for (mut node, deck, strip, map, briefing) in &mut legacy {
        if deck.is_some() {
            node.display = if context == Context::Hidden && !lab.active() && !pilot.active() {
                Display::Flex
            } else {
                Display::None
            };
        }
        if strip.is_some() && context != Context::Hidden {
            node.bottom = px(height + 28.0);
        }
        // On a short window the mission card would be half hidden beneath
        // the catalogue/roster. The Summary drawer remains accessible.
        // HUD refresh restores normal visibility when the catalogue closes.
        if briefing.is_some()
            && context != Context::Hidden
            && windows.single().is_ok_and(|w| w.height() < 760.0)
        {
            node.display = Display::None;
        }
        if map.is_some() {
            node.display = if context != Context::Hidden
                && windows.single().is_ok_and(|w| w.height() < 760.0)
            {
                Display::None
            } else {
                Display::Flex
            };
            node.bottom = px(if context != Context::Hidden {
                height + 32.0
            } else {
                234.0
            });
        }
    }
    if context == Context::Hidden || context != c.context {
        return;
    }
    c.per_page = windows
        .single()
        .map_or(8, |w| ((w.width() - 64.0) / 190.0) as usize)
        .clamp(3, SLOTS);
    let list = choices(&c, &scene.sim);
    let pages = list.len().div_ceil(c.per_page).max(1);
    c.page = c.page.min(pages - 1);
    let page: Vec<_> = list
        .into_iter()
        .skip(c.page * c.per_page)
        .take(c.per_page)
        .collect();
    if page.iter().all(|k| c.icons.contains_key(&k.index())) {
        c.image_ready_frames = c.image_ready_frames.saturating_add(1);
    } else {
        c.image_ready_frames = 0;
    }
    // Share the laboratory's actual template thumbnail renderer. One new image
    // per frame, cached by kind; no extra world or offscreen simulation.
    if let Some(kind) = page
        .iter()
        .find(|k| !c.icons.contains_key(&k.index()))
        .copied()
    {
        let element = Element {
            id: format!("build-{kind:?}"),
            name: name(kind).into(),
            category: Category::Unit,
            blocks: vec![],
            units: vec![modding::range::RangeUnitDef {
                kind,
                allied: true,
                at: [0; 3],
                weapon: None,
            }],
            unit_yaws: vec![],
            shields: vec![],
        };
        let icon = images.add(laboratory::visuals::thumbnail(&element, &templates));
        c.icons.insert(kind.index(), icon);
    }
    for (Slot(slot), mut node, mut background, mut border, interaction) in &mut cards {
        node.display = if *slot < page.len() {
            Display::Flex
        } else {
            Display::None
        };
        let active = page.get(*slot).is_some_and(|k| Some(*k) == c.pending);
        background.0 = theme::button_background(*interaction, active);
        *border = BorderColor::all(if active { theme::ACCENT } else { theme::FRAME });
    }
    for (action, interaction, mut background) in &mut controls {
        let active = match *action {
            Action::Tier(t) => t == c.tier,
            Action::Category(t) => t == c.category,
            Action::Side(s) => s == c.allied,
            _ => false,
        };
        background.0 = theme::button_background(*interaction, active);
    }
    for (scope, mut node) in &mut sides {
        node.display = if (scope.0 && context == Context::Sandbox)
            || (!scope.0
                && matches!(context, Context::Factory(id) if matches!(scene.sim.units[id].kind,
                RangeUnitKind::KilFactoryT1 | RangeUnitKind::KilFactoryT2 | RangeUnitKind::KilFactoryT3)))
        {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (Picture(slot), mut image) in &mut pictures {
        image.image = page
            .get(*slot)
            .and_then(|k| c.icons.get(&k.index()))
            .cloned()
            .unwrap_or_default();
    }
    if matches!(context, Context::Engineer(_)) {
        for &kind in &page {
            c.costs
                .entry(kind.index())
                .or_insert_with(|| scene.sim.construction_cost(kind).unwrap_or(0));
        }
    }
    for (caption, mut text) in &mut text {
        text.0 = match *caption {
            Caption::Heading => match context {
                Context::Sandbox => format!(
                    "ПОЛИГОН · свободное размещение · страница {}/{}",
                    c.page + 1,
                    pages
                ),
                Context::Engineer(_) => {
                    let sites = scene.sim.construction_sites();
                    let active = sites.iter().filter(|s| !s.complete).count();
                    format!(
                        "ИНЖЕНЕР · здания · стройки: {active} · страница {}/{}",
                        c.page + 1,
                        pages
                    )
                }
                Context::Factory(actor) => {
                    let queue = scene.sim.production_queue(actor).unwrap_or_default();
                    format!(
                        "ПРОИЗВОДСТВО · {} T{} · очередь: {} · {}{}",
                        name(scene.sim.units[actor].kind),
                        tier(scene.sim.units[actor].kind),
                        queue.len(),
                        queue.first().map_or("нет заказов", |k| name(*k)),
                        queue
                            .first()
                            .and_then(|k| scene.sim.production_cost(*k))
                            .map_or_else(String::new, |cost| format!(
                                " · оплачено {}%",
                                scene
                                    .sim
                                    .production_paid(actor)
                                    .unwrap_or(0)
                                    .saturating_mul(100)
                                    / cost.max(1)
                            ))
                    )
                }
                Context::Hidden => String::new(),
            },
            Caption::Item(slot) => page.get(slot).map_or(String::new(), |k| {
                let cost = if matches!(context, Context::Factory(_)) {
                    scene
                        .sim
                        .production_cost(*k)
                        .map(|v| format!(" · {v} мат."))
                        .unwrap_or_default()
                } else if matches!(context, Context::Engineer(_)) {
                    c.costs
                        .get(&k.index())
                        .map_or_else(String::new, |v| format!(" · {v} мат."))
                } else {
                    String::new()
                };
                format!("{} · T{}{cost}", name(*k), tier(*k))
            }),
            Caption::Hint => {
                if !scene.notice.is_empty() {
                    // The compact catalogue hides the briefing card. Keep
                    // confirmations and errors visible here instead.
                    let mut chars = scene.notice.chars();
                    let mut message: String = chars
                        .by_ref()
                        .take(150)
                        .map(|c| if c == '\n' { ' ' } else { c })
                        .collect();
                    if chars.next().is_some() {
                        message.push('…');
                    }
                    message
                } else if page.is_empty() {
                    "В этом тире и категории доступных объектов нет".into()
                } else if c.pending.is_some() {
                    "ЛКМ — разместить · Shift — несколько · ПКМ / Esc — отменить · зелёный: можно, красный: препятствие".into()
                } else if matches!(context, Context::Factory(_)) {
                    "Нажмите на юнит, чтобы добавить заказ. «Отмена» убирает последний заказ и возвращает материал.".into()
                } else {
                    "Выберите карточку. Полигон добавляет объект сразу; инженер строит за материал, доставляемый дронами.".into()
                }
            }
        };
        if matches!(caption, Caption::Heading) && context != Context::Sandbox {
            if let Some((held, cap)) = scene.sim.side_store(true) {
                text.0.push_str(&format!(" · материал: {held}/{cap}"));
            }
        }
    }
}
