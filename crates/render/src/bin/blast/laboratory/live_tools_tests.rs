use super::*;

#[test]
fn live_he_debris_settles_after_a_ground_shot() {
    let (mut lab,mut scene)=setup();
    action(&mut lab,&mut scene,LabAction::LiveTools);
    action(&mut lab,&mut scene,LabAction::LiveTool(6));
    action(&mut lab,&mut scene,LabAction::LiveApply([64,16,64],[40,24,64]));
    for _ in 0..40 {tick(&mut lab,&mut scene);scene.sim.step();if scene.sim.shell.is_some() || scene.sim.impact.is_some(){break;}}
    action(&mut lab,&mut scene,LabAction::LiveRelease);
    for _ in 0..150 {scene.sim.step();}
    let pieces=&scene.sim.matter.debris.fragments;
    let flying=pieces.iter().filter(|f|!f.resting).count();
    let max_y=pieces.iter().map(|f|f.position.y.to_int_trunc()).max().unwrap_or(0);
    println!("live HE: tick={} impact={:?} pieces={} flying={} resting={} max_y={}",scene.sim.tick,scene.sim.impact,pieces.len(),flying,pieces.len()-flying,max_y);
    println!("full={} sparse={} rotated={} above_25={}",pieces.iter().filter(|f|f.shape==u64::MAX&&f.side==4).count(),pieces.iter().filter(|f|f.shape!=u64::MAX).count(),pieces.iter().filter(|f|f.basis!=sim_core::burst::IDENTITY).count(),pieces.iter().filter(|f|f.resting&&f.position.y.to_int_trunc()>25).count());
    println!("shape 1={} 2-8={} 9-32={} 33+={}",pieces.iter().filter(|f|f.shape.count_ones()==1).count(),pieces.iter().filter(|f|(2..=8).contains(&f.shape.count_ones())).count(),pieces.iter().filter(|f|(9..=32).contains(&f.shape.count_ones())).count(),pieces.iter().filter(|f|f.shape.count_ones()>32).count());
    let mut false_support=0;
    let cells=|q:&sim_core::burst::Fragment| {
        let qp=[q.position.x,q.position.y,q.position.z].map(|v|v.to_f64_lossy());
        let basis=q.basis.map(|v|[v.x,v.y,v.z].map(|x|x.to_f64_lossy()));
        let centre=sim_core::burst::shape_centre(q.shape,q.side);let centre=[centre.x,centre.y,centre.z].map(|v|v.to_f64_lossy());
        let scale=q.mm as f64/100.0;let radius=std::array::from_fn::<_,3,_>(|k|basis.iter().map(|v|v[k].abs()).sum::<f64>()*0.49*scale);
        (0..u32::from(q.side).pow(3)).filter(|&bit|q.shape&(1u64<<bit)!=0).map(|bit|{
            let s=q.side as u32;let cell=[bit%s,(bit/s)%s,bit/(s*s)];
            let p=std::array::from_fn::<_,3,_>(|k|qp[k]+(0..3).map(|axis|basis[axis][k]*(cell[axis] as f64-centre[axis])*scale).sum::<f64>());
            (std::array::from_fn::<_,3,_>(|k|p[k]-radius[k]),std::array::from_fn::<_,3,_>(|k|p[k]+radius[k]))
        }).collect::<Vec<_>>()
    };
    let mut false_moving_support=0;
    for (index,f) in pieces.iter().enumerate().filter(|(_,f)|f.resting) {
        let p=[f.position.x,f.position.y,f.position.z].map(|v|v.to_f64_lossy());
        let (a,b)=f.bounds();let lo=std::array::from_fn::<_,3,_>(|k|p[k]+a[k].to_f64_lossy());let hi=std::array::from_fn::<_,3,_>(|k|p[k]+b[k].to_f64_lossy());
        if lo[1]<0.5 {continue;}
        let low=[lo[0],lo[1]-0.5,lo[2]].map(|v|v.floor().clamp(0.0,127.0) as u32);
        let high=hi.map(|v|v.floor().clamp(0.0,127.0) as u32);
        if scene.sim.world.any_solid_box(low,high){continue;}
        let mut broad=false;let mut actual=false;let mut actual_pair=false;let moving_cells=cells(f);
        for (other,q) in pieces.iter().enumerate().filter(|(i,q)|*i!=index&&q.resting) {
            let _=other;let qp=[q.position.x,q.position.y,q.position.z].map(|v|v.to_f64_lossy());let (qa,qb)=q.bounds();
            let qlo=std::array::from_fn::<_,3,_>(|k|qp[k]+qa[k].to_f64_lossy());let qhi=std::array::from_fn::<_,3,_>(|k|qp[k]+qb[k].to_f64_lossy());
            let testlo=[lo[0],lo[1]-0.5,lo[2]];
            if qhi[1]>lo[1]||!(0..3).all(|k|testlo[k]<qhi[k]&&qlo[k]<hi[k]){continue;}
            broad=true;
            let basis=q.basis.map(|v|[v.x,v.y,v.z].map(|x|x.to_f64_lossy()));
            let centre=sim_core::burst::shape_centre(q.shape,q.side);let centre=[centre.x,centre.y,centre.z].map(|v|v.to_f64_lossy());
            let scale=q.mm as f64/100.0;let radius=std::array::from_fn::<_,3,_>(|k|basis.iter().map(|v|v[k].abs()).sum::<f64>()*0.49*scale);
            for bit in 0..u32::from(q.side).pow(3) {if q.shape&(1u64<<bit)==0{continue;}
                let s=q.side as u32;let cell=[bit%s,(bit/s)%s,bit/(s*s)];
                let centre_world=std::array::from_fn::<_,3,_>(|k|qp[k]+(0..3).map(|axis|basis[axis][k]*(cell[axis] as f64-centre[axis])*scale).sum::<f64>());
                let cell_lo=std::array::from_fn::<_,3,_>(|k|centre_world[k]-radius[k]);
                let cell_hi=std::array::from_fn::<_,3,_>(|k|centre_world[k]+radius[k]);
                if (0..3).all(|k|testlo[k]<cell_hi[k]&&cell_lo[k]<hi[k]) {
                    actual=true;
                    if moving_cells.iter().any(|(a,b)|(0..3).all(|k|a[k]-if k==1 {0.5}else{0.0}<cell_hi[k]&&cell_lo[k]<b[k])) {actual_pair=true;break;}
                }
            }
            if actual_pair {break;}
        }
        if broad&&!actual {false_support+=1;}
        if actual&&!actual_pair {false_moving_support+=1;}
    }
    assert_eq!(false_support,0,"a resting piece is supported only by empty parts of other fragment boxes");
    assert_eq!(false_moving_support,0,"a resting piece is supported only under its own empty bounding corner");
    assert!(scene.sim.impact.is_some(),"live gun must actually fire");
    assert!(!pieces.is_empty(),"ground shot must produce fragments");
}

