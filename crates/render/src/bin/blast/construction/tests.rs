use super::*;
use modding::range::{RangeBlock, RangeLayout, RangeUnitDef};

#[test]
fn fixed_catalogue_has_one_owner_independent_of_projection() {
    let mut app = App::new();
    app.init_resource::<Catalog>()
        .init_resource::<spatial_ui::State>()
        .add_systems(Update, sync_visibility);
    let root = app.world_mut().spawn((Root, Node::default())).id();
    let unrelated = app.world_mut().spawn(Node::default()).id();
    // Sandbox fallback is requested even for non-producing selected units.
    app.world_mut().resource_mut::<Catalog>().legacy_requested = true;
    for projected in [true, true, false, true, false] {
        app.world_mut().resource_mut::<spatial_ui::State>().visible = projected;
        app.update();
        assert_eq!(app.world().get::<Node>(root).unwrap().display,
            Display::Flex);
        assert_eq!(app.world().get::<Node>(unrelated).unwrap().display, Display::Flex);
    }
    // Closing the catalogue (menu/lab/placement/hidden context) must not reopen
    // it when the projection disappears in the same frame.
    app.world_mut().resource_mut::<Catalog>().legacy_requested = false;
    app.update();
    assert_eq!(app.world().get::<Node>(root).unwrap().display, Display::None);
}

#[test]
fn greben_preview_distinguishes_headroom_from_real_terrain() {
    let at = [900, 16, 700];
    for height in [192, 512] {
        let mut l = RangeLayout::from_ron(include_str!("../../../../../../data/fixture-drive.ron")).unwrap();
        l.size = [1600, height, 1600]; l.battle = true; l.course = None;
        l.ready = None; l.shields.clear(); l.clear_min = [1, 1]; l.clear_max = [1599, 1599];
        l.blocks = vec![RangeBlock { min: [0; 3], max: [1600, height as i32, 1600], material: 0 },
            RangeBlock { min: [0; 3], max: [1600, 16, 1600], material: 1 }];
        l.units = vec![RangeUnitDef { kind: RangeUnitKind::Commander, allied: true,
            at: [250, 16, 700], weapon: None }];
        l.economy = Some(modding::range_economy::RangeEconomy::stock());
        let mut sim = BlastRange::with_layout(config(false, false), l).unwrap();
        let issue = placement_issue(&sim, RangeUnitKind::GrebenStorage, at, 0, true);
        if height == 192 {
            assert!(issue.unwrap().contains("высоты карты"));
        } else {
            assert!(issue.is_none(), "clear level pad should have a green preview: {issue:?}");
            let (lo, _) = sim_core::blast::placement::Validator::default()
                .runtime_bounds(RangeUnitKind::GrebenStorage, at, 0);
            sim.world.queue_edit(voxel::Edit::put(lo[0] as u32, lo[1] as u32, lo[2] as u32, 2));
            sim.world.flush();
            assert!(placement_issue(&sim, RangeUnitKind::GrebenStorage, at, 0, true)
                .unwrap().contains("грунтом"));
            assert!(placement_issue(&sim, RangeUnitKind::GrebenStorage, [40, 16, 40], 0, true)
                .unwrap().contains("край карты"));
        }
    }
}

fn fixture() -> (BlastRange, Workshop) {
    let mut layout = modding::laboratory::Definition::stock().layout;
    layout.battle = true;
    layout.idle_enemies = true;
    layout.size = [4096, 1024, 4096];
    layout.gun = [128, 24, 128];
    layout.target = [3600, 24, 2800];
    layout.clear_min = [1, 1];
    layout.clear_max = [4095, 4095];
    layout.blocks = vec![
        RangeBlock {
            min: [0; 3],
            max: [4096, 1024, 4096],
            material: 0,
        },
        RangeBlock {
            min: [0; 3],
            max: [4096, 16, 4096],
            material: 1,
        },
    ];
    layout.units = [
        (RangeUnitKind::GrebenStorage, true, [900, 16, 900]),
        (RangeUnitKind::KilFactoryT1, true, [2600, 16, 900]),
        (RangeUnitKind::SkobaDrone, true, [1700, 80, 1300]),
        (RangeUnitKind::Tank, true, [1200, 16, 2800]),
        (RangeUnitKind::Tank, false, [3600, 16, 2800]),
        (RangeUnitKind::PowerPlantT1, true, [2500, 16, 1600]),
        (RangeUnitKind::PowerPlantT1, true, [2700, 16, 1800]),
        (RangeUnitKind::PowerPlantT1, true, [900, 16, 1500]),
    ]
    .map(|(kind, allied, at)| RangeUnitDef {
        kind,
        allied,
        at,
        weapon: None,
    })
    .to_vec();
    let mut economy = modding::range_economy::RangeEconomy::stock();
    economy.bank.as_mut().unwrap().base_store = 1 << 40;
    layout.economy = Some(economy);
    layout.ready = Some(modding::range::RangeReady {
        store: 1 << 36, // Covers the complete current T1→T4 physical body quotes.
        ..default()
    });
    let mut sim = BlastRange::with_layout(config(false, false), layout.clone()).unwrap();
    assert!(sim.economic_rules().timed_upgrades, "UI fixture must retain stock timed upgrades");
    sim.set_physics_workers(1);
    sim.set_navigation_workers(1);
    sim.start_battle().unwrap();
    (sim, Workshop::new(Some(layout), false))
}
pub(crate) fn app() -> App {
    let (sim, edit) = fixture();
    let mut app = panel::hud::tests::app_with_scene(sim, edit);
    app.init_resource::<Catalog>()
        .insert_resource(laboratory::Laboratory::new(false))
        .init_resource::<pilot::Pilot>()
        .init_resource::<ButtonInput<MouseButton>>()
        .add_systems(Update, input.after(panel::actions));
    app.world_mut().resource_mut::<panel::Panel>().sandbox = false;
    app
}
fn click(app: &mut App, action: Action) {
    let e = app.world_mut().spawn((action, Interaction::Pressed)).id();
    app.update();
    app.world_mut().despawn(e);
}
fn select(app: &mut App, id: usize) {
    app.world_mut().resource_mut::<Workshop>().controlled = Some(id);
    app.world_mut().resource_mut::<Workshop>().selected_units = vec![id];
    app.update();
}

