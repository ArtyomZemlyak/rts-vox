//! Editable scenario commands and ordinary source launches. No private physics.
use super::*;
use modding::laboratory::{experiments::Input, sources::Source};
use std::collections::VecDeque;

pub struct Launch {
    pub commands: Vec<Command>,
    pub point: bool,
    pub visual: Option<(u8, u32)>,
}

pub fn materialize(lab: &mut Laboratory) -> Result<(), String> {
    let Some(plan) = lab.constructor.experiment.as_ref() else {
        return Ok(());
    };
    let inputs = plan.inputs.clone();
    let layout = lab
        .constructor
        .base
        .as_ref()
        .unwrap_or(&lab.definition.layout);
    let mut origin = None;
    let target = layout.target;
    let config = lab.preset_config();
    let mut added = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        if lab
            .constructor
            .workspace
            .sources
            .iter()
            .any(|s| s.input == Some(i))
        {
            continue;
        }
        let at = match input {
            Input::Fire => *origin.get_or_insert_with(|| BlastRange::authored_muzzle(layout)),
            Input::Detonate { at } => *at,
            _ => continue,
        };
        if lab.constructor.workspace.sources.len() + added.len() >= 32 {
            return Err("Не более 32 источников".into());
        }
        let detonate = matches!(input, Input::Detonate { .. });
        let source = Source {
            name: format!(
                "{} {}",
                if detonate {
                    "Заряд"
                } else {
                    "Оружие"
                },
                i + 1
            ),
            at,
            target: if detonate { at } else { target },
            binding: None,
            config,
            power_milli: 1000,
            visual: if lab.recipe.effect == 6 {
                2
            } else if lab.recipe.effect == 2 {
                1
            } else {
                0
            },
            effect: Some(lab.recipe.effect),
            detonate,
            input: Some(i),
        };
        source.validate(layout, lab.constructor.placed.len())?;
        added.push(source);
    }
    lab.constructor.workspace.sources.extend(added);
    Ok(())
}

pub fn validate(lab: &Laboratory, layout: &RangeLayout) -> Result<(), String> {
    if let Some(plan) = &lab.constructor.experiment {
        plan.validate_sources(layout,&lab.constructor.workspace.sources).map_err(|e|if e=="invalid laboratory experiment input" {"Команда недопустима: проверьте участника, точку и порядок ручного управления и начала боя".to_string()}else{e})?;
    }
    Ok(())
}

pub fn sync_sources(lab: &mut Laboratory) {
    let Some(plan) = lab.constructor.experiment.as_mut() else {
        return;
    };
    for s in &lab.constructor.workspace.sources {
        if let Some(input) = s.input.and_then(|i| plan.inputs.get_mut(i)) {
            *input = if s.detonate {
                Input::Detonate { at: s.at }
            } else {
                Input::Fire
            };
        }
    }
}

pub fn attach_selected(lab: &mut Laboratory) -> Result<(), String> {
    let Some(plan) = lab.constructor.experiment.as_mut() else {
        return Ok(());
    };
    if plan.inputs.len() >= 32 {
        return Err("Не более 32 команд в сценарии".into());
    }
    let s = lab
        .constructor
        .workspace
        .sources
        .get_mut(lab.constructor.workspace.selected)
        .ok_or("Нет источника")?;
    if s.input.is_none() {
        s.input = Some(plan.inputs.len());
        plan.inputs.push(if s.detonate {
            Input::Detonate { at: s.at }
        } else {
            Input::Fire
        });
    }
    Ok(())
}

