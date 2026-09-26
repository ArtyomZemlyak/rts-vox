use super::ai::{
    AiOrder, Decision, Observation, Policy, Rules, UnitObservation, OBSERVATION_VERSION,
};
use super::*;
use modding::range::RangeUnitDef;

fn arena(kind: RangeUnitKind) -> BlastRange {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    l.battle = true;
    l.size = [640, 192, 640];
    l.blocks.clear();
    l.shields.clear();
    l.course = None;
    l.clear_min = [64, 64];
    l.clear_max = [600, 600];
    l.units = vec![
        RangeUnitDef {
            kind,
            at: [170, 16, 336],
            allied: true,
        },
        RangeUnitDef {
            kind,
            at: [450, 16, 336],
            allied: false,
        },
    ];
    BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        l,
    )
    .unwrap()
}

#[test]
fn enemy_rules_focus_fire_and_switch_only_for_a_better_target() {
    let mut tuning =
        modding::battle_ai::Tuning::from_ron(include_str!("../../../data/battle-ai.ron")).unwrap();
    tuning.switch_margin = 20;
    let mut observation = Observation {
        version: OBSERVATION_VERSION,
        observed_tick: 20,
        decision_tick: 30,
        size: [640, 192, 640],
        units: vec![
            UnitObservation {
                id: 0,
                allied: true,
                alive: true,
                armed: true,
                mobile: true,
                kind: RangeUnitKind::Tank,
                position: [100, 100],
                ordered_target: None,
            },
            UnitObservation {
                id: 1,
                allied: true,
                alive: true,
                armed: false,
                mobile: true,
                kind: RangeUnitKind::Tank,
                position: [350, 100],
                ordered_target: None,
            },
            UnitObservation {
                id: 2,
                allied: false,
                alive: true,
                armed: true,
                mobile: true,
                kind: RangeUnitKind::Tank,
                position: [230, 100],
                ordered_target: Some(1),
            },
            UnitObservation {
                id: 3,
                allied: false,
                alive: true,
                armed: true,
                mobile: true,
                kind: RangeUnitKind::Tank,
                position: [250, 100],
                ordered_target: Some(1),
            },
        ],
    };
    let rules = Rules { tuning: &tuning };
    assert_eq!(
        rules.decide(&observation),
        vec![
            Decision {
                actor: 2,
                order: AiOrder::Attack(0)
            },
            Decision {
                actor: 3,
                order: AiOrder::Attack(0)
            }
        ]
    );
    observation.units[0].alive = false;
    assert_eq!(
        rules.decide(&observation),
        vec![
            Decision {
                actor: 2,
                order: AiOrder::Attack(1)
            },
            Decision {
                actor: 3,
                order: AiOrder::Attack(1)
            }
        ]
    );
    observation.units[0].alive = true;
    observation.units[2].ordered_target = Some(0);
    observation.units[3].ordered_target = Some(0);
    observation.units[0].position = [265, 100];
    assert_eq!(rules.decide(&observation)[0].order, AiOrder::Attack(0));
}

#[test]
fn external_ai_plan_is_validated_scheduled_and_saved() {
    let mut r = arena(RangeUnitKind::Tank);
    assert!(r.enemy_ai_observation().is_none());
    r.start_battle().unwrap();
    let observation = r.enemy_ai_observation().unwrap();
    assert_eq!(observation.version, OBSERVATION_VERSION);
    assert_eq!(observation.decision_tick, r.tick);
    assert!(r
        .submit_enemy_ai_plan(
            observation.decision_tick,
            vec![Decision {
                actor: 0,
                order: AiOrder::Hold
            }]
        )
        .is_err());
    assert!(r
        .submit_enemy_ai_plan(
            observation.decision_tick,
            vec![Decision {
                actor: 1,
                order: AiOrder::Attack(1)
            }]
        )
        .is_err());
    let plan = vec![Decision {
        actor: 1,
        order: AiOrder::Hold,
    }];
    r.submit_enemy_ai_plan(observation.decision_tick, plan.clone())
        .unwrap();
    assert!(r
        .submit_enemy_ai_plan(observation.decision_tick, plan)
        .is_err());
    let mut restored = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    assert_eq!(r.state_hash(), restored.state_hash());
    r.step();
    restored.step();
    assert_eq!(r.state_hash(), restored.state_hash());
    assert_eq!(r.battle.actors[1].order, Order::Hold);
    assert!(r
        .submit_enemy_ai_plan(
            observation.decision_tick,
            vec![Decision {
                actor: 1,
                order: AiOrder::Auto
            }]
        )
        .is_err());
    for _ in 0..r.battle.ai.tuning.decision_ticks {
        r.step();
        restored.step();
        assert_eq!(r.state_hash(), restored.state_hash());
    }
    assert_eq!(
        r.battle.actors[1].order,
        Order::Attack(0),
        "missing model plan must fall back to rules"
    );
}

