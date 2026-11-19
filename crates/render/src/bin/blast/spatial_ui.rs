//! World-anchored control deck. One pooled panel; no simulation entities or HUD per unit.
use super::*;
use bevy::text::LetterSpacing;
use bevy::ui::{FocusPolicy, RelativeCursorPosition};
use panel::{Action, CrawlerMode};
use sim_core::blast::battle::Weapon as BattleWeapon;
use workbench::EditorCamera;

#[path = "spatial_ui/chrome.rs"]
mod chrome;
#[path = "spatial_ui/projection.rs"]
mod projection;
#[path = "spatial_ui/acceptance.rs"]
pub(super) mod acceptance;

const WIDTH: f32 = 328.;
const SHELF_HEIGHT: f32 = 148.;
const INK: Color = panel::theme::OPTICAL.text;
const CYAN: Color = panel::theme::CYAN;
const FRAME: Color = panel::theme::OPTICAL.frame;
const GLASS: Color = panel::theme::OPTICAL.panel;
const DIM: Color = panel::theme::OPTICAL.muted;

#[derive(Resource)]
pub(super) struct State {
    pub enabled: bool,
    pub visible: bool,
    pinned: Option<(usize, Vec2)>,
    pinned_anchor:Option<Vec2>,
    inspected: Option<(u64, usize)>,
    pub catalog_open: bool,
    pub tools_open: bool,
    hovered: Option<Part>,
    hover_seconds: f32,
    preview_recipe: Option<usize>,
    quote_visible: bool,
    collapsed: bool,
    layout_hold:f32,
    layout_tools:bool,
    motion: projection::Motion,
    occluders:Vec<(Vec2,Vec2)>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            enabled: true,
            visible: false,
            pinned: None,
            pinned_anchor:None,
            inspected: None,
            catalog_open: false,
            tools_open: false,
            hovered: None,
            hover_seconds: 0.,
            preview_recipe: None,
            quote_visible: false,
            collapsed: false,
            layout_hold:0.,
            layout_tools:false,
            motion: default(),
            occluders:Vec::new(),
        }
    }
}
impl State {
    pub(super) fn audit_obstacles(&self)->&[(Vec2,Vec2)]{&self.occluders}
    fn place(&mut self,id:usize,at:Vec2,half:Vec2,viewport:Vec2,size:Vec2,queue:f32,hovered:bool)->Option<Vec2> {
        let stable=self.pinned_anchor.is_some_and(|p|p.distance_squared(at)<1.);
        let previous=self.pinned;
        let pin=self.pinned.filter(|(actor,p)|*actor==id && (hovered||stable)
            && projection::allowed(*p,at,half,viewport,size,queue) && projection::clears(*p,size,&self.occluders)).map(|(_,p)|p);
        let p=pin.or_else(||projection::place_avoiding(at,half,viewport,size,queue,&self.occluders));
        self.pinned=p.map(|p|(id,p));
        if self.pinned!=previous {self.pinned_anchor=Some(at);}
        p
    }
    /// Reproducible native UI captures use the same disclosure state as clicks.
    pub fn initial(catalog: bool, tools: bool) -> Self {
        Self { catalog_open: catalog, tools_open: tools && !catalog, ..default() }
    }
    pub fn screen_mode(mut self, screen:bool)->Self {self.enabled=!screen;self}
    pub fn recipe_preview(&self)->Option<usize> {self.preview_recipe}
    pub(super) fn reserve_quote(&mut self, visible: bool) {self.quote_visible=visible;}
    pub fn with_tooltip_preview(mut self, slot: Option<usize>) -> Self {
        self.preview_recipe = slot;
        if slot.is_some() { self.catalog_open = true; self.tools_open = false; }
        self
    }
    fn inspect(&mut self, selected: Option<(u64, usize)>) {
        if self.inspected == selected { return }
        if self.inspected.is_some() {
            self.catalog_open = false;
            self.tools_open = false;
            self.preview_recipe = None;
        }
        self.inspected = selected;
        self.collapsed = false;
        self.layout_hold=0.;
        self.pinned = None;
        self.pinned_anchor=None;
        self.hovered = None;
        self.hover_seconds = 0.;
    }
}
#[derive(Component)]
pub(super) struct Toggle;
#[derive(Component, Clone, Copy, PartialEq)]
pub(super) enum Part {
    Deck,
    CatalogShelf,
    #[cfg(test)]
    Production,
    Recipe(usize),
    Command(Action),
    Corner(usize),
    Ray(usize),
    Emitter(usize),
    Pip(usize),
    CatalogToggle,
    ToolsToggle,
    Tooltip,
    Scan,
    #[cfg(test)]
    QueueRow,
    Signal(usize),
    FrameEdge(usize,usize),
    Spoke(usize),
    Vertex(usize,usize),
    SourceSpot,
    FoldToggle,
    Restore,
}
#[derive(Component)]
pub(super) enum Caption {
    Title,
    Status,
    Name(usize),
    Toggle,
    Tooltip,
    CatalogToggle,
}
#[derive(Component)]
pub(super) enum Visual {
    Recipe(usize),
}

pub fn input(
    mut state: ResMut<State>,
    buttons: Query<&Interaction, (With<Toggle>, Changed<Interaction>)>,
    keys: Res<ButtonInput<KeyCode>>,
    menu: Res<frontend::Menu>,
    lab: Res<laboratory::Laboratory>,
    pilot: Res<pilot::Pilot>,
    local: Query<(&Part, &Interaction), Changed<Interaction>>,
) {
    if menu.shown() || lab.active() || pilot.active() {
        return;
    }
    if keys.just_pressed(KeyCode::F8) || buttons.iter().any(|i| *i == Interaction::Pressed) {
        state.enabled = !state.enabled;
        state.pinned = None;
    }
    if !state.enabled { return; }
    for (part, interaction) in &local {
        if *interaction == Interaction::Pressed {
            match part {
                Part::CatalogToggle => { state.catalog_open = !state.catalog_open; state.tools_open = false; }
                Part::FoldToggle => { state.collapsed = true; }
                Part::Restore => { state.collapsed = false; }
                Part::ToolsToggle => { state.tools_open = !state.tools_open; state.catalog_open = false; }
                _ => {}
            }
        }
    }
}
pub fn entry(p: &mut ChildSpawnerCommands, font: &TextFont) {
    p.spawn((
        Toggle,
        Button,
        Node {
            height: px(30),
            padding: UiRect::axes(px(9), px(4)),
            border: UiRect::all(px(1)),
            ..default()
        },
        BackgroundColor(GLASS),
        BorderColor::all(FRAME),
    ))
    .with_children(|p| {
        p.spawn((
            Caption::Toggle,
            Text::new("У объекта · F8"),
            TextFont {
                font_size: FontSize::Px(13.),
                ..font.clone()
            },
            TextColor(INK),
        ));
    });
}
fn label(p: &mut ChildSpawnerCommands, font: &TextFont, caption: Caption, size: f32, color: Color) {
    p.spawn((
        caption,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(size),
            ..font.clone()
        },
        TextColor(color),
        LetterSpacing::Px(0.6),
    ));
}
pub fn build(
    commands: &mut Commands,
    camera: Entity,
    fallback: &TextFont,
    fonts: &mut Assets<Font>,
    images: &mut Assets<Image>,
) {
    chrome::build(commands, camera, fallback, fonts, images);
}
pub(super) fn hologram_thumbnail(image: Image) -> Image { chrome::hologram_thumbnail(image) }

