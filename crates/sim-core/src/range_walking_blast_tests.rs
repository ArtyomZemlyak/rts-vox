//! End-to-end walking inside the changing blast/debris neighbourhood.
//! Uses the ordinary battle rounds, world edits, collapse, navigation and feet.
use super::*;
use phys::V3;

fn arena(kind:RangeUnitKind,section:bool)->BlastRange {
    let mut l=RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    l.size=[1536,768,1536];l.ground_level=64;l.clear_min=[0,0];l.clear_max=[1536,1536];
    l.blocks=if section {vec![modding::range::RangeBlock{min:[419,64,511],max:[422,80,514],material:2},
        modding::range::RangeBlock{min:[408,80,496],max:[432,88,528],material:3}]}else{vec![]};
    l.shields.clear();l.mounds.clear();l.roads.clear();l.course=None;l.economy=None;l.ready=None;
    l.gun=[48,100,48];l.target=[420,63,512];l.battle=true;l.endless_battle=true;
    l.units=vec![modding::range::RangeUnitDef{kind,at:[320,64,512],allied:true,weapon:None}];
    let c=ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r=BlastRange::with_layout(c,l).unwrap();
    r.set_physics_workers(1);r.set_navigation_workers(1);r.start_free_control().unwrap();
    r
}
fn salvo(r:&mut BlastRange,at:[i32;2]) {
    for [dx,dz] in [[0,-8],[0,8],[8,0]] {
        let [x,z]=[at[0]+dx,at[1]+dz];
        let y=r.world.column_ceiling(x as u32,z as u32).min(64) as i32+2;
        let pos=[x,y,z].map(Fx::from_int);
        r.battle.rounds.push(battle::Round{owner:0,weapon:Weapon::Blast,pos,previous:pos,
            velocity:[Fx::ZERO,Fx::from_int(-240),Fx::ZERO],distance:Fx::from_int(1000)});
    }
}
fn checkpoint(r:&BlastRange,name:&str)->BlastRange {
    let bytes=r.snapshot();
    if let Some(dir)=std::env::var_os("RTS_WALKING_BLAST_CAPTURE_DIR") {
        let dir=std::path::PathBuf::from(dir);std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{:?}-{name}.bin",r.units[0].kind)),&bytes).unwrap();
    }
    let mut copy=BlastRange::restore(r.config,&bytes).expect("walking blast snapshot");
    copy.set_physics_workers(2);copy.set_navigation_workers(2);
    assert_eq!(r.state_hash(),copy.state_hash(),"checkpoint {name}");copy
}
fn trial(kind:RangeUnitKind,section_case:bool,flank_order:bool) {
    let mut r=arena(kind,section_case);assert!(r.battle_order(0,Order::Move,Some([720,512])));
    for tick in 0..160 {
        r.step();
        if tick>=15 && r.units[0].motion.pos[0]>Fx::from_int(320)
            && r.units[0].motion.feet.iter().any(|f|!f.planted() && !f.departing()
                && f.contact.zip(f.placement()).is_some_and(|(c,g)|c.pos.y.sub(g.pos.y).abs()<=Fx::ONE)) {break;}
    }
    let start=r.units[0].motion.pos;
    assert!(start[0]>Fx::from_int(320),"walker did not start before the blasts");
    let mut copy=checkpoint(&r,"before");
    r.units[0].motion.trace_anchors(true);
    salvo(&mut r,[420,512]);salvo(&mut copy,[420,512]);
    let mut live_saved=false;let mut swing_saved=false;let mut landed_saved=false;
    let mut order_live=false;let mut overlap_ticks=0;let mut moving_live=0;let mut live_landings=0;
    let mut landings=0;let mut crater_landings=0;let mut small=false;let mut large=false;let mut section=false;let mut airborne=false;
    let mut detour=Fx::ZERO;let mut local_distance=Fx::MAX;let mut first_landing=None;
    let mut progress_after_landing=Fx::ZERO;let mut max_wait=0;let mut wait=0;
    let before_removed=r.removed;let mut destroyed_overlap=0;let mut destroyed_landings=0;let mut small_max=0;let mut piece_max=0;
    let mut rows=String::from("tick,waves,x_q32,y_q32,z_q32,ready,landings,small,large,queued\n");
    for tick in 0..900 {
        // Further genuine impacts arrive while the first salvo still works;
        // no unsaved battle-config overrides or artificial fragment injection.
        if [2,4].contains(&tick) {salvo(&mut r,[420,512]);salvo(&mut copy,[420,512]);}
        let old_pos=r.units[0].motion.pos;
        let previous=r.units[0].motion.feet.clone();
        r.units[0].motion.clear_anchor_trace();
        let active_before=r.battle.wave_counts();
        r.step();copy.step();
        assert_eq!(r.state_hash(),copy.state_hash(),"{kind:?} workers/restore tick {tick}");
        assert_eq!(r.units[0].motion.feet,copy.units[0].motion.feet);
        assert!(r.matter.economy.ledger.balanced());
        assert!(r.matter.spent_mj<=r.matter.supplied_mj);
        let waves=r.battle.wave_counts();let overlapping=waves[0]+waves[1]>1;
        let live=active_before[0]+active_before[1]>0 || waves[0]+waves[1]>0;
        let u=&r.units[0];let m=&u.motion;
        assert!(u.status.alive,"{kind:?} died instead of walking at {tick}");
        let moved=m.pos[0]!=old_pos[0] || m.pos[2]!=old_pos[2];
        overlap_ticks+=usize::from(overlapping);moving_live+=usize::from(overlapping && moved);
        destroyed_overlap+=usize::from(overlapping && moved && r.removed>before_removed);
        if m.destination.is_some() && !moved {wait+=1;max_wait=max_wait.max(wait);}else{wait=0;}
        detour=detour.max(m.pos[2].sub(Fx::from_int(512)).abs());
        if live {local_distance=local_distance.min(m.pos[0].sub(Fx::from_int(420)).abs().add(m.pos[2].sub(Fx::from_int(512)).abs()));}
        let root=m.frame(&u.body);
        for (leg,(foot,rig)) in m.feet.iter().zip(m.rigs()).enumerate() {
            if foot.ready() {
                let c=foot.contact.unwrap();let d=c.pos.sub(root.base);
                let target=V3::new(d.dot(root.x),d.dot(root.y),d.dot(root.z));
                let error=rig.solve_contact(target).ankle.sub(target).len_sq().sqrt();
                assert!(error<=Fx::from_ratio(1,20),"{kind:?} ready IK {error:?} tick {tick} leg {leg}");
                if previous[leg].ready() {assert_eq!(foot.contact,previous[leg].contact,"planted slide {kind:?}/{tick}/{leg}");}
                if !previous[leg].planted() {
                    landings+=1;live_landings+=usize::from(live);
                    destroyed_landings+=usize::from(live && r.removed>before_removed);
                    crater_landings+=usize::from(c.pos.y<Fx::from_int(64));
                    if first_landing.is_none() {first_landing=Some(m.pos[0]);}
                }
            }
        }
        if let Some(x)=first_landing {progress_after_landing=progress_after_landing.max(m.pos[0].sub(x));}
        small|=r.matter.debris.fragments.iter().any(|f|f.chunk.is_none() && f.count<=8);
        small_max=small_max.max(r.matter.debris.fragments.iter().filter(|f|f.chunk.is_none() && f.count<=8).count());
        piece_max=piece_max.max(r.matter.debris.fragments.len());
        large|=r.matter.debris.fragments.iter().any(|f|f.chunk.is_some() || f.count>=32)
            || r.bodies.list.iter().any(|b|b.voxels.len()>64);
        section|=r.matter.debris.fragments.iter().any(|f|f.chunk.is_some()) || r.bodies.list.iter().any(|b|b.voxels.len()>64);
        airborne|=r.matter.debris.fragments.iter().any(|f|!f.resting) || !r.bodies.list.is_empty();
        rows.push_str(&format!("{tick},{},{},{},{},{},{},{},{},{}\n",waves[0]+waves[1],m.pos[0].raw(),m.pos[1].raw(),m.pos[2].raw(),m.feet.iter().filter(|f|f.ready()).count(),landings,u8::from(small),u8::from(large),r.world.queued()));
        let swinging=m.feet.iter().any(|f|!f.planted());
        if overlapping && !order_live {
            let destination=[740,if flank_order {600}else{512}];
            assert!(r.battle_order(0,Order::Move,Some(destination)));
            assert!(copy.battle_order(0,Order::Move,Some(destination)));order_live=true;
        }
        if overlapping && !live_saved {copy=checkpoint(&r,"live-waves");live_saved=true;}
        if live && swinging && !swing_saved {copy=checkpoint(&r,"live-swing");swing_saved=true;}
        if landings>0 && !landed_saved {copy=checkpoint(&r,"landed");landed_saved=true;}
        if tick%100==0 || (250..260).contains(&tick) {println!("WALK_BLAST {kind:?} tick={tick} waves={waves:?} pos={:?} goal={:?} landings={landings} live_landings={live_landings} pieces={} detour={detour:?} window={:?} route={:?}/{:?}/{:?} motor={:?} head={} vel={:?} terrain={:?} feet={:?}",r.units[0].motion.pos,r.units[0].motion.destination,r.matter.debris.fragments.len(),r.units[0].motion.anchor_trace().and_then(|t|t.window_reason),r.units[0].motion.route_wait_evidence(),r.units[0].motion.fine_work(),r.units[0].motion.path,r.units[0].motion.anchor_trace().and_then(|t|t.motor_command),r.units[0].motion.drive.head,r.units[0].motion.drive.vel,r.units[0].motion.anchor_trace().and_then(|t|t.motor_terrain),r.units[0].motion.feet.iter().map(|f|(f.planted(),f.support.reason,f.placement().map(|c|c.pos))).collect::<Vec<_>>());}
        if tick>100 && r.units[0].motion.destination.is_none() && waves==[0,0] {break;}
    }
    if let Some(dir)=std::env::var_os("RTS_WALKING_BLAST_CAPTURE_DIR") {
        std::fs::write(std::path::PathBuf::from(dir).join(format!("{kind:?}.csv")),rows).unwrap();
    }
    let _=checkpoint(&r,"final");
    let crater_floor=(392..448).flat_map(|x|(484..540).map(move|z|(x,z))).map(|(x,z)|r.world.column_ceiling(x,z)).min().unwrap();
    println!("WALK_BLAST_GEOMETRY {kind:?} floor={crater_floor} crater_landings={crater_landings} section={section}");
    let checks=[("overlap",overlap_ticks>0),("walking_overlap",moving_live>0),("walking_during_destruction",destroyed_overlap>0),
        ("order_live",order_live),("live_restore",live_saved),("swing_restore",swing_saved),("landing_restore",landed_saved),
        ("small_debris",small),("large_debris",large),("detached_section",!section_case || section),("airborne_debris",airborne),
        ("crater",crater_floor<64),("landing_during_destruction",destroyed_landings>0),
        ("continuation",progress_after_landing>Fx::from_int(40)),("near_blast",local_distance<Fx::from_int(120)),
        ("arrived",r.units[0].motion.destination.is_none()),("detour",detour>Fx::from_int(8)),("bounded_wait",max_wait<160)];
    if let Some(dir)=std::env::var_os("RTS_WALKING_BLAST_CAPTURE_DIR") {
        let report=format!("kind,overlap_ticks,moving_overlap,destroyed_overlap,landings,live_landings,destroyed_landings,crater_landings,nearest_q32,detour_q32,progress_after_landing_q32,max_wait,small_max,piece_max,crater_floor,section\n{kind:?},{overlap_ticks},{moving_live},{destroyed_overlap},{landings},{live_landings},{destroyed_landings},{crater_landings},{},{},{},{max_wait},{small_max},{piece_max},{crater_floor},{}\n",local_distance.raw(),detour.raw(),progress_after_landing.raw(),u8::from(section));
        let dir=std::path::PathBuf::from(dir);std::fs::write(dir.join(format!("{kind:?}-summary.csv")),report).unwrap();
        std::fs::write(dir.join(format!("{kind:?}-checks.csv")),format!("check,passed\n{}",checks.iter().map(|(k,v)|format!("{k},{}\n",u8::from(*v))).collect::<String>())).unwrap();
    }
    assert_eq!(r.snapshot(),copy.snapshot());
    let failed=checks.iter().filter(|(_,v)|!*v).map(|(k,_)|*k).collect::<Vec<_>>();
    assert!(failed.is_empty(),"{kind:?} failed {failed:?}: pos={:?}",r.units[0].motion.pos);
    println!("WALK_BLAST_ACCEPT {kind:?} overlap={overlap_ticks} moving_live={moving_live} live_landings={live_landings} landings={landings} nearest={local_distance:?} detour={detour:?} max_wait={max_wait}");
}
#[test] fn walking_blast_strider_overlap_orders_landings_and_restore(){trial(RangeUnitKind::Strider,true,true);}
#[test] fn walking_blast_spider_overlap_orders_landings_and_restore(){trial(RangeUnitKind::Spider,false,true);}
#[test] fn walking_blast_commander_overlap_orders_landings_and_restore(){trial(RangeUnitKind::Commander,false,true);}
#[test] fn walking_blast_walker04_overlap_orders_landings_and_restore(){trial(RangeUnitKind::Walker04,false,true);}