fn finish_timed_factory_upgrade(sim: &mut BlastRange, next: RangeUnitKind) {
    let previous = sim.units[1].kind;
    let body_matter = sim.matter.economy.ledger.held(sim.matter.body_container(1));
    assert!(sim.upgrade_kil(1));
    assert_eq!(sim.units[1].kind, previous, "upgrade must wait in the real queue");
    assert_eq!(sim.production_queue(1), Some(vec![next]));
    let required = sim.production_progress(1).expect("queued timed upgrade").1;
    assert!(required > 1);
    assert!(sim.production_power(1) > 0);
    assert!(sim.construction_power_networks()[1].is_some_and(|(supply, _)| supply >= sim.production_power(1)));
    let mut checked_partial = false;
    for _ in 0..required + 2 {
        if sim.units[1].kind == next {
            break;
        }
        sim.step();
        if let Some((elapsed, ticks, paid, price)) = sim.production_progress(1) {
            if !checked_partial && elapsed >= ticks / 2 && elapsed < ticks {
                assert!(paid > 0 && price > paid, "partial upgrade must pay real matter");
                let mut restored = BlastRange::restore(sim.config, &sim.snapshot())
                    .expect("partially paid timed upgrade restores");
                assert_eq!(restored.state_hash(), sim.state_hash());
                restored.set_physics_workers(1);
                restored.set_navigation_workers(1);
                sim.step();
                restored.step();
                assert_eq!(restored.state_hash(), sim.state_hash(), "timed upgrade continues identically after restore");
                *sim = restored;
                checked_partial = true;
            }
        }
    }
    assert_eq!(sim.units[1].kind, next,
        "factory upgrade stalled: queue={:?} progress={:?} power={} network={:?} store={:?} phase={:?}",
        sim.production_queue(1), sim.production_progress(1), sim.production_power(1),
        sim.construction_power_networks()[1], sim.side_store(true), sim.battle.phase);
    assert!(checked_partial, "must exercise an unfinished paid upgrade and restore");
    assert!(sim.production_queue(1).unwrap_or_default().is_empty());
    assert!(sim.matter.economy.ledger.held(sim.matter.body_container(1)) > body_matter);
    assert!(sim.matter.economy.ledger.balanced());
}

#[test]
fn greben_switches_between_buildings_and_paid_unit_queue() {
    let mut app=app();
    select(&mut app,0);
    assert_eq!(app.world().resource::<Catalog>().context,Context::Engineer(0));
    click(&mut app,Action::GrebenMode);app.update();
    assert_eq!(app.world().resource::<Catalog>().context,Context::Factory(0));
    click(&mut app,Action::Choose(0));
    assert_eq!(app.world().resource::<Scene>().sim.production_queue(0),Some(vec![RangeUnitKind::SkobaDrone]));
    click(&mut app,Action::GrebenMode);app.update();
    assert_eq!(app.world().resource::<Catalog>().context,Context::Engineer(0));
    assert_eq!(queue_actor(Context::Engineer(0),&app.world().resource::<Scene>().sim),Some(0));
    click(&mut app,Action::Cancel);
    assert!(app.world().resource::<Scene>().sim.production_queue(0).is_none());
    select(&mut app,1);
    assert_eq!(app.world().resource::<Catalog>().context,Context::Factory(1));
}

#[test]
fn blueprint_catalogue_queue_cancel_and_preview_fixture() {
    let mut layout=RangeLayout::from_ron(include_str!("../../../../../../data/fixture-drive.ron")).unwrap();
    layout.size=[1600,384,1600];layout.battle=true;layout.idle_enemies=true;layout.course=None;layout.ready=None;layout.shields.clear();layout.unit_yaws.clear();
    layout.gun=[250,24,400];layout.target=[950,16,850];layout.clear_min=[1,1];layout.clear_max=[1599,1599];
    layout.blocks=vec![RangeBlock{min:[0;3],max:[1600,384,1600],material:0},RangeBlock{min:[0;3],max:[1600,16,1600],material:1}];
    layout.units=vec![RangeUnitDef{kind:RangeUnitKind::Commander,allied:true,at:[250,16,700],weapon:None},RangeUnitDef{kind:RangeUnitKind::Base,allied:true,at:[250,16,1050],weapon:None},RangeUnitDef{kind:RangeUnitKind::Tank,allied:false,at:[1450,16,1400],weapon:None}];
    layout.economy=Some(modding::range_economy::RangeEconomy::stock());
    let mut sim=BlastRange::with_layout(config(false,false),layout.clone()).unwrap();sim.start_battle().unwrap();sim.set_physics_workers(1);sim.set_navigation_workers(1);
    let mut edit=Workshop::new(Some(layout),false);edit.controlled=Some(0);edit.selected_units=vec![0];
    let first=place_selected_building(&mut sim,&edit,RangeUnitKind::TurretT1,[500,16,700],false).unwrap();
    place_selected_building(&mut sim,&edit,RangeUnitKind::PowerPlantT1,[800,16,700],true).unwrap();
    let last=place_selected_building(&mut sim,&edit,RangeUnitKind::KilFactoryT1,[1100,16,1100],true).unwrap();
    assert_eq!(sim.order_queue(0).len(),3);assert_eq!(sim.battle.actors[0].order,sim_core::blast::battle::Order::Build(first));
    let tank=sim.mining().unwrap().tank(0).unwrap();sim.matter.economy.ledger.generate(tank,2_000_000);
    for _ in 0..24 {sim.step();}
    let upper=production_hud::collect(&sim);
    let plan=upper.iter().find(|l|l.actor==0).expect("queued construction remains globally visible");
    assert_eq!(plan.queue.len(),3);
    assert_eq!(plan.queue[0],RangeUnitKind::TurretT1);
    let bytes=sim.snapshot();assert_eq!(BlastRange::restore(sim.config,&bytes).unwrap().state_hash(),sim.state_hash());
    let fixture_dir=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out/build-plans-539");
    std::fs::create_dir_all(&fixture_dir).unwrap();
    std::fs::write(fixture_dir.join("preview.bin"),bytes).unwrap();
    let mut app=panel::hud::tests::app_with_scene(sim,edit);
    app.init_resource::<Catalog>().insert_resource(laboratory::Laboratory::new(false)).init_resource::<pilot::Pilot>()
        .init_resource::<ButtonInput<MouseButton>>().add_systems(Update,input.after(panel::actions));
    app.world_mut().resource_mut::<panel::Panel>().sandbox=false;
    select(&mut app,0);click(&mut app,Action::Cancel);
    assert!(app.world().resource::<Scene>().sim.construction_cancelled(last));
    assert_eq!(app.world().resource::<Scene>().sim.order_queue(0).len(),2);
}
#[test]
fn explicit_group_assigns_all_selected_drones_and_warehouse_resumes_automatic_dispatch() {
    let (mut sim, mut edit) = fixture();
    let drones: Vec<_> = sim.units.iter().enumerate().filter(|(id, u)|
        sim.allied(*id) && u.kind == RangeUnitKind::SkobaDrone).map(|(id, _)| id).collect();
    assert!(drones.len() >= 2);
    edit.controlled = Some(drones[0]);
    edit.selected_units = drones.clone();
    let site = sim.create_construction_site(RangeUnitKind::TurretT1, true, [1800, 16, 1800]).unwrap();
    assert_eq!(assign_selected_builders_queued(&mut sim, &edit, site,false), Some(drones.len()));
    assert_eq!(sim.construction_sites()[site].quota, 0);
    assert!(drones.iter().all(|&id| sim.battle.actors[id].order == sim_core::blast::battle::Order::Build(site)));
    edit.controlled = Some(0);
    edit.selected_units = vec![0];
    let auto = sim.create_construction_site(RangeUnitKind::TurretT1, true, [2100, 16, 1800]).unwrap();
    assert_eq!(assign_selected_builders_queued(&mut sim, &edit, auto,false), Some(1));
    assert!(sim.construction_sites()[auto].quota > 0);
}

