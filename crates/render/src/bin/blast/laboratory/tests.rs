use super::*;
use modding::laboratory::catalog::{self, SceneFile, WeaponAsset};

#[test]
fn scenario_every_stock_case_exposes_its_authored_sources_and_roundtrips() {
    use modding::laboratory::experiments::Input;
    let mut lab=Laboratory::new(false);
    for index in 0..lab.experiments.len() {
        experiments::select(&mut lab,index).unwrap();
        let plan=lab.constructor.experiment.as_ref().unwrap();
        for (i,input) in plan.inputs.iter().enumerate() {
            let source=lab.constructor.workspace.sources.iter().find(|s|s.input==Some(i));
            match input {
                Input::Fire=>assert!(source.is_some_and(|s|!s.detonate)),
                Input::Detonate{at}=>assert!(source.is_some_and(|s|s.detonate&&s.at==*at)),
                _=>assert!(source.is_none()),
            }
        }
        let layout=lab.constructor.layout(&lab.definition,lab.recipe).unwrap();
        scenario::validate(&lab,&layout).unwrap();
        let file=SceneFile{version:1,base:lab.constructor.base_layout(&lab.definition,lab.recipe).unwrap(),elements:lab.constructor.placed.clone(),weapon:lab.config(),strength:None,visual:0,experiment:lab.constructor.experiment.clone(),workspace:lab.constructor.workspace.clone()};
        let restored=SceneFile::from_ron(&file.to_ron().unwrap()).unwrap();
        assert_eq!(restored.experiment,file.experiment,"{}",lab.experiments[index].id);
        assert_eq!(restored.workspace,file.workspace,"{}",lab.experiments[index].id);
    }
}

#[test]
fn scenario_buried_charge_edits_change_the_real_detonation_and_survive_save() {
    let mut layout=Definition::stock().layout;
    layout.size=[128,128,128];layout.clear_min=[1,1];layout.clear_max=[127,127];
    layout.gun=[40,24,64];layout.gun_point=true;layout.target=[64,16,64];
    let mut sim=BlastRange::with_layout(config(false,false),layout.clone()).unwrap();
    sim.set_physics_workers(1);sim.set_navigation_workers(1);
    let mut app=app_with_base(panel::hud::tests::app_with_scene(sim,Workshop::new(Some(layout.clone()),false)));
    let index=app.world().resource::<Laboratory>().experiments.iter().position(|e|e.id=="buried-dome").unwrap();
    {
        let mut lab=app.world_mut().resource_mut::<Laboratory>();
        lab.definition.layout=layout.clone();
        let experiment=&mut lab.experiments[index];
        experiment.layout=layout;experiment.layout.laboratory=true;experiment.layout.target=[64,8,64];
        experiment.plan.inputs=vec![modding::laboratory::experiments::Input::Detonate{at:[64,8,64]}];
    }
    enter(&mut app);
    click(&mut app,LabAction::Experiment(index));
    {
        let lab=app.world().resource::<Laboratory>();
        let source=&lab.constructor.workspace.sources[0];
        assert_eq!(source.at,[64,8,64]);assert!(source.detonate);
        assert_eq!(source.input,Some(0));
    }
    click(&mut app,LabAction::SetNumber(10,70));
    click(&mut app,LabAction::SetNumber(11,6));
    click(&mut app,LabAction::SetNumber(12,68));
    click(&mut app,LabAction::SetNumber(0,500));
    {
        let lab=app.world().resource::<Laboratory>();
        let source=&lab.constructor.workspace.sources[0];
        assert_eq!(source.at,[70,6,68]);assert_eq!(source.power_milli,500);
        assert_eq!(lab.constructor.experiment.as_ref().unwrap().inputs[0],modding::laboratory::experiments::Input::Detonate{at:[70,6,68]});
        let file=SceneFile{version:1,base:lab.constructor.base_layout(&lab.definition,lab.recipe).unwrap(),elements:lab.constructor.placed.clone(),weapon:lab.config(),strength:None,visual:0,experiment:lab.constructor.experiment.clone(),workspace:lab.constructor.workspace.clone()};
        let restored=SceneFile::from_ron(&file.to_ron().unwrap()).unwrap();
        assert_eq!(restored.workspace.sources[0],*source);
    }
    click(&mut app,LabAction::Fire);
    app.world_mut().resource_scope(|world,mut lab:Mut<Laboratory>|scenario::advance(&mut lab,&mut world.resource_mut::<Scene>()));
    let scene=app.world().resource::<Scene>();
    assert_eq!(scene.sim.impact,Some([70,6,68]));
    assert_eq!(scene.sim.config.energy,app.world().resource::<Laboratory>().constructor.workspace.sources[0].resolved().energy);
}

