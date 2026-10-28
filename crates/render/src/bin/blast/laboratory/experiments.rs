//! UI adapters: every experiment input becomes an existing engine command.
use super::*;
use modding::laboratory::experiments::{Input, Plan};
use sim_core::blast::battle::{commands::BattleLifecycle, Order};

#[derive(Component)]
pub struct Menu;

pub fn commands(plan: &Plan) -> Vec<Command> {
    plan.inputs
        .iter()
        .map(|input| match *input {
            Input::Fire => Command::Gun(GunCommand::Refire),
            Input::Detonate {at}=>Command::Gun(GunCommand::Detonate {at}),
            Input::FreeControl => Command::BattleLifecycle(BattleLifecycle::FreeControl),
            Input::StartBattle => Command::BattleLifecycle(BattleLifecycle::StartBattle),
            Input::Move { actor, to } => Command::Order {
                actor,
                order: Order::Move,
                destination: Some(to),
            },
            Input::Dig { actor, at } => Command::Order {
                actor,
                order: Order::Dig(at),
                destination: None,
            },
            Input::Ground { actor, at } => Command::Order {
                actor,
                order: Order::Ground(at),
                destination: None,
            },
            Input::Pilot {
                actor,
                throttle,
                turn,
                aim,
                fire,
            } => Command::Order {
                actor,
                order: Order::Pilot {
                    throttle,
                    turn,
                    aim,
                    fire,
                },
                destination: None,
            },
        })
        .collect()
}
pub fn launch(plan: &Plan, sim: &mut BlastRange) -> Result<(), String> {
    plan.validate(sim.layout.as_ref().ok_or("missing laboratory layout")?)?;
    for command in commands(plan) {
        let outcome = sim.execute_command(command);
        if !outcome.admitted() || outcome == sim_core::blast::battle::commands::Outcome::Busy {
            return Err(format!(
                "Движок отклонил воздействие: {outcome:?}; восстановите опыт"
            ));
        }
    }
    Ok(())
}
pub fn select(lab: &mut Laboratory, index: usize) -> Result<(), String> {
    if lab.constructor.creating() {
        return Err("Сначала завершите свой элемент".into());
    }
    let e = lab.experiments.get(index).ok_or("unknown experiment")?;
    sim_core::blast::placement::Validator::default()
        .validate(&e.layout)
        .map_err(|e| e.to_string())?;
    let parts = modding::laboratory::catalog::parts_of_layout(e.layout.clone())?;
    let mut base = e.layout.clone();
    base.blocks.clear();
    base.units.clear();
    base.unit_yaws.clear();
    base.shields.clear();
    lab.constructor.remember_scene(lab.recipe);
    lab.constructor.base = Some(base);
    lab.constructor.workspace=Default::default();
    lab.constructor.workspace.devices=e.layout.lab_devices.clone();
    lab.constructor.placed = parts;
    lab.constructor.selected = None;
    lab.constructor.aim = None;
    lab.constructor.stand = false;
    lab.constructor.weapon = None;
    lab.constructor.variant = None;
    lab.constructor.strength = None;
    lab.constructor.experiment = Some(e.plan.clone());
    lab.constructor.mode = constructor::Mode::Select;
    lab.constructor.revision += 1;
    lab.recipe = Recipe::default();
    lab.recipe.effect = e.weapon;
    lab.constructor.workspace.observation_ticks=Some(e.plan.duration_ticks);
    lab.ui.scenario_input=0;lab.ui.device=0;
    scenario::materialize(lab)?;
    lab.browser = false;
    lab.ui.tab=if lab.constructor.workspace.sources.is_empty(){24}else{10};
    lab.notice = "Сценарий открыт для редактирования: объекты, источники и команды. F — запустить.".into();
    Ok(())
}
pub fn build(commands: &mut Commands, camera: Entity, font: &TextFont) {
    commands.spawn((root(Node {position_type:PositionType::Absolute,left:px(262),top:px(134),
        bottom:px(282),width:px(520),padding:UiRect::all(px(12)),row_gap:px(8),
        flex_direction:FlexDirection::Column,overflow:Overflow::scroll_y(),display:Display::None,
        border:UiRect::all(px(1)),..default()},camera),Menu,panel::ScrollPanel,ScrollPosition::default()))
        .with_children(|p| {
            label(p,font,"ФИЗИЧЕСКИЕ ОПЫТЫ · общий движок",None);
            for (i,e) in modding::laboratory::experiments::stock().iter().enumerate() {
                row(p,font,&[(&e.plan.name,LabAction::Experiment(i))]);
            }
            label(p,font,"Газ, дым и свет — общие эффекты рендера.\nВода, намокание, плавучесть, осыпание и тепловые превращения — в общей симуляции.",None);
            row(p,font,&[("Закрыть",LabAction::Experiments)]);
        });
}
pub fn readout(sim: &BlastRange) -> String {
    let mined = sim.mining().map_or(0, |m| m.cells);
    let (moved,pushed,pending,_) = sim.material_flow();
    let water=sim.water.stats();
    let mut text=format!("Вода: {:.2} / {:.2} ед. объёма · активных ячеек: {}\nВпитано: {:.2} · пар: {:.2}\nОсыпание: {} клеток · сдвинуто при ударе: {} · активных участков: {}\nОгонь: {} · нагретых клеток корпуса: {} · передано тепла: {}\nЩиты: {}/{} · попаданий в щиты: {} · добыто клеток: {} · баланс материи: {}",
        water.units as f64/sim.water.tuning.units_per_voxel as f64,water.initial_units as f64/sim.water.tuning.units_per_voxel as f64,water.active,water.absorbed_units as f64/sim.water.tuning.units_per_voxel as f64,water.vapor_units as f64/sim.water.tuning.units_per_voxel as f64,moved,pushed,pending,sim.heat.burning(),sim.thermals.hot(),sim.heat_exchange.transferred,
        sim.shields.iter().filter(|s|s.standing()).count(),sim.shields.len(),sim.shields.iter().map(|s|u64::from(s.hits)).sum::<u64>(),
        mined,if sim.matter.economy.ledger.balanced(){"сходится"}else{"НАРУШЕН"});
    for (i, unit) in sim.units.iter().enumerate().filter(|(_,u)| u.kind == RangeUnitKind::Crawler) {
        if let Some((held, capacity)) = sim.tank_fill(i) {
            let (stored, limit) = sim.side_store(sim.allied(i)).unwrap_or((0, 0));
            text.push_str(&format!("\nКраулер {} · {}\nБак: {} / {} · запас базы: {} / {}", i+1,
                if !unit.status.alive {"УНИЧТОЖЕН"} else if sim.hauling(i) {"ДОСТАВКА НА БАЗУ"} else {"ДОБЫЧА / РЕКЛЕЙМ"},
                held, capacity, stored, limit));
        }
    }
    for (i,d) in sim.layout.as_ref().into_iter().flat_map(|l|&l.lab_devices).enumerate() {
        if let Some(c)=d.collector.as_ref() {
            text.push_str(&format!("\nКраулер {} · {}\nБак: {} / {} · склад: {}\nСобрано: {} обломков · сдано: {} · рейсов: {}",i+1,
                if c.returning{"РАЗГРУЗКА / ВОЗВРАТ"}else if c.spent>0{"СБОР"}else if c.target.is_some(){"ЕДЕТ К ОБЛОМКАМ"}else{"ПОИСК"},
                d.container.map_or(0,|id|sim.matter.economy.ledger.held(id)),c.capacity,c.depot.map_or(0,|id|sim.matter.economy.ledger.held(id)),c.pieces,c.delivered,c.trips));
        }
    }
    text
}
pub fn weapon_label(lab: &Laboratory) -> Option<String> {
    let plan = lab.constructor.experiment.as_ref()?;
    if plan.inputs.iter().any(|i|matches!(i,Input::Fire|Input::Detonate{..})) {
        return None;
    }
    let actor_weapon = plan
        .inputs
        .iter()
        .any(|i| matches!(i, Input::Ground { .. } | Input::Pilot { fire: true, .. }));
    Some(format!(
        "{}\nОкно наблюдения: {} с",
        if actor_weapon {
            "Штатное оружие участника. Цель и участник меняются в командах сценария."
        } else {
            "Ход общей физики без выстрела. Источник можно добавить в сценарий."
        },
        plan.duration_ticks / sim_core::TICKS_PER_SECOND as u32
    ))
}
