use super::super::ui_snapshot::UiSnapshot;
use super::*;

#[cfg(test)]
fn command_app() -> App {
    let mut app = panel::hud::tests::app();
    app.insert_resource(Overrides::new(true, true))
        .init_resource::<GlobalAmbientLight>()
        .add_systems(Update, actions.after(panel::actions));
    app.update();
    app
}

#[cfg(test)]
fn click_command(app: &mut App, action: Action) {
    let e = app.world_mut().spawn((action, Interaction::Pressed)).id();
    app.update();
    app.world_mut().despawn(e);
}

#[cfg(test)]
#[test]
fn weapon_knobs_send_all_thirteen_rows_through_commands_and_reset() {
    let mut app = command_app();
    // Select the same stock preset represented by Scene's UI state.
    click_command(&mut app, Action::Weapon(1));
    let initial = app.world().resource::<Scene>().sim.config;
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .start_battle_command_capture(32)
        .unwrap();
    for slot in 0..WEAPON.len() {
        click_command(&mut app, Action::Knob(slot as u8, true));
    }
    let trace = app
        .world_mut()
        .resource_mut::<Scene>()
        .sim
        .finish_battle_command_capture()
        .unwrap();
    assert!(trace.complete());
    assert_eq!(trace.records.len(), WEAPON.len());
    for record in trace.records {
        let Command::ConfigureWeapon { config } = record.command else {
            panic!("wrong command")
        };
        assert!(config.validate().is_ok());
        assert!(record.outcome.accepted());
    }
    assert_ne!(app.world().resource::<Scene>().sim.config, initial);
    click_command(&mut app, Action::ResetKnobs);
    assert_eq!(app.world().resource::<Scene>().sim.config, initial);
}

#[cfg(test)]
#[test]
fn weapon_typed_bounds_and_paused_rebuild_remain_restorable() {
    let mut app = command_app();
    for (slot, expected) in [
        (1, 1_000_000_000.0),
        (6, 10_000.0),
        (8, 32.0),
        (11, WAVE_WORK_MAX as f64),
    ] {
        app.world_mut().resource_mut::<Overrides>().editing = Some((slot, "9999999999".into()));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Enter);
        app.world_mut()
            .resource_mut::<Scene>()
            .sim
            .start_battle_command_capture(8)
            .unwrap();
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset(KeyCode::Enter);
        let mut scene = app.world_mut().resource_mut::<Scene>();
        let trace = scene.sim.finish_battle_command_capture().unwrap();
        assert_eq!(trace.records.len(), 1);
        assert!(trace.records[0].outcome.accepted());
        let field = WEAPON[slot].key.strip_prefix("weapon.").unwrap();
        assert_eq!(get_weapon(&scene.sim.config, field), Some(expected));
        let bytes = scene.sim.snapshot();
        assert_eq!(
            bytes,
            BlastRange::restore(config(false, false), &bytes)
                .unwrap()
                .snapshot()
        );
    }
    let rebuilt_config = effective_config(
        true,
        false,
        1000,
        Delivery::Blast,
        app.world().resource::<Overrides>(),
    );
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        scene.sim =
            BlastRange::with_layout(rebuilt_config, scene.sim.layout.clone().unwrap()).unwrap();
        scene.generation += 1;
        scene.sim.start_battle_command_capture(8).unwrap();
    }
    app.update();
    app.update();
    let mut scene = app.world_mut().resource_mut::<Scene>();
    assert_eq!(scene.sim.tick, 0);
    assert_eq!(scene.sim.config.wave_work_per_tick, WAVE_WORK_MAX);
    assert!(scene.sim.config.validate().is_ok());
    let trace = scene.sim.finish_battle_command_capture().unwrap();
    assert_eq!(trace.records.len(), 0); // Rebuild already includes effective weapon settings.
}

