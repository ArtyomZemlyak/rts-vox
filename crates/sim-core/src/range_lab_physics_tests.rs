use super::*;
use battle::commands::{Command, Outcome};
use modding::lab_tools::{Device, DeviceKind};
fn lab() -> BlastRange {
    let mut l = modding::laboratory::Definition::stock().layout;
    l.size = [128, 128, 128];
    l.clear_min = [1, 1];
    l.clear_max = [127, 127];
    l.gun_point = true;
    l.gun = [32, 32, 32];
    l.target = [64, 16, 64];
    l.laboratory = true;
    let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, l).unwrap();
    r.control();
    r
}

#[test]
fn tank_energy_cannon_damages_more_than_the_previous_shot() {
    use modding::range::{RangeUnitDef,RangeUnitKind};
    let mut layout=modding::laboratory::Definition::stock().layout;
    layout.size=[128,128,128];layout.clear_min=[1,1];layout.clear_max=[127,127];
    layout.gun_point=true;layout.gun=[32,32,32];layout.target=[64,16,64];layout.laboratory=true;
    layout.units=vec![RangeUnitDef{allied:true,kind:RangeUnitKind::Tank,at:[64,16,64],weapon:None}];
    let current=ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap();
    let mut previous=current;previous.energy=1550;previous.heat_per_cell=105;
    let mut damage=[0;2];
    for (index, config) in [current,previous].into_iter().enumerate() {
        let mut sim=BlastRange::with_layout(config,layout.clone()).unwrap();
        sim.control();sim.set_physics_workers(1);sim.set_navigation_workers(1);
        assert!(sim.execute_command(Command::Gun(battle::commands::GunCommand::Detonate {at:[64,30,64]})).admitted());
        for _ in 0..40 {sim.step();}
        damage[index]=sim.units[0].removed.len()+sim.units[0].seg_removed.len();
        assert!(sim.matter.economy.ledger.balanced());
    }
    eprintln!("energy cannon tank cells removed: new={} previous={}",damage[0],damage[1]);
    assert!(damage[1]>0 && damage[0]>damage[1],"current/previous tank damage: {damage:?}");
}

#[test]
fn spider_weapon_survives_repeated_t2_energy_impacts_at_the_muzzle() {
    use modding::range::{RangeUnitDef,RangeUnitKind};
    let mut layout=modding::laboratory::Definition::stock().layout;
    layout.size=[256,128,256];layout.clear_min=[1,1];layout.clear_max=[255,255];
    layout.gun_point=true;layout.gun=[32,32,32];layout.target=[128,16,128];layout.laboratory=true;
    layout.units=vec![RangeUnitDef{allied:true,kind:RangeUnitKind::Spider,at:[128,16,128],weapon:None}];
    let mut config=ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap().at_power(1200);
    config.hull_impulse_milli=0; // Keep each deliberate strike on the same spot.
    let mut r=BlastRange::with_layout(config,layout).unwrap();r.control();
    r.set_physics_workers(1);r.set_navigation_workers(1);
    let at=r.units[0].world_pos([86,45,40]);
    let mut landed=0;
    for _ in 0..4 {
        for _ in 0..120 {if r.ready(){break;}r.step();}
        assert!(r.execute_command(Command::Gun(battle::commands::GunCommand::Detonate{at})).admitted());
        for _ in 0..120 {r.step();if r.ready(){break;}}
        landed+=1;
        assert!(r.units[0].status.alive && r.units[0].status.weapon,
            "spider core or weapon lost after {landed} concentrated hits: {:?}",r.units[0].status);
        assert!(r.matter.economy.ledger.balanced());
    }
}