#[test]
fn laboratory_preview_matches_atomic_placement_and_reuses_unchanged_validation() {
    let mut lab = Laboratory::new(false);
    lab.constructor.stand = false;
    let c = &mut lab.constructor;
    let tank = c.catalog.iter().position(|e| e.id == "tank").unwrap();
    c.action(
        LabAction::PickCatalog(tank),
        &lab.definition,
        &mut lab.recipe,
    )
    .unwrap();
    c.hover = Some([384, 16, 336]);
    c.preview_at([384, 16, 336], &lab.definition, lab.recipe);
    assert!(c.preview.as_ref().unwrap().issue.is_none());
    for _ in 0..20 {
        c.preview_at([384, 16, 336], &lab.definition, lab.recipe);
    }
    assert_eq!(
        c.preview_builds, 1,
        "stationary ghost must not rerun the validator"
    );
    let geometry = c.preview.as_ref().unwrap().geometry.units.clone();
    c.action(
        LabAction::PlaceAt([384, 16, 336]),
        &lab.definition,
        &mut lab.recipe,
    )
    .unwrap();
    assert_eq!(
        c.layout(&lab.definition, lab.recipe).unwrap().units,
        geometry
    );
    c.preview_at([384, 16, 336], &lab.definition, lab.recipe);
    assert!(
        c.preview.as_ref().unwrap().issue.is_some(),
        "overlapping object must turn red"
    );
    let before = c.placed.clone();
    assert!(c
        .action(
            LabAction::PlaceAt([384, 16, 336]),
            &lab.definition,
            &mut lab.recipe
        )
        .is_err());
    assert_eq!(c.placed, before);
    c.preview_at([0, 16, 0], &lab.definition, lab.recipe);
    assert!(c.preview.as_ref().unwrap().issue.is_some());
    let (_, invalid) = lab
        .ghost_pose(&workbench::ghost::Ghost::Unit(RangeUnitKind::Tank))
        .unwrap();
    assert!(invalid);
}

#[test]
fn laboratory_rotated_unit_ghost_core_pose_and_saved_prefab_agree() {
    let mut lab = Laboratory::new(false);
    lab.constructor.stand = false;
    let index = lab
        .constructor
        .catalog
        .iter()
        .position(|e| e.id == "tank")
        .unwrap();
    let c = &mut lab.constructor;
    c.action(
        LabAction::PickCatalog(index),
        &lab.definition,
        &mut lab.recipe,
    )
    .unwrap();
    c.action(LabAction::Turn(1), &lab.definition, &mut lab.recipe)
        .unwrap();
    c.hover = Some([384, 16, 336]);
    c.preview_at([384, 16, 336], &lab.definition, lab.recipe);
    assert!(c.preview.as_ref().unwrap().issue.is_none());
    let (ghost, invalid) = lab
        .ghost_pose(&workbench::ghost::Ghost::Unit(RangeUnitKind::Tank))
        .unwrap();
    assert!(!invalid);
    assert!(ghost
        .rotation
        .abs_diff_eq(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2), 0.0001));
    let c = &mut lab.constructor;
    c.action(
        LabAction::PlaceAt([384, 16, 336]),
        &lab.definition,
        &mut lab.recipe,
    )
    .unwrap();
    let layout = c.layout(&lab.definition, lab.recipe).unwrap();
    assert_eq!(layout.unit_yaw(0), 1);
    let unit_asset = catalog::combine(
        &c.placed,
        [384, 16, 336],
        "rotated-tank".into(),
        "Rotated tank".into(),
        &lab.definition.layout,
    )
    .unwrap();
    let unit_asset = catalog::Element::from_ron(&unit_asset.to_ron().unwrap()).unwrap();
    let parts = catalog::parts_of_layout(layout.clone()).unwrap();
    assert_eq!(
        catalog::compose(&lab.definition.layout, &parts).unwrap(),
        layout
    );
    assert_eq!(unit_asset.unit_yaws, [1]);
    let (sim, _) = lab.prepare().unwrap();
    assert_eq!(sim.units[0].motion.drive.head, fx::angle::FULL / 4);
    assert_eq!(sim.units[0].pose.x.z, fx::Fx::ONE);
    assert_eq!(
        BlastRange::restore(sim.config, &sim.snapshot())
            .unwrap()
            .snapshot(),
        sim.snapshot()
    );
}

#[test]
fn laboratory_move_and_duplicate_use_the_same_checked_scene_boundary() {
    let mut app = app();
    enter(&mut app);
    click(&mut app, LabAction::Clear);
    click(&mut app, LabAction::PickCatalog(0));
    click(&mut app, LabAction::PlaceAt([384, 16, 336]));
    let before = app.world().resource::<Scene>().sim.snapshot();
    click(&mut app, LabAction::Tool(1));
    click(&mut app, LabAction::MoveAt([0, 16, 0]));
    assert_eq!(app.world().resource::<Scene>().sim.snapshot(), before);
    click(&mut app, LabAction::MoveAt([416, 16, 336]));
    assert_eq!(
        app.world().resource::<Laboratory>().constructor.placed[0].at,
        [416, 16, 336]
    );
    click(&mut app, LabAction::Duplicate);
    click(&mut app, LabAction::PlaceAt([448, 16, 336]));
    let lab = app.world().resource::<Laboratory>();
    assert_eq!(lab.constructor.placed.len(), 2);
    assert_eq!(
        lab.constructor.placed[0].element,
        lab.constructor.placed[1].element
    );
    assert_eq!(lab.constructor.placed[0].at, [416, 16, 336]);
}