#[test]
fn catalogue_modes_gate_engineer_factory_enemy_and_sandbox() {
    let mut app = app();
    for (id, expected) in [
        (3, Context::Hidden),
        (4, Context::Hidden),
        (2, Context::Engineer(2)),
        (0, Context::Engineer(0)),
        (1, Context::Factory(1)),
    ] {
        select(&mut app, id);
        assert_eq!(app.world().resource::<Catalog>().context, expected);
    }
    app.world_mut().resource_mut::<panel::Panel>().sandbox = true;
    select(&mut app, 4);
    assert_eq!(app.world().resource::<Catalog>().context, Context::Sandbox);
    select(&mut app, 2);
    assert_eq!(app.world().resource::<Catalog>().context, Context::Engineer(2));
    select(&mut app, 0);
    assert_eq!(app.world().resource::<Catalog>().context, Context::Engineer(0));
    select(&mut app, 1);
    assert_eq!(app.world().resource::<Catalog>().context, Context::Factory(1));
    select(&mut app, 4);
    assert_eq!(
        choices(
            app.world().resource::<Catalog>(),
            &app.world().resource::<Scene>().sim
        )
        .len(),
        RangeUnitKind::COUNT
    );
    app.world_mut().resource_mut::<pilot::Pilot>().actor = Some(3);
    app.update();
    assert_eq!(app.world().resource::<Catalog>().context, Context::Hidden);
}
#[test]
fn free_placement_in_battle_can_be_toggled_without_changing_battle_mode() {
    let mut layout = modding::laboratory::Definition::stock().layout;
    layout.battle = true;
    layout.units = [(true, [160, 16, 160]), (false, [480, 16, 480])]
        .map(|(allied, at)| RangeUnitDef { kind: RangeUnitKind::Tank, allied, at, weapon: None })
        .to_vec();
    let mut sim = BlastRange::with_layout(config(false, false), layout.clone()).unwrap();
    sim.set_physics_workers(1);
    sim.set_navigation_workers(1);
    sim.start_battle().unwrap();
    let mut app = panel::hud::tests::app_with_scene(sim, Workshop::new(Some(layout), false));
    app.init_resource::<Catalog>()
        .insert_resource(laboratory::Laboratory::new(false))
        .init_resource::<pilot::Pilot>()
        .init_resource::<ButtonInput<MouseButton>>()
        .add_systems(Update, input.after(panel::actions));
    app.world_mut().resource_mut::<panel::Panel>().sandbox = false;
    select(&mut app, 0);
    assert_eq!(app.world().resource::<Catalog>().context, Context::Hidden);
    let generation = app.world().resource::<Scene>().generation;
    let e = app.world_mut().spawn((panel::Action::FreePlacement, Interaction::Pressed)).id();
    app.update();
    app.world_mut().despawn(e);
    assert_eq!(app.world().resource::<Catalog>().context, Context::Sandbox);
    assert_eq!(app.world().resource::<Scene>().generation, generation);
    assert!(!app.world().resource::<panel::Panel>().sandbox);
    assert!(!editing_allowed(&app.world().resource::<Scene>().sim, Some(app.world().resource::<panel::Panel>())));
    assert!(choices(app.world().resource::<Catalog>(), &app.world().resource::<Scene>().sim)
        .contains(&RangeUnitKind::Tank));
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        let count = scene.sim.units.len();
        let tick = scene.sim.tick;
        scene.sim.place_sandbox_unit(RangeUnitKind::Tank, true, [320, 16, 320], 0).unwrap();
        assert_eq!(scene.sim.units.len(), count + 1);
        assert_eq!(scene.sim.tick, tick);
    }
    select(&mut app, 1);
    assert_eq!(app.world().resource::<Catalog>().context, Context::Sandbox);
    let e = app.world_mut().spawn((panel::Action::FreePlacement, Interaction::Pressed)).id();
    app.update();
    app.world_mut().despawn(e);
    assert_eq!(app.world().resource::<Catalog>().context, Context::Hidden);
    assert_eq!(app.world().resource::<Scene>().generation, generation);
}
#[test]
fn all_tiers_filter_real_recipes_and_factory_clicks_queue_and_refund() {
    let mut app = app();
    assert!(app.world_mut().resource_mut::<Scene>().sim.battle_order(
        3, sim_core::blast::battle::Order::Hold, None,
    ));
    select(&mut app, 1);
    click(&mut app, Action::Choose(0));
    assert_eq!(
        app.world()
            .resource::<Scene>()
            .sim
            .production_queue(1)
            .unwrap(),
        vec![RangeUnitKind::Tank]
    );
    click(&mut app, Action::Cancel);
    assert!(app
        .world()
        .resource::<Scene>()
        .sim
        .production_queue(1)
        .unwrap_or_default()
        .is_empty());
    assert!(!app.world().resource::<Catalog>().placing());
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        for next in [RangeUnitKind::KilFactoryT2, RangeUnitKind::KilFactoryT3] {
            finish_timed_factory_upgrade(&mut scene.sim, next);
        }
    }
    click(&mut app, Action::Tier(3));
    let kinds = choices(
        app.world().resource::<Catalog>(),
        &app.world().resource::<Scene>().sim,
    );
    assert!(kinds.contains(&RangeUnitKind::Spider) && kinds.contains(&RangeUnitKind::Walker04));
    assert!(kinds.iter().all(|k| tier(*k) == 3 && !k.emplacement()));
    click(&mut app, Action::Choose(0));
    assert_eq!(
        app.world()
            .resource::<Scene>()
            .sim
            .production_queue(1)
            .unwrap(),
        vec![RangeUnitKind::Spider]
    );
    assert!(app
        .world()
        .resource::<Scene>()
        .sim
        .matter
        .economy
        .ledger
        .balanced());
    let sim = &app.world().resource::<Scene>().sim;
    let restored = BlastRange::restore(sim.config, &sim.snapshot())
        .expect("upgraded factory keeps paid matter and its queue");
    assert_eq!(sim.state_hash(), restored.state_hash());
}
#[test]
fn placement_cancels_on_selection_generation_and_escape() {
    let mut app = app();
    select(&mut app, 2);
    click(&mut app, Action::Choose(0));
    assert!(app.world().resource::<Catalog>().placing());
    click(&mut app, Action::DismissPlacement);
    assert!(!app.world().resource::<Catalog>().placing());
    click(&mut app, Action::Choose(0));
    assert!(app.world().resource::<Catalog>().placing());
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Escape);
    app.update();
    assert!(!app.world().resource::<Catalog>().placing());
    assert!(!app
        .world()
        .resource::<ButtonInput<KeyCode>>()
        .just_pressed(KeyCode::Escape));
    click(&mut app, Action::Choose(0));
    select(&mut app, 3);
    assert!(!app.world().resource::<Catalog>().placing());
    select(&mut app, 2);
    click(&mut app, Action::Choose(0));
    app.world_mut().resource_mut::<Scene>().generation += 1;
    app.update();
    assert!(!app.world().resource::<Catalog>().placing());
}