fn tooltip_top(position: Vec2, panel_height: f32, viewport: Vec2, zoom: f32, height: f32) -> f32 {
    if position.y + (panel_height + height + 12.) * zoom <= viewport.y - 24. {
        panel_height + 6.
    } else if position.y - (height + 6.) * zoom >= 82. {
        -height - 12.
    } else {
        // On a short viewport reuse the panel's space rather than leave the screen.
        ((viewport.y - 24. - position.y) / zoom - height - 6.).max(0.)
    }
}
/// Cache local occupied bounds once per template/scene, never scan voxels per frame.
#[derive(Default)]
pub(super) struct BoundsCache(std::collections::BTreeMap<(u64, usize), Vec<([i32; 3], [i32; 3])>>);
fn occupied_bounds(body: &voxel::unit::UnitBody) -> ([i32; 3], [i32; 3]) {
    let mut lo = [i32::MAX; 3];
    let mut hi = [0; 3];
    for z in 0..usize::from(body.vol.nz) {
        for x in 0..usize::from(body.vol.nx) {
            for y in 0..usize::from(body.vol.ny) {
                if body.vol.get(x, y, z) {
                    for (a, v) in [x, y, z].into_iter().enumerate() {
                        lo[a] = lo[a].min(v as i32);
                        hi[a] = hi[a].max(v as i32 + 1);
                    }
                }
            }
        }
    }
    if lo[0] == i32::MAX {
        ([0; 3], [1; 3])
    } else {
        (lo, hi)
    }
}
/// Try the open side first, then the opposite side, and keep controls on screen.
fn projection_viewport(viewport:Vec2)->Vec2 {
    let clearance=if viewport.y<780. {208.} else {construction::HEIGHT};
    Vec2::new(viewport.x,(viewport.y-clearance-100.).max(240.))
}
fn compact_detail(viewport:Vec2,position:Vec2,zoom:f32)->(f32,f32,f32) {
    let (at,size)=construction::quote_bounds(viewport);
    ((at.x-position.x)/zoom,(at.y-position.y)/zoom,size.x/zoom)
}
#[cfg(test)]
fn panel_position(anchor: Vec2, half: Vec2, viewport: Vec2, size: Vec2) -> Vec2 {
    let gap = 22.;
    let top = 104.;
    let mut x = anchor.x + half.x + gap;
    if x + size.x > viewport.x - 16. {
        x = anchor.x - half.x - gap - size.x;
    }
    Vec2::new(
        x.clamp(16., (viewport.x - size.x - 16.).max(16.)),
        (anchor.y - size.y * 0.4).clamp(top, (viewport.y - size.y - 24.).max(top)),
    )
}

/// A roof voxel above the core, rather than the top of a remote tower's bounds.
fn projector_origin(body: &voxel::unit::UnitBody) -> [i32; 3] {
    let x = usize::from(body.core.0);
    let z = usize::from(body.core.2);
    let top = (0..usize::from(body.vol.ny)).rev().find(|&y| body.vol.get(x, y, z));
    [x as i32, top.unwrap_or(usize::from(body.core.1)) as i32, z as i32]
}
fn command_visible(action: Action, kind: RangeUnitKind, armed: bool, fighting: bool) -> bool {
    match action {
        Action::CrawlerMode(_) | Action::CrawlerUnload | Action::UpgradeCrawler => {
            kind == RangeUnitKind::Crawler
        }
        Action::ElectricSupport => kind == RangeUnitKind::LightningTank,
        Action::NuclearLaunch => kind == RangeUnitKind::NuclearSilo && fighting,
        Action::UpgradeKil => matches!(
            kind,
            RangeUnitKind::KilFactoryT1 | RangeUnitKind::KilFactoryT2 | RangeUnitKind::KilFactoryT3
        ),
        Action::Fire | Action::AutoAttack => {
            armed && kind != RangeUnitKind::NuclearSilo && fighting
        }
        Action::BattleWeapon(_) => matches!(kind, RangeUnitKind::Tank | RangeUnitKind::Artillery),
        _ => true,
    }
}
fn secondary(action: Action) -> bool {
    matches!(action, Action::Ranges | Action::FirstPerson | Action::UpgradeKil
        | Action::UpgradeCrawler | Action::BattleWeapon(_))
}
/// Density-aware, not resolution-proportional: 4K must not double the world UI.
fn optical_scale(viewport: Vec2) -> f32 {
    (1. + (viewport.y - 1080.).max(0.) / 4320.).clamp(1., 1.25)
}