#[test]
fn external_move_uses_the_regular_pathfinder_and_replays() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    let tick = r.enemy_ai_observation().unwrap().decision_tick;
    assert!(r
        .submit_enemy_ai_plan(
            tick,
            vec![Decision {
                actor: 1,
                order: AiOrder::Move([-1, 400]),
            }],
        )
        .is_err());
    r.submit_enemy_ai_plan(
        tick,
        vec![Decision {
            actor: 1,
            order: AiOrder::Move([480, 420]),
        }],
    )
    .unwrap();
    r.step();
    assert_eq!(r.battle.actors[1].order, Order::Move);
    assert!(r.units[1].motion.destination.is_some());
    let mut restored = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    for _ in 0..3 {
        r.step();
        restored.step();
        assert_eq!(r.state_hash(), restored.state_hash());
    }
}

#[test]
fn custom_enemy_rules_survive_snapshot() {
    let mut r = arena(RangeUnitKind::Tank);
    let mut tuning = r.battle.ai.tuning.clone();
    tuning.decision_ticks = 8;
    tuning.focus_fire = false;
    assert!(r.configure_enemy_ai(tuning.clone()).is_ok());
    assert_eq!(r.battle.ai.tuning, tuning);
    let restored = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    assert_eq!(restored.battle.ai.tuning, tuning);
    assert_eq!(restored.state_hash(), r.state_hash());
    tuning.decision_ticks = 0;
    assert!(r.configure_enemy_ai(tuning).is_err());
}
#[test]
fn setup_is_quiet_and_commands_reject_enemy_and_friendly_fire_orders() {
    let mut r = arena(RangeUnitKind::Tank);
    for _ in 0..60 {
        r.step();
    }
    assert!(r.battle.rounds.is_empty());
    assert_eq!(r.battle.actors[0].fired, 0);
    assert!(!r.battle_order(0, Order::Attack(1), None));
    r.start_battle().unwrap();
    assert!(!r.battle_order(1, Order::Move, Some([400, 400])));
    assert!(!r.battle_order(0, Order::Attack(0), None));
    assert!(r.battle_order(0, Order::Attack(1), None));
    assert_eq!(r.battle.phase, Phase::Fighting);
}
#[test]
fn both_tanks_fire_and_take_real_spatial_damage() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    for _ in 0..750 {
        r.step();
    }
    assert!(
        r.battle.actors.iter().all(|a| a.fired > 0),
        "shots {:?}",
        r.battle.actors.iter().map(|a| a.fired).collect::<Vec<_>>()
    );
    assert!(
        r.units.iter().all(|u| u.revision > 0),
        "damage {:?}",
        r.units.iter().map(|u| u.revision).collect::<Vec<_>>()
    );
    assert!(r.matter.economy.ledger.balanced());
    assert!(r.battle.waves.len() <= r.battle.tuning.max_waves);
}
/// **A lobbed shell over the world ceiling comes back down.** The firing
/// solution rises `g·T²/8` above the muzzle; past the ceiling the round used
/// to retire as a miss, so every target in the far part of a tank's reach
/// was out of reach. A low ceiling and a long lane reproduce it: the tanks
/// stand 460 apart, and the shell tops out about 150 in a 128-high world.
#[test]
fn a_shell_lobbed_over_the_ceiling_still_lands() {
    let mut l = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    l.battle = true;
    l.size = [640, 128, 640];
    l.blocks.clear();
    l.shields.clear();
    l.course = None;
    l.clear_min = [64, 64];
    l.clear_max = [600, 600];
    l.units = [(100, true), (560, false)]
        .map(|(x, allied)| RangeUnitDef {
            kind: RangeUnitKind::Tank,
            at: [x, 16, 336],
            allied,
        })
        .to_vec();
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        l,
    )
    .unwrap();
    r.start_battle().unwrap();
    let mut above = false;
    for _ in 0..750 {
        r.step();
        above |= r
            .battle
            .rounds
            .iter()
            .any(|round| round.shell && round.pos[1] >= Fx::from_int(128));
    }
    assert!(
        above,
        "no shell rose over the ceiling: the lane is too short"
    );
    assert!(
        r.units.iter().all(|u| u.revision > 0),
        "a shell over the ceiling never landed: damage {:?}",
        r.units.iter().map(|u| u.revision).collect::<Vec<_>>()
    );
    let mut b = BlastRange::restore(r.config.clone(), &r.snapshot()).unwrap();
    for _ in 0..60 {
        r.step();
        b.step();
    }
    assert_eq!(r.state_hash(), b.state_hash());
}