#[test]
fn laboratory_escape_cancels_tools_and_help_without_exiting_the_session() {
    let mut app = app();
    enter(&mut app);
    click(&mut app, LabAction::PickCatalog(0));
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Escape);
    app.update();
    assert!(app.world().resource::<Laboratory>().active());
    assert_eq!(
        app.world().resource::<Laboratory>().constructor.mode,
        constructor::Mode::Select
    );
    click(&mut app, LabAction::Help);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Escape);
    app.update();
    assert!(!app.world().resource::<Laboratory>().ui.help);
    assert!(app.world().resource::<Laboratory>().active());
}

pub(super) fn app() -> App {
    app_with_base(panel::hud::tests::app())
}
pub(super) fn app_with_base(mut app: App) -> App {
    app.insert_resource(Laboratory::new(false))
        .insert_resource(knobs::Overrides::new(false, true))
        .init_resource::<pilot::Pilot>()
        .insert_resource(profile::Profile::new(None, 1.0))
        .init_resource::<streaming::Streaming>()
        .init_resource::<moving::Visual>()
        .init_resource::<fire::Effects>()
        .init_resource::<ButtonInput<MouseButton>>()
        .add_systems(Update, (actions, measure).chain().after(panel::actions));
    app
}
pub(super) fn enter(app: &mut App) {
    let e = app
        .world_mut()
        .spawn((panel::Action::Laboratory, Interaction::Pressed))
        .id();
    app.update();
    app.world_mut().despawn(e);
    assert!(app.world().resource::<Laboratory>().active());
}
pub(super) fn click(app: &mut App, action: LabAction) {
    let e = app.world_mut().spawn((action, Interaction::Pressed)).id();
    app.update();
    app.world_mut().despawn(e);
}

#[test]
fn laboratory_sources_are_independent_and_numbers_apply_atomically() {
    let mut app = app();
    enter(&mut app);
    assert!(app.world().resource::<Scene>().sim.point_gun());
    let original = app
        .world()
        .resource::<Laboratory>()
        .constructor
        .workspace
        .sources[0]
        .clone();
    click(&mut app, LabAction::AddSource);
    click(&mut app, LabAction::SetNumber(0, 2500));
    let l = app.world().resource::<Laboratory>();
    assert_eq!(l.constructor.workspace.sources.len(), 2);
    assert_eq!(l.constructor.workspace.sources[0], original);
    assert_eq!(l.constructor.workspace.sources[1].power_milli, 2500);
    let speed = l.config().shell_speed;
    click(&mut app, LabAction::SetNumber(2, speed as i64 + 1));
    assert_eq!(
        app.world()
            .resource::<Laboratory>()
            .constructor
            .workspace
            .sources[1]
            .power_milli,
        2500,
        "speed edits retain independent multiplier"
    );
    let before = app.world().resource::<Scene>().sim.snapshot();
    let authored = app
        .world()
        .resource::<Laboratory>()
        .constructor
        .workspace
        .clone();
    click(&mut app, LabAction::SetNumber(2, -1));
    assert_eq!(app.world().resource::<Scene>().sim.snapshot(), before);
    assert_eq!(
        app.world().resource::<Laboratory>().constructor.workspace,
        authored
    );
    click(&mut app, LabAction::SetNumber(10, -100));
    assert_eq!(app.world().resource::<Scene>().sim.snapshot(), before);
    click(&mut app, LabAction::Source(0));
    assert_eq!(
        app.world().resource::<Scene>().sim.config.energy,
        original.resolved().energy
    );
    let before = app.world().resource::<Scene>().sim.snapshot();
    click(&mut app, LabAction::EditNumber(0));
    for key in [KeyCode::Digit3, KeyCode::Period, KeyCode::Digit5] {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(key);
        app.update();
    }
    assert_eq!(
        app.world().resource::<Scene>().sim.snapshot(),
        before,
        "typing cannot tick or reset SIM"
    );
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Enter);
    app.update();
    assert_eq!(
        app.world()
            .resource::<Laboratory>()
            .constructor
            .workspace
            .sources[0]
            .power_milli,
        3500
    );
}