#[test]
fn scenario_edits_actor_commands_and_device_parameters_atomically() {
    use modding::laboratory::experiments::{Input,Plan};
    use modding::lab_tools::{Device,DeviceKind,Collector};
    let (mut lab,scene)=setup();
    let mut layout=scene.sim.layout.clone().unwrap();
    layout.battle=true;
    let mut unit=lab.experiments.iter().find(|e|e.id=="navigation").or_else(||lab.experiments.iter().find(|e|e.plan.inputs.iter().any(|i|matches!(i,Input::Move{..})))).unwrap().layout.units[0].clone();
    unit.at=[64,16,64];layout.units.push(unit);
    lab.constructor.base=Some(layout);
    lab.constructor.experiment=Some(Plan{name:"Проверка".into(),description:String::new(),duration_ticks:100,inputs:vec![Input::FreeControl,Input::Move{actor:0,to:[80,64]}]});
    lab.ui.scenario_input=1;
    scenario::set_number(&mut lab,61,85,&scene).unwrap();
    scenario::set_number(&mut lab,63,75,&scene).unwrap();
    assert!(matches!(lab.constructor.experiment.as_ref().unwrap().inputs[1],Input::Move{actor:0,to:[85,75]}));
    let previous=lab.constructor.experiment.clone();
    assert!(scenario::set_number(&mut lab,60,2,&scene).is_err());
    assert_eq!(lab.constructor.experiment,previous);
    let at=[60,16,60];
    lab.constructor.workspace.devices.push(Device{kind:DeviceKind::Collector,at,turns:0,work:0,dug:0,container:None,collector:Some(Collector::at(at))});
    scenario::set_number(&mut lab,70,65,&scene).unwrap();
    scenario::set_number(&mut lab,74,1234,&scene).unwrap();
    assert_eq!(lab.constructor.workspace.devices[0].at[0],65);
    assert_eq!(lab.constructor.workspace.devices[0].collector.as_ref().unwrap().capacity,1234);
    let before=lab.constructor.workspace.clone();
    assert!(scenario::set_number(&mut lab,75,-1,&scene).is_err());
    assert_eq!(lab.constructor.workspace,before);
}

