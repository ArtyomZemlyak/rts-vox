//! One catalogue for sandbox placement, engineering and real production queues.
use super::*;
use modding::laboratory::catalog::{Category, Element};
use panel::theme;
use sim_core::blast::battle::{commands::Command, Phase};
use std::collections::BTreeMap;
use workbench::ghost::Templates;
use workbench::Selection;

const SLOTS: usize = 16;
const QUEUE_SLOTS: usize = 16;
const QUEUE_HEIGHT: f32 = 68.0;
#[path="construction/plans.rs"]
pub mod plans;
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
    queue_page: usize,
    per_page: usize,
    pending: Option<RangeUnitKind>,
    production_mode: bool,
    allied: bool,
    yaw: u8,
    pub hover: Option<([i32; 3], bool)>,
    hover_tick: Option<u32>,
    hover_issue: Option<String>,
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
            sim.allied(id) && u.status.alive && matches!(u.kind, RangeUnitKind::SkobaDrone | RangeUnitKind::Commander | RangeUnitKind::GrebenStorage)
        })
    })
}
/// Explicitly selected engineers build this site themselves. Selecting only
/// a warehouse keeps its automatic quota-based dispatch.
pub(super) fn assign_selected_builders_queued(sim:&mut BlastRange,edit:&Workshop,site:usize,queued:bool)->Option<usize> {
    let mut builders = edit.battle_selected_ids(sim);
    builders.sort_unstable();
    builders.dedup();
    builders.retain(|&id| matches!(sim.units[id].kind, RangeUnitKind::SkobaDrone | RangeUnitKind::Commander));
    if builders.is_empty() {
        let actor=edit.controlled?;
        if !sim.greben_build_admitted(actor,site) {return None;}
        let order=sim_core::blast::battle::Order::Build(site);
        return Some(usize::from(if queued {sim.execute_command(Command::QueueOrder{actor,order,destination:None}).accepted()}else{sim.battle_order(actor,order,None)}));
    }
    sim.set_construction_quota(site, 0);
    Some(builders.into_iter().filter(|&actor| {
        let order=sim_core::blast::battle::Order::Build(site);
        if queued {sim.execute_command(Command::QueueOrder{actor,order,destination:None}).accepted()}
        else {sim.battle_order(actor,order,None)}
    }).count())
}
pub(super) fn place_selected_building(sim:&mut BlastRange,edit:&Workshop,kind:RangeUnitKind,at:[i32;3],queued:bool)->Option<usize> {
    let mut builders=edit.battle_selected_ids(sim);
    builders.retain(|&id|matches!(sim.units[id].kind,RangeUnitKind::Commander|RangeUnitKind::SkobaDrone));
    builders.sort_unstable();builders.dedup();
    let actor=builders.first().copied().or(edit.controlled)?;
    if !sim.execute_command(Command::PlaceConstruction{actor,kind,at,queued}).accepted() {return None;}
    let site=sim.construction_site_at(at)?;
    for &actor in builders.iter().skip(1) {
        let order=sim_core::blast::battle::Order::Build(site);
        if queued {sim.execute_command(Command::QueueOrder{actor,order,destination:None});}
        else {sim.battle_order(actor,order,None);}
    }
    Some(site)
}
pub(super) fn context(sim: &BlastRange, panel: &panel::Panel, edit: &Workshop) -> Context {
    if edit.layout.is_none() || edit.pending.is_some() {
        return Context::Hidden;
    }
    if panel.free_placement || (edit.editing && sim.battle.phase == Phase::Setup) {
        return Context::Sandbox;
    }
    if edit.editing || !sim.battle_mode() || sim.battle.phase != Phase::Fighting {
        return if panel.sandbox { Context::Sandbox } else { Context::Hidden };
    }
    let Some(id) = edit.controlled else {
        return if panel.sandbox { Context::Sandbox } else { Context::Hidden };
    };
    if sim.unit_under_construction(id) {
        return if panel.sandbox { Context::Sandbox } else { Context::Hidden };
    }
    let Some(u) = sim
        .units
        .get(id)
        .filter(|u| u.status.alive && sim.allied(id))
    else {
        return if panel.sandbox { Context::Sandbox } else { Context::Hidden };
    };
    if matches!(u.kind, RangeUnitKind::SkobaDrone | RangeUnitKind::Commander | RangeUnitKind::GrebenStorage) {
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
    if panel.sandbox { Context::Sandbox } else { Context::Hidden }
}
fn view_context(c:&Catalog,sim:&BlastRange,panel:&panel::Panel,edit:&Workshop)->Context {
    let base=context(sim,panel,edit);
    match base {
        Context::Engineer(id) if c.production_mode && sim.units[id].kind==RangeUnitKind::GrebenStorage=>Context::Factory(id),
        _=>base,
    }
}
fn queue_actor(context:Context,sim:&BlastRange)->Option<usize> {
    match context {
        Context::Factory(id)=>Some(id),
        Context::Engineer(id) if sim.units[id].kind==RangeUnitKind::GrebenStorage=>Some(id), _=>None,
    }
}
pub fn tier(kind: RangeUnitKind) -> u8 {
    use RangeUnitKind::*;
    match kind {
        Strider | TurretT2 | ArtilleryT2 | ArtilleryTurretT2 | LightningTank | KilFactoryT2 | PowerPlantT2 => 2,
        Spider | Walker04 | ArtilleryTurretT3 | NuclearSilo | RazlomT3 | KilFactoryT3 | PowerPlantT3 => 3,
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
        Commander => "Командир",
        PowerPlantT1 => "Электростанция T1",
        PowerPlantT2 => "Электростанция T2",
        PowerPlantT3 => "Электростанция T3",
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
        Context::Sandbox => RangeUnitKind::ALL.into_iter().collect(),
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
    QueuePage(i32),
    Choose(usize),
    Side(bool),
    Cancel,
    Rotate,
    Upgrade,
    GrebenMode,
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
pub(super) struct QueueWidget;
#[derive(Component)]
pub(super) struct QueueRow;
#[derive(Component)]
pub(super) struct QueueCard(usize);
#[derive(Component)]
pub(super) struct QueueImage(usize);
#[derive(Component)]
pub(super) struct QueueText(Option<usize>);
#[derive(Component)]
pub(super) struct QueuePip(usize, usize);
#[derive(Component)]
pub(super) struct SandboxControl(u8);

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
                                matches!(u.kind, RangeUnitKind::SkobaDrone | RangeUnitKind::Commander)
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
            "plans" => Some(RangeUnitKind::Commander),
              "walker" => Some(RangeUnitKind::Walker04),
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
                  if matches!(case.as_str(), "plans" | "walker") {scene.paused=true;scene.view=6;}
            }
        }
    }
    let next = if lab.active() || pilot.active() {
        Context::Hidden
    } else {
        view_context(&c,&scene.sim, &panel, &edit)
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
        c.queue_page = 0;
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
            Action::QueuePage(delta) => {
                c.queue_page = (c.queue_page as i32 + delta).max(0) as usize;
            }
            Action::Side(allied) if next == Context::Sandbox => {
                c.allied = allied;
                c.hover = None;
            }
            Action::Rotate if next == Context::Sandbox => {
                c.yaw = (c.yaw + 1) % 4;
                c.hover = None;
            }
            Action::GrebenMode => {
                if queue_actor(next,&scene.sim).is_some_and(|id|scene.sim.units[id].kind==RangeUnitKind::GrebenStorage) {
                    c.production_mode=!c.production_mode; c.pending=None; c.hover=None;
                    panel.construction_kind=None; return;
                }
            }
            Action::Upgrade => {
                if let Context::Factory(actor) = next {
                    scene.notice = if scene.sim.upgrade_kil(actor) {
                        "Заказ улучшения принят"
                    } else {
                        "Улучшение недоступно: проверьте материал, тир и состояние завода"
                    }
                    .into();
                }
            }
            Action::Cancel => {
                let planner=match next {Context::Engineer(actor) if c.pending.is_none() && !scene.sim.order_queue(actor).is_empty()=>Some(actor),_=>None};
                if let Some(actor)=planner {
                    scene.notice=if scene.sim.execute_command(Command::CancelLastOrder{actor}).accepted() {"Последний приказ отменён; начатую стройку можно продолжить"}else{"Планов нет"}.into();
                } else if let Some(actor) = queue_actor(next,&scene.sim).filter(|_|c.pending.is_none()) {
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
        let yaw = if next == Context::Sandbox { c.yaw } else { 0 };
        if c.hover_issue.as_deref() == Some(scene.notice.as_str()) { scene.notice.clear(); }
        c.hover_issue = placement_issue(&scene.sim, kind, at, yaw, next != Context::Sandbox);
        c.hover = Some((at, c.hover_issue.is_some()));
        c.hover_tick = Some(scene.sim.tick);
    }
    if mouse.just_pressed(MouseButton::Left) {
        if c.hover.is_some_and(|(_, invalid)| invalid) {
            scene.notice = c.hover_issue.clone().unwrap_or_else(|| "Здесь разместить нельзя".into());
            mouse.reset(MouseButton::Left);
            return;
        }
        let result = if next == Context::Sandbox {
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
            let queued=keys.pressed(KeyCode::ShiftLeft)||keys.pressed(KeyCode::ShiftRight);
            match place_selected_building(&mut scene.sim,&edit,kind,at,queued) {
                Some(_) => Ok(()),
                None => Err("Стройплощадка недоступна или место уже занято"),
            }
        } else {
            Err("Выберите живого союзного инженера")
        };
        match result {
            Ok(()) => {
                scene.notice = if next == Context::Sandbox {
                    format!("Размещён {} · без сброса симуляции", name(kind))
                } else if edit.controlled.is_some_and(|id| scene.sim.units[id].kind == RangeUnitKind::Commander) {
                    "План принят · Shift: очередь · ПКМ строителем по площадке: продолжить".into()
                } else if edit.controlled.is_some_and(|id| scene.sim.units[id].kind == RangeUnitKind::SkobaDrone) {
                    "Выбранные дроны строят эту площадку".into()
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

fn placement_issue(sim: &BlastRange, kind: RangeUnitKind, at: [i32; 3], yaw: u8,
    engineer: bool) -> Option<String> {
    use sim_core::blast::placement::{Rejection, Validator};
    let validator = Validator::default();
    if let Err(issue) = validator.runtime(sim, kind, at, yaw) {
        return Some(match issue {
            Rejection::Outside if validator.runtime_bounds(kind, at, yaw).1[1] >= sim.size()[1] as i32 - 1 => {
                let top = validator.runtime_bounds(kind, at, yaw).1[1];
                format!("Недостаточно высоты карты: верх здания {:.1} м, потолок {:.1} м",
                    top as f32 / 10.0, sim.size()[1] as f32 / 10.0)
            }
            Rejection::Outside => "Часть здания выходит за край карты".into(),
            Rejection::Terrain => "Корпус пересекается с грунтом или препятствием".into(),
            Rejection::Units => "Место занято юнитом, зданием или планом стройки".into(),
            Rejection::Layout(message) => message,
        });
    }
    (engineer && !sim.can_construct(kind, true, at))
        .then(|| "Стройплощадка недоступна: ограничение строительства".into())
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
                    p.commands().entity(id).insert(SandboxControl(0));
                }
                let id = button(p, font, "Повернуть · Q/E", Action::Rotate);
                p.commands().entity(id).insert(SandboxControl(0));
                let id = button(p, font, "Улучшить завод", Action::Upgrade);
                p.commands().entity(id).insert(SandboxControl(1));
                let id=button(p,font,"Стройка ↔ Техника",Action::GrebenMode);
                p.commands().entity(id).insert(SandboxControl(2));
                button(p, font, "Отмена", Action::Cancel);
                button(p, font, "←", Action::Page(-1));
                button(p, font, "→", Action::Page(1));
            });
            let id = label(p, font, "", 14.0);
            p.commands().entity(id).insert(Caption::Heading);
            p.spawn((QueueWidget, QueueRow, Node {
                display: Display::None, height: px(QUEUE_HEIGHT), flex_shrink: 0.0,
                align_items: AlignItems::Center, column_gap: px(5), ..default()
            })).with_children(|p| {
                let id = label(p, font, "ОЧЕРЕДЬ", 13.0);
                p.commands().entity(id).insert(QueueText(None));
                button(p, font, "‹", Action::QueuePage(-1));
                for slot in 0..QUEUE_SLOTS {
                    p.spawn((QueueWidget, QueueCard(slot), Node {
                        position_type: PositionType::Relative, width: px(54), height: px(58),
                        flex_shrink: 0.0, border: UiRect::all(px(1)), ..default()
                    }, BackgroundColor(theme::BUTTON), BorderColor::all(theme::FRAME)))
                    .with_children(|p| {
                        p.spawn((QueueImage(slot), ImageNode::default(), Node {
                            position_type: PositionType::Absolute, left: px(1), top: px(0),
                            width: px(50), height: px(46), ..default()
                        }));
                        let id = p.spawn((QueueText(Some(slot)), Text::new(""),
                            TextFont { font_size: FontSize::Px(11.0), ..font.clone() },
                            TextColor(theme::TEXT), Node { position_type: PositionType::Absolute,
                                left: px(3), top: px(0), ..default() })).id();
                        let _ = id;
                        p.spawn(Node { position_type: PositionType::Absolute, bottom: px(2),
                            left: px(2), column_gap: px(1), ..default() }).with_children(|p| {
                            for segment in 0..10 {
                                p.spawn((QueueWidget, QueuePip(slot, segment), Node {
                                    width: px(4), height: px(5), ..default()
                                }, BackgroundColor(theme::FRAME)));
                            }
                        });
                    });
                }
                button(p, font, "›", Action::QueuePage(1));
            });
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
    (mut c, spatial): (ResMut<Catalog>, Option<Res<spatial_ui::State>>),
    (scene, publication): (Res<Scene>, Option<Res<super::ui_snapshot::UiSnapshot>>),
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
                Without<QueueWidget>,
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
    mut controls: Query<(&Action, &Interaction, &mut BackgroundColor), (Without<Slot>, Without<QueueWidget>)>,
    mut sides: Query<(&SandboxControl, &mut Node), (Without<Slot>, Without<Root>)>,
    (mut text, mut pictures): (
        Query<(&Caption, &mut Text), Without<QueueText>>,
        Query<(&Picture, &mut ImageNode), Without<QueueImage>>,
    ),
    (mut queue_nodes, mut queue_text, mut queue_images): (
        Query<(&mut Node, Option<&QueueRow>, Option<&QueueCard>, Option<&QueuePip>, Option<&mut BackgroundColor>, Option<&mut BorderColor>),
            (With<QueueWidget>, Without<Root>, Without<Slot>, Without<SandboxControl>)>,
        Query<(&QueueText, &mut Text), Without<Caption>>,
        Query<(&QueueImage, &mut ImageNode), Without<Picture>>,
    ),
) {
    let context = if lab.active() || pilot.active() || menu.shown() {
        Context::Hidden
    } else {
        view_context(&c,&scene.sim, &panel, &edit)
    };
    let compact = windows.single().is_ok_and(|w| w.width() < 1280.0);
    let plans:Vec<_>=if let Context::Engineer(actor)=context {scene.sim.order_queue(actor).into_iter().filter_map(|p|match p.order {
        sim_core::blast::battle::Order::Build(site)=>scene.sim.construction_site_state(site).filter(|s|!s.cancelled&&!s.destroyed&&!s.complete),_=>None}).collect()}else{Vec::new()};
    let planning=!plans.is_empty();
    let producer=if planning {None}else{queue_actor(context,&scene.sim)};
    let queue_visible=producer.is_some() || planning;
    let queue_height = if queue_visible { QUEUE_HEIGHT + 8.0 } else { 0.0 };
    for mut root in &mut root {
        root.display = if context == Context::Hidden {
            Display::None
        } else {
            Display::Flex
        };
        root.height = px((if compact { HEIGHT + 52.0 } else { HEIGHT }) + queue_height);
    }
    let height = (if compact { HEIGHT + 52.0 } else { HEIGHT }) + queue_height;
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
    c.per_page = if spatial.as_ref().is_some_and(|s| s.enabled) && matches!(context,Context::Engineer(_)|Context::Factory(_)) {4}else{windows
        .single()
        .map_or(8, |w| ((w.width() - 64.0) / 190.0) as usize)
        .clamp(3, SLOTS)};
    let list = choices(&c, &scene.sim);
    let pages = list.len().div_ceil(c.per_page).max(1);
    c.page = c.page.min(pages - 1);
    let page: Vec<_> = list
        .into_iter()
        .skip(c.page * c.per_page)
        .take(c.per_page)
        .collect();
    let queue = if planning {plans.iter().map(|s|s.kind).collect()}else if let Some(actor) = producer {
        scene.sim.production_queue(actor).unwrap_or_default()
    } else { Vec::new() };
    let queue_per_page = windows.single().map_or(8, |w| ((w.width() - 250.0).max(54.0) / 59.0) as usize).clamp(1, QUEUE_SLOTS);
    let queue_pages = queue.len().div_ceil(queue_per_page).max(1);
    c.queue_page = c.queue_page.min(queue_pages - 1);
    let queue_start = c.queue_page * queue_per_page;
    let queue_page = &queue[queue_start.min(queue.len())..(queue_start + queue_per_page).min(queue.len())];
    let progress = if planning {(plans[0].delivered.saturating_mul(100)/plans[0].required.max(1)).min(100)}else if let Some(actor) = producer {
        scene.sim.production_progress(actor).map_or(0, |(ticks, build, paid, cost)| {
            (u64::from(ticks) * 100 / u64::from(build.max(1))).min(paid.saturating_mul(100) / cost.max(1)).min(100)
        })
    } else { 0 };
    for (mut node, row, card, pip, background, border) in &mut queue_nodes {
        if row.is_some() { node.display = if queue_visible { Display::Flex } else { Display::None }; }
        if let Some(QueueCard(slot)) = card {
            node.display = if *slot < queue_page.len() { Display::Flex } else { Display::None };
            if let Some(mut b) = border { *b = BorderColor::all(if queue_start + *slot == 0 { theme::ACCENT } else { theme::FRAME }); }
        }
        if let (Some(QueuePip(slot, segment)), Some(mut bg)) = (pip, background) {
            node.display = if queue_start + *slot == 0 { Display::Flex } else { Display::None };
            bg.0 = if (*segment as u64 + 1) * 10 <= progress { theme::ACCENT } else { theme::FRAME };
        }
    }
    for (QueueText(slot), mut text) in &mut queue_text {
        text.0 = match slot {
            None if queue.is_empty() => "ОЧЕРЕДЬ · пусто".into(),
            None => format!("{} · {} / {}",if planning {"ПЛАН СТРОЙКИ"}else{"ОЧЕРЕДЬ"}, c.queue_page + 1, queue_pages),
            Some(slot) if *slot < queue_page.len() => format!("{}", queue_start + slot + 1),
            _ => String::new(),
        };
    }
    if page.iter().chain(queue_page.iter()).all(|k| c.icons.contains_key(&k.index())) {
        c.image_ready_frames = c.image_ready_frames.saturating_add(1);
    } else {
        c.image_ready_frames = 0;
    }
    // Share the laboratory's actual template thumbnail renderer. One new image
    // per frame, cached by kind; no extra world or offscreen simulation.
    if let Some(kind) = page
        .iter().chain(queue_page.iter())
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
            Action::GrebenMode => c.production_mode,
            _ => false,
        };
        background.0 = theme::button_background(*interaction, active);
    }
    for (scope, mut node) in &mut sides {
        let shown=match scope.0 {
            0=>context==Context::Sandbox,
            1=>matches!(context,Context::Factory(id) if matches!(scene.sim.units[id].kind,RangeUnitKind::KilFactoryT1|RangeUnitKind::KilFactoryT2|RangeUnitKind::KilFactoryT3)),
            _=>producer.is_some_and(|id|scene.sim.units[id].kind==RangeUnitKind::GrebenStorage),
        };
        node.display=if shown {Display::Flex}else{Display::None};
    }
    for (Picture(slot), mut image) in &mut pictures {
        image.image = page
            .get(*slot)
            .and_then(|k| c.icons.get(&k.index()))
            .cloned()
            .unwrap_or_default();
    }
    for (QueueImage(slot), mut image) in &mut queue_images {
        image.image = queue_page.get(*slot).and_then(|k| c.icons.get(&k.index())).cloned().unwrap_or_default();
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
                Context::Engineer(actor) => {
                    let sites = scene.sim.construction_sites();
                    let active = sites.iter().filter(|s| !s.complete&&!s.cancelled&&!s.destroyed).count();
                    let title = match scene.sim.units[actor].kind {
                        RangeUnitKind::GrebenStorage => "ГРЕБЕНЬ · автоматическое строительство",
                        RangeUnitKind::SkobaDrone => "ВЫБРАННЫЕ ДРОНЫ · строительство",
                        _ => "КОМАНДИР · строительство",
                    };
                    format!(
                        "{title} · стройки: {active} · страница {}/{}",
                        c.page + 1,
                        pages
                    )
                }
                Context::Factory(actor) => {
                    let waiting = if progress >= 100 && !queue.is_empty() {
                        if scene.sim.factory_upgrading(actor) {" · ожидает места для нового корпуса"} else {" · ожидает свободного выезда"}
                    } else { "" };
                    format!("ПРОИЗВОДСТВО · {} T{} · заказов: {}{} · ПКМ по карте — точка сбора",
                        name(scene.sim.units[actor].kind), tier(scene.sim.units[actor].kind), queue.len(), waiting)
                }
                Context::Hidden => String::new(),
            },
            Caption::Item(slot) => page.get(slot).map_or(String::new(), |k| {
                let cost = if let Context::Factory(actor) = context {
                    scene
                        .sim
                        .production_quote(actor,*k)
                        .map(|(v,ticks,power)| format!(" · {} мат. · {} энерг. · {} мощн.", economy_display::mass(v), economy_display::energy_work(power.saturating_mul(u64::from(ticks))), economy_display::energy(power)))
                        .unwrap_or_default()
                } else if let Context::Engineer(actor) = context {
                    c.costs.get(&k.index()).map_or_else(String::new, |v| {
                        let mut label = format!(" · {} мат.", economy_display::mass(*v));
                        if scene.sim.units[actor].kind == RangeUnitKind::Commander {
                            let demand = scene.sim.economic_rules().print_power;
                            let work = scene.sim.construction_energy_estimate(*v);
                            label.push_str(&format!(" · ~{} энерг. · мощн. {}", economy_display::energy_work(work), economy_display::energy(demand)));
                        } else { label.push_str(&format!(" · {} мощн./дрон", economy_display::energy(scene.sim.economic_rules().drone_power))); }
                        label
                    })
                } else {
                    String::new()
                };
                let generation = scene.sim.economic_rules().generation(*k);
                let output = if generation > 0 { format!(" · +{} мощн.", economy_display::energy(generation)) } else { String::new() };
                format!("{} · T{}{cost}{output}", name(*k), tier(*k))
            }),
            Caption::Hint => {
                if c.pending.is_some() && c.hover.is_some_and(|(_, invalid)| invalid) {
                    c.hover_issue.clone().unwrap_or_else(|| "Здесь разместить нельзя".into())
                } else if !scene.notice.is_empty() {
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
                } else if c.pending == Some(RangeUnitKind::GrebenStorage)
                    && matches!(context, Context::Engineer(_)) {
                    let rules=scene.sim.economic_rules();
                    let count=if rules.queued_crews {u64::from(rules.greben_crew_drones)}else{8};
                    let crew=scene.sim.production_cost(RangeUnitKind::SkobaDrone).unwrap_or(0).saturating_mul(count)
                        .saturating_add(scene.sim.production_cost(RangeUnitKind::Crawler).unwrap_or(0));
                    if rules.queued_crews {
                        format!("ЛКМ — разместить · Команда встанет в очередь: до {} массы, время и питание · Снабжение: {} м", economy_display::mass(crew), scene.sim.supply_radius() / 10)
                    } else {
                        format!("ЛКМ — разместить · Комплектование по запасу: до {} массы · Снабжение: {} м", economy_display::mass(crew), scene.sim.supply_radius() / 10)
                    }
                } else if c.pending.is_some_and(|k| matches!(k, RangeUnitKind::KilFactoryT1 | RangeUnitKind::KilFactoryT2 | RangeUnitKind::KilFactoryT3 | RangeUnitKind::KilFactoryT4))
                    && matches!(context, Context::Engineer(_)) && scene.sim.supply_radius() > 0 {
                    c.hover.map_or_else(|| "Укажите место: покажем стоимость подключения к складу".into(), |(at, _)| {
                        scene.sim.supply_quote_at(at, true).map_or_else(
                            || format!("Склада в радиусе {} м нет: завод можно построить, но снабжение недоступно", scene.sim.supply_radius() / 10),
                            |(_, distance, cost)| format!("ЛКМ — разместить · Труба к складу: {:.1} м · стоимость {} массы", distance as f32 / 10.0, economy_display::mass(cost)),
                        )
                    })
                } else if c.pending.is_some() {
                    "ЛКМ — разместить · Shift — очередь построек · ПКМ / Esc — закрыть размещение · зелёный: можно, красный: препятствие".into()
                } else if let Context::Factory(actor) = context {
                    if scene.sim.units[actor].kind==RangeUnitKind::GrebenStorage {
                        "Сборка техники: нужны масса склада и местное питание. «Отмена» снимает последний заказ; «Стройка ↔ Техника» меняет каталог.".into()
                    } else if scene.sim.supply_radius() == 0 {
                        "Нажмите на юнит, чтобы добавить заказ. «Отмена» возвращает материал.".into()
                    } else if let Some((_, distance, cost)) = scene.sim.factory_supply_quote(actor) {
                        format!("{} · склад {:.1} м · труба: {} массы · «Отмена» убирает заказ", if scene.sim.factory_supply_connected(actor) { "Снабжение подключено" } else { "Ожидает оплаты подключения" }, distance as f32 / 10.0, economy_display::mass(cost))
                    } else {
                        format!("Нет склада в радиусе {} м — завод не получает массу. Приблизьте склад или доставьте массу командиром.", scene.sim.supply_radius() / 10)
                    }
                } else if let Context::Engineer(actor) = context {
                    if scene.sim.units[actor].kind == RangeUnitKind::Commander {
                        let reach = sim_core::blast::economy::construction::COMMANDER_SUPPLY_REACH / 10;
                        format!("Shift: очередь · ПКМ по плану: продолжить · Снабжение командира: {reach} м")
                    } else {
                        "Shift + размещение / ПКМ: очередь · ПКМ строителем по плану: продолжить · «Отмена»: последний приказ".into()
                    }
                } else {
                    "Выберите карточку: выделенные дроны строят сами; «Гребень» распределяет задания автоматически.".into()
                }
            }
        };
        if matches!(caption, Caption::Heading) && context != Context::Sandbox {
            if let Some((held, cap)) = scene.sim.side_store(true) {
                text.0.push_str(&format!(" · склад: {}/{}", economy_display::mass(held), economy_display::mass(cap)));
            }
            if let Context::Engineer(actor) = context {
                if scene.sim.units[actor].kind == RangeUnitKind::Commander {
                    if let Some((held, cap)) = scene.sim.tank_fill(actor) {
                        let income = scene.sim.economic_rules().commander_matter_per_second;
                        let actual = if held >= cap { 0 } else { income };
                        text.0.push_str(&format!(" · командир: {}/{} (+{}/с)", economy_display::mass(held), economy_display::mass(cap), economy_display::mass(actual)));
                    }
                }
            }
            let (generation, demand) = publication.as_ref().map_or_else(|| scene.sim.construction_energy(true), |p| p.energy_total);
            text.0.push_str(&format!(" · все сети: +{} / −{} мощн.", economy_display::energy(generation), economy_display::energy(demand)));
            if let Context::Engineer(actor) | Context::Factory(actor) = context {
                let local = publication.as_ref().map_or_else(
                    || scene.sim.construction_power_networks().get(actor).copied().flatten(),
                    |p| p.energy_local.get(actor).copied().flatten());
                if let Some((generation, demand)) = local {
                    text.0.push_str(&format!(" · рядом: +{} / −{}", economy_display::energy(generation), economy_display::energy(demand)));
                    if demand > generation { text.0.push_str(" (дефицит)"); }
                } else { text.0.push_str(" · нет подключения к энергии"); }
            }
        }
    }
}

