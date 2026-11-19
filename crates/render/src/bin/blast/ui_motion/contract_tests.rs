//! Fixed-time regression coverage of the existing production motion systems.
use super::*;

fn frame(app:&mut App,ms:u64) {
    app.world_mut().resource_mut::<Time>().advance_by(Duration::from_millis(ms));
    app.update();
}
fn advance(app:&mut App,mut ms:u64) {
    while ms>0 {let step=ms.min(10);frame(app,step);ms-=step;}
}
fn app()->App {
    let mut app=App::new();
    app.init_resource::<Registry>().init_resource::<Settings>().init_resource::<Time>()
        .add_systems(Update,(prepare,render).chain());
    let camera=app.world_mut().spawn_empty().id();
    build(&mut app.world_mut().commands(),camera);app.world_mut().flush();app
}
fn target(app:&mut App,role:Role)->Entity {
    app.world_mut().spawn((Tracked,Target::panel(role),
        Node{left:px(20),top:px(30),width:px(160),height:px(40),..default()},
        ComputedNode{size:Vec2::new(160.,40.),inverse_scale_factor:1.,..default()},
        UiGlobalTransform::default(),BackgroundColor(panel::theme::ACTIVE),
        Interaction::None,Text::new("М108 / Э +68"),
        TextFont{font_size:FontSize::Px(14.),..default()})).id()
}
fn effect(owner:Entity,kind:Kind)->Effect {
    Effect{owner,kind,from:Vec2::new(4.,6.),to:Vec2::new(80.,60.),
        rect:Box2{at:Vec2::new(20.,30.),size:Vec2::new(160.,40.)},
        old:None,age:Duration::ZERO,color:panel::theme::AMBER}
}
fn visible_pieces(app:&mut App)->usize {
    let mut q=app.world_mut().query_filtered::<&Node,With<Piece>>();
    q.iter(app.world()).filter(|n|n.display!=Display::None).count()
}

#[test]
fn fresh_short_effect_renders_its_first_frame_even_after_a_slow_combat_step() {
    for delta in [65,80,90,100] {
        for (over,kind) in [(true,Kind::Hover),(false,Kind::Unhover)] {
            let mut app=app();let owner=target(&mut app,Role::Button);
            app.world_mut().get_mut::<Target>(owner).unwrap().active=Some(false);
            app.world_mut().get_mut::<BackgroundColor>(owner).unwrap().0=Color::BLACK;
            *app.world_mut().get_mut::<Interaction>(owner).unwrap()=if over {Interaction::Hovered}else{Interaction::None};
            app.world_mut().resource_mut::<Registry>().tracks.insert(owner,Track{role:Role::Button,
                shown:true,over:!over,active:false,rect:Box2{at:Vec2::new(20.,30.),size:Vec2::new(160.,40.)},
                base:Color::BLACK,output:Color::BLACK,opening:1.,optical:false,service:false});
            frame(&mut app,delta);
            let effects=&app.world().resource::<Registry>().effects;
            assert_eq!(effects.len(),1,"{kind:?} birth at {delta}ms must render before ageing");
            assert_eq!(effects[0].kind,kind);assert_eq!(effects[0].age,Duration::ZERO);
            assert!(visible_pieces(&mut app)>0,"fresh {kind:?} must have a visible stroke");
            frame(&mut app,kind.lifetime().as_millis() as u64);
            assert!(app.world().resource::<Registry>().effects.is_empty());
            assert_eq!(visible_pieces(&mut app),0);
        }
    }
}