#[cfg(test)]
#[test]
fn wave_work_plus_preserves_and_increases_each_stock_preset() {
    for weapon in 0..7 {
        let mut app = command_app();
        click_command(&mut app, Action::Weapon(weapon));
        let mut expected = app.world().resource::<Scene>().sim.config;
        expected.wave_work_per_tick =
            (expected.wave_work_per_tick + WEAPON[11].step as usize).min(WAVE_WORK_MAX);
        click_command(&mut app, Action::Knob(11, true));
        let actual = app.world().resource::<Scene>().sim.config;
        assert_eq!(
            actual, expected,
            "weapon {weapon}: '+' changed the preset instead of increasing work"
        );
        assert!(actual.validate().is_ok());
    }
}

#[cfg(test)]
#[test]
fn weapon_presets_and_powers_are_valid_commands_and_keep_overrides() {
    let mut app = command_app();
    click_command(&mut app, Action::Knob(0, true));
    let energy = app.world().resource::<Scene>().sim.config.energy;
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .start_battle_command_capture(64)
        .unwrap();
    for weapon in 0..7 {
        click_command(&mut app, Action::Weapon(weapon));
        for power in [500, 1000, 2000, 4000] {
            click_command(&mut app, Action::Power(power));
            let scene = app.world().resource::<Scene>();
            assert_eq!(scene.sim.config.energy, energy);
            assert!(scene.sim.config.validate().is_ok());
        }
    }
    let trace = app
        .world_mut()
        .resource_mut::<Scene>()
        .sim
        .finish_battle_command_capture()
        .unwrap();
    assert_eq!(trace.records.len(), 35);
    assert!(trace
        .records
        .iter()
        .all(|r| matches!(r.command, Command::ConfigureWeapon { .. }) && r.outcome.accepted()));
}

#[cfg(test)]
#[test]
fn material_buttons_send_all_strength_and_melt_commands_and_reset_preserves_density() {
    let mut app = command_app();
    let stock: Vec<_> = app
        .world()
        .resource::<Scene>()
        .sim
        .matter
        .economy
        .mats
        .definitions()
        .copied()
        .collect();
    app.world_mut().resource_mut::<Overrides>().group = 5;
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .start_battle_command_capture(32)
        .unwrap();
    for slot in 0..10 {
        click_command(&mut app, Action::Knob(slot, true));
    }
    let trace = app
        .world_mut()
        .resource_mut::<Scene>()
        .sim
        .finish_battle_command_capture()
        .unwrap();
    assert!(trace.complete());
    assert_eq!(trace.records.len(), 10);
    for (i, record) in trace.records.iter().enumerate() {
        let (material, field) = if i < 8 {
            (i as u16 + 1, MaterialField::Strength)
        } else {
            (i as u16 - 6, MaterialField::Melt)
        };
        assert!(
            matches!(record.command, Command::TuneMaterial { material: m, field: f, .. } if material == m && field == f)
        );
    }
    for d in &stock {
        assert_eq!(
            d.density,
            app.world()
                .resource::<Scene>()
                .sim
                .matter
                .economy
                .mats
                .get(d.id)
                .unwrap()
                .density
        );
    }
    click_command(&mut app, Action::ResetKnobs);
    let reset: Vec<_> = app
        .world()
        .resource::<Scene>()
        .sim
        .matter
        .economy
        .mats
        .definitions()
        .copied()
        .collect();
    assert_eq!(stock, reset);
}

#[cfg(test)]
#[test]
fn density_buttons_send_all_commands_and_reset_preserves_initial_stock() {
    let mut app = command_app();
    let stock: Vec<_> = app
        .world()
        .resource::<Scene>()
        .sim
        .matter
        .economy
        .mats
        .definitions()
        .copied()
        .collect();
    app.world_mut().resource_mut::<Overrides>().group = 4;
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .start_battle_command_capture(32)
        .unwrap();
    for slot in 0..8 {
        click_command(&mut app, Action::Knob(slot, true));
    }
    let trace = app
        .world_mut()
        .resource_mut::<Scene>()
        .sim
        .finish_battle_command_capture()
        .unwrap();
    assert!(trace.complete());
    assert_eq!(trace.records.len(), 8);
    for (i, record) in trace.records.iter().enumerate() {
        let (material, field) = (i as u16 + 1, MaterialField::Density);
        assert!(
            matches!(record.command, Command::TuneMaterial { material: m, field: f, .. } if material == m && field == f)
        );
    }
    click_command(&mut app, Action::ResetKnobs);
    let reset: Vec<_> = app
        .world()
        .resource::<Scene>()
        .sim
        .matter
        .economy
        .mats
        .definitions()
        .copied()
        .collect();
    assert_eq!(stock, reset);
}