#[test]
fn laboratory_event_presets_configure_the_shared_weapon_without_replacing_geometry() {
    let mut app = app();
    enter(&mut app);
    let generation = app.world().resource::<Scene>().generation;
    let count = app.world().resource::<Laboratory>().events.len();
    for i in 0..count {
        click(&mut app, LabAction::PickEvent(i));
        click(
            &mut app,
            LabAction::PlaceEvent(i, [400, 40, 320 + i as i32 * 8]),
        );
        let lab = app.world().resource::<Laboratory>();
        let source = sources::active(lab).unwrap();
        assert_eq!(source.effect, Some(lab.events[i].effect));
        assert_eq!(source.detonate, lab.events[i].detonate);
        let mut expected = source.resolved();
        expected.unit_ablation_resistance = app
            .world()
            .resource::<Scene>()
            .sim
            .layout
            .as_ref()
            .unwrap()
            .unit_resistance;
        assert_eq!(app.world().resource::<Scene>().sim.config, expected);
        assert_eq!(app.world().resource::<Scene>().generation, generation);
    }
}

#[test]
fn laboratory_placed_charge_is_authored_saved_and_runs_the_real_blast_at_its_marker() {
    let mut app = app();
    enter(&mut app);
    click(&mut app, LabAction::Clear);
    let slab = app
        .world()
        .resource::<Laboratory>()
        .constructor
        .catalog
        .iter()
        .position(|e| e.id == "collapse-tower")
        .unwrap();
    click(&mut app, LabAction::PickCatalog(slab));
    click(&mut app, LabAction::PlaceAt([384, 16, 336]));
    let before = app.world().resource::<Scene>().sim.snapshot();
    let generation = app.world().resource::<Scene>().generation;
    click(&mut app, LabAction::PickEvent(4));
    assert_eq!(
        app.world().resource::<Scene>().sim.snapshot(),
        before,
        "selecting a card only arms placement"
    );
    click(&mut app, LabAction::PlaceEvent(4, [-1, 24, 336]));
    assert_eq!(
        app.world().resource::<Scene>().sim.snapshot(),
        before,
        "invalid placement is atomic"
    );
    click(&mut app, LabAction::PlaceEvent(4, [384, 28, 336]));
    assert_eq!(
        app.world().resource::<Scene>().generation,
        generation,
        "adding a charge cannot reset the current world"
    );
    let l = app.world().resource::<Laboratory>();
    assert!(sources::active(l).unwrap().detonate);
    let workspace = l.constructor.workspace.clone();
    let file = SceneFile {
        version: 1,
        base: l.constructor.base_layout(&l.definition, l.recipe).unwrap(),
        elements: l.constructor.placed.clone(),
        weapon: l.config(),
        strength: None,
        visual: sources::active(l).unwrap().visual,
        experiment: None,
        workspace: workspace.clone(),
    };
    assert_eq!(
        SceneFile::from_ron(&file.to_ron().unwrap())
            .unwrap()
            .workspace,
        workspace
    );
    click(&mut app, LabAction::Fire);
    let scene = app.world().resource::<Scene>();
    assert_eq!(scene.sim.impact, Some([384, 28, 336]));
    assert!(scene.sim.shell.is_none());
    let mut sim = BlastRange::restore(scene.sim.config, &scene.sim.snapshot()).unwrap();
    sim.set_physics_workers(1);
    sim.set_navigation_workers(1);
    let mut falling = false;
    for _ in 0..100 {
        sim.step();
        falling |= sim.bodies.list.iter().any(|b| b.fallen > 0);
    }
    assert!(sim.removed > 0);
    assert!(
        falling || sim.collapsed_cells > 0,
        "removing an actual support must detach its construction"
    );
    assert!(sim.matter.economy.ledger.balanced());
}

#[test]
fn laboratory_scenarios_return_to_the_authored_assembly_and_its_sources() {
    let mut app = app();
    enter(&mut app);
    click(&mut app, LabAction::Clear);
    click(&mut app, LabAction::PickCatalog(0));
    click(&mut app, LabAction::PlaceAt([384, 16, 336]));
    click(&mut app, LabAction::PlaceEvent(0, [400, 20, 336]));
    let lab = app.world().resource::<Laboratory>();
    let items = lab.constructor.placed.clone();
    let workspace = lab.constructor.workspace.clone();
    let layout = lab.constructor.layout(&lab.definition, lab.recipe).unwrap();
    for id in ["fall", "collapse"] {
        let index = app
            .world()
            .resource::<Laboratory>()
            .experiments
            .iter()
            .position(|e| e.id == id)
            .unwrap();
        click(&mut app, LabAction::Experiment(index));
    }
    click(&mut app, LabAction::ReturnConstruction);
    let lab = app.world().resource::<Laboratory>();
    assert_eq!(lab.constructor.placed, items);
    assert_eq!(lab.constructor.workspace, workspace);
    assert_eq!(
        lab.constructor.layout(&lab.definition, lab.recipe).unwrap(),
        layout
    );
    assert!(!lab.constructor.can_return());
}

