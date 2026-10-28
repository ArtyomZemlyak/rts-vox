//! Asset constructor and experiments using the range's real weapon/terrain physics. The suspended
//! session is moved, never rebuilt from its pristine layout or ticked offscreen.
use super::*;
use modding::laboratory::{Definition, Recipe};
use modding::range::RangeBlock;
use panel::theme;
use sim_core::blast::battle::commands::{Command, GunCommand, MaterialField};
#[path = "laboratory/constructor.rs"]
mod constructor;
#[path = "laboratory/experiments.rs"]
mod experiments;
#[path = "laboratory/scenario.rs"]
mod scenario;
#[path = "laboratory/ui.rs"]
mod ui;
#[path = "laboratory/scrolling.rs"]
mod scrolling;
#[path = "laboratory/sources.rs"]
mod sources;
#[path = "laboratory/parameters.rs"]
mod parameters;
#[path = "laboratory/events.rs"]
mod events;
#[path = "laboratory/visuals.rs"]
mod visuals;
#[path="laboratory/tools.rs"]
pub(super) mod tools;
#[path="laboratory/first_person.rs"]
pub(super) mod first_person;
#[path="laboratory/orbit.rs"]
mod orbit;
#[path="laboratory/collectors.rs"]
mod collectors;
pub use collectors::draw as collectors_draw;
pub use first_person::{input as first_person_input, draw as first_person_draw, flush as flush_live};
#[path="laboratory/live_tools.rs"]
mod live_tools;
pub fn tick_live(lab:&mut Laboratory,scene:&mut Scene) {
    scenario::advance(lab,scene);
    live_tools::tick(lab,scene);
}
#[path="laboratory/highlight.rs"]
mod highlight;
#[path="laboratory/section.rs"]
mod section;
pub use section::camera as section_camera;
pub use highlight::update as highlight;
pub use tools::input as tools_input;
pub use constructor::pointer;
pub use ui::{capture_ready, sync as sync_ui};
pub use scrolling::update as scroll_ui;
pub use parameters::sync as sync_parameters;
pub use scenario::sync as sync_scenario;
#[cfg(test)]
#[path = "laboratory/tests.rs"]
mod integration_tests;
#[cfg(test)]
#[path="laboratory/material_tests.rs"]
mod material_tests;

#[cfg(test)]
#[path="laboratory/inspector_tests.rs"]
mod inspector_tests;

#[cfg(test)]
#[path="laboratory/voxel_editor_tests.rs"]
mod voxel_editor_tests;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum LabAction {
    SectionToggle,SectionShift(i32),SectionFocus,
    LiveTools,LiveTool(u8),LivePoint,LivePower(i32),LiveApply([i32;3],[i32;3]),LiveRelease,LiveCapture,LiveSpeed(i32),LiveLook(i32),LiveFov(i32),
    SurfaceSnap,AllowOverlap,Flying,
    ProbeMove(u8,i32),ClearProbe,ProbeFocus,
    BeginVoxelEdit,VoxelTool(u8),VoxelStroke,VoxelFinish(bool),VoxelUndo,VoxelRedo,SavePart,
    BrushSize(i32),BrushDistance(i32),
    Sculpt([i32;3],bool),
    DeviceAt(modding::lab_tools::DeviceKind,[i32;3]),
    PickEvent(usize),
    PlaceEvent(usize, [i32;3]),
    LocalCharge(bool),
    ReturnConstruction,
    Source(usize),
    AddSource,
    RemoveSource,
    BindSource(bool),
    SourcePosition,
    SourceAt([i32;3]),
    ScenarioInput(usize),ScenarioAdd(u8),ScenarioRemove,ScenarioMove(i32),ScenarioAt([i32;3]),
    Device(usize),RemoveDevice,
    EditNumber(u8),
    ApplyNumber,
    CancelNumber,
    #[cfg_attr(not(test), allow(dead_code))]
    SetNumber(u8, i64),
    AdjustNumber(u8, i32),
    ParameterDetails,
    Back,
    PickCatalog(usize),
    PickObject(usize),
    Category(u8),
    Page(i32),
    Tab(u8),
    Tool(u8),
    Turn(i32),
    Elevation(i32),
    MoveAt([i32; 3]),
    Duplicate,
    Grid,
    Help,
    #[cfg_attr(not(test), allow(dead_code))]
    Material(i32),
    PickMaterial(u16),
    Form(i32),
    Thickness,
    Strength,
    #[allow(dead_code)]
    Power,
    Effect(u8),
    Fire,
    Repeat,
    Reset,
    Pause,
    Step,
    Keep,
    Save,
    Camera,
    Frame,
    StandView,
    #[allow(dead_code)]
    Catalog(i32),
    #[allow(dead_code)]
    Place,
    AddPart,
    Aim,
    PlaceAt([i32; 3]),
    AimAt([i32; 3]),
    #[allow(dead_code)]
    Select(i32),
    Move(u8, i32),
    #[allow(dead_code)]
    Rotate,
    Delete,
    Clear,
    Stand,
    Dimension(u8, i32),
    Create,
    CancelCreate,
    SaveElement,
    SaveScene,
    LoadScene,
    SaveWeapon,
    Variant(i32),
    Split,
    ApplyMaterial,
    ApplySize,
    Experiments,
    Experiment(usize),
}
#[derive(Component)]
pub struct Root;
#[derive(Component)]
pub(super) struct Label(u8);