#[test]
fn scenario_multiple_sources_wait_for_completion_and_keep_separate_settings() {
    use modding::laboratory::experiments::{Input,Plan};
    let (mut lab,mut scene)=setup();
    lab.constructor.base=scene.sim.layout.clone();
    lab.constructor.experiment=Some(Plan{name:"Два заряда".into(),description:String::new(),duration_ticks:100,inputs:vec![Input::Detonate{at:[60,20,64]},Input::Detonate{at:[74,20,64]}]});
    scenario::materialize(&mut lab).unwrap();
    for s in &mut lab.constructor.workspace.sources {s.config.half_cells=3;s.power_milli=100;}
    lab.constructor.workspace.sources[1].power_milli=200;
    lab.fire(&mut scene);
    scenario::advance(&mut lab,&mut scene);
    assert_eq!(scene.sim.impact,Some([60,20,64]));
    assert_eq!(lab.launch.len(),1);
    for _ in 0..150 {
        scenario::advance(&mut lab,&mut scene);
        if lab.launch.is_empty(){break;}
        scene.sim.step();
    }
    assert!(lab.launch.is_empty(),"the second charge must not be dropped while the first is busy");
    assert_eq!(scene.sim.impact,Some([74,20,64]));
    assert_eq!(scene.sim.config.energy,lab.constructor.workspace.sources[1].resolved().energy);
}

#[test]
fn scenario_reorder_and_remove_follow_the_correct_source() {
    use modding::laboratory::experiments::{Input,Plan};
    let (mut lab,scene)=setup();
    lab.constructor.base=scene.sim.layout.clone();
    lab.constructor.experiment=Some(Plan{name:"Порядок".into(),description:String::new(),duration_ticks:100,inputs:vec![Input::Detonate{at:[60,20,64]},Input::Detonate{at:[74,20,64]}]});
    scenario::materialize(&mut lab).unwrap();
    scenario::action(&mut lab,LabAction::ScenarioMove(1),&scene).unwrap();
    assert_eq!(lab.constructor.workspace.sources[0].input,Some(1));
    assert_eq!(lab.constructor.workspace.sources[1].input,Some(0));
    assert_eq!(lab.constructor.experiment.as_ref().unwrap().inputs[0],Input::Detonate{at:[74,20,64]});
    scenario::action(&mut lab,LabAction::ScenarioRemove,&scene).unwrap();
    assert_eq!(lab.constructor.workspace.sources.len(),1);
    assert_eq!(lab.constructor.workspace.sources[0].at,[74,20,64]);
    assert_eq!(lab.constructor.workspace.sources[0].input,Some(0));
    assert_eq!(lab.constructor.experiment.as_ref().unwrap().inputs.len(),1);
}

