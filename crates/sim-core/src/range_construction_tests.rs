use super::*;
use crate::blast::ExplosionConfig;
use modding::range::{RangeBlock, RangeLayout};

fn range() -> BlastRange {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.battle = true;
    layout.idle_enemies = true;
    layout.size = [3200, 1024, 3200];
    layout.blocks = vec![
        RangeBlock {
            min: [0, 0, 0],
            max: [3200, 1024, 3200],
            material: 0,
        },
        RangeBlock {
            min: [0, 0, 0],
            max: [3200, 16, 3200],
            material: 1,
        },
    ];
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [3136, 3136];
    layout.units = vec![
        RangeUnitDef {
            allied: true,
            kind: RangeUnitKind::GrebenStorage,
            at: [900, 16, 1000],
            weapon: None,
        },
        RangeUnitDef {
            allied: false,
            kind: RangeUnitKind::Tank,
            at: [2900, 16, 2800],
            weapon: None,
        },
    ];
    let mut economy = modding::range_economy::RangeEconomy::stock();
    economy.bank.as_mut().unwrap().base_store = 1 << 40;
    layout.economy = Some(economy);
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap();
    let store = r.mining().unwrap().haul().unwrap().store(true);
    let seeded = 1 << 30;
    let economy = &mut r.matter.economy;
    assert_eq!(economy.ledger.reveal(economy.ground, seeded), seeded);
    assert_eq!(
        economy.ledger.transfer(economy.ground, store, seeded),
        seeded
    );
    r.start_battle().unwrap();
    r
}

#[test]
fn greben_starts_with_eight_real_drones_and_one_crawler() {
    let r = range();
    assert_eq!(
        r.units
            .iter()
            .filter(|u| u.kind == RangeUnitKind::SkobaDrone)
            .count(),
        8
    );
    assert_eq!(
        r.units
            .iter()
            .filter(|u| u.kind == RangeUnitKind::Crawler)
            .count(),
        1
    );
    assert!(r
        .units
        .iter()
        .filter(|u| u.kind == RangeUnitKind::SkobaDrone)
        .all(|u| u.status.alive));
}

#[test]
fn higher_tier_construction_keeps_every_part_paid_and_round_trips() {
    for kind in [RangeUnitKind::TurretT2,RangeUnitKind::ArtilleryTurretT3,RangeUnitKind::NuclearSilo] {
        let mut r = range();
        let at = [2300,16,1000];
        let id = r.create_construction_site(kind,true,at).unwrap();
        let mut site = r.mining.as_ref().unwrap().construction.as_ref().unwrap().sites[id].clone();
        assert!(ensure_site_unit(&mut r,&mut site));
        let unit = usize::from(site.unit.unwrap());
        let body = r.matter.body_container(unit);
        let store = r.mining().unwrap().haul().unwrap().store(true);
        let density = Amount::from(r.matter.source_density(STEEL));
        let first = (BATCH / density).max(1) * density;
        for delivered in [first,site.required] {
            let previous = site.delivered;
            let amount = delivered-previous;
            assert_eq!(r.matter.economy.ledger.transfer(store,body,amount),amount);
            site.delivered = delivered;
            {
                let c = r.mining.as_mut().unwrap().construction.as_mut().unwrap();
                c.delivered = delivered;
                c.sites[id] = site.clone();
            }
            apply_site_progress(&mut r,unit,kind,at,site.required,previous,delivered);
            assert!(r.units[unit].status.alive,"{kind:?}");
            if delivered < site.required {assert!(!r.units[unit].status.weapon);}
            assert!(r.matter.economy.ledger.balanced());
            let mut section = ("",0,None);
            let restored = BlastRange::restore_observed(r.config,&r.snapshot(),|a,b,c|section=(a,b,c))
                .unwrap_or_else(||panic!("{kind:?} at {delivered}: {section:?}"));
            assert_eq!(r.state_hash(),restored.state_hash(),"{kind:?} at {delivered}");
        }
        assert!(r.units[unit].removed.is_empty() && r.units[unit].seg_removed.is_empty());
    }
}