struct Suspended {
    scene: Scene,
    workshop: Workshop,
    panel: panel::Panel,
    overrides: knobs::Overrides,
    pilot: pilot::Pilot,
    projection: Option<Projection>,
}
#[derive(Clone)]
struct ResultRow {
    recipe: Recipe,
    ticks: u32,
    remaining: usize,
    removed: usize,
    burned: u64,
    melted: u64,
    revision: u64,
    shots: u32,
    start: Option<u32>,
}
#[derive(Resource)]
pub struct Laboratory {
    pub definition: Definition,
    pub recipe: Recipe,
    saved: Option<Box<Suspended>>,
    requested: bool,
    specimen: Option<RangeBlock>,
    initial: usize,
    remaining: usize,
    sampled: Option<u32>,
    start: Option<u32>,
    elapsed: u32,
    shots: u32,
    observing: bool,
    deadline: Option<u32>,
    launch: std::collections::VecDeque<scenario::Launch>,
    pub step_requested: bool,
    baseline: Option<ResultRow>,
    hidden: Vec<(Entity, Display)>,
    pub notice: String,
    constructor: constructor::Constructor,
    experiments: Vec<modding::laboratory::experiments::Experiment>,
    events: Vec<modding::laboratory::events::Event>,
    browser: bool,
    metrics: String,
    ui: ui::State,
    tools:tools::State,
}
impl Laboratory {
    pub fn new(requested: bool) -> Self {
        Self {
            definition: Definition::stock(),
            recipe: Recipe::default(),
            saved: None,
            requested,
            specimen: None,
            initial: 0,
            remaining: 0,
            sampled: None,
            start: None,
            elapsed: 0,
            shots: 0,
            observing: false,
            deadline: None,
            launch:Default::default(),
            step_requested: false,
            baseline: None,
            hidden: Vec::new(),
            notice: String::new(),
            constructor: constructor::Constructor::new(),
            experiments: modding::laboratory::experiments::stock(),
            events: modding::laboratory::events::stock(),
            browser: false,
            metrics: String::new(),
            ui: ui::State::default(),
            tools:tools::State::default(),
        }
    }
    pub fn active(&self) -> bool {
        self.saved.is_some()
    }
    pub fn limit_steps(&self, tick: u32, steps: u32) -> u32 {
        if !self.active() || !self.observing || !self.launch.is_empty() {
            return steps;
        }
        self.deadline
            .map_or(steps, |end| steps.min(end.saturating_sub(tick).max(1)))
    }
    fn config(&self) -> ExplosionConfig {
        self.constructor.workspace.sources.get(self.constructor.workspace.selected)
            .map_or_else(|| self.preset_config(), |s| s.resolved())
    }
    fn preset_config(&self) -> ExplosionConfig {
        let p = self.definition.layout.power_milli[self.recipe.power];
        if let Some((mut c, _)) = self.constructor.weapon {
            if matches!(c.delivery, Delivery::Blast | Delivery::Kinetic) {
                return c.at_power(p);
            }
            c.energy = c.energy.saturating_mul(p) / 1000;
            return c;
        }
        match self.recipe.effect {
            1 => weapon_config(false, false, p, Delivery::Kinetic),
            2 => weapon_config(true, false, p, Delivery::Blast),
            3 => weapon_config(false, false, p, Delivery::Laser),
            4 => weapon_config(false, false, p, Delivery::Flame),
            6 => weapon_config(true, true, p, Delivery::Blast),
            7 => weapon_config(false, false, p, Delivery::MachineGun),
            8 => ExplosionConfig::from_ron(include_str!("../../../../../data/nuclear-warhead.ron"))
                .expect("nuclear RON")
                .at_power(p),
            9 => ExplosionConfig::from_ron(include_str!("../../../../../data/siege-shell.ron"))
                .expect("siege RON")
                .at_power(p),
            _ => weapon_config(false, false, p, Delivery::Blast),
        }
    }
    fn prepare(&mut self) -> Result<(BlastRange, Workshop), String> {
        sources::ensure(self)?;
        scenario::sync_sources(self);
        let layout = self.constructor.layout(&self.definition, self.recipe)?;
        scenario::validate(self,&layout)?;
        if let Some(plan) = &self.constructor.experiment {
            plan.validate(&layout)?;
        }
        let specimen = if self.constructor.stand && self.constructor.base.is_none() {
            Some(self.definition.fixture(self.recipe)?.1)
        } else {
            None
        };
        let mut sim = BlastRange::with_layout(self.config(), layout.clone())?;
        sim.execute_command(Command::Gun(GunCommand::Control));
        for &(material, field, value) in &self.constructor.workspace.material {
            let field = match field { 0 => MaterialField::Strength, 1 => MaterialField::Density, _ => MaterialField::Melt };
            if !sim.execute_command(Command::TuneMaterial { material, field, value }).admitted() {
                return Err("Движок отклонил параметр материала".into());
            }
        }
        let material = self.definition.materials[self.recipe.material].id;
        let stock = sim
            .matter
            .economy
            .mats
            .get(material)
            .ok_or("material absent from range registry")?;
        let (material, strength) = self.constructor.strength.unwrap_or((
            material,
            stock
                .strength
                .saturating_mul(self.definition.strength_milli[self.recipe.strength])
                / 1000,
        ));
        if !sim
            .execute_command(Command::TuneMaterial {
                material,
                field: MaterialField::Strength,
                value: strength,
            })
            .admitted()
        {
            return Err("material override rejected".into());
        }
        self.initial = specimen.as_ref().map_or(0, |s| {
            (0..3).map(|a| (s.max[a] - s.min[a]) as usize).product()
        });
        self.remaining = self.initial;
        self.specimen = specimen;
        self.sampled = None;
        self.start = None;
        self.elapsed = 0;
        self.shots = 0;
        self.observing = false;
        self.deadline = None;
        self.launch.clear();
        self.step_requested = false;
        let mut edit = Workshop::new(Some(layout), false);
        edit.zoom = 0.9;
        Ok((sim, edit))
    }
    fn row(&self, sim: &BlastRange) -> ResultRow {
        ResultRow {
            recipe: self.recipe,
            ticks: self.elapsed,
            remaining: self.remaining,
            removed: sim.removed,
            burned: sim.heat.burned_total,
            melted: sim.heat.melted_total,
            revision: self.constructor.revision,
            shots: self.shots,
            start: self.start,
        }
    }
    fn fire(&mut self, scene: &mut Scene) {
        if self.constructor.mode==constructor::Mode::Event {self.notice="Сначала разместите воздействие кликом; Esc — отмена".into();return;}
        if let Some(plan) = &self.constructor.experiment {
            if !self.launch.is_empty() {self.notice="Сценарий ещё выполняет предыдущие команды".into();return;}
            let duration=self.constructor.workspace.observation_ticks.unwrap_or(plan.duration_ticks);
            match scenario::queue(self,scene) {Ok(q)=>self.launch=q,Err(e)=>{self.notice=e;return;}}
            self.begin_observation(scene,duration);
            return;
        }
        if let Err(e) = sources::arm(self, scene) { self.notice = e; return; }
        let command = sources::active(self).filter(|s| s.detonate)
            .map_or(GunCommand::Refire, |s| GunCommand::Detonate { at: s.at });
        if scene
            .sim
            .execute_command(Command::Gun(command))
            .accepted()
        {
            self.begin_observation(scene, self.constructor.workspace.observation_ticks.unwrap_or(self.definition.observation_ticks));
        } else {
            self.notice = "Предыдущее воздействие ещё не завершено".into();
        }
    }
    fn begin_observation(&mut self, scene: &mut Scene, duration: u32) {
        self.start.get_or_insert(scene.sim.tick);
        self.shots += 1;
        self.observing = true;
        self.deadline = Some(scene.sim.tick.saturating_add(duration));
        self.sampled = None;
        scene.paused = false;
        self.notice = "Опыт запущен через общие команды движка".into();
    }
}

fn fresh_scene(sim: BlastRange, old: &Scene) -> Scene {
    Scene {
        sim,
        generation: old.generation + 1,
        energy: false,
        anni: false,
        paused: true,
        frame: 0,
        finish: old.finish,
        out: None,
        all_frames: false,
        wait: 0,
        captured: 0,
        acc: 0.0,
        repeat: false,
        view: 1,
        camera_sweep: false,
        patrol: 0,
        previous_shell: None,
        fps: old.fps,
        step_us: 0,
        flame: 0,
        realtime: old.realtime,
        live_elapsed: 0.0,
        last_capture: 0,
        layout_file: None,
        power: 1000,
        uncovered: false,
        unit_override: None,
        skip_delta: true,
        notice: String::new(),
    }
}

fn label(p: &mut ChildSpawnerCommands, font: &TextFont, text: &str, kind: Option<u8>) {
    let mut e = p.spawn((Text::new(text), font.clone(), TextColor(theme::TEXT)));
    if let Some(kind) = kind {
        e.insert(Label(kind));
    }
}
fn button(p: &mut ChildSpawnerCommands, font: &TextFont, text: &str, action: LabAction) {
    ui::control(
        p,
        font,
        text,
        if action == LabAction::Experiments {
            "x"
        } else {
            "flask-conical"
        },
        action,
    );
}
fn row(p: &mut ChildSpawnerCommands, font: &TextFont, buttons: &[(&str, LabAction)]) {
    p.spawn(Node {
        column_gap: px(6),
        row_gap: px(5),
        flex_wrap: FlexWrap::Wrap,
        ..default()
    })
    .with_children(|p| {
        for &(text, action) in buttons {
            button(p, font, text, action);
        }
    });
}
fn root(node: Node, camera: Entity) -> impl Bundle {
    (
        Root,
        node,
        BackgroundColor(theme::PANEL),
        BorderColor::all(theme::FRAME),
        ZIndex(80),
        UiTargetCamera(camera),
        bevy::ui::RelativeCursorPosition::default(),
    )
}
pub fn build(commands: &mut Commands, camera: Entity, font: &TextFont) {
    ui::build(commands, camera, font);
    let mut live_font=font.clone();live_font.font_size=14.0.into();
    first_person::build_ui(commands,camera,&live_font);
}