#[test]
fn walking_blast_actual_incoming_sole_revalidates_lands_and_continues() {
    let mut r=arena(RangeUnitKind::Strider,false);
    assert!(r.battle_order(0,Order::Move,Some([720,512])));
    for _ in 0..16 {r.step();}
    let (leg,target)=r.units[0].motion.feet.iter().enumerate()
        .filter_map(|(i,f)|f.placement().map(|c|(i,c)))
        .max_by_key(|(_,c)|c.pos.x).expect("real incoming placement");
    let prior=r.units[0].motion.feet[leg].support.signature;
    let [x,z]=[target.pos.x.to_int_trunc(),target.pos.z.to_int_trunc()];
    let mut copy=checkpoint(&r,"underfoot-before");
    // Three real rounds excavate the actual saved landing target. A shallow
    // buried impact reduces direct air exposure; damage is still enabled.
    for scene in [&mut r,&mut copy] {
        for [dx,dz] in [[-2,0],[2,0],[0,2]] {
            let pos=[x+dx,61,z+dz].map(Fx::from_int);
            scene.battle.rounds.push(battle::Round{owner:0,weapon:Weapon::Blast,pos,previous:pos,
                velocity:[Fx::ZERO,Fx::from_int(-240),Fx::ZERO],distance:Fx::from_int(1000)});
        }
    }
    r.units[0].motion.trace_anchors(true);
    let mut overlap=false;let mut changed_landing=false;let mut saved_live=false;let mut saved_landing=false;
    let mut continued=Fx::ZERO;let mut landed_at=None;let mut landings=0;let mut live_landings=0;
    let mut max_ready_ik=Fx::ZERO;let mut max_planted_slide=Fx::ZERO;
    let mut peak_ready_ik=None;
    let mut max_supported_dy=Fx::ZERO;let mut peak_dy_tick=0;let mut peak_dy_waves=0;
    for tick in 0..600 {
        let previous=r.units[0].motion.feet.clone();
        let previous_y=r.units[0].motion.pos[1];
        r.units[0].motion.clear_anchor_trace();
        r.step();copy.step();assert_eq!(r.state_hash(),copy.state_hash(),"underfoot workers tick {tick}");
        let waves=r.battle.wave_counts();overlap|=waves[0]+waves[1]>1;
        assert!(r.matter.economy.ledger.balanced());assert!(r.matter.spent_mj<=r.matter.supplied_mj);
        assert!(r.units[0].status.alive,"underfoot hit killed the fixture");
        if [80,160,320].contains(&tick) {
            let u=&r.units[0];let m=&u.motion;let root=m.frame(&u.body);
            println!("UNDERFOOT_WINDOW tick={tick} pos={:?} velocity={:?} head={} trace={:?}",m.pos,m.drive.vel,m.drive.head,
                m.anchor_trace().map(|t|(t.window_reason,t.motor_command,t.motor_terrain)));
            println!("UNDERFOOT_REACH tick={tick} feet={:?}",m.rigs().iter().zip(&m.feet).map(|(rig,f)|
                f.contact.map(|c| {let d=c.pos.sub(root.base);(f.ready(),c.pos,
                    rig.contact_reachable(V3::new(d.dot(root.x),d.dot(root.y),d.dot(root.z))))})).collect::<Vec<_>>());
        }
        let m=&r.units[0].motion;
        let root=m.frame(&r.units[0].body);
        if previous.iter().filter(|f|f.ready()).count()>=2 && m.feet.iter().filter(|f|f.ready()).count()>=2 {
            let dy=m.pos[1].sub(previous_y).abs();
            if dy>max_supported_dy {max_supported_dy=dy;peak_dy_tick=tick;peak_dy_waves=waves[0]+waves[1];}
        }
        for (index,(rig,(old,f))) in m.rigs().iter().zip(previous.iter().zip(&m.feet)).enumerate() {
            if f.ready() {if let Some(c)=f.contact {
                let d=c.pos.sub(root.base);let target=V3::new(d.dot(root.x),d.dot(root.y),d.dot(root.z));
                let error=rig.solve_contact(target).ankle.sub(target).len_sq();
                if error>max_ready_ik {
                    max_ready_ik=error;
                    peak_ready_ik=Some((tick,index,root,c,f.clone(),rig.clone()));
                }
            }}
            if old.planted() && f.planted() {if let Some((a,b))=old.contact.zip(f.contact) {
                let dx=a.pos.x.sub(b.pos.x);let dz=a.pos.z.sub(b.pos.z);
                max_planted_slide=max_planted_slide.max(dx.mul(dx).add(dz.mul(dz)));
            }}
        }
        for (i,f) in m.feet.iter().enumerate() {
            if m.legs_lost[i] || m.legs_buckled[i] {continue;}
            if f.ready() && !previous[i].planted() {
                landings+=1;live_landings+=usize::from(waves[0]+waves[1]>0);
                if i==leg && (f.support.signature!=prior || f.support.supported_mask.count_ones()<u32::from(f.support.samples)) {
                    changed_landing=true;landed_at.get_or_insert(m.pos[0]);
                }
            }
        }
        if let Some(at)=landed_at {continued=continued.max(m.pos[0].sub(at));}
        if !saved_live && waves[0]+waves[1]>1 {copy=checkpoint(&r,"underfoot-live");saved_live=true;}
        if changed_landing && !saved_landing {copy=checkpoint(&r,"underfoot-landed");saved_landing=true;}
        if tick>80 && r.units[0].motion.destination.is_none() && waves==[0,0] {break;}
    }
    let _=checkpoint(&r,"underfoot-final");
    let floor=r.world.column_ceiling(x as u32,z as u32);
    let arrived=r.units[0].motion.destination.is_none();
    let u=&r.units[0];let lost=u.motion.legs_lost.iter().filter(|v|**v).count();let buckled=u.motion.legs_buckled.iter().filter(|v|**v).count();
    let report=format!("leg,target_x,target_z,floor,overlap,changed_landing,live_saved,landing_saved,landings,live_landings,continued_q32,arrived,engine,lost,buckled,root_x_q32,root_y_q32,root_z_q32\n{leg},{x},{z},{floor},{},{},{},{},{landings},{live_landings},{},{},{},{lost},{buckled},{},{},{}\n",u8::from(overlap),u8::from(changed_landing),u8::from(saved_live),u8::from(saved_landing),continued.raw(),u8::from(arrived),u8::from(u.status.engine),u.motion.pos[0].raw(),u.motion.pos[1].raw(),u.motion.pos[2].raw());
    if let Some(dir)=std::env::var_os("RTS_WALKING_BLAST_CAPTURE_DIR") {std::fs::write(std::path::PathBuf::from(dir).join("underfoot-summary.csv"),&report).unwrap();}
    println!("WALK_BLAST_UNDERFOOT {report}");
    println!("UNDERFOOT_SUPPORT {:?}",u.motion.feet.iter().map(|f|(f.ready(),f.support.reason,f.placement().map(|c|c.pos))).collect::<Vec<_>>());
    println!("UNDERFOOT_GEOMETRY max_ready_ik_sq={:?} planted_slide_sq={:?} max_supported_dy={:?}",max_ready_ik,max_planted_slide,max_supported_dy);
    if max_ready_ik>Fx::from_ratio(1,20).mul(Fx::from_ratio(1,20)) {
        println!("UNDERFOOT_READY_PEAK {peak_ready_ik:?}");
    }
    if let Some(dir)=std::env::var_os("RTS_WALKING_BLAST_CAPTURE_DIR") {
        let row=format!("ready_ik_squared_q32,planted_slide_squared_q32,max_supported_dy_q32,peak_dy_tick,peak_dy_waves\n{},{},{},{},{}\n",max_ready_ik.raw(),max_planted_slide.raw(),max_supported_dy.raw(),peak_dy_tick,peak_dy_waves);
        std::fs::write(std::path::PathBuf::from(dir).join("underfoot-geometry.csv"),row).unwrap();
    }
    let ik=Fx::from_ratio(1,20);
    assert!(max_ready_ik<=ik.mul(ik),"Ready sole is not reached by its real leg");
    assert_eq!(max_planted_slide,Fx::ZERO,"planted sole slid horizontally");
    assert_eq!(r.snapshot(),copy.snapshot());
    assert!(overlap && saved_live && floor<64,"landing target was not excavated by overlapping real waves");
    assert!(changed_landing && saved_landing && continued>Fx::from_int(40),"no revalidated actual landing and continuation");
    assert!(arrived,"underfoot walk did not finish");
}