#[cfg(test)]
#[test]
fn material_typed_input_and_paused_rebuild_use_the_same_engine_command() {
    let mut app = command_app();
    {
        let mut o = app.world_mut().resource_mut::<Overrides>();
        o.group = 5;
        o.editing = Some((2, "999999".into())); // Steel strength: UI ceiling 5000.
    }
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Enter);
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .start_battle_command_capture(8)
        .unwrap();
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        assert_eq!(scene.sim.matter.economy.mats.get(3).unwrap().strength, 5000);
        let trace = scene.sim.finish_battle_command_capture().unwrap();
        assert_eq!(trace.records.len(), 1);
        assert_eq!(
            trace.records[0].command,
            Command::TuneMaterial {
                material: 3,
                field: MaterialField::Strength,
                value: 5000
            }
        );
        scene.sim =
            BlastRange::with_layout(scene.sim.config.clone(), scene.sim.layout.clone().unwrap())
                .unwrap();
        scene.generation += 1;
        scene.sim.start_battle_command_capture(8).unwrap();
    }
    app.update();
    app.update();
    let mut scene = app.world_mut().resource_mut::<Scene>();
    assert_eq!(scene.sim.tick, 0);
    assert_eq!(scene.sim.matter.economy.mats.get(3).unwrap().strength, 5000);
    let trace = scene.sim.finish_battle_command_capture().unwrap();
    assert_eq!(trace.records.len(), 1);
    assert!(matches!(
        trace.records[0].command,
        Command::TuneMaterial {
            material: 3,
            field: MaterialField::Strength,
            value: 5000
        }
    ));
}

#[cfg(test)]
#[test]
fn density_typed_input_and_paused_rebuild_use_the_same_engine_command() {
    let mut app = command_app();
    {
        let mut o = app.world_mut().resource_mut::<Overrides>();
        o.group = 4;
        o.editing = Some((2, "999999".into())); // Steel density: UI ceiling 5000.
    }
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Enter);
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .start_battle_command_capture(8)
        .unwrap();
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        assert_eq!(scene.sim.matter.economy.mats.get(3).unwrap().density, 5000);
        let trace = scene.sim.finish_battle_command_capture().unwrap();
        assert_eq!(trace.records.len(), 1);
        assert_eq!(
            trace.records[0].command,
            Command::TuneMaterial {
                material: 3,
                field: MaterialField::Density,
                value: 5000
            }
        );
        scene.sim =
            BlastRange::with_layout(scene.sim.config.clone(), scene.sim.layout.clone().unwrap())
                .unwrap();
        scene.generation += 1;
        scene.sim.start_battle_command_capture(8).unwrap();
    }
    app.update();
    app.update();
    let mut scene = app.world_mut().resource_mut::<Scene>();
    assert_eq!(scene.sim.tick, 0);
    assert_eq!(scene.sim.matter.economy.mats.get(3).unwrap().density, 5000);
    let trace = scene.sim.finish_battle_command_capture().unwrap();
    assert_eq!(trace.records.len(), 1);
    assert!(matches!(
        trace.records[0].command,
        Command::TuneMaterial {
            material: 3,
            field: MaterialField::Density,
            value: 5000
        }
    ));
}

#[cfg(test)]
#[test]
fn debris_buttons_send_one_command_per_field_and_reset_restores_the_configuration() {
    let mut app = command_app();
    let stock = app.world().resource::<Scene>().sim.matter.tuning;
    app.world_mut().resource_mut::<Overrides>().group = 3;
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .start_battle_command_capture(32)
        .unwrap();
    for slot in 0..14 {
        click_command(&mut app, Action::Knob(slot, true));
    }
    let trace = app
        .world_mut()
        .resource_mut::<Scene>()
        .sim
        .finish_battle_command_capture()
        .unwrap();
    assert!(trace.complete());
    assert_eq!(trace.records.len(), 14);
    for (field, record) in DebrisField::ALL.into_iter().zip(trace.records) {
        assert!(matches!(record.command, Command::TuneDebris { field: f, .. } if f == field));
    }
    click_command(&mut app, Action::ResetKnobs);
    assert_eq!(stock, app.world().resource::<Scene>().sim.matter.tuning);
}

