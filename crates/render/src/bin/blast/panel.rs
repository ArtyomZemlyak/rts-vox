//! Mouse-first controls for the one forest range.
use super::*;
use modding::range::{RangeScenario, RangeScenarioWeapon};
use sim_core::blast::battle::Weapon as BattleWeapon;
use std::collections::BTreeMap;
use workbench::{Selection, Tool};

#[path = "panel/hud.rs"]
pub mod hud;
#[path = "panel/minimap.rs"]
pub mod minimap;
#[path = "panel/theme.rs"]
pub mod theme;

const SCENARIO_GAP_TICKS: u32 = 30;

#[derive(Clone, PartialEq, Eq)]
struct ShotResult {
    terrain: usize,
    armor: usize,
    lost_units: usize,
    shield_hits: u32,
}

pub struct ScenarioRun {
    id: u8,
    title: String,
    description: String,
    patrols: bool,
    prepared: bool,
    targets: Vec<[i32; 3]>,
    fired: usize,
    completed: usize,
    ready_at: Option<u32>,
    baseline_terrain: usize,
    baseline_armor: usize,
    baseline_lost_units: usize,
    baseline_shield_hits: u32,
    results: Vec<ShotResult>,
    previous: Option<Vec<ShotResult>>,
    saved: bool,
}
impl ScenarioRun {
    fn new(id: u8, s: &RangeScenario, previous: Option<Vec<ShotResult>>) -> Self {
        Self {
            id,
            title: s.title.clone(),
            description: s.description.clone(),
            patrols: s.patrols,
            prepared: false,
            targets: s.shots.clone(),
            fired: 1,
            completed: 0,
            ready_at: None,
            baseline_terrain: 0,
            baseline_armor: 0,
            baseline_lost_units: 0,
            baseline_shield_hits: 0,
            results: Vec::new(),
            previous,
            saved: false,
        }
    }
    fn progress(&mut self, sim: &mut BlastRange) {
        if !self.prepared {
            sim.patrols_running = self.patrols;
            self.prepared = true;
        }
        if !sim.ready() {
            return;
        }
        if self.completed < self.fired {
            let armor: usize = sim
                .units
                .iter()
                .map(|u| u.removed.len() + u.seg_removed.len())
                .sum();
            let shield_hits: u32 = sim.shields.iter().map(|s| s.hits).sum();
            let lost_units = sim.units.iter().filter(|u| !u.status.alive).count();
            self.results.push(ShotResult {
                terrain: sim.removed.saturating_sub(self.baseline_terrain),
                armor: armor.saturating_sub(self.baseline_armor),
                lost_units: lost_units.saturating_sub(self.baseline_lost_units),
                shield_hits: shield_hits.saturating_sub(self.baseline_shield_hits),
            });
            self.baseline_terrain = sim.removed;
            self.baseline_armor = armor;
            self.baseline_lost_units = lost_units;
            self.baseline_shield_hits = shield_hits;
            self.completed = self.fired;
            self.ready_at = Some(sim.tick);
        }
        if self.completed < self.targets.len()
            && self
                .ready_at
                .is_some_and(|t| sim.tick >= t.saturating_add(SCENARIO_GAP_TICKS))
        {
            sim.aim = Some(self.targets[self.completed]);
            if sim.refire() {
                self.fired = self.completed + 1;
                self.ready_at = None;
            }
        }
    }
    fn readout(&self) -> String {
        let mut text = format!(
            "\nОпыт: {} · {}/{} выстрелов\n{}\n",
            self.title,
            self.completed,
            self.targets.len(),
            self.description
        );
        if self.previous.is_some() {
            text.push_str("К предыдущему запуску:\n");
        }
        for (i, result) in self.results.iter().enumerate() {
            text.push_str(&format!(
                "{}: грунт −{}, броня −{}, потеряно {}, щит ×{}\n",
                i + 1,
                result.terrain,
                result.armor,
                result.lost_units,
                result.shield_hits
            ));
            if let Some(before) = self.previous.as_ref().and_then(|v| v.get(i)) {
                text.push_str(&format!(
                    "   Δ грунт {:+}, броня {:+}, потери {:+}, щит {:+}\n",
                    result.terrain as i64 - before.terrain as i64,
                    result.armor as i64 - before.armor as i64,
                    result.lost_units as i64 - before.lost_units as i64,
                    i64::from(result.shield_hits) - i64::from(before.shield_hits)
                ));
            }
        }
        text
    }
}

