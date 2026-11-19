//! Source evidence for event-driven, stable-only sole contacts.
use super::{Changed, Probe, Rubble};
use fx::Fx;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SupportRef {
    /// 0 = terrain, 1 = small fragment, 2 = authored static column overlay.
    /// Live debris always supplies the complete fragment catalogue.
    pub kind: u8,
    pub id: u64,
    pub content: u64,
    pub pose: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Validity { Supported, Moving, Unsupported, Removed }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub height: Fx,
    pub source: SupportRef,
    pub validity: Validity,
}
pub trait Surface {
    fn sample(&self, x: Fx, y:Fx, z: Fx) -> Sample;
    fn validity(&self, source: SupportRef) -> Validity;
}
pub fn mix(hash:u64, value:u64)->u64 { (hash ^ value).wrapping_mul(0x100000001b3) }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub source: SupportRef,
    pub position: [Fx;3],
    pub basis: [[Fx;3];3],
    pub low: [Fx;3],
    pub high: [Fx;3],
    pub shape: u64,
    pub centre: [Fx;3],
    pub side: u8,
    pub mm: u16,
    pub stable: bool,
    pub moving: bool,
}
impl Piece {
    /// top() never returns above its ray start. Keep negative/invalid bounds
    /// on the full path; positive rounding has a monotone integer upper bound.
    fn rounded_upper(&self)->Option<Fx> {
        let start=self.high[1].add(Fx::from_ratio(1,1000));
        if start<=Fx::ZERO || self.low[1]>self.high[1] {return None;}
        start.to_int_trunc().checked_add(i32::from(start.raw()&0xffff_ffff!=0)).map(Fx::from_int)
    }
    fn pages(&self)->([i32;2],[i32;2]) {
        ([0,2].map(|a|self.low[a].to_int_trunc().div_euclid(super::PAGE_SIDE as i32)),
         [0,2].map(|a|self.high[a].to_int_trunc().div_euclid(super::PAGE_SIDE as i32)))
    }
    /// A vertical ray crosses at most 6*side cell-face planes. Test occupied
    /// intervals, not every cell in the piece or every voxel in the world.
    fn top(&self,x:Fx,z:Fx)->Option<Fx> {
        if self.mm==0 || !(1..=4).contains(&self.side) {return None;}
        if x<self.low[0] || x>self.high[0] || z<self.low[2] || z>self.high[2] {return None;}
        let eps=Fx::from_ratio(1,1000);
        let start=self.high[1].add(eps);let span=start.sub(self.low[1]).add(eps);
        let scale=Fx::from_ratio(i32::from(self.mm),i32::from(voxel::unit::WORLD_MM));
        let d=[x.sub(self.position[0]),start.sub(self.position[1]),z.sub(self.position[2])];
        let q=std::array::from_fn::<_,3,_>(|i| (0..3).fold(Fx::ZERO,|v,a|v.add(d[a].mul(self.basis[i][a]))).div(scale).add(self.centre[i]));
        let direction=self.basis.map(|b|b[1].neg().div(scale));
        let half=Fx::from_ratio(49,100); // Same occupied cell skin as Fragment::bounds.
        let mut planes=[Fx::ZERO;26];planes[1]=span;let mut count=2;
        for axis in 0..3 {
            if direction[axis]==Fx::ZERO {continue;}
            for cell in 0..self.side {for edge in [half.neg(),half] {
                let t=Fx::from_int(i32::from(cell)).add(edge).sub(q[axis]).div(direction[axis]);
                if t>Fx::ZERO && t<span {planes[count]=t;count+=1;}
            }}
        }
        planes[..count].sort_unstable();
        for window in planes[..count].windows(2) {
            if window[0]==window[1] {continue;}
            let t=window[0].add(window[1]).mul(Fx::from_ratio(1,2));
            let point=std::array::from_fn::<_,3,_>(|a|q[a].add(direction[a].mul(t)));
            let cell=point.map(|p|(p.add(Fx::from_ratio(1,2)).raw()>>32) as i32);
            let side=i32::from(self.side);
            if cell.iter().any(|&c|c<0 || c>=side) {continue;}
            if (0..3).any(|a|point[a].sub(Fx::from_int(cell[a])).abs()>half) {continue;}
            let bit=(cell[0]+side*cell[1]+side*side*cell[2]) as u32;
            if self.shape & (1u64<<bit)!=0 {return Some(start.sub(window[0]));}
        }
        None
    }
}