#[cfg(test)]
#[test]
fn debris_typed_input_and_paused_rebuild_use_the_same_engine_command() {
    let mut app = command_app();
    {
        let mut o = app.world_mut().resource_mut::<Overrides>();
        o.group = 3;
        o.editing = Some((1, "999".into())); // Bounce: UI clamps to 99.
    }
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Enter);
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .start_battle_command_capture(8)
        .unwrap();
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        assert_eq!(scene.sim.matter.tuning.bounce_pct, 99);
        let trace = scene.sim.finish_battle_command_capture().unwrap();
        assert_eq!(trace.records.len(), 1);
        assert_eq!(
            trace.records[0].command,
            Command::TuneDebris {
                field: DebrisField::BouncePct,
                value: 99
            }
        );
        scene.sim =
            BlastRange::with_layout(scene.sim.config.clone(), scene.sim.layout.clone().unwrap())
                .unwrap();
        scene.generation += 1;
        scene.sim.start_battle_command_capture(8).unwrap();
    }
    app.update();
    app.update();
    let mut scene = app.world_mut().resource_mut::<Scene>();
    assert_eq!(scene.sim.tick, 0);
    assert_eq!(scene.sim.matter.tuning.bounce_pct, 99);
    assert_eq!(
        scene
            .sim
            .finish_battle_command_capture()
            .unwrap()
            .records
            .len(),
        1
    );
}

#[cfg(test)]
#[test]
fn every_chassis_knob_dispatches_a_single_typed_command_and_reset_restores_values() {
    let mut app = command_app();
    let stock = Parameters::new(&app.world().resource::<Scene>().sim, 0, std::iter::empty());
    for (group, kind, gun) in [
        (1, RangeUnitKind::Tank, true),
        (2, RangeUnitKind::Strider, false),
    ] {
        app.world_mut().resource_mut::<Overrides>().group = group;
        app.world_mut()
            .resource_mut::<Scene>()
            .sim
            .start_battle_command_capture(32)
            .unwrap();
        for slot in 0..18 {
            click_command(&mut app, Action::Knob(slot, true));
        }
        let trace = app
            .world_mut()
            .resource_mut::<Scene>()
            .sim
            .finish_battle_command_capture()
            .unwrap();
        assert!(trace.complete());
        assert_eq!(trace.records.len(), 18);
        for (field, record) in MotionField::ALL.into_iter().zip(trace.records) {
            assert!(
                matches!(record.command, Command::TuneMotion { kind: k, include_gun: g, field: f, .. } if k == kind && g == gun && f == field)
            );
        }
    }
    click_command(&mut app, Action::ResetKnobs);
    let reset = Parameters::new(&app.world().resource::<Scene>().sim, 0, std::iter::empty());
    assert_eq!(reset.gun, stock.gun);
    assert_eq!(reset.motions, stock.motions);
    assert!(app.world().resource::<Overrides>().values.is_empty());
}

#[cfg(test)]
#[test]
fn paused_tick_zero_rebuild_reapplies_overrides_once_by_generation() {
    let mut app = command_app();
    app.world_mut().resource_mut::<Overrides>().group = 1;
    click_command(&mut app, Action::Knob(0, true));
    let speed = app.world().resource::<Scene>().sim.gun.motion.speed;
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        assert_eq!(scene.sim.tick, 0);
        scene.sim =
            BlastRange::with_layout(scene.sim.config.clone(), scene.sim.layout.clone().unwrap())
                .unwrap();
        scene.generation += 1;
        assert_ne!(scene.sim.gun.motion.speed, speed);
        scene.sim.start_battle_command_capture(8).unwrap();
    }
    app.update();
    app.update();
    let mut scene = app.world_mut().resource_mut::<Scene>();
    assert_eq!(scene.sim.tick, 0);
    assert_eq!(scene.sim.gun.motion.speed, speed);
    let trace = scene.sim.finish_battle_command_capture().unwrap();
    assert_eq!(trace.records.len(), 1);
    assert_eq!(
        trace.records[0].outcome,
        sim_core::blast::battle::commands::Outcome::Applied
    );
}