#[test]
fn spider_armour_rebalance_increases_damage_from_t2_energy() {
    use modding::range::{RangeUnitDef,RangeUnitKind};
    let mut damage = [0usize; 2];
    for (index, previous) in [true, false].into_iter().enumerate() {
        let mut layout=modding::laboratory::Definition::stock().layout;
        layout.size=[256,128,256];layout.clear_min=[1,1];layout.clear_max=[255,255];
        layout.gun_point=true;layout.gun=[32,32,32];layout.target=[128,16,128];layout.laboratory=true;
        layout.units=vec![RangeUnitDef{allied:true,kind:RangeUnitKind::Spider,at:[128,16,128],weapon:None}];
        let mut config=ExplosionConfig::from_ron(include_str!("../../../data/energy-cannon.ron")).unwrap().at_power(1200);
        config.hull_impulse_milli=0;
        let mut r=BlastRange::with_layout(config,layout).unwrap();r.control();
        r.set_physics_workers(1);r.set_navigation_workers(1);
        if previous {
            let grade=[4500,3000,2500,20000,2000];
            r.units[0].body.ablation_milli=grade;
            for part in &mut r.units[0].body.segments { part.body.ablation_milli=grade; }
        }
        let at=r.units[0].world_pos([86,45,40]);
        for _ in 0..4 {
            for _ in 0..120 {if r.ready(){break;}r.step();}
            assert!(r.execute_command(Command::Gun(battle::commands::GunCommand::Detonate{at})).admitted());
            for _ in 0..120 {r.step();if r.ready(){break;}}
        }
        damage[index]=r.units[0].removed.len()+r.units[0].seg_removed.len();
        assert!(r.units[0].status.alive && r.units[0].status.weapon,
            "spider lost its core or weapon during four T2 energy impacts");
        assert!(r.matter.economy.ledger.balanced());
    }
    eprintln!("T2 energy, four muzzle impacts: previous/current removed cells {damage:?}");
    assert!(damage[1]>damage[0], "previous vs current removed cells: {damage:?}");
}
#[test]
fn lab_live_cannon_tracks_during_charge_keeps_its_deadline_and_restores_flight() {
    use battle::commands::GunCommand as G;
    let mut r=lab();r.set_physics_workers(1);r.set_navigation_workers(1);
    let config=ExplosionConfig::from_ron(include_str!("../../../data/kinetic-shell.ron")).unwrap();
    r.execute_command(Command::ConfigureWeapon{config});
    let mut deadline=None;
    for i in 0..=config.charge_ticks {
        let origin=[40,70+i as i32,64];let target=[90,70+i as i32,64];
        assert_eq!(r.execute_command(Command::Gun(G::LabStream{origin,target,enabled:true})),Outcome::Applied);
        if let Some(want)=deadline {assert_eq!(r.fire_tick,want,"tracking must not restart charge");}else{deadline=Some(r.fire_tick);}
        r.step();
    }
    assert!(r.shell.is_some(),"the charged round must launch");
    let before=r.state_hash();
    assert_eq!(r.execute_command(Command::Gun(G::LabStream{origin:[48,60,64],target:[80,60,64],enabled:true})),Outcome::Busy);
    assert_eq!(r.state_hash(),before,"busy must not move a flying round's source");
    let mut back=BlastRange::restore(r.config,&r.snapshot()).unwrap();back.set_physics_workers(1);back.set_navigation_workers(1);
    let stop=Command::Gun(G::LabStream{origin:[0;3],target:[0;3],enabled:false});
    r.execute_command(stop);back.execute_command(stop);
    assert!(r.shell.is_some());
    for _ in 0..4 {r.step();back.step();assert_eq!(r.state_hash(),back.state_hash());}
}