#[test]
fn start_middle_exact_end_and_zero_delta_use_real_time_for_every_motion_kind() {
    for (kind,duration_ms) in [(Kind::Open,240),(Kind::Close,180),(Kind::Hover,90),
        (Kind::Unhover,60),(Kind::Select,120),(Kind::Change,180),
        (Kind::Advance,200),(Kind::Complete,280)] {
        let mut app=app();let owner=app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<Registry>().emit(effect(owner,kind));
        frame(&mut app,0);
        assert_eq!(app.world().resource::<Registry>().effects[0].age,Duration::ZERO);
        assert!(visible_pieces(&mut app)>0,"{kind:?} must be visible at start");
        advance(&mut app,duration_ms/2);
        let age=app.world().resource::<Registry>().effects[0].age;
        assert!(age==Duration::from_millis(duration_ms/2));
        let transforms:Vec<_>=app.world_mut().query_filtered::<&UiTransform,With<Piece>>()
            .iter(app.world()).copied().collect();
        frame(&mut app,0);
        assert_eq!(app.world().resource::<Registry>().effects[0].age,age);
        let unchanged:Vec<_>=app.world_mut().query_filtered::<&UiTransform,With<Piece>>()
            .iter(app.world()).copied().collect();assert_eq!(transforms,unchanged);
        advance(&mut app,duration_ms-duration_ms/2-1);
        assert_eq!(app.world().resource::<Registry>().effects.len(),1,"{kind:?} ended early");
        frame(&mut app,1);
        assert!(app.world().resource::<Registry>().effects.is_empty(),"{kind:?} must end exactly at {duration_ms}ms");
        assert_eq!(visible_pieces(&mut app),0);
        let emitted=app.world().resource::<Registry>().emitted;
        frame(&mut app,0);advance(&mut app,100);
        assert_eq!(app.world().resource::<Registry>().emitted,emitted);
        assert_eq!(visible_pieces(&mut app),0);
    }
}

#[test]
fn real_panel_visibility_and_hover_reverse_without_stale_parallel_effects() {
    let mut app=app();let owner=target(&mut app,Role::Panel);frame(&mut app,0);
    advance(&mut app,70);
    app.world_mut().get_mut::<Node>(owner).unwrap().display=Display::None;
    frame(&mut app,0);
    let kinds:Vec<_>=app.world().resource::<Registry>().effects.iter().map(|e|e.kind).collect();
    assert!(kinds.contains(&Kind::Close));assert!(!kinds.contains(&Kind::Open));
    advance(&mut app,30);
    app.world_mut().get_mut::<Node>(owner).unwrap().display=Display::Flex;
    frame(&mut app,0);
    let r=app.world().resource::<Registry>();
    assert_eq!(r.effects.iter().filter(|e|matches!(e.kind,Kind::Open|Kind::Close)).count(),1);
    assert!(r.effects.iter().any(|e|e.kind==Kind::Open && e.age==Duration::ZERO));
    *app.world_mut().get_mut::<Interaction>(owner).unwrap()=Interaction::Hovered;
    frame(&mut app,0);advance(&mut app,30);
    *app.world_mut().get_mut::<Interaction>(owner).unwrap()=Interaction::None;
    frame(&mut app,0);
    let r=app.world().resource::<Registry>();
    assert!(r.effects.iter().any(|e|e.kind==Kind::Unhover));
    assert!(!r.effects.iter().any(|e|e.kind==Kind::Hover));
    advance(&mut app,300);assert!(app.world().resource::<Registry>().effects.is_empty());
}

#[test]
fn retrigger_restarts_only_the_matching_event_and_preserves_other_owners() {
    let mut app=app();let a=app.world_mut().spawn_empty().id();let b=app.world_mut().spawn_empty().id();
    for owner in [a,b] {app.world_mut().resource_mut::<Registry>().emit(effect(owner,Kind::Select));}
    advance(&mut app,40);
    app.world_mut().resource_mut::<Registry>().emit(effect(a,Kind::Select));frame(&mut app,0);
    let r=app.world().resource::<Registry>();assert_eq!(r.effects.len(),2);
    assert_eq!(r.effects.iter().find(|e|e.owner==a).unwrap().age,Duration::ZERO);
    assert!(r.effects.iter().find(|e|e.owner==b).unwrap().age==Duration::from_millis(40));
    advance(&mut app,80);
    let r=app.world().resource::<Registry>();assert_eq!(r.effects.len(),1);assert_eq!(r.effects[0].owner,a);
    advance(&mut app,40);assert!(app.world().resource::<Registry>().effects.is_empty());
}