#[test]
fn scenario_deleting_a_unit_removes_its_orders_and_remaps_the_remaining_unit() {
    use modding::laboratory::{catalog,experiments::{Input,Plan}};
    let (mut lab,scene)=setup();
    let mut layout=scene.sim.layout.clone().unwrap();layout.battle=true;
    let mut first=lab.experiments.iter().find(|e|e.plan.inputs.iter().any(|i|matches!(i,Input::Move{..}))).unwrap().layout.units[0].clone();
    first.at=[48,16,48];let mut second=first.clone();second.at=[80,16,80];
    layout.units=vec![first,second];
    lab.constructor.placed=catalog::parts_of_layout(layout.clone()).unwrap();
    layout.units.clear();layout.unit_yaws.clear();
    lab.constructor.base=Some(layout);
    lab.constructor.experiment=Some(Plan{name:"Участники".into(),description:String::new(),duration_ticks:100,inputs:vec![Input::FreeControl,Input::Move{actor:0,to:[50,50]},Input::Move{actor:1,to:[75,75]}]});
    lab.constructor.selected=Some(0);
    lab.constructor.action(LabAction::Delete,&lab.definition,&mut lab.recipe).unwrap();
    let inputs=&lab.constructor.experiment.as_ref().unwrap().inputs;
    assert_eq!(inputs.len(),2);
    assert!(matches!(inputs[1],Input::Move{actor:0,to:[75,75]}));
    scenario::validate(&lab,&lab.constructor.layout(&lab.definition,lab.recipe).unwrap()).unwrap();
}

#[test]
fn scenario_rejects_stale_or_duplicate_source_links() {
    use modding::laboratory::experiments::{Input,Plan};
    let (mut lab,scene)=setup();lab.constructor.base=scene.sim.layout.clone();
    lab.constructor.experiment=Some(Plan{name:"Связи".into(),description:String::new(),duration_ticks:100,inputs:vec![Input::Detonate{at:[60,20,64]}]});
    scenario::materialize(&mut lab).unwrap();
    let layout=lab.constructor.layout(&lab.definition,lab.recipe).unwrap();
    assert!(scenario::validate(&lab,&layout).is_ok());
    lab.constructor.workspace.sources[0].at[0]+=1;
    assert!(scenario::validate(&lab,&layout).is_err());
    scenario::sync_sources(&mut lab);
    let copy=lab.constructor.workspace.sources[0].clone();lab.constructor.workspace.sources.push(copy);
    assert!(scenario::validate(&lab,&layout).is_err());
}

#[test]
fn entering_live_tools_cancels_an_authored_burst_before_switching_to_a_cannon() {
    let (mut lab, mut scene) = setup();
    scene.sim.execute_command(Command::ConfigureWeapon {
        config: weapon_config(false, false, 1000, Delivery::Laser),
    });
    scene
        .sim
        .execute_command(Command::Gun(GunCommand::LabStream {
            origin: [40, 40, 64],
            target: [90, 40, 64],
            enabled: true,
        }));
    assert!(scene.sim.streams.left > 0);
    action(&mut lab, &mut scene, LabAction::LiveTools);
    assert_eq!(scene.sim.streams.left, 0);
    action(&mut lab, &mut scene, LabAction::LiveTool(6));
    action(
        &mut lab,
        &mut scene,
        LabAction::LiveApply([90, 40, 64], [40, 40, 64]),
    );
    tick(&mut lab, &mut scene);
    scene.sim.step();
    assert!(
        scene.sim.shell.is_some(),
        "a stale stream must not lock the cannon forever"
    );
}

#[test]
fn live_fire_keeps_emitting_with_a_custom_specimen_resistance() {
    for tool in [3, 4, 5] {
        let mut emitted = None;
        for resistance in [180, 720] {
            let (mut lab, mut scene) = setup();
            let mut layout = scene.sim.layout.clone().unwrap();
            layout.unit_resistance = resistance;
            scene.sim = BlastRange::with_layout(lab.config(), layout).unwrap();
            scene.sim.set_physics_workers(1);
            scene.sim.set_navigation_workers(1);
            action(&mut lab, &mut scene, LabAction::LiveTools);
            action(&mut lab, &mut scene, LabAction::LiveTool(tool));
            action(
                &mut lab,
                &mut scene,
                LabAction::LiveApply([90, 40, 64], [40, 40, 64]),
            );
            for _ in 0..8 {
                tick(&mut lab, &mut scene);
                scene.sim.step();
            }
            let count = scene.sim.streams.emitted();
            assert!(count >= 5, "tool={tool}: held fire stalled");
            if let Some(want) = emitted {
                assert_eq!(count, want, "custom resistance throttled tool={tool}");
            } else {
                emitted = Some(count);
            }
            assert_eq!(scene.sim.config.unit_ablation_resistance, resistance);
        }
    }
}