#[test]
fn a_card_from_the_previous_selection_cannot_order_for_another_factory() {
    let mut app = app();
    select(&mut app, 1);
    app.world_mut().resource_mut::<Workshop>().controlled = Some(0);
    click(&mut app, Action::Choose(0));
    assert_eq!(
        app.world().resource::<Catalog>().context,
        Context::Engineer(0)
    );
    assert!(app
        .world()
        .resource::<Scene>()
        .sim
        .production_queue(0)
        .is_none());
    click(&mut app, Action::GrebenMode);
    app.update();
    assert_eq!(app.world().resource::<Catalog>().context, Context::Factory(0));
    click(&mut app, Action::Choose(0));
    assert_eq!(
        app.world().resource::<Scene>().sim.production_queue(0),
        Some(vec![RangeUnitKind::SkobaDrone])
    );
}

#[test]
fn combat_edit_shortcut_and_placement_camera_follow_the_mode_policy() {
    use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
    let mut app = app();
    let window = app
        .world_mut()
        .spawn(Window {
            focused: true,
            ..default()
        })
        .id();
    app.add_message::<MouseWheel>();
    app.add_systems(Update, workbench::input.after(input));
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyE);
    app.update();
    assert!(!app.world().resource::<Workshop>().editing);
    assert!(app.world().resource::<Workshop>().pending.is_none());
    assert!(app.world().resource::<Scene>().notice.contains("инженерам"));
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
    select(&mut app, 2);
    click(&mut app, Action::Choose(0));
    let before = app.world().resource::<Workshop>().zoom;
    app.world_mut().write_message(MouseWheel {
        unit: MouseScrollUnit::Line,
        phase: bevy::input::touch::TouchPhase::Moved,
        x: 0.0,
        y: 1.0,
        window,
    });
    app.update();
    assert!(app.world().resource::<Workshop>().zoom < before);
    assert!(app.world().resource::<Catalog>().placing());
    app.world_mut().resource_mut::<Catalog>().pending = None;
    app.world_mut().resource_mut::<panel::Panel>().sandbox = true;
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyE);
    app.update();
    assert!(app.world().resource::<Workshop>().editing);
    assert!(app.world().resource::<Workshop>().pending.is_some());
}

#[test]
fn all_stock_scenarios_offer_only_their_live_allied_builders_recipes() {
    let config = config(false, false);
    let base = RangeLayout::from_ron(include_str!("../../../../../../data/range-km.ron")).unwrap();
    for id in 0..modding::range_skirmish::STOCK.len() {
        let scenario = modding::range_skirmish::stock(id).unwrap();
        let layout = scenario.apply_to(base.clone()).unwrap();
        let mut sim = BlastRange::with_layout(config, layout.clone()).unwrap();
        sim.set_physics_workers(1);
        sim.set_navigation_workers(1);
        let mut panel = panel::Panel::new(false, false);
        panel.sandbox = false;
        let mut edit = Workshop::new(Some(layout), true);
        assert_eq!(
            context(&sim, &panel, &edit),
            Context::Sandbox,
            "{id}: setup"
        );
        sim.start_battle().unwrap();
        edit.editing = false;
        assert!(!editing_allowed(&sim, Some(&panel)), "{id}: combat");
        for actor in 0..sim.units.len() {
            edit.controlled = Some(actor);
            let actual = context(&sim, &panel, &edit);
            let u = &sim.units[actor];
            if !sim.allied(actor) || !u.status.alive || sim.unit_under_construction(actor) {
                assert_eq!(actual, Context::Hidden, "{id}/{actor}");
            } else if matches!(u.kind, RangeUnitKind::SkobaDrone | RangeUnitKind::Commander | RangeUnitKind::GrebenStorage) {
                assert_eq!(actual, Context::Engineer(actor), "{id}/{actor}");
            } else {
                let recipes = products(&sim, actor);
                assert_eq!(
                    actual,
                    if recipes.is_empty() {
                        Context::Hidden
                    } else {
                        Context::Factory(actor)
                    },
                    "{id}/{actor}"
                );
                for kind in recipes {
                    assert!(
                        sim.production_cost(kind).is_some(),
                        "{id}: unpriced {kind:?}"
                    );
                }
            }
        }
        panel.sandbox = true;
        edit.controlled = None;
        assert_eq!(
            context(&sim, &panel, &edit),
            Context::Sandbox,
            "{id}: sandbox"
        );
        eprintln!("catalogue scenario {id}: {} OK", scenario.title);
    }
}

