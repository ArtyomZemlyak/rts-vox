//! Opt-in usability acceptance: real native UI focus/mouse path, wall-clock video, measured hit islands.
use super::*;
use bevy::ui::UiGlobalTransform;
#[path = "ui_acceptance/geometry.rs"]
mod geometry;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Off,
    Motion,
    Nav,
    Layout,
    NavLayout,
    Battle,
}
#[derive(Resource)]
pub(super) struct Acceptance {
    mode: Mode,
    pub ready: bool,
    dir: Option<String>,
    start: Option<f64>,
    clock: f64,
    event: usize,
    pressed: bool,
    recording: Option<Recording>,
    shots: usize,
    rows: Vec<String>,
    inputs: Vec<String>,
    pending: Option<(String, Option<[i32; 2]>, usize)>,
    profile_step: u32,
    errors: Vec<String>,
    profile_tick: Option<u32>,
    profile_rows: Vec<String>,
    profile_saved: bool,
    cursor: Option<Vec2>,
    saved_cursor: Option<(Option<Vec2>, bool)>,
    shot_rows: Vec<String>,
}
impl Default for Acceptance {
    fn default() -> Self {
        Self::new(None, None)
    }
}
impl Acceptance {
    pub fn new(mode: Option<&str>, dir: Option<&str>) -> Self {
        let mode = match mode {
            None => Mode::Off,
            Some("motion") => Mode::Motion,
            Some("nav") => Mode::Nav,
            Some("layout") => Mode::Layout,
            Some("nav-layout") => Mode::NavLayout,
            Some("battle") => Mode::Battle,
            _ => panic!("invalid --ui-acceptance"),
        };
        let recording = if matches!(mode, Mode::Motion | Mode::Nav) {
            Some(Recording::new(Some(format!("{}/video/frames", dir.unwrap())), 11.).unwrap())
        } else {
            None
        };
        Self {
            mode,
            ready: mode == Mode::Battle,
            dir: dir.map(str::to_string),
            start: None,
            clock: 0.,
            event: 0,
            pressed: false,
            recording,
            shots: 0,
            rows: Vec::new(),
            inputs: Vec::new(),
            pending: None,
            profile_step: u32::MAX,
            errors: Vec::new(),
            profile_tick: None,
            profile_rows: Vec::new(),
            profile_saved: false,
            cursor: None,
            saved_cursor: None,
            shot_rows: Vec::new(),
        }
    }
    pub fn enabled(&self) -> bool {
        self.mode != Mode::Off && self.mode != Mode::Battle
    }
    pub fn mark_ready(&mut self) {
        self.ready = true;
    }
    fn nav(&self) -> bool {
        matches!(self.mode, Mode::Nav | Mode::NavLayout)
    }
}
pub(super) struct AcceptancePlugin;
impl Plugin for AcceptancePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Acceptance>()
            .add_systems(
                PreUpdate,
                drive
                    .after(bevy::input::InputSystems)
                    .before(bevy::ui::UiSystems::Focus),
            )
            .add_systems(PostUpdate, capture.after(bevy::ui::UiSystems::PostLayout))
            .add_systems(
                PostUpdate,
                restore_cursor
                    .after(capture)
                    .after(bevy::ui::UiSystems::Stack),
            );
    }
}
fn virtual_cursor(a: &mut Acceptance, window: &mut Window, point: Vec2) {
    if a.saved_cursor.is_none() {
        a.saved_cursor = Some((window.cursor_position(), window.focused));
    }
    a.cursor = Some(point);
    window.focused = true;
    window.set_cursor_position(Some(point));
}
// The harness targets native focus without asking the OS to warp the desktop cursor.
fn restore_cursor(mut a: ResMut<Acceptance>, mut windows: Query<&mut Window>) {
    if let Some((point, focused)) = a.saved_cursor.take() {
        if let Ok(mut w) = windows.single_mut() {
            let w = w.bypass_change_detection();
            w.set_cursor_position(point);
            w.focused = focused;
        }
    }
}
#[derive(Clone)]
enum Aim {
    Recipe(usize),
    Front(frontend::Action),
    Lab(laboratory::LabAction),
    Part(spatial_ui::Part),
    Cancel,
    Queue,
    Producer(usize),
    World(usize),
    Fan,
    Gap,
    Deck,
    MenuWorld,
    LibraryScroll,
}
fn drive(
    mut a: ResMut<Acceptance>,
    time: Res<Time<Real>>,
    mut scene: ResMut<Scene>,
    mut edit: ResMut<Workshop>,
    mut projection: ResMut<spatial_ui::State>,
    scale: Res<UiScale>,
    mut windows: Query<&mut Window>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    nodes: Query<
        (
            Option<&frontend::Action>,
            Option<&laboratory::LabAction>,
            Option<&spatial_ui::Part>,
            Option<&construction::production_hud::Control>,
            &Node,
            &ComputedNode,
            &UiGlobalTransform,
            Option<&ChildOf>,
            Option<&construction::Action>,
        ),
        With<Button>,
    >,
    ancestors: Query<(&Node, Option<&ChildOf>)>,
    parts: Query<(&spatial_ui::Part, &ComputedNode, &UiGlobalTransform)>,
    hit_roots:Query<(&Node,&ComputedNode,&UiGlobalTransform,Option<&ChildOf>),
        (Or<(With<panel::PanelRoot>,With<panel::HitIsland>)>,Without<panel::LayoutOnly>)>,
    cameras: Query<(&Camera, &Transform), With<workbench::EditorCamera>>,
    menu: Res<frontend::Menu>,
    (mut wheel, headers, window_ids): (
        MessageWriter<bevy::input::mouse::MouseWheel>,
        Query<(&ComputedNode, &UiGlobalTransform), With<laboratory::LibraryHeader>>,
        Query<Entity, With<Window>>,
    ),
) {
    if a.mode == Mode::Off {
        return;
    }
    if a.mode == Mode::Battle {
        let step = scene.sim.tick / 60;
        if step != a.profile_step || edit.controlled.is_none() {
            let ids: Vec<_> = scene
                .sim
                .units
                .iter()
                .enumerate()
                .filter_map(|(id, u)| (u.status.alive && scene.sim.allied(id)).then_some(id))
                .collect();
            if !ids.is_empty() {
                let id = ids[step as usize % ids.len()];
                edit.controlled = Some(id);
                edit.selected_units = vec![id];
            }
            a.profile_step = step;
        }
        return;
    }
    if !a.ready {
        projection.enabled = false;
        return;
    }
    let now = time.elapsed_secs_f64();
    if a.recording.as_ref().is_some_and(|r| !r.running()) {
        scene.paused = true;
        return;
    }
    let start = *a.start.get_or_insert(now);
    a.clock = now - start;
    projection.enabled = true;
    if a.nav() {
        scene.out = None;
    } else if !menu.shown() {
        scene.paused = a.clock < 7.3;
    }
    mouse.clear();
    keys.reset_all();
    if let Some(point) = a.cursor {
        if let Ok(mut w) = windows.single_mut() {
            virtual_cursor(&mut a, w.bypass_change_detection(), point);
        }
    }
    if a.pressed {
        mouse.release(MouseButton::Left);
        mouse.release(MouseButton::Right);
        a.pressed = false;
        return;
    }
    let nav = [
        (
            0.5,
            Aim::Front(frontend::Action::Start(frontend::Destination::Battle)),
        ),
        (1.3, Aim::Front(frontend::Action::Back)),
        (2.1, Aim::Front(frontend::Action::Settings)),
        (3., Aim::Front(frontend::Action::Back)),
        (
            3.7,
            Aim::Front(frontend::Action::Start(frontend::Destination::Laboratory)),
        ),
        (5.2, Aim::Lab(laboratory::LabAction::Tab(1))),
        (6., Aim::Lab(laboratory::LabAction::Category(1))),
        (6.2, Aim::LibraryScroll),
        (6.5, Aim::Lab(laboratory::LabAction::Category(8))),
        (6.8, Aim::Front(frontend::Action::Menu)),
        (7.6, Aim::Front(frontend::Action::Resume)),
        (8.4, Aim::Lab(laboratory::LabAction::Back)),
    ];
    let motion = [
        (0.8, Aim::Queue),
        (1.6, Aim::Fan),
        (2.0, Aim::Deck),
        (2.4, Aim::Gap),
        (2.75, Aim::Queue),
        (2.9, Aim::World(1)),
        (3.6, Aim::Queue),
        (3.9, Aim::Producer(0)),
        (4.3, Aim::Part(spatial_ui::Part::CatalogToggle)),
        (4.8, Aim::Part(spatial_ui::Part::CatalogToggle)),
        (5.2, Aim::Part(spatial_ui::Part::FoldToggle)),
        (5.7, Aim::Part(spatial_ui::Part::Restore)),
        (6.9, Aim::Cancel),
        (8.8, Aim::Front(frontend::Action::Menu)),
        (9.2, Aim::MenuWorld),
        (9.8, Aim::Front(frontend::Action::Resume)),
    ];
    let event = if a.nav() {
        nav.get(a.event)
    } else {
        motion.get(a.event)
    };
    let mut hover = None;
    if !a.nav() && (4.4..4.7).contains(&a.clock) {
        hover = Some(Aim::Recipe(0));
    }
    if !a.nav() && (6.1..6.65).contains(&a.clock) {
        hover = Some(Aim::Part(spatial_ui::Part::ToolsToggle));
    }
    let (at, aim, click) = if let Some((at, aim)) = event.filter(|(at, _)| a.clock >= *at) {
        (*at, aim.clone(), true)
    } else if let Some(aim) = hover {
        (a.clock, aim, false)
    } else {
        return;
    };
    let point=match &aim {
        Aim::LibraryScroll=>headers.iter().next().and_then(|(c,g)|geometry::rect(c,g)).map(|(p,s)|p+s*0.5),
        Aim::World(id) => {
            let unit = scene.sim.units.get(*id);
            unit.and_then(|u| {
                let (camera, transform) = cameras.single().ok()?;
                let p = u.pose.world([u.body.core.0, u.body.core.1, u.body.core.2]
                    .map(|v| fx::Fx::from_int(i32::from(v))));
                camera.world_to_viewport(&GlobalTransform::from(*transform), Vec3::new(
                    p.x.to_f64_lossy() as f32, p.y.to_f64_lossy() as f32, p.z.to_f64_lossy() as f32))
                    .ok().map(|v| v / scale.0)
            })
        },
        Aim::Fan=>{let source=parts.iter().find(|(p,..)|matches!(p,spatial_ui::Part::Emitter(0))).and_then(|(_,n,g)|geometry::rect(n,g)).map(|r|r.0+r.1*0.5);let deck=parts.iter().find(|(p,..)|matches!(p,spatial_ui::Part::Deck)).and_then(|(_,n,g)|geometry::rect(n,g));source.zip(deck).map(|(s,(p,size))|s.lerp(p+size*0.5,0.20))},
        Aim::Gap=>{
            let areas:Vec<_>=hit_roots.iter().filter(|(n,_,_,p)|geometry::visible(n,*p,&ancestors))
                .filter_map(|(_,c,g,_)|geometry::rect(c,g)).collect();
            let viewport=windows.single().unwrap().size()/scale.0;
            edit.controlled.and_then(|id|scene.sim.units.get(id)).and_then(|unit| {
                let (lo,hi)=unit.bounds();let x=(lo[0]+hi[0])/2;let z=(lo[2]+hi[2])/2;
                let candidates=[[hi[0]+80,z],[x,hi[2]+80],[hi[0]+80,hi[2]+80],
                    [lo[0]-80,z],[x,lo[2]-80],[lo[0]-80,lo[2]-80]];
                let (camera,transform)=cameras.single().ok()?;
                candidates.into_iter().filter(|p|p[0]>0&&p[1]>0&&p[0]<scene.sim.world.nx as i32&&p[1]<scene.sim.world.nz as i32)
                    .filter_map(|p| {
                        let y=scene.sim.world.column_ceiling(p[0] as u32,p[1] as u32);
                        camera.world_to_viewport(&GlobalTransform::from(*transform),Vec3::new(p[0] as f32,y as f32,p[1] as f32)).ok().map(|p|p/scale.0)
                    }).find(|p|p.x>16.&&p.y>104.&&p.x<viewport.x-16.&&p.y<viewport.y-16.
                        && geometry::point_clear(*p,&areas,projection.audit_obstacles()))
            })
        },
        Aim::MenuWorld=>Some(windows.single().unwrap().size()/scale.0*Vec2::new(0.72,0.40)),
        Aim::Deck=>parts.iter().find(|(p,..)|matches!(p,spatial_ui::Part::Deck)).and_then(|(_,n,g)|geometry::rect(n,g)).map(|(p,s)|p+Vec2::new(s.x*0.5,12.)),
        _=>nodes.iter().find_map(|(f,l,part,q,n,c,g,parent,ctor)| {
            let matches=match &aim{Aim::Recipe(slot)=>ctor==Some(&construction::Action::Choose(*slot)),Aim::Front(x)=>f==Some(x),Aim::Lab(x)=>l==Some(x),Aim::Part(x)=>part==Some(x),Aim::Cancel=>ctor==Some(&construction::Action::Cancel),Aim::Queue=>matches!(q,Some(construction::production_hud::Control::Toggle)),Aim::Producer(slot)=>matches!(q,Some(construction::production_hud::Control::Select(i)) if i==slot),_=>false};
            (matches&&geometry::visible(n,parent,&ancestors)).then(||geometry::rect(c,g).map(|(p,s)|p+s*0.5)).flatten()
        })
    };
    if let Some(point) = point {
        let Ok(mut window) = windows.single_mut() else {
            return;
        };
        virtual_cursor(&mut a, window.bypass_change_detection(), point * scale.0);
        if !click && matches!(aim,Aim::Recipe(_)) && !a.inputs.iter().any(|s|s.starts_with("HOVER\trecipe")) {
            let clock=a.clock;
            a.inputs.push(format!("HOVER\trecipe\t{clock:.5}\t{:.2}\t{:.2}",point.x*scale.0,point.y*scale.0));
        }
        if click {
            if matches!(aim, Aim::LibraryScroll) {
                wheel.write(bevy::input::mouse::MouseWheel {
                    unit: bevy::input::mouse::MouseScrollUnit::Line,
                    x: 0.,
                    y: -2.,
                    window: window_ids.single().unwrap(),
                    phase: bevy::input::touch::TouchPhase::Moved,
                });
                a.inputs.push("library-wheel".into());
                a.event += 1;
                return;
            }
            let right = matches!(aim, Aim::Fan | Aim::Gap | Aim::Deck | Aim::MenuWorld);
            mouse.press(if right {
                MouseButton::Right
            } else {
                MouseButton::Left
            });
            a.pressed = true;
            let name = if right {
                match aim {
                    Aim::Fan => "fan-rally",
                    Aim::Gap => "gap-rally",
                    Aim::MenuWorld => "menu-block",
                    _ => "deck-block",
                }
                .to_string()
            } else {
                match aim {
                    Aim::Lab(laboratory::LabAction::Tab(id)) => format!("lab-tab:{id}"),
                    Aim::Lab(laboratory::LabAction::Category(id)) => format!("lab-category:{id}"),
                    _ => format!("control-{}", a.event),
                }
            };
            let line = format!(
                "{}\t{:.5}\t{:.2}\t{:.2}\t{}\t{:?}",
                name,
                a.clock,
                point.x * scale.0,
                point.y * scale.0,
                scene.sim.tick,
                edit.controlled
            );
            a.inputs.push(line);
            a.pending = Some((
                name,
                scene.sim.factory_rally(edit.controlled.unwrap_or(usize::MAX)),
                edit.controlled.unwrap_or(usize::MAX),
            ));
            a.event += 1;
        }
    } else if a.clock > at + 1. {
        let event = a.event;
        a.errors.push(format!("missing native target {event}"));
        a.event += 1;
    }
}
fn capture(
    mut a: ResMut<Acceptance>,
    time: Res<Time<Real>>,
    scene: Res<Scene>,
    edit: Res<Workshop>,
    panel: Res<panel::Panel>,
    menu: Res<frontend::Menu>,
    scale: Res<UiScale>,
    windows: Query<&Window>,
    nodes: geometry::Nodes,
    ancestors: Query<(&Node, Option<&ChildOf>)>,
    spatial: Res<spatial_ui::State>,
    motion: Res<ui_motion::Registry>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    (profiler, materials, lab): (
        Res<profile::Profile>,
        Res<optical_ui::Audit>,
        Res<laboratory::Laboratory>,
    ),
) {
    if a.mode == Mode::Battle {
        if a.profile_tick != Some(scene.sim.tick) {
            a.profile_tick = Some(scene.sim.tick);
            // A cheap workload signature, not a whole-world save hash or a SIM mutation.
            let mut hash = 0xcbf29ce484222325_u64;
            let mut alive = [0, 0];
            for (id, u) in scene.sim.units.iter().enumerate() {
                for v in u.motion.pos {
                    hash = (hash ^ v.raw() as u64).wrapping_mul(0x100000001b3);
                }
                hash = (hash ^ u.revision).wrapping_mul(0x100000001b3);
                if u.status.alive {
                    let allied = scene.sim.allied(id);
                    alive[usize::from(!allied)] += 1;
                }
            }
            let row = format!(
                "{}\t{}\t{}\t{}\t{}\t{hash:016x}\t{:?}\t{}\t{}\t{}\t{}\t{}",
                scene.sim.tick,
                scene.sim.units.len(),
                alive[0],
                alive[1],
                scene.sim.removed,
                edit.controlled,
                motion.emitted,
                motion.completed,
                materials.drawn,
                materials.allocated,
                scale.0
            );
            a.profile_rows.push(row);
        }
        if profiler.finished() && !a.profile_saved {
            if let Some(path) = &profiler.output {
                std::fs::write(std::path::Path::new(path).with_extension("ui.tsv"),format!("tick\troster\talive_allied\talive_enemy\tremoved\tworkload_signature\tselected\tmotion_events\tcompleted\tmaterials_drawn\tmaterials_allocated\tui_scale\n{}\n",a.profile_rows.join("\n"))).unwrap();
                a.profile_saved = true;
            }
        }
        return;
    }
    if !a.enabled() || !a.ready {
        return;
    }
    let now = time.elapsed_secs_f64();
    if let Some((name, before, actor)) = a.pending.take() {
        let after = scene.sim.factory_rally(actor);
        let valid = match name.as_str() {
            "fan-rally" | "gap-rally" => after != before && !panel.blocked,
            "deck-block" => after == before && panel.blocked,
            "menu-block" => after == before && menu.shown(),
            _ => {
                if let Some(id) = name.strip_prefix("lab-tab:") {
                    lab.audit_ui().0 == id.parse::<u8>().unwrap()
                } else if let Some(id) = name.strip_prefix("lab-category:") {
                    lab.audit_ui().1 == id.parse::<u8>().unwrap()
                } else {
                    true
                }
            }
        };
        let row=format!("RESULT\t{name}\tblocked={}\trally={before:?}->{after:?}\tselected={actor}->{:?}\tvalid={valid}",panel.blocked,edit.controlled);
        a.inputs.push(row);
        if !valid {
            a.errors.push(format!("input {name} failed"));
        }
    }
    if a.start.is_some() {
        let record = geometry::sample(
            a.clock, &scene, &edit, &panel, &menu, &scale, &windows, &nodes, &ancestors, &spatial,
            &motion,
        );
        if let Some(error) = record.1 {
            a.errors.push(error);
        }
        a.rows.push(record.0);
        let times = if a.nav() {
            vec![0.2, 0.85, 1.65, 2.55, 3.4, 4.6, 5.6, 6.4, 7.2, 8., 8.9, 9.5]
        } else {
            vec![
                0.5, 1.2, 1.8, 2.2, 2.6, 3.3, 4.1, 4.6, 5.4, 6., 6.45, 8.8, 10.4,
            ]
        };
        if times.get(a.shots).is_some_and(|t| a.clock >= *t) {
            let dir = a.dir.as_ref().unwrap();
            if a.recording.is_none() {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(format!("{dir}/{:02}-audit.png", a.shots)));
            }
            let row = format!("{:02}-audit\t{:.6}", a.shots, a.clock);
            a.shot_rows.push(row);
            a.shots += 1;
        }
    }
    let dir = a.dir.clone().unwrap();
    let done = if let Some(recording) = a.recording.as_mut() {
        recording.capture(
            &mut commands,
            now,
            u64::from(scene.sim.tick),
            scene.sim.tick as f64 / 20.,
        )
    } else if a.clock > 11.5 {
        Some(AppExit::Success)
    } else {
        None
    };
    if let Some(done) = done {
        std::fs::write(
            format!("{dir}/geometry.tsv"),
            format!("{}\n{}\n", geometry::HEADER, a.rows.join("\n")),
        )
        .unwrap();
        std::fs::write(format!("{dir}/input.tsv"), a.inputs.join("\n")).unwrap();
        std::fs::write(
            format!("{dir}/shots.tsv"),
            format!("stage\tclock\n{}\n", a.shot_rows.join("\n")),
        )
        .unwrap();
        std::fs::write(format!("{dir}/errors.txt"), a.errors.join("\n")).unwrap();
        println!(
            "UI ACCEPTANCE events={} errors={} produced={} complete={} tick={}",
            a.event,
            a.errors.len(),
            scene.sim.spawned().len(),
            motion.completed,
            scene.sim.tick
        );
        exit.write(done);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readable_battle_scale_preserves_compact_click_targets_and_doubles_4k_text() {
        assert_eq!(panel::hud::scale(Vec2::new(960., 540.)), 1.);
        assert_eq!(panel::hud::scale(Vec2::new(1920., 1080.)), 1.);
        assert_eq!(panel::hud::scale(Vec2::new(3840., 2160.)), 2.);
    }
    #[test]
    fn native_acceptance_queries_have_no_aliases() {
        use bevy::ecs::system::{IntoSystem, System};
        let mut w = World::new();
        IntoSystem::into_system(drive).initialize(&mut w);
        IntoSystem::into_system(capture).initialize(&mut w);
        IntoSystem::into_system(restore_cursor).initialize(&mut w);
    }
    #[test]
    fn virtual_pointer_restores_the_os_window_state_after_native_focus() {
        let mut app = App::new();
        app.insert_resource(Acceptance::default())
            .add_systems(Update, restore_cursor);
        let mut window = Window::default();
        window.focused = false;
        window.set_cursor_position(None);
        virtual_cursor(
            &mut app.world_mut().resource_mut::<Acceptance>(),
            &mut window,
            Vec2::new(100., 80.),
        );
        assert_eq!(window.cursor_position(), Some(Vec2::new(100., 80.)));
        let e = app.world_mut().spawn(window).id();
        app.update();
        let window = app.world().get::<Window>(e).unwrap();
        assert_eq!(window.cursor_position(), None);
        assert!(!window.focused);
    }
}