#[test]
fn laboratory_number_errors_keep_the_field_open_for_correction_or_cancellation() {
    let mut app = app();
    enter(&mut app);
    let before = app.world().resource::<Scene>().sim.snapshot();
    click(&mut app, LabAction::EditNumber(20));
    app.world_mut()
        .resource_mut::<Laboratory>()
        .ui
        .editing
        .as_mut()
        .unwrap()
        .1 = "1001".into();
    click(&mut app, LabAction::ApplyNumber);
    let lab = app.world().resource::<Laboratory>();
    assert_eq!(lab.ui.editing.as_ref().unwrap().1, "1001");
    assert!(lab.ui.edit_error.as_ref().unwrap().contains("16"));
    assert_eq!(app.world().resource::<Scene>().sim.snapshot(), before);
    // An integer field must not silently round a fractional value.
    app.world_mut()
        .resource_mut::<Laboratory>()
        .ui
        .editing
        .as_mut()
        .unwrap()
        .1 = "1024.5".into();
    click(&mut app, LabAction::ApplyNumber);
    assert!(app
        .world()
        .resource::<Laboratory>()
        .ui
        .edit_error
        .as_ref()
        .unwrap()
        .contains("целое"));
    click(&mut app, LabAction::CancelNumber);
    assert!(app.world().resource::<Laboratory>().ui.editing.is_none());
    assert!(app.world().resource::<Laboratory>().ui.edit_error.is_none());
    assert_eq!(app.world().resource::<Scene>().sim.snapshot(), before);
    click(&mut app, LabAction::EditNumber(0));
    app.world_mut()
        .resource_mut::<Laboratory>()
        .ui
        .editing
        .as_mut()
        .unwrap()
        .1 = "11111111111111111111".into();
    click(&mut app, LabAction::ApplyNumber);
    assert!(app.world().resource::<Laboratory>().ui.editing.is_some());
    {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::KeyA);
    }
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Digit2);
    app.update();
    assert_eq!(
        app.world()
            .resource::<Laboratory>()
            .ui
            .editing
            .as_ref()
            .unwrap()
            .1,
        "2"
    );
    click(&mut app, LabAction::ApplyNumber);
    let lab = app.world().resource::<Laboratory>();
    assert!(lab.ui.editing.is_none());
    assert!(lab.ui.edit_error.is_none());
    assert_eq!(lab.constructor.workspace.sources[0].power_milli, 2000);
}

#[test]
fn laboratory_resizes_preserving_authored_objects_and_rejects_clipping() {
    let mut app = app();
    enter(&mut app);
    click(&mut app, LabAction::Clear);
    click(&mut app, LabAction::PickCatalog(0));
    click(&mut app, LabAction::PlaceAt([384, 16, 336]));
    let placed = app
        .world()
        .resource::<Laboratory>()
        .constructor
        .placed
        .clone();
    click(&mut app, LabAction::SetNumber(20, 1024));
    assert_eq!(app.world().resource::<Scene>().sim.size()[0], 1024);
    assert_eq!(
        app.world().resource::<Laboratory>().constructor.placed,
        placed
    );
    let before = app.world().resource::<Scene>().sim.snapshot();
    click(&mut app, LabAction::SetNumber(20, 256));
    assert_eq!(app.world().resource::<Scene>().sim.snapshot(), before);
    assert_eq!(
        app.world().resource::<Laboratory>().constructor.placed,
        placed
    );
}

#[test]
fn laboratory_source_binding_uses_real_unit_muzzle_and_survives_scene_roundtrip() {
    let mut app = app();
    enter(&mut app);
    click(&mut app, LabAction::Clear);
    let i = app
        .world()
        .resource::<Laboratory>()
        .constructor
        .catalog
        .iter()
        .position(|e| e.id == "tank")
        .unwrap();
    click(&mut app, LabAction::PickCatalog(i));
    click(&mut app, LabAction::PlaceAt([384, 16, 336]));
    click(&mut app, LabAction::Tool(0));
    click(&mut app, LabAction::BindSource(true));
    let scene = app.world().resource::<Scene>();
    assert_eq!(
        scene.sim.muzzle(),
        scene.sim.battle_muzzles(0)[0].map(|v| fx::Fx::from_int(v.to_int_trunc()))
    );
    let l = app.world().resource::<Laboratory>();
    let file = SceneFile {
        version: 1,
        base: l.constructor.base_layout(&l.definition, l.recipe).unwrap(),
        elements: l.constructor.placed.clone(),
        weapon: l.config(),
        strength: None,
        visual: 0,
        experiment: None,
        workspace: l.constructor.workspace.clone(),
    };
    let restored = SceneFile::from_ron(&file.to_ron().unwrap()).unwrap();
    assert_eq!(restored.workspace, file.workspace);
    click(&mut app, LabAction::BindSource(false));
    assert!(app
        .world()
        .resource::<Laboratory>()
        .constructor
        .workspace
        .sources[0]
        .binding
        .is_none());
}