#[cfg(test)]
#[test]
fn typed_chassis_value_is_clamped_before_command_ingress() {
    let mut app = command_app();
    {
        let mut o = app.world_mut().resource_mut::<Overrides>();
        o.group = 1;
        o.editing = Some((0, "99999".into()));
    }
    app.world_mut()
        .resource_mut::<Scene>()
        .sim
        .start_battle_command_capture(8)
        .unwrap();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Enter);
    app.update();
    let mut scene = app.world_mut().resource_mut::<Scene>();
    assert_eq!(scene.sim.gun.motion.speed, 200);
    let trace = scene.sim.finish_battle_command_capture().unwrap();
    assert_eq!(trace.records.len(), 1);
    assert!(matches!(
        trace.records[0].command,
        Command::TuneMotion {
            field: MotionField::Speed,
            value: 200,
            ..
        }
    ));
}

#[cfg(test)]
#[test]
fn owned_parameters_match_every_live_knob_with_and_without_walker_or_course() {
    let mut source = panel::hud::tests::app();
    for variant in 0..3 {
        let mut scene = source.world_mut().resource_mut::<Scene>();
        if variant > 0 {
            scene.sim.units.retain(|u| u.kind != RangeUnitKind::Strider);
        }
        if variant > 1 {
            scene.sim.layout.as_mut().unwrap().course = None;
        }
        let view = Parameters::new(&scene.sim, scene.generation, material_ids());
        let gas = GasInput::default();
        for knob in knobs() {
            assert_eq!(
                read_parameters(knob.key, &view, &gas),
                read(knob.key, &scene, &gas),
                "variant {variant}: {}",
                knob.key
            );
        }
    }
}

#[cfg(test)]
#[test]
fn publisher_requests_parameters_only_for_visible_populated_settings_and_refreshes_paused_edits() {
    let mut app = panel::hud::tests::app();
    app.insert_resource(Overrides::new(false, true));
    let value = app.world_mut().spawn((SlotValue(0), Text::new(""))).id();
    app.update();
    assert!(app.world().resource::<UiSnapshot>().parameters.is_none());
    app.world_mut().resource_mut::<Overrides>().open = true;
    app.update();
    let old = app
        .world()
        .resource::<UiSnapshot>()
        .parameters
        .as_ref()
        .unwrap()
        .clone();
    {
        let mut scene = app.world_mut().resource_mut::<Scene>();
        scene.sim.config.energy += 1;
        scene.generation += 1;
    }
    app.update();
    let current = app
        .world()
        .resource::<UiSnapshot>()
        .parameters
        .as_ref()
        .unwrap();
    assert_eq!(current.tick, old.tick);
    assert_eq!(current.generation, old.generation + 1);
    assert_eq!(current.weapon.energy, old.weapon.energy + 1);
    app.world_mut().resource_mut::<Overrides>().open = false;
    app.update();
    assert!(app.world().resource::<UiSnapshot>().parameters.is_none());
    app.world_mut().resource_mut::<Overrides>().open = true;
    app.world_mut().despawn(value);
    app.update();
    assert!(app.world().resource::<UiSnapshot>().parameters.is_none());
}

