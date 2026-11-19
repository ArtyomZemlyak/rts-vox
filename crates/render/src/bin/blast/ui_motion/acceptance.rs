//! Opt-in native input witness. Work mode advances the real SIM; navigation uses real mode handlers.
use super::*;
#[derive(Resource,Default)]pub(crate) struct Sequence {
    pub enabled:bool,pub ready:bool,nav:bool,dir:Option<String>,clock:f32,event:usize,shot:usize,wait:usize,
    records:Vec<String>,inputs:Vec<String>,completed:u64,quiet:bool,
}
impl Sequence {pub fn new(mode:Option<&str>)->Self {Self{enabled:mode.is_some(),nav:mode==Some("nav"),..default()}}
    pub fn ready(&mut self,dir:&str){self.ready=true;if self.dir.is_none(){self.dir=Some(dir.into());}}
}
fn visible(node:&Node,parent:Option<&ChildOf>,ancestors:&Query<(&Node,Option<&ChildOf>)>)->bool {
    if node.display==Display::None{return false;}let mut p=parent.map(ChildOf::parent);
    while let Some(e)=p {let Ok((n,next))=ancestors.get(e)else{break};if n.display==Display::None{return false;}p=next.map(ChildOf::parent);}true
}
pub(crate) fn drive(mut s:ResMut<Sequence>,time:Res<Time>,registry:Res<Registry>,mut scene:ResMut<Scene>,mut projection:ResMut<spatial_ui::State>,
    mut buttons:Query<(Option<&construction::Action>,Option<&frontend::Action>,Option<&laboratory::LabAction>,Option<&spatial_ui::Part>,Option<&construction::production_hud::Control>,&Node,Option<&ChildOf>,&mut Interaction),With<Button>>,
    ancestors:Query<(&Node,Option<&ChildOf>)>) {
    if !s.enabled{return;}if !s.ready{projection.enabled=false;return;}projection.enabled=true;
    // The output directory is retained above. Ordinary lab entry is intentionally
    // disabled in batch capture mode, so navigation now uses the interactive path.
    if s.nav {scene.out=None;}
    s.clock+=time.delta_secs().min(0.1);
    for (_,_,_,_,_,_,_,mut i) in &mut buttons{*i=Interaction::None;}
    if !s.nav {
        scene.paused=s.clock<2.8 || s.clock>5.8 && registry.completed>=2;
        if (1.4..1.95).contains(&s.clock) {
            for (c,_,_,_,_,n,p,mut i) in &mut buttons{if c==Some(&construction::Action::Choose(0))&&visible(n,p,&ancestors){*i=Interaction::Hovered;}}
        }
        if (5.95..6.5).contains(&s.clock) {
            for (_,_,_,part,_,n,p,mut i) in &mut buttons{if part==Some(&spatial_ui::Part::ToolsToggle)&&visible(n,p,&ancestors){*i=Interaction::Hovered;}}
        }
    }
    let nav=[(0.45,Some(frontend::Action::Start(frontend::Destination::Battle)),None),
        (1.05,Some(frontend::Action::Back),None),(1.65,Some(frontend::Action::Settings),None),
        (2.25,Some(frontend::Action::Back),None),(2.85,Some(frontend::Action::Start(frontend::Destination::Laboratory)),None),
        (3.65,None,Some(laboratory::LabAction::Tab(1))),(4.25,None,Some(laboratory::LabAction::Category(1))),
        (4.85,Some(frontend::Action::Menu),None),(5.45,Some(frontend::Action::Resume),None),(6.05,None,Some(laboratory::LabAction::Back))];
    let work=[(0.40,Some(construction::Action::Tier(1)),None),(0.85,Some(construction::Action::Tier(0)),None),
        (1.20,Some(construction::Action::Cancel),None),(2.0,None,Some(spatial_ui::Part::ToolsToggle)),(2.4,None,Some(spatial_ui::Part::ToolsToggle))];
    let at=if s.nav {nav.get(s.event).map(|e|e.0)}else{work.get(s.event).map(|e|e.0).or_else(||(s.event==5).then_some(2.6))};
    if let Some(at)=at {if s.clock>=at {
        let mut found=false;
        for (c,f,l,sp,hud,n,p,mut i) in &mut buttons {
            let matches=if s.nav {let (_,want_f,want_l)=nav[s.event];want_f.is_some()&&f.copied()==want_f || want_l.is_some()&&l.copied()==want_l}
                else if s.event==5 {matches!(hud,Some(construction::production_hud::Control::Toggle))}
                else{let (_,want_c,want_s)=work[s.event];want_c.is_some()&&c.copied()==want_c || want_s.is_some()&&sp.copied()==want_s};
            if matches&&visible(n,p,&ancestors){*i=Interaction::Pressed;found=true;break;}
        }
        if found {let line=format!("{}\t{:.4}\tok",s.event,s.clock);s.inputs.push(line);s.event+=1;}
        else if s.clock>at+1. {panic!("UI motion witness: visible control missing at event {} nav={}",s.event,s.nav);}
    }}
}
pub(crate) fn capture(mut s:ResMut<Sequence>,registry:Res<Registry>,settings:Res<Settings>,scene:Res<Scene>,menu:Res<frontend::Menu>,
    nodes:Query<(&Node,Option<&ChildOf>,Option<&Text>,Option<&ComputedNode>,Option<&UiGlobalTransform>,Has<IntensityLabel>)>,
    ancestors:Query<(&Node,Option<&ChildOf>)>,mut commands:Commands,mut exit:MessageWriter<AppExit>) {
    if !s.enabled||!s.ready{return;}let Some(dir)=s.dir.clone()else{return;};
    let nav=[0.12,0.52,0.80,1.12,1.72,2.0,2.32,2.95,3.3,3.72,4.32,4.92,5.52,6.12,6.55];
    let work=[0.12,0.47,0.92,1.27,1.85,1.98,2.07,2.47,2.72,3.1,5.95,6.3];
    let samples:&[f32]=if s.nav{&nav}else{&work};let mut name=None;
    if samples.get(s.shot).is_some_and(|at|s.clock>=*at){name=Some(format!("{:02}-phase",s.shot));s.shot+=1;}
    if registry.completed>s.completed {s.completed=registry.completed;name=Some(format!("complete-{}",s.completed));}
    if name.is_none() && s.clock>6.7 && scene.paused && registry.effects.is_empty() && !s.quiet {s.quiet=true;name=Some("quiet".into());}
    if let Some(name)=name {
        let mut info=Vec::new();
        for (n,p,t,c,g,intensity) in &nodes {if intensity||!visible(n,p,&ancestors){continue;}if let (Some(t),Some(c),Some(g))=(t,c,g) {if let Some(r)=rect(c,g){info.push(format!("{}|{:.1},{:.1},{:.1},{:.1}",t.0,r.at.x,r.at.y,r.size.x,r.size.y));}}}
        info.sort();let hash=info.join("\n").bytes().fold(0xcbf29ce484222325_u64,|h,b|(h^u64::from(b)).wrapping_mul(0x100000001b3));
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(format!("{dir}/{name}.png")));
        let row=format!("{name}\t{:.4}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{hash:016x}",s.clock,scene.sim.tick,menu.signal_key(),settings.percent,registry.tracks.len(),registry.effects.len(),registry.emitted,registry.advanced,registry.completed,scene.sim.spawned().len(),info.len());s.records.push(row);
    }
    if s.shot==samples.len()&&s.clock>7.0&&(s.nav || registry.completed>=2 || s.clock>25.) {
        s.wait+=1;if s.wait==35 {
            std::fs::write(format!("{dir}/motion-sequence.tsv"),format!("stage\tclock\ttick\tpage\tintensity\ttracked\teffects\temitted\tadvanced\tcompleted\tproduced\ttext_count\tinformation_hash\n{}\n",s.records.join("\n"))).unwrap();
            std::fs::write(format!("{dir}/motion-inputs.tsv"),format!("event\tclock\tresult\n{}\n",s.inputs.join("\n"))).unwrap();
            if s.nav {assert_eq!(s.event,10);}else {assert_eq!(s.event,6);assert!(registry.advanced>=1&&registry.completed>=2&&scene.sim.spawned().len()>=2,"real queue missing: tick={} advance={} complete={} spawned={} queue={:?} progress={:?}",scene.sim.tick,registry.advanced,registry.completed,scene.sim.spawned().len(),scene.sim.production_queue(0),scene.sim.production_progress(0));}
            assert!(registry.effects.is_empty(),"idle motion must settle");
            println!("MOTION sequence complete: {} stages, {} actual inputs, {} completions",s.records.len(),s.event,registry.completed);exit.write(AppExit::Success);
        }
    }
}
