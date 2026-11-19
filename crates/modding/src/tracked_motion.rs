//! Stock mobility balance for the live, voxel-body tracked roster.
use crate::range::RangeUnitKind;
use serde::Deserialize;

#[derive(Clone,Copy,Debug,Deserialize)]
pub struct Tuning {
    pub speed_multiplier: i32,
    pub acceleration_multiplier: i32,
    pub brake_multiplier: i32,
    pub tier2_speed_pct: i32,
    pub crawler_speed_pct: i32,
}
impl Tuning {
    pub fn from_ron(text:&str)->Result<Self,String> {
        let t:Self=ron::from_str(text).map_err(|e|e.to_string())?;
        if !(1..=10).contains(&t.speed_multiplier)
            || !(1..=100).contains(&t.acceleration_multiplier)
            || !(1..=100).contains(&t.brake_multiplier)
            || !(1..=100).contains(&t.tier2_speed_pct)
            || !(1..=100).contains(&t.crawler_speed_pct) {
            return Err("invalid tracked mobility balance".into());
        }
        Ok(t)
    }
    /// None for walkers, aircraft and static installations sharing a chassis.
    pub fn speed(&self,kind:RangeUnitKind,baseline:i32)->Option<i32> {
        use RangeUnitKind::*;
        let pct=match kind {
            Tank|EidolonTank|Artillery=>100,
            LightningTank|ArtilleryT2=>self.tier2_speed_pct,
            Crawler=>self.tier2_speed_pct,
            _=>return None,
        };
        let speed=baseline*self.speed_multiplier*pct/100;
        Some((if kind==Crawler {speed*self.crawler_speed_pct/100} else {speed}).max(1))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stock_tracks_have_tier_and_cargo_roles_with_bounded_data() {
        let text=include_str!("../../../data/tracked-motion.ron");
        let t=Tuning::from_ron(text).unwrap();
        use RangeUnitKind::*;
        for kind in [Tank,EidolonTank,Artillery] {assert_eq!(t.speed(kind,40),Some(120));}
        for kind in [LightningTank,ArtilleryT2] {assert_eq!(t.speed(kind,40),Some(109));}
        assert_eq!(t.speed(Crawler,40),Some(98));
        for kind in [Commander,Strider,Spider,Walker04,KilFactoryT1,TurretT1] {assert_eq!(t.speed(kind,40),None);}
        for (a,b) in [("speed_multiplier: 3","speed_multiplier: 0"),
            ("acceleration_multiplier: 9","acceleration_multiplier: 101"),
            ("brake_multiplier: 9","brake_multiplier: 0"),
            ("tier2_speed_pct: 91","tier2_speed_pct: 101"),
            ("crawler_speed_pct: 90","crawler_speed_pct: 0")] {
            assert!(Tuning::from_ron(&text.replace(a,b)).is_err());
        }
    }
}
