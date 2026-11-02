//! Product bridge: all HUD consumers see the same completed publication.
//! Other terrain/geometry readers are migrated independently of this roster.
use super::*;
use sim_core::blast::read_model::{Inspection, Parameters, View};

#[derive(Resource, Default)]
pub(super) struct UiSnapshot {
    pub view: Option<View>,
    pub inspection: Option<Inspection>,
    pub parameters: Option<Parameters>,
    pub paused: bool,
    pub fps: f32,
    pub power: u32,
    pub weapon: u8,
    pub step_us: u128,
    pub notice: String,
    pub crawler_cargo: Vec<Option<(u64, u64, bool, bool, u64, u64, &'static str)>>,
    pub base_cargo: Vec<Option<(u64, u64)>>,
    pub electric_support: Vec<u32>,
}

pub(super) fn publish(
    scene: Res<Scene>,
    mut panel: ResMut<panel::Panel>,
    readouts: Query<(), With<panel::Readout>>,
    knobs: Option<Res<knobs::Overrides>>,
    values: Query<(), With<knobs::SlotValue>>,
    mut output: ResMut<UiSnapshot>,
) {
    match &mut output.view {
        Some(view) => view.update(&scene.sim, scene.generation),
        None => output.view = Some(View::new(&scene.sim, scene.generation)),
    }
    // A queued replay binds after its layout commits. An unrelated load/reset
    // has no such launch and must not keep another world's goals or markers.
    if !panel.battle_launch
        && panel
            .mission
            .as_ref()
            .is_some_and(|mission| !mission.belongs_to(scene.generation))
    {
        panel.mission = None;
        panel.briefing.clear();
    }
    if let (Some(mission), Some(view)) = (&mut panel.mission, &output.view) {
        mission.update(view);
    }
    // Hidden diagnostics must not scan the entire debris history every frame.
    if panel.drawer == 5 && !readouts.is_empty() {
        match &mut output.inspection {
            Some(view) => view.update(&scene.sim, scene.generation),
            None => output.inspection = Some(Inspection::new(&scene.sim, scene.generation)),
        }
    } else {
        output.inspection = None;
    }
    if knobs.is_some_and(|o| o.open) && !values.is_empty() {
        match &mut output.parameters {
            Some(view) => view.update(&scene.sim, scene.generation, knobs::material_ids()),
            None => {
                output.parameters = Some(Parameters::new(
                    &scene.sim,
                    scene.generation,
                    knobs::material_ids(),
                ))
            }
        }
    } else {
        output.parameters = None;
    }
    output.paused = scene.paused;
    output.fps = scene.fps;
    output.power = scene.power;
    output.weapon = panel::weapon_of(&scene);
    output.step_us = scene.step_us;
    output.notice.clone_from(&scene.notice);
    output.crawler_cargo.clear();
    output.crawler_cargo.extend(scene.sim.units.iter().enumerate().map(|(i, u)| {
        matches!(u.kind, RangeUnitKind::Crawler).then(|| scene.sim.tank_fill(i).map(|(held, cap)| {
            let (stored, limit) = scene.sim.side_store(scene.sim.allied(i)).unwrap_or((0, 0));
            let mode = match scene.sim.battle.actors.get(i).map(|a| a.order) {
                Some(sim_core::blast::battle::Order::Dig(_)) => "БУРЕНИЕ",
                Some(sim_core::blast::battle::Order::Reclaim(_)) => "СБОР ОБЛОМКОВ",
                _ => "ОЖИДАНИЕ",
            };
            (held, cap, scene.sim.hauling(i), u.status.weapon && u.status.engine, stored, limit, mode)
        })).flatten()
    }));
    output.base_cargo.clear();
    output.base_cargo.extend(scene.sim.units.iter().enumerate().map(|(i, u)| {
        matches!(u.kind, RangeUnitKind::Base | RangeUnitKind::GrebenStorage | RangeUnitKind::KilFactoryT1 | RangeUnitKind::KilFactoryT2 | RangeUnitKind::KilFactoryT3 | RangeUnitKind::KilFactoryT4).then(|| scene.sim.side_store(scene.sim.allied(i))).flatten()
    }));
    output.electric_support.clear();
    output.electric_support.extend((0..scene.sim.units.len()).map(|i| scene.sim.electric_support_count(i)));
}