#[cfg(test)]
#[test]
fn detached_refresh_keeps_all_groups_editing_and_marks_but_rejects_stale_parameters() {
    use sim_core::blast::read_model::View;
    let source = panel::hud::tests::app();
    let scene = source.world().resource::<Scene>();
    let view = View::new(&scene.sim, 9);
    let parameters = Parameters::new(&scene.sim, 9, material_ids());
    let gas = GasInput::default();
    let expected: BTreeMap<_, _> = knobs().map(|k| (k.key, read(k.key, scene, &gas))).collect();
    drop(source);
    let mut ui = App::new();
    ui.insert_resource(UiSnapshot {
        view: Some(view),
        parameters: Some(parameters),
        ..default()
    })
    .insert_resource(Overrides::new(true, true))
    .insert_resource(gas)
    .add_systems(Update, refresh);
    let root = ui.world_mut().spawn((SettingsRoot, Node::default())).id();
    let rows: Vec<_> = (0..SLOTS)
        .map(|slot| ui.world_mut().spawn((SlotValue(slot), Text::new(""))).id())
        .collect();
    let label = ui.world_mut().spawn((SlotLabel(0), Text::new(""))).id();
    for group in 0..Group::ALL.len() {
        ui.world_mut().resource_mut::<Overrides>().group = group;
        ui.update();
        for (slot, knob) in shown(ui.world().resource::<Overrides>()).iter().enumerate() {
            assert_eq!(
                ui.world().get::<Text>(rows[slot]).unwrap().0,
                expected[knob.key].map_or_else(|| "—".into(), |v| format_value(knob, v)),
                "{}",
                knob.key
            );
        }
    }
    {
        let mut o = ui.world_mut().resource_mut::<Overrides>();
        o.group = 0;
        o.values.insert(WEAPON[0].key, 123.0);
        o.editing = Some((0, "123".into()));
    }
    ui.update();
    assert_eq!(ui.world().get::<Text>(rows[0]).unwrap().0, "123_");
    assert!(ui.world().get::<Text>(label).unwrap().0.ends_with(" •"));
    ui.world_mut().resource_mut::<Overrides>().editing = None;
    ui.world_mut()
        .resource_mut::<UiSnapshot>()
        .parameters
        .as_mut()
        .unwrap()
        .generation = 8;
    ui.update();
    assert_eq!(ui.world().get::<Text>(rows[0]).unwrap().0, "—");
    {
        let mut s = ui.world_mut().resource_mut::<UiSnapshot>();
        let p = s.parameters.as_mut().unwrap();
        p.generation = 9;
        p.tick += 1;
    }
    ui.update();
    assert_eq!(ui.world().get::<Text>(rows[0]).unwrap().0, "—");
    ui.world_mut().resource_mut::<UiSnapshot>().parameters = None;
    ui.update();
    assert_eq!(ui.world().get::<Text>(rows[0]).unwrap().0, "—");
    ui.world_mut().resource_mut::<Overrides>().open = false;
    ui.update();
    assert_eq!(ui.world().get::<Node>(root).unwrap().display, Display::None);
}

#[cfg(test)]
#[test]
#[ignore = "isolated diagnostic, not a timing assertion"]
fn measure_absent_walker_defaults() {
    use std::{hint::black_box, time::Instant};
    let app = panel::hud::tests::app();
    let scene = app.world().resource::<Scene>();
    let view = Parameters::new(&scene.sim, scene.generation, material_ids());
    println!(
        "product_parameters requests={} materials={} payload_bytes={} retained_material_bytes={}",
        material_ids().count(),
        view.materials.len(),
        view.payload_bytes(),
        view.materials.capacity() * std::mem::size_of::<modding::MatDef>()
    );
    let fields: Vec<_> = WALKER
        .iter()
        .filter_map(|k| k.key.strip_prefix("walker."))
        .filter(|f| !matches!(*f, "speed" | "step"))
        .collect();
    let _ = stock_chassis(RangeUnitKind::Strider);
    for cached in [false, true, true, false] {
        let began = Instant::now();
        let mut sum = 0i64;
        for _ in 0..2000 {
            for field in &fields {
                let mut c = if cached {
                    stock_chassis(RangeUnitKind::Strider)
                } else {
                    modding::Tuning::from_ron(black_box(include_str!(
                        "../../../../../data/sim.ron"
                    )))
                    .unwrap()
                    .walking
                };
                sum += i64::from(*chassis_field(black_box(&mut c), black_box(field)).unwrap());
            }
        }
        println!(
            "walker_defaults cached={cached} frames=2000 fields={} elapsed_ns={} checksum={}",
            fields.len(),
            began.elapsed().as_nanos(),
            black_box(sum)
        );
    }
}
