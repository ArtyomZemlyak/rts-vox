//! Repeated excavation and overlapping real battle blasts; no render or pacing.
use super::*;

struct DeepSoil;
impl Generator for DeepSoil {
    fn material(&self,_:u32,y:u32,_:u32)->u16 {u16::from(y<80)}
    fn batches_columns(&self)->bool {true}
    fn column(&self,_:u32,_:u32,y:u32,out:&mut[u16]) {
        for (dy,m) in out.iter_mut().enumerate() {*m=u16::from(y+(dy as u32)<80);}
    }
    fn material_candidates(&self,_:[u32;3],_:[u32;3])->Option<Vec<u16>> {Some(vec![0,1])}
    fn uniform_box(&self,lo:[u32;3],hi:[u32;3])->Option<u16> {
        if hi[1]<=80 {Some(1)} else if lo[1]>=80 {Some(0)} else {None}
    }
    fn soil_height(&self,_:u32,_:u32)->Option<u32> {Some(80)}
}
fn top(w:&World,x:u32,z:u32)->u32 {w.column_ceiling(x,z)}
fn profile(w:&World)->Vec<u16> {
    (48..112).flat_map(|z|(0..80).flat_map(move|y|(48..112).map(move|x|w.material(x,y,z)))).collect()
}
fn dig(w:&mut World,dents:&mut SurfaceWork,c:&ExplosionConfig,at:[i32;3],nuclear:bool,filtered:bool,slice:usize)->usize {
    let mut e=Explosion::with_damage(w,at,c,&[],dents);e.nuclear_ground=nuclear;
    let matter=matter::Matter::new(&[]);let mut changed=Vec::new();let mut removed=0;
    for _ in 0..512 {
        if !e.working() && w.queued()==0 {return removed;}
        e.advance_frontier(slice,w,&[]);
        if filtered {e.read_contacts_with(w,slice,&[],Some((&matter,c,u64::MAX)));}
        e.step_with_room::<true>(w,slice,&mut[],dents,&mut changed,c,None,None);
        w.flush();removed+=w.completed().iter().filter(|(e,old)|e.mat==0 && *old!=0).count();
    }
    panic!("repeated crater failed to drain at {at:?}");
}

#[test]
fn repeated_surface_hits_deepen_the_live_floor_and_invalidate_cached_navigation() {
    let c=ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut w=World::new(160,128,160,65536,Box::new(DeepSoil));
    let mut dents=SurfaceWork::new();let mut floors=vec![top(&w,80,80)];
    let mut p=nav::vehicle::Probe::new(&w);assert_eq!(p.floor(80,80),80);
    let mut columns=p.take_columns();drop(p);
    for shot in 0..4 {
        let y=top(&w,80,80) as i32-1;assert!(y>0);
        let before=profile(&w);
        let removed=dig(&mut w,&mut dents,&c,[80,y,80],false,true,4096);
        assert!(removed>0,"shot {shot} did not excavate");
        let after=profile(&w);
        assert!(before.iter().zip(&after).all(|(&a,&b)|a!=0 || b==0),"blast refilled previous voids");
        floors.push(top(&w,80,80));
        assert!(floors[floors.len()-1]<floors[floors.len()-2],"new impact ignored the deeper floor: {floors:?}");
        let mut dirty=nav::vehicle::Changed::default();dirty.column(80,80);
        let mut cached=nav::vehicle::Probe::for_local(&w,&[],columns,&dirty);
        let mut fresh=nav::vehicle::Probe::new(&w);
        assert_eq!(cached.floor(80,80),fresh.floor(80,80),"stale crater surface");
        assert_eq!(cached.floor(80,80),floors[floors.len()-1] as i32);
        columns=cached.take_columns();
    }
    println!("MULTI_CRATER repeated shell floor={floors:?}");
}

