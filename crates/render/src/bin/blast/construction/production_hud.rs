//! Global production publication. Selection never owns the queue or its screen anchor.
use super::*;

#[derive(Clone, Debug)]
pub(super) struct Line {
    pub actor: usize,
    pub label: String,
    pub queue: Vec<RangeUnitKind>,
    pub progress: u64,
    work: Option<Work>,
}
#[derive(Clone, Debug)]
enum Work {
    Production { ticks: u32, build: u32, paid: u64, cost: u64 },
    Construction { delivered: u64, required: u64 },
}
impl Work {
    fn detail(&self) -> String {
        match *self {
            Self::Production {ticks,build,paid,cost} => format!("Сборка {}% · оплата {}%",(u64::from(ticks)*100/u64::from(build.max(1))).min(100),(paid.saturating_mul(100)/cost.max(1)).min(100)),
            Self::Construction {delivered,required} => format!("Материя: {delivered} / {required}"),
        }
    }
    fn ready(&self) -> bool {matches!(*self,Self::Production{ticks,build,paid,cost} if ticks>=build && paid>=cost)}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Future {kind:RangeUnitKind,count:usize}
fn upcoming(queue:&[RangeUnitKind])->(Vec<Future>,usize) {
    if queue.len()<2 {return (Vec::new(),0);}
    let mut groups:Vec<Future>=Vec::with_capacity(2);let mut remaining=0;
    for &kind in queue.iter().skip(1) {
        if remaining>0 {remaining+=1;continue;}
        if let Some(last)=groups.last_mut().filter(|g|g.kind==kind) {last.count+=1;}
        else if groups.len()<2 {groups.push(Future{kind,count:1});}
        else {remaining=1;}
    }
    (groups,remaining)
}
#[derive(Component)]
pub struct FutureSlot(usize);

pub(super) fn collect(sim: &BlastRange) -> Vec<Line> {
    // Lightweight active-site publication has no assigned-drone census or voxel reads.
    let sites: BTreeMap<_, _> = sim
        .construction_progress_views()
        .filter(|s| s.allied)
        .map(|s| (s.id, s))
        .collect();
    let building_units: std::collections::BTreeSet<_> =
        sites.values().filter_map(|s| s.unit).collect();
    let mut lines: Vec<_> = sim
        .production_lines()
        .into_iter()
        .filter_map(|actor| {
            let u = sim.units.get(actor)?;
            if !u.status.alive || !sim.allied(actor) || building_units.contains(&actor) {
                return None;
            }
            let queue = sim.production_queue(actor).unwrap_or_default();
            let current=sim.production_progress(actor);
            let progress =
                current
                    .map_or(0, |(ticks, build, paid, cost)| {
                        (u64::from(ticks) * 100 / u64::from(build.max(1)))
                            .min(paid.saturating_mul(100) / cost.max(1))
                            .min(100)
                    });
            let work=current.map(|(ticks,build,paid,cost)|Work::Production{ticks,build,paid,cost});
            Some(Line {
                actor,
                label: format!("{} #{}", name(u.kind), actor + 1),
                queue,
                progress,
                work,
            })
        })
        .collect();
    let mut represented = std::collections::BTreeSet::new();
    if !sites.is_empty() {
        for (actor, u) in sim.units.iter().enumerate().filter(|(i, u)| {
            u.status.alive
                && sim.allied(*i)
                && matches!(u.kind, RangeUnitKind::Commander | RangeUnitKind::SkobaDrone)
        }) {
            let plans: Vec<_> = sim
                .order_queue(actor)
                .into_iter()
                .filter_map(|p| match p.order {
                    sim_core::blast::battle::Order::Build(id) => sites.get(&id),
                    _ => None,
                })
                .filter(|s| represented.insert(s.id))
                .collect();
            if let Some(first) = plans.first() {
                lines.push(Line {
                    actor,
                    label: format!("План · {} #{}", name(u.kind), actor + 1),
                    queue: plans.iter().map(|s| s.kind).collect(),
                    progress: (first.delivered.saturating_mul(100) / first.required.max(1))
                        .min(100),
                    work:Some(Work::Construction{delivered:first.delivered,required:first.required}),
                });
            }
        }
        for site in sites.values().filter(|s| !represented.contains(&s.id)) {
            lines.push(Line {
                actor: site.unit.unwrap_or(usize::MAX),
                label: format!("Стройка #{}", site.id + 1),
                queue: vec![site.kind],
                progress: (site.delivered.saturating_mul(100) / site.required.max(1)).min(100),
                work:Some(Work::Construction{delivered:site.delivered,required:site.required}),
            });
        }
    }
    lines.sort_by(|a, b| a.actor.cmp(&b.actor).then(a.label.cmp(&b.label)));
    lines
}

#[derive(Resource, Default)]
pub struct State {
    pub expanded: bool,
    revision:u64,
    page: usize,
    shown_generation: Option<u64>,
    shown_actors: Vec<usize>,
    lines: Vec<Line>,
}
#[derive(Component)]
pub struct Root;
impl State {
    pub fn motion_revision(&self)->u64{self.revision}
    pub fn motion_snapshot(&self)->impl Iterator<Item=(usize,Option<usize>,usize,u64)>+'_ {self.lines.iter().filter(|l|l.actor!=usize::MAX).map(|l|(l.actor,l.queue.first().map(|k|k.index()),l.queue.len(),l.progress))}
    pub fn initial(expanded: bool) -> Self {
        Self {
            expanded,
            ..default()
        }
    }
}
#[derive(Component)]
pub struct Detail(usize);
#[derive(Component, Clone, Copy)]
pub enum Caption {
    Header,
    Disclosure,
    Producer,
    Summary,
    Next,
    Upcoming(usize),
    Detail(usize),
    Page,
}
#[derive(Component)]
pub struct Meter(usize);
#[derive(Component)]
pub enum Control {
    Toggle,
    Page(i32),
    Select(usize),
}

fn label(p: &mut ChildSpawnerCommands, f: &TextFont, caption: Caption, color: Color) {
    let base = if matches!(caption, Caption::Header) {
        f.clone()
    } else {
        theme::data_font(f)
    };
    let id = p
        .spawn((
            caption,
            Text::new(""),
            TextFont {
                font_size: (if matches!(caption, Caption::Summary) {
                    20.0
                } else if matches!(caption, Caption::Upcoming(_)) {
                    13.0
                } else if matches!(caption, Caption::Header | Caption::Disclosure | Caption::Producer | Caption::Next | Caption::Detail(_)) {
                    14.0
                } else {
                    16.0
                })
                .into(),
                ..base
            },
            TextColor(color),
        ))
        .id();
    if matches!(caption, Caption::Summary | Caption::Next | Caption::Producer) {
        p.commands()
            .entity(id)
            .insert((Detail(usize::MAX - 1), Node::default()));
    }
}
fn control(p: &mut ChildSpawnerCommands, f: &TextFont, value: &str, action: Control) {
    p.spawn((
        Button,
        action,
        Node {
            padding: UiRect::axes(px(8), px(4)),
            ..default()
        },
        BackgroundColor(theme::BUTTON),
    ))
    .with_children(|p| {
        p.spawn((Text::new(value), f.clone(), TextColor(theme::AMBER)));
    });
}
pub fn build(commands: &mut Commands, camera: Entity, font: &TextFont) {
    commands.init_resource::<State>();
    commands
        .spawn((
            Root,
            panel::PanelRoot,
            theme::Style::Pixel,
            bevy::ui::RelativeCursorPosition::default(),
            UiTargetCamera(camera),
            GlobalZIndex(24),
            Node {
                position_type: PositionType::Absolute,
                right: px(16),
                top: px(12),
                width: px(360),
                padding: UiRect::axes(px(10),px(7)),
                row_gap: px(3),
                flex_direction: FlexDirection::Column,
                border: UiRect::top(px(1)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(theme::PANEL),
            BorderColor::all(theme::FRAME),
        ))
        .with_children(|p| {
            theme::corners(p);
            p.spawn((
                Button,
                Control::Toggle,
                Node {
                    min_height: px(22),
                    justify_content:JustifyContent::SpaceBetween,
                    align_items:AlignItems::Center,
                    ..default()
                },
            ))
            .with_children(|p| {
                label(p, font, Caption::Header, theme::AMBER);
                label(p, font, Caption::Disclosure, theme::MUTED);
            });
            label(p, font, Caption::Producer, theme::MUTED);
            label(p, font, Caption::Summary, theme::TEXT);
            p.spawn((Detail(usize::MAX-1),Node{height:px(32),column_gap:px(4),align_items:AlignItems::Center,..default()},Pickable::IGNORE))
                .with_children(|p| {
                    label(p,font,Caption::Next,theme::MUTED);
                    for slot in 0..2 {
                        p.spawn((FutureSlot(slot),Node{flex_basis:px(0),flex_grow:1.,min_width:px(0),height:px(32),
                            padding:UiRect::axes(px(4),px(2)),border:UiRect::left(px(1)),align_items:AlignItems::Center,..default()},
                            BackgroundColor(Color::NONE),BorderColor::all(theme::AMBER.with_alpha(0.45)),Pickable::IGNORE))
                            .with_children(|p|{label(p,font,Caption::Upcoming(slot),theme::TEXT);});
                    }
                });
            p.spawn((
                Detail(usize::MAX - 2),
                Node {
                    height: px(4),
                    width: percent(100),
                    ..default()
                },
                BackgroundColor(theme::FRAME),
            ))
            .with_children(|p| {
                p.spawn((
                    Meter(0),
                    ui_motion::Progress::default(),
                    Node {
                        height: percent(100),
                        width: percent(0),
                        ..default()
                    },
                    BackgroundColor(theme::AMBER),
                    Pickable::IGNORE,
                ));
            });
            for i in 0..5 {
                p.spawn((
                    Detail(i),
                    Node {
                        display: Display::None,
                        row_gap: px(4),
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::all(px(6)),
                        border: UiRect::left(px(2)),
                        ..default()
                    },
                    BackgroundColor(theme::BUTTON),
                    BorderColor::all(theme::FRAME),
                ))
                .with_children(|p| {
                    p.spawn((Button, Control::Select(i), Node::default()))
                        .with_children(|p| {
                            label(p, font, Caption::Detail(i), theme::TEXT);
                        });
                    p.spawn((
                        Node {
                            height: px(4),
                            width: percent(100),
                            ..default()
                        },
                        BackgroundColor(theme::FRAME),
                    ))
                    .with_children(|p| {
                        p.spawn((
                            Meter(i),
                            ui_motion::Progress::default(),
                            Node {
                                height: percent(100),
                                width: percent(0),
                                ..default()
                            },
                            BackgroundColor(theme::AMBER),
                            Pickable::IGNORE,
                        ));
                    });
                });
            }
            p.spawn((
                Detail(usize::MAX),
                Node {
                    column_gap: px(8),
                    align_items: AlignItems::Center,
                    ..default()
                },
            ))
            .with_children(|p| {
                control(p, font, "‹", Control::Page(-1));
                label(p, font, Caption::Page, theme::MUTED);
                control(p, font, "›", Control::Page(1));
            });
        });
}

fn next(queue: &[RangeUnitKind]) -> String {
    if queue.len() < 2 {
        return "далее —".into();
    }
    let names: Vec<_> = queue.iter().skip(1).take(3).map(|&k| name(k)).collect();
    format!(
        "далее: {}{}",
        names.join(" → "),
        if queue.len() > 4 {
            format!(" · +{}", queue.len() - 4)
        } else {
            String::new()
        }
    )
}

#[allow(clippy::type_complexity)]
pub fn refresh(
    mut state: ResMut<State>,
    scene: Res<Scene>,
    mut edit: ResMut<Workshop>,
    menu: Res<frontend::Menu>,
    lab: Res<laboratory::Laboratory>,
    _pilot: Res<pilot::Pilot>,
    windows: Query<&Window>,
    time: Option<Res<Time>>,
    controls: Query<(&Control, &Interaction), Changed<Interaction>>,
    mut roots: Query<&mut Node, (With<Root>, Without<Detail>, Without<Meter>)>,
    mut details: Query<(&Detail, &mut Node), (Without<Root>, Without<Meter>)>,
    mut meters: Query<(&Meter, &mut Node,Option<&mut ui_motion::Progress>), (Without<Root>, Without<Detail>)>,
    mut captions: Query<(&Caption, &mut Text)>,
    mut future_nodes:Query<(&FutureSlot,&mut Node),(Without<Root>,Without<Detail>,Without<Meter>)>,
    mut elapsed: Local<f32>,
    mut last: Local<Option<(u64, u32)>>,
) {
    if menu.shown() || lab.active() {
        for mut node in &mut roots {
            node.display = Display::None;
        }
        *last = None;
        state.shown_generation = None;
        state.shown_actors.clear();
        return;
    }
    let key = (scene.generation, scene.sim.tick);
    *elapsed += time.as_ref().map_or(0.05, |t| t.delta_secs());
    if *last != Some(key) || *elapsed >= 0.05 {
        state.lines = collect(&scene.sim);
        state.revision=state.revision.wrapping_add(1);
        *last = Some(key);
        *elapsed = 0.;
    }
    let size = windows
        .single()
        .map_or(Vec2::new(1600., 900.), |w| Vec2::new(w.width(), w.height()));
    let page_size = if size.y < 760. { 2 } else { 5 };
    for (control, interaction) in &controls {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match *control {
            Control::Toggle => state.expanded = !state.expanded,
            Control::Page(delta) => state.page = state.page.saturating_add_signed(delta as isize),
            Control::Select(slot) => {
                // Bind to the row that was displayed, before this frame's queue publication.
                if state.shown_generation == Some(scene.generation) {
                    if let Some(actor) = state.shown_actors.get(slot).copied().filter(|&actor| {
                        scene
                            .sim
                            .units
                            .get(actor)
                            .is_some_and(|u| u.status.alive && scene.sim.allied(actor))
                    }) {
                        edit.select_battle_unit(&scene.sim, actor, false);
                    }
                }
            }
        }
    }
    let active: Vec<_> = state.lines.iter().filter(|l| !l.queue.is_empty()).collect();
    let (future,remaining)=active.first().map_or_else(||(Vec::new(),0),|l|upcoming(&l.queue));
    let pages = active.len().div_ceil(page_size).max(1);
    let page = state.page.min(pages - 1);
    let expanded = state.expanded;
    let visible = !state.lines.is_empty() && !menu.shown() && !lab.active();
    for mut node in &mut roots {
        let display = if visible {
            Display::Flex
        } else {
            Display::None
        };
        let width = px((size.x * 0.28).clamp(256., 360.));
        if node.display != display { node.display = display; }
        if node.width != width { node.width = width; }
    }
    for (Detail(slot), mut node) in &mut details {
        let display = if if *slot == usize::MAX - 2 {
            !expanded && active.len() == 1
        } else if *slot == usize::MAX - 1 {
            !expanded
        } else {
            expanded
                && (if *slot == usize::MAX {
                    pages > 1
                } else {
                    *slot < page_size && page * page_size + slot < active.len()
                })
        } {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    for (Meter(slot), mut node, motion) in &mut meters {
        let goal =
            active
                .get(page * page_size + slot)
                .map_or(0., |l| l.progress as f32);
        if let Some(mut p) = motion {
            if p.goal != goal {
                p.goal = goal;
            }
        } else {
            let width = percent(goal);
            if node.width != width {
                node.width = width;
            }
        }
    }
    for (FutureSlot(slot),mut node) in &mut future_nodes {
        let display=if !expanded && *slot<future.len(){Display::Flex}else{Display::None};
        if node.display!=display {node.display=display;}
    }
    for (caption, mut text) in &mut captions {
        let value = match *caption {
            Caption::Header => format!("ПРОИЗВОДСТВО · {}/{}",active.len(),state.lines.len()),
            Caption::Disclosure => if expanded {"свернуть −"} else {"детали +"}.into(),
            Caption::Producer => active.first().map_or_else(
                || format!("Готовы к заказу: {}",state.lines.len()),
                |l| if active.len()==1 {l.label.clone()}else{format!("{} · ещё: {}",l.label,active.len()-1)}),
            Caption::Summary => {
                if active.is_empty() {
                    "Нет заказов".into()
                } else {
                    let l=active[0];if l.work.as_ref().is_some_and(Work::ready) {format!("{} · готово",name(l.queue[0]))}else{format!("{} · {}%",name(l.queue[0]),l.progress)}
                }
            }
            Caption::Next => if future.is_empty(){"далее —".into()}else{String::new()},
            Caption::Upcoming(slot) => future.get(slot).map_or(String::new(),|g|format!("› {}{}{}",name(g.kind),if g.count>1{format!(" ×{}",g.count)}else{String::new()},if slot==1 && remaining>0{format!(" · +{remaining}")}else{String::new()})),
            Caption::Detail(slot) => {
                active
                    .get(page * page_size + slot)
                    .map_or(String::new(), |l| {
                        format!(
                            "{} · {} · {}%\n{}\n{}",
                            l.label,
                            name(l.queue[0]),
                            l.progress,
                            l.work.as_ref().map_or(String::new(),Work::detail),
                            next(&l.queue)
                        )
                    })
            }
            Caption::Page => format!("{} / {}", page + 1, pages),
        };
        if text.0 != value {
            text.0 = value;
        }
    }
    let shown_actors = if expanded && visible {
        active
            .iter()
            .skip(page * page_size)
            .take(page_size)
            .map(|l| l.actor)
            .collect()
    } else {
        Vec::new()
    };
    drop(active);
    state.shown_actors = shown_actors;
    state.shown_generation = visible.then_some(scene.generation);
    state.page = page;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn advance(app: &mut App) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(60));
        app.update();
    }
    #[test]
    fn upcoming_groups_preserve_order_and_real_completion_advances_the_published_queue() {
        use RangeUnitKind::*;
        let (groups,rest)=upcoming(&[Tank,Tank,Tank,EidolonTank,Artillery,Tank]);
        assert_eq!(groups,vec![Future{kind:Tank,count:2},Future{kind:EidolonTank,count:1}]);
        assert_eq!(rest,2,"the later Tank must not merge across a different order");
        let mut app=super::super::tests::app();app.insert_resource(frontend::Menu::new(false)).init_resource::<Time>();
        let camera=app.world_mut().spawn(Camera2d).id();build(&mut app.world_mut().commands(),camera,&TextFont::default());app.world_mut().flush();app.add_systems(PostUpdate,refresh);
        {
            let sim=&mut app.world_mut().resource_mut::<Scene>().sim;
            for kind in [Tank,Tank,EidolonTank,Artillery,Tank] {assert!(sim.execute_command(Command::Produce{actor:1,kind}).accepted());}
        }
        advance(&mut app);
        let state=app.world().resource::<State>();let line=state.lines.iter().find(|l|l.actor==1).unwrap();
        assert_eq!(line.queue,[Tank,Tank,EidolonTank,Artillery,Tank]);
        assert!(line.work.as_ref().unwrap().detail().contains("оплата"));
        let initial_count=app.world().resource::<Scene>().sim.units.len();
        let required=app.world().resource::<Scene>().sim.production_progress(1).unwrap().1;
        for _ in 0..required+400 {
            app.world_mut().resource_mut::<Scene>().sim.step();
            if app.world().resource::<Scene>().sim.production_exit(1).is_some(){break;}
        }
        let exit=app.world().resource::<Scene>().sim.production_exit(1).expect("real powered,paid production must finish");
        assert!(exit.0>=initial_count);assert_eq!(app.world().resource::<Scene>().sim.units[exit.0].kind,Tank);
        let hash=app.world().resource::<Scene>().sim.state_hash();
        advance(&mut app);
        let line=app.world().resource::<State>().lines.iter().find(|l|l.actor==1).unwrap();
        assert_eq!(line.queue,[Tank,EidolonTank,Artillery,Tank]);
        assert_eq!(upcoming(&line.queue).0,vec![Future{kind:EidolonTank,count:1},Future{kind:Artillery,count:1}]);
        assert_eq!(app.world().resource::<Scene>().sim.state_hash(),hash,"publishing the advanced queue must remain read-only");
        app.world_mut().resource_mut::<State>().expanded=true;advance(&mut app);
        let mut q=app.world_mut().query::<(&Caption,&Text)>();
        assert!(q.iter(app.world()).any(|(c,t)|matches!(c,Caption::Detail(_))&&t.0.contains("Сборка")&&t.0.contains("оплата")));
    }
    #[test]
    fn global_queues_survive_selection_and_disclose_real_lines() {
        let mut app = super::super::tests::app();
        app.insert_resource(frontend::Menu::new(false))
            .init_resource::<Time>();
        app.world_mut().spawn(Window {
            resolution: (960, 540).into(),
            ..default()
        });
        let camera = app.world_mut().spawn(Camera2d).id();
        build(
            &mut app.world_mut().commands(),
            camera,
            &TextFont::default(),
        );
        app.world_mut().flush();
        app.add_systems(PostUpdate, refresh);
        {
            let mut scene = app.world_mut().resource_mut::<Scene>();
            for (actor, kind) in [
                (0, RangeUnitKind::SkobaDrone),
                (1, RangeUnitKind::Tank),
                (1, RangeUnitKind::Tank),
            ] {
                assert!(scene
                    .sim
                    .execute_command(Command::Produce { actor, kind })
                    .accepted());
            }
        }
        advance(&mut app);
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<Root>>()
            .single(app.world())
            .unwrap();
        let anchor = app.world().get::<Node>(root).unwrap().clone();
        for selected in [None, Some(1), Some(3), Some(0), None] {
            app.world_mut().resource_mut::<Workshop>().controlled = selected;
            advance(&mut app);
            let state = app.world().resource::<State>();
            assert_eq!(
                state.lines.iter().filter(|l| !l.queue.is_empty()).count(),
                2
            );
            let node = app.world().get::<Node>(root).unwrap();
            assert_eq!(node.display, Display::Flex);
            assert_eq!((node.top, node.right), (anchor.top, anchor.right));
        }
        let toggle = app
            .world_mut()
            .query::<(Entity, &Control)>()
            .iter(app.world())
            .find(|(_, c)| matches!(c, Control::Toggle))
            .unwrap()
            .0;
        *app.world_mut().get_mut::<Interaction>(toggle).unwrap() = Interaction::Pressed;
        advance(&mut app);
        assert!(app.world().resource::<State>().expanded);
        let lines: Vec<_> = app
            .world_mut()
            .query::<(&Detail, &Node)>()
            .iter(app.world())
            .filter(|(d, n)| d.0 < 5 && n.display == Display::Flex)
            .collect();
        assert_eq!(lines.len(), 2);
        assert!(app
            .world_mut()
            .query::<&Text>()
            .iter(app.world())
            .any(|t| t.0.contains("далее: Танк")));
        let first = app
            .world_mut()
            .query::<(Entity, &Control)>()
            .iter(app.world())
            .find(|(_, c)| matches!(c, Control::Select(0)))
            .unwrap()
            .0;
        *app.world_mut().get_mut::<Interaction>(first).unwrap() = Interaction::Pressed;
        assert!(app
            .world_mut()
            .resource_mut::<Scene>()
            .sim
            .execute_command(Command::Cancel { actor: 0 })
            .accepted());
        advance(&mut app);
        assert_eq!(
            app.world().resource::<Workshop>().controlled,
            Some(0),
            "a click belongs to the shown producer even when its queue completes in this frame"
        );
        *app.world_mut().get_mut::<Interaction>(first).unwrap() = Interaction::None;
        advance(&mut app);
        app.world_mut().resource_mut::<Workshop>().controlled = None;
        app.world_mut().resource_mut::<Scene>().generation += 1;
        *app.world_mut().get_mut::<Interaction>(first).unwrap() = Interaction::Pressed;
        advance(&mut app);
        assert_eq!(
            app.world().resource::<Workshop>().controlled,
            None,
            "a stale producer binding cannot select in a replacement scene"
        );
        assert_eq!(
            app.world()
                .resource::<State>()
                .lines
                .iter()
                .filter(|l| !l.queue.is_empty())
                .count(),
            1
        );
        assert!(app
            .world()
            .resource::<Scene>()
            .sim
            .matter
            .economy
            .ledger
            .balanced());
    }
    #[test]
    fn transparent_layout_gaps_do_not_block_world_input() {
        let mut app = super::super::tests::app();
        let layout = app
            .world_mut()
            .spawn((
                panel::PanelRoot,
                panel::LayoutOnly,
                bevy::ui::RelativeCursorPosition {
                    cursor_over: true,
                    normalized: Some(Vec2::splat(0.5)),
                },
            ))
            .id();
        app.update();
        assert!(
            !app.world().resource::<panel::Panel>().blocked,
            "empty layout is world input"
        );
        let island = app
            .world_mut()
            .spawn((
                panel::HitIsland,
                bevy::ui::RelativeCursorPosition {
                    cursor_over: true,
                    normalized: Some(Vec2::splat(0.5)),
                },
            ))
            .id();
        app.update();
        assert!(
            app.world().resource::<panel::Panel>().blocked,
            "visible island must block orders behind it"
        );
        app.world_mut()
            .get_mut::<bevy::ui::RelativeCursorPosition>(island)
            .unwrap()
            .cursor_over = false;
        app.update();
        assert!(!app.world().resource::<panel::Panel>().blocked);
        app.world_mut().despawn(layout);
    }

    #[test]
    fn global_queue_width_tracks_compact_viewport_without_idle_layout_writes() {
        let mut app = super::super::tests::app();
        app.insert_resource(frontend::Menu::new(false)).init_resource::<Time>();
        let window = app.world_mut().spawn(Window { resolution: (960, 540).into(), ..default() }).id();
        let camera = app.world_mut().spawn(Camera2d).id();
        build(&mut app.world_mut().commands(), camera, &TextFont::default());
        app.world_mut().flush();
        app.add_systems(PostUpdate, refresh);
        assert!(app.world_mut().resource_mut::<Scene>().sim.execute_command(Command::Produce { actor: 1, kind: RangeUnitKind::Tank }).accepted());
        let root = app.world_mut().query_filtered::<Entity, With<Root>>().single(app.world()).unwrap();
        let anchor = app.world().get::<Node>(root).unwrap().clone();
        for (width, height) in [(960, 540), (1280, 720), (1920, 1080), (3840, 2160)] {
            app.world_mut().get_mut::<Window>(window).unwrap().resolution = (width, height).into();
            advance(&mut app);
            let node = app.world().get::<Node>(root).unwrap();
            assert_eq!(node.display, Display::Flex);
            assert_eq!(node.width, px((width as f32 * 0.28).clamp(256., 360.)));
            assert_eq!((node.top, node.right), (anchor.top, anchor.right));
            let changed = app.world().entity(root).get_ref::<Node>().unwrap().last_changed();
            let child_ticks: Vec<_> = app
                .world_mut()
                .query_filtered::<Entity, Or<(With<Detail>, With<Meter>)>>()
                .iter(app.world())
                .map(|e| (e, app.world().entity(e).get_ref::<Node>().unwrap().last_changed()))
                .collect();
            assert!(!child_ticks.is_empty());
            advance(&mut app);
            assert_eq!(app.world().entity(root).get_ref::<Node>().unwrap().last_changed(), changed,
                "a steady queue island must not invalidate layout at {width}x{height}");
            for (child, tick) in child_ticks {
                assert_eq!(app.world().entity(child).get_ref::<Node>().unwrap().last_changed(), tick,
                    "steady queue child {child:?} invalidated layout at {width}x{height}");
            }
        }
    }

    #[test]
    fn global_queue_meter_refresh_preserves_interpolated_width_and_static_fallback() {
        let mut app=super::super::tests::app();app.insert_resource(frontend::Menu::new(false)).init_resource::<Time>();
        app.world_mut().spawn(Window{resolution:(960,540).into(),..default()});let camera=app.world_mut().spawn(Camera2d).id();
        build(&mut app.world_mut().commands(),camera,&TextFont::default());app.world_mut().flush();app.add_systems(PostUpdate,refresh);
        assert!(app.world_mut().resource_mut::<Scene>().sim.execute_command(Command::Produce{actor:1,kind:RangeUnitKind::Tank}).accepted());
        advance(&mut app);
        let meter=app.world_mut().query::<(Entity,&Meter)>().iter(app.world()).find(|(_,m)|m.0==0).map(|(e,_)|e).unwrap();
        app.world_mut().get_mut::<Node>(meter).unwrap().width=percent(23.);
        app.world_mut().get_mut::<ui_motion::Progress>(meter).unwrap().goal=77.;
        let tick=app.world().entity(meter).get_ref::<Node>().unwrap().last_changed();
        advance(&mut app);
        assert_eq!(app.world().get::<Node>(meter).unwrap().width,percent(23.),"refresh must leave interpolated width to ui_motion");
        assert_eq!(app.world().entity(meter).get_ref::<Node>().unwrap().last_changed(),tick);
        let goal=app.world().get::<ui_motion::Progress>(meter).unwrap().goal;assert_ne!(goal,77.,"refresh must publish the live queue goal");
        app.world_mut().entity_mut(meter).remove::<ui_motion::Progress>();app.world_mut().get_mut::<Node>(meter).unwrap().width=percent(35.);
        advance(&mut app);assert_eq!(app.world().get::<Node>(meter).unwrap().width,percent(goal),"unanimated meter must still display information");
    }
}