/// Read-only bridge for the world-anchored deck. Recipes and actions still use
/// the single catalogue, permission checks, thumbnails and simulation queues.
pub(super) struct SpatialRecipe {
    pub kind: RangeUnitKind,
    pub image: Handle<Image>,
    pub cost: String,
}
pub(super) struct SpatialCatalog {
    pub recipes: Vec<SpatialRecipe>,
    pub heading: String,
    pub queue: Vec<(RangeUnitKind,Handle<Image>)>,
    pub progress: u64,
    pub hint: String,
    pub greben: bool,
}
pub(super) fn spatial_catalog(c:&Catalog,sim:&BlastRange,generation:u64,selected:usize)->Option<SpatialCatalog> {
    let actor=match c.context {Context::Engineer(id)|Context::Factory(id)=>id,_=>return None};
    if c.generation!=generation || actor!=selected {return None;}
    sim.units.get(actor)?;
    let list=choices(c,sim);
    let pages=list.len().div_ceil(c.per_page.max(1)).max(1);
    let recipes:Vec<_>=list.into_iter().skip(c.page*c.per_page.max(1)).take(4).map(|kind| {
        let cost=if matches!(c.context,Context::Factory(_)) {
            sim.production_quote(actor,kind).map(|(mass,ticks,power)|format!("М {} · Э {}",economy_display::mass(mass),economy_display::energy_work(power.saturating_mul(u64::from(ticks))))).unwrap_or_else(||"Недоступно".into())
        }else {
            c.costs.get(&kind.index()).map(|&mass|if sim.units[actor].kind==RangeUnitKind::Commander {
                format!("М {} · Э ~{}",economy_display::mass(mass),economy_display::energy_work(sim.construction_energy_estimate(mass)))
            }else{format!("М {} · пит. {}/дрон",economy_display::mass(mass),economy_display::energy(sim.economic_rules().drone_power))}).unwrap_or_default()
        };
        SpatialRecipe{kind,image:c.icons.get(&kind.index()).cloned().unwrap_or_default(),cost}
    }).collect();
    let empty=recipes.is_empty();
    let plans:Vec<_>=if matches!(c.context,Context::Engineer(_)) {sim.order_queue(actor).into_iter().filter_map(|p|match p.order {
        sim_core::blast::battle::Order::Build(site)=>sim.construction_site_state(site).filter(|s|!s.complete&&!s.cancelled&&!s.destroyed),_=>None}).collect()}else{Vec::new()};
    let queue=if plans.is_empty(){queue_actor(c.context,sim).and_then(|id|sim.production_queue(id)).unwrap_or_default()}else{plans.iter().map(|s|s.kind).collect()};
    let progress=if plans.is_empty(){sim.production_progress(actor).map_or(0,|(ticks,build,paid,cost)|
        (u64::from(ticks)*100/u64::from(build.max(1))).min(paid.saturating_mul(100)/cost.max(1)).min(100))}else{plans[0].delivered.saturating_mul(100)/plans[0].required.max(1)};
    Some(SpatialCatalog{recipes,heading:format!("{} · {}/{}",if matches!(c.context,Context::Factory(_)){"ПРОИЗВОДСТВО"}else{"СТРОИТЕЛЬСТВО"},c.page+1,pages),
        queue:queue.into_iter().map(|k|(k,c.icons.get(&k.index()).cloned().unwrap_or_default())).collect(),progress,
        hint:if empty {"В этом тире нет доступных рецептов · выберите другой тир".into()}else if matches!(c.context,Context::Factory(_)){"ЛКМ — в очередь · ПКМ по карте — точка сбора".into()}else if c.pending.is_some(){"ЛКМ по земле — поставить · Shift — очередь · Esc — отменить".into()}else{"Выберите постройку · Shift — очередь размещений".into()},
        greben:sim.units.get(actor).is_some_and(|u|u.kind==RangeUnitKind::GrebenStorage)})
}