#[test]
fn a_loaded_drone_waits_without_a_home_and_returns_cargo_when_one_is_available() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1500])
        .unwrap();
    r.set_construction_quota(0, 1);
    let (id, cargo) = (0..600)
        .find_map(|_| {
            r.construction_phase();
            r.mining
                .as_ref()
                .unwrap()
                .construction
                .as_ref()
                .unwrap()
                .drones
                .iter()
                .find_map(|(&id, d)| d.cargo.map(|b| (id, b.container)))
        })
        .expect("loaded delivery");
    r.set_construction_quota(0, 0);
    let homes: Vec<_> = r
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| {
            matches!(
                u.kind,
                RangeUnitKind::GrebenStorage | RangeUnitKind::Crawler
            )
        })
        .map(|(id, _)| id)
        .collect();
    for &home in &homes {
        r.units[home].status.alive = false;
    }
    let at = r.units[usize::from(id)].motion.pos;
    let amount = r.matter.economy.ledger.held(cargo);
    let store = r.side_store(true).unwrap().0;
    for _ in 0..64 {
        r.construction_phase();
    }
    assert_eq!(r.units[usize::from(id)].motion.pos, at);
    assert_eq!(r.matter.economy.ledger.held(cargo), amount);
    assert!(r.matter.economy.ledger.balanced());
    r.units[homes[0]].status.alive = true;
    for _ in 0..400 {
        r.construction_phase();
    }
    assert!(r.drone_cargo(usize::from(id)).is_none());
    assert_eq!(r.drone_leg(usize::from(id)), Some(Leg::Idle));
    assert_eq!(r.side_store(true).unwrap().0, store + amount);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn drones_split_six_two_and_build_two_turrets_from_real_batches() {
    let mut r = range();
    let first = r
        .create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1000])
        .unwrap();
    let second = r
        .create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1500])
        .unwrap();
    assert!(r.set_construction_quota(first, 6));
    assert!(r.set_construction_quota(second, 2));
    r.step();
    let sites = r.construction_sites();
    assert_eq!(sites[first].assigned, 6);
    assert_eq!(sites[second].assigned, 2);
    for _ in 0..6000 {
        r.step();
        if r.construction_sites().iter().all(|s| s.complete) {
            break;
        }
    }
    let sites = r.construction_sites();
    assert!(sites.iter().all(|s| s.complete), "{sites:?}");
    assert_eq!(
        r.units
            .iter()
            .filter(|u| u.kind == RangeUnitKind::TurretT1)
            .count(),
        2
    );
    let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    assert!(c.trips >= 2 && c.delivered > 0);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn a_shot_down_loaded_drone_leaves_one_reclaimable_batch() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1000])
        .unwrap();
    assert!(r.set_construction_quota(0, 1));
    let mut loaded = None;
    for _ in 0..500 {
        r.step();
        loaded = (0..r.units.len()).find(|&id| r.drone_cargo(id).is_some());
        if loaded.is_some() {
            break;
        }
    }
    let id = loaded.expect("a drone picked up a batch");
    let cargo = r.drone_cargo(id).unwrap();
    let carried = r.matter.economy.ledger.held(cargo.container);
    assert!(carried > 0 && carried <= BATCH);
    assert_eq!(carried % Amount::from(r.matter.source_density(STEEL)),0);
    let before = r.side_store(true).unwrap().0;
    r.units[id].status.alive = false;
    r.construction_phase();
    assert!(r.drone_cargo(id).is_none());
    assert_eq!(r.side_store(true).unwrap().0, before);
    assert_eq!(r.wreck_cargo(id), Some(carried));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn crawler_upgrade_buy_four_parked_drones_only_once() {
    let mut r = range();
    let crawler = r
        .units
        .iter()
        .position(|u| u.kind == RangeUnitKind::Crawler)
        .unwrap();
    assert!(r.upgrade_crawler_drones(crawler));
    assert!(!r.upgrade_crawler_drones(crawler));
    let parked: Vec<_> = r
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.kind == RangeUnitKind::SkobaDrone && u.body.vol.get(64, 16, 26))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(parked.len(), 4);
    let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    assert!(parked.iter().all(|id| c
        .drones
        .get(&(*id as u16))
        .is_some_and(|d| usize::from(d.home) == crawler)));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn a_loaded_flight_restores_and_continues_bit_for_bit() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1000])
        .unwrap();
    for _ in 0..500 {
        r.step();
        if (0..r.units.len()).any(|id| r.drone_cargo(id).is_some()) {
            break;
        }
    }
    assert!((0..r.units.len()).any(|id| r.drone_cargo(id).is_some()));
    let bytes = r.snapshot();
    let mut restored = BlastRange::restore(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        &bytes,
    )
    .expect("loaded flight restores");
    assert_eq!(r.state_hash(), restored.state_hash());
    for _ in 0..120 {
        r.step();
        restored.step();
    }
    assert_eq!(r.state_hash(), restored.state_hash());
}