#[test]
fn dual_machine_guns_keep_owner_and_survive_stop() {
    let mut r = arena(RangeUnitKind::Strider);
    r.start_battle().unwrap();
    for _ in 0..15 {
        r.step();
    }
    let fired = r.battle.actors[0].fired;
    assert!(fired > 0);
    assert!(r.battle_order(0, Order::Hold, None));
    for _ in 0..150 {
        r.step();
    }
    assert_eq!(r.battle.actors[0].fired, fired);
    assert!(r.units[0].revision > 0, "enemy never shot back");
    assert!(
        r.units[1].revision > 0,
        "own bullets disappeared after stop"
    );
    assert!(r.matter.economy.ledger.balanced());
}
#[test]
fn replay_with_multiple_projectiles_and_waves() {
    let mut a = arena(RangeUnitKind::Tank);
    a.start_battle().unwrap();
    for _ in 0..180 {
        a.step();
        if !a.battle.rounds.is_empty() {
            break;
        }
    }
    assert!(!a.battle.rounds.is_empty());
    let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
    assert_eq!(a.state_hash(), b.state_hash());
    for _ in 0..150 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash());
    }
}
#[test]
fn cover_blocks_enemy_machine_gun_and_winner_is_reported() {
    let mut r = arena(RangeUnitKind::Strider);
    for x in 295..305 {
        for y in 16..110 {
            for z in 260..410 {
                r.world.queue_edit(voxel::Edit::put(x, y, z, 3));
            }
        }
    }
    while r.world.queued() > 0 {
        r.world.flush();
    }
    r.start_battle().unwrap();
    for _ in 0..150 {
        r.step();
    }
    assert!(
        r.units.iter().all(|u| u.revision == 0),
        "bullets ignored cover"
    );
    r.units[1].status.alive = false;
    r.battle.rounds.clear();
    r.battle_tick();
    assert_eq!(r.battle.phase, Phase::Victory);
}
#[test]
fn manual_move_preserves_identity_and_actually_drives() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    let before = r.units[0].at;
    r.battle.actors[1].order = Order::Hold;
    assert!(r.battle_order(0, Order::Move, Some([230, 430])));
    for _ in 0..180 {
        r.step();
    }
    assert!(r.units[0].at[2] > before[2] + 20);
    assert!(r.allied(0));
    assert!(!r.allied(1));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn simultaneous_impacts_keep_both_waves_and_replay() {
    let mut a = arena(RangeUnitKind::Tank);
    a.start_battle().unwrap();
    for actor in &mut a.battle.actors {
        actor.order = Order::Hold;
    }
    for owner in 0..2 {
        let u = &a.units[1 - owner];
        let p = u
            .world_pos([
                u.body.core.0 as usize,
                u.body.core.1 as usize,
                u.body.core.2 as usize,
            ])
            .map(Fx::from_int);
        a.battle.rounds.push(Round {
            owner,
            shell: true,
            pos: p,
            previous: p,
            velocity: [Fx::ZERO; 3],
            distance: Fx::from_int(100),
        });
    }
    a.step();
    assert_eq!(a.battle.waves.len(), 2, "one impact replaced another");
    let mut b = BlastRange::restore(a.config.clone(), &a.snapshot()).unwrap();
    for _ in 0..50 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash());
    }
    assert!(a.units.iter().all(|u| u.revision > 0));
    assert!(a.matter.economy.ledger.balanced());
}

#[test]
fn rotated_turret_cells_are_hit_where_they_are_drawn() {
    let mut r = arena(RangeUnitKind::Tank);
    let u = &mut r.units[0];
    u.update_pose(fx::angle::FULL / 4);
    let mut tested = 0;
    for x in 40..u.body.vol.nx as usize {
        for y in u.body.turret_y as usize..u.body.vol.ny as usize {
            for z in 0..u.body.vol.nz as usize {
                if u.body.vol.get(x, y, z) {
                    let p = u.world_pos([x, y, z]);
                    assert_eq!(u.hit(p), Some((0, [x, y, z])));
                    tested += 1;
                }
            }
        }
    }
    assert!(tested > 10);
}