fn command_hint(action: Action) -> &'static str {
    match action {
        Action::Stop => "Стоп · остановить движение и текущую работу.",
        Action::Fire => "Атака · выберите цель или точку на карте.",
        Action::AutoAttack => "Автоатака · самостоятельно атаковать врагов в радиусе оружия.",
        Action::Ranges => "Радиусы · показать дальность оружия и рабочую область.",
        Action::FirstPerson => "1-е лицо · перейти к управлению выбранным юнитом [V].",
        Action::CrawlerMode(CrawlerMode::Move) => "Движение · ПКМ по карте задаёт маршрут краулера.",
        Action::CrawlerMode(CrawlerMode::Reclaim) => "Сбор · ПКМ по останкам. Затем краулер собирает ближайшие обломки.",
        Action::CrawlerMode(CrawlerMode::Drill) => "Бурение · ПКМ по грунту задаёт место добычи.",
        Action::CrawlerUnload => "Разгрузка · отправить краулер в доступное гнездо склада.",
        Action::ElectricSupport => "Саппорт · выберите союзный электротанк для усиления его разряда.",
        Action::NuclearLaunch => "Пуск · выберите точку ядерного удара.",
        Action::UpgradeKil => "Улучшить завод · перейти к следующему тиру производства.",
        Action::UpgradeCrawler => "Дроны · установить строительный модуль краулера.",
        Action::BattleWeapon(None) => "По цели · автоматически выбирать подходящий боеприпас.",
        Action::BattleWeapon(Some(BattleWeapon::Blast)) => "Фугас · осколочно-фугасный боеприпас.",
        Action::BattleWeapon(Some(BattleWeapon::Kinetic)) => "Бронебойный · кинетическое пробитие брони.",
        Action::BattleWeapon(Some(BattleWeapon::Energy)) => "Энергия · тепловое поражение брони.",
        _ => "",
    }
}
pub fn update(
    (mut state, scene, snapshot): (ResMut<State>, Res<Scene>, Res<ui_snapshot::UiSnapshot>),
    (edit, panel): (Res<Workshop>, Res<panel::Panel>),
    (catalog, menu, lab, pilot, scale): (
        Res<construction::Catalog>,
        Res<frontend::Menu>,
        Res<laboratory::Laboratory>,
        Res<pilot::Pilot>,
        Res<UiScale>,
    ),
    cameras: Query<(&Camera, &Transform), With<EditorCamera>>,
    roots: Query<&RelativeCursorPosition, With<Part>>,
    mut parts: Query<(
        &Part,
        &mut Node,
        Option<&mut UiTransform>,
        Option<&mut BackgroundColor>,
        Option<&mut BorderColor>,
        Option<&ComputedNode>,
        Option<&Interaction>,
    )>,
    mut legacy: Query<
        (
            &mut Node,
            Option<&panel::hud::CommandDeck>,
            Option<&panel::hud::RosterStrip>,
            Option<&panel::minimap::MiniRoot>,
            Option<&construction::CatalogPart>,
        ),
        (
            Or<(
                With<panel::hud::CommandDeck>,
                With<panel::hud::RosterStrip>,
                With<panel::minimap::MiniRoot>,
                With<construction::CatalogPart>,
            )>,
            Without<Part>,
            Without<construction::production_hud::Root>,
        ),
    >,
    mut texts: Query<(&Caption, &mut Text)>,
    mut visuals: Query<(&Visual, &mut ImageNode)>,
    mut giz: Gizmos,
    mut bounds: Local<BoundsCache>,
    time: Option<Res<Time>>,
    mut envelopes: Query<&mut optical_ui::ProjectionEnvelope>,
    queues: Query<(&ComputedNode,&Node),(With<construction::production_hud::Root>,Without<Part>)>,
    mut lights: Query<(&Part,&mut BoxShadow)>,
    effects:Option<Res<ui_motion::Settings>>,
) {
    let strength=effects.as_ref().map_or(1.,|e|e.strength());
    let dt=time.as_ref().map_or(0.,|t|t.delta_secs().min(0.1));
    if state.layout_tools!=state.tools_open {state.layout_hold=0.20;state.layout_tools=state.tools_open;}
    state.layout_hold=(state.layout_hold-dt).max(0.);
    bounds
        .0
        .retain(|(generation, _), _| *generation == scene.generation);
    let active = state.enabled
        && !menu.shown()
        && !lab.active()
        && !pilot.active()
        && !edit.editing
        && !panel.free_placement
        && scene.sim.battle_mode();
    let id = edit.controlled.filter(|&id| {
        scene
            .sim
            .units
            .get(id)
            .is_some_and(|u| u.status.alive && scene.sim.allied(id))
    });
    let inspected = id.map(|id| (scene.generation, id));
    state.inspect(inspected);
    let projection = active.then_some(id).flatten().and_then(|id| {
        let u = &scene.sim.units[id];
        let (camera, transform) = cameras.single().ok()?;
        let viewport = camera.logical_viewport_size()? / scale.0;
        let gt = GlobalTransform::from(*transform);
        let boxes = bounds
            .0
            .entry((scene.generation, u.kind.index()))
            .or_insert_with(|| {
                std::iter::once(&u.body)
                    .chain(u.body.segments.iter().map(|s| &s.body))
                    .map(occupied_bounds)
                    .collect()
            });
        let mut screen_lo = Vec2::splat(f32::INFINITY);
        let mut screen_hi = Vec2::splat(f32::NEG_INFINITY);
        let mut world_lo = Vec3::splat(f32::INFINITY);
        let mut world_hi = Vec3::splat(f32::NEG_INFINITY);
        for (index, (lo, hi)) in boxes.iter().enumerate() {
            if index > 0 && u.seg_gone.get(index - 1).copied().unwrap_or(false) {
                continue;
            }
            let frame = if index == 0 {
                &u.pose
            } else {
                u.seg_frames.get(index - 1)?
            };
            for k in 0..8 {
                let local = std::array::from_fn(|a| {
                    fx::Fx::from_int(if k & (1 << a) == 0 { lo[a] } else { hi[a] })
                });
                let point = frame.world(local);
                let p = Vec3::new(
                    point.x.to_f64_lossy() as f32,
                    point.y.to_f64_lossy() as f32,
                    point.z.to_f64_lossy() as f32,
                );
                world_lo = world_lo.min(p);
                world_hi = world_hi.max(p);
                if let Ok(pixel) = camera.world_to_viewport(&gt, p) {
                    screen_lo = screen_lo.min(pixel / scale.0);
                    screen_hi = screen_hi.max(pixel / scale.0);
                }
            }
        }
        if !screen_lo.is_finite() || !screen_hi.is_finite() {
            return None;
        }
        let at = (screen_lo + screen_hi) * 0.5;
        if at.x < 0. || at.x > viewport.x || at.y < 70. || at.y > viewport.y {
            return None;
        }
        let half = ((screen_hi - screen_lo) * 0.5).max(Vec2::splat(18.));
        let centre = (world_lo + world_hi) * 0.5;
        Some((
            id,
            at,
            half,
            viewport,
            Vec3::new(centre.x, world_lo.y, centre.z),
            world_hi - world_lo,
        ))
    });
    state.visible = projection.is_some();
    let details = projection
        .as_ref()
        .and_then(|p| construction::spatial_catalog(&catalog, &scene.sim, scene.generation, p.0));
    let hovered = projection.and_then(|_| parts.iter().find_map(|(part, node, _, _, _, _, interaction)| {
        (node.display != Display::None && interaction.is_some_and(|i| *i == Interaction::Hovered))
            .then_some(*part)
    }));
    if state.hovered == hovered {
        state.hover_seconds += time.as_ref().map_or(0., |t| t.delta_secs().min(0.1));
    } else {
        state.hovered = hovered;
        state.hover_seconds = 0.;
    }
    let tooltip = if state.hover_seconds >= 0.12 {
        match hovered {
            // Projected and ordinary recipes share construction::tooltip's live quote island.
            Some(Part::Recipe(_)) => None,
            Some(Part::Command(a)) if !secondary(a) || state.tools_open => Some(command_hint(a).into()),
            Some(Part::CatalogToggle) => Some("Строительство и производство · открыть каталог выбранного объекта.".into()),
            Some(Part::ToolsToggle) => Some("Ещё · радиусы, управление от первого лица и настройки оружия.".into()),
            _ => None,
        }
    } else { None::<String> };
    let tooltip_height = parts.iter().find_map(|(p, _, _, _, _, n, _)| {
        matches!(p, Part::Tooltip).then(|| n.map(|n| n.size().y * n.inverse_scale_factor())).flatten()
    }).filter(|h| *h > 20.).unwrap_or(120.);
    let mut position = Vec2::ZERO;
    let mut panel_size = Vec2::new(WIDTH, 200.);
    let mut zoom = 1.;
    if let Some((id, _at, _half, viewport, _, _)) = projection {
        zoom = optical_scale(viewport);
        panel_size.x = projection::width(viewport);
        let initial_height=if scene.sim.units[id].kind==RangeUnitKind::Crawler {147.}else{115.};
        // Layout size is from the previous finished UI pass; first-frame bound
        // reserves the maximum deck so appearing controls never leave the window.
        panel_size.y = parts
            .iter()
            .find_map(|(p, _, _, _, _, n, _)| {
                matches!(p, Part::Deck)
                    .then(|| n.map(|n| n.size().y * n.inverse_scale_factor()))
                    .flatten()
            })
            .filter(|h| *h > 50.)
            .unwrap_or(initial_height);
        let top = 132.;
        zoom = zoom.min(((viewport.y - top - 24.) / panel_size.y).max(0.8));
    } else {
        state.pinned = None;
    }
    let selected = projection
        .map(|p| p.0)
        .and_then(|id| snapshot.view.as_ref().and_then(|v| v.units.get(id)));
    // The centre of a projected bounding rectangle can lie in empty space,
    // especially for a wide factory. Project a real hull point instead.
    let project_source = |id:usize| {
        let u = &scene.sim.units[id];
        let (camera, transform) = cameras.single().ok()?;
        let point = u.pose.world(projector_origin(&u.body).map(fx::Fx::from_int));
        camera.world_to_viewport(&GlobalTransform::from(*transform), Vec3::new(
            point.x.to_f64_lossy() as f32, point.y.to_f64_lossy() as f32,
            point.z.to_f64_lossy() as f32)).ok().map(|p| p / scale.0)
    };
    let emitter=projection.and_then(|p|project_source(p.0)).unwrap_or(Vec2::ZERO);
    let queue_height=queues.iter().filter(|(_,n)|n.display!=Display::None).map(|(n,_)|n.size().y*n.inverse_scale_factor()+24.).fold(0.,f32::max);
    let hold=state.layout_hold>0.;
    state.occluders.clear();
    if let (Some((selected,..)),Ok((camera,transform)))=(projection,cameras.single()) {
        let global=GlobalTransform::from(*transform);
        for (id,u) in scene.sim.units.iter().enumerate().filter(|(id,u)|*id!=selected && u.status.alive) {
            let _=id;
            let (lo,hi)=u.bounds();let mut a=Vec2::splat(f32::INFINITY);let mut b=Vec2::splat(f32::NEG_INFINITY);let mut valid=true;
            for corner in 0..8 {
                let p=Vec3::new(if corner&1==0{lo[0]}else{hi[0]} as f32,if corner&2==0{lo[1]}else{hi[1]} as f32,if corner&4==0{lo[2]}else{hi[2]} as f32);
                if let Ok(p)=camera.world_to_viewport(&global,p){a=a.min(p/scale.0);b=b.max(p/scale.0);}else{valid=false;break;}
            }
            if valid && b.x>0. && b.y>0. && a.x<projection.unwrap().3.x && a.y<projection.unwrap().3.y {state.occluders.push((a-Vec2::splat(6.),b-a+Vec2::splat(12.)));}
        }
    }
    if state.quote_visible || projection.is_some_and(|p|p.3.y<780.) {
        if let Some((_,_,_,viewport,_,_))=projection {
            state.occluders.push(construction::quote_bounds(viewport));
        }
    }
    let placement = projection.and_then(|(id,at,half,viewport,_,_)| {
        // Reserve the optional shelf even while closed: disclosure never shifts commands.
        let height = if viewport.y<780. {136.} else {
            panel_size.y + if details.is_some() {SHELF_HEIGHT + 12.} else {0.}
        };
        state.place(id,at,half,viewport,Vec2::new(panel_size.x*zoom,height*zoom),queue_height,roots.iter().any(|r|r.cursor_over()) || hold)
    });
    if let Some(p)=placement {position=p;}
    // A hovered target is pinned only while it remains outside all protected areas.
    let requested=projection.filter(|_|placement.is_some() && !state.collapsed).map(|p|(scene.generation,p.0));
    if menu.shown() || lab.active() || pilot.active() || state.motion.key.is_some_and(|k|k.0!=scene.generation) {
        state.motion=default();
    }
    let tools=state.tools_open;
    state.motion.step(requested,tools,dt);
    state.motion.work=details.as_ref().is_some_and(|d|d.has_work);
    let live_rig=projection.map(|(_,at,_,viewport,_,_)|projection::Rig{source:emitter,position,
        size:panel_size*zoom,camera:(at/viewport-Vec2::splat(0.5)).clamp(Vec2::splat(-0.5),Vec2::splat(0.5))});
    if requested.is_some() && state.motion.key==requested {if let Some(rig)=live_rig {state.motion.publish(rig,dt);}}
    if let Some(source)=state.motion.key.and_then(|(_,id)|project_source(id)) {
        if let Some(r)=state.motion.last.as_mut() {r.source=source;}
    }
    let rig=state.motion.last.or(live_rig);
    let readable=state.motion.readable(requested);
    state.visible=readable;
    let projected_recipes = readable && state.catalog_open && projection.is_some_and(|p|p.3.y>=780.);
    let closed_compact_drawer = readable && projection.is_some_and(|p|p.3.y<780.) && !state.catalog_open;
    let hidden_catalog = catalog.is_hidden();
    for (mut node, deck, strip, _map, catalog_part) in &mut legacy {
        if matches!(catalog_part,Some(construction::CatalogPart::Row)) {
            let hide=hidden_catalog || projected_recipes || closed_compact_drawer;
            let display=if hide {Display::None}else{Display::Flex};
            if node.display!=display {node.display=display;}
        }
        if matches!(catalog_part,Some(construction::CatalogPart::Filters)) {
            let hide=hidden_catalog || closed_compact_drawer;
            let display=if hide {Display::None}else{Display::Flex};
            if node.display!=display {node.display=display;}
        }
        if (deck.is_some() || (strip.is_some() && state.catalog_open)) && node.display!=Display::None {
            node.display=Display::None;
        }
    }
    for (part,mut shadow) in &mut lights {
        if matches!(part,Part::Emitter(_)|Part::SourceSpot) {
            let e=state.motion.energy*strength;
            let next=BoxShadow::new(CYAN.with_alpha((0.07+e*0.30)*strength),px(0),px(0),px(2),px(8.+e*12.));
            if *shadow!=next {*shadow=next;}
        }
    }
    for mut e in &mut envelopes {
        let next_source=(emitter-position)/(panel_size*zoom);
        if e.source!=next_source || e.aperture!=state.motion.aperture || e.energy!=state.motion.energy {
            e.source=next_source;e.aperture=state.motion.aperture;e.energy=state.motion.energy;
        }
    }

    let integrity = selected.map_or(0, |u| {
        u.integrity
            .core
            .mul(fx::Fx::from_int(100))
            .to_int_trunc()
            .clamp(0, 100) as usize
    });
    for (part, mut target_node, mut transform, mut background, border, _, interaction) in &mut parts {
        // Resolve the part locally; only a real layout change dirties its Node.
        let mut node=(*target_node).clone();
        if let Some(rig)=rig {
            if projection::decorate(*part,&mut node,transform.as_deref_mut(),background.as_deref_mut(),rig,&state.motion,strength) {target_node.set_if_neq(node);continue;}
        } else if matches!(part,Part::FrameEdge(..)|Part::Spoke(_)|Part::Vertex(..)|Part::SourceSpot|Part::Ray(_)|Part::Signal(_)|Part::Emitter(_)) {
            if node.display!=Display::None {node.display=Display::None;}
            target_node.set_if_neq(node);continue;
        }
        if let Some((id, at, half, _, base, size)) = projection {
            let unit = &scene.sim.units[id];
            match *part {
                Part::Deck => {
                    node.display = if readable {Display::Flex}else{Display::None};
                    node.width=px(panel_size.x);
                    let viewport=projection.unwrap().3;
                    node.max_height=px(((viewport.y-construction::dock_height(viewport)-120.-104.)/zoom).max(112.));
                    node.overflow=Overflow::scroll_y();
                    node.left = px(position.x + (zoom - 1.) * panel_size.x * 0.5);
                    node.top = px(position.y + (zoom - 1.) * panel_size.y * 0.5);
                    if let Some(mut t) = transform {
                        if t.scale!=Vec2::splat(zoom) {t.scale=Vec2::splat(zoom);}
                    }
                }
                Part::CatalogShelf => {
                    node.display = if projected_recipes {Display::Flex} else {Display::None};
                    node.width = px(panel_size.x);
                    node.left = px(position.x + (zoom-1.)*panel_size.x*0.5);
                    node.top = px(position.y+(panel_size.y+12.)*zoom+(zoom-1.)*SHELF_HEIGHT*0.5);
                    if let Some(mut t)=transform {if t.scale!=Vec2::splat(zoom) {t.scale=Vec2::splat(zoom);}}
                }
                Part::Corner(i) => {
                    node.display = Display::Flex;
                    node.left = px(at.x
                        + if i % 2 == 0 {
                            -half.x - 6.
                        } else {
                            half.x - 7.
                        });
                    node.top = px(at.y + if i < 2 { -half.y - 6. } else { half.y - 7. });
                }
                Part::Ray(_) | Part::Emitter(_) | Part::Signal(_) | Part::FrameEdge(..) | Part::Spoke(_) | Part::Vertex(..) | Part::SourceSpot => {}
                Part::FoldToggle => {}
                Part::Restore => {
                    let badge=projection.and_then(|(_,at,half,viewport,_,_)|projection::place(at,half,viewport,Vec2::new(140.,28.),queue_height));
                    node.display=if state.collapsed && state.motion.aperture==0. && badge.is_some() {Display::Flex}else{Display::None};
                    if let Some(p)=badge {node.left=px(p.x);node.top=px(p.y);}
                }
                Part::Scan => {
                    node.display=if state.motion.work {Display::Flex}else{Display::None};
                    if state.motion.work {
                        let phase = time.as_ref().map_or(0., |t| (t.elapsed_secs() / 9.).fract());
                        node.top = percent(phase * 98.);
                    }
                }
                Part::CatalogToggle => {
                    node.display = if details.is_some() { Display::Flex } else { Display::None };
                }
                Part::ToolsToggle => {}
                Part::Tooltip => {
                    node.display = if readable && tooltip.is_some() { Display::Flex } else { Display::None };
                    let viewport=projection.unwrap().3;
                    let (left,top,width)=if viewport.y<760. {
                        compact_detail(viewport,position,zoom)
                    } else {
                        (0.,tooltip_top(position,panel_size.y+if projected_recipes{SHELF_HEIGHT+12.}else{0.},projection_viewport(viewport),zoom,tooltip_height),panel_size.x)
                    };
                    node.width=px(width);
                    node.max_height=if viewport.y<780. {px(112.)} else {Val::Auto};
                    node.overflow=Overflow::scroll_y();
                    node.left=px(position.x+left*zoom+(zoom-1.)*width*0.5);
                    node.top=px(position.y+top*zoom+(zoom-1.)*tooltip_height*0.5);
                    if let Some(mut t)=transform {if t.scale!=Vec2::splat(zoom) {t.scale=Vec2::splat(zoom);}}
                }
                #[cfg(test)]
                Part::QueueRow | Part::Production => {
                    node.display = Display::None;
                }
                Part::Recipe(i) => {
                    node.display = if projected_recipes && details.as_ref().is_some_and(|d| i < d.recipes.len()) {
                        Display::Flex
                    } else {
                        Display::None
                    }
                }
                Part::Command(a) => {
                    node.display = if command_visible(
                        a,
                        unit.kind,
                        unit.status.weapon,
                        scene.sim.battle.phase == sim_core::blast::battle::Phase::Fighting,
                    ) && (!secondary(a) || state.tools_open) {
                        Display::Flex
                    } else {
                        Display::None
                    }
                }
                Part::Pip(i) => {
                    if let Some(ref mut bg) = background {
                        let next=if i * 5 < integrity { CYAN } else { FRAME };
                        if bg.0!=next {bg.0=next;}
                    }
                }
            }
            if matches!(part, Part::Recipe(_) | Part::Command(_) | Part::CatalogToggle | Part::ToolsToggle) {
                let active = background
                    .as_ref()
                    .is_some_and(|b| b.0 == panel::theme::OPTICAL.active)
                    || matches!(part,Part::Command(Action::CrawlerMode(m)) if *m==panel.crawler_mode)
                    || matches!(part,Part::Command(Action::Ranges) if panel.ranges)
                    || matches!(part,Part::Command(Action::Fire|Action::NuclearLaunch) if panel.ground_fire)
                    || matches!(part, Part::CatalogToggle if state.catalog_open)
                    || matches!(part, Part::ToolsToggle if state.tools_open);
                let hover = interaction.is_some_and(|i| *i != Interaction::None);
                if let Some(mut b) = border {
                    let next=BorderColor::all(if active || hover { CYAN } else { FRAME });
                    if *b!=next {*b=next;}
                }
                if let Some(ref mut b) = background {
                    let next = if active {
                        panel::theme::OPTICAL.active
                    } else if hover { panel::theme::OPTICAL.hover } else {
                        Color::NONE
                    };
                    if b.0!=next {b.0=next;}
                }
            }
            // Drawing is emitted once, not once per UI entity.
            if matches!(part, Part::Deck) {
                let y = base.y + 1.;
                let extent = Vec2::new(size.x, size.z) * 0.55;
                let leg = extent.min_element() * 0.35;
                for x in [-1., 1.] {
                    for z in [-1., 1.] {
                        let c = Vec3::new(base.x + x * extent.x, y, base.z + z * extent.y);
                        giz.line(c, c - Vec3::X * x * leg, CYAN);
                        giz.line(c, c - Vec3::Z * z * leg, CYAN);
                        giz.line(c, c + Vec3::Y * 5., CYAN);
                    }
                }
            }
        } else if matches!(part, Part::Deck | Part::CatalogShelf | Part::Tooltip | Part::Restore | Part::Corner(_) | Part::Ray(_) | Part::Signal(_) | Part::Emitter(_)) {
            if node.display != Display::None { node.display = Display::None; }
        }
        target_node.set_if_neq(node);
    }
    for (caption, mut text) in &mut texts {
        match caption {
            Caption::Toggle => {
                let s = if state.enabled { "У объекта · F8" } else { "Панель HUD · F8" };
                if text.0 != s { text.0 = s.into(); }
                continue;
            }
            Caption::Tooltip => {
                let s = tooltip.as_deref().unwrap_or("");
                if text.0 != s { text.0 = s.into(); }
                continue;
            }
            Caption::CatalogToggle => {
                let s = if state.catalog_open { "Свернуть" } else { "Строить" };
                if text.0 != s { text.0 = s.into(); }
                continue;
            }
            Caption::Name(i) => {
                let s = details
                    .as_ref()
                    .and_then(|d| d.recipes.get(*i))
                    .map_or("", |r| construction::name(r.kind));
                if text.0 != s { text.0 = s.into(); }
                continue;
            }
            Caption::Title | Caption::Status => {}
        }
        let value = match caption {
            Caption::Title => projection.map_or(String::new(), |(id, _, _, _, _, _)| {
                format!(
                    "{} · {:02} / T{}",
                    construction::name(scene.sim.units[id].kind).to_uppercase(),
                    id + 1,
                    construction::tier(scene.sim.units[id].kind)
                )
            }),
            Caption::Status => projection.map_or(String::new(), |(id, _, _, _, _, _)| {
                let group = edit.battle_selected_ids(&scene.sim).len();
                let cargo = snapshot
                    .crawler_cargo
                    .get(id)
                    .copied()
                    .flatten()
                    .map(|(held, cap, _, _, _, _, mode)| {
                        format!(
                            "М {} / {} · {mode}",
                            economy_display::mass(held),
                            economy_display::mass(cap)
                        )
                    })
                    .or_else(|| {
                        snapshot
                            .base_cargo
                            .get(id)
                            .copied()
                            .flatten()
                            .map(|(held, cap)| {
                                format!(
                                    "М {} / {}",
                                    economy_display::mass(held),
                                    economy_display::mass(cap)
                                )
                            })
                    })
                    .unwrap_or_else(|| format!("Ядро {integrity}% · выбрано {group}"));
                let power = snapshot
                    .energy_local
                    .get(id)
                    .copied()
                    .flatten()
                    .map(|(have, need)| {
                        format!(
                            " · лок. сеть +{} / −{}",
                            economy_display::energy(have),
                            economy_display::energy(need)
                        )
                    })
                    .unwrap_or_default();
                format!("{cargo}{power}")
            }),
            Caption::Toggle | Caption::Tooltip | Caption::CatalogToggle | Caption::Name(_) => unreachable!(),
        };
        if text.0 != value {
            text.0 = value;
        }
    }
    for (visual, mut image) in &mut visuals {
        let handle = match visual {
            Visual::Recipe(i) => details
                .as_ref()
                .and_then(|d| d.recipes.get(*i))
                .map(|r| r.image.clone()),
        }
        .unwrap_or_default();
        let settled=if handle==Handle::default(){Color::NONE}else{Color::WHITE};
        if image.image!=handle || image.color!=settled {optical_ui::preview_image(&mut image,handle);}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hover_pins_the_final_safe_rectangle_and_releases_it_if_the_hull_moves_under_it() {
        let mut s=State::default();let viewport=Vec2::new(1600.,900.);let half=Vec2::new(60.,50.);let size=Vec2::new(328.,115.);
        let p=s.place(0,Vec2::new(500.,330.),half,viewport,size,140.,false).unwrap();
        assert_eq!(s.pinned,Some((0,p)));
        assert_eq!(s.place(0,Vec2::new(505.,330.),half,viewport,size,140.,true),Some(p));
        let moved=s.place(0,Vec2::new(600.,330.),half,viewport,size,140.,true).unwrap();
        assert_ne!(moved,p);assert_eq!(s.pinned,Some((0,moved)));
        assert!(projection::allowed(moved,Vec2::new(600.,330.),half,viewport,size,140.));
    }
    #[test]
    fn projector_system_queries_initialize_without_aliasing_the_global_production_root() {
        use bevy::ecs::system::{IntoSystem,System};
        let mut system=IntoSystem::into_system(update);
        system.initialize(&mut World::new());
    }
    #[test]
    fn actual_fold_restore_controls_and_the_bounded_decor_pool_keep_input_separate() {
        let mut app=App::new();
        app.init_resource::<State>().init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(frontend::Menu::new(false)).insert_resource(laboratory::Laboratory::new(false))
            .init_resource::<pilot::Pilot>().add_systems(Update,input);
        let camera=app.world_mut().spawn_empty().id();
        chrome::build(&mut app.world_mut().commands(),camera,&TextFont::default(),&mut Assets::default(),&mut Assets::default());
        app.world_mut().flush();
        let mut decorative=0;
        for (part,button,root,focus) in app.world_mut().query::<(&Part,Has<Button>,Has<panel::PanelRoot>,Option<&FocusPolicy>)>().iter(app.world()) {
            if matches!(part,Part::FrameEdge(..)|Part::Vertex(..)|Part::Ray(_)|Part::Signal(_)|Part::Spoke(_)|Part::Emitter(_)|Part::SourceSpot) {
                decorative+=1;assert!(!button && !root);assert_eq!(focus,Some(&FocusPolicy::Pass));
            }
        }
        assert_eq!(decorative,43,"fixed pool, independent of actor count");
        let tip=app.world_mut().query::<(Entity,&Part)>().iter(app.world()).find_map(|(e,p)|(*p==Part::Tooltip).then_some(e)).unwrap();
        assert!(app.world().get::<ChildOf>(tip).is_none(),"tooltip must remain outside the command scroll clip");
        assert!(app.world().get::<UiTargetCamera>(tip).is_some());
        let fold=app.world_mut().query::<(Entity,&Part)>().iter(app.world()).find_map(|(e,p)|(*p==Part::FoldToggle).then_some(e)).unwrap();
        let restore=app.world_mut().query::<(Entity,&Part)>().iter(app.world()).find_map(|(e,p)|(*p==Part::Restore).then_some(e)).unwrap();
        *app.world_mut().get_mut::<Interaction>(fold).unwrap()=Interaction::Pressed;
        app.update();assert!(app.world().resource::<State>().collapsed);
        *app.world_mut().get_mut::<Interaction>(fold).unwrap()=Interaction::None;
        *app.world_mut().get_mut::<Interaction>(restore).unwrap()=Interaction::Pressed;
        app.update();assert!(!app.world().resource::<State>().collapsed);
    }
    #[test]
    fn compact_world_details_clear_the_fixed_catalogue_and_minimap() {
        for viewport in [Vec2::new(960.,540.),Vec2::new(1280.,720.),Vec2::new(1600.,900.)] {
            let size=Vec2::new(WIDTH,120.);
            let position=panel_position(viewport*0.5,Vec2::splat(40.),projection_viewport(viewport),size);
            let dock_top=viewport.y-construction::HEIGHT-112.;
            assert!(position.y+size.y<=dock_top-8.,"projection cannot cover fixed categories");
            if viewport.y<760. {
                let (left,top,width)=compact_detail(viewport,position,1.);
                let at=position+Vec2::new(left,top);
                assert!(at.x+width<186.,"detail island stays left of catalogue cards");
                assert!(at.y>=position.y+size.y,"detail does not cover world controls");
                assert!(at.y+92.<=viewport.y-192.,"detail stays above the fixed minimap");
            }
        }
    }
    #[test]
    fn projector_origin_lies_on_the_occupied_roof_above_the_factory_core() {
        for body in [voxel::unit::kil_factory(1),voxel::unit::tank(),voxel::unit::spider(),voxel::unit::strider(),voxel::unit::walker04(),voxel::unit::commander()] {
        let [x, y, z] = projector_origin(&body).map(|v| v as usize);
        assert!(body.vol.get(x, y, z));
        assert!((y + 1..usize::from(body.vol.ny)).all(|above| !body.vol.get(x, above, z)));
        }
    }
    #[test]
    fn optical_disclosure_resets_on_actor_and_scene_changes() {
        let mut state = State::initial(true, false);
        state.inspect(Some((1,0)));
        assert!(state.catalog_open);
        state.inspect(Some((1,0)));
        assert!(state.catalog_open);
        state.inspect(Some((1,1)));
        assert!(!state.catalog_open);
        state.tools_open = true;
        state.inspect(Some((2,1)));
        assert!(!state.tools_open);
        state.catalog_open = true;
        state.inspect(None);
        assert!(!state.catalog_open);
    }
    #[test]
    fn hologram_preview_preserves_dark_geometry_and_canvas_coverage() {
        use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
        let source = Image::new(Extent3d {width:2,height:2,depth_or_array_layers:1},
            TextureDimension::D2, vec![0,0,0,255, 12,30,40,0, 4,4,4,255, 200,220,240,255],
            TextureFormat::Rgba8UnormSrgb, bevy::asset::RenderAssetUsages::default());
        let optical = hologram_thumbnail(source.clone());
        let pixels = optical.data.as_ref().unwrap();
        assert_eq!(pixels.chunks_exact(4).filter(|p| p[3] > 0).count(), 3);
        assert!(pixels[3] > 0, "black model geometry must survive");
        assert_eq!(pixels[7], 0, "only the canvas is transparent");
        let legacy = laboratory::visuals::thumbnail_canvas(source);
        let pixels = legacy.data.as_ref().unwrap();
        assert_eq!(&pixels[..4], &[0,0,0,255]);
        assert_eq!(&pixels[4..8], &[20,29,37,255]);
        assert_eq!(&pixels[12..], &[200,220,240,255]);
    }
    #[test]
    fn tooltip_stays_inside_small_and_large_viewports() {
        for (position, h, viewport) in [
            (Vec2::new(100.,100.), 140., Vec2::new(1920.,1080.)),
            (Vec2::new(100.,800.), 140., Vec2::new(1920.,1080.)),
            (Vec2::new(100.,164.), 310., Vec2::new(960.,540.)),
        ] {
            let top = position.y + tooltip_top(position, h, viewport, 1., 90.) + 6.;
            assert!(top >= 82. && top + 90. <= viewport.y - 24.);
        }
    }
    #[test]
    fn optical_controls_disclose_details_without_changing_the_world_mode() {
        let mut app = App::new();
        app.init_resource::<State>().init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(frontend::Menu::new(false))
            .insert_resource(laboratory::Laboratory::new(false))
            .init_resource::<pilot::Pilot>().add_systems(Update, input);
        let catalog = app.world_mut().spawn((Part::CatalogToggle, Interaction::Pressed)).id();
        app.update();
        assert!(app.world().resource::<State>().catalog_open);
        assert!(app.world().resource::<State>().enabled);
        *app.world_mut().get_mut::<Interaction>(catalog).unwrap() = Interaction::None;
        app.world_mut().spawn((Part::ToolsToggle, Interaction::Pressed));
        app.update();
        assert!(app.world().resource::<State>().tools_open);
        assert!(!app.world().resource::<State>().catalog_open);
        app.insert_resource(frontend::Menu::new(true));
        *app.world_mut().get_mut::<Interaction>(catalog).unwrap() = Interaction::Pressed;
        app.update();
        assert!(!app.world().resource::<State>().catalog_open);
    }
    #[test]
    fn optical_deck_stays_compact_at_4k_and_inside_small_viewports() {
        assert_eq!(optical_scale(Vec2::new(3840., 2160.)), 1.25);
        assert!(WIDTH * optical_scale(Vec2::new(3840., 2160.)) <= 410.);
        for viewport in [Vec2::new(960.,540.), Vec2::new(1920.,1080.), Vec2::new(3840.,2160.)] {
            let size = Vec2::new(WIDTH, 300.) * optical_scale(viewport);
            for at in [Vec2::new(100.,200.), viewport * 0.5, viewport - Vec2::new(10.,10.)] {
                let p = panel_position(at, Vec2::splat(20.), viewport, size);
                assert!(p.x >= 16. && p.x + size.x <= viewport.x - 16.);
                assert!(p.y >= 82. && p.y + size.y <= viewport.y - 24.);
            }
        }
        assert!(secondary(Action::FirstPerson));
        assert!(!secondary(Action::CrawlerUnload));
    }
    #[test]
    fn spatial_panel_uses_opposite_side_and_stays_inside_viewport() {
        let viewport = Vec2::new(1920., 1080.);
        let size = Vec2::new(WIDTH, 460.);
        let left = panel_position(Vec2::new(300., 500.), Vec2::splat(40.), viewport, size);
        assert!(left.x > 340.);
        let right = panel_position(Vec2::new(1700., 950.), Vec2::splat(40.), viewport, size);
        assert!(right.x + size.x < 1660.);
        assert!(right.y + size.y <= 1056.);
        let small = panel_position(
            Vec2::new(400., 350.),
            Vec2::splat(80.),
            Vec2::new(960., 540.),
            Vec2::new(WIDTH, 320.),
        );
        assert!(small.x >= 16. && small.x + size.x <= 944.);
    }
    #[test]
    fn spatial_mode_toggle_preserves_selection_and_ignores_main_menu() {
        let mut app = App::new();
        app.init_resource::<State>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(frontend::Menu::new(false))
            .insert_resource(laboratory::Laboratory::new(false))
            .init_resource::<pilot::Pilot>()
            .add_systems(Update, input);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F8);
        app.update();
        assert!(!app.world().resource::<State>().enabled);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F8);
        app.update();
        assert!(app.world().resource::<State>().enabled);
        app.insert_resource(frontend::Menu::new(true));
        app.update();
        assert!(app.world().resource::<State>().enabled);
    }
    #[test]
    fn fixed_catalogue_recipe_uses_the_selected_factory_and_blocks_map_input() {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../../../data/fixture-drive.ron")).unwrap();
        layout.size = [1536, 256, 1536];
        layout.gun = [64, 16, 64];
        layout.target = [1400, 16, 1400];
        layout.clear_min = [1, 1];
        layout.clear_max = [1535, 1535];
        layout.idle_enemies = true;
        layout.units = vec![
            modding::range::RangeUnitDef {
                kind: RangeUnitKind::KilFactoryT1,
                allied: true,
                at: [550, 16, 550],
                weapon: None,
            },
            modding::range::RangeUnitDef {
                kind: RangeUnitKind::Tank,
                allied: false,
                at: [1400, 16, 1400],
                weapon: None,
            },
        ];
        layout.blocks = vec![
            modding::range::RangeBlock {
                min: [0; 3],
                max: [1536, 256, 1536],
                material: 0,
            },
            modding::range::RangeBlock {
                min: [0; 3],
                max: [1536, 16, 1536],
                material: 1,
            },
        ];
        layout.economy = Some(modding::range_economy::RangeEconomy::stock());
        layout.ready = None;
        let mut sim = BlastRange::with_layout(config(false, false), layout.clone()).unwrap();
        sim.set_navigation_workers(1);
        sim.set_physics_workers(1);
        sim.start_battle().unwrap();
        let mut edit = Workshop::new(Some(layout), false);
        edit.controlled = Some(0);
        edit.selected_units = vec![0];
        let mut app = panel::hud::tests::app_with_scene(sim, edit);
        app.init_resource::<construction::Catalog>()
            .insert_resource(laboratory::Laboratory::new(false))
            .init_resource::<pilot::Pilot>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_systems(Update, construction::input.after(panel::actions));
        app.world_mut().resource_mut::<panel::Panel>().sandbox = false;
        let camera = app.world_mut().spawn(Camera2d).id();
        let mut fonts = Assets::<Font>::default();
        let mut images = Assets::<Image>::default();
        build(
            &mut app.world_mut().commands(),
            camera,
            &TextFont::default(),
            &mut fonts,
            &mut images,
        );
        construction::build(&mut app.world_mut().commands(),camera,&TextFont::default());
        app.world_mut().flush();
        assert!(!app.world_mut().query::<&Part>().iter(app.world()).any(|p|matches!(p,Part::Production|Part::QueueRow)),"upper production queue remains the only queue");
        assert_eq!(app.world_mut().query::<&Part>().iter(app.world()).filter(|p|matches!(p,Part::Recipe(_))).count(),4,"real builder must provide optional context recipes");
        let mut camera3d=Camera::default();
        camera3d.computed.target_info=Some(bevy::camera::RenderTargetInfo{physical_size:UVec2::new(1920,1080),scale_factor:1.});
        camera3d.computed.clip_from_view=Mat4::orthographic_rh(-2200.,2200.,-1237.5,1237.5,0.1,10000.);
        app.world_mut().spawn((EditorCamera,camera3d,Transform::from_xyz(1550.,1500.,1950.)
            .looking_at(Vec3::new(550.,100.,550.),Vec3::Y)));
        let mut gizmo_config=bevy::gizmos::config::GizmoConfigStore::default();
        gizmo_config.insert(bevy::gizmos::config::GizmoConfig::default(),bevy::gizmos::config::DefaultGizmoConfigGroup);
        app.insert_resource(gizmo_config).insert_resource(images).insert_resource(workbench::ghost::Templates::default())
            .init_resource::<Time>().init_resource::<UiScale>()
            .init_resource::<State>().insert_resource(frontend::Menu::new(false))
            .add_systems(Update,input.after(construction::input))
            .add_systems(PostUpdate,(construction::refresh,construction::sync_visibility,construction::tooltip,update).chain().after(ui_snapshot::publish));
        app.world_mut().spawn(Window{resolution:(1920,1080).into(),..default()});
        for _ in 0..4 {app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(100));app.update();}
        assert!(app.world().resource::<State>().visible,"real update must place the built projection");
        // Let the bounded image cache and the opening lifecycle settle, then
        // exercise actual visible text/previews rather than only hidden nodes.
        for _ in 0..8 {app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(100));app.update();}
        let quiet_ticks=|world:&mut World|->Vec<_>{world.query_filtered::<Entity,With<Node>>().iter(world).map(|e| {
            let entity=world.entity(e);
            (e,entity.get_ref::<Node>().unwrap().last_changed(),entity.get_ref::<Text>().map(|v|v.last_changed()),
                entity.get_ref::<ImageNode>().map(|v|v.last_changed()))
        }).collect()};
        let quiet=quiet_ticks(app.world_mut());
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(50));app.update();
        for (before,after) in quiet.iter().zip(quiet_ticks(app.world_mut())) {
            let entity=app.world().entity(before.0);
            let part=entity.get::<Part>().map(std::mem::discriminant);
            let catalog=entity.get::<construction::CatalogPart>().map(std::mem::discriminant);
            let root=entity.contains::<construction::Root>();let aux=entity.contains::<construction::AuxRoot>();
            let node=entity.get::<Node>().unwrap();
            assert_eq!(*before,after,"visible idle write: part={part:?} catalog={catalog:?} root={root} aux={aux} position={:?}/{:?} size={:?}/{:?}",node.left,node.top,node.width,node.height);
        }
        let toggle=app.world_mut().query::<(Entity,&Part)>().iter(app.world())
            .find_map(|(e,p)|(*p==Part::CatalogToggle).then_some(e)).expect("production builder creates toggle");
        let more=app.world_mut().query::<(Entity,&Part)>().iter(app.world())
            .find_map(|(e,p)|(*p==Part::ToolsToggle).then_some(e)).unwrap();
        assert_eq!(app.world().get::<Node>(more).unwrap().display,Display::Flex,"initial projection must retain its secondary commands toggle");
        let first=app.world_mut().query::<(Entity,&Part)>().iter(app.world())
            .find_map(|(e,p)|(*p==Part::Recipe(0)).then_some(e)).unwrap();
        let caption=app.world_mut().query::<(Entity,&Caption,&ChildOf)>().iter(app.world())
            .find_map(|(e,c,parent)|(matches!(c,Caption::Name(0))&&parent.parent()==first).then_some(e)).unwrap();
        let picture=app.world_mut().query::<(Entity,&Visual,&ChildOf)>().iter(app.world())
            .find_map(|(e,v,parent)|(matches!(v,Visual::Recipe(0))&&parent.parent()==first).then_some(e)).unwrap();
        assert_eq!(app.world().get::<Node>(first).unwrap().display,Display::None);
        let row=app.world_mut().query::<(Entity,&construction::CatalogPart)>().iter(app.world())
            .find_map(|(e,p)|matches!(p,construction::CatalogPart::Row).then_some(e)).unwrap();
        assert_eq!(app.world().get::<Node>(row).unwrap().display,Display::Flex);
        let rectangles=|world:&mut World|->Vec<_>{world.query::<(Entity,&Part,&Node)>().iter(world)
            .filter(|(_,p,_)|matches!(p,Part::Command(_)|Part::Deck|Part::CatalogToggle|Part::ToolsToggle)).map(|(e,_,n)|(e,n.left,n.top,n.width,n.height,n.padding)).collect()};
        let before=rectangles(app.world_mut());
        *app.world_mut().get_mut::<Interaction>(toggle).unwrap()=Interaction::Pressed;
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::ZERO);
        app.update();
        assert!(app.world().resource::<State>().catalog_open);
        assert_eq!(app.world().get::<Node>(more).unwrap().display,Display::Flex,"opening recipes must retain secondary commands");
        assert_eq!(app.world().get::<Node>(first).unwrap().display,Display::Flex);
        assert_eq!(app.world().get::<Node>(row).unwrap().display,Display::None,"desktop uses one recipe surface at a time");
        assert_eq!(app.world().get::<Text>(caption).unwrap().0,construction::name(RangeUnitKind::Tank));
        let preview={let scene=app.world().resource::<Scene>();
            construction::spatial_catalog(app.world().resource::<construction::Catalog>(),&scene.sim,scene.generation,0)
                .unwrap().recipes[0].image.clone()};
        assert_ne!(preview,Handle::<Image>::default(),"use a real cached model preview");
        assert_eq!(app.world().get::<ImageNode>(picture).unwrap().image,preview);
        assert_eq!(app.world().get::<ImageNode>(picture).unwrap().color,Color::WHITE);
        assert_eq!(rectangles(app.world_mut()),before,"disclosure must preserve command hit layout");
        *app.world_mut().get_mut::<Interaction>(toggle).unwrap()=Interaction::None;
        *app.world_mut().get_mut::<Interaction>(first).unwrap()=Interaction::Hovered;
        for _ in 0..3 {app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(70));app.update();}
        let tip=app.world_mut().query_filtered::<&Node,With<construction::RecipeTooltip>>().single(app.world()).unwrap();
        assert_eq!(tip.display,Display::Flex,"projected hover must use the real quote island");
        let quote=app.world_mut().query_filtered::<&Text,With<construction::RecipeTooltipText>>().single(app.world()).unwrap();
        assert!(quote.0.contains("М ")&&quote.0.contains("Э "));
        assert!(app.world_mut().query::<(&Part,&Node)>().iter(app.world()).all(|(p,n)|*p!=Part::Tooltip||n.display==Display::None),"never cover recipes with a second quote");
        assert_eq!(rectangles(app.world_mut()),before,"hover price must not move controls");
        let first = app
            .world_mut()
            .query::<(Entity, &Part)>()
            .iter(app.world())
            .find_map(|(e, p)| matches!(p, Part::Recipe(0)).then_some(e))
            .unwrap();
        let _root = app
            .world_mut()
            .query::<(Entity, &Part)>()
            .iter(app.world())
            .find_map(|(e, p)| matches!(p, Part::Deck).then_some(e))
            .unwrap();
        *app.world_mut()
            .get_mut::<RelativeCursorPosition>(first)
            .unwrap() = RelativeCursorPosition {
            cursor_over: true,
            normalized: Some(Vec2::splat(0.5)),
        };
        *app.world_mut().get_mut::<Interaction>(first).unwrap() = Interaction::Pressed;
        app.update();
        assert!(
            app.world().resource::<panel::Panel>().blocked,
            "world panel must consume pointer input"
        );
        assert_eq!(
            app.world().resource::<Scene>().sim.production_queue(0),
            Some(vec![RangeUnitKind::Tank]),
            "fixed catalogue uses the existing selected-actor queue"
        );
        *app.world_mut().get_mut::<Interaction>(first).unwrap()=Interaction::None;
        *app.world_mut().get_mut::<Interaction>(toggle).unwrap()=Interaction::Pressed;
        app.update();
        assert!(!app.world().resource::<State>().catalog_open);
        assert_eq!(app.world().get::<Node>(more).unwrap().display,Display::Flex);
        assert_eq!(app.world().get::<Node>(first).unwrap().display,Display::None);
        assert_eq!(app.world().get::<Node>(row).unwrap().display,Display::Flex,"closing must restore the ordinary palette");
        assert_eq!(rectangles(app.world_mut()),before,"catalogue close must preserve command hit layout");
        *app.world_mut().get_mut::<Interaction>(toggle).unwrap()=Interaction::None;
        let toggle = app.world_mut().query::<(Entity, &Part)>().iter(app.world())
            .find_map(|(e,p)| matches!(p, Part::ToolsToggle).then_some(e)).unwrap();
        *app.world_mut().get_mut::<Interaction>(first).unwrap() = Interaction::None;
        *app.world_mut().get_mut::<Interaction>(toggle).unwrap() = Interaction::Pressed;
        app.update();
        assert_eq!(app.world().resource::<Scene>().sim.production_queue(0), Some(vec![RangeUnitKind::Tank]),
            "disclosing world controls must not cancel queued units");
        assert!(!app.world().resource::<State>().catalog_open);
        assert_eq!(app.world().get::<Node>(first).unwrap().display,Display::None);
        assert_eq!(rectangles(app.world_mut()),before,"closing must preserve command hit layout");
        *app.world_mut().get_mut::<Interaction>(toggle).unwrap()=Interaction::None;
        app.world_mut().resource_mut::<State>().catalog_open=true;app.update();
        let shelf=app.world_mut().query::<(Entity,&Part)>().iter(app.world())
            .find_map(|(e,p)|matches!(p,Part::CatalogShelf).then_some(e)).unwrap();
        assert_eq!(app.world().get::<Node>(shelf).unwrap().display,Display::Flex);
        app.world_mut().resource_mut::<Workshop>().controlled=None;
        app.world_mut().resource_mut::<Workshop>().selected_units.clear();
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(100));app.update();
        assert_eq!(app.world().get::<Node>(shelf).unwrap().display,Display::None,"selection loss must retire the independent shelf");
    }
    #[test]
    fn spatial_commands_match_capabilities() {
        assert!(command_visible(
            Action::CrawlerUnload,
            RangeUnitKind::Crawler,
            false,
            true
        ));
        assert!(!command_visible(
            Action::CrawlerUnload,
            RangeUnitKind::Tank,
            true,
            true
        ));
        assert!(!command_visible(
            Action::Fire,
            RangeUnitKind::NuclearSilo,
            true,
            true
        ));
        assert!(!command_visible(
            Action::NuclearLaunch,
            RangeUnitKind::NuclearSilo,
            true,
            false
        ));
        assert!(!command_visible(
            Action::BattleWeapon(None),
            RangeUnitKind::Commander,
            true,
            true
        ));
    }
    #[test]
    fn idle_combat_hud_refresh_does_not_dirty_layout_nodes() {
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../../../data/fixture-drive.ron")).unwrap();
        layout.size = [512, 128, 512];
        layout.gun = [64, 16, 64];
        layout.target = [400, 16, 400];
        layout.clear_min = [1, 1];
        layout.clear_max = [511, 511];
        layout.idle_enemies = true;
        layout.units = vec![
            modding::range::RangeUnitDef {
                kind: RangeUnitKind::Tank,
                allied: true,
                at: [160, 16, 160],
                weapon: None,
            },
            modding::range::RangeUnitDef {
                kind: RangeUnitKind::Tank,
                allied: false,
                at: [380, 16, 380],
                weapon: None,
            },
        ];
        layout.blocks = vec![
            modding::range::RangeBlock { min: [0; 3], max: [512, 128, 512], material: 0 },
            modding::range::RangeBlock { min: [0; 3], max: [512, 16, 512], material: 1 },
        ];
        layout.economy = Some(modding::range_economy::RangeEconomy::stock());
        layout.ready = None;
        let mut sim = BlastRange::with_layout(config(false, false), layout.clone()).unwrap();
        sim.set_navigation_workers(1);
        sim.set_physics_workers(1);
        sim.start_battle().unwrap();
        let edit = Workshop::new(Some(layout), false);
        let mut app = panel::hud::tests::app_with_scene(sim, edit);
        app.init_resource::<construction::Catalog>()
            .insert_resource(laboratory::Laboratory::new(false))
            .init_resource::<pilot::Pilot>()
            .insert_resource(Assets::<Image>::default())
            .insert_resource(workbench::ghost::Templates::default())
            .insert_resource(frontend::Menu::new(false))
            .init_resource::<Time>()
            .init_resource::<UiScale>()
            .init_resource::<State>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ui_snapshot::UiSnapshot>()
            .add_systems(
                PostUpdate,
                (
                    construction::refresh,
                    construction::responsive,
                    construction::sync_visibility,
                    construction::tooltip,
                    update,
                )
                    .chain(),
            );
        app.world_mut()
            .get_resource_or_init::<bevy::gizmos::config::GizmoConfigStore>()
            .insert(bevy::gizmos::config::GizmoConfig::default(), DefaultGizmoConfigGroup);
        app.world_mut().resource_mut::<panel::Panel>().sandbox = false;
        app.world_mut().spawn(Window {
            resolution: (960, 540).into(),
            ..default()
        });
        let camera = app.world_mut().spawn(Camera2d).id();
        let mut fonts = Assets::<Font>::default();
        let mut images = Assets::<Image>::default();
        build(
            &mut app.world_mut().commands(),
            camera,
            &TextFont::default(),
            &mut fonts,
            &mut images,
        );
        construction::build(&mut app.world_mut().commands(), camera, &TextFont::default());
        app.world_mut().flush();
        for (width,height,scale) in [(960,540,1.),(1920,1080,1.),(3840,2160,2.)] {
            app.world_mut().query::<&mut Window>().single_mut(app.world_mut()).unwrap().resolution=(width,height).into();
            app.world_mut().resource_mut::<UiScale>().0=scale;
            app.update();app.update();
            let mut ticks = Vec::new();
            for e in app.world_mut().query_filtered::<Entity, With<Node>>().iter(app.world()) {
                let entity=app.world().entity(e);
                ticks.push((e,entity.get_ref::<Node>().unwrap().last_changed(),
                    entity.get_ref::<Text>().map(|v|v.last_changed()),entity.get_ref::<ImageNode>().map(|v|v.last_changed())));
            }
            assert!(ticks.len() >= 80);
            for delta in [50,0] {
                app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(delta));app.update();
                for (e,node,text,image) in &ticks {
                    let entity=app.world().entity(*e);
                    assert_eq!(entity.get_ref::<Node>().unwrap().last_changed(),*node,"idle Node at{width}x{height} on{e:?}");
                    assert_eq!(entity.get_ref::<Text>().map(|v|v.last_changed()),*text,"idle Text on{e:?}");
                    assert_eq!(entity.get_ref::<ImageNode>().map(|v|v.last_changed()),*image,"idle Image on{e:?}");
                }
            }
        }
    }

    #[test]
    fn spatial_catalog_tracks_work_state_for_factory_and_engineer() {
        let mut app = construction::tests::app();
        app.world_mut().resource_mut::<Workshop>().controlled = Some(1);
        app.world_mut().resource_mut::<Workshop>().selected_units = vec![1];
        app.update();
        let idle = {
            let scene = app.world().resource::<Scene>();
            construction::spatial_catalog(app.world().resource::<construction::Catalog>(), &scene.sim, scene.generation, 1).unwrap()
        };
        assert!(!idle.has_work);
        assert!(!idle.recipes.is_empty());
        assert!(app.world_mut().resource_mut::<Scene>().sim.execute_command(
            sim_core::blast::battle::commands::Command::Produce { actor: 1, kind: RangeUnitKind::Tank }
        ).accepted());
        let busy = {
            let scene = app.world().resource::<Scene>();
            construction::spatial_catalog(app.world().resource::<construction::Catalog>(), &scene.sim, scene.generation, 1).unwrap()
        };
        assert!(busy.has_work);
        assert!(app.world_mut().resource_mut::<Scene>().sim.execute_command(
            sim_core::blast::battle::commands::Command::Cancel { actor: 1 }
        ).accepted());
        let cleared = {
            let scene = app.world().resource::<Scene>();
            construction::spatial_catalog(app.world().resource::<construction::Catalog>(), &scene.sim, scene.generation, 1).unwrap()
        };
        assert!(!cleared.has_work);
    }

}