#[derive(Clone, Debug, Default)]
struct QueryPage {
    pieces:Vec<usize>,
    fine:std::sync::OnceLock<RayBins>,
}
/// A probe borrows one immutable catalogue. Cache only page ownership, never
/// a ray, contact, revision or readiness result. References cannot outlive it.
#[derive(Default)]
pub(super) struct PageCursor<'a> {
    source:Option<&'a Sources>,
    key:[i32;2],
    page:Option<&'a QueryPage>,
}
#[derive(Clone, Debug, Default)]
struct RayBins {
    starts:Vec<usize>,
    indices:Vec<usize>,
    wide:Vec<usize>,
}
#[derive(Clone, Debug, Default)]
pub struct Sources {
    catalogued:bool,
    pieces:Vec<Piece>,
    pages:std::sync::OnceLock<BTreeMap<[i32;2],QueryPage>>,
}
impl PartialEq for Sources {fn eq(&self,other:&Self)->bool {self.catalogued==other.catalogued && self.pieces==other.pieces}}
impl Eq for Sources {}
impl Sources {
    pub fn new(mut pieces:Vec<Piece>)->Self {
        if !pieces.windows(2).all(|w|w[0].source.id<=w[1].source.id) {
            pieces.sort_unstable_by_key(|p|p.source.id);
        }
        Self{catalogued:true,pieces,pages:Default::default()}
    }
    /// The previous support phase is semantic state: a cold catalogue must
    /// not look like a birth/change of every carrier after restoring a save.
    /// Only piece metadata is stored; occupied cubes are never enumerated.
    pub fn save_baseline(&self,b:&mut fx::blob::Blob) {
        b.bool(self.catalogued);b.u32(self.pieces.len() as u32);
        for p in &self.pieces {
            b.u8(p.source.kind);b.u64(p.source.id);b.u64(p.source.content);b.u64(p.source.pose);
            for v in p.position {b.fx(v);}
            for axis in p.basis {for v in axis {b.fx(v);}}
            for v in p.low {b.fx(v);}for v in p.high {b.fx(v);}
            b.u64(p.shape);for v in p.centre {b.fx(v);}
            b.u8(p.side);b.u16(p.mm);b.bool(p.stable);b.bool(p.moving);
        }
    }
    pub fn load_baseline(s:&mut fx::blob::Scan)->Option<Self> {
        let catalogued=s.bool();let count=s.u32() as usize;
        if count>1<<20 || count>s.left()/206 || !catalogued && count!=0 {return None;}
        let mut pieces=Vec::with_capacity(count);let mut last=None;
        for _ in 0..count {
            let source=SupportRef{kind:s.u8(),id:s.u64(),content:s.u64(),pose:s.u64()};
            let position=std::array::from_fn(|_|s.fx());
            let basis=std::array::from_fn(|_|std::array::from_fn(|_|s.fx()));
            let low=std::array::from_fn(|_|s.fx());let high=std::array::from_fn(|_|s.fx());
            let shape=s.u64();let centre=std::array::from_fn(|_|s.fx());
            let side=s.u8();let mm=s.u16();let stable=s.bool();let moving=s.bool();
            let bounded=|v:Fx|v.raw().unsigned_abs()<=Fx::from_int(Fx::MAX_SAFE_COORD).raw() as u64;
            if !(1..=2).contains(&source.kind) || last.is_some_and(|id|id>=source.id)
                || !(1..=4).contains(&side) || mm==0 || shape==0
                || side<4 && shape>>(u32::from(side).pow(3))!=0
                || position.into_iter().chain(low).chain(high).chain(centre).any(|v|!bounded(v))
                || (0..3).any(|a|low[a]>high[a])
                || basis.iter().flatten().any(|v|v.raw().unsigned_abs()>Fx::from_int(2).raw() as u64)
                || !s.ok() {return None;}
            last=Some(source.id);
            pieces.push(Piece{source,position,basis,low,high,shape,centre,side,mm,stable,moving});
        }
        s.ok().then_some(Self{catalogued,pieces,pages:Default::default()})
    }
    pub fn changes(&self,next:&Self,dirty:&mut Changed) {
        // No piece can change when both catalogues are empty. Legacy column
        // fallback changes are marked locally by Rubble::mark_support_changes.
        if self.catalogued!=next.catalogued && !(self.pieces.is_empty() && next.pieces.is_empty()) {dirty.all=true;}
        let mark=|p:&Piece,dirty:&mut Changed| {
            let (lo,hi)=p.pages();
            for x in lo[0]..=hi[0] {for z in lo[1]..=hi[1] {dirty.pages.insert([x,z]);}}
        };
        let (mut a,mut b)=(0,0);
        while a<self.pieces.len() || b<next.pieces.len() {
            match (self.pieces.get(a),next.pieces.get(b)) {
                (Some(old),Some(new)) if old.source.id==new.source.id => {
                    if old!=new {mark(old,dirty);mark(new,dirty);} a+=1;b+=1;
                }
                (Some(old),Some(new)) if old.source.id<new.source.id => {mark(old,dirty);a+=1;}
                (Some(_),Some(new)) => {mark(new,dirty);b+=1;}
                (Some(old),None) => {mark(old,dirty);a+=1;}
                (None,Some(new)) => {mark(new,dirty);b+=1;}
                _=>break,
            }
        }
    }
    fn validity(&self,source:SupportRef)->Validity {
        if source.kind==0 {return Validity::Supported;}
        let Ok(i)=self.pieces.binary_search_by_key(&source.id,|p|p.source.id) else {return Validity::Removed;};
        let p=&self.pieces[i];
        if p.moving || p.source.pose!=source.pose {Validity::Moving}
        else if p.stable {Validity::Supported} else {Validity::Unsupported}
    }
    fn ray_bins(&self,page:&QueryPage,key:[i32;2])->RayBins {
        let side=super::PAGE_SIDE as i32;let cells=(side*side) as usize;
        let origin=key.map(|p|p*side);
        let mut ranges=Vec::new();let mut wide=Vec::new();
        for &i in &page.pieces {
            let p=&self.pieces[i];
            let lo=[0,2].map(|a|(p.low[a].raw()>>32) as i32);
            let hi=[0,2].map(|a|(p.high[a].raw()>>32) as i32);
            if lo[0]>hi[0] || lo[1]>hi[1] {continue;}
            let w=i64::from(hi[0])-i64::from(lo[0])+1;
            let h=i64::from(hi[1])-i64::from(lo[1])+1;
            // Bound duplication before multiplication or cell enumeration.
            if w>64 || h>64 || w*h>64 {wide.push(i);continue;}
            let local=|v:i32,a:usize|(i64::from(v)-i64::from(origin[a])).clamp(0,i64::from(side-1)) as i32;
            ranges.push((i,[local(lo[0],0),local(lo[1],1)],[local(hi[0],0),local(hi[1],1)]));
        }
        if ranges.is_empty() {return RayBins{wide,..Default::default()};}
        let mut starts=vec![0;cells+1];
        for &(_,lo,hi) in &ranges {for z in lo[1]..=hi[1] {for x in lo[0]..=hi[0] {
            starts[(x+side*z) as usize+1]+=1;
        }}}
        for i in 0..cells {starts[i+1]+=starts[i];}
        let mut indices=vec![0;starts[cells]];let mut next=starts[..cells].to_vec();
        // Page pieces already follow catalogue order. Each row keeps it.
        for (i,lo,hi) in ranges {for z in lo[1]..=hi[1] {for x in lo[0]..=hi[0] {
            let cell=(x+side*z) as usize;indices[next[cell]]=i;next[cell]+=1;
        }}}
        RayBins{starts,indices,wide}
    }
    fn query_pages(&self)->&BTreeMap<[i32;2],QueryPage> {
        let side=super::PAGE_SIDE as i32;
        // Coarse ownership is cheap globally; exact columns are built only in
        // queried busy pages. Sparse pages need no dense offset allocation.
        self.pages.get_or_init(|| {
            let mut pages:BTreeMap<[i32;2],QueryPage>=BTreeMap::new();
            for (i,p) in self.pieces.iter().enumerate() {
                let lo=[0,2].map(|a|((p.low[a].raw()>>32) as i32).div_euclid(side));
                let hi=[0,2].map(|a|((p.high[a].raw()>>32) as i32).div_euclid(side));
                for x in lo[0]..=hi[0] {for z in lo[1]..=hi[1] {pages.entry([x,z]).or_default().pieces.push(i);}}
            }
            pages
        })
    }
    #[cfg(test)]
    fn at(&self,x:Fx,z:Fx)->impl Iterator<Item=&Piece> {
        let side=super::PAGE_SIDE as i32;
        let cell=[x,z].map(|p|(p.raw()>>32) as i32);let key=cell.map(|p|p.div_euclid(side));
        self.at_page(key,cell,self.query_pages().get(&key))
    }
    fn at_cached<'a>(&'a self,x:Fx,z:Fx,cursor:&mut PageCursor<'a>)->impl Iterator<Item=&'a Piece>+'a {
        if self.pieces.is_empty() {return self.at_page([0;2],[0;2],None);}
        let side=super::PAGE_SIDE as i32;
        let cell=[x,z].map(|p|(p.raw()>>32) as i32);let key=cell.map(|p|p.div_euclid(side));
        let page=if cursor.key==key && cursor.source.is_some_and(|source|std::ptr::eq(source,self)) {
            cursor.page
        }else{
            let page=self.query_pages().get(&key);
            *cursor=PageCursor{source:Some(self),key,page};page
        };
        self.at_page(key,cell,page)
    }
    fn at_page<'a>(&'a self,key:[i32;2],cell:[i32;2],page:Option<&'a QueryPage>)->impl Iterator<Item=&'a Piece>+'a {
        let side=super::PAGE_SIDE as i32;
        let (row,wide):(&[usize],&[usize])=if let Some(page)=page {
            if page.pieces.len()<=32 {(page.pieces.as_slice(),&[])}else{
                let bins=page.fine.get_or_init(||self.ray_bins(page,key));
                let at=(cell[0].rem_euclid(side)+side*cell[1].rem_euclid(side)) as usize;
                let row=if bins.starts.is_empty(){&[][..]}else{&bins.indices[bins.starts[at]..bins.starts[at+1]]};
                (row,bins.wide.as_slice())
            }
        }else{(&[],&[])};
        let mut a=row.iter().copied().peekable();let mut b=wide.iter().copied().peekable();
        std::iter::from_fn(move||match (a.peek(),b.peek()) {
            (Some(i),Some(j)) if i<=j=>a.next(),
            (Some(_),Some(_)) | (None,Some(_))=>b.next(),
            (Some(_),None)=>a.next(),(None,None)=>None,
        }).map(move |i|&self.pieces[i])
    }

}
impl Rubble {
    /// Piece identity is independent of column heights. Changing legacy
    /// fallback mode also changes support only on the columns it can own.
    pub fn mark_support_changes(&self,next:&Self,dirty:&mut Changed) {
        self.3.changes(&next.3,dirty);
        if self.3.catalogued!=next.3.catalogued && !dirty.all {
            for (at,_) in self.0.iter().chain(next.0.iter()) {
                dirty.pages.insert(at.map(|v|v.div_euclid(super::PAGE_SIDE as i32)));
            }
        }
    }
    pub fn with_sources(mut self,sources:Sources)->Self {self.3=sources;self}
    pub fn sources(&self)->&Sources {&self.3}
    pub fn replace_sources(&mut self,sources:Sources) {self.3=sources;}
    pub fn take_sources(&mut self)->Sources {std::mem::take(&mut self.3)}
    pub fn support_changes(&self)->&Changed {&self.4}
    pub fn set_support_changes(&mut self,changed:Changed) {self.4=changed;}
    fn source_validity(&self,source:SupportRef)->Validity {
        if source.kind==2 {
            let at=[(source.id>>32) as u32 as i32,source.id as u32 as i32];
            return if !self.3.catalogued && self.get(&at).is_some_and(|h|u64::from(*h)==source.content) {
                Validity::Supported
            } else {Validity::Removed};
        }
        self.3.validity(source)
    }
}
impl Probe<'_> {
    pub fn source_validity(&self,source:SupportRef)->Validity {
        self.rubble.map_or(if source.kind==0 {Validity::Supported}else{Validity::Removed},|r|r.source_validity(source))
    }
    pub fn support_sample(&mut self,x:Fx,z:Fx)->Sample {
        self.support_sample_at(x,z,None)
    }
    pub(super) fn support_sample_at(&mut self,x:Fx,z:Fx,contact_y:Option<Fx>)->Sample {
        let at=[x,z].map(|p|(p.raw()>>32) as i32);
        let height=Fx::from_int(self.floor(at[0],at[1]));
        let mut result=Sample {height,source:SupportRef::default(),validity:Validity::Unsupported};
        if at[0]<0 || at[1]<0 || at[0]>=self.world.nx as i32 || at[1]>=self.world.nz as i32 {return result;}
        let y=height.to_int_trunc();
        if y>0 && y<=self.world.ny as i32 {
            let material=self.world.material(at[0] as u32,(y-1) as u32,at[1] as u32);
            if material!=voxel::store::AIR && !self.yields(material) {
                result.source=SupportRef {kind:0,id:0,content:mix(y as u64,u64::from(material)),pose:0};
                result.validity=Validity::Supported;
            }
        }
        if let Some(rubble)=self.rubble {
            if !rubble.3.catalogued && result.validity!=Validity::Supported && rubble.get(&at).is_some_and(|h|Fx::from_int(i32::from(*h))==height) {
                result=Sample{height,source:SupportRef{kind:2,id:(u64::from(at[0] as u32)<<32)|u64::from(at[1] as u32),content:y as u64,pose:0},validity:Validity::Supported};
            }
            for piece in rubble.3.at_cached(x,z,&mut self.support_page) {
                // A stable piece cannot replace a supported source or a moving
                // obstruction already found. Avoid solving rays with no effect.
                if piece.stable && (result.validity==Validity::Supported || result.source.kind==1) {continue;}
                if !piece.stable && contact_y.is_some_and(|y|y<piece.low[1]) {continue;}
                if let Some(upper)=piece.rounded_upper() {
                    let moving=result.source.kind==1 && result.validity!=Validity::Supported;
                    if upper<height || !piece.stable && (contact_y.is_some_and(|y|y>upper)
                        || moving && upper<=result.height) {continue;}
                }
                let Some(top)=piece.top(x,z) else {continue;};
                let rounded=Fx::from_int(top.to_int_trunc()+i32::from(top.raw()&0xffff_ffff!=0));
                if !piece.stable && rounded>=height && contact_y.is_none_or(|y|y>=piece.low[1] && y<=rounded) {
                    if result.source.kind!=1 || result.validity==Validity::Supported || rounded>result.height {
                        result=Sample{height:rounded,source:piece.source,validity:if piece.moving {Validity::Moving}else{Validity::Unsupported}};
                    }
                } else if !(result.source.kind==1 && result.validity!=Validity::Supported) && piece.stable && rounded==height && result.validity!=Validity::Supported {
                    result=Sample{height,source:piece.source,validity:Validity::Supported};
                }
            }
        }
        result
    }
    /// Exact `support_sample_at(x, z, Some(y))` plus whether the integer column
    /// has no overlapping piece capable of changing the sample. Stable pieces
    /// cannot replace established terrain. A clean sample is invariant across
    /// fractional `(x, z)` within that cell and across `y`.
    pub fn support_sample_cell(&mut self,x:Fx,y:Fx,z:Fx)->(Sample,bool) {
        let at=[x,z].map(|p|(p.raw()>>32) as i32);
        let height=Fx::from_int(self.floor(at[0],at[1]));
        let mut result=Sample {height,source:SupportRef::default(),validity:Validity::Unsupported};
        if at[0]<0 || at[1]<0 || at[0]>=self.world.nx as i32 || at[1]>=self.world.nz as i32 {return (result,true);}
        let hy=height.to_int_trunc();
        if hy>0 && hy<=self.world.ny as i32 {
            let material=self.world.material(at[0] as u32,(hy-1) as u32,at[1] as u32);
            if material!=voxel::store::AIR && !self.yields(material) {
                result.source=SupportRef {kind:0,id:0,content:mix(hy as u64,u64::from(material)),pose:0};
                result.validity=Validity::Supported;
            }
        }
        let mut cell_clean=true;
        if let Some(rubble)=self.rubble {
            if !rubble.3.catalogued && result.validity!=Validity::Supported && rubble.get(&at).is_some_and(|h|Fx::from_int(i32::from(*h))==height) {
                result=Sample{height,source:SupportRef{kind:2,id:(u64::from(at[0] as u32)<<32)|u64::from(at[1] as u32),content:hy as u64,pose:0},validity:Validity::Supported};
            }
            let supported_terrain=result.validity==Validity::Supported;
            for piece in rubble.3.at_cached(x,z,&mut self.support_page) {
                if piece.stable && supported_terrain {continue;}
                let overlaps=(piece.low[0].raw()>>32) as i32<=at[0] && at[0]<=(piece.high[0].raw()>>32) as i32
                    && (piece.low[2].raw()>>32) as i32<=at[1] && at[1]<=(piece.high[2].raw()>>32) as i32;
                if !overlaps {continue;}
                cell_clean=false;
                if piece.stable && (result.validity==Validity::Supported || result.source.kind==1) {continue;}
                if y<piece.low[1] && !piece.stable {continue;}
                if let Some(upper)=piece.rounded_upper() {
                    let moving=result.source.kind==1 && result.validity!=Validity::Supported;
                    if upper<height || !piece.stable && (y>upper || moving && upper<=result.height) {continue;}
                }
                let Some(top)=piece.top(x,z) else {continue;};
                let rounded=Fx::from_int(top.to_int_trunc()+i32::from(top.raw()&0xffff_ffff!=0));
                if !piece.stable && rounded>=height && y>=piece.low[1] && y<=rounded {
                    if result.source.kind!=1 || result.validity==Validity::Supported || rounded>result.height {
                        result=Sample{height:rounded,source:piece.source,validity:if piece.moving {Validity::Moving}else{Validity::Unsupported}};
                    }
                } else if !(result.source.kind==1 && result.validity!=Validity::Supported) && piece.stable && rounded==height && result.validity!=Validity::Supported {
                    result=Sample{height,source:piece.source,validity:Validity::Supported};
                }
            }
        }
        (result,cell_clean)
    }
    /// Moving pieces obstruct a foot even though they are never terrain floor.
    pub fn foot_floor(&mut self,x:Fx,z:Fx)->Fx {
        self.foot_floor_below(x,z,Fx::MAX)
    }
    /// Foot-level obstruction query bounded by the walker's hip ceiling so
    /// airborne canopy or shrapnel passing overhead cannot pull soles skyward.
    pub fn foot_floor_below(&mut self,x:Fx,z:Fx,ceiling:Fx)->Fx {
        self.foot_floor_cell(x,z,ceiling).0
    }
    /// Exact `foot_floor_below` plus whether the integer column has zero
    /// overlapping unstable pieces below `ceiling` above the terrain floor.
    pub fn foot_floor_cell(&mut self,x:Fx,z:Fx,ceiling:Fx)->(Fx,bool) {
        let ix=(x.raw()>>32) as i32;let iz=(z.raw()>>32) as i32;
        let base=Fx::from_int(self.floor(ix,iz));
        let mut height=base;
        let mut cell_clean=true;
        if let Some(rubble)=self.rubble {for piece in rubble.3.at_cached(x,z,&mut self.support_page).filter(|p|!p.stable) {
            if piece.low[1]>=ceiling || piece.rounded_upper().is_some_and(|upper|upper<=base) {continue;}
            let overlaps=(piece.low[0].raw()>>32) as i32<=ix && ix<=(piece.high[0].raw()>>32) as i32
                && (piece.low[2].raw()>>32) as i32<=iz && iz<=(piece.high[2].raw()>>32) as i32;
            if !overlaps {continue;}
            cell_clean=false;
            if piece.rounded_upper().is_some_and(|upper|upper<=height) {continue;}
            if let Some(top)=piece.top(x,z) {
                height=height.max(Fx::from_int(top.to_int_trunc()+i32::from(top.raw()&0xffff_ffff!=0)));
            }
        }}
        (height,cell_clean)
    }
}
impl Surface for std::cell::RefCell<Probe<'_>> {
    fn sample(&self,x:Fx,y:Fx,z:Fx)->Sample {self.borrow_mut().support_sample_at(x,z,Some(y))}
    fn validity(&self,source:SupportRef)->Validity {
        self.borrow().source_validity(source)
    }
}
impl Surface for std::cell::RefCell<&mut Probe<'_>> {
    fn sample(&self,x:Fx,y:Fx,z:Fx)->Sample {self.borrow_mut().support_sample_at(x,z,Some(y))}
    fn validity(&self,source:SupportRef)->Validity {
        self.borrow().source_validity(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn piece()->Piece {
        Piece {source:SupportRef{kind:1,id:7,content:1,pose:2},position:[Fx::from_ratio(129,2),Fx::from_ratio(39,2),Fx::from_ratio(129,2)],
            basis:[[Fx::ONE,Fx::ZERO,Fx::ZERO],[Fx::ZERO,Fx::ONE,Fx::ZERO],[Fx::ZERO,Fx::ZERO,Fx::ONE]],
            low:[Fx::from_int(64),Fx::from_int(19),Fx::from_int(64)],high:[Fx::from_int(65),Fx::from_int(20),Fx::from_int(65)],
            centre:[Fx::ZERO;3],shape:1,side:1,mm:100,stable:true,moving:false}
    }
    #[test]
    fn empty_support_catalogue_mode_changes_are_quiet_in_both_directions_and_after_restore() {
        let cold=Sources::default();let live=Sources::new(vec![]);
        for (old,next) in [(&cold,&live),(&live,&cold)] {
            let mut dirty=Changed::default();old.changes(next,&mut dirty);
            assert!(dirty.is_empty(),"empty catalogue mode must not invent a global support edit");
            let mut bytes=fx::blob::Blob::new();old.save_baseline(&mut bytes);
            let restored=Sources::load_baseline(&mut fx::blob::Scan::new(&bytes.v)).unwrap();
            restored.changes(next,&mut dirty);assert!(dirty.is_empty());
            dirty.pages.insert([7,9]);old.changes(next,&mut dirty);
            assert!(!dirty.all);assert_eq!(dirty.pages.len(),1,"existing edits must be retained");
        }
    }
    #[test]
    fn real_support_piece_changes_keep_old_and_new_pages_and_nonempty_mode_changes() {
        let p=piece();let old=Sources::new(vec![p.clone()]);
        for case in 0..7 {
            let mut next=p.clone();
            match case {
                0=>next.source.content+=1,
                1=>next.source.pose+=1,
                2=>{next.moving=true;next.stable=false;},
                3=>next.source.id+=1,
                4=>{next.position[0]=next.position[0].add(Fx::from_int(256));next.low[0]=next.low[0].add(Fx::from_int(256));next.high[0]=next.high[0].add(Fx::from_int(256));},
                5=>{next.side=2;next.shape=3;},
                _=>{},
            }
            let next=Sources::new(if case==6 {vec![]}else{vec![next]});
            let mut dirty=Changed::default();old.changes(&next,&mut dirty);
            assert!(!dirty.all,"real piece edits remain addressed");
            assert!(dirty.touches([64,64],[65,65]),"old footprint missing for case{case}");
            if case==4 {assert!(dirty.touches([320,64],[321,65]),"new footprint missing");}
            assert!(!dirty.touches([10000,10000],[10001,10001]));
        }
        let cold=Sources::default();
        for (a,b) in [(&cold,&old),(&old,&cold)] {
            let mut dirty=Changed::default();a.changes(b,&mut dirty);
            assert!(dirty.all,"nonempty catalogue transitions retain conservative invalidation");
            assert!(dirty.touches([64,64],[65,65]));
        }
    }
    #[test]
    fn empty_piece_mode_changes_still_invalidate_real_legacy_columns_locally() {
        let world=voxel::World::new(128,64,128,4096,Box::new(Ground));
        let legacy=Rubble::from_sorted(vec![([64,64],20)]);
        let live=legacy.clone().with_sources(Sources::new(vec![]));
        let x=Fx::from_int(64);
        let mut before=Probe::new(&world).with_rubble(&legacy);
        let sample=before.support_sample(x,x);
        assert_eq!((sample.validity,sample.source.kind),(Validity::Supported,2));
        let mut after=Probe::new(&world).with_rubble(&live);
        assert_eq!(after.source_validity(sample.source),Validity::Removed);
        assert_eq!(after.support_sample(x,x).validity,Validity::Unsupported);
        for (a,b) in [(&legacy,&live),(&live,&legacy)] {
            let mut dirty=Changed::default();a.mark_support_changes(b,&mut dirty);
            assert!(!dirty.all);assert!(dirty.touches([64,64],[64,64]));
            assert!(!dirty.touches([1000,1000],[1001,1001]));
        }
        let cold=Rubble::new();let empty=Rubble::new().with_sources(Sources::new(vec![]));
        let mut quiet=Changed::default();cold.mark_support_changes(&empty,&mut quiet);
        assert!(quiet.is_empty());
    }
    #[test]
    fn support_baseline_restore_preserves_exact_pending_events_without_a_cold_birth() {
        let old=Sources::new(vec![piece()]);let mut next=old.clone();
        next.pieces[0].source.pose+=1;next.pieces[0].moving=true;next.pieces[0].stable=false;
        let mut bytes=fx::blob::Blob::new();old.save_baseline(&mut bytes);
        let restored=Sources::load_baseline(&mut fx::blob::Scan::new(&bytes.v)).unwrap();
        assert_eq!(old,restored);
        let mut a=Changed::default();let mut b=Changed::default();
        old.changes(&next,&mut a);restored.changes(&next,&mut b);assert_eq!(a,b);
        assert!(a.touches([64,64],[65,65]));assert!(!a.touches([1000,1000],[1001,1001]));
        let mut quiet=Changed::default();restored.changes(&old,&mut quiet);assert!(quiet.is_empty());
        assert!(Sources::load_baseline(&mut fx::blob::Scan::new(&bytes.v[..bytes.v.len()-1])).is_none());
        let cold=Sources::default();let mut raw=fx::blob::Blob::new();cold.save_baseline(&mut raw);
        assert_eq!(cold,Sources::load_baseline(&mut fx::blob::Scan::new(&raw.v)).unwrap());
    }

    #[test]
    fn support_dense_page_queries_keep_exact_rays_and_catalogue_order() {
        let mut pieces=Vec::new();
        for z in 0..64 {for x in 0..64 {
            let mut p=piece();p.source.id=(1+x+64*z) as u64;
            p.position=[Fx::from_int(x),Fx::from_int(19),Fx::from_int(z)];
            let skin=Fx::from_ratio(49,100);
            p.low=p.position.map(|v|v.sub(skin));p.high=p.position.map(|v|v.add(skin));
            pieces.push(p);
        }}
        let mut large=piece();large.source.id=0;
        large.position=[Fx::ZERO,Fx::from_int(19),Fx::ZERO];
        large.low=[Fx::from_int(-64),Fx::from_ratio(1851,100),Fx::from_int(-64)];
        large.high=[Fx::from_int(64),Fx::from_ratio(1949,100),Fx::from_int(64)];
        pieces.push(large);
        let sources=Sources::new(pieces);
        let mut cursor=PageCursor::default();
        let q=Fx::from_ratio(17,4);
        assert!(sources.at(q,q).count()<128,"a sole ray must not visit all4096 pieces in one dirty page");
        for [x,z] in [[q,q],[Fx::ZERO,Fx::ZERO],[Fx::from_int(8),Fx::from_int(8)],
            [Fx::from_ratio(-1,4),Fx::ZERO],[Fx::from_ratio(253,4),Fx::from_int(63)]] {
            let expected:Vec<_>=sources.pieces.iter().filter_map(|p|p.top(x,z).map(|y|(p.source.id,y))).collect();
            let actual:Vec<_>=sources.at(x,z).filter_map(|p|p.top(x,z).map(|y|(p.source.id,y))).collect();
            assert_eq!(actual,expected,"fine bins preserve exact cell skins, boundaries and source order");
            let cached:Vec<_>=sources.at_cached(x,z,&mut cursor).filter_map(|p|p.top(x,z).map(|y|(p.source.id,y))).collect();
            assert_eq!(cached,expected,"page ownership never caches an individual occupied ray");
        }
    }
    #[test]
    fn support_page_cursor_rebinds_equal_height_sources_wake_remove_and_restore() {
        let original=Sources::new(vec![piece()]);
        let mut replacement=piece();replacement.source.id+=1;replacement.source.content+=1;
        let mut awake=piece();awake.source.pose+=1;awake.stable=false;awake.moving=true;
        let mut blob=fx::blob::Blob::new();original.save_baseline(&mut blob);
        let restored=Sources::load_baseline(&mut fx::blob::Scan::new(&blob.v)).unwrap();
        let catalogues=[Sources::default(),original,Sources::new(vec![replacement]),
            Sources::new(vec![awake]),Sources::new(vec![]),restored];
        let mut cursor=PageCursor::default();
        for _ in 0..2 {for sources in &catalogues {
            for [x,z] in [[Fx::from_ratio(129,2);2],[Fx::from_int(65),Fx::from_int(64)],
                [Fx::from_int(-1);2],[Fx::from_ratio(129,2);2]] {
                let ray=|p:&Piece|p.top(x,z).map(|top|(p.source,p.stable,p.moving,top));
                let expected:Vec<_>=sources.pieces.iter().filter_map(ray).collect();
                let cached:Vec<_>=sources.at_cached(x,z,&mut cursor).filter_map(ray).collect();
                assert_eq!(cached,expected,"same page/height must retain actual source and occupied geometry");
            }
        }}
    }
    #[test]
    fn support_source_events_include_equal_height_replacement_wake_pose_and_remove() {
        let p=piece();let old=Sources::new(vec![p.clone()]);
        let mut quiet=Changed::default();old.changes(&old,&mut quiet);assert!(quiet.is_empty());
        for case in 0..4 {
            let mut next=p.clone();match case {0=>next.source.id+=1,1=>{next.stable=false;next.moving=true;},2=>next.source.pose+=1,_=>{}}
            let next=Sources::new(if case==3 {vec![]}else{vec![next]});
            let mut dirty=Changed::default();old.changes(&next,&mut dirty);
            assert!(dirty.touches([64,64],[65,65]));assert!(!dirty.touches([512,512],[520,520]));
            assert_eq!(next.validity(p.source),match case {0|3=>Validity::Removed,_=>Validity::Moving});
        }
    }
    #[test]
    fn support_ray_visits_occupied_intervals_and_preserves_shape_holes() {
        let mut p=piece();p.side=2;p.mm=100;p.centre=[Fx::ZERO;3];
        p.position=[Fx::from_int(64),Fx::from_int(19),Fx::from_int(64)];
        p.low=[Fx::from_ratio(127,2),Fx::from_ratio(37,2),Fx::from_ratio(127,2)];
        p.high=[Fx::from_ratio(131,2),Fx::from_ratio(41,2),Fx::from_ratio(131,2)];
        p.shape=1|(1<<2); // Two cells vertically; the neighbouring column is empty.
        assert_eq!(p.top(Fx::from_int(64),Fx::from_int(64)),Some(Fx::from_ratio(2049,100)));
        assert_eq!(p.top(Fx::from_int(65),Fx::from_int(64)),None);
        p.shape=3;
        assert_eq!(p.top(Fx::from_ratio(129,2),Fx::from_int(64)),None,"cell-skin gaps must not become support");
    }
    struct Ground;
    impl voxel::Generator for Ground {fn material(&self,_:u32,y:u32,_:u32)->u16 {u16::from(y<16)}}
    #[test]
    fn support_buried_and_overhead_rays_preserve_floor_and_live_carrier() {
        let world=voxel::World::new(128,128,128,4096,Box::new(Ground));
        let layer=|id:u64,y:i32,moving:bool| {
            let mut p=piece();p.source.id=id;
            p.position[1]=Fx::from_ratio(2*y+1,2);
            p.low[1]=Fx::from_int(y);p.high[1]=Fx::from_int(y+1);
            p.stable=!moving;p.moving=moving;p
        };
        for (above,floor,validity,id) in [(None,20,Validity::Supported,7),
            (Some(19),20,Validity::Moving,8),(Some(21),22,Validity::Supported,7),
            (Some(100),101,Validity::Supported,7)] {
            let mut pieces:Vec<_>=[-2,0,4,9,14].into_iter().enumerate()
                .map(|(i,y)|layer(i as u64+1,y,true)).collect();
            pieces.push(layer(7,19,false));
            if let Some(y)=above {pieces.push(layer(8,y,true));}
            let rubble=Rubble::from_sorted(vec![([64,64],20)]).with_sources(Sources::new(pieces));
            let surface=std::cell::RefCell::new(Probe::new(&world).with_rubble(&rubble));
            let x=Fx::from_ratio(129,2);
            assert_eq!(surface.borrow_mut().foot_floor(x,x),Fx::from_int(floor));
            let sample=surface.sample(x,Fx::from_int(20),x);
            assert_eq!((sample.height,sample.validity,sample.source.id),
                (Fx::from_int(20),validity,id));
        }
    }
    #[test]
    fn support_moving_piece_is_obstruction_but_never_ready_floor() {
        let world=voxel::World::new(128,64,128,4096,Box::new(Ground));let p=piece();
        let stable=Rubble::from_sorted(vec![([64,64],20)]).with_sources(Sources::new(vec![p.clone()]));
        let mut probe=Probe::new(&world).with_rubble(&stable);
        let x=Fx::from_ratio(129,2);
        let sample=probe.support_sample(x,x);assert_eq!(sample.source.id,7);assert_eq!(sample.validity,Validity::Supported);
        let moving=Rubble::new().with_sources(Sources::new(vec![Piece{moving:true,stable:false,..p}]));
        let mut probe=Probe::new(&world).with_rubble(&moving);
        assert_eq!(probe.floor(64,64),16);
        assert_eq!(probe.foot_floor(x,x),Fx::from_int(20));
        assert_eq!(probe.support_sample(x,x).validity,Validity::Moving);
        let probe=std::cell::RefCell::new(probe);
        assert_eq!(probe.sample(x,Fx::from_int(16),x).validity,Validity::Supported,"an overhead piece cannot remove terrain contact");
        assert_eq!(probe.sample(x,Fx::from_int(20),x).validity,Validity::Moving);
        let canopy=Piece{source:SupportRef{kind:1,id:9,content:1,pose:2},
            position:[Fx::from_ratio(129,2),Fx::from_int(90),Fx::from_ratio(129,2)],
            low:[Fx::from_int(64),Fx::from_int(88),Fx::from_int(64)],
            high:[Fx::from_int(65),Fx::from_int(92),Fx::from_int(65)],
            moving:true,stable:false,..p};
        let mixed=Rubble::new().with_sources(Sources::new(vec![Piece{moving:true,stable:false,..p},canopy]));
        let mut bounded=Probe::new(&world).with_rubble(&mixed);
        assert_eq!(bounded.foot_floor_below(x,x,Fx::from_int(48)),Fx::from_int(20),
            "low moving rubble still obstructs the foot while overhead falling canopy is ignored");
        assert_eq!(bounded.foot_floor(x,x),Fx::from_int(91));
    }
    #[test]
    fn support_sparse_source_index_excludes_ten_thousand_remote_fragments() {
        let p=piece();let mut pieces=vec![p.clone()];
        for i in 0..10_000 {
            let mut far=p.clone();far.source.id=100+i;
            let dx=Fx::from_int(1024+(i%100) as i32*64);let dz=Fx::from_int(1024+(i/100) as i32*64);
            for coords in [&mut far.position,&mut far.low,&mut far.high] {coords[0]=coords[0].add(dx);coords[2]=coords[2].add(dz);}
            pieces.push(far);
        }
        let sources=Sources::new(pieces);assert!(sources.pages.get().is_none());
        let at=Fx::from_ratio(129,2);
        assert_eq!(sources.at(at,at).count(),1);
        assert!(sources.pages.get().is_some());
        let mut dirty=Changed::default();sources.changes(&sources,&mut dirty);assert!(dirty.is_empty());
        let world=voxel::World::new(128,64,128,4096,Box::new(Ground));
        let rubble=Rubble::from_sorted(vec![([64,64],20)]).with_sources(sources);
        let mut probe=Probe::new(&world).with_rubble(&rubble);
        assert_eq!(probe.support_sample(at,at).source.id,7);
    }
    #[test]
    fn support_sample_cell_and_foot_floor_cell_distinguish_clean_and_occupied_columns() {
        let world=voxel::World::new(128,64,128,4096,Box::new(Ground));
        let mut half=piece();
        half.moving=true;half.stable=false;
        half.position=[Fx::from_ratio(257,4),Fx::from_ratio(39,2),Fx::from_ratio(257,4)];
        half.low=[Fx::from_int(64),Fx::from_int(19),Fx::from_int(64)];
        half.high=[Fx::from_ratio(129,2),Fx::from_int(20),Fx::from_ratio(129,2)];
        let rubble=Rubble::new().with_sources(Sources::new(vec![half]));
        let mut probe=Probe::new(&world).with_rubble(&rubble);
        // Column (66, 66) on the same 32x32 page has no overlapping piece: clean=true.
        let clean_x=Fx::from_ratio(133,2);
        assert_eq!(probe.foot_floor_cell(clean_x,clean_x,Fx::from_int(48)),(Fx::from_int(16),true));
        let (clean_sample,clean_flag)=probe.support_sample_cell(clean_x,Fx::from_int(16),clean_x);
        assert!(clean_flag && clean_sample.validity==Validity::Supported);
        // Column (64, 64) partially covered by `half`: clean=false so fractional sub-cell queries remain exact.
        let hit_x=Fx::from_ratio(257,4);
        let miss_x=Fx::from_ratio(259,4);
        assert_eq!(probe.foot_floor_cell(hit_x,hit_x,Fx::from_int(48)),(Fx::from_int(20),false));
        assert_eq!(probe.foot_floor_cell(miss_x,miss_x,Fx::from_int(48)),(Fx::from_int(16),false));
        assert_eq!(probe.support_sample_cell(hit_x,Fx::from_int(20),hit_x).0.validity,Validity::Moving);
        assert_eq!(probe.support_sample_cell(miss_x,Fx::from_int(16),miss_x).0.validity,Validity::Supported);
    }

    #[test]
    fn cell_queries_preserve_exact_samples_and_only_mark_invariant_columns_clean() {
        let world=voxel::World::new(128,64,128,4096,Box::new(Ground));
        for stable in [false,true] {for moving in [false,true] {
            let mut half=piece();half.stable=stable;half.moving=moving;
            half.position=[Fx::from_ratio(257,4),Fx::from_ratio(39,2),Fx::from_ratio(257,4)];
            half.low=[Fx::from_int(64),Fx::from_int(19),Fx::from_int(64)];
            half.high=[Fx::from_ratio(129,2),Fx::from_int(20),Fx::from_ratio(129,2)];
            let rubble=Rubble::new().with_sources(Sources::new(vec![half]));
            let mut actual=Probe::new(&world).with_rubble(&rubble);
            let mut reference=Probe::new(&world).with_rubble(&rubble);
            for cell in [-1,0,63,64,65,127,128] {for fraction in [0,1,3] {for y in [0,16,19,20,48] {
                let x=Fx::from_int(cell).add(Fx::from_ratio(fraction,4));let z=x;
                let (sample,clean)=actual.support_sample_cell(x,Fx::from_int(y),z);
                assert_eq!(sample,reference.support_sample_at(x,z,Some(Fx::from_int(y))));
                if clean {for (sub,other_y) in [(1,0),(7,48)] {
                    let other=Fx::from_int(cell).add(Fx::from_ratio(sub,8));
                    assert_eq!(sample,reference.support_sample_at(other,other,Some(Fx::from_int(other_y))),"clean column must be invariant");
                }}
                for ceiling in [18,19,20,48] {
                    let ceiling=Fx::from_int(ceiling);let base=Fx::from_int(reference.floor(cell,cell));
                    let expected=rubble.3.pieces.iter().filter(|p|!p.stable && p.low[1]<ceiling)
                        .filter_map(|p|p.top(x,z)).map(|top|Fx::from_int(top.to_int_trunc()+i32::from(top.raw()&0xffff_ffff!=0)))
                        .fold(base,Fx::max);
                    assert_eq!(actual.foot_floor_cell(x,z,ceiling).0,expected,"full piece scan reference");
                }
            }}}
        }}
    }
}