pub fn remove_input(lab: &mut Laboratory, index: usize) {
    remove_from(
        &mut lab.constructor.experiment,
        &mut lab.constructor.workspace,
        index,
    );
    lab.ui.scenario_input = lab.ui.scenario_input.min(
        lab.constructor
            .experiment
            .as_ref()
            .map_or(0, |p| p.inputs.len().saturating_sub(1)),
    );
}
pub fn remove_from(
    plan: &mut Option<modding::laboratory::experiments::Plan>,
    workspace: &mut modding::laboratory::sources::Workspace,
    index: usize,
) {
    let Some(p) = plan.as_mut().filter(|p| index < p.inputs.len()) else {
        return;
    };
    p.inputs.remove(index);
    let selected = workspace.selected;
    let removed_before = workspace
        .sources
        .iter()
        .take(selected)
        .filter(|s| s.input == Some(index))
        .count();
    workspace.sources.retain(|s| s.input != Some(index));
    for s in &mut workspace.sources {
        if let Some(i) = &mut s.input {
            if *i > index {
                *i -= 1;
            }
        }
    }
    workspace.selected = selected
        .saturating_sub(removed_before)
        .min(workspace.sources.len().saturating_sub(1));
}
pub fn remove_actors(
    plan: &mut Option<modding::laboratory::experiments::Plan>,
    workspace: &mut modding::laboratory::sources::Workspace,
    first: usize,
    count: usize,
) {
    let actor = |i: &Input| match i {
        Input::Move { actor, .. }
        | Input::Dig { actor, .. }
        | Input::Ground { actor, .. }
        | Input::Pilot { actor, .. } => Some(*actor),
        _ => None,
    };
    let remove: Vec<_> = plan.as_ref().map_or(Vec::new(), |p| {
        p.inputs
            .iter()
            .enumerate()
            .filter(|(_, i)| actor(i).is_some_and(|a| a >= first && a < first + count))
            .map(|(i, _)| i)
            .collect()
    });
    for i in remove.into_iter().rev() {
        remove_from(plan, workspace, i);
    }
    if let Some(p) = plan {
        for i in &mut p.inputs {
            match i {
                Input::Move { actor, .. }
                | Input::Dig { actor, .. }
                | Input::Ground { actor, .. }
                | Input::Pilot { actor, .. } => {
                    if *actor >= first + count {
                        *actor -= count;
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn queue(lab: &Laboratory, scene: &Scene) -> Result<VecDeque<Launch>, String> {
    let plan = lab.constructor.experiment.as_ref().ok_or("Нет сценария")?;
    validate(lab, scene.sim.layout.as_ref().ok_or("Нет карты")?)?;
    let mut out = VecDeque::new();
    for (i, input) in plan.inputs.iter().enumerate() {
        if let Some(s) = lab
            .constructor
            .workspace
            .sources
            .iter()
            .find(|s| s.input == Some(i))
        {
            let at = if let Some(id) = sources::actor(lab, s, &scene.sim)? {
                scene
                    .sim
                    .battle_muzzles(id)
                    .first()
                    .ok_or("У источника нет ствола")?
                    .map(fx::Fx::to_int_trunc)
            } else {
                s.at
            };
            out.push_back(Launch {
                point: true,
                visual: Some((s.visual, s.power_milli)),
                commands: vec![
                    Command::Gun(GunCommand::Origin { at }),
                    Command::ConfigureWeapon {
                        config: s.resolved(),
                    },
                    Command::Gun(GunCommand::Aim { at: Some(s.target) }),
                    Command::Gun(if s.detonate {
                        GunCommand::Detonate { at: s.at }
                    } else {
                        GunCommand::Refire
                    }),
                ],
            });
        } else {
            if matches!(input, Input::Fire | Input::Detonate { .. }) {
                return Err("Источник сценария отсутствует; восстановите опыт".into());
            }
            let mut single = plan.clone();
            single.inputs = vec![input.clone()];
            out.push_back(Launch {
                point: false,
                visual: None,
                commands: experiments::commands(&single),
            });
        }
    }
    Ok(out)
}

pub fn advance(lab: &mut Laboratory, scene: &mut Scene) {
    if scene.paused || lab.tools.live {
        return;
    }
    while let Some(next) = lab.launch.front() {
        if next.point && !scene.sim.ready() {
            break;
        }
        let next = lab.launch.pop_front().unwrap();
        let visual = next.visual;
        for c in next.commands {
            let outcome = scene.sim.execute_command(c);
            if !outcome.admitted() || outcome == sim_core::blast::battle::commands::Outcome::Busy {
                lab.launch.clear();
                lab.notice = format!("Команда сценария отклонена: {outcome:?}");
                scene.paused = true;
                lab.observing = false;
                return;
            }
        }
        if let Some((visual, power)) = visual {
            scene.energy = visual != 0;
            scene.anni = visual == 2;
            scene.power = power;
        }
        if lab.launch.is_empty() && lab.observing {
            let duration = lab
                .constructor
                .workspace
                .observation_ticks
                .unwrap_or_else(|| lab.constructor.experiment.as_ref().unwrap().duration_ticks);
            lab.deadline = Some(scene.sim.tick.saturating_add(duration));
        }
    }
}

pub fn label(lab: &Laboratory, index: usize) -> String {
    if let Some(s) = lab
        .constructor
        .workspace
        .sources
        .iter()
        .find(|s| s.input == Some(index))
    {
        return s.name.clone();
    }
    let Some(input) = lab
        .constructor
        .experiment
        .as_ref()
        .and_then(|p| p.inputs.get(index))
    else {
        return String::new();
    };
    match input {
        Input::Fire => "Выстрел".into(),
        Input::Detonate { .. } => "Заряд".into(),
        Input::FreeControl => "Ручные команды участников".into(),
        Input::StartBattle => "Начало боя".into(),
        Input::Move { actor, .. } => format!("Юнит {} · движение", actor + 1),
        Input::Dig { actor, .. } => format!("Юнит {} · копка", actor + 1),
        Input::Ground { actor, .. } => format!("Юнит {} · штатное оружие", actor + 1),
        Input::Pilot { actor, .. } => format!("Юнит {} · управление", actor + 1),
    }
}
fn selected(lab: &Laboratory) -> Option<&Input> {
    lab.constructor
        .experiment
        .as_ref()?
        .inputs
        .get(lab.ui.scenario_input)
}
pub fn has_actor(lab: &Laboratory) -> bool {
    matches!(
        selected(lab),
        Some(Input::Move { .. } | Input::Dig { .. } | Input::Ground { .. } | Input::Pilot { .. })
    )
}
pub fn has_target(lab: &Laboratory) -> bool {
    has_actor(lab)
}
pub fn has_pilot(lab: &Laboratory) -> bool {
    matches!(selected(lab), Some(Input::Pilot { .. }))
}
pub fn pick_device(
    lab: &Laboratory,
    camera: &Camera,
    transform: &GlobalTransform,
    cursor: Vec2,
) -> Option<usize> {
    lab.constructor
        .workspace
        .devices
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            let p = Vec3::from(d.at.map(|v| v as f32));
            let screen = camera.world_to_viewport(transform, p).ok()?;
            let distance = screen.distance_squared(cursor);
            (distance <= 14.0 * 14.0).then_some((i, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
        .map(|(i, _)| i)
}

#[derive(Component)]
pub struct Row(pub u8, pub usize);
#[derive(Component)]
pub struct Title(pub u8, pub usize);
pub fn list(p: &mut ChildSpawnerCommands, f: &TextFont, kind: u8) {
    for i in 0..32 {
        p.spawn((
            Button,
            Row(kind, i),
            if kind == 0 {
                LabAction::ScenarioInput(i)
            } else {
                LabAction::Device(i)
            },
            Node {
                display: Display::None,
                min_height: px(32),
                padding: UiRect::all(px(6)),
                border: UiRect::all(px(1)),
                ..default()
            },
            BackgroundColor(theme::BUTTON),
            BorderColor::all(theme::FRAME),
        ))
        .with_children(|p| {
            p.spawn((
                Title(kind, i),
                Text::new(""),
                f.clone(),
                TextColor(theme::TEXT),
            ));
        });
    }
}
pub fn sync(
    lab: Res<Laboratory>,
    mut rows: Query<(&Row, &mut Node, &mut BorderColor)>,
    mut titles: Query<(&Title, &mut Text)>,
) {
    if !lab.active() {
        return;
    }
    for (Row(kind, i), mut n, mut border) in &mut rows {
        let count = if *kind == 0 {
            lab.constructor
                .experiment
                .as_ref()
                .map_or(0, |p| p.inputs.len())
        } else {
            lab.constructor.workspace.devices.len()
        };
        n.display = if *i < count {
            Display::Flex
        } else {
            Display::None
        };
        *border = BorderColor::all(
            if *i
                == if *kind == 0 {
                    lab.ui.scenario_input
                } else {
                    lab.ui.device
                }
            {
                theme::ACCENT
            } else {
                theme::FRAME
            },
        );
    }
    for (Title(kind, i), mut text) in &mut titles {
        let value = if *kind == 0 {
            format!("{}. {}", i + 1, label(&lab, *i))
        } else {
            lab.constructor
                .workspace
                .devices
                .get(*i)
                .map_or(String::new(), |d| {
                    format!(
                        "{}. {}",
                        i + 1,
                        match d.kind {
                            modding::lab_tools::DeviceKind::Drill => "Бур",
                            modding::lab_tools::DeviceKind::Crawler => "Краулер",
                            modding::lab_tools::DeviceKind::Collector => "Сборщик",
                        }
                    )
                })
        };
        if text.0 != value {
            text.0 = value;
        }
    }
}

pub fn action(lab: &mut Laboratory, a: LabAction, scene: &Scene) -> Result<bool, String> {
    match a {
        LabAction::ScenarioInput(i) => {
            if !lab
                .constructor
                .experiment
                .as_ref()
                .is_some_and(|p| i < p.inputs.len())
            {
                return Err("Нет такой команды".into());
            }
            lab.ui.scenario_input = i;
            lab.ui.tab = 24;
            if let Some(source) = lab
                .constructor
                .workspace
                .sources
                .iter()
                .position(|s| s.input == Some(i))
            {
                lab.constructor.workspace.selected = source;
                lab.ui.tab = 10;
            }
            return Ok(false);
        }
        LabAction::Device(i) => {
            if i >= lab.constructor.workspace.devices.len() {
                return Err("Нет устройства".into());
            }
            lab.ui.device = i;
            lab.ui.tab = 25;
            return Ok(false);
        }
        _ => {}
    }
    if !scene.paused {
        return Err("Поставьте опыт на паузу перед изменением сценария".into());
    }
    let old_plan = lab.constructor.experiment.clone();
    let old_workspace = lab.constructor.workspace.clone();
    let old_selection = (lab.ui.scenario_input, lab.ui.device, lab.constructor.mode);
    let at = lab
        .constructor
        .base
        .as_ref()
        .unwrap_or(&lab.definition.layout)
        .target;
    match a {
        LabAction::ScenarioRemove => remove_input(lab, lab.ui.scenario_input),
        LabAction::ScenarioMove(delta) => {
            let plan = lab.constructor.experiment.as_mut().ok_or("Нет сценария")?;
            let from = lab.ui.scenario_input;
            let to = from as i32 + delta;
            if from >= plan.inputs.len() || to < 0 || to as usize >= plan.inputs.len() {
                return Err("Команда уже на краю списка".into());
            }
            let to = to as usize;
            plan.inputs.swap(from, to);
            for s in &mut lab.constructor.workspace.sources {
                if s.input == Some(from) {
                    s.input = Some(to);
                } else if s.input == Some(to) {
                    s.input = Some(from);
                }
            }
            lab.ui.scenario_input = to;
        }
        LabAction::RemoveDevice => {
            if lab.ui.device >= lab.constructor.workspace.devices.len() {
                return Err("Выберите устройство".into());
            }
            lab.constructor.workspace.devices.remove(lab.ui.device);
            lab.ui.device = lab.ui.device.saturating_sub(1);
        }
        LabAction::ScenarioAdd(kind) => {
            if lab.constructor.experiment.is_none() {
                lab.constructor.experiment = Some(modding::laboratory::experiments::Plan {
                    name: "Свой сценарий".into(),
                    description: "Редактируемые команды движка".into(),
                    duration_ticks: lab.definition.observation_ticks,
                    inputs: Vec::new(),
                });
            }
            let p = lab.constructor.experiment.as_mut().unwrap();
            if p.inputs.len() >= 32 {
                return Err("Не более 32 команд".into());
            }
            let input = match kind {
                0 => Input::Fire,
                1 => Input::Detonate { at },
                2 => Input::FreeControl,
                3 => Input::StartBattle,
                4 => Input::Move {
                    actor: 0,
                    to: [at[0], at[2]],
                },
                5 => Input::Dig { actor: 0, at },
                6 => Input::Ground { actor: 0, at },
                _ => Input::Pilot {
                    actor: 0,
                    throttle: 0,
                    turn: 0,
                    aim: at,
                    fire: false,
                },
            };
            lab.ui.scenario_input = p.inputs.len();
            p.inputs.push(input);
        }
        LabAction::ScenarioAt(at) => {
            set_target(lab, at)?;
            lab.constructor.mode = constructor::Mode::Select;
        }
        _ => return Ok(false),
    }
    let result = materialize(lab)
        .and_then(|_| lab.constructor.layout(&lab.definition, lab.recipe))
        .and_then(|layout| validate(lab, &layout));
    if let Err(e) = result {
        lab.constructor.experiment = old_plan;
        lab.constructor.workspace = old_workspace;
        lab.ui.scenario_input = old_selection.0;
        lab.ui.device = old_selection.1;
        lab.constructor.mode = old_selection.2;
        return Err(e);
    }
    lab.constructor.revision += 1;
    Ok(true)
}

fn target(input: &Input) -> [i32; 3] {
    match input {
        Input::Detonate { at } | Input::Dig { at, .. } | Input::Ground { at, .. } => *at,
        Input::Move { to, .. } => [to[0], 0, to[1]],
        Input::Pilot { aim, .. } => *aim,
        _ => [0; 3],
    }
}
fn set_target(lab: &mut Laboratory, at: [i32; 3]) -> Result<(), String> {
    let input = lab
        .constructor
        .experiment
        .as_mut()
        .and_then(|p| p.inputs.get_mut(lab.ui.scenario_input))
        .ok_or("Выберите команду")?;
    match input {
        Input::Dig { at: p, .. } | Input::Ground { at: p, .. } | Input::Detonate { at: p } => {
            *p = at
        }
        Input::Move { to, .. } => *to = [at[0], at[2]],
        Input::Pilot { aim, .. } => *aim = at,
        _ => return Err("У этой команды нет точки".into()),
    }
    Ok(())
}
pub fn get_number(lab: &Laboratory, id: u8) -> i64 {
    let input = lab
        .constructor
        .experiment
        .as_ref()
        .and_then(|p| p.inputs.get(lab.ui.scenario_input));
    let device = lab.constructor.workspace.devices.get(lab.ui.device);
    match id {
        60 => input.map_or(0, |i| match i {
            Input::Move { actor, .. }
            | Input::Dig { actor, .. }
            | Input::Ground { actor, .. }
            | Input::Pilot { actor, .. } => *actor as i64 + 1,
            _ => 0,
        }),
        61..=63 => input.map_or(0, |i| target(i)[(id - 61) as usize] as i64),
        64 => input.map_or(0, |i| {
            if let Input::Pilot { throttle, .. } = i {
                *throttle as i64
            } else {
                0
            }
        }),
        65 => input.map_or(0, |i| {
            if let Input::Pilot { turn, .. } = i {
                *turn as i64
            } else {
                0
            }
        }),
        66 => input.map_or(0, |i| {
            if let Input::Pilot { fire, .. } = i {
                i64::from(*fire)
            } else {
                0
            }
        }),
        70..=72 => device.map_or(0, |d| d.at[(id - 70) as usize] as i64),
        73 => device.map_or(0, |d| d.turns as i64),
        74 => device
            .and_then(|d| d.collector.as_ref())
            .map_or(0, |c| c.capacity as i64),
        75..=77 => device
            .and_then(|d| d.collector.as_ref())
            .map_or(0, |c| c.dock[(id - 75) as usize] as i64),
        78..=80 => device
            .and_then(|d| d.collector.as_ref())
            .map_or(0, |c| c.area[(id - 78) as usize] as i64),
        _ => 0,
    }
}
pub fn set_number(lab: &mut Laboratory, id: u8, value: i64, scene: &Scene) -> Result<bool, String> {
    if !scene.paused {
        return Err("Поставьте опыт на паузу".into());
    }
    let old_plan = lab.constructor.experiment.clone();
    let old_workspace = lab.constructor.workspace.clone();
    let result = (|| {
        let value = i32::try_from(value).map_err(|_| "Слишком большое значение")?;
        if id < 70 {
            let input = lab
                .constructor
                .experiment
                .as_mut()
                .and_then(|p| p.inputs.get_mut(lab.ui.scenario_input))
                .ok_or("Выберите команду сценария")?;
            match id {
                60 => match input {
                    Input::Move { actor, .. }
                    | Input::Dig { actor, .. }
                    | Input::Ground { actor, .. }
                    | Input::Pilot { actor, .. } => {
                        *actor = usize::try_from(value.checked_sub(1).ok_or("Участник: от 1")?)
                            .map_err(|_| "Участник: от 1")?
                    }
                    _ => return Err("У команды нет участника".into()),
                },
                61..=63 => {
                    let mut at = target(input);
                    at[(id - 61) as usize] = value;
                    set_target(lab, at)?;
                }
                64..=66 => {
                    if let Input::Pilot {
                        throttle,
                        turn,
                        fire,
                        ..
                    } = input
                    {
                        if (id < 66 && !(-1..=1).contains(&value))
                            || (id == 66 && !(0..=1).contains(&value))
                        {
                            return Err("Ход/поворот: −1…1, огонь: 0/1".into());
                        }
                        match id {
                            64 => *throttle = value as i8,
                            65 => *turn = value as i8,
                            _ => *fire = value == 1,
                        }
                    } else {
                        return Err("Параметр только у команды управления".into());
                    }
                }
                _ => return Err("Нет такого поля".into()),
            }
        } else {
            let d = lab
                .constructor
                .workspace
                .devices
                .get_mut(lab.ui.device)
                .ok_or("Выберите устройство")?;
            match id {
                70..=72 => d.at[(id - 70) as usize] = value,
                73 => {
                    if !(0..=3).contains(&value) {
                        return Err("Поворот: 0…3".into());
                    }
                    d.turns = value as u8;
                }
                74..=80 => {
                    let c = d
                        .collector
                        .as_mut()
                        .ok_or("Настройка только для сборщика")?;
                    match id {
                        74 => {
                            if value <= 0 {
                                return Err("Бак должен быть положительным".into());
                            }
                            c.capacity = value as u64;
                        }
                        75..=77 => c.dock[(id - 75) as usize] = value,
                        _ => c.area[(id - 78) as usize] = value,
                    }
                }
                _ => return Err("Нет поля устройства".into()),
            }
        }
        let layout = lab.constructor.layout(&lab.definition, lab.recipe)?;
        lab.constructor
            .workspace
            .validate(&layout, lab.constructor.placed.len())?;
        validate(lab, &layout)
    })();
    if let Err(e) = result {
        lab.constructor.experiment = old_plan;
        lab.constructor.workspace = old_workspace;
        return Err(e);
    }
    lab.constructor.revision += 1;
    Ok(true)
}
