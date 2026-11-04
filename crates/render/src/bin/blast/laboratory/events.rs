//! Authoring adapters for placeable events; physical work stays in BlastRange.
use super::*;
use modding::laboratory::sources::Source;

pub fn icon(name: &str) -> &'static str {
    match name {
        "radiation" => "radiation",
        "hammer" => "hammer",
        "zap" => "zap",
        "flame" => "flame",
        "sparkles" => "sparkles",
        "crosshair" => "crosshair",
        _ => "bomb",
    }
}

pub fn action(lab: &mut Laboratory, action: LabAction, scene: &mut Scene) -> Result<bool, String> {
    let index = match action {
        LabAction::PickEvent(i) | LabAction::PlaceEvent(i, _) => i,
        _ => return Ok(false),
    };
    let event = lab
        .events
        .get(index)
        .ok_or("Нет такого воздействия")?
        .clone();
    if lab.constructor.base.as_ref().is_some_and(|b| b.battle) {
        return Err("В боевом сценарии редактируйте штатное оружие участника и его команду".into());
    }
    if lab.constructor.workspace.sources.len() >= 32 {
        return Err("В сцене не более 32 источников".into());
    }
    if lab
        .constructor
        .experiment
        .as_ref()
        .is_some_and(|p| p.inputs.len() >= 32)
    {
        return Err("Не более 32 команд в сценарии".into());
    }
    if matches!(action, LabAction::PickEvent(_)) {
        lab.ui.event = Some(index);
        lab.constructor.mode = constructor::Mode::Event;
        lab.constructor.elevation = 0;
        lab.notice = event.description;
        return Ok(false);
    }
    let LabAction::PlaceEvent(_, at) = action else {
        unreachable!()
    };
    if !scene.sim.ready() {
        return Err("Дождитесь завершения воздействия или нажмите «Сброс»".into());
    }
    let power = lab
        .definition
        .layout
        .power_milli
        .iter()
        .position(|p| *p == 1000)
        .ok_or("Нет базовой мощности")?;
    let recipe = lab.recipe;
    let weapon = lab.constructor.weapon.take();
    lab.recipe.effect = event.effect;
    lab.recipe.power = power;
    let config = lab.preset_config();
    lab.recipe = recipe;
    lab.constructor.weapon = weapon;
    let source = Source {
        name: format!(
            "{} {}",
            event.name,
            lab.constructor.workspace.sources.len() + 1
        ),
        at,
        target: if event.detonate {
            at
        } else {
            scene.sim.target()
        },
        binding: None,
        config,
        power_milli: event.power_milli,
        effect: Some(event.effect),
        visual: if event.effect == 6 {
            2
        } else if event.effect == 2 {
            1
        } else {
            0
        },
        detonate: event.detonate,
        input: None,
    };
    source.validate(
        scene.sim.layout.as_ref().ok_or("Нет карты")?,
        lab.constructor.placed.len(),
    )?;
    source.resolved().validate()?;
    let before = lab.constructor.workspace.clone();
    let rebuild =
        lab.constructor.experiment.is_some() || !scene.sim.point_gun() || scene.sim.battle_mode();
    lab.constructor.workspace.selected = lab.constructor.workspace.sources.len();
    lab.constructor.workspace.sources.push(source);
    if !rebuild {
        if let Err(e) = sources::arm(lab, scene) {
            lab.constructor.workspace = before;
            return Err(e);
        }
    }
    scenario::attach_selected(lab)?;
    lab.constructor.weapon = None;
    lab.constructor.variant = None;
    if let Some(base) = &mut lab.constructor.base {
        base.battle = false;
    }
    lab.constructor.aim = None;
    lab.constructor.revision += 1;
    lab.constructor.mode = if event.detonate {
        constructor::Mode::Select
    } else {
        constructor::Mode::Aim
    };
    lab.ui.event = None;
    lab.ui.tab = 2;
    lab.notice = if event.detonate {
        "Заряд размещён. Настройте мощность и нажмите F. «Сброс» восстанавливает сборку.".into()
    } else {
        "Источник размещён. Выберите цель кликом, затем нажмите F.".into()
    };
    Ok(rebuild)
}
