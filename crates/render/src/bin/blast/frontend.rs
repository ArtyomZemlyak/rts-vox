//! Application navigation. Modes keep using their existing scene/control paths.
use super::*;
use bevy::camera::RenderTarget;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::ui::FocusPolicy;
use bevy::window::{CursorGrabMode, CursorOptions};
use panel::theme;

/// Bevy's atmosphere probes need an active 3D view to allocate their shared
/// GPU uniforms. A tiny offscreen target keeps those resources valid while
/// the menu covers the window, without drawing a full-resolution world.
#[derive(Resource, Default)]
pub struct MenuTarget {
    image: Handle<Image>,
    original: Option<RenderTarget>,
}
pub fn setup_menu_target(commands: &mut Commands, images: &mut Assets<Image>) {
    let image = images.add(Image::new_target_texture(
        32,
        32,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    commands.insert_resource(MenuTarget {
        image,
        original: None,
    });
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Page {
    Session,
    Home,
    BattleRoom,
    Settings,
    Help,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Destination {
    Range,
    Battle,
    CommanderBattle,
    CommanderSpectate,
    Laboratory,
    Spectate,
}
impl Destination {
    fn command(self) -> panel::Action {
        match self {
            Self::Range => panel::Action::RangeMode,
            Self::Battle => panel::Action::BattleWorld,
            Self::CommanderBattle => panel::Action::BattleCommanders,
            Self::CommanderSpectate => panel::Action::BattleCommandersAuto,
            Self::Laboratory => panel::Action::Laboratory,
            Self::Spectate => panel::Action::BattleWorldAuto,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BattleSetup {
    pub player_southeast: bool,
    pub player_stock: u64,
    pub enemy_stock: u64,
    pub spectator: bool,
}
impl Default for BattleSetup {
    fn default() -> Self {
        Self { player_southeast: false, player_stock: modding::range_economy::Balance::stock().initial_matter, enemy_stock: modding::range_economy::Balance::stock().initial_matter, spectator: false }
    }
}
fn next_stock(value: u64) -> u64 {
    match value { 25_000_000 => 50_000_000, 50_000_000 => 75_000_000, 75_000_000 => 100_000_000, _ => 25_000_000 }
}
#[derive(Resource)]
pub struct Menu {
    page: Page,
    has_session: bool,
    paused_before: bool,
    scale_before: f32,
    pending: Option<Destination>,
    launching: Option<(u64, bool)>,
    panel_command: Option<panel::Action>,
    lab_command: Option<laboratory::LabAction>,
    notice: String,
    battle_setup: BattleSetup,
}
impl Menu {
    pub fn new(open: bool) -> Self {
        Self {
            page: if open { Page::Home } else { Page::Session },
            has_session: !open,
            paused_before: false,
            scale_before: 1.0,
            pending: None,
            launching: None,
            panel_command: None,
            lab_command: None,
            notice: String::new(),
            battle_setup: BattleSetup::default(),
        }
    }
    pub fn shown(&self) -> bool {
        self.page != Page::Session
    }
    pub fn settings(&self) -> bool {
        self.page == Page::Settings
    }
    pub fn panel_command(&self) -> Option<panel::Action> {
        self.panel_command
    }
    pub fn battle_setup(&self) -> BattleSetup { self.battle_setup }
    pub fn lab_command(&self) -> Option<laboratory::LabAction> {
        self.lab_command
    }
    fn open(&mut self, scene: &mut Scene, scale: f32) {
        if !self.shown() {
            self.paused_before = scene.paused;
            self.scale_before = scale;
        }
        self.page = Page::Home;
        self.notice.clear();
        scene.paused = true;
        scene.acc = 0.0;
    }
    fn resume(&mut self, scene: &mut Scene, scale: &mut UiScale) {
        if !self.has_session {
            return;
        }
        self.page = Page::Session;
        scene.paused = self.paused_before;
        scene.acc = 0.0;
        scene.skip_delta = true;
        scale.0 = self.scale_before;
    }
}

pub fn requested(args: &[String]) -> bool {
    if args.iter().any(|a| a == "--main-menu") {
        return true;
    }
    !args.iter().any(|a| {
        matches!(
            a.as_str(),
            "--skip-menu"
                | "--capture"
                | "--shots"
                | "--profile"
                | "--record"
                | "--load"
                | "--laboratory"
                | "--settings"
                | "--edit"
                | "--battle"
                | "--battle-eight"
                | "--world-battle"
                | "--commander-battle"
                | "--skirmish"
                | "--drive-demo"
                | "--move"
                | "--attack-unit"
        )
    })
}
pub fn in_session(menu: Option<Res<Menu>>) -> bool {
    menu.is_none_or(|m| !m.shown())
}
pub fn can_scroll(menu: Option<Res<Menu>>) -> bool {
    menu.is_none_or(|m| !m.shown() || m.settings())
}

#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Menu,
    Resume,
    Start(Destination),
    StartPreparedBattle,
    RoomPosition,
    RoomPlayerStock,
    RoomEnemyStock,
    RoomSpectator,
    Settings,
    Help,
    Back,
    Exit,
}
#[derive(Component)]
pub(super) struct Root;
#[derive(Component)]
pub(super) struct Part(Page);
#[derive(Component)]
pub(super) struct Resume;
#[derive(Component)]
pub(super) struct Notice;
#[derive(Component)]
pub(super) struct RoomValue(u8);

/// Runs before any game input. Commands remain readable by all existing mode
/// handlers for this frame, then clear before the next frame's dispatch.
pub fn input(
    mut menu: ResMut<Menu>,
    mut scene: ResMut<Scene>,
    edit: Res<Workshop>,
    mut knobs: ResMut<knobs::Overrides>,
    lab: Res<laboratory::Laboratory>,
    mut pilot: ResMut<pilot::Pilot>,
    mut scale: ResMut<UiScale>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut motion: ResMut<AccumulatedMouseMotion>,
    buttons: Query<(&Action, &Interaction), Changed<Interaction>>,
    mut cursors: Query<&mut CursorOptions>,
    mut exit: MessageWriter<AppExit>,
    catalog: Option<Res<construction::Catalog>>,
) {
    menu.panel_command = None;
    menu.lab_command = None;
    if let Some((generation, had_session)) = menu.launching {
        if edit.pending.is_none() {
            menu.launching = None;
            if scene.generation == generation {
                menu.has_session = had_session;
                menu.open(&mut scene, scale.0);
                menu.notice = if scene.notice.is_empty() {
                    "Не удалось открыть режим. Выберите другой режим или повторите запуск.".into()
                } else {
                    scene.notice.clone()
                };
            }
        }
    }
    let mut action = buttons
        .iter()
        .find_map(|(a, i)| (*i == Interaction::Pressed).then_some(*a));
    if keys.just_pressed(KeyCode::F10) {
        action = match menu.page {
            Page::Session => Some(Action::Menu),
            Page::Settings | Page::Help | Page::BattleRoom => Some(Action::Back),
            Page::Home => menu.has_session.then_some(Action::Resume),
        };
    }
    if keys.just_pressed(KeyCode::Escape) && action.is_none() {
        action = match menu.page {
            Page::Home => menu.has_session.then_some(Action::Resume),
            Page::Settings if knobs.editing.is_some() => None,
            Page::Settings | Page::Help | Page::BattleRoom => Some(Action::Back),
            Page::Session if knobs.open => {
                knobs.open = false;
                keys.reset(KeyCode::Escape);
                None
            }
            Page::Session if catalog.as_ref().is_some_and(|c| c.placing()) => None,
            Page::Session if pilot.active() || lab.escape_cancels_edit() => None,
            Page::Session => Some(Action::Menu),
        };
    }
    if let Some(action) = action {
        match action {
            Action::Menu => {
                menu.open(&mut scene, scale.0);
                knobs.open = false;
                knobs.editing = None;
            }
            Action::Resume => {
                knobs.open = false;
                knobs.editing = None;
                menu.resume(&mut scene, &mut scale);
            }
            Action::Start(destination) => {
                if destination == Destination::Battle {
                    menu.page = Page::BattleRoom;
                } else {
                    menu.pending = Some(destination);
                }
                menu.notice.clear();
            }
            Action::StartPreparedBattle => {
                menu.pending = Some(if menu.battle_setup.spectator { Destination::CommanderSpectate } else { Destination::CommanderBattle });
                menu.notice.clear();
            }
            Action::RoomPosition => menu.battle_setup.player_southeast = !menu.battle_setup.player_southeast,
            Action::RoomPlayerStock => menu.battle_setup.player_stock = next_stock(menu.battle_setup.player_stock),
            Action::RoomEnemyStock => menu.battle_setup.enemy_stock = next_stock(menu.battle_setup.enemy_stock),
            Action::RoomSpectator => menu.battle_setup.spectator = !menu.battle_setup.spectator,
            Action::Settings => {
                menu.page = Page::Settings;
                knobs.open = false;
                knobs.group = knobs::Group::ALL
                    .iter()
                    .position(|g| *g == knobs::Group::Picture)
                    .unwrap();
                menu.panel_command = Some(panel::Action::Settings);
            }
            Action::Help => menu.page = Page::Help,
            Action::Back => {
                menu.page = Page::Home;
                knobs.open = false;
                knobs.editing = None;
            }
            Action::Exit => {
                exit.write(AppExit::Success);
            }
        }
        keys.reset_all();
        mouse.reset_all();
        motion.delta = Vec2::ZERO;
    }
    if let Some(destination) = menu.pending {
        knobs.open = false;
        if lab.active() && destination != Destination::Laboratory {
            // Restore the suspended range before asking its normal control
            // path to create another mode; never interpret lab geometry as RTS.
            menu.lab_command = Some(laboratory::LabAction::Back);
        } else if lab.active() {
            menu.pending = None;
            menu.resume(&mut scene, &mut scale);
        } else {
            pilot.leave(&mut scene);
            scene.view = 1;
            menu.pending = None;
            menu.launching = Some((scene.generation, menu.has_session));
            menu.panel_command = Some(destination.command());
            menu.page = Page::Session;
            menu.has_session = true;
            scene.paused = menu.paused_before;
            scene.acc = 0.0;
            scene.skip_delta = true;
            scale.0 = menu.scale_before;
        }
        keys.reset_all();
        mouse.reset_all();
        motion.delta = Vec2::ZERO;
    }
    if menu.shown() {
        scene.paused = true;
        scene.acc = 0.0;
        for mut cursor in &mut cursors {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
        }
    }
}

fn text(p: &mut ChildSpawnerCommands, font: &TextFont, value: &str, size: f32, color: Color) {
    p.spawn((
        Text::new(value),
        TextFont {
            font_size: FontSize::Px(size),
            ..font.clone()
        },
        TextColor(color),
    ));
}
fn button(p: &mut ChildSpawnerCommands, font: &TextFont, label: &str, action: Action) -> Entity {
    let mut node = p.spawn((
        Button,
        action,
        Node {
            min_height: px(48),
            padding: UiRect::axes(px(20), px(12)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border: UiRect::all(px(1)),
            ..default()
        },
        BackgroundColor(theme::BUTTON),
        BorderColor::all(theme::FRAME),
    ));
    node.with_children(|p| {
        let mut text = p.spawn((Text::new(label), TextFont { font_size: FontSize::Px(16.0), ..font.clone() }, TextColor(theme::TEXT)));
        let field = match action {
            Action::RoomPosition => Some(0), Action::RoomPlayerStock => Some(1),
            Action::RoomEnemyStock => Some(2), Action::RoomSpectator => Some(3), _ => None,
        };
        if let Some(field) = field { text.insert(RoomValue(field)); }
    });
    node.id()
}
pub fn entry(p: &mut ChildSpawnerCommands, font: &TextFont) {
    let id = button(p, font, "Меню", Action::Menu);
    p.commands().entity(id).insert(Node {
        min_height: px(36),
        padding: UiRect::axes(px(9), px(6)),
        align_items: AlignItems::Center,
        justify_content: JustifyContent::Center,
        border: UiRect::all(px(1)),
        ..default()
    });
}
fn content() -> Node {
    Node {
        width: percent(88),
        max_width: px(1240),
        flex_direction: FlexDirection::Column,
        row_gap: px(24),
        ..default()
    }
}

pub fn build(commands: &mut Commands, camera: Entity, font: &TextFont) {
    commands.spawn((Root, GlobalZIndex(100), FocusPolicy::Block, UiTargetCamera(camera),
        Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: px(0), bottom: px(0),
            align_items: AlignItems::Center, justify_content: JustifyContent::Center, display: Display::None, ..default() },
        BackgroundColor(Color::srgb(0.018, 0.030, 0.048))))
        .with_children(|root| {
            // A bounded command-grid backdrop; no world rendering or particles.
            for i in 0..14 { root.spawn((Node { position_type: PositionType::Absolute,
                left: percent(i as f32 * 8.0), top: px(0), bottom: px(0), width: px(1), ..default() },
                BackgroundColor(Color::srgba(0.10, 0.20, 0.27, 0.2)))); }
            root.spawn((Part(Page::Home), content())).with_children(|p| {
                text(p, font, "КОМАНДНЫЙ ЦЕНТР / 01", 14.0, theme::ACCENT);
                text(p, font, "RTS VOX", 72.0, theme::TEXT);
                text(p, font, "Кубический мир. Полностью разрушаемая среда.", 20.0, theme::MUTED);
                p.spawn(Node { column_gap: px(18), row_gap: px(18), flex_wrap: FlexWrap::Wrap, ..default() }).with_children(|p| {
                    for (number, name, description, destination) in [
                        ("01", "ПОЛИГОН", "Техника, оружие и готовые испытания", Destination::Range),
                        ("02", "БОЙ ПРОТИВ ИИ", "Управляйте армией и уничтожьте базу противника", Destination::Battle),
                        ("03", "ЛАБОРАТОРИЯ", "Конструктор сцен, материалы, физика и реплеи", Destination::Laboratory),
                    ] {
                        p.spawn((Button, Action::Start(destination), Node {
                            flex_grow: 1.0, flex_basis: px(310), min_width: px(250), min_height: px(210),
                            padding: UiRect::all(px(24)), flex_direction: FlexDirection::Column,
                            justify_content: JustifyContent::SpaceBetween, row_gap: px(16), border: UiRect::all(px(1)), ..default()
                        }, BackgroundColor(theme::BUTTON), BorderColor::all(theme::FRAME)))
                        .with_children(|p| {
                            text(p, font, number, 28.0, theme::ACCENT);
                            text(p, font, name, 23.0, theme::TEXT);
                            text(p, font, description, 16.0, theme::MUTED);
                            text(p, font, "Открыть →", 16.0, Color::srgb(0.42, 0.79, 0.91));
                        });
                    }
                });
                p.spawn(Node { column_gap: px(12), row_gap: px(12), flex_wrap: FlexWrap::Wrap, ..default() }).with_children(|p| {
                    let id = button(p, font, "Продолжить", Action::Resume); p.commands().entity(id).insert(Resume);
                    button(p, font, "Настройки", Action::Settings);
                    button(p, font, "Как играть", Action::Help);
                    button(p, font, "Наблюдать ИИ × ИИ", Action::Start(Destination::Spectate));
                    button(p, font, "Выход", Action::Exit);
                });
                p.spawn((Notice, Text::new(""), TextFont {font_size: FontSize::Px(15.0), ..font.clone()}, TextColor(theme::ACCENT)));
                text(p, font, "Карта боя: лесной полигон     •     F10 — главное меню", 14.0, theme::MUTED);
            });
            root.spawn((Part(Page::BattleRoom), content())).with_children(|p| {
                text(p, font, "НАСТРОЙКА СРАЖЕНИЯ", 40.0, theme::TEXT);
                text(p, font, "КАРТА   Лесной полигон · 1 карта доступна", 21.0, theme::TEXT);
                text(p, font, "СТОРОНА 1  /  ИГРОК", 18.0, theme::ACCENT);
                p.spawn(Node { column_gap: px(12), row_gap: px(8), flex_wrap: FlexWrap::Wrap, ..default() }).with_children(|p| {
                    button(p, font, "Позиция: северо-запад", Action::RoomPosition);
                    button(p, font, "Масса: 500", Action::RoomPlayerStock);
                    button(p, font, "Управление: игрок", Action::RoomSpectator);
                });
                text(p, font, "СТОРОНА 2  /  ПРОТИВНИК: ИИ", 18.0, theme::ACCENT);
                p.spawn(Node { column_gap: px(12), row_gap: px(8), flex_wrap: FlexWrap::Wrap, ..default() }).with_children(|p| {
                    button(p, font, "Масса ИИ: 500", Action::RoomEnemyStock);
                });
                text(p, font, "На старте у каждой стороны один командир. ИИ строит базу и добывает ресурсы.\nПока доступны один противник, две враждующие стороны и единая фракция; отдельные команды и фракции появятся позже.", 16.0, theme::MUTED);
                p.spawn(Node { column_gap: px(12), row_gap: px(8), flex_wrap: FlexWrap::Wrap, ..default() }).with_children(|p| {
                    button(p, font, "НАЧАТЬ БОЙ", Action::StartPreparedBattle);
                    button(p, font, "← Главное меню", Action::Back);
                });
            });
            root.spawn((Part(Page::Help), content())).with_children(|p| {
                text(p, font, "КАК ИГРАТЬ", 40.0, theme::TEXT);
                text(p, font, "Полигон — выберите испытание, технику или оружие в верхней панели.\n\nБой — ЛКМ выбирает юнитов, рамка выделяет отряд. ПКМ отдаёт приказ двигаться или атаковать.\n\nЛаборатория — выберите объект в библиотеке снизу и разместите в сцене. «Живые» включает инструменты во время симуляции.\n\nКолесо мыши — приближение. В лаборатории средняя кнопка вращает вид, Shift + средняя кнопка перемещает его.\n\nF10 открывает главное меню и приостанавливает текущую сессию. Esc сначала отменяет активный инструмент или вид от первого лица.", 20.0, theme::TEXT);
                button(p, font, "← Главное меню", Action::Back);
            });
            root.spawn((Part(Page::Settings), Node {position_type: PositionType::Absolute,
                top: px(32), left: percent(8), right: percent(8), align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween, ..default()})).with_children(|p| {
                    text(p, font, "НАСТРОЙКИ", 32.0, theme::TEXT);
                    button(p, font, "← Главное меню", Action::Back);
                });
        });
}

pub fn sync(
    menu: Res<Menu>,
    scene: Res<Scene>,
    mut scale: ResMut<UiScale>,
    mut windows: Query<&mut Window>,
    mut root: Query<
        &mut Node,
        (
            With<Root>,
            Without<Part>,
            Without<Resume>,
            Without<knobs::SettingsRoot>,
        ),
    >,
    mut parts: Query<
        (&Part, &mut Node),
        (Without<Root>, Without<Resume>, Without<knobs::SettingsRoot>),
    >,
    mut resume: Query<
        &mut Node,
        (
            With<Resume>,
            Without<Root>,
            Without<Part>,
            Without<knobs::SettingsRoot>,
        ),
    >,
    mut settings: Query<
        (&mut Node, &mut GlobalZIndex),
        (
            With<knobs::SettingsRoot>,
            Without<Root>,
            Without<Part>,
            Without<Resume>,
        ),
    >,
    mut buttons: Query<(&Action, &Interaction, &mut BackgroundColor)>,
    mut notice: Query<&mut Text, With<Notice>>,
    mut room_values: Query<(&RoomValue, &mut Text), Without<Notice>>,
    mut cameras: Query<(&mut Camera, Option<&GasCamera>, &mut RenderTarget), With<Camera3d>>,
    mut cursors: Query<&mut CursorOptions>,
    mut menu_target: ResMut<MenuTarget>,
) {
    for mut n in &mut root {
        n.display = if menu.shown() {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (part, mut n) in &mut parts {
        n.display = if part.0 == menu.page {
            Display::Flex
        } else {
            Display::None
        };
    }
    for mut n in &mut resume {
        n.display = if menu.has_session {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (mut camera, gas, mut target) in &mut cameras {
        if gas.is_some() {
            camera.is_active = true;
            if menu.shown() && scene.out.is_none() {
                if menu_target.original.is_none() {
                    menu_target.original = Some(target.clone());
                    *target = RenderTarget::Image(menu_target.image.clone().into());
                }
            } else if let Some(original) = menu_target.original.take() {
                *target = original;
            }
        } else if menu.shown() {
            camera.is_active = false;
        }
    }
    let size = windows.single().map_or(Vec2::new(1920.0, 1080.0), |w| {
        Vec2::new(w.width(), w.height())
    });
    if menu.shown() {
        scale.0 = (size.x / 1920.0).min(size.y / 1080.0).clamp(0.7, 1.8);
        for mut cursor in &mut cursors {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
        }
        if let Ok(mut window) = windows.single_mut() {
            if window.title != "RTS VOX / Главное меню" {
                window.title = "RTS VOX / Главное меню".into();
            }
        }
    }
    for (mut n, mut z) in &mut settings {
        if menu.settings() {
            let width = (size.x / scale.0 - 64.0).clamp(300.0, 760.0);
            n.left = px((size.x / scale.0 - width) * 0.5);
            n.width = px(width);
            n.top = px(118);
            n.bottom = px(32);
            z.0 = 200;
        } else {
            n.left = px(310);
            n.width = px(330);
            n.top = px(76);
            n.bottom = px(224);
            z.0 = 50;
        }
    }
    for (action, interaction, mut background) in &mut buttons {
        background.0 = theme::button_background(*interaction, *action == Action::Resume);
    }
    for mut t in &mut notice {
        t.0 = menu.notice.clone();
    }
    for (field, mut text) in &mut room_values {
        text.0 = match field.0 {
            0 => format!("Позиция: {}", if menu.battle_setup.player_southeast { "юго-восток" } else { "северо-запад" }),
            1 => format!("Масса: {}", economy_display::mass(menu.battle_setup.player_stock)),
            2 => format!("Масса ИИ: {}", economy_display::mass(menu.battle_setup.enemy_stock)),
            _ => format!("Управление: {}", if menu.battle_setup.spectator { "ИИ × ИИ" } else { "игрок" }),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(open: bool) -> App {
        let mut layout = modding::laboratory::Definition::stock().layout;
        layout.size = [128, 128, 128];
        layout.ground_level = 16;
        layout.clear_min = [1, 1];
        layout.clear_max = [127, 127];
        layout.gun = [32, 24, 64];
        layout.gun_point = true;
        layout.target = [84, 16, 64];
        layout.blocks.clear();
        layout.units.clear();
        layout.unit_yaws.clear();
        layout.zones.clear();
        layout.course = None;
        let sim = BlastRange::with_layout(config(false, false), layout.clone()).unwrap();
        let mut app = panel::hud::tests::app_with_scene(sim, Workshop::new(Some(layout), false));
        app.insert_resource(Menu::new(open))
            .init_resource::<MenuTarget>()
            .insert_resource(laboratory::Laboratory::new(false))
            .insert_resource(knobs::Overrides::new(false, true))
            .init_resource::<pilot::Pilot>()
            .insert_resource(profile::Profile::new(None, 1.0))
            .init_resource::<streaming::Streaming>()
            .init_resource::<moving::Visual>()
            .init_resource::<fire::Effects>()
            .init_resource::<UiScale>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<AccumulatedMouseMotion>()
            .init_resource::<GlobalAmbientLight>()
            .add_message::<AppExit>()
            .add_systems(Update, input.before(panel::actions))
            .add_systems(Update, laboratory::actions.after(panel::actions))
            .add_systems(Update, knobs::actions.after(laboratory::actions));
        app
    }
    fn click(app: &mut App, action: Action) {
        let e = app.world_mut().spawn((action, Interaction::Pressed)).id();
        app.update();
        app.world_mut().despawn(e);
    }
    fn rebuild(app: &mut App) {
        let (config, layout) = app
            .world_mut()
            .resource_mut::<Workshop>()
            .pending
            .take()
            .unwrap();
        let mut scene = app.world_mut().resource_mut::<Scene>();
        scene.sim = BlastRange::with_layout(config, layout).unwrap();
        scene.generation += 1;
        drop(scene);
        app.update();
    }
    #[test]
    fn normal_launcher_opens_menu_and_automation_keeps_direct_entry() {
        let args = |v: &[&str]| v.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        assert!(requested(&args(&[
            "blast.exe",
            "--drive",
            "--range",
            "data/range-km.ron"
        ])));
        for flag in [
            "--laboratory",
            "--settings",
            "--shots",
            "--profile",
            "--skip-menu",
            "--skirmish",
            "--load",
        ] {
            assert!(!requested(&args(&["blast.exe", flag])), "{flag}");
            assert!(requested(&args(&["blast.exe", flag, "--main-menu"])));
        }
    }
    #[test]
    fn menu_preserves_pause_scale_and_clears_held_game_input() {
        let mut app = app(false);
        app.world_mut().resource_mut::<Scene>().paused = false;
        app.world_mut().resource_mut::<UiScale>().0 = 1.4;
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        click(&mut app, Action::Menu);
        assert!(app.world().resource::<Menu>().shown());
        assert_eq!(app.world().resource::<Menu>().scale_before, 1.4);
        assert!(app.world().resource::<Scene>().paused);
        assert!(!app
            .world()
            .resource::<ButtonInput<MouseButton>>()
            .pressed(MouseButton::Left));
        assert!(!app
            .world()
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::KeyW));
        app.world_mut().resource_mut::<UiScale>().0 = 0.7;
        click(&mut app, Action::Resume);
        assert!(!app.world().resource::<Menu>().shown());
        assert!(!app.world().resource::<Scene>().paused);
        assert_eq!(app.world().resource::<UiScale>().0, 1.0); // The RTS HUD owns its regular scale.
        app.world_mut().resource_mut::<Scene>().paused = true;
        click(&mut app, Action::Menu);
        click(&mut app, Action::Resume);
        assert!(app.world().resource::<Scene>().paused);
    }
    #[test]
    fn menu_blocks_game_actions_and_f10_returns_from_secondary_pages() {
        let mut app = app(true);
        let e = app
            .world_mut()
            .spawn((panel::Action::BattleWorld, Interaction::Pressed))
            .id();
        app.update();
        assert!(app.world().resource::<Workshop>().pending.is_none());
        app.world_mut().despawn(e);
        click(&mut app, Action::Settings);
        assert!(app.world().resource::<Menu>().settings());
        assert!(app.world().resource::<knobs::Overrides>().open);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F10);
        app.update();
        assert_eq!(app.world().resource::<Menu>().page, Page::Home);
        click(&mut app, Action::Help);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F10);
        app.update();
        assert_eq!(app.world().resource::<Menu>().page, Page::Home);
        assert!(app.world().resource::<Scene>().paused);
    }
    #[test]
    fn battle_room_launches_both_commander_modes() {
        let mut app = app(true);
        let layout = RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron")).unwrap();
        app.insert_resource(Workshop::new(Some(layout), false));
        for destination in [Destination::CommanderBattle, Destination::CommanderSpectate] {
            click(&mut app, Action::Start(Destination::Battle));
            assert_eq!(app.world().resource::<Menu>().page, Page::BattleRoom);
            click(&mut app, Action::Start(destination));
            assert!(app.world().resource::<Workshop>().pending.is_some());
            rebuild(&mut app);
            let sim = &app.world().resource::<Scene>().sim;
            assert_eq!(sim.battle.phase, sim_core::blast::battle::Phase::Fighting);
            assert_eq!(sim.battle.autonomous, destination == Destination::CommanderSpectate);
            assert_eq!(sim.units.len(), 2);
            assert!(sim.units.iter().all(|u| u.kind == RangeUnitKind::Commander));
            assert_eq!(sim.tank_held(0), Some(modding::range_economy::Balance::stock().initial_matter));
            assert_eq!(sim.tank_held(1), Some(modding::range_economy::Balance::stock().initial_matter));
            click(&mut app, Action::Menu);
        }
    }

    #[test]
    fn battle_room_settings_reach_the_commander_battle_without_starting_early() {
        let mut app = app(true);
        click(&mut app, Action::Start(Destination::Battle));
        assert_eq!(app.world().resource::<Menu>().page, Page::BattleRoom);
        assert!(app.world().resource::<Workshop>().pending.is_none());
        click(&mut app, Action::RoomPosition);
        click(&mut app, Action::RoomPlayerStock);
        click(&mut app, Action::RoomEnemyStock);
        click(&mut app, Action::RoomEnemyStock);
        click(&mut app, Action::StartPreparedBattle);
        let edit = app.world().resource::<Workshop>();
        let (_, layout) = edit.pending.as_ref().expect("battle is prepared only after Start");
        assert_eq!(layout.units.len(), 2);
        assert!(layout.units.iter().all(|u| u.kind == RangeUnitKind::Commander));
        assert!(layout.units[0].allied && !layout.units[1].allied);
        assert!(layout.units[0].at[0] > layout.units[1].at[0]);
        assert_eq!(layout.economy.as_ref().unwrap().kind(RangeUnitKind::Commander).unwrap().dig_edits_per_tick, 512);
        let ready = layout.ready.as_ref().unwrap();
        assert_eq!(ready.commander_stock, 75_000_000);
        assert_eq!(ready.enemy_commander_stock, Some(100_000_000));
    }

    #[test]
    fn battle_button_displays_room_instead_of_starting_the_match() {
        let mut app = app(true);
        app.add_systems(PostUpdate, sync);
        let home = app.world_mut().spawn((Part(Page::Home), Node::default())).id();
        let room = app.world_mut().spawn((Part(Page::BattleRoom), Node::default())).id();
        app.update();
        click(&mut app, Action::Start(Destination::Battle));
        assert_eq!(app.world().get::<Node>(home).unwrap().display, Display::None);
        assert_eq!(app.world().get::<Node>(room).unwrap().display, Display::Flex);
        assert!(app.world().resource::<Workshop>().pending.is_none());
    }
    #[test]
    fn room_spectator_setting_starts_ai_against_ai() {
        let mut app = app(true);
        click(&mut app, Action::Start(Destination::Battle));
        click(&mut app, Action::RoomSpectator);
        click(&mut app, Action::StartPreparedBattle);
        assert!(app.world().resource::<panel::Panel>().battle_auto);
        assert!(app.world().resource::<Workshop>().pending.is_some());
    }

    #[test]
    fn main_menu_launches_real_battle_spectator_and_range_paths() {
        let mut app = app(true);
        let layout =
            RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron")).unwrap();
        app.insert_resource(Workshop::new(Some(layout), false));
        for destination in [
            Destination::Battle,
            Destination::Spectate,
            Destination::Range,
        ] {
            click(&mut app, Action::Start(destination));
            if destination == Destination::Battle {
                assert_eq!(app.world().resource::<Menu>().page, Page::BattleRoom);
                assert!(app.world().resource::<Workshop>().pending.is_none());
                click(&mut app, Action::StartPreparedBattle);
            }
            assert!(!app.world().resource::<Menu>().shown());
            assert!(app.world().resource::<Workshop>().pending.is_some());
            rebuild(&mut app);
            assert!(app.world().resource::<Menu>().launching.is_none());
            let sim = &app.world().resource::<Scene>().sim;
            if destination != Destination::Range {
                assert_eq!(sim.battle.phase, sim_core::blast::battle::Phase::Fighting);
                assert_eq!(sim.battle.autonomous, destination == Destination::Spectate);
            }
            click(&mut app, Action::Menu);
        }
    }
    #[test]
    fn laboratory_can_resume_and_restore_range_before_switching_modes() {
        let mut app = app(true);
        click(&mut app, Action::Start(Destination::Laboratory));
        assert!(app.world().resource::<laboratory::Laboratory>().active());
        app.update();
        let hash = app.world().resource::<Scene>().sim.state_hash();
        click(&mut app, Action::Menu);
        click(&mut app, Action::Start(Destination::Laboratory));
        assert_eq!(app.world().resource::<Scene>().sim.state_hash(), hash);
        click(&mut app, Action::Menu);
        click(&mut app, Action::Start(Destination::Range));
        assert!(!app.world().resource::<laboratory::Laboratory>().active());
        assert!(app.world().resource::<Menu>().shown());
        app.update();
        assert!(app.world().resource::<Workshop>().pending.is_some());
        rebuild(&mut app);
        assert!(!app.world().resource::<Menu>().shown());
    }
    #[test]
    fn failed_scene_transition_restores_menu_with_error() {
        let mut app = app(true);
        click(&mut app, Action::Start(Destination::Range));
        app.world_mut().resource_mut::<Workshop>().pending = None;
        app.world_mut().resource_mut::<Scene>().notice = "Test load error".into();
        app.update();
        assert!(app.world().resource::<Menu>().shown());
        assert!(!app.world().resource::<Menu>().has_session);
        assert_eq!(app.world().resource::<Menu>().notice, "Test load error");
    }

    #[test]
    fn menu_uses_offscreen_world_view_releases_cursor_and_restores_window_at_4k() {
        let mut app = app(false);
        app.add_systems(PostUpdate, sync);
        let root = app.world_mut().spawn((Root, Node::default())).id();
        let camera = app.world_mut().spawn((Camera3d::default(), GasCamera)).id();
        let overlay = app.world_mut().spawn(Camera3d::default()).id();
        let window = app
            .world_mut()
            .spawn((
                Window {
                    resolution: bevy::window::WindowResolution::new(3840, 2160)
                        .with_scale_factor_override(1.0),
                    ..default()
                },
                CursorOptions {
                    grab_mode: CursorGrabMode::Locked,
                    visible: false,
                    ..default()
                },
            ))
            .id();
        click(&mut app, Action::Menu);
        assert_eq!(
            app.world().get::<Node>(root).unwrap().display,
            Display::Flex
        );
        assert!(app.world().get::<Camera>(camera).unwrap().is_active);
        assert!(matches!(
            app.world().get::<RenderTarget>(camera),
            Some(RenderTarget::Image(_))
        ));
        assert!(!app.world().get::<Camera>(overlay).unwrap().is_active);
        assert_eq!(
            app.world().get::<CursorOptions>(window).unwrap().grab_mode,
            CursorGrabMode::None
        );
        assert!(app.world().get::<CursorOptions>(window).unwrap().visible);
        assert_eq!(app.world().resource::<UiScale>().0, 1.8);
        click(&mut app, Action::Resume);
        assert_eq!(
            app.world().get::<Node>(root).unwrap().display,
            Display::None
        );
        assert!(app.world().get::<Camera>(camera).unwrap().is_active);
        assert!(matches!(
            app.world().get::<RenderTarget>(camera),
            Some(RenderTarget::Window(_))
        ));
        assert_eq!(app.world().resource::<UiScale>().0, 1.0);
    }
}
