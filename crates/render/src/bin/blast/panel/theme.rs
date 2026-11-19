//! Signal tokens: shared semantics, distinct pixel/optical/service registers.
use bevy::prelude::*;
use bevy::text::{FontSmoothing, FontVariationTag, FontVariations, FontWeight};
pub const GRID:f32=4.;
pub const SPACE:f32=8.;
pub const CORNER:f32=8.;
pub const HAIRLINE:f32=1.;
pub const PIXEL_LINE:f32=2.;
pub const TYPE_SCALE:[f32;6]=[12.,14.,16.,20.,28.,40.];
pub const PRESS_SECONDS:f32=0.09;
pub const HOVER_SECONDS:f32=0.09;
pub const REVEAL_SECONDS:f32=0.22;
pub const TRANSITION_SECONDS:f32=0.32;
pub const AMBER:Color=Color::srgb(1.0,0.80,0.29);
pub const CYAN:Color=Color::srgb(0.35,0.86,1.0);
pub const LAVENDER:Color=Color::srgb(0.73,0.68,0.94);
pub const DANGER:Color=Color::srgb(0.97,0.32,0.28);
#[derive(Component,Clone,Copy,Debug,Default,PartialEq,Eq)]
pub enum Style { #[default] Pixel, Optical, Service, Crt }
#[derive(Clone,Copy)]
pub struct Palette { pub panel:Color,pub frame:Color,pub button:Color,pub hover:Color,pub active:Color,pub pressed:Color,pub text:Color,pub muted:Color }
pub const PIXEL:Palette=Palette {
 panel:Color::srgba(0.025,0.045,0.065,0.78),frame:Color::srgba(0.36,0.54,0.65,0.35),
 button:Color::srgba(0.035,0.075,0.105,0.65),hover:Color::srgba(0.07,0.20,0.26,0.84),
 active:Color::srgba(0.055,0.20,0.27,0.86),pressed:Color::srgba(0.085,0.28,0.37,0.94),
 text:Color::srgb(0.89,0.95,0.98),muted:Color::srgb(0.59,0.69,0.76),
};
pub const OPTICAL:Palette=Palette {
 panel:Color::srgba(0.018,0.048,0.066,0.42),frame:Color::srgba(0.45,0.78,0.86,0.48),
 button:Color::srgba(0.04,0.12,0.15,0.16),hover:Color::srgba(0.10,0.30,0.35,0.38),
 active:Color::srgba(0.06,0.28,0.34,0.47),pressed:Color::srgba(0.08,0.37,0.44,0.64),
 text:Color::srgb(0.87,0.95,0.98),muted:Color::srgb(0.59,0.78,0.84),
};
pub const SERVICE:Palette=Palette {
 panel:Color::srgba(0.86,0.91,0.92,0.96),frame:Color::srgba(0.28,0.49,0.54,0.60),
 button:Color::srgba(0.93,0.96,0.95,0.94),hover:Color::srgba(0.75,0.87,0.88,0.98),
 active:Color::srgba(0.98,0.77,0.29,0.92),pressed:Color::srgba(0.91,0.65,0.16,1.),
 text:Color::srgb(0.07,0.16,0.19),muted:Color::srgb(0.25,0.37,0.40),
};
impl Style {
 pub const fn palette(self)->Palette { match self {Self::Pixel|Self::Crt=>PIXEL,Self::Optical=>OPTICAL,Self::Service=>SERVICE} }
 pub fn button(self,i:Interaction,active:bool)->Color {let p=self.palette();match i {Interaction::Pressed=>p.pressed,Interaction::Hovered=>p.hover,Interaction::None if active=>p.active,Interaction::None=>p.button}}
 pub fn border(self,active:bool)->Color {if active {match self {Self::Service=>service::FOCUS,Self::Optical|Self::Pixel|Self::Crt=>CYAN}}else{self.palette().frame}}
}
// Battle/static categories are pixel by default; other callers opt in explicitly.
pub const PANEL:Color=PIXEL.panel;
pub const FRAME:Color=PIXEL.frame;
pub const BUTTON:Color=PIXEL.button;
pub const ACTIVE:Color=PIXEL.active;
pub const PRESSED:Color=PIXEL.pressed;
pub const TEXT:Color=PIXEL.text;
pub const MUTED:Color=PIXEL.muted;
pub const FOCUS:Color=CYAN;
pub const ACCENT:Color=AMBER;
pub mod service {
 use super::*;
 pub const PANEL:Color=SERVICE.panel;pub const FRAME:Color=SERVICE.frame;
 pub const BUTTON:Color=SERVICE.button;
 pub const TEXT:Color=SERVICE.text;pub const MUTED:Color=SERVICE.muted;
 pub const FOCUS:Color=Color::srgb(0.53,0.30,0.02);
 pub const DISABLED:Color=Color::srgb(0.78,0.84,0.85);pub const ACCENT:Color=FOCUS;
 pub fn corners(p:&mut ChildSpawnerCommands){super::corners_for(p,Style::Service);}
 pub fn button_background(i:Interaction,active:bool)->Color {Style::Service.button(i,active)}
}
pub fn data_font(base:&TextFont)->TextFont {
 TextFont{weight:FontWeight::NORMAL,font_smoothing:FontSmoothing::AntiAliased,
  font_variations:FontVariations::builder().set(FontVariationTag::new(b"ELSH"),1.).set(FontVariationTag::new(b"ELGR"),1.).build(),..base.clone()}
}
#[derive(Resource,Clone)]
pub struct Fonts {pub pixel:TextFont,pub data:TextFont,pub optical:TextFont,pub service:TextFont}
impl Fonts {
 pub fn load(fonts:&mut Assets<Font>)->Self {
  let narrow=fonts.add(Font::from_bytes(include_bytes!("../../../../../../data/ui/menu/Oswald.ttf").to_vec()));
  let raster=fonts.add(Font::from_bytes(include_bytes!("../../../../../../data/ui/menu/Handjet.ttf").to_vec()));
  let optical=TextFont{font:narrow.into(),font_size:FontSize::Px(TYPE_SCALE[1]),weight:FontWeight::LIGHT,..default()};
  let service=TextFont{weight:FontWeight::NORMAL,..optical.clone()};
  let pixel=TextFont{font:raster.clone().into(),font_size:FontSize::Px(TYPE_SCALE[2]),weight:FontWeight::BOLD,font_smoothing:FontSmoothing::AntiAliased,
   font_variations:FontVariations::builder().set(FontVariationTag::new(b"ELSH"),8.).set(FontVariationTag::new(b"ELGR"),1.).build(),..default()};
  let data=TextFont{font:raster.into(),font_size:FontSize::Px(TYPE_SCALE[2]),font_variations:FontVariations::builder().set(FontVariationTag::new(b"ELSH"),1.).set(FontVariationTag::new(b"ELGR"),1.).build(),..default()};
  Self{pixel,data,optical,service}
 }
}
/// Fixed marks; decoration never intercepts input or changes a hit rectangle.
pub fn corners(p:&mut ChildSpawnerCommands){corners_for(p,Style::Pixel);}
pub fn corners_for(p:&mut ChildSpawnerCommands,style:Style){
 let pixel=style==Style::Pixel;let line=if pixel{PIXEL_LINE}else{HAIRLINE};let ink=style.palette().frame;
 for (right,bottom) in [(false,false),(true,false),(false,true),(true,true)] {
  p.spawn((Node{position_type:PositionType::Absolute,
   left:if right{Val::Auto}else{px(-line)},right:if right{px(-line)}else{Val::Auto},
   top:if bottom{Val::Auto}else{px(-line)},bottom:if bottom{px(-line)}else{Val::Auto},
   width:px(CORNER),height:px(CORNER),border:UiRect{left:px(if right{0.}else{line}),right:px(if right{line}else{0.}),top:px(if bottom{0.}else{line}),bottom:px(if bottom{line}else{0.})},..default()},BorderColor::all(ink),Pickable::IGNORE));
  if pixel {p.spawn((Node{position_type:PositionType::Absolute,left:if right{Val::Auto}else{px(GRID)},right:if right{px(GRID)}else{Val::Auto},top:if bottom{Val::Auto}else{px(GRID)},bottom:if bottom{px(GRID)}else{Val::Auto},width:px(PIXEL_LINE),height:px(PIXEL_LINE),..default()},BackgroundColor(ink),Pickable::IGNORE));}
 }
}
pub fn button_background(i:Interaction,active:bool)->Color{Style::Pixel.button(i,active)}