#[test]
fn manual_pilot_keeps_a_loaded_drone_from_automatic_delivery_and_restores_it() {
    let mut r = range();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1000])
        .unwrap();
    r.set_construction_quota(0, 1);
    let id = (0..600)
        .find_map(|_| {
            r.construction_phase();
            (0..r.units.len()).find(|&id| r.drone_cargo(id).is_some())
        })
        .expect("loaded drone");
    let cargo = r.drone_cargo(id);
    let at = r.units[id].motion.pos;
    let head = r.units[id].motion.drive.head;
    assert!(r.battle_order(
        id,
        battle::Order::Pilot {
            throttle: 1,
            turn: 0,
            aim: [2500, 150, 1000],
            fire: false,
        },
        None
    ));
    for _ in 0..8 {
        r.step();
    }
    let forward = r.units[id].motion.pos[0]
        .sub(at[0])
        .mul(fx::angle::cos(head))
        .add(
            r.units[id].motion.pos[2]
                .sub(at[2])
                .mul(fx::angle::sin(head)),
        );
    assert!(
        forward > Fx::ZERO,
        "manual thrust must follow the craft's existing heading"
    );
    assert_eq!(r.units[id].motion.pos[1], at[1]);
    assert_eq!(r.drone_cargo(id), cargo);
    let saved = r.snapshot();
    assert_eq!(u32::from_le_bytes(saved[8..12].try_into().unwrap()), 77);
    let forms = u32::from_le_bytes(saved[12..16].try_into().unwrap()) as usize;
    let legacy: Vec<_> = saved[..8]
        .iter()
        .chain(&saved[16 + forms * 5..])
        .copied()
        .collect();
    let old = BlastRange::restore(r.config, &legacy).expect("legacy loaded craft restores");
    assert_eq!(old.drone_cargo(id), cargo);
    assert!(old.units[id].body.vol == r.units[id].body.vol);
    let mut copy =
        BlastRange::restore(r.config, &r.snapshot()).expect("piloted loaded drone restores");
    for _ in 0..8 {
        r.step();
        copy.step();
        assert_eq!(r.state_hash(), copy.state_hash());
        assert_eq!(r.drone_cargo(id), cargo);
    }
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn skoba_forms_restore_damaged_cargo_cells_and_reject_malformed_metadata() {
    let mut layout = RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    layout.size = [640, 192, 640];
    layout.blocks.clear();
    layout.shields.clear();
    layout.course = None;
    layout.clear_min = [64, 64];
    layout.clear_max = [600, 600];
    for u in &mut layout.units {
        u.kind = RangeUnitKind::SkobaDrone;
    }
    let mut r = BlastRange::with_layout(
        ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
        layout,
    )
    .unwrap();
    for (id, form) in [(0, 1), (1, 2)] {
        r.units[id].restore_skoba_form(form);
        r.units[id].refresh_damage(id);
        r.units[id].update_pose(0);
    }
    let cell = [12usize, 4, 20]; // Cargo-only cell, absent from the open frame.
    assert!(!voxel::unit::skoba().vol.get(cell[0], cell[1], cell[2]));
    assert!(r.units[1].body.remove(cell[0], cell[1], cell[2]));
    r.units[1].removed.push(cell.map(|v| v as u8));
    r.matter.annihilate_body(1, 1);
    r.units[1].refresh_damage(1);
    let bytes = r.snapshot();
    let back = BlastRange::restore(r.config, &bytes).expect("damaged loaded form restores");
    assert_eq!(bytes, back.snapshot());
    assert!(back.units[1].body.vol == r.units[1].body.vol);
    assert!(back.matter.economy.ledger.balanced());
    for (at, value) in [
        (12, u32::MAX),
        (16, u32::MAX),
        (21, 0),
        (21, r.units.len() as u32),
    ] {
        let mut bad = bytes.clone();
        bad[at..at + 4].copy_from_slice(&value.to_le_bytes());
        assert!(BlastRange::restore(r.config, &bad).is_none(), "field {at}");
    }
    for form in [0, 3, 255] {
        let mut bad = bytes.clone();
        bad[20] = form;
        assert!(BlastRange::restore(r.config, &bad).is_none());
    }
    for end in 12..26 {
        assert!(BlastRange::restore(r.config, &bytes[..end]).is_none());
    }
}

#[test]
fn ready_menu_scenarios_start_real_construction() {
    for id in [24, 25, 26] {
        let preset = modding::range_skirmish::stock(id).unwrap();
        let layout =
            modding::range::RangeLayout::from_ron(include_str!("../../../data/range-km.ron"))
                .unwrap();
        let layout = preset.apply_to(layout).unwrap();
        let mut r = BlastRange::with_layout(
            ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap(),
            layout,
        )
        .unwrap();
        assert_eq!(
            r.units
                .iter()
                .filter(|u| u.kind == RangeUnitKind::SkobaDrone)
                .count(),
            8
        );
        if matches!(id, 24 | 26) {
            let factory = if id == 24 { 0 } else { 1 };
            assert_eq!(r.production_queue(factory), Some(vec![RangeUnitKind::Tank]));
            assert!(r.side_store(true).unwrap().0 > 0);
        } else {
            let sites = r.construction_sites();
            assert_eq!(sites.len(), 2);
            assert_eq!((sites[0].quota, sites[1].quota), (6, 2));
        }
        r.start_battle().unwrap();
        r.step();
        if id == 25 {
            assert!((0..r.units.len()).any(|id| r.drone_leg(id) == Some(Leg::ToSource)));
        }
    }
}