#[test]
fn laboratory_disassembly_keeps_enemy_spawn_heading() {
    let mut d = Definition::stock();
    d.layout.battle = true;
    let mut c = constructor::Constructor::new();
    c.stand = false;
    c.placed.push(catalog::Placed {
        element: c.catalog.iter().find(|e| e.id == "tank").unwrap().clone(),
        at: [384, 16, 336],
        turns: 0,
    });
    c.selected = Some(0);
    let mut recipe = Recipe::default();
    let before = c.layout(&d, recipe).unwrap();
    assert_eq!(before.unit_yaw(0), 2);
    c.action(LabAction::Split, &d, &mut recipe).unwrap();
    assert_eq!(c.layout(&d, recipe).unwrap(), before);
}

#[test]
fn suspended_battle_and_parameters_return_exactly_after_a_live_material_test() {
    let mut app = app();
    let original = app.world().resource::<Scene>().sim.snapshot();
    let energy = app.world().resource::<Scene>().energy;
    let layout = app.world().resource::<Workshop>().layout.clone();
    enter(&mut app);
    click(&mut app, LabAction::Material(2)); // wood palette
    click(&mut app, LabAction::ApplyMaterial);
    click(&mut app, LabAction::Effect(1));
    click(&mut app, LabAction::Fire);
    let fired = app.world().resource::<Scene>().sim.snapshot();
    {
        let mut s = app.world_mut().resource_mut::<Scene>();
        s.sim.set_physics_workers(1);
        for _ in 0..80 {
            s.sim.step();
        }
        assert!(
            s.sim.removed > 0,
            "the real kinetic shot must damage the wood"
        );
    }
    click(&mut app, LabAction::Repeat);
    assert_eq!(
        app.world().resource::<Scene>().sim.snapshot(),
        fired,
        "repeat must restore geometry, heat, queues and charge the same shot"
    );
    click(&mut app, LabAction::Back);
    let scene = app.world().resource::<Scene>();
    assert_eq!(scene.sim.snapshot(), original);
    assert_eq!(scene.energy, energy);
    assert_eq!(app.world().resource::<Workshop>().layout, layout);
    assert!(!app.world().resource::<Laboratory>().active());
}

#[test]
fn constructor_builds_a_reusable_assembly_and_restores_the_previous_workspace() {
    let mut app = app();
    enter(&mut app);
    {
        let mut view = app.world_mut().resource_mut::<Workshop>();
        view.pan = Vec2::new(12.0, 16.0);
        view.zoom = 0.8;
        view.orbit = 0.4;
    }
    let previous = app.world().resource::<Scene>().sim.snapshot();
    click(&mut app, LabAction::Create);
    click(&mut app, LabAction::AddPart);
    click(&mut app, LabAction::PlaceAt([384, 16, 336]));
    click(&mut app, LabAction::PlaceAt([384, 32, 336]));
    assert_eq!(
        app.world().resource::<Workshop>().pan,
        Vec2::new(12.0, 16.0)
    );
    assert_eq!(app.world().resource::<Workshop>().zoom, 0.8);
    assert_eq!(app.world().resource::<Workshop>().orbit, 0.4);
    {
        let lab = app.world().resource::<Laboratory>();
        assert_eq!(lab.constructor.placed.len(), 2);
        let element = catalog::combine(
            &lab.constructor.placed,
            [384, 16, 336],
            "test-assembly".into(),
            "Two blocks".into(),
            &lab.definition.layout,
        )
        .unwrap();
        let roundtrip = catalog::Element::from_ron(&element.to_ron().unwrap()).unwrap();
        assert_eq!(element, roundtrip);
        let item = catalog::Placed {
            element: roundtrip,
            at: [448, 16, 336],
            turns: 1,
        };
        let layout = catalog::compose(&lab.definition.layout, &[item]).unwrap();
        let sim = BlastRange::with_layout(config(false, false), layout).unwrap();
        assert_eq!(sim_core::blast::material(&sim.world, [448, 40, 336]), 3);
    }
    click(&mut app, LabAction::CancelCreate);
    assert_eq!(app.world().resource::<Scene>().sim.snapshot(), previous);
    assert!(app
        .world()
        .resource::<Laboratory>()
        .constructor
        .placed
        .is_empty());
}

#[test]
fn individual_parts_can_change_material_and_size_without_mutating_the_library() {
    let d = Definition::stock();
    let mut r = Recipe::default();
    let mut c = constructor::Constructor::new();
    c.stand = false;
    let stock = c.catalog[0].clone();
    c.action(LabAction::PlaceAt([384, 16, 336]), &d, &mut r)
        .unwrap();
    r.material = 4; // wood, explicit material ID5
    c.action(LabAction::ApplyMaterial, &d, &mut r).unwrap();
    c.part_size = [8, 24, 12];
    c.action(LabAction::ApplySize, &d, &mut r).unwrap();
    let b = &c.placed[0].element.blocks[0];
    assert_eq!(b.material, 5);
    assert_eq!(
        std::array::from_fn::<_, 3, _>(|a| b.max[a] - b.min[a]),
        [8, 24, 12]
    );
    assert_eq!(c.catalog[0], stock);
}

