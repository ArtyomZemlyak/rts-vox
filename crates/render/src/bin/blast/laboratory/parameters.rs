//! Commit-on-Enter numeric fields: authoring changes never rebuild on each keystroke.
use super::*;
#[derive(Component)]
pub struct Number(u8);
#[derive(Component)]
pub struct SourceRow(usize);
#[derive(Component)]
pub struct SourceName(usize);
#[derive(Component)]
pub enum Info {
    Source,
    Binding,
    Edit,
    Pause,
    Trigger,
}

pub fn row(p: &mut ChildSpawnerCommands, f: &TextFont, id: u8, title: &str) {
    p.spawn((Text::new(title), f.clone(), TextColor(theme::MUTED)));
    p.spawn(Node {
        column_gap: px(5),
        ..default()
    })
    .with_children(|p| {
        ui::control(p, f, "−", "chevron-left", LabAction::AdjustNumber(id, -1));
        field(p, f, id);
        ui::control(p, f, "+", "chevron-right", LabAction::AdjustNumber(id, 1));
    });
}
fn field(p: &mut ChildSpawnerCommands, f: &TextFont, id: u8) {
    p.spawn((
        Button,
        LabAction::EditNumber(id),
        Node {
            flex_grow: 1.0,
            min_width: px(0),
            min_height: px(37),
            overflow: Overflow::clip(),
            padding: UiRect::all(px(7)),
            border: UiRect::all(px(1)),
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(theme::BUTTON),
        BorderColor::all(theme::FRAME),
    ))
    .with_children(|p| {
        p.spawn((Number(id), Text::new(""), f.clone(), TextColor(theme::TEXT)));
    });
}
/// Three editable coordinates on one row; no six rows of repeated +/- chrome.
pub fn coordinates(p: &mut ChildSpawnerCommands, f: &TextFont, first: u8, title: &str) {
    p.spawn((Text::new(title), f.clone(), TextColor(theme::MUTED)));
    p.spawn(Node {
        column_gap: px(5),
        ..default()
    })
    .with_children(|p| {
        for (axis, name) in ["X", "Y", "Z"].iter().enumerate() {
            p.spawn(Node {
                flex_basis: px(0),
                flex_grow: 1.0,
                min_width: px(0),
                flex_direction: FlexDirection::Column,
                row_gap: px(3),
                ..default()
            })
            .with_children(|p| {
                p.spawn((Text::new(*name), f.clone(), TextColor(theme::MUTED)));
                field(p, f, first + axis as u8);
            });
        }
    });
}
pub fn info(p: &mut ChildSpawnerCommands, f: &TextFont, role: Info) {
    p.spawn((role, Text::new(""), f.clone(), TextColor(theme::MUTED)));
}
pub fn source_list(p: &mut ChildSpawnerCommands, f: &TextFont) {
    for i in 0..32 {
        p.spawn((
            Button,
            SourceRow(i),
            LabAction::Source(i),
            Node {
                min_height: px(32),
                padding: UiRect::all(px(6)),
                border: UiRect::all(px(1)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(theme::BUTTON),
            BorderColor::all(theme::FRAME),
        ))
        .with_children(|p| {
            p.spawn((
                SourceName(i),
                Text::new(""),
                f.clone(),
                TextColor(theme::TEXT),
            ));
        });
    }
}
pub fn get(l: &Laboratory, id: u8) -> i64 {
    if id>=60 {return scenario::get_number(l,id);}
    let c = l.config();
    let s = sources::active(l);
    match id {
        0 => s.map_or(1000, |s| s.power_milli) as i64,
        1 => c.energy as i64,
        2 => c.shell_speed as i64,
        3 => c.gravity as i64,
        4 => c.charge_ticks as i64,
        5 => c.heat_per_cell as i64,
        6 => c.material_energy_j as i64,
        7 => c.hull_impulse_milli as i64,
        8 => c.wave_work_per_tick as i64,
        9 => c.edits_per_tick as i64,
        40 => c.heat_units as i64,
        41 => c.gas_source_radius as i64,
        42 => c.gas_temperature_k as i64,
        43 => c.gas_fuel_milli as i64,
        44 => c.gas_pressure_milli as i64,
        45 => c.cooling_milli as i64,
        46 => c.reaction_milli as i64,
        47 => c.buoyancy_milli as i64,
        48 => c.soil_resistance as i64,
        49 => c.concrete_resistance as i64,
        50 => c.steel_resistance as i64,
        51 => c.fragment_box as i64,
        52 => c.half_cells as i64,
        53 => c.cell_units as i64,
        54 => c.capture_cells_per_tick as i64,
        10..=12 => s.map_or(0, |s| s.at[(id - 10) as usize]) as i64,
        13..=15 => s.map_or(0, |s| s.target[(id - 13) as usize]) as i64,
        20..=22 => {
            l.constructor
                .base
                .as_ref()
                .unwrap_or(&l.definition.layout)
                .size[(id - 20) as usize] as i64
        }
        23 => l
            .constructor
            .workspace
            .observation_ticks
            .unwrap_or(l.definition.observation_ticks) as i64,
        24 => l.definition.editor_step as i64,
        25 => {
            l.constructor
                .base
                .as_ref()
                .unwrap_or(&l.definition.layout)
                .unit_resistance as i64
        }
        30..=32 => {
            let mat = l.constructor.material(&l.definition,l.recipe);
            l.constructor
                .workspace
                .material
                .iter()
                .find(|(m, f, _)| *m == mat && *f == id - 30)
                .map_or(0, |(_, _, v)| *v as i64)
        }
        _ => 0,
    }
}
fn step(id: u8) -> i64 {
    match id {
        0 => 100,
        1 => 100,
        6 => 1000,
        7 => 100,
        8 | 9 => 1024,
        20..=22 => 64,
        23 => 30,
        _ => 1,
    }
}
fn live_value(l: &Laboratory, scene: &Scene, id: u8) -> i64 {
    if (30..=32).contains(&id) {
        if let Some(m) = scene
            .sim
            .matter
            .economy
            .mats
            .get(l.constructor.material(&l.definition,l.recipe))
        {
            return match id {
                30 => m.strength as i64,
                31 => m.density as i64,
                _ => m.melt as i64,
            };
        }
    }
    get(l, id)
}
pub fn apply(l: &mut Laboratory, id: u8, value: i64, scene: &mut Scene) -> Result<bool, String> {
    if id>=60 {return scenario::set_number(l,id,value,scene);}
    if matches!(id,0..=15|40..=54) && !scene.sim.ready() {
        return Err(
            "Изменяйте параметры после завершения опыта; при необходимости нажмите «Сброс»".into(),
        );
    }
    if matches!(id, 20..=22 | 25) && !scene.paused {
        return Err("Для изменения карты поставьте опыт на паузу".into());
    }
    if matches!(id, 0..=15 | 40..=54) {
        let before = l.constructor.workspace.clone();
        let mut candidate = before.clone();
        let source = candidate
            .sources
            .get_mut(candidate.selected)
            .ok_or("Выберите свободное оружие")?;
        if id == 0 {
            if !(100..=100_000).contains(&value) {
                return Err("Мощность: от 0.1 до 100 ×".into());
            }
            source.power_milli = value as u32;
        } else if (10..=15).contains(&id) {
            if id <= 12 {
                source.at[(id - 10) as usize] =
                    i32::try_from(value).map_err(|_| "Координата вне диапазона")?;
                source.binding = None;
            } else {
                source.target[(id - 13) as usize] =
                    i32::try_from(value).map_err(|_| "Координата вне диапазона")?;
            }
        } else {
            // Editing a resolved property bakes its current multiplier once.
            if matches!(id, 1 | 6) {
                source.config = source.resolved();
                source.power_milli = 1000;
            }
            let c = &mut source.config;
            let n = u32::try_from(value).map_err(|_| "Нужно неотрицательное число")?;
            match id {
                1 => c.energy = n,
                2 => c.shell_speed = i32::try_from(n).map_err(|_| "Слишком большая скорость")?,
                3 => c.gravity = i32::try_from(n).map_err(|_| "Слишком большая гравитация")?,
                4 => c.charge_ticks = n,
                5 => c.heat_per_cell = n,
                6 => c.material_energy_j = n,
                7 => c.hull_impulse_milli = n,
                8 => c.wave_work_per_tick = n as usize,
                9 => c.edits_per_tick = n as usize,
                40 => c.heat_units = i32::try_from(n).map_err(|_| "Слишком большое значение")?,
                41 => {
                    c.gas_source_radius =
                        i32::try_from(n).map_err(|_| "Слишком большое значение")?
                }
                42 => {
                    c.gas_temperature_k =
                        i32::try_from(n).map_err(|_| "Слишком большое значение")?
                }
                43 => {
                    c.gas_fuel_milli = i32::try_from(n).map_err(|_| "Слишком большое значение")?
                }
                44 => {
                    c.gas_pressure_milli =
                        i32::try_from(n).map_err(|_| "Слишком большое значение")?
                }
                45 => c.cooling_milli = i32::try_from(n).map_err(|_| "Слишком большое значение")?,
                46 => {
                    c.reaction_milli = i32::try_from(n).map_err(|_| "Слишком большое значение")?
                }
                47 => {
                    c.buoyancy_milli = i32::try_from(n).map_err(|_| "Слишком большое значение")?
                }
                48 => c.soil_resistance = n,
                49 => c.concrete_resistance = n,
                50 => c.steel_resistance = n,
                51 => c.fragment_box = n,
                52 => c.half_cells = n as usize,
                53 => c.cell_units = n as i32,
                54 => c.capture_cells_per_tick = n as usize,
                _ => {}
            }
        }
        l.constructor.workspace = candidate;
        let layout = scene.sim.layout.as_ref().ok_or("Нет карты")?;
        let result = sources::active(l)
            .unwrap()
            .validate(layout, l.constructor.placed.len())
            .and_then(|_| sources::active(l).unwrap().resolved().validate())
            .and_then(|_| sources::arm(l, scene));
        if let Err(e) = result {
            l.constructor.workspace = before;
            return Err(e);
        }
        scenario::sync_sources(l);
        l.constructor.aim = None;
        l.constructor.revision += 1;
        return Ok(false);
    }
    if (20..=22).contains(&id) {
        let mut base = l.constructor.base_layout(&l.definition, l.recipe)?;
        let axis = (id - 20) as usize;
        let n = u32::try_from(value).map_err(|_| "Размер должен быть положительным")?;
        if n % 16 != 0
            || n < if axis == 1 { 128 } else { 256 }
            || n > if axis == 1 { 512 } else { 2048 }
        {
            return Err("Размер: кратный 16, стороны 256–2048, высота 128–512".into());
        }
        base.size[axis] = n;
        base.clear_max = [base.size[0] as i32 - 1, base.size[2] as i32 - 1];
        let layout = modding::laboratory::catalog::compose(&base, &l.constructor.placed)?;
        l.constructor
            .workspace
            .validate(&layout, l.constructor.placed.len())?;
        sim_core::blast::placement::Validator::default()
            .validate(&layout)
            .map_err(|e| e.to_string())?;
        // Retain the parameter stand as explicit geometry, avoiding duplicate fixtures.
        l.constructor.base = Some(base);
        l.constructor.stand = false;
    } else if id == 23 {
        if !(1..=18000).contains(&value) {
            return Err("Наблюдение: 1–18000 тиков".into());
        }
        l.constructor.workspace.observation_ticks = Some(value as u32);
        l.constructor.revision += 1;
        return Ok(false);
    } else if id == 24 {
        if !(1..=16).contains(&value) {
            return Err("Шаг привязки: 1–16".into());
        }
        l.definition.editor_step = value as i32;
        l.constructor.workspace.editor_step = Some(value as i32);
        return Ok(false);
    } else if id == 25 {
        if !(1..=1_000_000).contains(&value) {
            return Err("Стойкость корпуса: 1–1000000".into());
        }
        let mut base = l.constructor.base_layout(&l.definition, l.recipe)?;
        base.unit_resistance = value as u32;
        base.validate()?;
        l.constructor.base = Some(base);
        l.constructor.stand = false;
    } else if (30..=32).contains(&id) {
        let material = l.constructor.material(&l.definition,l.recipe);
        let field = match id {
            30 => MaterialField::Strength,
            31 => MaterialField::Density,
            _ => MaterialField::Melt,
        };
        let value = u32::try_from(value).map_err(|_| "Нужно положительное число")?;
        if value == 0 || value > 1_000_000 {
            return Err("Параметр материала: 1–1000000".into());
        }
        if !scene
            .sim
            .execute_command(Command::TuneMaterial {
                material,
                field,
                value,
            })
            .admitted()
        {
            return Err("Движок отклонил значение материала".into());
        }
        l.constructor
            .workspace
            .material
            .retain(|(m, f, _)| *m != material || *f != id - 30);
        l.constructor
            .workspace
            .material
            .push((material, id - 30, value));
        if id == 30 {
            l.constructor.strength = Some((material, value));
        }
        l.constructor.revision += 1;
        return Ok(false);
    }
    l.constructor.revision += 1;
    Ok(true)
}
pub fn action(l: &mut Laboratory, a: LabAction, scene: &mut Scene) -> Result<bool, String> {
    match a {
        LabAction::EditNumber(id) => {
            l.ui.editing = Some((id, format_number(id, live_value(l, scene, id)), true));
            l.ui.edit_error = None;
            Ok(false)
        }
        LabAction::CancelNumber => {
            l.ui.editing = None;
            l.ui.edit_error = None;
            Ok(false)
        }
        LabAction::ApplyNumber => {
            let Some((id, text, _)) = l.ui.editing.clone() else {
                return Ok(false);
            };
            let result = parse_number(id, &text).and_then(|n| apply(l, id, n, scene));
            match result {
                Ok(changed) => {
                    l.ui.editing = None;
                    l.ui.edit_error = None;
                    l.notice = "Значение применено".into();
                    Ok(changed)
                }
                Err(e) => {
                    l.ui.edit_error = Some(e.clone());
                    Err(e)
                }
            }
        }
        LabAction::AdjustNumber(id, delta) => apply(
            l,
            id,
            live_value(l, scene, id) + step(id) * delta as i64,
            scene,
        ),
        LabAction::SetNumber(id, n) => apply(l, id, n, scene),
        _ => Ok(false),
    }
}
fn format_number(id: u8, n: i64) -> String {
    if id == 0 {
        format!("{:.3}", n as f64 / 1000.0)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .into()
    } else {
        n.to_string()
    }
}
fn parse_number(id: u8, text: &str) -> Result<i64, String> {
    let n = text
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .map_err(|_| "Введите число")?;
    if !n.is_finite() || (n < 0.0 && !matches!(id,64|65)) || n < -1.0 || n > u32::MAX as f64 {
        return Err("Нужно неотрицательное конечное число".into());
    }
    let value = n * if id == 0 { 1000.0 } else { 1.0 };
    if (value - value.round()).abs() > 0.000001 {
        return Err(if id == 0 {
            "Мощность: не более трёх знаков после запятой"
        } else {
            "Для этого параметра нужно целое число"
        }
        .into());
    }
    Ok(value.round() as i64)
}
pub fn keyboard(l: &mut Laboratory, keys: &mut ButtonInput<KeyCode>) {
    let Some((id, mut text, mut selected)) = l.ui.editing.take() else {
        return;
    };
    let mut keep = true;
    let control = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    for key in keys.get_just_pressed() {
        match key {
            KeyCode::Escape => {
                keep = false;
                l.ui.edit_error = None;
                break;
            }
            KeyCode::Enter | KeyCode::NumpadEnter => {
                l.constructor.pending.push(LabAction::ApplyNumber);
                break;
            }
            KeyCode::KeyA if control => selected = true,
            KeyCode::Backspace => {
                l.ui.edit_error = None;
                if selected {
                    text.clear();
                    selected = false;
                } else {
                    text.pop();
                }
            }
            key => {
                if let Some(c) = knobs::typed(*key) {
                    l.ui.edit_error = None;
                    if selected || text.len() < 20 {
                        if selected {
                            text.clear();
                            selected = false;
                        }
                        text.push(c);
                    }
                }
            }
        }
    }
    if keep {
        l.ui.editing = Some((id, text, selected));
    }
    keys.clear();
}
pub fn sync(
    lab: Res<Laboratory>,
    scene: Res<Scene>,
    mut numbers: Query<(&Number, &mut Text), (Without<SourceName>, Without<Info>)>,
    mut rows: Query<(&SourceRow, &mut Node, &mut BorderColor)>,
    mut names: Query<(&SourceName, &mut Text), (Without<Number>, Without<Info>)>,
    mut infos: Query<(&Info, &mut Text, &mut TextColor), (Without<Number>, Without<SourceName>)>,
) {
    if !lab.active() {
        return;
    }
    for (Number(id), mut text) in &mut numbers {
        let mut value = lab
            .ui
            .editing
            .as_ref()
            .filter(|(i, _, _)| i == id)
            .map_or_else(
                || format_number(*id, get(&lab, *id)),
                |(_, t, _)| format!("{t} |"),
            );
        if (30..=32).contains(id) && value == "0" {
            if let Some(m) = scene
                .sim
                .matter
                .economy
                .mats
                .get(lab.constructor.material(&lab.definition,lab.recipe))
            {
                value = match id {
                    30 => m.strength.to_string(),
                    31 => m.density.to_string(),
                    _ => m.melt.to_string(),
                };
            }
        }
        if text.0 != value {
            **text = value;
        }
    }
    for (SourceRow(i), mut node, mut border) in &mut rows {
        let display = if *i < lab.constructor.workspace.sources.len() {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
        let color = if *i == lab.constructor.workspace.selected {
            theme::ACCENT
        } else {
            theme::FRAME
        };
        if *border != BorderColor::all(color) {
            *border = BorderColor::all(color);
        }
    }
    for (SourceName(i), mut text) in &mut names {
        let value = lab
            .constructor
            .workspace
            .sources
            .get(*i)
            .map_or(String::new(), |s| {
                format!(
                    "{} · {:.2}× · {}",
                    s.name,
                    s.power_milli as f64 / 1000.0,
                    if s.detonate {
                        "заряд"
                    } else if s.binding.is_some() {
                        "юнит"
                    } else {
                        "точка"
                    }
                )
            });
        if text.0 != value {
            **text = value;
        }
    }
    for (role, mut text, mut colour) in &mut infos {
        let value = match role {
            Info::Trigger=>if sources::active(&lab).is_some_and(|s|s.detonate) {"Взрыв в точке заряда. Полёт и время зарядки не используются."}else{"Выстрел из источника по цели. T — выбрать цель."}.into(),
            Info::Pause => if scene.paused { "Продолжить" } else { "Пауза" }.into(),
            Info::Source => sources::active(&lab).map_or(lab.constructor.experiment.as_ref().map_or("Выберите воздействие в библиотеке снизу".into(),|p|format!("Готовый опыт: {}",p.name)), |s| format!("Выбран: {}\n{}", s.name, if scene.sim.ready() { "Готов к пуску · F" } else { "Воздействие идёт · дождитесь или сбросьте" })),
            Info::Binding => sources::active(&lab).map_or(String::new(), |s| {
                let position = sources::position(&lab, s, &scene.sim);
                let binding = s.binding.and_then(|i| lab.constructor.placed.get(i).map(|p| format!("Ствол: {} · {}", i + 1, p.element.name))).unwrap_or(if s.detonate {"Локальный заряд · взрыв в этой точке"}else{"Свободная точка без модели"}.into());
                format!("{binding}\nСейчас: X {} · Y {} · Z {}", position[0], position[1], position[2])
            }),
            Info::Edit => lab.ui.edit_error.clone().unwrap_or_else(|| if lab.ui.editing.as_ref().is_some_and(|(id,_,_)| *id == 0) { "Мощность: 0.1–100× · до 3 знаков после запятой\nEnter — применить · Esc — отмена · Ctrl+A — всё".into() } else { "Введите целое число\nEnter — применить · Esc — отмена · Ctrl+A — всё".into() }),
        };
        if text.0 != value {
            **text = value;
        }
        let value = if matches!(role, Info::Edit) && lab.ui.edit_error.is_some() {
            Color::srgb(1.0, 0.5, 0.4)
        } else {
            theme::MUTED
        };
        if colour.0 != value {
            colour.0 = value;
        }
    }
}