#[derive(Resource)]
pub struct Panel {
    pub drive: bool,
    pub demo: bool,
    pub ground_fire: bool,
    pub blocked: bool,
    pub routes: bool,
    pub scenarios_open: bool,
    pub drawer: u8,
    pub ranges: bool,
    pub projectile_markers: bool,
    pub battle_launch: bool,
    pub battle_auto: bool,
    pub battle_name: String,
    pub roster_page: usize,
    pub briefing: String,
    pub scenario_run: Option<ScenarioRun>,
    history: BTreeMap<u8, Vec<ShotResult>>,
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
            scenarios_open: true,
            drawer: 0,
            ranges: true,
            projectile_markers: true,
            battle_launch: false,
            battle_auto: false,
            battle_name: String::new(),
            roster_page: 0,
            briefing: String::new(),
            scenario_run: None,
            history: BTreeMap::new(),
            keys: Vec::new(),
            pending: None,
        }
    }
}
#[derive(Component, Clone, Copy, PartialEq)]
pub enum Action {
    Drawer(u8),
    Ranges,
    ProjectileMarkers,
    SelectUnit(usize),
    SelectAllOwn,
    RosterPage(i32),
    BattleWeapon(Option<BattleWeapon>),
    Skirmish(u8),
    Play,
    BattlePreset,
    BattleWorld,
    BattleWorldAuto,
    RangeMode,
    BattleStart,
    FreeControl,
    BattleReplay,
    BattleChoose,
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
    WholeMap,
    Zone(i32),
    RestartHere,
    ScenarioMenu,
    Scenario(u8),
    Orbit(f32),
    Pitch(f32),
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
#[derive(Component)]
pub struct ScenarioOnly;

fn scenario_weapon(weapon: RangeScenarioWeapon) -> (bool, bool, Delivery) {
    match weapon {
        RangeScenarioWeapon::Shell => (false, false, Delivery::Blast),
        RangeScenarioWeapon::Energy => (true, false, Delivery::Blast),
        RangeScenarioWeapon::Annihilation => (true, true, Delivery::Blast),
        RangeScenarioWeapon::Kinetic => (false, false, Delivery::Kinetic),
    }
}

pub fn setup(mut commands: Commands, edit: Res<Workshop>, mut fonts: ResMut<Assets<Font>>) {
    if edit.layout.is_none() {
        return;
    }
    let stock = RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron"))
        .expect("stock scenario layout");
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
    hud::build(&mut commands, camera, &text_font);
    minimap::build(&mut commands, camera, &text_font);
    let mut root = commands.spawn((
        PanelRoot,
        hud::Drawer,
        ScrollPanel,
        ScrollPosition::default(),
        bevy::ui::RelativeCursorPosition::default(),
        Node {
            position_type: PositionType::Absolute,
            right: px(16),
            top: px(76),
            width: px(330),
            bottom: px(236),
            display: Display::None,
            overflow: Overflow::scroll_y(),
            padding: UiRect::all(px(14)),
            row_gap: px(5),
            flex_direction: FlexDirection::Column,
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(10)),
            ..default()
        },
        BackgroundColor(theme::PANEL),
        BorderColor::all(theme::FRAME),
        ZIndex(10),
        UiTargetCamera(camera),
    ));
    root.with_children(|p| {
        p.spawn((
            Text::new("ЦЕНТР УПРАВЛЕНИЯ"),
            TextFont {
                font_size: FontSize::Px(23.0),
                ..text_font.clone()
            },
            TextColor(theme::TEXT),
        ));
        p.spawn((
            Text::new("Операции на километровой карте"),
            text_font.clone(),
            TextColor(theme::MUTED),
        ));
        p.spawn((
            Text::new(""),
            text_font.clone(),
            TextColor(theme::ACCENT),
            SectorLabel,
        ));
        let mut rows: Vec<(bool, Vec<(&str, Action)>)> = vec![
            (
                false,
                vec![("Прорыв укреплений · 3 на 3", Action::Skirmish(0))],
            ),
            (
                false,
                vec![("Встреча в окружении · 3 против 4", Action::Skirmish(1))],
            ),
            (false, vec![("Бой на всей карте", Action::BattleWorld)]),
            (
                false,
                vec![("ИИ против ИИ · вся карта", Action::BattleWorldAuto)],
            ),
            (false, vec![("Бой 2×2 / расстановка", Action::BattlePreset)]),
            (false, vec![("Паук: лазерный прорыв", Action::Skirmish(2))]),
            (false, vec![("Штурм турелей T1/T2", Action::Skirmish(3))]),
            (false, vec![("Режим испытаний / сброс", Action::RangeMode)]),
            (false, vec![("Сценарии испытаний", Action::ScenarioMenu)]),
            (
                false,
                vec![
                    ("Следующий свой", Action::NextOwn),
                    ("Все свои", Action::SelectAllOwn),
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
                    ("Огонь по точке", Action::Fire),
                    ("Стоп выбранных", Action::Stop),
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
                    ("Бронебой", Action::Weapon(5)),
                    ("Огнемёт", Action::Weapon(3)),
                    ("Пулемёт", Action::Weapon(4)),
                    ("Лазер", Action::Weapon(6)),
                ],
            ),
            (
                false,
                vec![
                    ("Танк", Action::Chassis(RangeUnitKind::Tank)),
                    ("Шагоход", Action::Chassis(RangeUnitKind::Strider)),
                    ("Паук", Action::Chassis(RangeUnitKind::Spider)),
                    ("Артиллерия", Action::Chassis(RangeUnitKind::Artillery)),
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
                    ("Танк", Action::Tool(Tool::Tank)),
                    ("Шагоход", Action::Tool(Tool::Strider)),
                    ("Паук", Action::Tool(Tool::Spider)),
                ],
            ),
            (
                true,
                vec![
                    ("Артиллерия", Action::Tool(Tool::Artillery)),
                    ("Турель T1", Action::Tool(Tool::TurretT1)),
                    ("Турель T2", Action::Tool(Tool::TurretT2)),
                    ("База", Action::Tool(Tool::Base)),
                ],
            ),
            (
                true,
                vec![
                    ("Бетон", Action::Tool(Tool::Concrete)),
                    ("Сталь", Action::Tool(Tool::Steel)),
                    ("Грунт", Action::Tool(Tool::Earth)),
                    ("Стереть", Action::Tool(Tool::Clear)),
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
                    ("Карта", Action::WholeMap),
                    ("Юнит", Action::Own),
                    ("Цель", Action::Target),
                ],
            ),
            (
                false,
                vec![("Метки снарядов: вкл", Action::ProjectileMarkers)],
            ),
            (false, vec![("Настройки", Action::Settings)]),
            (
                false,
                vec![
                    ("Зум −", Action::Zoom(1.2)),
                    ("Зум +", Action::Zoom(0.8)),
                    ("Влево", Action::Pan(-24.0, 0.0)),
                    ("Вправо", Action::Pan(24.0, 0.0)),
                ],
            ),
            (
                false,
                vec![
                    ("Вверх", Action::Pan(0.0, -24.0)),
                    ("Вниз", Action::Pan(0.0, 24.0)),
                    ("Вращ. -", Action::Orbit(-0.25)),
                    ("Вращ. +", Action::Orbit(0.25)),
                ],
            ),
            (
                false,
                vec![
                    ("Наклон +", Action::Pitch(0.12)),
                    ("Наклон -", Action::Pitch(-0.12)),
                ],
            ),
        ];
        let scenario_rows = stock.zones.iter().enumerate().filter_map(|(id, zone)| {
            zone.scenario
                .as_ref()
                .map(|s| (false, vec![(s.title.as_str(), Action::Scenario(id as u8))]))
        });
        rows.splice(5..5, scenario_rows);
        for (editor, row) in rows {
            let section = hud::section(editor, row[0].1);
            let mut parent = p.spawn(Node {
                width: percent(100),
                flex_shrink: 0.0,
                column_gap: px(6),
                ..default()
            });
            parent.insert(hud::DrawerSection(section));
            if editor {
                parent.insert(EditorOnly);
            }
            if row.iter().any(|(_, a)| matches!(a, Action::Scenario(_))) {
                parent.insert(ScenarioOnly);
            }
            if row.iter().any(|(_, a)| {
                matches!(
                    a,
                    Action::Weapon(_)
                        | Action::Chassis(_)
                        | Action::Power(_)
                        | Action::Patrol
                        | Action::Scenario(_)
                        | Action::ScenarioMenu
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
                        BackgroundColor(theme::BUTTON),
                    ))
                    .with_children(|b| {
                        let mut text =
                            b.spawn((Text::new(label), text_font.clone(), TextColor(theme::TEXT)));
                        if action == Action::ProjectileMarkers {
                            text.insert(hud::HudText(7));
                        }
                    });
                }
            });
        }
        p.spawn((
            Text::new(""),
            text_font,
            TextColor(theme::MUTED),
            Readout,
            hud::DrawerSection(5),
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
    mut scrolls: Query<&mut ScrollPosition, With<ScrollPanel>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut gas: ResMut<GasInput>,
    scale: Option<ResMut<UiScale>>,
    overrides: Option<Res<knobs::Overrides>>,
) {
    let stock = knobs::Overrides::default();
    let overrides = overrides.as_deref().unwrap_or(&stock);
    let factor = windows
        .single()
        .map_or(1.0, |w| hud::scale(Vec2::new(w.width(), w.height())));
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
    let additive_selection = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let mut key = |k: KeyCode, p: &mut Panel| {
        keys.press(k);
        p.keys.push(k);
    };
    for (&action, interaction) in &interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        scene.notice.clear();
        if scene.sim.battle_mode()
            && scene.sim.battle.phase == sim_core::blast::battle::Phase::Setup
            && matches!(action, Action::Fire | Action::AutoAttack)
        {
            scene.notice = "В свободном управлении доступны движение и остановка. Для стрельбы нажмите «Начать бой».".into();
            continue;
        }
        if !matches!(
            action,
            Action::Drawer(_)
                | Action::Ranges
                | Action::ProjectileMarkers
                | Action::Settings
                | Action::SelectUnit(_)
                | Action::SelectAllOwn
                | Action::RosterPage(_)
                | Action::NextOwn
                | Action::Scenario(_)
                | Action::ScenarioMenu
                | Action::Pause
                | Action::Orbit(_)
                | Action::Pitch(_)
                | Action::Zoom(_)
                | Action::Pan(..)
                | Action::Overview
                | Action::WholeMap
                | Action::Own
                | Action::Target
        ) {
            panel.scenario_run = None;
        }
        if matches!(
            action,
            Action::BattleWorld
                | Action::BattleWorldAuto
                | Action::BattlePreset
                | Action::RangeMode
                | Action::Scenario(_)
        ) {
            panel.briefing.clear();
            panel.battle_launch = false;
            panel.battle_auto = false;
        }
        match action {
            Action::Drawer(id) => {
                panel.drawer = if panel.drawer == id { 0 } else { id };
                if panel.drawer != 0 {
                    for mut scroll in &mut scrolls {
                        scroll.y = 0.0;
                    }
                }
            }
            Action::Ranges => panel.ranges = !panel.ranges,
            Action::ProjectileMarkers => panel.projectile_markers = !panel.projectile_markers,
            Action::SelectUnit(id) => {
                if scene.sim.battle_mode()
                    && scene.sim.allied(id)
                    && scene.sim.units.get(id).is_some_and(|u| u.status.alive)
                {
                    edit.select_battle_unit(&scene.sim, id, additive_selection);
                }
            }
            Action::SelectAllOwn => {
                if scene.sim.battle_mode() {
                    let ids: Vec<_> = scene
                        .sim
                        .units
                        .iter()
                        .enumerate()
                        .filter(|(i, u)| scene.sim.allied(*i) && u.status.alive)
                        .map(|(i, _)| i)
                        .collect();
                    edit.select_battle_group(&scene.sim, &ids, false);
                }
            }
            Action::RosterPage(delta) => {
                let page_size = windows.single().map_or(hud::ROSTER_PAGE_SIZE, |window| {
                    hud::roster_page_size(window.width())
                });
                let count = scene
                    .sim
                    .units
                    .iter()
                    .enumerate()
                    .filter(|(i, u)| scene.sim.allied(*i) && u.status.alive)
                    .count();
                let pages = count.div_ceil(page_size).max(1);
                panel.roster_page =
                    (panel.roster_page as i32 + delta).rem_euclid(pages as i32) as usize;
            }
            Action::BattleWeapon(weapon) => {
                let ids = edit.battle_selected_ids(&scene.sim);
                let mut changed = false;
                for id in ids {
                    changed |= scene.sim.set_battle_weapon(id, weapon);
                }
                if !changed {
                    scene.notice = "Выберите свой действующий танк".into();
                }
            }
            Action::Skirmish(id) => match edit.skirmish(id, scene.sim.config.clone()) {
                Ok(briefing) => {
                    panel.battle_name = edit.sector().map_or_else(String::new, |z| z.name.clone());
                    panel.briefing = briefing;
                    panel.battle_launch = true;
                    panel.drawer = 0;
                    panel.pending = None;
                    panel.drive = true;
                    panel.demo = false;
                    panel.ground_fire = false;
                    scene.repeat = false;
                    scene.view = 1;
                }
                Err(e) => scene.notice = e,
            },
            Action::BattleWorld | Action::BattleWorldAuto => {
                match edit.battle_world(scene.sim.config.clone()) {
                    Ok(briefing) => {
                        panel.battle_name = if action == Action::BattleWorldAuto {
                            "Война за базы · ИИ × ИИ".into()
                        } else {
                            "Война за базы".into()
                        };
                        panel.battle_auto = action == Action::BattleWorldAuto;
                        panel.briefing = briefing;
                        panel.battle_launch = true;
                        panel.drawer = 0;
                        panel.pending = None;
                        panel.drive = true;
                        scene.view = 1;
                        scene.repeat = false;
                        panel.demo = false;
                        panel.ground_fire = false;
                    }
                    Err(e) => scene.notice = e,
                }
            }
            Action::BattlePreset => match edit.battle_preset(scene.sim.config.clone()) {
                Ok(()) => {
                    panel.battle_name = "Бой 2×2 / расстановка".into();
                    panel.drawer = 0;
                    panel.pending = None;
                    scene.view = 1;
                    scene.repeat = false;
                    panel.demo = false;
                    panel.ground_fire = false;
                }
                Err(e) => scene.notice = e,
            },
            Action::RangeMode => match edit.range_mode(scene.sim.config.clone()) {
                Ok(()) => {
                    panel.battle_name.clear();
                    panel.battle_launch = false;
                    panel.battle_auto = false;
                    panel.roster_page = 0;
                    panel.drawer = 0;
                    panel.pending = None;
                    panel.drive = true;
                    panel.demo = false;
                    panel.ground_fire = false;
                    panel.pending = None;
                    scene.repeat = false;
                    scene.paused = false;
                }
                Err(e) => scene.notice = e,
            },
            Action::FreeControl => match scene.sim.start_free_control() {
                Ok(()) => {
                    panel.drawer = 0;
                    edit.editing = false;
                    scene.paused = false;
                    panel.drive = true;
                    panel.demo = false;
                    panel.ground_fire = false;
                    panel.battle_launch = false;
                    scene.repeat = false;
                    scene.notice = "Свободное управление: ЛКМ — выбор, ПКМ — движение. ИИ и стрельба выключены до начала боя.".into();
                    edit.controlled = scene
                        .sim
                        .units
                        .iter()
                        .enumerate()
                        .find(|(i, u)| {
                            scene.sim.allied(*i) && u.status.alive && !u.kind.emplacement()
                        })
                        .map(|(i, _)| i);
                    edit.selected_units = edit.controlled.into_iter().collect();
                    edit.selected = edit.controlled.map(Selection::Unit);
                }
                Err(e) => scene.notice = e.into(),
            },
            Action::BattleStart => match scene.sim.start_battle() {
                Ok(()) => {
                    panel.briefing.clear();
                    panel.drawer = 0;
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
                        .find(|(i, u)| {
                            scene.sim.allied(*i)
                                && u.status.alive
                                && u.status.engine
                                && u.status.weapon
                        })
                        .map(|(i, _)| i);
                    edit.selected_units = edit.controlled.into_iter().collect();
                    edit.selected = edit.controlled.map(Selection::Unit);
                }
                Err(e) => scene.notice = e.into(),
            },
            Action::BattleReplay => match edit.replay_battle(scene.sim.config.clone()) {
                Ok(()) => {
                    panel.battle_launch = true;
                    panel.battle_auto = scene.sim.battle.autonomous;
                    panel.briefing.clear();
                    panel.drawer = 0;
                    panel.roster_page = 0;
                    panel.ground_fire = false;
                    panel.pending = None;
                    scene.paused = true;
                    scene.repeat = false;
                }
                Err(e) => scene.notice = e,
            },
            Action::BattleChoose => {
                panel.drawer = 1;
                for mut scroll in &mut scrolls {
                    scroll.y = 0.0;
                }
            }
            Action::Side(allied) => {
                if edit.editing {
                    match edit.side(allied, scene.sim.config.clone()) {
                        Ok(()) if panel.battle_name.is_empty() => {
                            panel.battle_name = "Свой бой".into();
                        }
                        Err(e) => scene.notice = e,
                        _ => {}
                    }
                }
            }
            Action::NextOwn => {
                let page_size = windows.single().map_or(hud::ROSTER_PAGE_SIZE, |window| {
                    hud::roster_page_size(window.width())
                });
                let n = scene.sim.units.len();
                let start = edit.controlled.map_or(0, |i| i + 1);
                if n > 0 {
                    edit.controlled = (0..n)
                        .map(|i| (start + i) % n)
                        .find(|&i| scene.sim.allied(i) && scene.sim.units[i].status.alive);
                    edit.selected = edit.controlled.map(Selection::Unit);
                    edit.selected_units = edit.controlled.into_iter().collect();
                    if let Some(id) = edit.controlled {
                        panel.roster_page = scene
                            .sim
                            .units
                            .iter()
                            .enumerate()
                            .filter(|(i, u)| scene.sim.allied(*i) && u.status.alive)
                            .position(|(i, _)| i == id)
                            .unwrap_or(0)
                            / page_size;
                    }
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
                    for id in edit.battle_selected_ids(&scene.sim) {
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
                    for id in edit.battle_selected_ids(&scene.sim) {
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
                    let w = match kind {
                        RangeUnitKind::Strider => 4,
                        RangeUnitKind::Spider => 6,
                        RangeUnitKind::Tank => 5,
                        RangeUnitKind::Artillery => 0,
                        RangeUnitKind::TurretT1 => 5,
                        RangeUnitKind::TurretT2 => 1,
                        RangeUnitKind::Base => 0,
                    };
                    let delivery = match w {
                        3 => Delivery::Flame,
                        4 => Delivery::MachineGun,
                        5 => Delivery::Kinetic,
                        6 => Delivery::Laser,
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
            Action::Delete => {
                if edit.selected.is_some() {
                    key(KeyCode::Delete, &mut panel);
                } else {
                    scene.notice = "Сначала выберите объект для удаления".into();
                }
            }
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
                } else {
                    scene.notice = "Сначала выберите юнит для маршрута".into();
                }
            }
            Action::Save => key(KeyCode::F6, &mut panel),
            Action::Load => {
                panel.battle_name.clear();
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
            Action::ScenarioMenu => panel.scenarios_open = !panel.scenarios_open,
            Action::Scenario(id) => {
                let stock = RangeLayout::from_ron(include_str!("../../../../../data/range-km.ron"));
                match stock.and_then(|mut layout| {
                    let zone = layout
                        .zones
                        .get(id as usize)
                        .ok_or("Unknown scenario sector")?
                        .clone();
                    let scenario = zone.scenario.clone().ok_or("No scenario in sector")?;
                    let (energy, anni, delivery) = scenario_weapon(scenario.weapon);
                    layout.gun = zone.spawn;
                    layout.target = zone.target;
                    layout.battle = false;
                    if let Some(course) = layout.course.as_mut() {
                        course.patrols.retain(|patrol| {
                            scenario.patrols
                                && layout
                                    .units
                                    .get(patrol.unit)
                                    .is_some_and(|unit| zone.contains([unit.at[0], unit.at[2]], 0))
                        });
                    }
                    let config = knobs::effective_config(
                        energy,
                        anni,
                        scenario.power_milli,
                        delivery,
                        overrides,
                    );
                    edit.commit(layout, config)?;
                    Ok::<_, String>((scenario, energy, anni))
                }) {
                    Ok((scenario, energy, anni)) => {
                        edit.zone = id as usize;
                        edit.editing = false;
                        edit.selected = None;
                        edit.controlled = None;
                        edit.pan = Vec2::ZERO;
                        edit.zoom = 1.0;
                        edit.orbit = 0.0;
                        edit.pitch = 0.0;
                        panel.scenarios_open = false;
                        panel.drive = false;
                        panel.demo = false;
                        panel.pending = None;
                        panel.ground_fire = false;
                        panel.scenario_run = Some(ScenarioRun::new(
                            id,
                            &scenario,
                            panel.history.get(&id).cloned(),
                        ));
                        scene.energy = energy;
                        scene.anni = anni;
                        scene.power = scenario.power_milli;
                        scene.repeat = false;
                        scene.view = 1;
                        scene.layout_file = Some("data/range-km.ron".into());
                    }
                    Err(e) => scene.notice = e,
                }
            }
            Action::Orbit(delta) => {
                edit.orbit = (edit.orbit + delta).rem_euclid(std::f32::consts::TAU)
            }
            Action::Pitch(delta) => edit.pitch = (edit.pitch + delta).clamp(-0.6, 0.6),
            Action::Overview => {
                scene.view = 1;
                edit.pan = Vec2::ZERO;
                edit.zoom = 1.0;
            }
            Action::WholeMap => {
                scene.view = 6;
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
            Action::Zoom(z) => edit.zoom = (edit.zoom * z).clamp(0.3, 32.0),
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
            5 => Delivery::Kinetic,
            6 => Delivery::Laser,
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
/// Stable panel IDs: shell 0, energy 1, annihilation 2, flame 3,
/// machine gun 4, solid kinetic shot 5 and thermal laser 6.
fn weapon_of(scene: &Scene) -> u8 {
    match scene.sim.config.delivery {
        Delivery::Flame => return 3,
        Delivery::MachineGun => return 4,
        Delivery::Kinetic => return 5,
        Delivery::Laser => return 6,
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

/// Resume an authored series only after the previous physical impact has
/// finished. Manual control cancels automation without discarding damage.
pub fn scenario_progress(
    mut panel: ResMut<Panel>,
    mut scene: ResMut<Scene>,
    mut edit: ResMut<Workshop>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if panel.battle_launch && edit.pending.is_none() && scene.sim.battle_mode() {
        if panel.battle_auto {
            scene.sim.set_ai_vs_ai(true);
        }
        match scene.sim.start_battle() {
            Ok(()) => {
                panel.briefing.clear();
                edit.editing = false;
                scene.paused = false;
            }
            Err(e) => scene.notice = e.into(),
        }
        panel.battle_launch = false;
        panel.battle_auto = false;
    }
    if scene.sim.player
        || scene.sim.battle_mode()
        || [KeyCode::KeyF, KeyCode::KeyR, KeyCode::F5, KeyCode::Enter]
            .iter()
            .any(|key| keys.just_pressed(*key))
    {
        panel.scenario_run = None;
        return;
    }
    if let Some(run) = &mut panel.scenario_run {
        run.progress(&mut scene.sim);
    }
    let finished = panel.scenario_run.as_mut().and_then(|run| {
        if run.completed == run.targets.len() && !run.saved {
            run.saved = true;
            Some((run.id, run.results.clone()))
        } else {
            None
        }
    });
    if let Some((id, results)) = finished {
        panel.history.insert(id, results);
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
        (
            &mut Node,
            Option<&EditorOnly>,
            Option<&RangeOnly>,
            Option<&ScenarioOnly>,
            Option<&hud::DrawerSection>,
        ),
        Or<(
            With<EditorOnly>,
            With<RangeOnly>,
            With<ScenarioOnly>,
            With<hud::DrawerSection>,
        )>,
    >,
    mut buttons: Query<(
        &Action,
        &Interaction,
        &mut BackgroundColor,
        Option<&mut BorderColor>,
    )>,
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
        **t = if scene.sim.battle_mode() && panel.drawer == 1 {
            let name = if panel.battle_name.is_empty() {
                "Текущий бой"
            } else {
                &panel.battle_name
            };
            format!("Сейчас: {name}\nНовый сценарий заменит текущий бой")
        } else if panel.drawer == 1 {
            "Выберите сценарий боя".into()
        } else {
            sector
        };
    }
    for (mut n, editor, range, scenario, section) in &mut rows {
        n.display = if (editor.is_none() || edit.editing)
            && (range.is_none() || !scene.sim.battle_mode())
            && (scenario.is_none() || panel.scenarios_open)
            && section.is_none_or(|s| s.0 == panel.drawer)
        {
            Display::Flex
        } else {
            Display::None
        };
    }
    let selected_ids = edit.battle_selected_ids(&scene.sim);
    let weapons: Vec<_> =
        selected_ids
            .iter()
            .filter(|&&id| {
                scene.sim.units.get(id).is_some_and(|u| {
                    matches!(u.kind, RangeUnitKind::Tank | RangeUnitKind::Artillery)
                })
            })
            .filter_map(|&id| scene.sim.battle.actors.get(id).map(|a| a.weapon))
            .collect();
    let group_weapon = weapons
        .first()
        .copied()
        .filter(|weapon| weapons.iter().all(|other| other == weapon));
    for (a, interaction, mut color, border) in &mut buttons {
        let active = match *a {
            Action::Drawer(id) => panel.drawer == id,
            Action::Ranges => panel.ranges,
            Action::ProjectileMarkers => panel.projectile_markers,
            Action::SelectUnit(id) => {
                if edit.selected_units.is_empty() {
                    edit.controlled == Some(id)
                } else {
                    edit.selected_units.contains(&id)
                }
            }
            Action::SelectAllOwn => {
                let selected = selected_ids.len();
                selected > 0 && selected == scene.sim.battle_counts()[0]
            }
            Action::BattleWeapon(w) => group_weapon == Some(w),
            Action::Side(own) => edit.allied == own,
            Action::BattleStart => {
                scene.sim.battle.phase == sim_core::blast::battle::Phase::Fighting
                    && scene.sim.battle_mode()
            }
            Action::Play => !edit.editing,
            Action::Edit => edit.editing,
            Action::Pause => scene.paused,
            Action::Fire => panel.ground_fire,
            Action::AutoAttack => {
                if scene.sim.battle_mode() {
                    edit.controlled
                        .and_then(|i| scene.sim.battle.actors.get(i))
                        .is_some_and(|a| a.order == sim_core::blast::battle::Order::Auto)
                } else {
                    scene.sim.auto_attack
                }
            }
            Action::Chassis(k) => k == scene.sim.gun.kind,
            Action::Weapon(w) => w == panel.pending.map_or(weapon_of(&scene), |v| v.0),
            Action::Power(p) => p == panel.pending.map_or(scene.power, |v| v.1),
            Action::Tool(t) => edit.tool == t,
            Action::Patrol => scene.sim.patrols_running,
            Action::Routes => panel.routes,
            Action::ScenarioMenu => panel.scenarios_open,
            _ => false,
        };
        color.0 = theme::button_background(*interaction, active);
        if let Some(mut border) = border {
            *border = BorderColor::all(if active { theme::ACCENT } else { theme::FRAME });
        }
    }
    let instructions = if edit.editing {
        "ЛКМ — поставить / выбрать\nПКМ — переместить выбранное"
    } else if scene.sim.battle_mode()
        && scene.sim.battle.phase == sim_core::blast::battle::Phase::Setup
    {
        "ЛКМ / рамка — выбрать своих\nПКМ — движение группы\nСтрельба и ИИ — после «Начать бой»"
    } else if scene.sim.battle_mode() {
        "ЛКМ — выбрать своего · рамка — отряд
Shift+ЛКМ — добавить / убрать
ПКМ — движение / атака всей группы"
    } else if panel.ground_fire {
        "Укажите точку выстрела на карте"
    } else if scene.sim.auto_attack {
        "Автоатака — мишени в радиусе\nПКМ — ехать и продолжать стрельбу"
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
            sim_core::blast::battle::Phase::Setup if !edit.editing => {
                "СВОБОДНОЕ УПРАВЛЕНИЕ · бой не запущен"
            }
            sim_core::blast::battle::Phase::Setup => "Расстановка · можно управлять без боя",
            sim_core::blast::battle::Phase::Fighting if scene.sim.battle.autonomous => {
                "БОЙ · ИИ × ИИ"
            }
            sim_core::blast::battle::Phase::Fighting => "БОЙ",
            sim_core::blast::battle::Phase::Victory => "ПОБЕДА",
            sim_core::blast::battle::Phase::Defeat => "ПОРАЖЕНИЕ",
            sim_core::blast::battle::Phase::Draw => "НИЧЬЯ",
        };
        let controlled = edit
            .controlled
            .map_or_else(|| "не выбран".into(), |i| format!("№{}", i + 1));
        let reach = selected
            .and_then(|i| scene.sim.units.get(i))
            .map(|_u| {
                let units = selected
                    .and_then(|i| scene.sim.battle_attack_range(i))
                    .unwrap_or(0);
                format!("{} м", units / 10)
            })
            .unwrap_or_else(|| "выберите юнита".into());
        target = format!(
            "{phase}
Свои: {own} · Враги: {enemy}
Выделено: {} · ведущий: {controlled}
Танки — выбор боеприпаса · Шагоходы — два пулемёта
Дальность выбранного: {reach}
{target}",
            edit.battle_selected_ids(&scene.sim).len()
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
            Delivery::Kinetic => "Бронебойный · локальное пробитие".into(),
            Delivery::Laser => format!(
                "Лазер · плавит цель · дальность {} м",
                scene.sim.streams.tuning.laser.range / 10
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
    let scenario = panel
        .scenario_run
        .as_ref()
        .map(ScenarioRun::readout)
        .unwrap_or_default();
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
            "{state} · {:.0} FPS · тик {:.1} мс\n\n{instructions}\n\n{readiness}\n{weapon_hint}\n{target}{shields}{scenario}\n{}{debris}{}",
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

#[cfg(test)]
mod scenario_tests {
    use super::*;

    #[test]
    fn a_series_waits_for_completion_and_keeps_the_damaged_world() {
        let layout =
            RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap();
        let config =
            ExplosionConfig::from_ron(include_str!("../../../../../data/explosion.ron")).unwrap();
        let mut sim = BlastRange::with_layout(config, layout).unwrap();
        sim.tick = 1000;
        sim.fire_tick = u32::MAX;
        sim.removed = 7;
        assert!(sim.ready());
        let scenario = RangeScenario {
            title: "Two hits".into(),
            description: "Same world".into(),
            weapon: RangeScenarioWeapon::Shell,
            power_milli: 1000,
            patrols: false,
            shots: vec![[392, 24, 336], [400, 24, 336]],
        };
        let mut run = ScenarioRun::new(0, &scenario, None);
        run.progress(&mut sim);
        assert!(!sim.patrols_running);
        run.progress(&mut sim);
        assert_eq!(run.results.len(), 1);
        assert_eq!(run.results[0].terrain, 7);
        assert_eq!(run.fired, 1);
        sim.tick += SCENARIO_GAP_TICKS;
        run.progress(&mut sim);
        assert_eq!(run.fired, 2);
        assert_eq!(sim.aim, Some([400, 24, 336]));
        assert!(!sim.ready());
        assert_eq!(sim.removed, 7);
    }

    #[test]
    fn a_two_shot_series_completes_on_the_live_damageable_range() {
        let layout =
            RangeLayout::from_ron(include_str!("../../../../../data/range-cover.ron")).unwrap();
        let mut config =
            ExplosionConfig::from_ron(include_str!("../../../../../data/explosion.ron")).unwrap();
        config.fire_tick = 10;
        config.charge_ticks = 1;
        let mut sim = BlastRange::with_layout(config, layout).unwrap();
        let scenario = RangeScenario {
            title: "Two hits".into(),
            description: "Same world".into(),
            weapon: RangeScenarioWeapon::Shell,
            power_milli: 1000,
            patrols: false,
            shots: vec![[392, 24, 336], [400, 24, 336]],
        };
        let mut run = ScenarioRun::new(0, &scenario, None);
        for _ in 0..600 {
            sim.step();
            run.progress(&mut sim);
            if run.results.len() == 2 {
                break;
            }
        }
        assert_eq!(run.results.len(), 2, "two impacts did not complete");
        assert!(sim.tick > SCENARIO_GAP_TICKS);
        assert_eq!(sim.aim, Some([400, 24, 336]));
    }

    #[test]
    fn a_rerun_shows_per_shot_differences() {
        let scenario = RangeScenario {
            title: "Two hits".into(),
            description: "Same world".into(),
            weapon: RangeScenarioWeapon::Shell,
            power_milli: 1000,
            patrols: false,
            shots: vec![[392, 24, 336]],
        };
        let mut run = ScenarioRun::new(
            0,
            &scenario,
            Some(vec![ShotResult {
                terrain: 5,
                armor: 2,
                lost_units: 0,
                shield_hits: 0,
            }]),
        );
        run.results.push(ShotResult {
            terrain: 8,
            armor: 1,
            lost_units: 1,
            shield_hits: 0,
        });
        let text = run.readout();
        assert!(text.contains("Δ грунт +3, броня -1, потери +1"), "{text}");
    }
}