#[test]
fn scene_and_weapon_files_preserve_resolved_geometry_material_ids_and_weapon_parameters() {
    let d = Definition::stock();
    let r = Recipe::default();
    let (base, _) = d.fixture(r).unwrap();
    let element = catalog::stock()
        .into_iter()
        .find(|e| e.id == "tree")
        .unwrap();
    let file = SceneFile {
        workspace: Default::default(),
        experiment: Some(modding::laboratory::experiments::stock()[0].plan.clone()),
        version: 1,
        base,
        elements: vec![catalog::Placed {
            element,
            at: [460, 16, 400],
            turns: 0,
        }],
        weapon: weapon_config(false, false, 2000, Delivery::Kinetic),
        strength: Some((3, 1000)),
        visual: 0,
    };
    let text = file.to_ron().unwrap();
    let restored = SceneFile::from_ron(&text).unwrap();
    assert_eq!(restored.weapon, file.weapon);
    assert_eq!(restored.elements, file.elements);
    assert_eq!(restored.base, file.base);
    assert_eq!(restored.strength, file.strength);
    assert_eq!(restored.experiment, file.experiment);
    let a = catalog::compose(&file.base, &file.elements).unwrap();
    let b = catalog::compose(&restored.base, &restored.elements).unwrap();
    assert_eq!(
        BlastRange::with_layout(file.weapon, a).unwrap().snapshot(),
        BlastRange::with_layout(restored.weapon, b)
            .unwrap()
            .snapshot()
    );
    let w = WeaponAsset {
        version: 1,
        name: "Custom kinetic".into(),
        config: file.weapon,
        visual: 0,
    };
    assert_eq!(
        WeaponAsset::from_ron(&w.to_ron().unwrap()).unwrap().config,
        w.config
    );
    let mut bad = file;
    bad.version = 99;
    assert!(bad.to_ron().is_err());
}

#[test]
fn invalid_placement_is_atomic_and_observation_deadline_does_not_depend_on_frame_batching() {
    let mut app = app();
    enter(&mut app);
    click(&mut app, LabAction::Place);
    let before = app.world().resource::<Scene>().sim.snapshot();
    click(&mut app, LabAction::PlaceAt([0, 16, 0]));
    assert!(app
        .world()
        .resource::<Laboratory>()
        .constructor
        .placed
        .is_empty());
    assert_eq!(app.world().resource::<Scene>().sim.snapshot(), before);
    click(&mut app, LabAction::Fire);
    {
        let lab = app.world().resource::<Laboratory>();
        let end = lab.deadline.unwrap();
        assert_eq!(lab.limit_steps(end - 3, 8), 3);
        assert_eq!(lab.limit_steps(end - 3, 1), 1);
    }
    click(&mut app, LabAction::Keep);
    {
        let mut lab = app.world_mut().resource_mut::<Laboratory>();
        lab.constructor.revision += 1;
        assert!(
            !comparison(&lab).contains("B − A:"),
            "a changed construction cannot masquerade as a controlled material comparison"
        );
    }
}

#[test]
fn all_laboratory_experiments_reuse_core_inputs_and_the_same_physics_step() {
    let mut lab = Laboratory::new(false);
    for index in 0..lab.experiments.len() {
        experiments::select(&mut lab, index).unwrap();
        let mut expected=lab.experiments[index].layout.clone();
        expected.laboratory=true; // Lab authoring permission is independent of geometry/physics.
        if let Some(source)=lab.constructor.workspace.sources.first() {
            expected.gun_point=true;expected.gun=source.at;expected.target=source.target;
        }
        assert_eq!(
            lab.constructor.layout(&lab.definition, lab.recipe).unwrap(),
            expected,
            "editable parts must preserve the authored case"
        );
        let (mut sim, _) = lab
            .prepare()
            .unwrap_or_else(|e| panic!("{}: {e}", lab.experiments[index].id));
        sim.set_physics_workers(1);
        sim.set_navigation_workers(1);
        let mut direct = BlastRange::restore(sim.config, &sim.snapshot()).unwrap();
        direct.set_physics_workers(1);
        direct.set_navigation_workers(1);
        let plan = lab.constructor.experiment.as_ref().unwrap();
        experiments::launch(plan, &mut sim)
            .unwrap_or_else(|e| panic!("{}: {e}", lab.experiments[index].id));
        for command in experiments::commands(plan) {
            assert!(direct.execute_command(command).admitted());
        }
        sim.step();
        direct.step();
        assert_eq!(
            sim.snapshot(),
            direct.snapshot(),
            "{} must run the common engine exactly",
            lab.experiments[index].id
        );
    }
}

