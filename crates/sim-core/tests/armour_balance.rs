//! Full range physics/section extraction, finite stock siege payload, no GPU.
use modding::{
    explosion::ExplosionConfig,
    range::{RangeLayout, RangeUnitDef, RangeUnitKind},
};
use sim_core::blast::battle::{Order, Weapon};
use sim_core::blast::{BlastRange, Explosion};

fn arena(workers: usize) -> BlastRange {
    let c = ExplosionConfig::from_ron(include_str!("../../../data/siege-shell.ron")).unwrap();
    let mut l = RangeLayout::from_ron(include_str!("../../../data/fixture-corpses.ron")).unwrap();
    l.battle = false;
    l.size = [1024, 256, 1024];
    l.target = [600, 80, 512];
    l.clear_min = [64, 64];
    l.clear_max = [960, 960];
    l.units = vec![RangeUnitDef {
        kind: RangeUnitKind::Spider,
        allied: false,
        at: [600, 16, 512],
        weapon: None,
    }];
    let mut r = BlastRange::with_layout(c, l).unwrap();
    assert!(r.set_physics_workers(workers));
    assert!(r.set_navigation_workers(1));
    r.player = true;
    r.auto_attack = false;
    r.fire_tick = u32::MAX;
    for _ in 0..20 {
        r.step();
    }
    r
}
fn strike(r: &mut BlastRange, cell: [usize; 3]) {
    let at = r.units[0].world_pos(cell);
    r.matter
        .begin(r.config.material_energy_j, r.config.fragment_box);
    r.explosion = Some(Explosion::begin_material(
        at,
        &r.config,
        &r.units,
        r.matter.tuning.body_energy_pct,
    ));
}
fn settle(r: &mut BlastRange) {
    for _ in 0..40 {
        r.step();
        assert!(r.matter.economy.ledger.balanced());
    }
    assert!(!r.explosion.as_ref().unwrap().working());
}
fn functional(r: &BlastRange) {
    let u = &r.units[0];
    assert!(
        u.status.alive && u.status.engine && u.status.weapon,
        "nonfunctional at tick {}: {:?}",
        r.tick,
        u.status
    );
    assert!(u.integrity.drive >= fx::Fx::from_ratio(75, 100));
}
#[test]
fn spider_remains_functional_after_four_concentrated_siege_hits_on_roof_or_weapon() {
    for cell in [[44, 42, 40], [76, 48, 40]] {
        let mut r = arena(1);
        for _ in 0..4 {
            strike(&mut r, cell);
            settle(&mut r);
            functional(&r);
        }
        assert!(
            !r.units[0].removed.is_empty(),
            "the armour must really take damage"
        );
        // Deliberate local damage eventually disables the machine/weapon.
        let mut disabled = false;
        for _ in 0..12 {
            strike(&mut r, cell);
            settle(&mut r);
            let s = r.units[0].status;
            if !s.alive || !s.weapon {
                disabled = true;
                break;
            }
        }
        assert!(
            disabled,
            "material grade must not make the target invulnerable"
        );
    }
}
#[test]
fn spider_takes_eight_distributed_siege_hits_with_exact_worker_and_midwave_replay_match() {
    let mut serial = arena(1);
    let mut parallel = arena(4);
    let mut replay = None;
    for (shot, cell) in [
        [28, 40, 28],
        [60, 42, 48],
        [44, 42, 40],
        [22, 39, 46],
        [68, 40, 34],
        [36, 42, 52],
        [52, 42, 28],
        [40, 40, 40],
    ]
    .into_iter()
    .enumerate()
    {
        strike(&mut serial, cell);
        strike(&mut parallel, cell);
        if let Some(copy) = replay.as_mut() {
            strike(copy, cell);
        }
        for tick in 0..40 {
            serial.step();
            parallel.step();
            assert_eq!(
                serial.state_hash(),
                parallel.state_hash(),
                "shot {} tick {}",
                shot + 1,
                tick
            );
            if let Some(copy) = replay.as_mut() {
                copy.step();
                assert_eq!(serial.state_hash(), copy.state_hash(), "midwave replay");
            } else if shot == 2 && tick == 2 {
                let mut copy = BlastRange::restore(serial.config, &serial.snapshot())
                    .expect("armour/dent snapshot");
                assert!(copy.set_physics_workers(4));
                assert!(copy.set_navigation_workers(1));
                assert_eq!(serial.state_hash(), copy.state_hash());
                replay = Some(copy);
            }
            assert!(serial.matter.economy.ledger.balanced());
        }
        assert!(!serial.explosion.as_ref().unwrap().working());
        functional(&serial);
    }
    assert!(serial.units[0].removed.len() > 1000);
    assert!(!serial.matter.debris.fragments.is_empty());
}

#[test]
fn live_siege_projectiles_preserve_the_spider_after_four_aimed_or_eight_spread_hits() {
    for spread in [false, true] {
        let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
        let mut l =
            RangeLayout::from_ron(include_str!("../../../data/fixture-corpses.ron")).unwrap();
        l.size = [1024, 256, 1024];
        l.battle = true;
        l.idle_enemies = true;
        l.clear_min = [64, 64];
        l.clear_max = [960, 960];
        l.units = vec![
            RangeUnitDef {
                kind: RangeUnitKind::ArtilleryTurretT3,
                allied: true,
                at: [160, 16, 512],
                weapon: None,
            },
            RangeUnitDef {
                kind: RangeUnitKind::Spider,
                allied: false,
                at: [700, 16, 512],
                weapon: None,
            },
        ];
        let mut r = BlastRange::with_layout(c, l).unwrap();
        assert!(r.set_physics_workers(1));
        assert!(r.set_navigation_workers(1));
        r.start_battle().unwrap();
        assert!(r.battle_order(0, Order::Hold, None));
        for _ in 0..20 {
            r.step();
        }
        for shot in 0..if spread { 8 } else { 4 } {
            let cell = if spread {
                [
                    [28, 40, 28],
                    [60, 42, 48],
                    [44, 42, 40],
                    [22, 39, 46],
                    [68, 40, 34],
                    [36, 42, 52],
                    [52, 42, 28],
                    [40, 40, 40],
                ][shot]
            } else {
                [44, 42, 40]
            };
            let at = r.units[1].world_pos(cell);
            let fired = r.battle.actors[0].fired;
            let removed = r.units[1].removed.len() + r.units[1].seg_removed.len();
            assert!(r.battle_order(0, Order::Ground(at), None));
            let mut held = false;
            let mut impacts = 0;
            let mut after = 0;
            for _ in 0..400 {
                r.step();
                assert!(r.matter.economy.ledger.balanced());
                if !held && r.battle.actors[0].fired > fired {
                    assert!(r.battle_order(0, Order::Hold, None));
                    held = true;
                }
                impacts += r
                    .battle
                    .impacts
                    .iter()
                    .filter(|i| i.weapon == Weapon::Siege)
                    .count();
                if impacts > 0 {
                    after += 1;
                }
                if after >= 40 && r.battle.rounds.is_empty() && r.battle.wave_counts() == [0, 0] {
                    break;
                }
            }
            let u = &r.units[1];
            assert!(
                held && impacts == 1 && after >= 40,
                "unfinished live shot {}",
                shot + 1
            );
            assert!(
                u.removed.len() + u.seg_removed.len() > removed,
                "live shot did no receiver damage"
            );
            assert!(
                u.status.alive && u.status.engine && u.status.weapon,
                "spread={spread} shot={} status={:?}",
                shot + 1,
                u.status
            );
        }
    }
}