/// All changes go through the same scene boundary; no second tick loop.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn actions(
    mut commands: Commands,
    mut lab: ResMut<Laboratory>,
    mut scene: ResMut<Scene>,
    mut edit: ResMut<Workshop>,
    mut panel: ResMut<panel::Panel>,
    (mut overrides, mut pilot, profile, mut streaming, mut visual, mut effects, mut gas): (
        ResMut<knobs::Overrides>,
        ResMut<pilot::Pilot>,
        ResMut<profile::Profile>,
        ResMut<streaming::Streaming>,
        ResMut<moving::Visual>,
        ResMut<fire::Effects>,
        ResMut<GasInput>,
    ),
    inputs: Query<(&LabAction, &Interaction), Changed<Interaction>>,
    open: Query<(&panel::Action, &Interaction), Changed<Interaction>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut projections: Query<&mut Projection, With<GasCamera>>,
    mut cursors: Query<&mut bevy::window::CursorOptions>,
    ejecta: Query<Entity, With<Ejecta>>,
) {
    let startup = lab.requested;
    let enter = startup
        || open
            .iter()
            .any(|(a, i)| *i == Interaction::Pressed && *a == panel::Action::Laboratory);
    lab.requested = false;
    let mut transition = false;
    if enter && !lab.active() {
        if (scene.out.is_some() && !startup) || scene.generation >= u64::MAX - 1 {
            scene.notice = "Лаборатория доступна в интерактивной сессии".into();
            return;
        }
        let capture_fixture = startup && scene.out.is_some() && ui::capture_fixture(&mut lab);
        match lab.prepare() {
            Err(e) => {
                scene.notice = e;
                return;
            }
            Ok((mut sim, workshop)) => {
                if capture_fixture {
                    let case=std::env::var("RTS_LAB_CAPTURE_CASE").unwrap_or_default();
                    if case=="melt-steel" {
                        sim.set_physics_workers(1);sim.set_navigation_workers(1);
                        sim.execute_command(Command::ConfigureWeapon{config:weapon_config(false,false,1000,Delivery::Laser)});
                        sim.execute_command(Command::Gun(GunCommand::LabStream{origin:[350,30,345],target:[410,30,345],enabled:true}));
                        for _ in 0..24 {sim.step();}
                        println!("MELT_CAPTURE melted={} hot={}",sim.heat.melted_total,sim.heat.hot());
                    }
                    if case.starts_with("collector-") {
                        sim.set_physics_workers(1);sim.set_navigation_workers(1);
                        experiments::launch(lab.constructor.experiment.as_ref().unwrap(),&mut sim).expect("salvage scenario");
                        for _ in 0..match case.as_str(){"collector-before"=>20,"collector-profile"=>180,_=>900} {sim.step();}
                    }
                    if case.starts_with("dune-") {
                        sim.set_physics_workers(1);sim.set_navigation_workers(1);
                        experiments::launch(lab.constructor.experiment.as_ref().unwrap(),&mut sim).expect("dune scenario");
                        for _ in 0..match case.as_str(){"dune-during"=>35,"dune-after"=>180,_=>0} {sim.step();}
                    }
                    if case.starts_with("water-gravity-") {
                        sim.set_physics_workers(1);sim.set_navigation_workers(1);
                        for _ in 0..match case.as_str(){"water-gravity-during"=>8,"water-gravity-after"=>60,_=>0} {sim.step();}
                    }
                    let ticks=match case.as_str(){"material-result"|"water-result"|"water-float"=>300,"physics-wetting"=>120,"physics-erosion"=>300,"physics-devices"=>40,"physics-dome"=>8,"physics-glass"=>100,_=>0};
                    if case.starts_with("he-fragments-") {
                        sim.set_physics_workers(1);sim.set_navigation_workers(1);
                        sim.execute_command(Command::ConfigureWeapon{config:weapon_config(false,false,1000,Delivery::Blast)});
                        assert!(sim.execute_command(Command::Gun(GunCommand::Detonate{at:[392,20,344]})).accepted());
                        for _ in 0..if case.ends_with("flight") {16}else{140} {sim.step();}
                        let pieces=&sim.matter.debris.fragments;
                        println!("HE_LANDING case={case} fragments={} flying={} resting={} inventory={} geometry={}",pieces.len(),pieces.iter().filter(|f|!f.resting).count(),pieces.iter().filter(|f|f.resting).count(),pieces.iter().map(|f|u64::from(f.count)).sum::<u64>(),pieces.iter().map(|f|u64::from(f.shape.count_ones())).sum::<u64>());
                    }
                    if ticks>0 {
                        sim.set_physics_workers(1);sim.set_navigation_workers(1);
                        if case.starts_with("physics-") {if let Some(plan)=&lab.constructor.experiment {experiments::launch(plan,&mut sim).expect("capture experiment input");}}
                        for _ in 0..ticks {sim.step();}
                    }
                }
                let mut replacement = fresh_scene(sim, &scene);
                if startup {
                    replacement.out = scene.out.clone();
                    replacement.all_frames = scene.all_frames;
                }
                let projection = projections.iter_mut().next().map(|mut p| {
                    std::mem::replace(
                        &mut *p,
                        Projection::Orthographic(OrthographicProjection::default_3d()),
                    )
                });
                lab.saved = Some(Box::new(Suspended {
                    scene: std::mem::replace(&mut *scene, replacement),
                    workshop: std::mem::replace(&mut *edit, workshop),
                    panel: std::mem::replace(&mut *panel, panel::Panel::new(true, false)),
                    overrides: std::mem::replace(
                        &mut *overrides,
                        knobs::Overrides::new(false, true),
                    ),
                    pilot: std::mem::replace(&mut *pilot, pilot::Pilot::default()),
                    projection,
                }));
                for mut c in &mut cursors {
                    c.visible = true;
                    c.grab_mode = bevy::window::CursorGrabMode::None;
                }
                if !capture_fixture {
                    lab.constructor.mode = constructor::Mode::Select;
                }
                lab.constructor.pending.clear();
                lab.browser = false;
                lab.notice = "Лаборатория готова. Соберите сцену и выберите воздействие.".into();
                if capture_fixture && std::env::var("RTS_LAB_CAPTURE_CASE").is_ok_and(|s|s.starts_with("collector-")) {
                    let profile=std::env::var("RTS_LAB_CAPTURE_CASE").as_deref()==Ok("collector-profile");
                    let centre=Vec3::new(367.0,20.0,336.0);
                    lab.tools.camera=Some(Transform::from_translation(centre+Vec3::new(-64.0,62.0,90.0)).looking_at(centre,Vec3::Y));
                    lab.tools.flying=true;lab.tools.flight.preview=true;lab.tools.flight.profile=profile;
                    scene.paused=!profile;scene.realtime=true;scene.acc=0.0;scene.skip_delta=true;
                }
                if capture_fixture && std::env::var("RTS_LAB_CAPTURE_CASE").is_ok_and(|s|s.starts_with("dune-")) {
                    let profile=std::env::var("RTS_LAB_CAPTURE_CASE").as_deref()==Ok("dune-profile");
                    let centre=Vec3::new(360.0,28.0,344.0);
                    lab.tools.camera=Some(Transform::from_translation(centre+Vec3::new(100.0,95.0,112.0)).looking_at(centre,Vec3::Y));
                    lab.tools.flying=true;lab.tools.flight.preview=true;lab.tools.flight.profile=profile;
                    scene.paused=!profile;scene.realtime=true;scene.acc=0.0;scene.skip_delta=true;
                }
                if capture_fixture && std::env::var("RTS_LAB_CAPTURE_CASE").as_deref()==Ok("melt-steel") {
                    lab.tools.flight.preview=true;scene.paused=true;scene.realtime=true;
                }
                if capture_fixture && std::env::var("RTS_LAB_CAPTURE_CASE").is_ok_and(|s|s.starts_with("water-gravity-")) {
                    let profile=std::env::var("RTS_LAB_CAPTURE_CASE").as_deref()==Ok("water-gravity-profile");
                    let centre=Vec3::new(384.0,22.0,336.0);
                    lab.tools.camera=Some(Transform::from_translation(centre+Vec3::new(52.0,48.0,66.0)).looking_at(centre,Vec3::Y));
                    lab.tools.flying=true;lab.tools.flight.preview=true;lab.tools.flight.profile=profile;
                    scene.paused=!profile;scene.realtime=true;scene.acc=0.0;scene.skip_delta=true;
                }
                if capture_fixture && std::env::var("RTS_LAB_CAPTURE_CASE").as_deref()==Ok("surround") {
                    scene.view = 6;
                    edit.zoom = 0.82;
                }
                if capture_fixture && std::env::var("RTS_LAB_CAPTURE_CASE").is_ok_and(|v|v.starts_with("fps-")) {
                    let case=std::env::var("RTS_LAB_CAPTURE_CASE").unwrap();
                    let target=Vec3::new(384.0,26.0,400.0);
                    lab.tools.camera=Some(Transform::from_translation(Vec3::new(324.0,70.0,494.0)).looking_at(target,Vec3::Y));
                    live_tools::action(&mut lab,&mut scene,LabAction::LiveTools);
                    live_tools::action(&mut lab,&mut scene,LabAction::LiveTool(if case.starts_with("fps-water"){2}else if case.starts_with("fps-fire"){3}else if case.starts_with("fps-mg"){5}else if case.starts_with("fps-he"){6}else if case.starts_with("fps-ap"){7}else{4}));
                    lab.tools.flight.preview=true;
                    lab.tools.flight.profile=case.ends_with("profile");
                    if case=="fps-water-profile" {lab.tools.point=true;}
                    lab.tools.flight.captured=case!="fps-menu";
                    lab.tools.flight.request=false;
                    lab.tools.flight.trigger_ready=true;
                    let pose=lab.tools.camera.unwrap();
                    if case!="fps-menu" {
                        if let Some((at,origin))=live_tools::target(&lab,&scene,Ray3d::new(pose.translation,pose.forward())) {
                            for _ in 0..if case=="fps-fire" {6}else{1} {
                                effects.before(&scene.sim);
                                live_tools::action(&mut lab,&mut scene,LabAction::LiveApply(at,origin));
                                live_tools::tick(&mut lab,&mut scene);
                                scene.sim.step();
                                effects.capture(&scene.sim);
                            }
                        }
                    }
                    scene.paused=!case.ends_with("profile");
                }
                if capture_fixture && std::env::var("RTS_LAB_CAPTURE_CASE").as_deref()==Ok("live-tools") {
                    let p=Vec3::new(378.0,24.0,398.0);
                    lab.tools.camera=Some(Transform::from_translation(p+Vec3::new(-30.0,28.0,38.0)).looking_at(p,Vec3::Y));
                    live_tools::action(&mut lab,&mut scene,LabAction::LiveTools);
                    live_tools::action(&mut lab,&mut scene,LabAction::LiveTool(2));
                    lab.tools.point=true;
                    for i in 0..16 {
                        live_tools::action(&mut lab,&mut scene,LabAction::LiveApply([374+i/4,40,398],[350,50,430]));
                        live_tools::tick(&mut lab,&mut scene);
                        scene.sim.step();
                    }
                }
                if capture_fixture && std::env::var("RTS_LAB_CAPTURE_CASE").as_deref()==Ok("fragment-inspector") {
                    let sim=&mut scene.sim;
                    sim.world.queue_transfer(voxel::Edit::put(384,16,336,7));sim.world.flush_transfers();
                    let (made,returned)=sim.matter.debris.pieces_from_cells(&mut sim.world,&[([384,20,336],3)],&mut sim.matter.economy,&sim.matter.tuning,2);
                    assert_eq!((made,returned),(1,0));
                    for _ in 0..30 {sim.step();}
                    let f=&sim.matter.debris.fragments[0];let centre=Vec3::from([f.position.x,f.position.y,f.position.z].map(|v|v.raw() as f32/fx::ONE_RAW as f32));
                    let origin=centre+Vec3::new(5.0,4.0,7.0);let direction=(centre-origin).normalize();
                    let hit=fragments::picking::pick(&scene,Ray3d::new(origin,Dir3::new(direction).unwrap()),f32::INFINITY).expect("steel fragment probe");
                    let at=centre.floor().to_array().map(|v|v as i32);
                    tools::inspector::select(&mut lab,&scene,at);lab.tools.fragment=Some(hit);lab.tools.probe_stamp=None;lab.tools.probe_direction=Some(direction);tools::inspector::refresh(&mut lab,&scene);
                    lab.tools.camera=Some(Transform::from_translation(origin).looking_at(centre,Vec3::Y));lab.tools.flying=true;
                    println!("FRAGMENT_PROBE id={} material={} label_steel={}",hit.id,scene.sim.matter.debris.fragments[0].material,lab.tools.probe.contains("Сталь · ID 3"));
                }
                transition = true;
            }
        }
    }
    if !lab.active() {
        return;
    }
    parameters::keyboard(&mut lab, &mut keys);
    let mut actions = std::mem::take(&mut lab.constructor.pending);
    actions.extend(
        inputs
            .iter()
            .filter(|(_, i)| **i == Interaction::Pressed)
            .map(|(a, _)| *a),
    );
    if keys.just_pressed(KeyCode::Escape) {
        if lab.ui.help {
            lab.ui.help = false;
        } else if lab.browser {
            lab.browser = false;
        } else if lab.constructor.voxel_editing() {
            actions.push(LabAction::VoxelFinish(true));
        } else if lab.tools.live {
            actions.push(LabAction::LiveTools);
        } else if lab.constructor.mode != constructor::Mode::Select {
            lab.constructor.mode = constructor::Mode::Select;
            lab.constructor.hover = None;
        } else {
            lab.constructor.selected = None;
        }
    }
    for (key, action) in [
        (KeyCode::KeyF, if lab.constructor.voxel_editing() {LabAction::Frame}else{LabAction::Fire}),
        (KeyCode::NumpadDecimal, LabAction::Frame),
        (KeyCode::F5, LabAction::Repeat),
        (KeyCode::Space, LabAction::Pause),
        (KeyCode::KeyV, LabAction::Tool(0)),
        (KeyCode::KeyI, LabAction::Tool(4)),
        (KeyCode::KeyG, LabAction::Tool(1)),
        (KeyCode::KeyB, LabAction::AddPart),
        (KeyCode::KeyT, LabAction::Aim),
        (KeyCode::KeyQ, LabAction::Turn(-1)),
        (KeyCode::KeyE, LabAction::Turn(1)),
        (KeyCode::KeyR, LabAction::Turn(1)),
        (KeyCode::PageUp, LabAction::Elevation(1)),
        (KeyCode::PageDown, LabAction::Elevation(-1)),
        (KeyCode::Delete, LabAction::Delete),
        (KeyCode::F1, LabAction::Help),
        (KeyCode::Home, LabAction::Frame),
        (KeyCode::F6, LabAction::StandView),
        (KeyCode::F7, LabAction::SectionToggle),
        (KeyCode::F2, LabAction::LiveTools),
    ] {
        if keys.just_pressed(key) {
            actions.push(action);
        }
        keys.reset(key);
    }
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) {
        if keys.just_pressed(KeyCode::KeyD) {
            actions.push(LabAction::Duplicate);
        }
        if keys.just_pressed(KeyCode::KeyS) {
            actions.push(LabAction::SaveScene);
        }
    }
    // Consume editor shortcuts before the range/weapon handlers.
    for key in [
        KeyCode::Escape,
        KeyCode::KeyD,
        KeyCode::KeyS,
        KeyCode::KeyW,
        KeyCode::KeyA,
        KeyCode::KeyZ,
        KeyCode::KeyX,
        KeyCode::KeyP,
        KeyCode::Enter,
    ] {
        keys.reset(key);
    }
    for action in actions {
        if !ui::enabled(&lab, action) {
            continue;
        }
        if action == LabAction::Back {
            let generation = scene
                .generation
                .checked_add(1)
                .expect("reserved scene generation");
            let mut saved = lab.saved.take().expect("active session");
            saved
                .panel
                .rebind_generation(saved.scene.generation, generation);
            saved.pilot.rebind_generation(generation);
            saved.scene.generation = generation;
            saved.scene.skip_delta = true;
            saved.scene.acc = 0.0;
            *scene = saved.scene;
            *edit = saved.workshop;
            *panel = saved.panel;
            *overrides = saved.overrides;
            *pilot = saved.pilot;
            if let Some(p) = saved.projection {
                for mut current in &mut projections {
                    *current = p.clone();
                }
            }
            for mut c in &mut cursors {
                c.visible = !pilot.active();
                c.grab_mode = if pilot.active() {
                    bevy::window::CursorGrabMode::Locked
                } else {
                    bevy::window::CursorGrabMode::None
                };
            }
            lab.step_requested = false;
            transition = true;
            break;
        }
        if !lab.constructor.voxel_allowed(action) {
            lab.notice="Завершите редактирование детали кнопкой «Готово» или «Отмена»".into();
            continue;
        }
        if !live_tools::allowed(&lab,action) {lab.notice="Завершите режим «Живые инструменты» кнопкой «Готово» или Esc".into();continue;}
        if lab.tools.live && !matches!(action,LabAction::LiveApply(_,_)|LabAction::LiveRelease) {
            live_tools::stop(&mut lab,&mut scene);lab.tools.live_tick=None;
        }
        let before = lab.recipe;
        if !matches!(action,LabAction::EditNumber(_) | LabAction::ApplyNumber){lab.ui.editing=None;lab.ui.edit_error=None;}
        let mut rebuild = false;
        let mut fire = false;
        match action {
            LabAction::SectionToggle => section::toggle(&mut lab, &scene),
            LabAction::SectionShift(delta) => section::shift(&mut lab, delta),
            LabAction::SectionFocus => section::focus(&mut lab, &scene),
            LabAction::LiveTools|LabAction::LiveTool(_)|LabAction::LivePoint|LabAction::LivePower(_)|LabAction::LiveApply(_,_)|LabAction::LiveRelease|LabAction::LiveCapture|LabAction::LiveSpeed(_)|LabAction::LiveLook(_)|LabAction::LiveFov(_)=> {
                live_tools::action(&mut lab,&mut scene,action);
                configure_gas(&scene.sim,scene.energy,scene.anni,&mut gas);
            },
            LabAction::BeginVoxelEdit | LabAction::Tab(17) => {
                let l=&mut *lab;
                match l.constructor.action(LabAction::BeginVoxelEdit,&l.definition,&mut l.recipe) {
                    Ok((changed,message))=>{
                        rebuild=changed;l.ui.tab=17;l.notice=message;
                        if changed {if let Some(item)=l.constructor.selected.and_then(|i|l.constructor.placed.get(i)) {
                            if let Some(b)=item.element.blocks.iter().find(|b|b.material!=0) {
                                l.tools.selected=Some(modding::laboratory::catalog::voxels::world_cell(item,b.min));
                                let rotation=Quat::from_rotation_y(-(item.turns as f32)*std::f32::consts::FRAC_PI_2);
                                l.tools.probe_direction=Some(rotation*Vec3::new(6.0,-5.0,6.0).normalize());
                                orbit::frame(l);
                            }
                        }}
                    },
                    Err(e)=>{l.notice=e;continue;}
                }
            },
            LabAction::Tool(4)=> {
                lab.constructor.mode=constructor::Mode::Inspect;lab.constructor.hover=None;lab.constructor.selected=None;
                lab.ui.tab=14;lab.browser=false;lab.notice="Пипетка: ЛКМ — выбрать куб мира 10 × 10 × 10 см".into();
            },
            LabAction::ProbeMove(axis,delta)=>tools::inspector::shift(&mut lab,&scene,axis as usize,delta),
            LabAction::ProbeFocus=>if lab.constructor.voxel_editing() {orbit::frame(&mut lab)}else{tools::inspector::focus(&mut lab)},
            LabAction::ClearProbe=> {lab.tools.selected=None;lab.tools.falling=None;lab.tools.fragment=None;lab.tools.probe_stamp=None;lab.tools.probe="Выберите кубик кликом в сцене".into();},
            LabAction::SurfaceSnap=> {let v=lab.constructor.workspace.surface_snap.unwrap_or(true);lab.constructor.workspace.surface_snap=Some(!v);lab.constructor.revision+=1;},
            LabAction::AllowOverlap=> {lab.constructor.workspace.allow_overlap=!lab.constructor.workspace.allow_overlap;lab.constructor.revision+=1;},
            LabAction::Flying=> {lab.tools.flying=!lab.tools.flying;lab.tools.camera=None;},
            LabAction::BrushSize(delta)=> {lab.tools.edge=(lab.tools.edge as i32+delta).clamp(1,8) as u32;},
            LabAction::BrushDistance(delta)=> {lab.tools.distance=(lab.tools.distance+delta as f32*4.0).clamp(4.0,256.0);},
            LabAction::Sculpt(at,erase)=> {
                let material=lab.constructor.material(&lab.definition,lab.recipe);
                let outcome=scene.sim.execute_command(Command::Sculpt{at,edge:lab.tools.edge,material,erase});
                if outcome==sim_core::blast::battle::commands::Outcome::Queued {lab.step_requested=true;}
                lab.tools.last_tick=Some(scene.sim.tick);
            },
            LabAction::DeviceAt(kind,at)=> {
                if lab.constructor.workspace.devices.len()>=modding::lab_tools::Tuning::stock().devices {lab.notice="Достигнут лимит устройств".into();continue;}
                let turns=lab.constructor.turns;
                lab.constructor.workspace.devices.push(modding::lab_tools::Device{kind,at,turns,work:0,dug:0,container:None,
                    collector:(kind==modding::lab_tools::DeviceKind::Collector).then(||modding::lab_tools::Collector::at(at))});
                lab.constructor.revision+=1;rebuild=true;
            },
            LabAction::ReturnConstruction => {
                let l=&mut *lab;
                match l.constructor.return_scene(&mut l.recipe) {
                    Ok(())=>{rebuild=true;if let Some(step)=lab.constructor.workspace.editor_step {lab.definition.editor_step=step;} lab.ui.tab=0;lab.browser=false;lab.notice="Вернулись к вашей сборке и её воздействиям".into();},
                    Err(e)=>{lab.notice=e;continue;}
                }
            }
            LabAction::PickEvent(i) | LabAction::PlaceEvent(i, _) => {
                match events::action(&mut lab, action, &mut scene) {
                    Ok(changed) => {rebuild=changed; configure_gas(&scene.sim,scene.energy,scene.anni,&mut gas);},
                    Err(e) => {lab.notice=e;continue;}
                }
                let _=i;
            }
            LabAction::LocalCharge(local) => {
                let before=lab.constructor.workspace.clone();
                if let Some(source)=sources::active_mut(&mut lab) {
                    source.detonate=local;
                    if local {source.binding=None;}
                }
                if let Err(e)=sources::arm(&lab,&mut scene) {lab.constructor.workspace=before;lab.notice=e;continue;}
                scenario::sync_sources(&mut lab);
                lab.constructor.revision+=1;
                if !local {lab.constructor.mode=constructor::Mode::Aim;lab.notice="Выберите цель выстрела кликом, затем нажмите F".into();}
            }
            LabAction::Source(_) | LabAction::AddSource | LabAction::RemoveSource | LabAction::BindSource(_) | LabAction::SourcePosition | LabAction::SourceAt(_) => {
                match sources::action(&mut lab, action, &mut scene) {
                    Ok(changed) => { rebuild=changed; configure_gas(&scene.sim,scene.energy,scene.anni,&mut gas); },
                    Err(e) => {lab.notice=e;continue;}
                }
            }
            LabAction::ScenarioInput(_)|LabAction::ScenarioAdd(_)|LabAction::ScenarioRemove|LabAction::ScenarioMove(_)|LabAction::ScenarioAt(_)|LabAction::Device(_)|LabAction::RemoveDevice=> {
                match scenario::action(&mut lab,action,&scene) {Ok(changed)=>rebuild=changed,Err(e)=>{lab.notice=e;continue;}}
            }
            LabAction::EditNumber(_) | LabAction::ApplyNumber | LabAction::CancelNumber | LabAction::SetNumber(_,_) | LabAction::AdjustNumber(_,_) => {
                match parameters::action(&mut lab,action,&mut scene) {
                    Ok(changed) => {rebuild=changed; configure_gas(&scene.sim,scene.energy,scene.anni,&mut gas);},
                    Err(e) => {lab.notice=e;continue;}
                }
            }
            LabAction::Category(category) => {
                lab.ui.category = category;
                lab.ui.page = 0;
            }
            LabAction::Page(delta) => {
                lab.ui.page = (lab.ui.page as i32 + delta).max(0) as usize;
            }
            LabAction::Tab(tab) => {if tab==14 {lab.constructor.mode=constructor::Mode::Inspect;lab.constructor.selected=None;lab.browser=false;} if tab==1 {let l=&mut *lab;l.constructor.open_materials(&l.definition);}lab.ui.tab=tab;},
            LabAction::Grid => lab.ui.grid = !lab.ui.grid,
            LabAction::Help => lab.ui.help = !lab.ui.help,
            LabAction::ParameterDetails => lab.ui.parameter_details=!lab.ui.parameter_details,
            LabAction::Form(delta) => {
                lab.recipe.form = (lab.recipe.form as i32 + delta)
                    .rem_euclid(lab.definition.forms.len() as i32)
                    as usize;
                if lab.recipe.effect == 5 && !lab.definition.forms[lab.recipe.form].supports {
                    lab.recipe.effect = 0;
                }
            }
            LabAction::Thickness => {
                lab.recipe.thickness = (lab.recipe.thickness + 1) % lab.definition.thickness.len()
            }
            LabAction::Strength => {
                lab.recipe.strength =
                    (lab.recipe.strength + 1) % lab.definition.strength_milli.len();
                lab.constructor.strength = None;
            }
            LabAction::Power => {
                lab.recipe.power = (lab.recipe.power + 1) % lab.definition.layout.power_milli.len();
                let power=lab.definition.layout.power_milli[lab.recipe.power];
                if let Some(source)=sources::active_mut(&mut lab){source.power_milli=power;}
            }
            LabAction::Effect(effect) => {
                if !scene.paused && !scene.sim.ready() {
                    lab.notice = "Дождитесь завершения опыта или нажмите «Сброс»".into();
                    continue;
                }
                if effect == 5
                    && (!lab.constructor.stand || !lab.definition.forms[lab.recipe.form].supports)
                {
                    lab.notice = "Выберите форму «Балка на опорах»".into();
                    continue;
                }
                lab.constructor.weapon = None;
                if lab.constructor.base.as_ref().is_some_and(|b|b.battle) {lab.notice="Штатное оружие участника настраивается командой сценария и самим юнитом".into();continue;}
                lab.constructor.variant = None;
                lab.recipe.effect = effect;
                rebuild=true;
                let power=lab.recipe.power;
                lab.recipe.power=lab.definition.layout.power_milli.iter().position(|p|*p==1000).unwrap_or(0);
                let config=lab.preset_config();
                lab.recipe.power=power;
                if let Err(e)=sources::ensure_weapon(&mut lab){lab.notice=e;continue;}
                if let Some(source)=sources::active_mut(&mut lab) {source.config=config;source.effect=Some(effect);source.visual=if effect==6{2}else if effect==2{1}else{0};if config.delivery!=Delivery::Blast{source.detonate=false;}}
                scenario::sync_sources(&mut lab);
            }
            LabAction::Fire => lab.fire(&mut scene),
            LabAction::Experiments => lab.browser = !lab.browser,
            LabAction::Experiment(index) => match experiments::select(&mut lab, index) {
                Ok(()) => {
                    edit.pan = Vec2::ZERO;
                    edit.zoom = 0.9;
                    edit.orbit = 0.0;
                    edit.pitch = 0.0;
                    rebuild = true;
                }
                Err(e) => {
                    lab.notice = e;
                    continue;
                }
            },
            LabAction::Pause => scene.paused = !scene.paused,
            LabAction::Step => {
                scene.paused = true;
                lab.step_requested = true;
            }
            LabAction::Reset | LabAction::Repeat => {
                rebuild = true;
                fire = action == LabAction::Repeat;
            }
            LabAction::Keep => {
                if lab.shots == 0 {
                    lab.notice = "Сначала проведите опыт".into();
                } else {
                    lab.sampled = None;
                    sample(&mut lab, &scene.sim);
                    lab.baseline = Some(lab.row(&scene.sim));
                    lab.notice = "Результат A сохранён в этой сессии".into();
                }
            }
            LabAction::Save => {
                lab.sampled = None;
                sample(&mut lab, &scene.sim);
                lab.notice = save_report(&lab, &scene.sim)
                    .map_or_else(|e| e, |_| "Отчёт: out/material-lab/latest.txt".into());
            }
            LabAction::StandView => {
                scene.view = if scene.view == 6 { 1 } else { 6 };
                edit.pan = Vec2::ZERO;
                edit.zoom = 1.0;
                edit.orbit = 0.0;
                edit.pitch = 0.0;
            }
            LabAction::Frame => {
                if lab.constructor.voxel_editing() {orbit::frame(&mut lab);continue;}
                scene.view = 1;
                edit.pan = Vec2::ZERO;
                edit.zoom = 0.9;
                edit.orbit = 0.0;
                edit.pitch = 0.0;
                scene.view = 1;
            }
            LabAction::Camera => {
                edit.orbit += std::f32::consts::FRAC_PI_4;
            }
            LabAction::AimAt(at) => {
                let mut layout = scene.sim.layout.clone().expect("laboratory layout");
                layout.target = at;
                if let Err(e) = layout.validate() {
                    lab.notice = format!("Точка вне испытательной зоны: {e}");
                    continue;
                }
                if scene
                    .sim
                    .execute_command(Command::Gun(GunCommand::Aim { at: Some(at) }))
                    .admitted()
                {
                    lab.constructor.aim = Some(at);
                    if let Some(source)=sources::active_mut(&mut lab) {source.target=at;}
                    lab.constructor.mode = constructor::Mode::Select;
                    lab.constructor.revision += 1;
                    lab.notice = "Точка воздействия выбрана".into();
                }
            }
            LabAction::SaveScene => {
                lab.notice = save_scene(&lab, &scene.sim)
                    .map_or_else(|e| e, |_| format!("Сцена: {}", constructor::PROJECT));
            }
            LabAction::LoadScene => match load_scene(&mut lab) {
                Ok(()) => {
                    rebuild = true;
                }
                Err(e) => {
                    lab.notice = e;
                    continue;
                }
            },
            LabAction::SaveWeapon => {
                lab.notice = save_weapon(&mut lab).unwrap_or_else(|e| e);
            }
            LabAction::Variant(delta) => {
                let count = lab.constructor.weapons.len();
                if count == 0 {
                    lab.notice = "Сначала сохраните вариант оружия".into();
                    continue;
                }
                let i = (lab
                    .constructor
                    .variant
                    .map_or(if delta > 0 { -1 } else { 0 }, |i| i as i32)
                    + delta)
                    .rem_euclid(count as i32) as usize;
                let w = lab.constructor.weapons[i].clone();
                if lab.constructor.base.as_ref().is_some_and(|b|b.battle) {lab.notice="Выберите свободный источник в небойном сценарии".into();continue;}
                lab.constructor.weapon = Some((w.config, w.visual));
                if let Some(source)=sources::active_mut(&mut lab) {source.config=w.config;source.power_milli=1000;source.visual=w.visual;source.effect=None;source.detonate=false;}
                scenario::sync_sources(&mut lab);
                lab.constructor.variant = Some(i);
                lab.recipe.power = lab
                    .definition
                    .layout
                    .power_milli
                    .iter()
                    .position(|p| *p == 1000)
                    .unwrap_or(0);
                lab.constructor.revision += 1;
                lab.notice = w.name;
                rebuild = true;
            }
            LabAction::Back => unreachable!(),
            other => {
                let l = &mut *lab;
                match l.constructor.action(other, &l.definition, &mut l.recipe) {
                    Ok((changed, message)) => {
                        if matches!(
                            other,
                            LabAction::PickCatalog(_)
                                | LabAction::PickObject(_)
                                | LabAction::AddPart
                                | LabAction::Tool(1)
                        ) {
                            if !(matches!(other,LabAction::PickObject(_)) && l.ui.tab==1) {l.ui.tab=0;}
                        }
                        if matches!(other,LabAction::VoxelFinish(_)) {orbit::leave(l);l.ui.tab=0;}
                        if other == LabAction::SaveElement {
                            l.ui.category = 6;
                            l.ui.page = 0;
                        }
                        rebuild = changed;
                        if !message.is_empty() {
                            l.notice = message;
                        }
                    }
                    Err(e) => {
                        l.notice = e;
                        continue;
                    }
                }
            }
        }
        if before != lab.recipe || rebuild {
            if scene.generation >= u64::MAX - 1 {
                lab.recipe = before;
                lab.notice = "Scene generation exhausted".into();
                continue;
            }
            match lab.prepare() {
                Ok((sim, mut workshop)) => {
                    workshop.pan = edit.pan;
                    workshop.zoom = edit.zoom;
                    workshop.orbit = edit.orbit;
                    workshop.pitch = edit.pitch;
                    *scene = fresh_scene(sim, &scene);
                    *edit = workshop;
                    if before != lab.recipe && action!=LabAction::ApplyMaterial {
                        lab.notice = "Сцена восстановлена с новыми параметрами".into();
                    }
                    transition = true;
                    if fire {
                        lab.fire(&mut scene);
                    }
                }
                Err(e) => {
                    lab.recipe = before;
                    lab.notice = e;
                }
            }
        }
    }
    if transition {
        mouse.clear();
        if lab.active() {
            let visual = lab.constructor.weapon.map_or(
                match lab.recipe.effect {
                    2 => 1,
                    6 => 2,
                    _ => 0,
                },
                |(_, v)| v,
            );
            scene.energy = visual != 0;
            scene.anni = visual == 2;
            scene.power = sources::active(&lab).map_or(lab.definition.layout.power_milli[lab.recipe.power],|s|s.power_milli);
            if let Some(source)=sources::active(&lab){scene.energy=source.visual!=0;scene.anni=source.visual==2;}
        }
        profile.configure_workers(&mut scene.sim);
        moving::reset(&mut visual);
        if !(scene.out.is_some() && lab.tools.live && lab.tools.flight.preview) {effects.clear();}
        gas.atmosphere.clear();
        gas.params.control.z = 0.0;
        configure_gas(&scene.sim, scene.energy, scene.anni, &mut gas);
        streaming.invalidate();
        for e in &ejecta {
            commands.entity(e).despawn();
        }
    }
}