#[test]
fn overlapping_strategic_terraces_keep_scalar_and_filtered_geometry_identical() {
    // Small pressure grid exercises the shipped terrace/contact algorithm,
    // including holes left by previous strikes, without a full-map warhead run.
    let mut c=ExplosionConfig::from_ron(include_str!("../../../data/nuclear-warhead.ron")).unwrap();
    c.energy=600;c.cell_units=4;c.half_cells=4;
    let mut outputs=Vec::new();
    for filtered in [false,true] {
        let mut w=World::new(160,128,160,65536,Box::new(DeepSoil));
        let mut dents=SurfaceWork::new();let mut count=0;
        for [x,z] in [[80,80],[88,80],[80,88],[80,80]] {
            let y=top(&w,x,z) as i32-1;
            let before=profile(&w);
            count+=dig(&mut w,&mut dents,&c,[x as i32,y,z as i32],true,filtered,8192);
            let after=profile(&w);
            assert!(before.iter().zip(&after).all(|(&a,&b)|a!=0 || b==0));
        }
        assert!(count>0);assert!(top(&w,80,80)<70);
        assert!(dents.is_empty(),"strategic soil leaked per-voxel dents");
        outputs.push((profile(&w),count));
    }
    assert_eq!(outputs[0],outputs[1],"overlapping terraces depend on the contact reader");
}

fn arena(edits:usize)->BlastRange {
    let mut l=RangeLayout::from_ron(include_str!("../../../data/range-cover.ron")).unwrap();
    l.size=[640,192,640];l.ground_level=64;l.clear_min=[0,0];l.clear_max=[640,640];
    l.blocks.clear();l.shields.clear();l.mounds.clear();l.roads.clear();l.course=None;l.economy=None;l.ready=None;
    l.gun=[48,80,48];l.target=[400,63,400];
    l.battle=true;l.endless_battle=true;
    l.units=vec![
        modding::range::RangeUnitDef {kind:RangeUnitKind::Artillery,at:[48,64,48],allied:true,weapon:None},
        modding::range::RangeUnitDef {kind:RangeUnitKind::Tank,at:[48,64,592],allied:false,weapon:None},
    ];
    let mut c=ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    c.edits_per_tick=edits;
    let mut r=BlastRange::with_layout(c,l).unwrap();r.set_physics_workers(1);r.set_navigation_workers(1);
    r.start_battle().unwrap();
    for a in &mut r.battle.actors {a.order=battle::Order::Hold;a.next=u32::MAX;}
    r
}
fn land(r:&mut BlastRange,weapon:battle::Weapon,x:i32,z:i32) {
    let y=top(&r.world,x as u32,z as u32) as i32+2;
    let pos=[x,y,z].map(Fx::from_int);
    r.battle.rounds.push(battle::Round {owner:0,weapon,pos,previous:pos,
        velocity:[Fx::ZERO,Fx::from_int(-240),Fx::ZERO],distance:Fx::from_int(1000)});
}