#[test]
fn aim_follows_surviving_core_instead_of_shooting_through_a_hole() {
    let mut r = arena(RangeUnitKind::Strider);
    let u = &mut r.units[1];
    let first = aim_point(u);
    let (_, [x, y, z]) = u.hit(first).expect("aim must hit solid body");
    assert!(u.body.remove(x, y, z));
    assert!(!u.solid(first));
    let next = aim_point(u);
    assert_ne!(next, first);
    assert!(u.solid(next));
}

#[test]
fn auto_approaches_when_out_of_range_and_dead_order_returns_to_auto() {
    let mut r = arena(RangeUnitKind::Tank);
    std::sync::Arc::make_mut(&mut r.battle.tuning).tank_range = 100;
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    let before = r.units[0].at;
    for _ in 0..60 {
        r.step();
    }
    assert!(r.units[0].at[0] > before[0] + 5);
    assert_eq!(r.battle.actors[0].fired, 0, "cannot shoot outside range");
    r.battle.actors[0].order = Order::Attack(1);
    r.units[1].status.alive = false;
    r.battle_tick();
    assert_eq!(r.battle.actors[0].order, Order::Auto);
    assert_eq!(r.battle.phase, Phase::Victory);
}

#[test]
fn snapshot_rejects_projectile_positions_outside_the_range() {
    let mut r = arena(RangeUnitKind::Tank);
    r.start_battle().unwrap();
    for _ in 0..180 {
        r.step();
        if !r.battle.rounds.is_empty() {
            break;
        }
    }
    assert!(!r.battle.rounds.is_empty());
    r.battle.rounds[0].pos[0] = Fx::from_int(1_000_000);
    assert!(BlastRange::restore(r.config.clone(), &r.snapshot()).is_none());
}

#[test]
fn a_far_target_does_not_restart_its_route_search_on_each_pursuit_tick() {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-km.ron")).unwrap();
    layout.battle = true;
    layout.units = vec![
        RangeUnitDef {
            kind: RangeUnitKind::Tank,
            at: [128, 16, 336],
            allied: true,
        },
        RangeUnitDef {
            kind: RangeUnitKind::Tank,
            at: [1480, 16, 460],
            allied: false,
        },
    ];
    if let Some(course) = &mut layout.course {
        course.patrols.clear();
    }
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap(),
        layout,
    )
    .unwrap();
    let tuning = std::sync::Arc::make_mut(&mut r.battle.tuning);
    tuning.tank_range = 100;
    tuning.pursuit_ticks = 1;
    r.start_battle().unwrap();
    r.step();
    assert!(r.units[0].motion.searching(), "route was trivial");
    for _ in 0..30 {
        r.step();
        if r.units[0].motion.frontier_nodes() > 0 || !r.units[0].motion.path.is_empty() {
            break;
        }
    }
    assert!(
        r.units[0].motion.frontier_nodes() > 0 || !r.units[0].motion.path.is_empty(),
        "search never progressed: nodes={}, searching={}, destination={:?}, last_plan={:?}",
        r.units[0].motion.frontier_nodes(),
        r.units[0].motion.searching(),
        r.units[0].motion.destination,
        r.units[0].motion.last_plan
    );
}

#[test]
fn a_moving_target_changes_the_finished_pursuit_goal() {
    let mut r = arena(RangeUnitKind::Tank);
    let tuning = std::sync::Arc::make_mut(&mut r.battle.tuning);
    tuning.tank_range = 100;
    tuning.pursuit_ticks = 1;
    r.start_battle().unwrap();
    r.battle.actors[1].order = Order::Hold;
    r.step();
    for _ in 0..30 {
        if !r.units[0].motion.searching() {
            break;
        }
        r.step();
    }
    assert!(!r.units[0].motion.searching());
    let before = r.units[0].motion.destination.expect("initial approach");
    r.units[1].motion.pos[0] = Fx::from_int(520);
    let head = r.battle.actors[1].turret.angle;
    r.units[1].update_pose(head);
    r.step();
    let after = r.units[0].motion.destination.expect("updated approach");
    assert!(after[0] > before[0] + 25, "{before:?} -> {after:?}");
}