fn sample(lab: &mut Laboratory, sim: &BlastRange) {
    if lab
        .sampled
        .is_some_and(|tick| sim.tick >= tick && sim.tick - tick < lab.definition.measurement_ticks)
    {
        return;
    }
    lab.metrics = experiments::readout(sim);
    lab.sampled = Some(sim.tick);
    let Some(s) = &lab.specimen else { return };
    let mut remaining = 0;
    // Authored volume is <=16384 cells. This runs at the data-defined cadence,
    // never scans the map, and is absent entirely outside the laboratory.
    for z in s.min[2]..s.max[2] {
        for y in s.min[1]..s.max[1] {
            for x in s.min[0]..s.max[0] {
                remaining +=
                    usize::from(sim_core::blast::material(&sim.world, [x, y, z]) == s.material);
            }
        }
    }
    lab.remaining = remaining;
    lab.sampled = Some(sim.tick);
}
pub fn measure(mut lab: ResMut<Laboratory>, mut scene: ResMut<Scene>) {
    if !lab.active() {
        return;
    }
    lab.elapsed = lab.start.map_or(0, |t| scene.sim.tick.saturating_sub(t));
    // A paused single step must show its exact tick; quiet paused frames reuse it.
    if scene.paused && lab.sampled != Some(scene.sim.tick) {
        lab.sampled = None;
    }
    if lab.observing && lab.launch.is_empty() && lab.deadline.is_some_and(|end| scene.sim.tick >= end) {
        scene.paused = true;
        lab.observing = false;
        lab.sampled = None;
        lab.notice = "Окно наблюдения завершено; можно продолжить или повторить".into();
    }
    sample(&mut lab, &scene.sim);
}
fn readout(lab: &Laboratory, sim: &BlastRange) -> String {
    let edge = u64::from(voxel::unit::WORLD_MM);
    let volume = lab.initial as f64 * (edge * edge * edge) as f64 / 1e9;
    let material = lab.definition.materials[lab.recipe.material].id;
    let density = sim
        .matter
        .economy
        .mats
        .get(material)
        .map_or(0, |m| m.density);
    let sample = if lab.initial == 0 {
        format!(
            "Сцена: {} блоков · {} юнитов",
            sim.layout.as_ref().map_or(0, |l| l.blocks.len()),
            sim.units.len()
        )
    } else {
        format!(
            "Образец: {} клеток · {:.3} м³ · исходная масса {:.2} кг",
            lab.initial,
            volume,
            volume * density as f64 * 10.0
        )
    };
    let remaining = if lab.initial > 0 {
        format!(
            "В исходном объёме образца: {} / {} ({:.1}%)",
            lab.remaining,
            lab.initial,
            100.0 * lab.remaining as f64 / lab.initial as f64
        )
    } else {
        format!(
            "Юнитов в строю: {} / {} · повреждено клеток корпуса: {}",
            sim.units.iter().filter(|u| u.status.alive).count(),
            sim.units.len(),
            sim.units
                .iter()
                .map(|u| u.removed.len() + u.seg_removed.len())
                .sum::<usize>()
        )
    };
    format!("{} · {}\n{} · снято в сцене: {}\nСгорело: {} · оплавлено: {} · обрушилось: {} · обломков: {}\nВремя опыта: {:.2} с · горячих клеток: {} · очередь правок: {}{}",
        if sim.ready() { "ГОТОВ" } else { "ВОЗДЕЙСТВИЕ" }, sample,remaining,sim.removed,
        sim.heat.burned_total, sim.heat.melted_total, sim.collapsed_cells,
        sim.matter.debris.fragments.len(), lab.elapsed as f64/sim_core::TICKS_PER_SECOND as f64,
        sim.heat.hot(), sim.world.queued(),
        "\nОстаток — в исходном объёме. Перемещённые части не считаются уничтоженными.")
}
fn comparison(lab: &Laboratory) -> String {
    let Some(a) = &lab.baseline else {
        return "Сравнение: проведите опыт и нажмите «Запомнить A»".into();
    };
    let mut protocol = a.recipe;
    protocol.material = lab.recipe.material;
    protocol.strength = lab.recipe.strength;
    if protocol != lab.recipe
        || a.ticks != lab.elapsed
        || a.revision != lab.constructor.revision
        || a.shots != lab.shots
        || a.start != lab.start
    {
        return format!(
            "A: {} · {} тиков. Для сравнения нужны те же форма, воздействие, мощность и время.",
            lab.definition.materials[a.recipe.material].name, a.ticks
        );
    }
    format!("A: {} · прочность {}‰ · остаток {} · снято {} · сгорело {} · оплавлено {}\nB − A: на стенде {:+} клеток",
        lab.definition.materials[a.recipe.material].name, lab.definition.strength_milli[a.recipe.strength],
        a.remaining, a.removed, a.burned, a.melted, lab.remaining as i64-a.remaining as i64)
}
fn save_report(lab: &Laboratory, sim: &BlastRange) -> Result<(), String> {
    std::fs::create_dir_all("out/material-lab").map_err(|e| e.to_string())?;
    let recipe = lab.recipe;
    let report = format!("RTS material laboratory v1\nmaterial_id={}\nmaterial={}\nform={}\nthickness={}\nstrength_milli={}\npower_milli={}\neffect={}\nshots={}\ntick={}\nstate_hash={:016x}\n{}\n{}\n",
        lab.definition.materials[recipe.material].id, lab.definition.materials[recipe.material].name,
        lab.definition.forms[recipe.form].name, lab.definition.thickness[recipe.thickness],
        lab.definition.strength_milli[recipe.strength], sources::active(lab).map_or(lab.definition.layout.power_milli[recipe.power],|s|s.power_milli),
        sources::active(lab).and_then(|s|s.effect).unwrap_or(recipe.effect), lab.shots, sim.tick, sim.state_hash(), readout(lab, sim), comparison(lab));
    let report = format!(
        "{}\nexperiment={}\n{}\n{}\n",
        report,
        lab.constructor
            .experiment
            .as_ref()
            .map_or("free", |p| p.name.as_str()),
        lab.metrics,
        sources::readout(lab)
    );
    std::fs::write("out/material-lab/latest.txt", report).map_err(|e| e.to_string())
}