#[test]
fn lab_live_laser_can_cut_while_the_previous_explosion_is_working() {
    use battle::commands::GunCommand as G;
    let mut r=lab();r.set_physics_workers(1);r.set_navigation_workers(1);
    assert_eq!(r.execute_command(Command::Gun(G::Detonate{at:[64,15,64]})),Outcome::Applied);
    assert!(r.explosion.is_some());
    r.execute_command(Command::ConfigureWeapon{config:ExplosionConfig::from_ron(include_str!("../../../data/laser.ron")).unwrap()});
    assert_eq!(r.execute_command(Command::Gun(G::LabStream{origin:[40,60,64],target:[90,60,64],enabled:true})),Outcome::Applied);
    r.step();assert!(r.streams.emitted()>0);
}
#[test]
fn lab_fragment_landing_he_shell_retains_finite_shapes_and_restores_the_settled_world() {
    use battle::commands::GunCommand as G;
    let mut r=lab();r.set_physics_workers(1);r.set_navigation_workers(1);
    assert_eq!(r.execute_command(Command::Sculpt{at:[60,16,60],edge:8,material:2,erase:false}),Outcome::Queued);
    r.step();
    assert_eq!(r.execute_command(Command::Gun(G::Detonate{at:[64,18,64]})),Outcome::Applied);
    let mut seen=BTreeMap::new();let mut flying=0;let mut resting=0;
    for _ in 0..140 {
        r.step();
        for f in &r.matter.debris.fragments {
            if f.chunk.is_some() {continue;}
            flying+=usize::from(!f.resting);resting+=usize::from(f.resting);
            assert_eq!(f.count,f.shape.count_ones(),"landing must not hide inventory in a saturated enlarged pile");
            if let Some(&(count,shape,side,mm))=seen.get(&f.id) {
                if count==f.count {assert_eq!((shape,side,mm),(f.shape,f.side,f.mm),"a surviving shard changed shape on landing");}
            }
            seen.insert(f.id,(f.count,f.shape,f.side,f.mm));
        }
        assert!(r.matter.economy.ledger.balanced());
    }
    assert!(flying>0&&resting>0,"the HE fixture must actually fly and land debris");
    let mut back=BlastRange::restore(r.config,&r.snapshot()).unwrap();back.set_physics_workers(1);back.set_navigation_workers(1);
    for _ in 0..3 {r.step();back.step();assert_eq!(r.state_hash(),back.state_hash());}
}
#[test]
fn lab_live_stream_moves_the_real_laser_and_stops_without_resetting_the_world() {
    use battle::commands::GunCommand as G;
    let mut r=lab();
    r.set_physics_workers(1);r.set_navigation_workers(1);
    let config=ExplosionConfig::from_ron(include_str!("../../../data/laser.ron")).unwrap();
    r.execute_command(Command::ConfigureWeapon{config});
    for y in 16..35 {for z in 60..68 {r.world.queue_transfer(voxel::Edit::put(64,y,z,3));}}
    r.world.flush_transfers();
    let target=[64,30,64];
    for origin in [[48,30,64],[48,31,63]] {
        assert_eq!(r.execute_command(Command::Gun(G::LabStream{origin,target,enabled:true})),Outcome::Applied);
        r.step();
        assert_eq!(r.muzzle().map(Fx::to_int_trunc),origin);
        assert!(r.streams.traces.iter().any(|t|t.from.map(Fx::to_int_trunc)==origin&&t.hit));
    }
    assert!(r.heat.cell(target.map(|v|v as u32)).heat>0 || r.heat.melted_total>0);
    let mut restored=BlastRange::restore(r.config,&r.snapshot()).unwrap();
    restored.set_physics_workers(1);restored.set_navigation_workers(1);
    assert_eq!(r.state_hash(),restored.state_hash());
    let stop=Command::Gun(G::LabStream{origin:[0;3],target:[0;3],enabled:false});
    assert_eq!(r.execute_command(stop),Outcome::Applied);
    assert_eq!(restored.execute_command(stop),Outcome::Applied);
    let serial=r.streams.emitted();
    for _ in 0..4 {r.step();restored.step();assert_eq!(r.state_hash(),restored.state_hash());}
    assert_eq!(r.streams.emitted(),serial);
    assert!(r.streams.traces.is_empty());
    assert!(r.world.material(64,16,60)!=0,"stop must retain the existing world");
}
#[test]
fn lab_laser_makes_steel_flow_with_heat_and_replays() {
    use battle::commands::GunCommand as G;
    let mut r=lab(); r.set_physics_workers(1); r.set_navigation_workers(1);
    for y in 30..34 { for z in 62..66 { r.world.queue_transfer(voxel::Edit::put(80,y,z,3)); }}
    r.world.flush_transfers();
    let config=ExplosionConfig::from_ron(include_str!("../../../data/laser.ron")).unwrap();
    r.execute_command(Command::ConfigureWeapon{config});
    assert_eq!(r.execute_command(Command::Gun(G::LabStream{origin:[60,32,64],target:[90,32,64],enabled:true})),Outcome::Applied);
    for _ in 0..10 { r.step(); }
    assert!(r.heat.melted_total>0,"laser never melted the wall");
    assert!(r.heat.has_molten(),"steel did not enter a flowing phase");
    let before:Vec<_>=r.heat.molten_cells().filter(|p|r.world.material(p[0],p[1],p[2])==13).collect();
    assert!(!before.is_empty());
    assert!(r.matter.economy.ledger.balanced());
    let mut back=BlastRange::restore(r.config,&r.snapshot()).unwrap();
    back.set_physics_workers(1); back.set_navigation_workers(1);
    for _ in 0..8 { r.step(); back.step(); assert_eq!(r.state_hash(),back.state_hash()); }
    let after:Vec<_>=r.heat.molten_cells().filter(|p|r.world.material(p[0],p[1],p[2])==13).collect();
    assert_ne!(before,after,"melt never moved");
    assert!(r.matter.economy.ledger.balanced());
}
#[test]
fn lab_live_stream_rejects_invalid_or_nonlaboratory_sources_atomically() {
    use battle::commands::GunCommand as G;
    let mut r=lab();
    r.execute_command(Command::ConfigureWeapon{config:ExplosionConfig::from_ron(include_str!("../../../data/flamethrower.ron")).unwrap()});
    for origin in [[0,32,64],[i32::MAX,32,64],[32,-1,64],[32,32,127]] {
        let before=r.state_hash();
        assert_eq!(r.execute_command(Command::Gun(G::LabStream{origin,target:[64,32,64],enabled:true})),Outcome::Rejected);
        assert_eq!(r.state_hash(),before);
    }
    r.layout.as_mut().unwrap().laboratory=false;
    let before=r.state_hash();
    assert_eq!(r.execute_command(Command::Gun(G::LabStream{origin:[32,32,64],target:[64,32,64],enabled:true})),Outcome::Rejected);
    assert_eq!(r.state_hash(),before);
}
#[test]
fn lab_live_flame_stops_new_packets_but_launched_fire_keeps_flying() {
    use battle::commands::GunCommand as G;
    let mut r=lab();r.set_physics_workers(1);r.set_navigation_workers(1);
    r.execute_command(Command::ConfigureWeapon{config:ExplosionConfig::from_ron(include_str!("../../../data/flamethrower.ron")).unwrap()});
    r.execute_command(Command::Gun(G::LabStream{origin:[40,40,64],target:[80,40,64],enabled:true}));
    r.step();
    assert!(!r.streams.rounds.is_empty());
    let serial=r.streams.emitted();
    let pos=r.streams.rounds[0].pos;
    r.execute_command(Command::Gun(G::LabStream{origin:[0;3],target:[0;3],enabled:false}));
    r.step();
    assert_eq!(r.streams.emitted(),serial);
    assert!(!r.streams.rounds.is_empty());
    assert_ne!(r.streams.rounds[0].pos,pos);
}
#[test]
fn lab_flamethrower_evaporates_water_on_direct_contact_and_replays() {
    use battle::commands::GunCommand as G;
    let mut r=lab(); r.set_physics_workers(1); r.set_navigation_workers(1);
    for x in 60..68 { for y in 16..19 { for z in 60..68 {
        assert!(r.water.edit_volume([x,y,z],true));
    }}}
    let initial=r.water.stats().units;
    r.execute_command(Command::ConfigureWeapon{config:ExplosionConfig::from_ron(include_str!("../../../data/flamethrower.ron")).unwrap()});
    assert_eq!(r.execute_command(Command::Gun(G::LabStream{origin:[40,17,64],target:[66,17,64],enabled:true})),Outcome::Applied);
    for _ in 0..12 {r.step();}
    assert!(r.water.stats().vapor_units>0,"flame passed through water without evaporating it");
    assert_eq!(r.water.stats().units,initial);
    let mut copy=BlastRange::restore(r.config,&r.snapshot()).unwrap();
    copy.set_physics_workers(1); copy.set_navigation_workers(1);
    let start=r.water.stats().vapor_units;
    for _ in 0..12 {r.step();copy.step();assert_eq!(r.state_hash(),copy.state_hash());}
    assert!(r.water.stats().vapor_units>start,"continued flame stopped evaporating water");
    assert_eq!(r.water.stats().units,initial);
    assert!(r.matter.economy.ledger.balanced());
}
#[test]
fn lab_laser_evaporates_water_and_cannot_heat_the_wall_behind_it() {
    use battle::commands::GunCommand as G;
    let mut r=lab(); r.set_physics_workers(1); r.set_navigation_workers(1);
    let water=[60,40,64];
    assert!(r.water.edit_volume(water,true));
    r.world.queue_transfer(voxel::Edit::put(70,40,64,3));
    r.world.flush_transfers();
    let initial=r.water.stats().units;
    r.execute_command(Command::ConfigureWeapon{config:ExplosionConfig::from_ron(include_str!("../../../data/laser.ron")).unwrap()});
    assert_eq!(r.execute_command(Command::Gun(G::LabStream{origin:[40,40,64],target:[70,40,64],enabled:true})),Outcome::Applied);
    r.step();
    assert!(r.water.stats().vapor_units>0,"laser did not evaporate water");
    assert_eq!(r.heat.cell([70,40,64]).heat,0,"water did not attenuate the beam");
    assert!(r.streams.traces.iter().any(|t|t.hit && t.to[0].to_int_trunc()<=60));
    assert_eq!(r.water.stats().units,initial);
    let mut copy=BlastRange::restore(r.config,&r.snapshot()).unwrap();
    copy.set_physics_workers(1);copy.set_navigation_workers(1);
    for _ in 0..4 {r.step();copy.step();assert_eq!(r.state_hash(),copy.state_hash());}
    assert_eq!(r.water.stats().units,initial);
}
#[test]
fn laser_thermal_halo_evaporates_nearby_water() {
    let mut r=lab(); r.set_physics_workers(1); r.set_navigation_workers(1);
    assert!(r.water.edit_volume([60,80,65],true));
    let tuning=r.streams.tuning.laser.clone();
    let centre=|p:[i32;3]|p.map(|n|Fx::from_int(n).add(Fx::from_ratio(1,2)));
    r.laser_halo(centre([40,80,64]),centre([70,80,64]),None,&tuning);
    assert!(r.water.stats().vapor_units>0,"nearby water received no halo heat");
}
#[test]
fn fuel_fire_evaporates_exposed_water_but_not_water_behind_a_wall() {
    let mut r=lab(); r.set_physics_workers(1); r.set_navigation_workers(1);
    let water=[62,80,64];
    assert!(r.water.edit_volume(water,true));
    let from=[60,80,64].map(|v|Fx::from_int(v).add(Fx::from_ratio(1,2)));
    let initial=r.water.stats().units;
    let used=r.deposit_fire_heat(from,4,1000);
    assert!(used>0 && r.water.stats().vapor_units>0,"open water received no fire heat");
    assert_eq!(r.water.stats().units,initial);
    r.world.queue_transfer(voxel::Edit::put(61,80,64,3));
    r.world.flush_transfers();
    let before=r.water.stats().vapor_units;
    r.deposit_fire_heat(from,4,1000);
    assert_eq!(r.water.stats().vapor_units,before,"fire crossed a wall to reach water");
}
#[test]
fn explosion_thermal_pulse_evaporates_water_without_crossing_a_wall() {
    for blocked in [false,true] {
        let mut r=lab(); r.set_physics_workers(1); r.set_navigation_workers(1);
        assert!(r.water.edit_volume([62,80,64],true));
        if blocked {r.world.queue_transfer(voxel::Edit::put(61,80,64,3));r.world.flush_transfers();}
        let initial=r.water.stats().units;
        r.begin_charge_impact([60,80,64],[60,80,64]);
        assert_eq!(r.water.stats().vapor_units>0,!blocked);
        assert_eq!(r.water.stats().units,initial);
    }
}
#[test]
fn pending_water_heat_survives_full_scene_snapshot() {
    let mut r=lab();r.set_physics_workers(1);r.set_navigation_workers(1);
    let p=[60,80,64];assert!(r.water.edit_volume(p,true));
    assert_eq!(r.water.absorb_heat(p,1,r.size),1);
    let mut copy=BlastRange::restore(r.config,&r.snapshot()).expect("snapshot with partial water heat");
    copy.set_physics_workers(1);copy.set_navigation_workers(1);
    for _ in 1..r.water.tuning.evaporation_heat {
        r.water.absorb_heat(p,1,r.size);copy.water.absorb_heat(p,1,copy.size);
        assert_eq!(r.state_hash(),copy.state_hash());
    }
    assert_eq!(r.water.stats().vapor_units,1);
}
#[test]
fn lab_physics_sculpt_is_gated_bounded_and_actual_removals_are_accounted() {
    let mut r = lab();
    let paint = Command::Sculpt {
        at: [64, 16, 64],
        edge: 3,
        material: 6,
        erase: false,
    };
    assert_eq!(r.execute_command(paint), Outcome::Queued);
    r.step();
    assert_eq!(r.world.material(64, 16, 64), 6);
    let before = r.state_hash();
    assert_eq!(
        r.execute_command(Command::Sculpt {
            at: [64, 16, 64],
            edge: 100,
            material: 6,
            erase: false
        }),
        Outcome::Rejected
    );
    assert_eq!(r.state_hash(), before);
    assert_eq!(
        r.execute_command(Command::Sculpt {
            at: [64, 15, 64],
            edge: 1,
            material: 6,
            erase: true
        }),
        Outcome::Queued
    );
    r.step();
    assert_eq!(r.world.material(64, 15, 64), 0);
    assert!(
        r.matter
            .economy
            .ledger
            .sunk(crate::matter::Sink::Annihilation)
            > 0
    );
    assert!(r.matter.economy.ledger.balanced());
    r.layout.as_mut().unwrap().laboratory = false;
    assert_eq!(r.execute_command(paint), Outcome::Rejected);
}
#[test]
fn lab_physics_drills_and_crawlers_cut_real_rock_and_restore_mid_work() {
    let mut r = lab();
    for x in 42..52 {
        for y in 16..20 {
            for z in 62..67 {
                r.world.queue_edit(voxel::Edit::put(x, y, z, 6));
            }
        }
    }
    r.world.flush();
    r.layout.as_mut().unwrap().lab_devices = vec![
        Device {
            kind: DeviceKind::Drill,
            at: [64, 14, 64],
            turns: 0,
            work: 0,
            dug: 0,
            container: None,
            collector: None,
        },
        Device {
            kind: DeviceKind::Crawler,
            at: [40, 17, 64],
            turns: 0,
            work: 0,
            dug: 0,
            container: None,
            collector: None,
        },
    ];
    for _ in 0..13 {
        r.step();
    }
    let mut copy = BlastRange::restore(r.config, &r.snapshot()).expect("devices snapshot");
    for _ in 0..100 {
        r.step();
        copy.step();
        assert_eq!(r.state_hash(), copy.state_hash());
    }
    let d = &r.layout.as_ref().unwrap().lab_devices;
    assert!(
        d[0].at[1] < 14 && d[0].dug > 0,
        "drill did not advance: {d:?}"
    );
    assert!(
        d[1].at[0] > 44 && d[1].dug > 0,
        "crawler stayed before rock: {d:?}"
    );
    for device in d {
        assert!(r.matter.economy.ledger.held(device.container.unwrap()) > 0);
    }
    assert!(r.matter.economy.ledger.balanced());
}
#[test]
fn lab_physics_sand_vitrifies_through_shared_heat_without_losing_volume() {
    let mut r = lab();
    let p = [64, 16, 64];
    r.world.queue_edit(voxel::Edit::put(p[0], p[1], p[2], 10));
    r.world.flush();
    r.heat.add(&r.world, p, 1600);
    r.step();
    assert_eq!(r.world.material(p[0], p[1], p[2]), 12);
    assert!(r.heat.cell(p).heat > 0, "vitrification discarded stored heat");
    assert_eq!(
        r.matter.economy.mats.get(10).unwrap().density,
        r.matter.economy.mats.get(12).unwrap().density
    );
    assert!(r.heat.melted_total > 0);
    assert!(r.matter.economy.ledger.balanced());
    let copy = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    assert_eq!(r.state_hash(), copy.state_hash());
}