#[test]
fn catalogue_refresh_tracks_recipes_costs_upgrade_controls_and_hidden_modes() {
    let mut app = app();
    app.insert_resource(Assets::<Image>::default())
        .insert_resource(Templates::default())
        .insert_resource(frontend::Menu::new(false))
        .init_resource::<spatial_ui::State>()
        .add_systems(PostUpdate, (panel::hud::refresh, refresh, sync_visibility).chain());
    app.world_mut().spawn(Window {
        resolution: (960, 540).into(),
        ..default()
    });
    for kind in RangeUnitKind::ALL {
        app.world_mut()
            .resource_mut::<Catalog>()
            .icons
            .insert(kind.index(), Handle::default());
    }
    let queue_row = app.world_mut().spawn((QueueWidget, QueueRow, Node::default())).id();
    let queue_card = app.world_mut().spawn((QueueWidget, QueueCard(0), Node::default(), BackgroundColor(theme::BUTTON), BorderColor::all(theme::FRAME))).id();
    let queue_pip = app.world_mut().spawn((QueueWidget, QueuePip(0, 0), Node::default(), BackgroundColor(theme::FRAME))).id();
    let queue_number = app.world_mut().spawn((QueueText(Some(0)), Text::new(""))).id();
    let queue_header = app.world_mut().spawn((QueueText(None), Text::new(""))).id();
    let root = app.world_mut().spawn((Root, Node::default())).id();
    let deck = app
        .world_mut()
        .spawn((panel::hud::CommandDeck, Node::default()))
        .id();
    let map = app
        .world_mut()
        .spawn((panel::minimap::MiniRoot, Node {left:px(16),bottom:px(16),..default()}))
        .id();
    let upgrade = app
        .world_mut()
        .spawn((SandboxControl(1), Node::default()))
        .id();
    let caption = app
        .world_mut()
        .spawn((Caption::Item(0), Text::new("")))
        .id();
    let heading = app
        .world_mut()
        .spawn((Caption::Heading, Text::new("")))
        .id();
    let hint = app.world_mut().spawn((Caption::Hint, Text::new(""))).id();
    let briefing = app
        .world_mut()
        .spawn((panel::hud::BriefingCard, Node::default()))
        .id();
    select(&mut app, 1);
    assert_eq!(
        app.world().get::<Node>(root).unwrap().display,
        Display::Flex
    );
    assert_eq!(
        app.world().get::<Node>(deck).unwrap().display,
        Display::None
    );
    assert_eq!(app.world().get::<Node>(map).unwrap().display, Display::Flex);
    assert_eq!(app.world().get::<Node>(map).unwrap().bottom,px(16));
    assert_eq!(
        app.world().get::<Node>(briefing).unwrap().display,
        Display::None
    );
    assert_eq!(
        app.world().get::<Node>(upgrade).unwrap().display,
        Display::Flex
    );
    let recipe = choices(app.world().resource::<Catalog>(), &app.world().resource::<Scene>().sim)[0];
    assert_eq!(app.world().get::<Text>(caption).unwrap().0, name(recipe));
    let details = recipe_details(app.world().resource::<Catalog>(), &app.world().resource::<Scene>().sim, recipe);
    assert!(details.contains("Масса") && details.contains("Энергия"),
        "live quotes stay in hover details, not recipe cells");
    let mass = app.world().resource::<Scene>().sim.production_quote(1, recipe)
        .map(|(mass, _, _)| economy_display::mass(mass)).expect("live recipe quote");
    assert!(details.contains(&format!("Масса {mass}")));
    let projected = spatial_catalog(app.world().resource::<Catalog>(),
        &app.world().resource::<Scene>().sim, app.world().resource::<Scene>().generation, 1)
        .expect("selected factory projection");
    assert_eq!(projected.recipes.first().map(|r| r.kind), Some(recipe));
    assert!(!projected.has_work);
    assert_eq!(app.world().get::<Node>(queue_row).unwrap().display, Display::None);
    assert_eq!(app.world().get::<Node>(queue_card).unwrap().display, Display::None);
    assert!(app.world().get::<Text>(queue_header).unwrap().0.contains("пусто"));
    click(&mut app, Action::Choose(0));
    assert_eq!(app.world().get::<Node>(queue_card).unwrap().display, Display::Flex);
    assert_eq!(app.world().get::<Text>(queue_number).unwrap().0, "1");
    assert_eq!(app.world().get::<Node>(queue_pip).unwrap().display, Display::Flex);
    assert!(app.world().get::<Text>(queue_header).unwrap().0.contains("1 / 1"));
    assert!(app
        .world()
        .get::<Text>(hint)
        .unwrap()
        .0
        .contains("добавлен"));
    assert!(app
        .world()
        .get::<Text>(heading)
        .unwrap()
        .0
        .contains("заказов: 1"));
    click(&mut app, Action::Cancel);
    assert!(app.world_mut().resource_mut::<Scene>().sim.battle_order(
        3, sim_core::blast::battle::Order::Hold, None,
    ));
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        for next in [RangeUnitKind::KilFactoryT2, RangeUnitKind::KilFactoryT3, RangeUnitKind::KilFactoryT4] {
            finish_timed_factory_upgrade(&mut scene.sim, next);
        }
    }
    app.update();
    assert_eq!(
        app.world().get::<Node>(upgrade).unwrap().display,
        Display::None
    );
    select(&mut app, 0);
    assert_eq!(app.world().get::<Node>(queue_row).unwrap().display, Display::None,
        "Greben crew queue is owned by the upper global island");
    assert_eq!(
        app.world().get::<Node>(upgrade).unwrap().display,
        Display::None,
        "storage cannot upgrade like a Kil"
    );
    let first_building = choices(
        app.world().resource::<Catalog>(),
        &app.world().resource::<Scene>().sim,
    )[0];
    assert!(app.world().get::<Text>(caption).unwrap().0.contains(name(first_building)),
        "Greben's engineer catalogue must show its selected building");
    select(&mut app, 2);
    let recipe = choices(app.world().resource::<Catalog>(), &app.world().resource::<Scene>().sim)[0];
    assert_eq!(app.world().get::<Text>(caption).unwrap().0, name(recipe),
        "recipe cells keep the name only; the real quote belongs in the tooltip");
    app.insert_resource(frontend::Menu::new(true));
    app.update();
    assert_eq!(
        app.world().get::<Node>(root).unwrap().display,
        Display::None
    );
    app.insert_resource(frontend::Menu::new(false));
    app.world_mut().resource_mut::<pilot::Pilot>().actor = Some(3);
    app.update();
    assert_eq!(
        app.world().get::<Node>(root).unwrap().display,
        Display::None
    );
    app.world_mut().resource_mut::<pilot::Pilot>().actor = None;
    select(&mut app, 3);
    assert_eq!(
        app.world().get::<Node>(deck).unwrap().display,
        Display::None
    );
    assert_eq!(app.world().get::<Node>(map).unwrap().display, Display::Flex);
    assert_eq!(
        app.world().get::<Node>(briefing).unwrap().display,
        Display::Flex,
        "HUD restores feedback after the catalogue closes"
    );
    app.world_mut().resource_mut::<panel::Panel>().sandbox = true;
    app.update();
    assert_eq!(app.world().resource::<Catalog>().per_page, 4);
    click(&mut app, Action::Page(100));
    let c = app.world().resource::<Catalog>();
    let real_recipes = choices(c, &app.world().resource::<Scene>().sim);
    assert_eq!(c.page, (real_recipes.len() - 1) / c.per_page,
        "large page increments stop on the last page of the actual recipe inventory");
    click(&mut app, Action::Tier(4));
    let c = app.world().resource::<Catalog>();
    assert_eq!(
        choices(c, &app.world().resource::<Scene>().sim),
        vec![RangeUnitKind::KilFactoryT4]
    );
    assert_eq!(c.page, 0);
}

