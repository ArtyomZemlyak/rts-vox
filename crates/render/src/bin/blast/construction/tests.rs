use super::*;
use modding::range::{RangeBlock, RangeLayout, RangeUnitDef};

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
        (RangeUnitKind::Tank, true, [2600, 16, 2800]),
        (RangeUnitKind::Tank, false, [3600, 16, 2800]),
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
        store: 1 << 30,
        ..default()
    });
    let mut sim = BlastRange::with_layout(config(false, false), layout.clone()).unwrap();
    sim.set_physics_workers(1);
    sim.set_navigation_workers(1);
    sim.start_battle().unwrap();
    (sim, Workshop::new(Some(layout), false))
}
fn app() -> App {
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
#[test]
fn catalogue_modes_gate_engineer_factory_enemy_and_sandbox() {
    let mut app = app();
    for (id, expected) in [
        (3, Context::Hidden),
        (4, Context::Hidden),
        (2, Context::Engineer(2)),
        (1, Context::Factory(1)),
    ] {
        select(&mut app, id);
        assert_eq!(app.world().resource::<Catalog>().context, expected);
    }
    app.world_mut().resource_mut::<panel::Panel>().sandbox = true;
    select(&mut app, 4);
    assert_eq!(app.world().resource::<Catalog>().context, Context::Sandbox);
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
fn all_tiers_filter_real_recipes_and_factory_clicks_queue_and_refund() {
    let mut app = app();
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
        assert!(scene.sim.upgrade_kil(1));
        assert!(scene.sim.upgrade_kil(1));
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
    // The old first card is a tank. The storage's first recipe is a drone.
    app.world_mut().resource_mut::<Workshop>().controlled = Some(0);
    click(&mut app, Action::Choose(0));
    assert_eq!(
        app.world().resource::<Catalog>().context,
        Context::Factory(0)
    );
    assert!(app
        .world()
        .resource::<Scene>()
        .sim
        .production_queue(0)
        .is_none());
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
            } else if u.kind == RangeUnitKind::SkobaDrone {
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
        .add_systems(PostUpdate, (panel::hud::refresh, refresh).chain());
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
    let root = app.world_mut().spawn((Root, Node::default())).id();
    let deck = app
        .world_mut()
        .spawn((panel::hud::CommandDeck, Node::default()))
        .id();
    let map = app
        .world_mut()
        .spawn((panel::minimap::MiniRoot, Node::default()))
        .id();
    let upgrade = app
        .world_mut()
        .spawn((SandboxControl(false), Node::default()))
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
    assert_eq!(app.world().get::<Node>(map).unwrap().display, Display::None);
    assert_eq!(
        app.world().get::<Node>(briefing).unwrap().display,
        Display::None
    );
    assert_eq!(
        app.world().get::<Node>(upgrade).unwrap().display,
        Display::Flex
    );
    assert!(app.world().get::<Text>(caption).unwrap().0.contains("мат."));
    click(&mut app, Action::Choose(0));
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
        .contains("оплачено 0%"));
    for _ in 0..3 {
        assert!(app.world_mut().resource_mut::<Scene>().sim.upgrade_kil(1));
    }
    app.update();
    assert_eq!(
        app.world().get::<Node>(upgrade).unwrap().display,
        Display::None
    );
    select(&mut app, 0);
    assert_eq!(
        app.world().get::<Node>(upgrade).unwrap().display,
        Display::None,
        "storage cannot upgrade like a Kil"
    );
    assert!(app
        .world()
        .get::<Text>(caption)
        .unwrap()
        .0
        .contains("Скоба"));
    select(&mut app, 2);
    assert!(
        app.world().get::<Text>(caption).unwrap().0.contains("мат."),
        "buildings need their actual price too"
    );
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
        Display::Flex
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
    assert_eq!(app.world().resource::<Catalog>().page, 5);
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