#[test]
fn reduced_effects_preserve_live_text_hit_rectangles_and_same_frame_input() {
    for intensity in [100,35,0] {
        let mut app=app();app.world_mut().resource_mut::<Settings>().percent=intensity;
        let owner=target(&mut app,Role::Button);
        let font=app.world().get::<TextFont>(owner).unwrap().font_size;
        let before=rect(app.world().get::<ComputedNode>(owner).unwrap(),app.world().get::<UiGlobalTransform>(owner).unwrap());
        frame(&mut app,0);advance(&mut app,30);
        *app.world_mut().get_mut::<Interaction>(owner).unwrap()=Interaction::Pressed;
        frame(&mut app,0);
        assert_eq!(app.world().resource::<Registry>().pressed,before.map(Box2::centre),"{intensity}% must see input in the same frame");
        for ms in [0,10,40,100] {
            frame(&mut app,ms);
            let w=app.world();assert_eq!(w.get::<Text>(owner).unwrap().0,"М108 / Э +68");
            assert_eq!(w.get::<TextFont>(owner).unwrap().font_size,font);
            assert_eq!(rect(w.get::<ComputedNode>(owner).unwrap(),w.get::<UiGlobalTransform>(owner).unwrap()),before);
            let node=w.get::<Node>(owner).unwrap();
            assert_eq!((node.left,node.top,node.width,node.height),(px(20),px(30),px(160),px(40)));
        }
        if intensity==0 {assert_eq!(visible_pieces(&mut app),0);}
    }
}

#[test]
fn progress_retargets_mid_transition_without_inventing_work_or_advancing_at_zero_delta() {
    let mut p=Progress::default();p.step(0.);p.goal=40.;p.step(0.04);
    let middle=p.value;assert!(middle>0. && middle<40.);
    p.goal=80.;assert_eq!(p.step(0.),middle);assert_eq!(p.step(0.),middle);
    p.step(0.04);assert!(p.value>middle && p.value<80.);
    assert_eq!(p.step(0.04),80.);
    p.goal=0.;assert_eq!(p.step(0.),0.,"a new order resets immediately even without elapsed time");
}