#[test]
fn a_polygon_experiment_after_combat_restores_free_placement() {
    let mut app = app();
    let layout =
        RangeLayout::from_ron(include_str!("../../../../../../data/range-km.ron")).unwrap();
    let zone = layout
        .zones
        .iter()
        .position(|z| z.scenario.is_some())
        .unwrap();
    let id = app
        .world_mut()
        .spawn((panel::Action::Scenario(zone as u8), Interaction::Pressed))
        .id();
    app.update();
    app.world_mut().despawn(id);
    assert!(app.world().resource::<panel::Panel>().sandbox);
    assert!(app.world().resource::<Workshop>().pending.is_some());
}

#[test]
fn sandbox_spawn_preserves_damage_tick_matter_and_snapshot() {
    let mut layout =
        RangeLayout::from_ron(include_str!("../../../../../../data/range-cover.ron")).unwrap();
    layout.units.clear();
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [1, 1];
    layout.clear_max = [639, 639];
    let config = config(false, false);
    let mut sim = BlastRange::with_layout(config, layout).unwrap();
    for _ in 0..3 {
        sim.step();
    }
    let tick = sim.tick;
    let gun = sim.gun.motion.pos;
    let id = sim
        .place_sandbox_unit(RangeUnitKind::Tank, true, [500, 16, 500], 1)
        .unwrap();
    assert_eq!(id, 0);
    assert_eq!(sim.tick, tick);
    assert_eq!(sim.gun.motion.pos, gun);
    assert!(sim.matter.economy.ledger.balanced());
    let before = sim.state_hash();
    let restored = BlastRange::restore(config, &sim.snapshot()).unwrap();
    assert_eq!(restored.state_hash(), before);
    let rejected = sim.place_sandbox_unit(RangeUnitKind::Tank, true, [500, 16, 500], 1);
    assert!(rejected.is_err());
    assert_eq!(sim.state_hash(), before);
}
#[test]
fn higher_tier_sites_reserve_space_and_survive_save_and_completion() {
    let (mut sim, _) = fixture();
    sim.battle.phase = Phase::Setup;
    let at = [1840, 16, 900];
    assert!(sim.can_construct(RangeUnitKind::TurretT2, true, at));
    let site = sim
        .create_construction_site(RangeUnitKind::TurretT2, true, at)
        .unwrap();
    assert!(!sim.can_construct(RangeUnitKind::ArtilleryTurretT3, true, at));
    let copy = BlastRange::restore(sim.config, &sim.snapshot()).unwrap();
    assert_eq!(sim.state_hash(), copy.state_hash());
    let mut checked_partial = false;
    for _ in 0..1200 {
        sim.step();
        if let Some(u) = sim.units.iter().find(|u| u.kind == RangeUnitKind::TurretT2) {
            assert!(u.status.alive,"partial building: status {:?}, integrity {:?}, removed {}, core {:?}/{:?}, segments {:?}, pos {:?}",u.status,u.integrity,u.removed.len(),u.body.core,u.body.core_size,u.seg_gone,u.motion.pos);
            if !checked_partial {
                assert!(!u.status.weapon);
                let mut last = ("", 0, None);
                let restored =
                    BlastRange::restore_observed(sim.config, &sim.snapshot(), |a, b, c| {
                        last = (a, b, c)
                    })
                    .unwrap_or_else(|| panic!("decoder stopped at {last:?}"));
                assert_eq!(
                    sim.state_hash(),
                    restored.state_hash(),
                    "unfinished building round trip"
                );
                checked_partial = true;
            }
        }
        if sim.construction_sites()[site].complete {
            break;
        }
    }
    assert!(
        sim.construction_sites()[site].complete,
        "drones must complete the actual T2 building: {:?}, stock {:?}",
        sim.construction_sites()[site],
        sim.side_store(true)
    );
    assert!(sim.matter.economy.ledger.balanced());
    let restored = BlastRange::restore(sim.config, &sim.snapshot()).unwrap();
    assert_eq!(sim.state_hash(), restored.state_hash());
}