#[test]
fn laboratory_cases_really_fall_dig_heat_and_absorb_shots() {
    let mut lab = Laboratory::new(false);
    for id in ["fall", "mining", "melt", "shield", "canister"] {
        let index = lab.experiments.iter().position(|e| e.id == id).unwrap();
        experiments::select(&mut lab, index).unwrap();
        let (mut sim, _) = lab.prepare().unwrap();
        sim.set_physics_workers(1);
        sim.set_navigation_workers(1);
        let old_y = sim.units.first().map(|u| u.motion.pos[1]);
        experiments::launch(lab.constructor.experiment.as_ref().unwrap(), &mut sim).unwrap();
        let mut hot = 0;
        for _ in 0..160 {
            sim.step();
            hot = hot.max(sim.heat.hot() + sim.thermals.hot());
        }
        match id {
            "fall" => assert!(
                sim.units[0].motion.pos[1] < old_y.unwrap(),
                "gravity must move the body"
            ),
            "mining" => assert!(
                sim.mining().unwrap().cells > 0,
                "dig must remove and credit material"
            ),
            "melt" => assert!(
                hot > 0 && sim.heat.melted_total > 0,
                "laser must heat and melt actual steel"
            ),
            "shield" => assert!(
                sim.shields.iter().any(|s| s.hits > 0),
                "the shot must encounter the dome"
            ),
            "canister" => assert!(
                sim.battle.actors[0].fired > 0,
                "the real T2 weapon must fire"
            ),
            _ => unreachable!(),
        }
        assert!(sim.matter.economy.ledger.balanced(), "{id}");
    }
}

#[test]
fn laboratory_surface_snap_touches_top_and_side_without_overlap() {
    let lo=Vec3::new(-2.0,0.0,-2.0);let hi=Vec3::new(2.0,4.0,2.0);
    let top=Ray3d::new(Vec3::new(10.5,30.0,10.5),Dir3::NEG_Y);
    let at=tools::surface_anchor(top,[10,20,10],lo,hi,1,0);
    assert_eq!(at[1],21);
    let side=Ray3d::new(Vec3::new(40.0,22.5,10.5),Dir3::NEG_X);
    let at=tools::surface_anchor(side,[20,22,10],lo,hi,1,0);
    assert_eq!(at[0]+lo.x as i32,21);
}
#[test]
fn laboratory_overlap_toggle_changes_preview_and_portable_scene_policy() {
    let mut lab=Laboratory::new(false);lab.constructor.stand=false;
    let element=lab.constructor.catalog.iter().find(|e|e.id=="glass-block").unwrap().clone();
    lab.constructor.placed=vec![catalog::Placed{element:element.clone(),at:[384,16,336],turns:0}];
    lab.constructor.choice=lab.constructor.catalog.iter().position(|e|e.id==element.id).unwrap();
    lab.constructor.mode=constructor::Mode::Place;
    lab.constructor.preview_at([384,16,336],&lab.definition,lab.recipe);
    assert!(lab.constructor.preview.as_ref().unwrap().issue.is_some());
    lab.constructor.workspace.allow_overlap=true;lab.constructor.revision+=1;
    lab.constructor.preview_at([384,16,336],&lab.definition,lab.recipe);
    assert!(lab.constructor.preview.as_ref().unwrap().issue.is_none());
    lab.constructor.workspace.surface_snap=Some(false);
    let file=SceneFile{version:1,base:lab.constructor.base_layout(&lab.definition,lab.recipe).unwrap(),elements:lab.constructor.placed.clone(),weapon:lab.config(),strength:None,visual:0,experiment:None,workspace:lab.constructor.workspace.clone()};
    let restored=SceneFile::from_ron(&file.to_ron().unwrap()).unwrap().workspace;
    assert!(restored.allow_overlap);assert_eq!(restored.surface_snap,Some(false));
    let mut base=lab.definition.layout.clone();
    base.blocks.clear();base.units.clear();base.shields.clear();base.unit_yaws.clear();
    lab.constructor.placed[0].append(&mut base).unwrap();
    lab.constructor.base=Some(base);lab.constructor.placed.clear();
    lab.constructor.workspace.allow_overlap=false;lab.constructor.revision+=1;
    lab.constructor.preview_at([384,16,336],&lab.definition,lab.recipe);
    assert!(lab.constructor.preview.as_ref().unwrap().issue.is_some(),"base fixture must also prohibit overlap");
    lab.constructor.workspace.allow_overlap=true;lab.constructor.revision+=1;
    lab.constructor.preview_at([384,16,336],&lab.definition,lab.recipe);
    assert!(lab.constructor.preview.as_ref().unwrap().issue.is_none());
}