#[test]
fn live_fire_works_in_a_lab_scene_without_a_point_source() {
    let (mut lab, mut scene) = setup();
    let mut layout = scene.sim.layout.clone().unwrap();
    layout.gun_point = false;
    layout.size[0] = 256;
    layout.clear_max[0] = 255;
    layout.target[0] = 200;
    scene.sim = BlastRange::with_layout(lab.config(), layout).unwrap();
    scene.sim.set_physics_workers(1);
    scene.sim.set_navigation_workers(1);
    action(&mut lab, &mut scene, LabAction::LiveTools);
    action(&mut lab, &mut scene, LabAction::LiveTool(4));
    action(
        &mut lab,
        &mut scene,
        LabAction::LiveApply([90, 40, 64], [40, 40, 64]),
    );
    tick(&mut lab, &mut scene);
    scene.sim.step();
    assert!(
        scene.sim.streams.emitted() > 0,
        "the flying tool must not require an authored gun"
    );
    assert_eq!(
        scene.sim.streams.traces[0].from.map(fx::Fx::to_int_trunc),
        [40, 40, 64]
    );
}

#[test]
fn live_weapons_damage_real_materials_and_release_keeps_the_flying_shell() {
    for tool in [3, 4, 5, 6, 7] {
        let (mut lab, mut scene) = setup();
        // A ground-supported wooden wall, reached from an arbitrary heading.
        for x in 58..63 {
            for y in 16..54 {
                for z in 48..81 {
                    scene.sim.world.queue_transfer(voxel::Edit::put(x, y, z, 5));
                }
            }
        }
        scene.sim.world.flush_transfers();
        action(&mut lab, &mut scene, LabAction::LiveTools);
        action(&mut lab, &mut scene, LabAction::LiveTool(tool));
        action(
            &mut lab,
            &mut scene,
            LabAction::LiveApply([62, 35, 64], [95, 35, 64]),
        );
        tick(&mut lab, &mut scene);
        scene.sim.step();
        assert!(lab.tools.emitting, "tool={tool}: emitter rejected");
        if tool >= 6 {
            for _ in 0..scene.sim.config.charge_ticks {
                tick(&mut lab, &mut scene);
                scene.sim.step();
            }
            let shell = scene.sim.shell;
            assert!(
                shell.is_some() || scene.sim.impact.is_some(),
                "tool={tool}: cannon must launch a real shell"
            );
            let x = shell
                .map(|p| p[0].to_int_trunc())
                .or_else(|| scene.sim.impact.map(|p| p[0]))
                .unwrap();
            assert!(
                x < 90,
                "point shot must follow the crosshair without turret lag"
            );
            action(&mut lab, &mut scene, LabAction::LiveRelease);
            assert_eq!(
                scene.sim.shell, shell,
                "release must not erase a fired shell"
            );
        }
        for _ in 0..24 {
            tick(&mut lab, &mut scene);
            scene.sim.step();
        }
        let heat = scene.sim.heat.cell([62, 35, 64]).heat;
        let changed = scene.sim.world.material(62, 35, 64) != 5
            || scene.sim.removed > 0
            || heat > 0
            || scene.sim.heat.burned_total > 0
            || scene.sim.heat.melted_total > 0;
        assert!(
            changed,
            "tool={tool}: no physical impact or heat after firing"
        );
        assert!(scene.sim.matter.economy.ledger.balanced());
    }
}

#[test]
fn live_weapons_can_aim_into_empty_space_without_enabling_air_point() {
    let (mut lab, scene) = setup();
    lab.tools.live = true;
    lab.tools.flying = true;
    let pose = Transform::from_xyz(64.5, 60.5, 64.5);
    lab.tools.camera = Some(pose);
    let ray = Ray3d::new(pose.translation, pose.forward());
    assert!(tools::liquid_pick(&scene, ray).is_none());
    for tool in [3, 4, 5, 6, 7] {
        lab.tools.live_tool = tool;
        let (at, origin) = target(&lab, &scene, ray).expect("empty sky is a valid weapon aim");
        assert_eq!(&at[..2], &[64, 60]);
        assert!(at[2] < origin[2]);
    }
    lab.tools.live_tool = 0;
    assert!(
        target(&lab, &scene, ray).is_none(),
        "sculpting must still require a surface"
    );
}

