//! One catalogue for sandbox placement, engineering and real production queues.
use super::*;
use modding::laboratory::catalog::{Category, Element};
use panel::theme;
use sim_core::blast::battle::{commands::Command, Phase};
use std::collections::BTreeMap;
use workbench::ghost::Templates;
use workbench::Selection;

#[path="construction/production_hud.rs"]
pub mod production_hud;
const SLOTS: usize = 16;
const QUEUE_SLOTS: usize = 16;
#[path="construction/plans.rs"]
pub mod plans;
#[cfg(test)]
#[path = "construction/tests.rs"]
pub(super) mod tests;
pub const HEIGHT: f32 = 128.0;
fn catalogue_width(viewport:Vec2)->f32 {(viewport.x-186.-320.).clamp(96.,720.)}
fn catalogue_page_size(viewport:Vec2)->usize {((catalogue_width(viewport)/112.) as usize).clamp(1,6)}
/// Keep native-size text while giving short viewports their world space back.
pub(super) fn dock_height(viewport: Vec2) -> f32 {
    if viewport.y < 780. { 100. } else { HEIGHT }
}
fn quote_bottom(viewport: Vec2) -> f32 {
    200_f32.min((viewport.y-104.).max(0.))
}
/// The visible quote is protected from the projector above the fixed minimap.
pub(super) fn quote_bounds(viewport: Vec2) -> (Vec2, Vec2) {
    (Vec2::new(16.,viewport.y-quote_bottom(viewport)-112.),Vec2::new(160.,112.))
}