fn save_scene(lab: &Laboratory, sim: &BlastRange) -> Result<(), String> {
    use modding::laboratory::catalog::SceneFile;
    let material = lab.definition.materials[lab.recipe.material].id;
    let strength = lab.constructor.strength.or_else(|| {
        sim.matter
            .economy
            .mats
            .get(material)
            .map(|m| (material, m.strength))
    });
    let visual = sources::active(lab).map_or_else(|| lab.constructor.weapon.map_or(
        match lab.recipe.effect {
            2 => 1,
            6 => 2,
            _ => 0,
        },
        |(_, v)| v,
    ),|s|s.visual);
    let mut base = lab.constructor.base_layout(&lab.definition, lab.recipe)?;
    if let Some(at) = lab.constructor.aim {
        base.target = at;
    }
    let file = SceneFile {
        version: 1,
        base,
        elements: lab.constructor.placed.clone(),
        weapon: lab.config(),
        strength,
        visual,
        experiment: lab.constructor.experiment.clone(),
        workspace: lab.constructor.workspace.clone(),
    };
    let text = file.to_ron()?;
    let path = std::path::Path::new(constructor::PROJECT);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let temp = path.with_extension("ron.tmp");
    std::fs::write(&temp, text).map_err(|e| e.to_string())?;
    std::fs::rename(temp, path).map_err(|e| e.to_string())
}
fn save_weapon(lab: &mut Laboratory) -> Result<String, String> {
    use modding::laboratory::catalog::WeaponAsset;
    std::fs::create_dir_all(constructor::WEAPONS).map_err(|e| e.to_string())?;
    let number = (1..=10000)
        .find(|n| {
            !std::path::Path::new(constructor::WEAPONS)
                .join(format!("weapon-{n:04}.ron"))
                .exists()
        })
        .ok_or("Библиотека заполнена")?;
    let visual = sources::active(lab).map_or_else(|| lab.constructor.weapon.map_or(
        match lab.recipe.effect {
            2 => 1,
            6 => 2,
            _ => 0,
        },
        |(_, v)| v,
    ),|s|s.visual);
    let w = WeaponAsset {
        version: 1,
        name: format!("Моё оружие {number:04}"),
        config: lab.config(),
        visual,
    };
    let text = w.to_ron()?;
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(std::path::Path::new(constructor::WEAPONS).join(format!("weapon-{number:04}.ron")))
        .map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    let message = format!("{} сохранено; выберите его кнопкой «Моё оружие»", w.name);
    lab.constructor.weapons.push(w);
    Ok(message)
}
fn load_scene(lab: &mut Laboratory) -> Result<(), String> {
    use modding::laboratory::catalog::{self, SceneFile};
    if lab.constructor.creating() {
        return Err("Сначала завершите создание элемента".into());
    }
    let text = constructor::read_bounded(std::path::Path::new(constructor::PROJECT), 2 << 20)?;
    let file = SceneFile::from_ron(&text)?;
    let layout = catalog::compose(&file.base, &file.elements)?;
    sim_core::blast::placement::Validator::default()
        .validate(&layout)
        .map_err(|e| e.to_string())?;
    lab.constructor.base = Some(file.base);
    lab.constructor.workspace=file.workspace;
    if let Some(step)=lab.constructor.workspace.editor_step {lab.definition.editor_step=step;}
    lab.constructor.experiment = file.experiment;
    lab.constructor.placed = file.elements;
    lab.constructor.stand = false;
    lab.constructor.selected = lab.constructor.placed.len().checked_sub(1);
    if lab.constructor.workspace.selected_material.is_none() {lab.constructor.sync_material(&lab.definition);}
    lab.constructor.aim = None;
    lab.constructor.weapon = Some((file.weapon, file.visual));
    lab.constructor.strength = file.strength;
    lab.constructor.mode = constructor::Mode::Select;
    lab.constructor.revision += 1;
    lab.recipe.power = lab
        .definition
        .layout
        .power_milli
        .iter()
        .position(|p| *p == 1000)
        .unwrap_or(0);
    lab.notice = "Сохранённая расстановка загружена".into();
    Ok(())
}

