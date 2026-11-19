use super::*;
pub(super) type Nodes<'w,'s>=Query<'w,'s,(Entity,&'static Node,&'static ComputedNode,&'static UiGlobalTransform,
    Option<&'static ChildOf>,Has<laboratory::Root>,Option<&'static Text>,Option<&'static TextFont>,
    Has<panel::hud::BriefingCard>,Has<panel::PanelRoot>,Has<panel::minimap::MiniRoot>,Has<construction::Root>,
    Has<construction::production_hud::Root>,Option<&'static spatial_ui::Part>,(Option<&'static panel::Action>,Option<&'static laboratory::Dock>,Has<frontend::ReadableInk>,Option<&'static frontend::Action>))>;
pub(super) const HEADER:&str="clock\ttick\tselected\tpage\tui_scale\tqueue_orders\tqueue_rect\tminimap\tcatalog\tstop\tprojection\tother_object_overlaps\tmin_panel_font_physical\tblocked\tcompleted\tproduced\tnotice\tnotice_projection_overlap\tlab_island_overlaps\tmin_menu_font_physical\tmenu_control\tcatalog_toggle\tprojected_recipes\trecipe_object_overlaps";
pub(super) fn visible(n:&Node,parent:Option<&ChildOf>,ancestors:&Query<(&Node,Option<&ChildOf>)>)->bool {
    if n.display==Display::None{return false;}let mut p=parent.map(ChildOf::parent);
    while let Some(e)=p{let Ok((n,next))=ancestors.get(e)else{break};if n.display==Display::None{return false;}p=next.map(ChildOf::parent);}true
}
pub(super) fn rect(c:&ComputedNode,g:&UiGlobalTransform)->Option<(Vec2,Vec2)> {
    let half=c.size()*0.5;let points=[-half,Vec2::new(half.x,-half.y),half,Vec2::new(-half.x,half.y)].map(|p|g.affine().transform_point2(p)*c.inverse_scale_factor());
    let lo=points.into_iter().fold(Vec2::splat(f32::INFINITY),Vec2::min);let hi=points.into_iter().fold(Vec2::splat(f32::NEG_INFINITY),Vec2::max);
    (hi.x-lo.x>1.&&hi.y-lo.y>1.).then_some((lo,hi-lo))
}
fn intersects(a:(Vec2,Vec2),b:(Vec2,Vec2))->bool {a.0.x<b.0.x+b.1.x&&b.0.x<a.0.x+a.1.x&&a.0.y<b.0.y+b.1.y&&b.0.y<a.0.y+a.1.y}
/// Target an actual current gap instead of a pixel which may now belong to UI.
#[cfg(test)]
pub(super) fn free_point(viewport:Vec2,islands:&[(Vec2,Vec2)],objects:&[(Vec2,Vec2)])->Option<Vec2> {
    [Vec2::new(0.72,0.40),Vec2::new(0.72,0.65),Vec2::new(0.28,0.40),
     Vec2::new(0.50,0.70),Vec2::new(0.50,0.30),Vec2::new(0.25,0.65)]
        .into_iter().map(|p|p*viewport).find(|p|point_clear(*p,islands,objects))
}
pub(super) fn point_clear(p:Vec2,islands:&[(Vec2,Vec2)],objects:&[(Vec2,Vec2)])->bool {
    islands.iter().chain(objects).all(|(at,size)|p.x<at.x||p.y<at.y||p.x>at.x+size.x||p.y>at.y+size.y)
}
fn show(r:Option<(Vec2,Vec2)>)->String{r.map_or_else(||"None".into(),|(p,s)|format!("[{:.2},{:.2},{:.2},{:.2}]",p.x,p.y,s.x,s.y))}
pub(super) fn sample(clock:f64,scene:&Scene,edit:&Workshop,panel:&panel::Panel,menu:&frontend::Menu,scale:&UiScale,
    windows:&Query<&Window>,nodes:&Nodes,ancestors:&Query<(&Node,Option<&ChildOf>)>,spatial:&spatial_ui::State,motion:&ui_motion::Registry)->(String,Option<String>) {
    let mut queue=None;let mut mini=None;let mut catalog=None;let mut stop=None;let mut deck=None;let mut notice=None;let mut min_font=f32::INFINITY;let mut docks=Vec::new();let mut min_menu=f32::INFINITY;let mut menu_control=None;
    let mut toggle_rect=None;let mut recipe_rects=Vec::new();
    for (_,n,c,g,parent,lab_root,text,font,briefing,root,m,cat,q,part,(action,dock,menu_ink,front)) in nodes.iter(){
        if !visible(n,parent,ancestors){continue;}let r=rect(c,g);
        if dock.is_some(){if let Some(r)=r{docks.push(r);}}
        if matches!(front,Some(frontend::Action::Menu)){menu_control=r;}
        if m{mini=r;}if cat{catalog=r;}if q{queue=r;}
        if briefing{notice=r;}
        if matches!(part,Some(spatial_ui::Part::Deck)){deck=r;}
        if matches!(part,Some(spatial_ui::Part::CatalogToggle)){toggle_rect=r;}
        if matches!(part,Some(spatial_ui::Part::Recipe(_))){if let Some(r)=r{recipe_rects.push(r);}}
        if part.is_none()&&matches!(action,Some(panel::Action::Stop)){stop=r;}
        if menu_ink{if let Some(f)=font{if let FontSize::Px(value)=f.font_size{min_menu=min_menu.min(value*scale.0);}}}
        if text.is_some() {
            let mut under_panel=root||lab_root;let mut p=parent.map(ChildOf::parent);
            while let Some(e)=p{if let Ok((_,_,_,_,parent,is_lab,_,_,_,is_root,..))=nodes.get(e){under_panel|=is_root||is_lab;p=parent.map(ChildOf::parent);}else{break;}}
            if under_panel {if let Some(font)=font {if let FontSize::Px(value)=font.font_size{min_font=min_font.min(value*scale.0);}}}
        }
    }
    let lab_overlaps=docks.iter().enumerate().map(|(i,a)|docks.iter().skip(i+1).filter(|b|intersects(*a,**b)).count()).sum::<usize>();
    let orders:usize=scene.sim.production_lines().into_iter().map(|id|scene.sim.production_queue(id).map_or(0,|q|q.len())).sum();
    let overlaps=deck.map_or(0,|d|spatial.audit_obstacles().iter().filter(|o|intersects(d,**o)).count());
    let recipe_overlaps=recipe_rects.iter().map(|r|spatial.audit_obstacles().iter().filter(|o|intersects(*r,**o)).count()).sum::<usize>();
    let notice_overlap=notice.zip(deck).is_some_and(|(n,d)|intersects(n,d));
    let viewport=windows.single().map_or(Vec2::new(1920.,1080.),|w|w.size()/scale.0);
    let error=if clock>0.5&&orders>0&&!menu.shown()&&queue.is_none(){Some("active production unavailable".into())}
        else if queue.is_some_and(|(p,s)|p.y>32.||p.x<0.||p.x+s.x>viewport.x+1.){Some("queue outside upper viewport".into())}
        else if clock>0.5&&lab_overlaps>0{Some(format!("{lab_overlaps} laboratory islands overlap"))}
        else if menu.shown()&&min_menu<11.99{Some("menu text below12 physical pixels".into())}
        else if overlaps>0{Some(format!("projection covers {overlaps} other actors"))}else if recipe_overlaps>0{Some(format!("recipes cover {recipe_overlaps} protected objects"))}else if notice_overlap{Some("notice overlaps projection".into())}else{None};
    let recipe_geometry=recipe_rects.iter().map(|r|show(Some(*r))).collect::<Vec<_>>().join(";");
    (format!("{clock:.5}\t{}\t{:?}\t{}\t{:.2}\t{orders}\t{}\t{}\t{}\t{}\t{}\t{overlaps}\t{min_font:.2}\t{}\t{}\t{}\t{}\t{notice_overlap}\t{lab_overlaps}\t{min_menu:.2}\t{}\t{}\t{}\t{recipe_overlaps}",scene.sim.tick,edit.controlled,menu.signal_key(),scale.0,show(queue),show(mini),show(catalog),show(stop),show(deck),panel.blocked,motion.completed,scene.sim.spawned().len(),show(notice),show(menu_control),show(toggle_rect),recipe_geometry),error)
}
#[cfg(test)]mod tests {
    use super::*;
    #[test]fn gap_probe_uses_current_hit_islands_and_refuses_full_coverage(){
        let viewport=Vec2::new(960.,540.);
        let busy=(Vec2::new(680.,150.),Vec2::new(264.,140.));
        let point=free_point(viewport,&[busy],&[]).unwrap();
        assert!(!intersects((point,Vec2::ONE),busy));
        assert_eq!(free_point(viewport,&[(Vec2::ZERO,viewport)],&[]),None);
        assert_eq!(free_point(viewport,&[],&[(Vec2::ZERO,viewport)]),None);
    }
}
