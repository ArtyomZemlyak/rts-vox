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
}

pub(super) fn publish(
    scene: Res<Scene>,
    panel: Res<panel::Panel>,
    readouts: Query<(), With<panel::Readout>>,
    knobs: Option<Res<knobs::Overrides>>,
    values: Query<(), With<knobs::SlotValue>>,
    mut output: ResMut<UiSnapshot>,
) {
    match &mut output.view {
        Some(view) => view.update(&scene.sim, scene.generation),
        None => output.view = Some(View::new(&scene.sim, scene.generation)),
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
}