#[test]
fn one_click_queues_once_and_real_production_exit_completes_once_at_every_intensity() {
    use modding::range::{RangeBlock,RangeLayout,RangeUnitDef};
    use sim_core::blast::battle::commands::Command;
    for intensity in [100,35,0] {
        let mut layout=RangeLayout::from_ron(include_str!("../../../../../../data/fixture-drive.ron")).unwrap();
        layout.idle_enemies=true;layout.size=[1536,256,1536];layout.clear_min=[1,1];layout.clear_max=[1535,1535];
        layout.blocks=vec![RangeBlock{min:[0;3],max:[1536,256,1536],material:0},
            RangeBlock{min:[0;3],max:[1536,16,1536],material:1}];
        layout.units=[(RangeUnitKind::KilFactoryT1,true,[650,16,650]),
            (RangeUnitKind::Base,true,[150,16,1200]),
            (RangeUnitKind::PowerPlantT1,true,[1100,16,1000]),
            (RangeUnitKind::Tank,false,[1420,16,1420])].map(|(kind,allied,at)|RangeUnitDef{kind,allied,at,weapon:None}).to_vec();
        let mut economy=modding::range_economy::RangeEconomy::stock();
        economy.bank.as_mut().unwrap().base_store=1<<40;
        economy.kinds.iter_mut().find(|k|k.kind==RangeUnitKind::Tank).unwrap().build_secs=1;
        layout.economy=Some(economy);
        layout.ready=Some(modding::range::RangeReady{store:1<<34,..default()});
        let mut sim=BlastRange::with_layout(config(false,false),layout.clone()).unwrap();
        sim.set_physics_workers(1);sim.set_navigation_workers(1);sim.start_battle().unwrap();
        let mut edit=Workshop::new(Some(layout),false);edit.controlled=Some(0);edit.selected_units=vec![0];
        let mut app=panel::hud::tests::app_with_scene(sim,edit);
        app.world_mut().resource_mut::<panel::Panel>().sandbox=false;
        app.init_resource::<Time>().init_resource::<Registry>()
            .insert_resource(Settings{percent:intensity})
            .init_resource::<construction::Catalog>()
            .insert_resource(frontend::Menu::new(false))
            .insert_resource(laboratory::Laboratory::new(false))
            .init_resource::<pilot::Pilot>().init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<construction::production_hud::State>()
            .add_systems(Update,(construction::input,prepare,render).chain())
            .add_systems(PostUpdate,(construction::production_hud::refresh,production).chain());
        let root=target(&mut app,Role::Panel);
        app.world_mut().entity_mut(root).insert(construction::production_hud::Root);
        frame(&mut app,0);
        let choose=app.world_mut().spawn((construction::Action::Choose(0),Interaction::Pressed)).id();
        frame(&mut app,0);
        assert_eq!(app.world().resource::<Scene>().sim.production_queue(0),Some(vec![RangeUnitKind::Tank]),"{intensity}% must issue exactly one immediate order");
        for _ in 0..4 {frame(&mut app,10);}
        assert_eq!(app.world().resource::<Scene>().sim.production_queue(0),Some(vec![RangeUnitKind::Tank]),"held interaction must not repeat an order");
        *app.world_mut().get_mut::<Interaction>(choose).unwrap()=Interaction::None;
        // Cancellation advances the real queue, never reports a completed product.
        assert!(app.world_mut().resource_mut::<Scene>().sim.execute_command(Command::Cancel{actor:0}).accepted());
        frame(&mut app,60);assert_eq!(app.world().resource::<Registry>().completed,0);
        *app.world_mut().get_mut::<Interaction>(choose).unwrap()=Interaction::Pressed;
        frame(&mut app,0);
        *app.world_mut().get_mut::<Interaction>(choose).unwrap()=Interaction::None;
        let mut exit=None;
        for _ in 0..100 {
            app.world_mut().resource_mut::<Scene>().sim.step();frame(&mut app,50);
            exit=app.world().resource::<Scene>().sim.production_exit(0);
            if exit.is_some(){break;}
        }
        let exit=exit.expect("the real paid queue must release its product");
        assert_eq!(app.world().resource::<Scene>().sim.units[exit.0].kind,RangeUnitKind::Tank);
        assert_eq!(app.world().resource::<Registry>().completed,1);
        assert_eq!(app.world().resource::<Registry>().production[&0].exit,Some(exit));
        assert!(app.world().resource::<Scene>().sim.production_queue(0).unwrap_or_default().is_empty());
        for _ in 0..10 {app.world_mut().resource_mut::<Scene>().sim.step();frame(&mut app,50);}
        assert_eq!(app.world().resource::<Registry>().completed,1,"the same real exit may not retrigger completion");
        assert!(app.world().resource::<Scene>().sim.matter.economy.ledger.balanced());
    }
}

#[test]
fn intensity_label_settles_without_idle_writes_and_cycles_on_press() {
    let mut app=App::new();
    app.init_resource::<Settings>().init_resource::<Time>().add_systems(Update,input);
    app.world_mut().commands().spawn(Node::default()).with_children(|p|settings_control(p,&TextFont::default()));
    app.world_mut().flush();
    frame(&mut app,16);
    let label=app.world_mut().query_filtered::<Entity,With<IntensityLabel>>().single(app.world()).unwrap();
    let button=app.world_mut().query_filtered::<Entity,With<IntensityButton>>().single(app.world()).unwrap();
    assert_eq!(app.world().get::<Text>(label).unwrap().0,"UI эффекты: 100% · сменить");
    let quiet=app.world().entity(label).get_ref::<Text>().unwrap().last_changed();
    frame(&mut app,16);
    assert_eq!(app.world().entity(label).get_ref::<Text>().unwrap().last_changed(),quiet);
    *app.world_mut().get_mut::<Interaction>(button).unwrap()=Interaction::Pressed;
    frame(&mut app,16);
    assert_eq!(app.world().resource::<Settings>().percent,35);
    assert_eq!(app.world().get::<Text>(label).unwrap().0,"UI эффекты: 35% · сменить");
    *app.world_mut().get_mut::<Interaction>(button).unwrap()=Interaction::None;
    let settled=app.world().entity(label).get_ref::<Text>().unwrap().last_changed();
    frame(&mut app,16);
    assert_eq!(app.world().entity(label).get_ref::<Text>().unwrap().last_changed(),settled);
}