#[derive(Component)]
pub(super) enum CatalogPart { Filters, Row }
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
    legacy_requested: bool,
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
    optical_icons: BTreeMap<usize, Handle<Image>>,
    pixel_icons: BTreeMap<usize, Handle<Image>>,
    costs: BTreeMap<usize, u64>,
    capture_initialized: bool,
    image_ready_frames: u8,
}
impl Catalog {
    pub(super) fn motion_selected(&self,action:Action)->Option<bool> {match action {Action::Tier(t)=>Some(t==self.tier),Action::Category(c)=>Some(c==self.category),_=>None}}
    /// Refresh resolves menu/lab visibility; spatial UI alone owns the child islands.
    pub(super) fn is_hidden(&self) -> bool { self.context == Context::Hidden || !self.legacy_requested }
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
    DismissPlacement,
    Tier(u8),
    Category(u8),
    Page(i32),
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
    #[cfg_attr(not(test), allow(dead_code))]
    Heading,
    #[cfg_attr(not(test), allow(dead_code))]
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

#[derive(Component)]
pub(super) struct RecipeTooltip;
#[derive(Component)]
pub(super) struct RecipeTooltipText;
#[derive(Default)]
pub(super) struct RecipeHover {
    key: Option<(u64, usize, RangeUnitKind)>,
    seconds: f32,
}

/// Details use live economic quotes, never copied balance constants.
fn recipe_details(c: &Catalog, sim: &BlastRange, kind: RangeUnitKind) -> String {
    let mut info = format!("{} · T{}", name(kind), tier(kind));
    match c.context {
        Context::Factory(actor) => {
            if let Some((mass, ticks, power)) = sim.production_quote(actor, kind) {
                info.push_str(&format!("\nМасса {} · Энергия {}\nПотребление {}",
                    economy_display::mass(mass), economy_display::energy_work(power.saturating_mul(u64::from(ticks))),
                    economy_display::energy(power)));
            }
            info.push_str("\nЛКМ — в очередь");
        }
        Context::Engineer(actor) => {
            if let Some(&mass) = c.costs.get(&kind.index()) {
                info.push_str(&format!("\nМасса {}", economy_display::mass(mass)));
                if sim.units[actor].kind == RangeUnitKind::Commander {
                    info.push_str(&format!(" · Энергия ~{}", economy_display::energy_work(sim.construction_energy_estimate(mass))));
                } else {
                    info.push_str(&format!("\nПотребление {}/дрон", economy_display::energy(sim.economic_rules().drone_power)));
                }
            }
            info.push_str("\nЛКМ — разместить · Shift — очередь");
        }
        _ => info.push_str("\nЛКМ — выбрать · затем укажите место"),
    }
    let generation = sim.economic_rules().generation(kind);
    if generation > 0 { info.push_str(&format!("\nВыработка +{}", economy_display::energy(generation))); }
    info
}

/// The normal hover and capture preview use the same real quote, node and geometry.
pub fn tooltip(
    c: Res<Catalog>, scene: Res<Scene>, time: Res<Time>, scale: Res<UiScale>,
    mut spatial: ResMut<spatial_ui::State>, windows: Query<&Window>, roots: Query<&Node, With<Root>>,
    cards: Query<(Option<&Slot>,Option<&spatial_ui::Part>,&Interaction),Or<(With<Slot>,With<spatial_ui::Part>)>>, mut hover: Local<RecipeHover>,
    mut tip: Query<&mut Node, (With<RecipeTooltip>, Without<Root>)>,
    mut labels: Query<&mut Text, With<RecipeTooltipText>>,
) {
    spatial.reserve_quote(false);
    let visible=c.generation==scene.generation&&c.context!=Context::Hidden&&roots.iter().any(|n|n.display!=Display::None);
    let preview=visible.then(||spatial.recipe_preview()).flatten();
    let selected=preview.or_else(||visible.then(||cards.iter().find_map(|(slot,part,i)| {
        if *i!=Interaction::Hovered {return None;}
        slot.map(|slot|slot.0).or_else(||match part {Some(spatial_ui::Part::Recipe(i))=>Some(*i),_=>None})
    })).flatten());
    let kind=selected.and_then(|slot|choices(&c,&scene.sim).get(c.page*c.per_page.max(1)+slot).copied());
    let key=kind.map(|kind|(scene.generation,c.page,kind));
    if hover.key==key {hover.seconds+=time.delta_secs().min(0.1);} else {hover.key=key;hover.seconds=0.;}
    let Ok(window)=windows.single() else {return};
    let viewport=window.size()/scale.0;
    for mut node in &mut tip {
        let Some(kind)=kind.filter(|_|preview.is_some()||hover.seconds>=0.12) else {
            if node.display!=Display::None { node.display=Display::None; }
            continue;
        };
        spatial.reserve_quote(true);
        // Anchor the live quote above the fixed minimap, independently of card size.
        let bottom=px(quote_bottom(viewport));
        if node.display!=Display::Flex || node.left!=px(16) || node.top!=Val::Auto || node.bottom!=bottom || node.width!=px(160) {
            node.left=px(16);node.top=Val::Auto;node.bottom=bottom;node.width=px(160);
            node.display=Display::Flex;
        }
        // Abbreviate labels, not data: the live full quote remains the only price source.
        let value=recipe_details(&c,&scene.sim,kind)
            .replace("Масса ","М ").replace("Энергия ","Э ").replace("Потребление ","Пит. ")
            .replace("ЛКМ — разместить · Shift — очередь","ЛКМ / Shift — план")
            .replace("ЛКМ — в очередь","ЛКМ — очередь")
            .replace("ЛКМ — выбрать · затем укажите место","ЛКМ — выбрать")
            .replace("Выработка +","Ген. +");
        for mut text in &mut labels {if text.0!=value {text.0.clone_from(&value);}}
    }
}

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
    roots: Query<&bevy::ui::RelativeCursorPosition, (Or<(With<panel::PanelRoot>,With<panel::HitIsland>)>,Without<panel::LayoutOnly>)>,
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
            Action::DismissPlacement => {
                c.pending = None;
                c.hover = None;
                panel.construction_kind = None;
            }
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
        match action {Action::Tier(t)|Action::Category(t)=>optical_ui::Finish::pixel_filter(t),_=>optical_ui::Finish::pixel(0)},
        Node {
            column_gap:px(4),
            padding: UiRect::axes(px(10), px(6)),
            align_items: AlignItems::Center,
            border: if matches!(action,Action::Tier(_)|Action::Category(_)) {UiRect::bottom(px(1))} else {UiRect::all(px(1))},
            ..default()
        },
        BackgroundColor(theme::BUTTON),
        BorderColor::all(theme::FRAME),
    ));
    e.with_children(|p| {
        match action {Action::Tier(t)=>optical_ui::pixel_glyph(p,t),Action::Category(t)=>optical_ui::pixel_glyph(p,5+t),_=>{}}
        let face=TextFont{weight:if matches!(action,Action::Tier(_)|Action::Category(_)){bevy::text::FontWeight::MEDIUM}else{font.weight},..font.clone()};
        label(p, &face, text, 14.0);
    });
    e.id()
}
pub fn shortcuts(p: &mut ChildSpawnerCommands, font: &TextFont) {
    button(p, font, "Инженер", Action::Focus(true));
    button(p, font, "Завод", Action::Focus(false));
}
pub fn build(commands: &mut Commands, camera: Entity, font: &TextFont) {
    production_hud::build(commands,camera,font);
    commands.spawn((RecipeTooltip, panel::PanelRoot, UiTargetCamera(camera), GlobalZIndex(35),
        bevy::ui::FocusPolicy::Pass, Pickable::IGNORE,
        bevy::ui::RelativeCursorPosition::default(),
        Node { position_type: PositionType::Absolute, width: px(160), border:UiRect::all(px(1)),padding: UiRect::all(px(8)),
            display: Display::None, ..default() }, BackgroundColor(theme::PANEL),BorderColor::all(theme::FRAME)))
        .with_children(|p| { theme::corners(p);p.spawn((RecipeTooltipText, Text::new(""),
            TextFont { font_size: FontSize::Px(14.), ..theme::data_font(font) },bevy::text::LineHeight::Px(14.),TextColor(theme::TEXT))); });
    commands
        .spawn((
            Root,
            panel::LayoutOnly,
            Pickable::IGNORE,
            bevy::ui::FocusPolicy::Pass,
            panel::PanelRoot,
            bevy::ui::RelativeCursorPosition::default(),
            UiTargetCamera(camera),
            GlobalZIndex(10),
            Node {
                position_type: PositionType::Absolute,
                left: px(186),
                right: px(320),
                bottom: px(72),
                height: px(HEIGHT),
                max_width: px(720),
                padding: UiRect::all(px(0)),
                row_gap: px(8),
                flex_direction: FlexDirection::ColumnReverse,
                border: UiRect::ZERO,
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::NONE),
            BorderColor::all(Color::NONE),
        ))
        .with_children(|p| {
            p.spawn((CatalogPart::Filters, panel::HitIsland, optical_ui::Finish::data(), bevy::ui::RelativeCursorPosition::default(), BackgroundColor(theme::PANEL), Node {
                padding: UiRect::all(px(4)),
                align_self:AlignSelf::FlexStart,max_width:percent(100),
                column_gap: px(4),
                row_gap: px(4),
                flex_wrap: FlexWrap::Wrap,
                align_items: AlignItems::Center,
                ..default()
            }))
            .with_children(|p| {
                for (t, n) in [(0, "Все тиры"), (1, "T1"), (2, "T2"), (3, "T3"), (4, "T4")] {
                    button(p, font, n, Action::Tier(t));
                }
                for (t, n) in [(0, "Всё"), (1, "Юн."), (2, "Постр.")] {
                    button(p, font, n, Action::Category(t));
                }
            });
            p.spawn((CatalogPart::Row, Node {
                column_gap: px(8),
                width: percent(100),
                height: px(80),
                flex_grow: 0.0,
                flex_shrink: 0.0,
                ..default()
            }))
            .with_children(|p| {
                for slot in 0..SLOTS {
                    p.spawn((
                        Button,
                        panel::HitIsland,
                        bevy::ui::RelativeCursorPosition::default(),
                        Action::Choose(slot),
                        Slot(slot),
                        optical_ui::Finish::pixel(2),
                        Node {
                            flex_grow: 0.0,
                            flex_shrink: 0.0,
                            flex_basis: px(104),
                            min_width: px(90),
                            height: percent(100),
                            min_height: px(0),
                            flex_direction: FlexDirection::Column,
                            padding: UiRect::all(px(6)),
                            border: UiRect::bottom(px(1)),
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
                                height: px(64),
                                width: px(104),
                                ..default()
                            },
                        ));
                        let id = label(p, font, "", 14.0);
                        p.commands().entity(id).insert((Caption::Item(slot), bevy::text::LineHeight::Px(14.)));
                    });
                }
            });

        });
    commands.spawn((AuxRoot, panel::PanelRoot, bevy::ui::RelativeCursorPosition::default(),
        UiTargetCamera(camera),GlobalZIndex(12), Node {position_type:PositionType::Absolute,
        right:px(16),bottom:px(64),max_width:px(440),padding:UiRect::all(px(4)),column_gap:px(4),
        row_gap:px(4),flex_wrap:FlexWrap::Wrap,display:Display::None,..default()},BackgroundColor(theme::PANEL)))
        .with_children(|p| {
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
}

/// Resolve the catalogue after this frame's world projection is known.
/// A spider can request the sandbox catalogue while its command hologram owns
/// the screen. Only this system opens Root, including after a selection change.
#[derive(Component)]
pub struct AuxRoot;

pub fn sync_visibility(
    catalog: Res<Catalog>,
    _spatial: Res<spatial_ui::State>,
    mut roots: Query<(Has<AuxRoot>,&mut Node), Or<(With<Root>,With<AuxRoot>)>>,
) {
    for (aux,mut node) in &mut roots {
        let next = if catalog.legacy_requested && (!aux || catalog.context!=Context::Hidden) {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != next { node.display = next; }
    }
}

/// All bounds depend on the viewport, never on which actor is selected.
pub fn responsive(
    windows: Query<&Window>, scale: Res<UiScale>,
    mut nodes: Query<(
        &mut Node, Option<&Root>, Option<&Slot>, Option<&Picture>,
        Option<&CatalogPart>, Option<&Action>,
    ), Or<(With<Root>, With<Slot>, With<Picture>, With<CatalogPart>, With<Action>)>>,
) {
    let Ok(window)=windows.single() else { return };
    let viewport=window.size()/scale.0;
    let compact=viewport.y<780.;
    for (mut node, root, slot, picture, part, action) in &mut nodes {
        if root.is_some() {
            let height=px(dock_height(viewport));
            if node.height!=height {node.height=height;}
            if node.row_gap!=px(8.) {node.row_gap=px(8.);}
        }
        if slot.is_some() {
            let per_page=catalogue_page_size(viewport);
            let basis=px((catalogue_width(viewport)-(per_page-1) as f32*8.)/per_page as f32);
            let padding=UiRect::all(px(if compact {2.} else {6.}));
            let gap=px(if compact {2.} else {4.});
            if node.flex_basis!=basis {node.flex_basis=basis;}
            if node.padding!=padding {node.padding=padding;}
            if node.row_gap!=gap {node.row_gap=gap;}
        }
        if picture.is_some() {
            let (width,height)=(px(if compact {52.} else {76.}),px(if compact {32.} else {44.}));
            if node.width!=width {node.width=width;}
            if node.height!=height {node.height=height;}
        }
        match part {
            Some(CatalogPart::Filters)=>{
                if node.padding!=UiRect::all(px(2)) {node.padding=UiRect::all(px(2));}
                if node.column_gap!=px(4) {node.column_gap=px(4);}
            }
            Some(CatalogPart::Row)=>{
                let height=px(if compact {68.} else {80.});
                if node.height!=height {node.height=height;}
            }
            None=>{}
        }
        if matches!(action,Some(Action::Tier(_)|Action::Category(_))) {
            let padding=UiRect::axes(px(4),px(2));
            if node.padding!=padding {node.padding=padding;}
        }
    }
}

pub fn refresh(
    (mut c, _spatial): (ResMut<Catalog>, Option<Res<spatial_ui::State>>),
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
    mut controls: Query<(&Action, &Interaction, &mut BackgroundColor, Option<&mut BorderColor>), (Without<Slot>, Without<QueueWidget>)>,
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
    let legacy_queue_ui = !queue_nodes.is_empty()
        || !queue_text.is_empty()
        || !queue_images.is_empty()
        || text.iter().any(|(c, _)| !matches!(c, Caption::Item(_)));
    let mut plans = Vec::new();
    let planning = if let Context::Engineer(actor) = context {
        if legacy_queue_ui {
            plans = scene.sim.order_queue(actor).into_iter().filter_map(|p| match p.order {
                sim_core::blast::battle::Order::Build(site) => scene
                    .sim
                    .construction_site_state(site)
                    .filter(|s| !s.cancelled && !s.destroyed && !s.complete),
                _ => None,
            }).collect();
            !plans.is_empty()
        } else {
            scene.sim.order_queue(actor).into_iter().any(|p| match p.order {
                sim_core::blast::battle::Order::Build(site) => scene
                    .sim
                    .construction_site_state(site)
                    .is_some_and(|s| !s.cancelled && !s.destroyed && !s.complete),
                _ => false,
            })
        }
    } else {
        false
    };
    let producer = if planning { None } else { queue_actor(context, &scene.sim) };
    c.legacy_requested = edit.layout.is_some()&&!lab.active()&&!pilot.active()&&!menu.shown();
    // responsive owns viewport-dependent dimensions; refresh must not reset them.
    let _ = &mut root;
    // No selection-dependent movement: the bottom dock and minimap own fixed anchors.
    for (mut node, deck, strip, _map, _briefing) in &mut legacy {
        if (deck.is_some() || (strip.is_some() && context!=Context::Hidden)) && node.display!=Display::None {node.display=Display::None;}
    }
    if context == Context::Hidden || context != c.context {
        for (_,mut node,_,_,_) in &mut cards {if node.display!=Display::None {node.display=Display::None;}}
        return;
    }
    c.per_page = windows.single().map_or(6,|w|catalogue_page_size(w.size()/panel::hud::scale(w.size())));
    let list = choices(&c, &scene.sim);
    let pages = list.len().div_ceil(c.per_page).max(1);
    c.page = c.page.min(pages - 1);
    let page: Vec<_> = list
        .into_iter()
        .skip(c.page * c.per_page)
        .take(c.per_page)
        .collect();
    let queue = if !legacy_queue_ui {
        Vec::new()
    } else if planning {
        plans.iter().map(|s| s.kind).collect()
    } else if let Some(actor) = producer {
        scene.sim.production_queue(actor).unwrap_or_default()
    } else {
        Vec::new()
    };
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
        if row.is_some() && node.display != Display::None { node.display = Display::None; }
        if let Some(QueueCard(slot)) = card {
            let next_display = if *slot < queue_page.len() { Display::Flex } else { Display::None };
            if node.display != next_display { node.display = next_display; }
            let next_border = BorderColor::all(if queue_start + *slot == 0 { theme::ACCENT } else { theme::FRAME });
            if let Some(mut b) = border { if *b != next_border { *b = next_border; } }
        }
        if let (Some(QueuePip(slot, segment)), Some(mut bg)) = (pip, background) {
            let next_display = if queue_start + *slot == 0 { Display::Flex } else { Display::None };
            if node.display != next_display { node.display = next_display; }
            let next_bg = if (*segment as u64 + 1) * 10 <= progress { theme::ACCENT } else { theme::FRAME };
            if bg.0 != next_bg { bg.0 = next_bg; }
        }
    }
    for (QueueText(slot), mut text) in &mut queue_text {
        let next = match slot {
            None if queue.is_empty() => "ОЧЕРЕДЬ · пусто".into(),
            None => format!("{} · {} / {}",if planning {"ПЛАН СТРОЙКИ"}else{"ОЧЕРЕДЬ"}, c.queue_page + 1, queue_pages),
            Some(slot) if *slot < queue_page.len() => format!("{}", queue_start + slot + 1),
            _ => String::new(),
        };
        if text.0 != next { text.0 = next; }
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
        let thumbnail = laboratory::visuals::transparent_thumbnail(&element, &templates);
        let mut clean=thumbnail.clone();
        clean.sampler=bevy::image::ImageSampler::nearest();
        let pixel=images.add(clean);
        c.pixel_icons.insert(kind.index(),pixel);
        let optical = images.add(spatial_ui::hologram_thumbnail(thumbnail.clone()));
        c.optical_icons.insert(kind.index(), optical);
        let icon = images.add(laboratory::visuals::thumbnail_canvas(thumbnail));
        c.icons.insert(kind.index(), icon);
    }
    for (Slot(slot), mut node, mut background, mut border, interaction) in &mut cards {
        let next_display = if *slot < page.len() {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != next_display { node.display = next_display; }
        let active = page.get(*slot).is_some_and(|k| Some(*k) == c.pending);
        let next_bg = theme::button_background(*interaction, active);
        if background.0 != next_bg { background.0 = next_bg; }
        let next_border = BorderColor::all(if active { theme::FOCUS } else { theme::FRAME });
        if *border != next_border { *border = next_border; }
    }
    for (action, interaction, mut background, border) in &mut controls {
        let active = match *action {
            Action::Tier(t) => t == c.tier,
            Action::Category(t) => t == c.category,
            Action::Side(s) => s == c.allied,
            Action::GrebenMode => c.production_mode,
            _ => false,
        };
        let next_bg = if matches!(action,Action::Tier(_)|Action::Category(_)) && !active && *interaction==Interaction::None {Color::NONE}else{theme::button_background(*interaction, active)};
        if background.0 != next_bg { background.0 = next_bg; }
        let next_border = BorderColor::all(if active {theme::CYAN}else if matches!(action,Action::Tier(_)){theme::LAVENDER.with_alpha(0.28)}else{theme::FRAME});
        if let Some(mut border)=border { if *border != next_border { *border = next_border; } }
    }
    for (scope, mut node) in &mut sides {
        let shown=match scope.0 {
            0=>context==Context::Sandbox,
            1=>matches!(context,Context::Factory(id) if matches!(scene.sim.units[id].kind,RangeUnitKind::KilFactoryT1|RangeUnitKind::KilFactoryT2|RangeUnitKind::KilFactoryT3)),
            _=>producer.is_some_and(|id|scene.sim.units[id].kind==RangeUnitKind::GrebenStorage),
        };
        let next_display = if shown {Display::Flex}else{Display::None};
        if node.display != next_display { node.display = next_display; }
    }
    for (Picture(slot), mut image) in &mut pictures {
        let handle = page
            .get(*slot)
            .and_then(|k| c.pixel_icons.get(&k.index()))
            .cloned()
            .unwrap_or_default();
        let settled = if handle == Handle::default() { Color::NONE } else { Color::WHITE };
        if image.image != handle || image.color != settled { optical_ui::preview_image(&mut image,handle); }
    }
    for (QueueImage(slot), mut image) in &mut queue_images {
        let handle=queue_page.get(*slot).and_then(|k| c.optical_icons.get(&k.index())).cloned().unwrap_or_default();
        let settled = if handle == Handle::default() { Color::NONE } else { Color::WHITE };
        if image.image != handle || image.color != settled { optical_ui::preview_image(&mut image,handle); }
    }
    if matches!(context, Context::Engineer(_)) {
        for &kind in &page {
            c.costs
                .entry(kind.index())
                .or_insert_with(|| scene.sim.construction_cost(kind).unwrap_or(0));
        }
    }
    for (caption, mut text) in &mut text {
        let mut next = match *caption {
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
            Caption::Item(slot) => page.get(slot).map_or(String::new(), |k| name(*k).into()),
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
                next.push_str(&format!(" · склад: {}/{}", economy_display::mass(held), economy_display::mass(cap)));
            }
            if let Context::Engineer(actor) = context {
                if scene.sim.units[actor].kind == RangeUnitKind::Commander {
                    if let Some((held, cap)) = scene.sim.tank_fill(actor) {
                        let income = scene.sim.economic_rules().commander_matter_per_second;
                        let actual = if held >= cap { 0 } else { income };
                        next.push_str(&format!(" · командир: {}/{} (+{}/с)", economy_display::mass(held), economy_display::mass(cap), economy_display::mass(actual)));
                    }
                }
            }
            let (generation, demand) = publication.as_ref().map_or_else(|| scene.sim.construction_energy(true), |p| p.energy_total);
            next.push_str(&format!(" · все сети: +{} / −{} мощн.", economy_display::energy(generation), economy_display::energy(demand)));
            if let Context::Engineer(actor) | Context::Factory(actor) = context {
                let local = publication.as_ref().map_or_else(
                    || scene.sim.construction_power_networks().get(actor).copied().flatten(),
                    |p| p.energy_local.get(actor).copied().flatten());
                if let Some((generation, demand)) = local {
                    next.push_str(&format!(" · рядом: +{} / −{}", economy_display::energy(generation), economy_display::energy(demand)));
                    if demand > generation { next.push_str(" (дефицит)"); }
                } else { next.push_str(" · нет подключения к энергии"); }
            }
        }
        if text.0 != next { text.0 = next; }
    }
}

/// Read-only bridge for the world-anchored deck. Recipes and actions still use
/// the single catalogue, permission checks, thumbnails and simulation queues.
pub(super) struct SpatialRecipe {
    pub kind: RangeUnitKind,
    pub image: Handle<Image>,
}
pub(super) struct SpatialCatalog {
    pub recipes: Vec<SpatialRecipe>,
    pub has_work: bool,
}
pub(super) fn spatial_catalog(c:&Catalog,sim:&BlastRange,generation:u64,selected:usize)->Option<SpatialCatalog> {
    let actor=match c.context {Context::Engineer(id)|Context::Factory(id)=>id,_=>return None};
    if c.generation!=generation || actor!=selected {return None;}
    sim.units.get(actor)?;
    let list=choices(c,sim);
    let recipes:Vec<_>=list.into_iter().skip(c.page*c.per_page.max(1)).take(4).map(|kind| {
        SpatialRecipe{kind,image:c.optical_icons.get(&kind.index()).or_else(||c.icons.get(&kind.index())).cloned().unwrap_or_default()}
    }).collect();
    let engineer_work=matches!(c.context,Context::Engineer(_)) && sim.order_queue(actor).into_iter().any(|p|match p.order {
        sim_core::blast::battle::Order::Build(site)=>sim.construction_site_state(site).is_some_and(|s|!s.complete&&!s.cancelled&&!s.destroyed),_=>false});
    let has_work=engineer_work || queue_actor(c.context,sim).and_then(|id|sim.production_queue(id)).is_some_and(|q|!q.is_empty());
    Some(SpatialCatalog{recipes,has_work})
}
