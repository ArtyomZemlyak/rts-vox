//! Deterministic start/stop/pivot/backstep diagnostics for shipped walking rigs.
//! Timings cover SIM only. CSV records actual authoritative ankle contacts.
use fx::{angle, Fx};
use modding::range::{RangeLayout, RangeUnitDef, RangeUnitKind};
use sim_core::blast::{battle::Order, BlastRange};
use std::{
    fs::File,
    io::{BufWriter, Write},
    time::Instant,
    collections::BTreeMap,
};

fn mm(v: Fx) -> i64 {
    ((i128::from(v.0) * i128::from(voxel::unit::WORLD_MM)) >> 32) as i64
}
fn main() {
    println!("gait_meta tick_rate={} world_mm={} ticks=300 restore_at=45", sim_core::TICKS_PER_SECOND, voxel::unit::WORLD_MM);
    let args: Vec<_> = std::env::args().collect();
    let value = |key: &str| args.windows(2).find(|v| v[0] == key).map(|v| v[1].as_str());
    let terrain=args.iter().any(|s|s=="--terrain");
    let barriers=args.iter().any(|s|s=="--barriers");
    let profile=args.iter().any(|s|s=="--profile");
    let tracks=args.iter().any(|s|s=="--tracks");
    let hard_turns=args.iter().any(|s|s=="--hard-turns");
    let reason_trace=args.iter().any(|s|s=="--reason-trace");
    let prefix = value("--out").unwrap_or("out/gait566");
    let mut reasons=reason_trace.then(||BufWriter::new(File::create(format!("{prefix}.reasons.csv")).unwrap()));
    if let Some(w)=reasons.as_mut() {writeln!(w,"kind,tick,leg,reason,was_planted,planted,proposed_dx_q32,proposed_dz_q32,actual_dx_q32,actual_dz_q32,proposed_yaw,actual_yaw").unwrap();}
    let mut csv = BufWriter::new(File::create(format!("{prefix}.csv")).unwrap());
    writeln!(csv,"kind,tick,leg,phase,root_x_mm,root_y_mm,root_z_mm,body_head,ankle_x_mm,ankle_y_mm,ankle_z_mm,planted,lift_mm,body_up_x_q32,body_up_z_q32,contact_error_mm,has_feet,ready,support_state,support_reason,support_kind,support_id,support_content_rev,support_pose_rev,support_mask,support_samples").unwrap();
    let config = modding::explosion::ExplosionConfig::from_ron(include_str!(
        "../../../../data/explosion.ron"
    ))
    .unwrap();
    let kinds:&[RangeUnitKind]=if tracks {&[
        RangeUnitKind::Tank,RangeUnitKind::EidolonTank,RangeUnitKind::Artillery,
        RangeUnitKind::LightningTank,RangeUnitKind::ArtilleryT2,RangeUnitKind::Crawler,
    ]} else {&[
        RangeUnitKind::Commander,RangeUnitKind::Walker04,
        RangeUnitKind::Strider,RangeUnitKind::Spider,
    ]};
    if let Some(name)=value("--kind") {
        assert!(kinds.iter().any(|k|name.eq_ignore_ascii_case(&format!("{k:?}"))),"unknown gait unit: {name}");
    }
    for &kind in kinds {
        let name = format!("{kind:?}").to_lowercase();
        if value("--kind").is_some_and(|v|!v.eq_ignore_ascii_case(&name)) {continue;}
        let mut layout =
            RangeLayout::from_ron(include_str!("../../../../data/range-motion-yard.ron")).unwrap();
        layout.size = [2048, 512, 2048];
        layout.clear_max = [2048, 2048];
        layout.blocks.clear();
        let course = layout.course.as_mut().unwrap();
        course.max = [2008, 2008];
        course.ramps.clear();
        course.patrols.truncate(1);
        course.patrols[0].unit = 0;
        course.patrols[0].points = vec![[1400, 1024], [1024, 1024]];
        if terrain {
            course.ramps=vec![
                modding::range::RangeRamp {min:[1100,16,900],max:[1180,28,1150],axis:0,reverse:false},
                modding::range::RangeRamp {min:[1220,16,900],max:[1300,28,1150],axis:0,reverse:true},
            ];
            layout.blocks=vec![modding::range::RangeBlock {min:[1180,16,900],max:[1220,28,1150],material:1}];
        }
        if barriers {
            layout.blocks=vec![modding::range::RangeBlock {min:[1140,16,700],max:[1148,44,1350],material:1}];
        }
        layout.units = vec![RangeUnitDef {
            kind,
            at: [1024, 16, 1024],
            allied: true,
            weapon: None,
        }];
        let mut sim = BlastRange::with_layout(config, layout).unwrap();
        sim.set_navigation_workers(1);
        sim.set_physics_workers(1);
        sim.start_free_control().unwrap();
        let mut times = Vec::new();
        let mut previous = vec![None; sim.units[0].motion.feet.len()];
        let mut airborne = vec![false; previous.len()];
        let mut lifts = 0;
        let mut first_lift_mm = 0i64;
        let mut highest_mm = 0i64;
        let mut replay = None;
        let mut phases=BTreeMap::new();let mut details=BTreeMap::new();let mut unit_details=BTreeMap::new();
        let mut reason_rows=0;let mut reason_dropped=0;let mut window_rejected=0;let mut window_limited=0;let mut window_unready=0;
        for tick in 0..300 {
            sim.units[0].motion.trace_anchors(reason_trace && tick<=260);
            sim.units[0].motion.clear_anchor_trace();
            let input = if hard_turns && (200..260).contains(&tick) && (tick-200)%6==0 {
                Some(if (tick-200)/6%2==0 {[1,-1]}else{[-1,1]})
            } else if terrain || barriers {match tick {0=>Some([1,0]),260=>Some([0,0]),_=>None}} else {match tick {
                0 => Some([1, 0]),
                80 => Some([0, 0]),
                110 => Some([0, 1]),
                160 => Some([-1, 0]),
                200 => Some([1, -1]),
                260 => Some([0, 0]),
                _ => None,
            }};
            if let Some([throttle, turn]) = input {
                let order = Order::Pilot {
                    throttle,
                    turn,
                    aim: [1600, 32, 1024],
                    fire: false,
                };
                assert!(sim.battle_order(0, order, None));
                if let Some(copy) = replay.as_mut() {
                    let copy: &mut BlastRange = copy;
                    assert!(copy.battle_order(0, order, None));
                }
            }
            let begin = Instant::now();
            if profile {
                let mut last=Instant::now();let mut last_detail=last;let mut last_unit=last;
                sim.step_observed_detail(|stage| {
                    *phases.entry(stage).or_insert(0u128)+=last.elapsed().as_nanos();last=Instant::now();
                },|stage| {
                    *details.entry(stage).or_insert(0u128)+=last_detail.elapsed().as_nanos();last_detail=Instant::now();
                },|_,stage| {
                    *unit_details.entry(stage).or_insert(0u128)+=last_unit.elapsed().as_nanos();last_unit=Instant::now();
                });
            } else {sim.step();}
            times.push(begin.elapsed().as_nanos());
            if let Some(copy) = replay.as_mut() {
                let copy: &mut BlastRange = copy;
                copy.step();
                assert_eq!(
                    sim.state_hash(),
                    copy.state_hash(),
                    "{name} replay tick {tick}"
                );
            } else if tick == 45 {
                let mut copy = BlastRange::restore(config, &sim.snapshot()).unwrap();
                copy.set_navigation_workers(2);
                copy.set_physics_workers(2);
                replay = Some(copy);
            }
            let unit = &sim.units[0];
            let motion = &unit.motion;
            if let (Some(w),Some(trace))=(reasons.as_mut(),motion.anchor_trace()) {
                reason_dropped+=trace.dropped as usize;
                window_rejected+=trace.window_rejected;window_limited+=trace.window_limited;window_unready+=trace.window_unready;
                for event in trace.events() {
                    if reason_rows>=256 {reason_dropped+=1;continue;}
                    let before=event.before.unwrap_or(event.proposed);
                    let after=event.after.unwrap_or(before);
                    let proposed=event.proposed.pos.sub(before.pos);let actual=after.pos.sub(before.pos);
                    writeln!(w,"{name},{tick},{},{:?},{},{},{},{},{},{},{},{}",event.leg,event.reason,
                        u8::from(event.planted_before),u8::from(event.planted_after),proposed.x.raw(),proposed.z.raw(),
                        actual.x.raw(),actual.z.raw(),angle::diff(event.proposed.head,before.head),angle::diff(after.head,before.head)).unwrap();
                    reason_rows+=1;
                }
            }
            if matches!(kind, RangeUnitKind::Commander | RangeUnitKind::Walker04) && motion.hull.grounded() {
                assert!(motion.feet.iter().any(|f| f.planted()), "{name} lost both supports on tick {tick}");
            }
            let root = motion.frame(&unit.body);
            if motion.rigs().is_empty() {
                writeln!(csv,"{name},{tick},0,{},{},{},{},{},0,0,0,1,0,{},{},0,0,0,Pending,Unchecked,0,0,0,0,0,0",
                    motion.phase,mm(root.base.x),mm(root.base.y),mm(root.base.z),motion.drive.head,
                    motion.hull.up.x.raw(),motion.hull.up.z.raw()).unwrap();
            }
            for (leg, (rig, foot)) in motion.rigs().iter().zip(&motion.feet).enumerate() {
                let Some(c) = foot.contact else { continue };
                let bottom = rig
                    .sole
                    .probes()
                    .iter()
                    .map(|p| p.y)
                    .min()
                    .unwrap_or(Fx::ZERO);
                let lift = mm(c.pos.y.add(bottom).sub(Fx::from_int(16)));
                highest_mm = highest_mm.max(lift);
                if !foot.planted() && !airborne[leg] {
                    lifts += 1;
                    if let Some(before) = previous[leg] {
                        first_lift_mm = first_lift_mm.max(lift - before);
                    }
                }
                airborne[leg] = !foot.planted();
                previous[leg] = Some(lift);
                let d=c.pos.sub(root.base);
                let target=phys::V3::new(d.dot(root.x),d.dot(root.y),d.dot(root.z));
                let contact_error=mm(rig.solve_contact(target).ankle.sub(target).len_sq().sqrt());
                writeln!(
                    csv,
                    "{name},{tick},{leg},{},{},{},{},{},{},{},{},{},{lift},{},{},{},1,{},{:?},{:?},{},{},{},{},{},{}",
                    motion.phase,
                    mm(root.base.x),
                    mm(root.base.y),
                    mm(root.base.z),
                    motion.drive.head,
                    mm(c.pos.x),
                    mm(c.pos.y),
                    mm(c.pos.z),
                    u8::from(foot.planted()),
                    motion.hull.up.x.raw(),
                    motion.hull.up.z.raw(),
                    contact_error,
                    u8::from(foot.ready()),foot.support.readiness,foot.support.reason,
                    foot.support.source.map_or(0,|s|s.kind),foot.support.source.map_or(0,|s|s.id),
                    foot.support.source.map_or(0,|s|s.content),foot.support.source.map_or(0,|s|s.pose),
                    foot.support.supported_mask,foot.support.samples
                )
                .unwrap();
            }
            if [15, 70, 85, 100, 150, 170, 225, 295].contains(&tick) {
                std::fs::write(format!("{prefix}-{name}-{tick}.bin"), sim.snapshot()).unwrap();
            }
        }
        // Tick 300 is not necessarily a stop: the massive walker is still
        // braking at ~2 m/s in the terrain case. Validate the actual stop,
        // rather than requiring both feet down at an arbitrary moving phase.
        // Keep trace/hash/timings tied to the original 300 recorded ticks.
        let recorded_hash = sim.state_hash();
        let recorded_head = angle::norm(sim.units[0].motion.drive.head);
        let mut settle_ticks = 0;
        while sim.units[0].motion.drive.vel != Fx::ZERO || sim.units[0].motion.drive.yaw_rate != 0
            || !sim.units[0].motion.feet.iter().all(|f| f.ready()) {
            assert!(settle_ticks < sim_core::TICKS_PER_SECOND * 6, "{name} did not finish braking/planting");
            sim.step();
            let copy = replay.as_mut().unwrap();
            copy.step();
            assert_eq!(sim.state_hash(), copy.state_hash(), "{name} replay during stop");
            if matches!(kind, RangeUnitKind::Commander | RangeUnitKind::Walker04) && sim.units[0].motion.hull.grounded() {
                assert!(sim.units[0].motion.feet.iter().any(|f| f.planted()), "{name} lost both supports while braking");
            }
            settle_ticks += 1;
        }
        times.sort_unstable();
        if reason_trace {println!("gait_reasons kind={name} first_tick=0 last_tick=260 rows={reason_rows} dropped={reason_dropped} limit=256 window_rejected={window_rejected} window_limited={window_limited} window_unready={window_unready}");}
        for (group,values) in [("phase",phases),("detail",details),("unit",unit_details)] {
            let mut v:Vec<_>=values.into_iter().collect();v.sort_by_key(|v|std::cmp::Reverse(v.1));
            for (stage,nanos) in v.into_iter().take(8) {println!("gait_profile {name} {group} {stage} mean_us={}",nanos/300/1000);}
        }
        println!("gait kind={name} ticks=300 lifts={lifts} first_lift_max_mm={first_lift_mm} highest_mm={highest_mm} mean_tick_us={} p50_tick_us={} p95_tick_us={} p99_tick_us={} max_tick_us={} head={} hash={:016x} settle_ticks={settle_ticks}",
            times.iter().sum::<u128>()/300/1000,times[150]/1000,times[285]/1000,times[297]/1000,times[299]/1000,
            recorded_head,recorded_hash);
    }
    csv.flush().unwrap();
}