#[test]
fn construction_never_assigns_an_enemy_drone() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, false, [2300, 16, 1500])
        .unwrap();
    r.construction_phase();
    assert_eq!(r.construction_sites()[0].assigned, 0);
    assert!((0..r.units.len()).all(|id| r.drone_leg(id) != Some(Leg::ToSource)));
}

#[test]
fn a_dead_unloaded_drone_releases_its_reservation_and_is_replaced() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1500])
        .unwrap();
    r.set_construction_quota(0, 1);
    r.construction_phase();
    let id = (0..r.units.len())
        .find(|&id| r.drone_leg(id) == Some(Leg::ToSource))
        .unwrap();
    assert!(r.drone_cargo(id).is_none());
    r.units[id].status.alive = false;
    r.construction_phase();
    let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    assert_eq!(c.sites[0].reserved, 0);
    assert!(c.drones[&(id as u16)].site.is_none());
    r.construction_phase();
    assert_eq!(r.construction_sites()[0].assigned, 1);
    assert_ne!(r.drone_leg(id), Some(Leg::ToSource));
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn an_unstarted_site_with_empty_drones_restores() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1500])
        .unwrap();
    let bytes = r.snapshot();
    let restored = BlastRange::restore(r.config, &bytes).expect("idle construction must restore");
    assert_eq!(r.state_hash(), restored.state_hash());
}

#[test]
fn a_delivery_does_not_repair_already_built_cells_or_erase_dents() {
    let mut r = range();
    let kind = RangeUnitKind::TurretT1;
    let at = [2300, 16, 1000];
    let density = r.matter.source_density(STEEL) as Amount;
    let mut site = Site {
        kind,
        allied: true,
        at,
        required: 1 << 30,
        delivered: 0,
        reserved: 0,
        quota: 1,
        unit: None,
    };
    assert!(ensure_site_unit(&mut r, &mut site));
    let id = usize::from(site.unit.unwrap());
    let cells = connected_cells(&r.units[id].body);
    let first = cells.len() / 2;
    let next = first + 100;
    let required = cells.len() as Amount * density;
    apply_site_progress(&mut r, id, kind, at, required, 0, first as Amount * density);
    let broken = cells[..first]
        .iter()
        .rev()
        .find(|p| r.units[id].body.part_at(p[0], p[1], p[2]) == voxel::unit::Part::Armour)
        .unwrap();
    let broken = *broken;
    assert!(r.units[id].body.remove(broken[0], broken[1], broken[2]));
    r.units[id].removed.push(broken.map(|v| v as u8));
    let dent = cells[0].map(|v| v as u8);
    r.units[id].dents.insert(dent, 17);
    apply_site_progress(
        &mut r,
        id,
        kind,
        at,
        required,
        first as Amount * density,
        next as Amount * density,
    );
    assert!(!r.units[id].body.vol.get(broken[0], broken[1], broken[2]));
    assert_eq!(r.units[id].dents.get(&dent), Some(&17));
    assert_eq!(r.units[id].body.solid_count() as usize, next - 1);
    assert_eq!(r.units[id].removed.len(), cells.len() - next + 1);
}

#[test]
fn a_destroyed_site_releases_drones_and_returns_loaded_material() {
    let mut r = range();
    r.create_construction_site(RangeUnitKind::TurretT1, true, [2300, 16, 1000])
        .unwrap();
    r.set_construction_quota(0, 1);
    // Seed the first delivery, then destroy the construction while a new
    // loaded batch is travelling toward it.
    for _ in 0..1600 {
        r.construction_phase();
        let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
        if c.sites[0].unit.is_some() && c.drones.values().any(|d| d.cargo.is_some()) {
            break;
        }
    }
    let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    let unit = usize::from(c.sites[0].unit.expect("first batch built a body"));
    let (&drone, d) = c
        .drones
        .iter()
        .find(|(_, d)| d.cargo.is_some())
        .expect("next batch in flight");
    let cargo = d.cargo.unwrap().container;
    let amount = r.matter.economy.ledger.held(cargo);
    let before = r.side_store(true).unwrap().0;
    r.units[unit].status.alive = false;
    for _ in 0..400 {
        r.construction_phase();
    }
    let c = r.mining.as_ref().unwrap().construction.as_ref().unwrap();
    assert_eq!(c.sites[0].reserved, 0);
    assert!(c.drones.values().all(|d| d.site.is_none()));
    assert!(r.drone_cargo(usize::from(drone)).is_none());
    assert_eq!(r.side_store(true).unwrap().0, before + amount);
    assert!(r.matter.economy.ledger.balanced());
}