#[test]
fn actual_hover_and_capture_preview_share_the_real_quote_island() {
    let mut app=app();
    app.insert_resource(frontend::Menu::new(false)).init_resource::<Time>().init_resource::<UiScale>()
        .init_resource::<spatial_ui::State>().add_systems(PostUpdate,tooltip);
    app.world_mut().spawn(Window{resolution:(960,540).into(),..default()});
    {let mut c=app.world_mut().resource_mut::<Catalog>();c.context=Context::Factory(1);c.per_page=5;}
    app.world_mut().resource_mut::<Workshop>().controlled=Some(1);
    app.world_mut().spawn((Root,Node::default()));
    let card=app.world_mut().spawn((Slot(0),Interaction::Hovered)).id();
    let tip=app.world_mut().spawn((RecipeTooltip,Node::default())).id();
    let label=app.world_mut().spawn((RecipeTooltipText,Text::new(""))).id();
    for _ in 0..4 {app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(100));app.update();}
    assert_eq!(app.world().get::<Node>(tip).unwrap().display,Display::Flex);
    let hover=app.world().get::<Text>(label).unwrap().0.clone();
    let quote=app.world().resource::<Scene>().sim.production_quote(1,RangeUnitKind::Tank).unwrap();
    assert!(hover.contains(&economy_display::mass(quote.0))&&hover.contains("Пит."));
    let node=app.world().get::<Node>(tip).unwrap();
    let placement=(node.left,node.top,node.bottom,node.width);
    assert_eq!(placement,(px(16),Val::Auto,px(200),px(160)));
    *app.world_mut().get_mut::<Interaction>(card).unwrap()=Interaction::None;
    app.insert_resource(spatial_ui::State::default().with_tooltip_preview(Some(0)));
    app.update();
    assert_eq!(app.world().get::<Node>(tip).unwrap().display,Display::Flex);
    assert_eq!(app.world().get::<Text>(label).unwrap().0,hover);
    let node=app.world().get::<Node>(tip).unwrap();
    assert_eq!((node.left,node.top,node.bottom,node.width),placement);
}