#[test]
fn lab_glass_accepts_laser_heat_and_melts_again_without_losing_matter() {
    use battle::commands::GunCommand as G;
    let mut r = lab();
    r.set_physics_workers(1);
    r.set_navigation_workers(1);
    let p = [64, 30, 64];
    for y in 27..34 { for z in 61..68 {
        r.world.queue_transfer(voxel::Edit::put(64, y, z, 12));
    }}
    r.world.flush_transfers();
    r.execute_command(Command::ConfigureWeapon { config: ExplosionConfig::from_ron(include_str!("../../../data/laser.ron")).unwrap() });
    assert_eq!(r.execute_command(Command::Gun(G::LabStream { origin: [48, 30, 64], target: [64, 30, 64], enabled: true })), Outcome::Applied);
    r.step();
    assert!(r.heat.cell(p).heat > 0, "laser failed to heat glass");
    let threshold = r.matter.economy.mats.get(12).unwrap().melt;
    r.heat.add(&r.world, p, threshold);
    r.step();
    assert!(r.heat.molten_cells().any(|q|r.world.material(q[0],q[1],q[2])==16),
        "hot glass did not enter a conserved flowing phase");
    assert!(r.matter.economy.ledger.balanced());
    let copy = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    assert_eq!(r.state_hash(), copy.state_hash());
}
#[test]
fn lab_physics_buried_charge_lifts_real_ground_then_bakes_it_back() {
    let mut r = lab();
    let original = r.world.column_ceiling(64, 64);
    let volume = |r: &BlastRange| {
        (32..96)
            .flat_map(|z| (32..96).map(move |x| (x, z)))
            .map(|(x, z)| (1..64).filter(|&y| r.world.solid(x, y, z)).count())
            .sum::<usize>()
    };
    let original_volume = volume(&r);
    // Isolate the heave component from simultaneous blast removal to count
    // every cell through its world -> body -> world ownership transition.
    let energy = u64::from(r.config.material_energy_j) * 1000;
    r.matter
        .begin(r.config.material_energy_j, r.config.fragment_box);
    let reserved = r.upheaval.reserve(&r.world, [64, 8, 64], &r.config, energy);
    assert!(reserved > 0);
    r.matter.remaining_mj -= reserved;
    let mut highest = original as i64;
    let mut saved = false;
    for tick in 0..180 {
        r.step();
        for b in &r.bodies.list {
            highest = highest.max(b.oy + i64::from(b.height()));
        }
        if tick == 7 {
            let mut copy = BlastRange::restore(r.config, &r.snapshot()).expect("uplift snapshot");
            let mut a = BlastRange::restore(r.config, &r.snapshot()).unwrap();
            for _ in 0..12 {
                a.step();
                copy.step();
                assert_eq!(a.state_hash(), copy.state_hash());
            }
            saved = true;
        }
    }
    assert!(
        saved && r.upheaval.raised > 0 && highest > original as i64 + 2,
        "no dome: {highest}"
    );
    assert!(
        r.bodies.list.is_empty(),
        "uplifted earth never landed: {:?}",
        r.bodies
            .list
            .iter()
            .map(|b| (
                b.id,
                b.oy,
                b.vy,
                b.fresh,
                b.awaiting_removal(&r.world),
                b.voxels.len()
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(r.world.column_ceiling(64, 64), original);
    assert_eq!(
        volume(&r),
        original_volume,
        "heave lost or duplicated earth"
    );
    assert!(r.matter.spent_mj > 0 && r.matter.spent_mj <= r.matter.supplied_mj);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn buried_charge_varied_power_and_depth_preserves_energy_and_moves_units() {
    use modding::range::{RangeUnitDef, RangeUnitKind};
    for power in [100u32, 500, 1000, 4000] {
        for y in [1, 8, 14, 16] {
            let mut l = modding::laboratory::Definition::stock().layout;
            l.size=[128,128,128];l.clear_min=[1,1];l.clear_max=[127,127];l.gun_point=true;l.gun=[32,32,32];l.target=[64,16,64];l.laboratory=true;
            l.units=vec![RangeUnitDef{allied:true,kind:RangeUnitKind::Tank,at:[64,16,64],weapon:None}];
            let c=ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap().at_power(power);
            let mut r=BlastRange::with_layout(c,l).unwrap();r.control();r.set_physics_workers(1);r.set_navigation_workers(1);
            assert!(r.execute_command(Command::Gun(battle::commands::GunCommand::Detonate{at:[64,y,64]})).admitted());
            let initial=r.units[0].motion.pos[1];let mut max=initial;
            for _ in 0..35 {r.step();max=max.max(r.units[0].motion.pos[1]);}
            if y < 16 {
                assert!(r.upheaval.raised > 0, "no uplift: power={power} y={y}");
                assert!(max > initial, "unit remained suspended: power={power} y={y}");
            } else {
                assert_eq!(r.upheaval.raised, 0, "surface charge must not create underground uplift");
            }
            assert!(r.matter.economy.ledger.balanced());
            assert!(r.matter.spent_mj<=r.matter.supplied_mj);
        }
    }
}

#[test]
fn buried_scenario_charge_power_expands_real_uplift_and_replays() {
    let mut results = Vec::new();
    for power in [1000, 10_000, 100_000] {
        let mut l = modding::laboratory::Definition::stock().layout;
        l.size = [256, 128, 256];
        l.clear_min = [1, 1];
        l.clear_max = [255, 255];
        l.gun_point = true;
        l.gun = [32, 32, 32];
        l.target = [128, 16, 128];
        l.laboratory = true;
        l.units.clear();
        let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap().at_power(power);
        let mut r = BlastRange::with_layout(c, l).unwrap();
        r.control();
        r.set_physics_workers(1);
        r.set_navigation_workers(1);
        assert!(r.execute_command(Command::Gun(battle::commands::GunCommand::Detonate { at: [128, 8, 128] })).admitted());
        let mut radius = 0i64;
        let mut max_speed = 0i64;
        for tick in 0..8 {
            r.step();
            for body in &r.bodies.list {
                for v in &body.voxels {
                    let x = body.ox + i64::from(v.x) - 128;
                    let z = body.oz + i64::from(v.z) - 128;
                    radius = radius.max(x * x + z * z);
                }
                max_speed = max_speed.max(-body.vy.raw());
            }
            if tick == 1 {
                let mut restored = BlastRange::restore(r.config, &r.snapshot()).expect("scaled uplift snapshot");
                assert_eq!(restored.state_hash(), r.state_hash());
                let mut uninterrupted = BlastRange::restore(r.config, &r.snapshot()).unwrap();
                for _ in 0..2 {
                    restored.step();
                    uninterrupted.step();
                    assert_eq!(restored.state_hash(), uninterrupted.state_hash());
                }
            }
        }
        assert!(r.matter.economy.ledger.balanced());
        assert!(r.matter.spent_mj <= r.matter.supplied_mj);
        results.push((radius, max_speed, r.upheaval.raised));
    }
    eprintln!("buried uplift 1x, 10x, 100x: {results:?}");
    for pair in results.windows(2) {
        assert!(pair[1].0 > pair[0].0, "radius did not expand: {results:?}");
        assert!(pair[1].1 > pair[0].1, "uplift did not accelerate: {results:?}");
        assert!(pair[1].2 > pair[0].2, "uplift did not grow: {results:?}");
    }
}

#[test]
fn buried_heave_uses_edited_ground_above_the_original_surface() {
    let mut r=lab();r.set_physics_workers(1);r.set_navigation_workers(1);
    for x in 56..72 {for z in 56..72 {for y in 16..32 {
        r.world.queue_transfer(voxel::Edit::put(x,y,z,1));
    }}}
    r.world.flush_transfers();
    assert!(r.execute_command(Command::Gun(battle::commands::GunCommand::Detonate{at:[64,24,64]})).admitted());
    for _ in 0..16 {r.step();}
    assert!(r.upheaval.raised>0,"charge inside an authored mound must lift the current surface");
    assert!(r.matter.economy.ledger.balanced());
    assert!(r.matter.spent_mj<=r.matter.supplied_mj);
}

#[test]
fn buried_heave_bounds_for_weak_and_deep_charges() {
    for power in [0u32, 1, 1000, 4000] {
        for y in [1, 48, 94, 96] {
            let mut l=modding::laboratory::Definition::stock().layout;
            l.size=[128,128,128];l.ground_level=96;l.clear_min=[1,1];l.clear_max=[127,127];
            l.gun_point=true;l.gun=[32,100,32];l.target=[64,96,64];l.laboratory=true;
            let c=ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap().at_power(power);
            let mut r=BlastRange::with_layout(c,l).unwrap();r.set_physics_workers(1);r.set_navigation_workers(1);
            r.matter.begin(c.material_energy_j,c.fragment_box);
            let reserved=r.upheaval.reserve(&r.world,[64,y,64],&c,u64::from(c.material_energy_j)*1000);
            if power==0 || y==96 {assert_eq!(reserved,0,"power={power} y={y}");}
            else {assert!(reserved>0,"power={power} y={y}");}
            r.matter.remaining_mj-=reserved;
            for _ in 0..4 {r.step();}
            assert!(r.matter.spent_mj<=r.matter.supplied_mj);
            assert!(r.matter.economy.ledger.balanced());
            let back=BlastRange::restore(r.config,&r.snapshot()).unwrap();
            assert_eq!(r.state_hash(),back.state_hash());
        }
    }
}

#[test]
fn underground_lift_moves_mobile_unit_kinds_and_keeps_a_base_anchored() {
    use modding::range::{RangeUnitDef, RangeUnitKind};
    for kind in [RangeUnitKind::Tank, RangeUnitKind::Strider, RangeUnitKind::Spider, RangeUnitKind::Crawler, RangeUnitKind::Base] {
        let mut l=modding::laboratory::Definition::stock().layout;
        l.size=[256,128,256];l.clear_min=[1,1];l.clear_max=[255,255];l.gun_point=true;l.gun=[32,32,32];l.target=[128,16,128];l.laboratory=true;
        l.units=vec![RangeUnitDef{allied:true,kind,at:[128,16,128],weapon:None}];
        let c=ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
        let mut r=BlastRange::with_layout(c,l).unwrap();r.control();r.set_physics_workers(1);r.set_navigation_workers(1);
        r.matter.begin(c.material_energy_j,c.fragment_box);
        let reserved=r.upheaval.reserve(&r.world,[128,8,128],&c,u64::from(c.material_energy_j)*1000);
        r.matter.remaining_mj-=reserved;
        let initial=r.units[0].motion.pos[1];let mut peak=initial;
        for _ in 0..25 {r.step();peak=peak.max(r.units[0].motion.pos[1]);}
        if kind==RangeUnitKind::Base {assert_eq!(peak,initial,"base moved under heave");}
        else {assert!(peak>initial,"{kind:?} was not lifted");}
        assert!(r.matter.economy.ledger.balanced());
        assert!(r.matter.spent_mj<=r.matter.supplied_mj);
        let copy=BlastRange::restore(r.config,&r.snapshot()).unwrap();
        assert_eq!(r.state_hash(),copy.state_hash());
    }
}

#[test]
fn lab_physics_actual_buried_detonation_keeps_replay_and_energy_bounds() {
    let mut r = lab();
    assert!(r
        .execute_command(Command::Gun(battle::commands::GunCommand::Detonate {
            at: [64, 8, 64]
        }))
        .admitted());
    for _ in 0..8 {
        r.step();
    }
    assert!(r.upheaval.raised > 0);
    let mut copy = BlastRange::restore(r.config, &r.snapshot()).unwrap();
    for _ in 0..32 {
        r.step();
        copy.step();
        assert_eq!(r.state_hash(), copy.state_hash());
    }
    assert!(r.matter.economy.ledger.balanced());
    assert!(r.matter.spent_mj <= r.matter.supplied_mj);
}
#[test]
fn lab_physics_pending_brush_command_survives_owner_wire_checkpoint() {
    use battle::commands::runtime::*;
    let mut runtime = CommandRuntime::new(
        lab(),
        7,
        Limits {
            retained: 8,
            producers: 1,
            per_tick: 2,
        },
    )
    .unwrap();
    runtime
        .submit(Envelope {
            epoch: 7,
            producer: 0,
            sequence: 0,
            requested_tick: 2,
            command: Command::Sculpt {
                at: [64, 16, 64],
                edge: 2,
                material: 12,
                erase: false,
            },
        })
        .unwrap();
    let bytes = runtime.checkpoint().to_bytes();
    let bounds = DecodeLimits {
        world_bytes: 16 << 20,
        runtime: Limits {
            retained: 8,
            producers: 1,
            per_tick: 2,
        },
        plan_decisions: 16,
    };
    let checkpoint = Checkpoint::from_bytes(&bytes, bounds).unwrap();
    let mut restored = CommandRuntime::restore(runtime.world().config, checkpoint).unwrap();
    for _ in 0..5 {
        runtime.step().unwrap();
        restored.step().unwrap();
        assert_eq!(runtime.state_hash(), restored.state_hash());
    }
    assert_eq!(restored.world().world.material(64, 16, 64), 12);
}

#[test]
fn lab_physics_stock_laser_and_flame_vitrify_sand_through_real_fire_commands() {
    for (name, text) in [
        ("laser", include_str!("../../../data/laser.ron")),
        ("flame", include_str!("../../../data/flamethrower.ron")),
    ] {
        let mut layout = lab().layout.unwrap();
        layout.gun = [56, 18, 64];
        layout.target = [64, 16, 64];
        layout.units.clear();
        layout.unit_yaws.clear();
        layout.blocks = vec![modding::range::RangeBlock {
            min: [64, 16, 58],
            max: [76, 17, 70],
            material: 10,
        }];
        let config = ExplosionConfig::from_ron(text).unwrap();
        let mut r = BlastRange::with_layout(config, layout).unwrap();
        r.set_physics_workers(1);
        r.set_navigation_workers(1);
        r.control();
        assert!(r.refire(), "{name} fire rejected");
        let mut glass = false;
        let mut hottest = 0;
        for _ in 0..120 {
            r.step();
            hottest = hottest.max(r.heat.hottest().map_or(0, |(_, c)| c.heat));
            glass |= (58..70).any(|z| (64..76).any(|x| r.world.material(x, 16, z) == 12));
        }
        assert!(
            glass,
            "stock {name} never vitrified sand; peak={hottest}, melted={}",
            r.heat.melted_total
        );
        assert!(r.matter.economy.ledger.balanced());
    }
}
#[test]
fn lab_physics_stock_nuclear_heat_reaches_the_shared_sand_transition() {
    let mut r = lab();
    let p = [64, 16, 64];
    r.world.queue_edit(voxel::Edit::put(p[0], p[1], p[2], 10));
    r.world.flush();
    r.config =
        ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    assert_eq!(
        r.execute_command(Command::Gun(battle::commands::GunCommand::Detonate {
            at: [64, 18, 64]
        })),
        Outcome::Applied
    );
    assert!(r.heat.cell(p).heat >= r.matter.economy.mats.get(10).unwrap().melt);
    // Isolate the thermal phase from the simultaneous fracture/annihilation:
    // sufficient heat must vitrify any sand that the mechanical blast leaves.
    r.heat
        .step(&mut r.world, &r.matter.economy.mats, &r.heat_tuning, |_| {
            false
        });
    r.matter.melt_ground(&mut r.world, &mut r.heat);
    r.world.flush();
    assert_eq!(r.world.material(p[0], p[1], p[2]), 12);
    assert!(r.matter.economy.ledger.balanced());
}

#[test]
fn lab_physics_hydraulic_erosion_uses_real_current_and_replays() {
    let e = modding::laboratory::experiments::stock()
        .into_iter()
        .find(|e| e.id == "water-erosion")
        .unwrap();
    let c = ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r = BlastRange::with_layout(c, e.layout.clone()).unwrap();
    let mut control = BlastRange::with_layout(c, e.layout).unwrap();
    for s in [&mut r, &mut control] {
        s.set_physics_workers(1);
        s.set_navigation_workers(1);
        s.control();
    }
    control.granular.tuning.erosion.moves_per_tick = 0;
    let mut peak = 0;
    for _ in 0..300 {
        r.step();
        control.step();
        peak = peak.max(r.granular.moved);
    }
    let holes = |r: &BlastRange| {
        (384..388)
            .flat_map(|x| (326..346).map(move |z| (x, z)))
            .map(|(x, z)| (17..19).filter(|&y| r.world.material(x, y, z) == 0).count())
            .sum::<usize>()
    };
    eprintln!(
        "erosion bank: moved={} control={} holes={} control_holes={}",
        peak,
        control.granular.moved,
        holes(&r),
        holes(&control)
    );
    assert!(
        r.granular.moved > control.granular.moved && holes(&r) > holes(&control),
        "the real water current did not erode its sample bank"
    );
    let mut copy = BlastRange::restore(c, &r.snapshot()).unwrap();
    copy.set_physics_workers(1);
    copy.set_navigation_workers(1);
    for _ in 0..60 {
        r.step();
        copy.step();
        assert_eq!(r.state_hash(), copy.state_hash());
    }
    assert_eq!(r.water.stats().units, r.water.stats().initial_units);
    assert!(r.matter.economy.ledger.balanced());
}
