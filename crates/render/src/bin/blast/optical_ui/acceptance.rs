//! Opt-in real UI input and render-time material witness. Never advances SIM.
use super::*;
#[derive(Resource,Default)]
pub(crate) struct Sequence {pub enabled:bool,pub ready:bool,clock:f32,event:usize,shot:usize,wait:usize,menu:bool,
    split_shot:bool,sync_shot:bool,records:Vec<String>}
impl Sequence {pub fn new(enabled:bool)->Self {Self{enabled,..default()}}}
fn visible(node:&Node,parent:Option<&ChildOf>,ancestors:&Query<(&Node,Option<&ChildOf>)>)->bool {
    if node.display==Display::None{return false;}let mut parent=parent.map(ChildOf::parent);
    while let Some(e)=parent {let Ok((n,next))=ancestors.get(e)else{break};if n.display==Display::None{return false;}parent=next.map(ChildOf::parent);}true
}
pub(crate) fn drive(mut s:ResMut<Sequence>,time:Res<Time>,menu:Res<frontend::Menu>,mut projection:ResMut<spatial_ui::State>,
    mut buttons:Query<(Option<&construction::Action>,Option<&frontend::Action>,&Node,Option<&ChildOf>,&mut Interaction),With<Button>>,
    ancestors:Query<(&Node,Option<&ChildOf>)>) {
    if !s.enabled{return;}if !s.ready{projection.enabled=false;return;}
    s.clock+=time.delta_secs().min(0.1);projection.enabled=true;
    if s.clock<0.15{s.menu=menu.shown();}
    for (_,_,_,_,mut i) in &mut buttons {*i=Interaction::None;}
    let at=[0.45,0.95,1.45,1.95,2.45,2.95];
    if s.event<at.len() && s.clock>=at[s.event] {
        let category=[construction::Action::Tier(1),construction::Action::Category(1),construction::Action::Category(2),construction::Action::Tier(0),construction::Action::Category(0),construction::Action::Tier(1)][s.event];
        let page=[frontend::Action::Settings,frontend::Action::Back,frontend::Action::Help,frontend::Action::Back,frontend::Action::Settings,frontend::Action::Back][s.event];
        for (c,f,n,p,mut i) in &mut buttons {
            if visible(n,p,&ancestors) && if s.menu{f==Some(&page)}else{c==Some(&category)} {*i=Interaction::Pressed;break;}
        }
        s.event+=1;
    }
}
pub(crate) fn capture(mut s:ResMut<Sequence>,audit:Res<Audit>,scene:Res<Scene>,menu:Res<frontend::Menu>,mut commands:Commands,mut exit:MessageWriter<AppExit>) {
    if !s.enabled || !s.ready{return;}let Some(dir)=&scene.out else{return;};
    let sample=[(0.10,"00-open"),(0.32,"01-optical"),(0.50,"02-event"),(0.80,"03-quiet"),
        (1.0,"04-change"),(1.5,"05-change"),(2.0,"06-change"),(2.5,"07-change"),(3.0,"08-change"),(3.6,"09-idle")];
    let mut name=None;
    if let Some((at,label))=sample.get(s.shot) {if s.clock>=*at {name=Some((*label).to_string());s.shot+=1;}}
    let peak_visible=!s.menu || menu.signal_key()==frontend::Page::Home as u8;
    if peak_visible && name.is_none() && !s.sync_shot && audit.sync>0. {s.sync_shot=true;name=Some("peak-sync".into());}
    if peak_visible && name.is_none() && !s.split_shot && audit.split>0. {s.split_shot=true;name=Some("peak-split".into());}
    if let Some(name)=name {
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(format!("{dir}/{name}.png")));
        let clock=s.clock;s.records.push(format!("{name}\t{clock:.4}\t{}\t{}\t{:?}\t{:.5}\t{:.5}\t{:.5}\t{:.1}",audit.allocated,audit.drawn,audit.kinds,audit.flash,audit.split,audit.sync,audit.area));
    }
    if s.shot==sample.len() && s.clock>3.9 {
        s.wait+=1;if s.wait==35 {
            std::fs::write(format!("{dir}/material-sequence.tsv"),format!("stage\tclock\tallocated\tdrawn\tkinds\tflash\tsplit\tsync\tmax_area\n{}\n",s.records.join("\n"))).unwrap();
            println!("MATERIAL sequence complete: {} actual render-time samples",s.records.len());exit.write(AppExit::Success);
        }
    }
}