#[test]
fn simultaneous_overlapping_blasts_drain_and_replay_with_finite_fragment_capacity() {
    for edits in [65536,1024] {overlap_trial(edits);}
}
fn overlap_trial(edits:usize) {
    use battle::Weapon;
    let mut r=arena(edits);
    // Keep the stock hull reserve and leave only sixteen flying slots for
    // terrain. A pool below the reserve would pulverise every cell and never
    // exercise real fragment confirmation or its snapshot.
    r.matter.tuning.max_flying=r.battle.tuning.hull_piece_reserve+16;
    for weapon in [Weapon::Blast,Weapon::Energy,Weapon::Siege,Weapon::Blast] {
        land(&mut r,weapon,400,400);
    }
    let mut replay=None;let mut overlap=false;let mut copied_live=false;let mut drained=false;let mut pieces=false;
    let before=r.removed;let mut finished=0;
    for tick in 0..220 {
        r.step();
        if let Some(copy)=replay.as_mut() {
            let copy:&mut BlastRange=copy;copy.step();assert_eq!(r.state_hash(),copy.state_hash(),"overlap replay tick={tick}");
        }
        let [capturing,active]=r.battle.wave_counts();overlap|=capturing+active>1;
        pieces|=!r.matter.debris.fragments.is_empty();
        if [1,8,40,100,219].contains(&tick) {
            println!("MULTI_CRATER progress tick={tick} removed={} edits={} claims={} waves={:?}",r.removed,r.world.queued(),r.matter.debris.claims(),
                r.battle.wave_progress().map(|w|(w.weapon,w.capturing,w.pending,w.queued_continuations,w.contact_cursor,w.energy_mj)).collect::<Vec<_>>());
        }
        if replay.is_none() && capturing+active>1 {
            let mut copy=BlastRange::restore(r.config,&r.snapshot()).unwrap();
            copy.set_physics_workers(2);copy.set_navigation_workers(2);replay=Some(copy);copied_live=true;
        }
        assert!(r.matter.economy.ledger.balanced(),"mass at tick={tick}");
        assert!(r.matter.spent_mj<=r.matter.supplied_mj);
        if tick>0 && capturing+active==0 && r.world.queued()==0 && r.matter.debris.claims()==0 {drained=true;finished=tick+1;break;}
    }
    assert!(overlap && copied_live,"no overlapping resumable waves exercised");
    assert!(pieces,"the overlap fixture never created actual ground fragments");
    assert!(drained,"overlapping craters did not drain: edits={} claims={} waves={:?}",r.world.queued(),r.matter.debris.claims(),r.battle.wave_counts());
    assert!(r.removed>before,"no terrain destruction");
    assert!(top(&r.world,400,400)<64,"no actual crater");
    assert!(r.units.iter().all(|u|u.removed.is_empty() && u.seg_removed.is_empty()),"crater fixture hit remote hulls");
    if edits==65536 {assert!(finished<=16,"stock edit pacing delayed a small salvo: {finished}");}
    println!("MULTI_CRATER overlap edits={edits} ticks={finished} removed={} floor={}",r.removed-before,top(&r.world,400,400));
}

#[test]
fn repeated_buried_charges_while_earth_is_airborne_preserve_replay_and_drain() {
    use battle::commands::{Command,GunCommand};
    let mut l=modding::laboratory::Definition::stock().layout;
    l.size=[192,128,192];l.ground_level=48;l.clear_min=[0,0];l.clear_max=[192,192];
    l.blocks.clear();l.units.clear();l.gun_point=true;l.gun=[40,70,40];l.target=[96,40,96];
    let c=ExplosionConfig::from_ron(include_str!("../../../data/explosion.ron")).unwrap();
    let mut r=BlastRange::with_layout(c,l).unwrap();r.set_physics_workers(1);r.control();
    let mut copy:Option<BlastRange>=None;let mut overlapping=false;let mut fired=0;
    for tick in 0..260 {
        if fired<2 && r.ready() {
            overlapping|=fired>0 && !r.bodies.list.is_empty();
            let at=[96,top(&r.world,96,96).saturating_sub(8).max(1) as i32,96];
            assert!(r.execute_command(Command::Gun(GunCommand::Detonate {at})).admitted());
            if let Some(copy)=&mut copy {assert!(copy.execute_command(Command::Gun(GunCommand::Detonate {at})).admitted());}
            fired+=1;
        }
        r.step();
        if let Some(copy)=&mut copy {copy.step();assert_eq!(r.state_hash(),copy.state_hash(),"buried overlap tick={tick}");}
        else if !r.bodies.list.is_empty() {
            let mut restored=BlastRange::restore(c,&r.snapshot()).unwrap();restored.set_physics_workers(2);copy=Some(restored);
        }
        assert!(r.matter.economy.ledger.balanced());assert!(r.matter.spent_mj<=r.matter.supplied_mj);
        if fired==2 && tick>80 && r.ready() && r.bodies.list.is_empty() && r.world.queued()==0 && r.matter.debris.claims()==0 {break;}
    }
    assert_eq!(fired,2);assert!(overlapping,"no second explosion during the heave");
    assert!(copy.is_some() && r.upheaval.raised>0);
    assert!(r.ready() && r.bodies.list.is_empty() && r.world.queued()==0 && r.matter.debris.claims()==0,"buried overlap never settled");
}