#[allow(clippy::type_complexity)]
pub fn refresh(
    mut lab: ResMut<Laboratory>,
    scene: Res<Scene>,
    mut roots: Query<
        (&mut Node, Option<&experiments::Menu>, Option<&ui::Help>),
        (With<Root>, Without<panel::PanelRoot>),
    >,
    mut chrome: Query<(Entity, &mut Node), (With<panel::PanelRoot>, Without<Root>)>,
    mut labels: Query<(&Label, &mut Text)>,
    mut buttons: Query<(&LabAction, &Interaction, &mut BackgroundColor)>,
) {
    let active = lab.active();
    for (mut n, menu, help) in &mut roots {
        let value = if active && !first_person::immersive(&lab) && (menu.is_none() || lab.browser) && (help.is_none() || lab.ui.help)
        {
            Display::Flex
        } else {
            Display::None
        };
        if n.display != value {
            n.display = value;
        }
    }
    if active {
        if lab.hidden.is_empty() {
            lab.hidden = chrome.iter_mut().map(|(e, n)| (e, n.display)).collect();
        }
        for (_, mut n) in &mut chrome {
            if n.display != Display::None {
                n.display = Display::None;
            }
        }
    } else {
        for (e, display) in lab.hidden.drain(..) {
            if let Ok((_, mut n)) = chrome.get_mut(e) {
                n.display = display;
            }
        }
        return;
    }
    for (action, interaction, mut background) in &mut buttons {
        let value = if ui::enabled(&lab, *action) {
            theme::button_background(*interaction, ui::selected(&lab, *action))
        } else {
            Color::srgb(0.04, 0.055, 0.08)
        };
        if background.0 != value {
            background.0 = value;
        }
    }
    if active {tools::inspector::refresh(&mut lab,&scene);}
    let material_name=lab.constructor.material_name(&lab.definition,lab.recipe);
    for (Label(kind), mut text) in &mut labels {
        let value = match kind {
            0 => format!("Выбрано: {}",material_name),
            1 => lab.definition.forms[lab.recipe.form].name.clone(),
            2 => format!(
                "Толщина плиты: {} мм\nПрочность: {:.2}×",
                lab.definition.thickness[lab.recipe.thickness] * voxel::unit::WORLD_MM as i32,
                lab.definition.strength_milli[lab.recipe.strength] as f32 / 1000.0
            ),
            3 if sources::active(&lab).is_some() => sources::readout(&lab),
            3 => experiments::weapon_label(&lab).unwrap_or_else(|| {
                format!(
                    "{} · {:.1}×\nОкно наблюдения: {} с",
                    if lab.constructor.weapon.is_some() {
                        "Оружие сохранённой сцены"
                    } else {
                        [
                            "Фугас",
                            "Бронебой",
                            "Энергопушка",
                            "Лазер",
                            "Огнемёт",
                            "Удар по опоре",
                            "Анипушка",
                            "Пулемёт",
                            "Ядерный заряд",
                            "Осадный снаряд",
                        ][lab.recipe.effect as usize]
                    },
                    sources::active(&lab).map_or(lab.definition.layout.power_milli[lab.recipe.power],|s|s.power_milli) as f32 / 1000.0,
                    lab.constructor
                        .experiment
                        .as_ref()
                        .map_or(lab.definition.observation_ticks, |p| p.duration_ticks)
                        / sim_core::TICKS_PER_SECOND as u32
                )
            }),
            4 => format!("{}\n{}", readout(&lab, &scene.sim), lab.metrics),
            11 => lab.constructor.experiment.as_ref().map_or_else(
                || "Свободная сборка · общая физика полигона".into(),
                |p| format!("{}\n{}", p.name, p.description),
            ),
            5 => comparison(&lab),
            6 => format!(
                "{} · {}",
                if scene.paused {
                    "Пауза"
                } else {
                    "Симуляция идёт"
                },
                lab.notice
            ),
            23=>lab.constructor.material_context(&lab.definition,lab.recipe),
            21=>lab.tools.probe.clone(),
            24=>lab.constructor.voxel_text(&lab.definition,lab.recipe),
            25=>live_tools::text(&lab,scene.paused),
            26|27=>ui::text(&lab,*kind),
            22=>format!("Кисть: {}³ · дистанция: {} ед.\nПоверхность: {} · пересечения: {}",lab.tools.edge,lab.tools.distance as u32,if lab.constructor.workspace.surface_snap.unwrap_or(true){"вкл"}else{"выкл"},if lab.constructor.workspace.allow_overlap{"вкл"}else{"выкл"}),
            14 => format!(
                "{} · тик {}",
                if scene.paused {
                    "ПАУЗА"
                } else {
                    "ОПЫТ ИДЁТ"
                },
                scene.sim.tick
            ),
            12..=20 => ui::text(&lab, *kind),
            _ => constructor::text(&lab, *kind),
        };
        if text.0 != value {
            **text = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dune_experiment_roundtrips_through_the_constructor_and_shared_engine() {
        let mut lab=Laboratory::new(false);
        let index=lab.experiments.iter().position(|e|e.id=="dune-rockfall").unwrap();
        let expected=lab.experiments[index].layout.clone();
        experiments::select(&mut lab,index).unwrap();
        let (mut sim,_)=lab.prepare().unwrap();
        assert_eq!(sim.layout.as_ref().unwrap().blocks,expected.blocks);
        assert!(experiments::commands(lab.constructor.experiment.as_ref().unwrap()).is_empty());
        sim.set_physics_workers(1);sim.set_navigation_workers(1);
        for _ in 0..8 {sim.step();}
        assert!(sim.bodies.list.iter().any(|b|b.voxels.iter().all(|v|v.mat==6)),"suspended constructor rocks must fall without a shot");
    }
    #[test]
    fn stand_is_pristine_and_recipe_override_is_local() {
        let mut lab = Laboratory::new(false);
        let (sim, _) = lab.prepare().unwrap();
        let pristine = sim.snapshot();
        lab.recipe.strength = 2;
        let (hard, _) = lab.prepare().unwrap();
        assert_eq!(hard.matter.economy.mats.get(3).unwrap().strength, 1000);
        assert_eq!(sim.matter.economy.mats.get(3).unwrap().strength, 500);
        lab.recipe.strength = 1;
        let (again, _) = lab.prepare().unwrap();
        assert_eq!(again.snapshot(), pristine);
        assert!(sim.ready());
        assert_eq!(sim.fire_tick, u32::MAX);
    }
}