#[test]
fn live_material_and_water_sources_match_at_low_and_high_render_rates_and_stop_on_release() {
    for tool in [1, 2] {
        let mut expected = None;
        for fps in [8, 15, 30, 60, 200] {
            let (mut lab, mut scene) = setup();
            action(&mut lab, &mut scene, LabAction::LiveTools);
            lab.tools.edge = 1;
            lab.constructor.pick_material(10, &lab.definition).unwrap();
            action(&mut lab, &mut scene, LabAction::LiveTool(tool));
            let mut clock = render::realtime::TickClock::default();
            let dt = 1.0 / f64::from(sim_core::TICKS_PER_SECOND);
            clock.advance(0.0, 1, dt, 8);
            for _ in 0..fps * 2 {
                action(
                    &mut lab,
                    &mut scene,
                    LabAction::LiveApply([60, 40, 64], [40, 40, 64]),
                );
                for _ in 0..clock.advance(1.0 / f64::from(fps), 1, dt, 8) {
                    tick(&mut lab, &mut scene);
                    scene.sim.step();
                }
            }
            assert_eq!(scene.sim.tick, 40);
            let hash = scene.sim.state_hash();
            if let Some(want) = expected {
                assert_eq!(hash, want, "tool={tool} fps={fps}");
            } else {
                expected = Some(hash);
            }
            let initial = scene.sim.water.stats().initial_units;
            if tool == 2 {
                assert!(initial > 4096, "fixture must pour repeatedly");
            }
            let mouse = ButtonInput::default();
            release(&mut lab, &mouse, false);
            assert!(lab.constructor.pending.contains(&LabAction::LiveRelease));
            action(&mut lab, &mut scene, LabAction::LiveRelease);
            assert!(lab.tools.live_target.is_none());
            for _ in 0..3 {
                tick(&mut lab, &mut scene);
                scene.sim.step();
            }
            assert_eq!(
                scene.sim.water.stats().initial_units,
                initial,
                "released water must not keep spawning"
            );
            assert!(scene.sim.matter.economy.ledger.balanced());
        }
    }
}
fn setup() -> (Laboratory, Scene) {
    let mut lab = Laboratory::new(false);
    lab.constructor.stand = false;
    let mut l = lab.definition.layout.clone();
    l.size = [128, 128, 128];
    l.clear_min = [1, 1];
    l.clear_max = [127, 127];
    l.gun_point = true;
    l.laboratory = true;
    l.gun = [40, 40, 64];
    l.target = [64, 16, 64];
    let mut sim = BlastRange::with_layout(lab.config(), l).unwrap();
    sim.control();
    sim.set_physics_workers(1);
    sim.set_navigation_workers(1);
    let scene = Scene {
        sim,
        generation: 11,
        energy: false,
        anni: false,
        paused: true,
        frame: 0,
        finish: 0,
        out: None,
        all_frames: false,
        wait: 0,
        captured: 0,
        acc: 0.0,
        repeat: false,
        view: 1,
        camera_sweep: false,
        patrol: 0,
        previous_shell: None,
        fps: 60.0,
        step_us: 0,
        flame: 0,
        realtime: true,
        live_elapsed: 0.0,
        last_capture: 0,
        layout_file: None,
        power: 1000,
        uncovered: false,
        unit_override: None,
        skip_delta: false,
        notice: String::new(),
    };
    (lab, scene)
}
#[test]
fn live_tools_enter_without_rebuilding_and_apply_water_material_and_dig_to_the_running_simulation()
{
    let (mut lab, mut scene) = setup();
    let original = scene.sim.state_hash();
    action(&mut lab, &mut scene, LabAction::LiveTools);
    assert_eq!(scene.sim.state_hash(), original);
    assert_eq!(scene.generation, 11);
    assert!(lab.tools.flying && !scene.paused && !lab.observing && lab.deadline.is_none());
    assert!(ui::enabled(&lab, LabAction::LiveTool(2)));
    assert!(!ui::enabled(&lab, LabAction::Fire));
    lab.tools.edge = 1;
    action(&mut lab, &mut scene, LabAction::LiveTool(2));
    action(
        &mut lab,
        &mut scene,
        LabAction::LiveApply([60, 16, 64], [40, 40, 64]),
    );
    tick(&mut lab, &mut scene);
    assert!(scene.sim.water.amount([60, 16, 64]) > 0);
    let hash = scene.sim.state_hash();
    action(
        &mut lab,
        &mut scene,
        LabAction::LiveApply([61, 16, 64], [40, 40, 64]),
    );
    tick(&mut lab, &mut scene);
    assert_eq!(
        scene.sim.state_hash(),
        hash,
        "at most one input per simulation tick"
    );
    assert!(
        !lab.step_requested,
        "running simulation must not be forced into one-step mode"
    );
    scene.sim.step();
    assert!(scene.sim.tick > 0);
    action(&mut lab, &mut scene, LabAction::LiveTool(1));
    lab.constructor.pick_material(3, &lab.definition).unwrap();
    action(
        &mut lab,
        &mut scene,
        LabAction::LiveApply([64, 16, 64], [40, 40, 64]),
    );
    tick(&mut lab, &mut scene);
    scene.sim.step();
    assert_eq!(scene.sim.world.material(64, 16, 64), 3);
    action(&mut lab, &mut scene, LabAction::LiveTool(0));
    action(
        &mut lab,
        &mut scene,
        LabAction::LiveApply([64, 16, 64], [40, 40, 64]),
    );
    tick(&mut lab, &mut scene);
    scene.sim.step();
    assert_eq!(scene.sim.world.material(64, 16, 64), 0);
    action(&mut lab, &mut scene, LabAction::LiveTools);
    assert!(!lab.tools.live && !scene.paused);
    assert_eq!(scene.generation, 11);
    assert!(scene.sim.tick > 0);
}
#[test]
fn live_tools_release_over_ui_or_focus_loss_stops_real_emission_without_stuck_fire() {
    let (mut lab, mut scene) = setup();
    action(&mut lab, &mut scene, LabAction::LiveTools);
    action(&mut lab, &mut scene, LabAction::LiveTool(4));
    action(
        &mut lab,
        &mut scene,
        LabAction::LiveApply([80, 40, 64], [40, 40, 64]),
    );
    tick(&mut lab, &mut scene);
    assert!(lab.tools.emitting);
    scene.sim.step();
    assert!(!scene.sim.streams.traces.is_empty());
    let emitted = scene.sim.streams.emitted();
    let mut mouse = ButtonInput::default();
    mouse.press(MouseButton::Left);
    release(&mut lab, &mouse, true);
    release(&mut lab, &mouse, true);
    assert!(lab.constructor.pending == vec![LabAction::LiveRelease]);
    action(&mut lab, &mut scene, LabAction::LiveRelease);
    assert!(!lab.tools.emitting);
    scene.sim.step();
    assert_eq!(scene.sim.streams.emitted(), emitted);
    assert!(scene.sim.streams.traces.is_empty());
    scene.paused = true;
    let before = scene.sim.state_hash();
    action(
        &mut lab,
        &mut scene,
        LabAction::LiveApply([80, 40, 64], [40, 40, 64]),
    );
    tick(&mut lab, &mut scene);
    assert_eq!(
        scene.sim.state_hash(),
        before,
        "explicit pause freezes live instruments"
    );
}
#[test]
fn live_tools_surface_and_air_point_have_distinct_real_targets_and_ten_cm_brush() {
    let (mut lab, scene) = setup();
    lab.tools.live = true;
    lab.tools.edge = 1;
    lab.tools.live_tool = 2;
    let ray = Ray3d::new(Vec3::new(64.5, 40.0, 64.5), Dir3::NEG_Y);
    let (surface, _) = target(&lab, &scene, ray).unwrap();
    assert_eq!(surface, [64, 16, 64]);
    lab.tools.point = true;
    lab.tools.distance = 8.0;
    let (air, _) = target(&lab, &scene, ray).unwrap();
    assert_eq!(air, [64, 32, 64]);
    assert_eq!(voxel::unit::WORLD_MM, 100);
    assert_ne!(surface, air);
}
