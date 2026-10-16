//! Exercise the actual presentation controller with ordinary SIM commands.
use super::*;
use modding::range_skirmish::{self, MissionGoal};
use sim_core::blast::{
    battle::{Order, Phase},
    read_model::View,
};

fn setup(id: usize) -> (BlastRange, MissionRun) {
    let preset = range_skirmish::stock(id).unwrap();
    let base = RangeLayout::from_ron(include_str!("../../../../../../data/range-km.ron")).unwrap();
    let cfg =
        ExplosionConfig::from_ron(include_str!("../../../../../../data/explosion.ron")).unwrap();
    let mut sim = BlastRange::with_layout(cfg, preset.apply_to(base).unwrap()).unwrap();
    sim.set_physics_workers(4);
    sim.set_navigation_workers(8);
    sim.start_battle().unwrap();
    let mut run = MissionRun::new(preset.mission.unwrap());
    run.begin(&mut sim, 1);
    (sim, run)
}

#[test]
fn mission_terminal_result_does_not_leave_an_unreachable_waypoint() {
    let (mut sim, mut run) = setup(6);
    let mut view = View::new(&sim, 1);
    for unit in view.units.iter_mut().filter(|u| !u.allied) {
        unit.status.alive = false;
    }
    view.phase = Phase::Victory;
    run.update(&view);
    assert!(run.failure.is_some());
    assert!(run.readout().contains("РУБЕЖ НЕ ДОСТИГНУТ"));
    assert!(run.waypoint().is_none());
    // The final publication can contain arrival and victory together.
    run.begin(&mut sim, 1);
    let (point, _) = run.waypoint().unwrap();
    view.units[0].position[0] = fx::Fx::from_int(point[0]);
    view.units[0].position[2] = fx::Fx::from_int(point[1]);
    run.update(&view);
    assert!(run.failure.is_none());
    assert_eq!(run.step, run.definition.steps.len());
    assert!(run.readout().contains("ВЫПОЛНЕНА"));
    run.begin(&mut sim, 2);
    assert!(run.failure.is_none());
    assert_eq!(run.step, 0);
    view.generation = 2;
    view.phase = Phase::Defeat;
    view.units[0].status.alive = false;
    view.units[1].status.alive = false;
    run.update(&view);
    assert!(run.failure.is_some());
    assert!(run.waypoint().is_none());
}

#[test]
fn mission_protection_remains_active_until_in_flight_combat_finishes() {
    let (mut sim, mut run) = setup(13);
    let mut view = View::new(&sim, 1);
    let MissionGoal::Reach { actors, point, .. } = run.definition.steps[0].goal.clone() else {
        panic!()
    };
    for id in actors {
        view.units[id].position[0] = fx::Fx::from_int(point[0]);
        view.units[id].position[2] = fx::Fx::from_int(point[1]);
    }
    for unit in view.units.iter_mut().filter(|u| !u.allied) {
        unit.status.alive = false;
    }
    run.update(&view);
    assert_eq!(run.step, run.definition.steps.len());
    assert!(!run.complete);
    assert!(run.readout().contains("дождитесь завершения боя"));
    view.units[run.definition.protect[0]].status.alive = false;
    run.update(&view);
    assert!(run.readout().contains("потерян защищаемый объект"));
    assert!(!run.complete);
    // Even without a protected actor, mutually lethal final shots are a draw.
    run.begin(&mut sim, 1);
    run.definition.protect.clear();
    view.phase = Phase::Draw;
    run.update(&view);
    assert!(run.failure.is_some());
    assert!(!run.complete);
}

#[test]
fn mission_publication_clears_foreign_world_and_preserves_queued_replay() {
    let mut source = hud::tests::app();
    let mut scene = source.world_mut().remove_resource::<Scene>().unwrap();
    let mut run = MissionRun::new(range_skirmish::stock(6).unwrap().mission.unwrap());
    run.begin(&mut scene.sim, scene.generation);
    scene.generation += 1;
    let mut panel = Panel::new(true, false);
    panel.mission = Some(run);
    panel.briefing = "Previous world's briefing".into();
    panel.battle_launch = true;
    let mut app = App::new();
    app.insert_resource(scene)
        .insert_resource(panel)
        .init_resource::<super::super::ui_snapshot::UiSnapshot>()
        .add_systems(Update, super::super::ui_snapshot::publish);
    app.update();
    assert!(app.world().resource::<Panel>().mission.is_some());
    app.world_mut().resource_mut::<Panel>().battle_launch = false;
    app.update();
    let panel = app.world().resource::<Panel>();
    assert!(panel.mission.is_none());
    assert!(panel.briefing.is_empty());
}