#[test]
fn hidden_combat_context_hides_filters_and_deselection_preserves_projection_toggles() {
    let mut layout = RangeLayout::from_ron(include_str!("../../../../../../data/fixture-drive.ron")).unwrap();
    layout.size = [1600, 384, 1600];
    layout.battle = true;
    layout.idle_enemies = true;
    layout.course = None;
    layout.ready = None;
    layout.shields.clear();
    layout.unit_yaws.clear();
    layout.gun = [250, 24, 400];
    layout.target = [1250, 16, 1250];
    layout.clear_min = [1, 1];
    layout.clear_max = [1599, 1599];
    layout.blocks = vec![
        RangeBlock { min: [0; 3], max: [1600, 384, 1600], material: 0 },
        RangeBlock { min: [0; 3], max: [1600, 16, 1600], material: 1 },
    ];
    layout.units = vec![
        RangeUnitDef { kind: RangeUnitKind::KilFactoryT1, allied: true, at: [550, 16, 750], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::Tank, allied: true, at: [1150, 16, 650], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::Tank, allied: false, at: [1420, 16, 1420], weapon: None },
        RangeUnitDef { kind: RangeUnitKind::Spider, allied: true, at: [550, 16, 1100], weapon: None },
    ];
    layout.economy = Some(modding::range_economy::RangeEconomy::stock());
    let mut sim = BlastRange::with_layout(config(false, false), layout.clone()).unwrap();
    sim.set_physics_workers(1);
    sim.set_navigation_workers(1);
    sim.start_battle().unwrap();
    let edit = Workshop::new(Some(layout), false);
    let mut app = panel::hud::tests::app_with_scene(sim, edit);
    let mut fonts = Assets::<Font>::default();
    let mut images = Assets::<Image>::default();
    let mut gizmo_config = bevy::gizmos::config::GizmoConfigStore::default();
    gizmo_config.insert(bevy::gizmos::config::GizmoConfig::default(), bevy::gizmos::config::DefaultGizmoConfigGroup);
    let camera = app.world_mut().spawn(Camera2d).id();
    if app.world_mut().query_filtered::<Entity, With<CatalogPart>>().iter(app.world()).next().is_none() {
        build(&mut app.world_mut().commands(), camera, &TextFont::default());
    }
    if app.world_mut().query_filtered::<Entity, With<production_hud::Root>>().iter(app.world()).next().is_none() {
        production_hud::build(&mut app.world_mut().commands(), camera, &TextFont::default());
    }
    spatial_ui::build(&mut app.world_mut().commands(), camera, &TextFont::default(), &mut fonts, &mut images);
    let mut camera3d = Camera::default();
    camera3d.computed.target_info = Some(bevy::camera::RenderTargetInfo { physical_size: UVec2::new(960, 540), scale_factor: 1. });
    camera3d.computed.clip_from_view = Mat4::orthographic_rh(-2200., 2200., -1237.5, 1237.5, 0.1, 10000.);
    app.world_mut().spawn((workbench::EditorCamera, camera3d, Transform::from_xyz(1550., 1500., 1950.).looking_at(Vec3::new(550., 100., 550.), Vec3::Y)));
    app.init_resource::<Catalog>()
        .insert_resource(laboratory::Laboratory::new(false))
        .init_resource::<pilot::Pilot>()
        .init_resource::<ButtonInput<MouseButton>>()
        .insert_resource(frontend::Menu::new(false))
        .insert_resource(Templates::default())
        .insert_resource(images)
        .insert_resource(gizmo_config)
        .init_resource::<Time>()
        .init_resource::<UiScale>()
        .init_resource::<spatial_ui::State>()
        .init_resource::<production_hud::State>()
        .add_systems(Update, input.after(panel::actions))
        .add_systems(Update, spatial_ui::input.after(input))
        .add_systems(PostUpdate, (responsive, refresh, sync_visibility, production_hud::refresh, spatial_ui::update).chain().after(ui_snapshot::publish));
    app.world_mut().resource_mut::<panel::Panel>().sandbox = false;
    app.world_mut().spawn(Window { resolution: (960, 540).into(), ..default() });

    // Frame with no unit selected (projection == None), then select combat Tank (Context::Hidden).
    app.update();
    select(&mut app, 1);
    for _ in 0..4 {
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(100));
        app.update();
    }
    let mut parts = app.world_mut().query::<(&CatalogPart, &Node)>();
    assert!(
        parts.iter(app.world()).all(|(_, n)| n.display == Display::None),
        "combat unit without build/produce capability must hide both Filters and Row"
    );
    // FoldToggle and ToolsToggle must remain Display::Flex after projection == None frames!
    for required in [spatial_ui::Part::FoldToggle, spatial_ui::Part::ToolsToggle] {
        let d = app.world_mut().query::<(&spatial_ui::Part, &Node)>().iter(app.world())
            .find_map(|(p, n)| (*p == required).then_some(n.display)).unwrap();
        assert_eq!(d, Display::Flex, "projection toggle must survive projection == None frames");
    }
    let qroot = app.world_mut().query_filtered::<Entity, With<production_hud::Root>>().iter(app.world()).next().unwrap();
    assert_eq!(app.world().get::<Node>(qroot).unwrap().width, px(960. * 0.28));

    fn settle(app: &mut App) {
        for _ in 0..5 {app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(100));app.update();}
    }
    fn catalog_display(app: &mut App, filters: Display, row: Display) {
        let mut parts = app.world_mut().query::<(&CatalogPart, &Node)>();
        assert_eq!(parts.iter(app.world()).count(), 2);
        for (part, node) in parts.iter(app.world()) {
            assert_eq!(node.display, match part {CatalogPart::Filters=>filters,CatalogPart::Row=>row});
        }
    }
    fn part(app: &mut App, want: spatial_ui::Part) -> Entity {
        app.world_mut().query::<(Entity,&spatial_ui::Part)>().iter(app.world()).find_map(|(e,p)|(*p==want).then_some(e)).unwrap()
    }
    fn press(app: &mut App, want: spatial_ui::Part) {
        let e=part(app,want);assert_eq!(app.world().get::<Node>(e).unwrap().display,Display::Flex);
        *app.world_mut().get_mut::<Interaction>(e).unwrap()=Interaction::Pressed;app.update();
        *app.world_mut().get_mut::<Interaction>(e).unwrap()=Interaction::None;settle(app);
    }
    for (width,height,scale) in [(960,540,1.),(1920,1080,1.),(3840,2160,2.)] {
        app.world_mut().query::<&mut Window>().single_mut(app.world_mut()).unwrap().resolution=(width,height).into();
        app.world_mut().resource_mut::<UiScale>().0=scale;
        app.world_mut().query_filtered::<&mut Camera,With<workbench::EditorCamera>>().single_mut(app.world_mut()).unwrap()
            .computed.target_info=Some(bevy::camera::RenderTargetInfo{physical_size:UVec2::new(width,height),scale_factor:1.});
        for selected in [Some(1),None,Some(3),None] {
            {let mut edit=app.world_mut().resource_mut::<Workshop>();edit.controlled=selected;edit.selected_units=selected.into_iter().collect();}
            settle(&mut app);catalog_display(&mut app,Display::None,Display::None);
            for want in [spatial_ui::Part::FoldToggle,spatial_ui::Part::ToolsToggle] {
                let e=part(&mut app,want);assert_eq!(app.world().get::<Node>(e).unwrap().display,Display::Flex);
            }
        }
        select(&mut app,0);settle(&mut app);
        assert!(app.world().resource::<spatial_ui::State>().visible,"factory projection must actually open");
        let compact=height as f32/scale<780.;
        catalog_display(&mut app,if compact {Display::None}else{Display::Flex},if compact {Display::None}else{Display::Flex});
        press(&mut app,spatial_ui::Part::ToolsToggle);assert!(app.world().resource::<spatial_ui::State>().tools_open);
        press(&mut app,spatial_ui::Part::ToolsToggle);assert!(!app.world().resource::<spatial_ui::State>().tools_open);
        press(&mut app,spatial_ui::Part::CatalogToggle);
        assert!(app.world().resource::<spatial_ui::State>().catalog_open);
        catalog_display(&mut app,Display::Flex,if compact {Display::Flex}else{Display::None});
        let ticks:Vec<_>=app.world_mut().query_filtered::<Entity,With<CatalogPart>>().iter(app.world())
            .map(|e|(e,app.world().entity(e).get_ref::<Node>().unwrap().last_changed())).collect();
        settle(&mut app);
        for (e,tick) in ticks {assert_eq!(app.world().entity(e).get_ref::<Node>().unwrap().last_changed(),tick,"catalogue visibility must have one owner");}
        press(&mut app,spatial_ui::Part::CatalogToggle);
        press(&mut app,spatial_ui::Part::FoldToggle);
        let deck=part(&mut app,spatial_ui::Part::Deck);assert_eq!(app.world().get::<Node>(deck).unwrap().display,Display::None);
        press(&mut app,spatial_ui::Part::Restore);assert!(app.world().resource::<spatial_ui::State>().visible);
        app.insert_resource(frontend::Menu::new(true));settle(&mut app);catalog_display(&mut app,Display::None,Display::None);
        app.insert_resource(frontend::Menu::new(false));settle(&mut app);
        assert!(app.world().resource::<spatial_ui::State>().visible);
        press(&mut app,spatial_ui::Part::ToolsToggle);assert!(app.world().resource::<spatial_ui::State>().tools_open);
        press(&mut app,spatial_ui::Part::ToolsToggle);
    }
}
