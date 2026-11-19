//! Opt-in native motion witness. Drives the real controls and captures a paused world.
//! Its render clock is independent of SIM ticks; this is never FPS evidence.
use super::*;

#[derive(Resource,Default)]
pub(crate) struct Sequence {
    pub enabled: bool,
    pub ready: bool,
    clock: f32,
    event: usize,
    shot: usize,
    wait: usize,
    original: Option<usize>,
    records: Vec<String>,
}
impl Sequence {
    pub fn new(enabled:bool)->Self {Self{enabled,..default()}}
    pub fn capture(&mut self, dir:&str, state:&State, commands:&mut Commands, exit:&mut MessageWriter<AppExit>) {
        self.ready=true;
        let samples=[(0.06,"00-source"),(0.18,"01-forming"),(0.38,"02-open"),(0.85,"03-quiet"),
            (1.35,"04-tools"),(1.9,"05-tools-closed"),(2.18,"06-folding"),(2.45,"07-tab"),
            (2.72,"08-restoring"),(3.1,"09-restored"),(3.48,"10-switch-out"),(3.95,"11-switch-in"),
            (4.28,"12-exit"),(4.65,"13-gone")];
        if let Some((at,name))=samples.get(self.shot) {
            if self.clock>=*at {
                commands.spawn(Screenshot::primary_window()).observe(save_to_disk(format!("{dir}/{name}.png")));
                self.records.push(format!("{name}\t{:.4}\t{:.4}\t{:.4}\t{:?}\t{}\t{}\t{}\t{:?}\t{:?}",
                    self.clock,state.motion.aperture,state.motion.energy,state.motion.key,state.collapsed,state.tools_open,
                    state.visible,state.motion.last.map(|r|[r.source.x,r.source.y]),
                    state.motion.last.map(|r|[r.position.x,r.position.y,r.size.x,r.size.y])));
                self.shot+=1;
            }
        } else {
            self.wait+=1;
            if self.wait==35 {
                std::fs::write(format!("{dir}/projection-sequence.tsv"),format!("stage\tclock\taperture\tenergy\tactor\tfolded\ttools\treadable\tsource\tpanel\n{}\n",self.records.join("\n"))).unwrap();
                println!("PROJECTION sequence complete: {} real render-time stages",self.shot);
                exit.write(AppExit::Success);
            }
        }
    }
}
pub(crate) fn drive(mut sequence:ResMut<Sequence>,time:Res<Time>,mut state:ResMut<State>,
    mut edit:ResMut<Workshop>,scene:Res<Scene>,mut controls:Query<(&Part,&mut Interaction)>) {
    if !sequence.enabled {return;}
    if !sequence.ready {state.enabled=false;return;}
    if sequence.original.is_none() {sequence.original=edit.controlled;}
    sequence.clock+=time.delta_secs().min(0.1);
    state.enabled=true;
    for (part,mut i) in &mut controls {
        if matches!(part,Part::ToolsToggle|Part::FoldToggle|Part::Restore) {*i=Interaction::None;}
    }
    let events=[(1.0,Some(Part::ToolsToggle)),(1.65,Some(Part::ToolsToggle)),(2.1,Some(Part::FoldToggle)),
        (2.6,Some(Part::Restore)),(3.4,None),(4.2,None)];
    if let Some((at,part))=events.get(sequence.event) {
        if sequence.clock>=*at {
            if let Some(part)=part {
                for (p,mut i) in &mut controls {if p==part {*i=Interaction::Pressed;}}
            } else if sequence.event==4 {
                edit.controlled=scene.sim.units.iter().enumerate().find_map(|(id,u)|
                    (Some(id)!=sequence.original && scene.sim.allied(id) && u.status.alive).then_some(id));
                edit.selected_units=edit.controlled.into_iter().collect();
            } else {edit.controlled=None;edit.selected_units.clear();}
            sequence.event+=1;
        }
    }
}