fn attack(sim: &mut BlastRange, actors: &[usize], target: usize) {
    for &id in actors {
        if sim.units[id].status.alive {
            sim.battle_order(id, Order::Attack(target), None);
        }
    }
}
fn move_group(sim: &mut BlastRange, actors: &[usize], point: [i32; 2]) {
    let slots = sim.battle_formation_destinations(actors, point);
    assert!(sim.battle_group_move(&slots, point) > 0);
}

#[test]
#[ignore = "full twelve-mission playthroughs; run explicitly in release"]
fn mission_stock_playthroughs_use_real_orders_and_reach_victory() {
    for id in 6..range_skirmish::STOCK.len() {
        let (mut sim, mut run) = setup(id);
        let own: Vec<_> = (0..sim.units.len()).filter(|&i| sim.allied(i)).collect();
        for _ in 0..60 {
            sim.step();
        }
        assert!(sim.battle.actors.iter().all(|a| a.fired == 0), "start {id}");
        let MissionGoal::Reach { actors, point, .. } = run.definition.steps[0].goal.clone() else {
            panic!("deployment first")
        };
        move_group(&mut sim, &actors, point);
        if id >= 13 {
            for &i in &own {
                if !actors.contains(&i) {
                    sim.battle_order(i, Order::Auto, None);
                }
            }
        }
        if id == 16 {
            attack(&mut sim, &[0], 4);
        }
        if id == 17 {
            move_group(&mut sim, &[4], [5660, 3400]);
        }
        if id == 12 {
            attack(&mut sim, &[6, 7], 11);
        }
        let mut view = View::new(&sim, 1);
        let mut previous_step = usize::MAX;
        let mut target = None;
        let mut flank = false;
        while sim.tick < 9000 && sim.battle.phase == Phase::Fighting {
            view.update(&sim, 1);
            run.update(&view);
            assert!(
                run.failure.is_none(),
                "mission {id}, tick {}: {}",
                sim.tick,
                run.readout()
            );
            if run.step != previous_step {
                println!("mission={id} tick={} step={}", sim.tick, run.step);
                previous_step = run.step;
                target = None;
            }
            if let Some(range_skirmish::MissionStep {
                goal: MissionGoal::Destroy { actors: foes },
                ..
            }) = run.definition.steps.get(run.step)
            {
                let next = foes.iter().copied().find(|&i| sim.units[i].status.alive);
                if next != target {
                    target = next;
                    if let Some(t) = next {
                        if id == 8 && !flank {
                            move_group(&mut sim, &own, [6040, 3320]);
                            flank = true;
                        } else if id == 10 {
                            for (n, &i) in own.iter().enumerate() {
                                let point = [
                                    5240,
                                    sim.units[t].motion.pos[2].to_int_trunc() + (n as i32 - 1) * 80,
                                ];
                                sim.battle_order(i, Order::Move, Some(point));
                            }
                        } else if (13..=15).contains(&id) {
                            for &i in &own {
                                sim.battle_order(i, Order::Auto, None);
                            }
                        } else if id == 17 {
                            attack(&mut sim, &[4], 9);
                            attack(&mut sim, &[1, 2, 3, 5], t);
                        } else {
                            attack(&mut sim, &own, t);
                        }
                    }
                }
            }
            sim.step();
            if sim.tick % 600 == 0 {
                println!(
                    "mission={id} tick={} counts={:?}",
                    sim.tick,
                    sim.battle_counts()
                );
            }
        }
        view.update(&sim, 1);
        run.update(&view);
        println!("RESULT mission={id} tick={} step={} phase={:?} own={} fired={:?} balanced={} hash={:016x}", sim.tick, run.step, sim.battle.phase, sim.battle_counts()[0], sim.battle.actors.iter().map(|a|a.fired).collect::<Vec<_>>(), sim.matter.economy.ledger.balanced(), sim.state_hash());
        assert_eq!(sim.battle.phase, Phase::Victory, "mission {id}");
        assert!(run.failure.is_none(), "mission {id}: {}", run.readout());
        assert_eq!(run.step, run.definition.steps.len(), "mission {id}");
        assert!(run.complete, "mission {id}");
        assert!(
            run.definition
                .protect
                .iter()
                .all(|&i| sim.units[i].status.alive),
            "mission {id}"
        );
        assert!(
            own.iter()
                .any(|&i| sim.units[i].kind == run.definition.focus
                    && sim.battle.actors[i].fired > 0)
                || run.definition.focus == RangeUnitKind::Base,
            "focus weapon unused {id}"
        );
        assert!(sim.matter.economy.ledger.balanced(), "mission {id}");
    }
}
