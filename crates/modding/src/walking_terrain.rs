//! Bounded terrain capabilities of supported walking layouts.
use serde::Deserialize;

#[derive(Clone,Copy,Debug,Deserialize)]
pub struct Capability {
    pub step_pct: i32,
    pub max_step_cm: i32,
    pub leg_reach_pct: i32,
    pub min_up_pct: i32,
    pub approach_pct: i32,
    pub stance_reach_pct: i32,
    pub stance_drop_pct: i32,
    #[serde(default)] pub detail_pct: i32,
    #[serde(default)] pub max_detail_cm: i32,
    /// Share of healthy legs kept planted by the multiped lift coordinator.
    #[serde(default)] pub min_support_pct: i32,
    /// Minimum share of representative occupied sole probes admitting weight.
    pub sole_ready_pct: i32,
    pub sole_gap_mm: i32,
}
#[derive(Clone,Copy,Debug,Deserialize)]
pub struct Tuning {
    pub biped: Capability,
    pub multiped: Capability,
    pub spider: Capability,
}
impl Tuning {
    pub fn from_ron(text:&str)->Result<Self,String> {
        let t:Self=ron::from_str(text).map_err(|e|e.to_string())?;
        for p in [t.biped,t.multiped,t.spider] {
            if !(1..=100).contains(&p.sole_ready_pct) || !(1..=100).contains(&p.sole_gap_mm) {
                return Err("invalid walking sole readiness".into());
            }
            if !(0..=25).contains(&p.detail_pct) || !(0..=50).contains(&p.max_detail_cm) {
                return Err("invalid walking terrain detail".into());
            }
            if !(0..=90).contains(&p.min_support_pct) {return Err("invalid walking support share".into());}
            if !(25..=300).contains(&p.step_pct) || !(10..=400).contains(&p.max_step_cm)
                || !(10..=60).contains(&p.leg_reach_pct) || !(60..=99).contains(&p.min_up_pct)
                || !(10..=100).contains(&p.approach_pct) {
                return Err("invalid walking terrain capability".into());
            }
            if !(1..=100).contains(&p.stance_drop_pct) || !(75..=99).contains(&p.stance_reach_pct) {return Err("invalid walking stance extension".into());}
        }
        Ok(t)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stock_capabilities_are_ordered_and_bounded() {
        let text=include_str!("../../../data/walking-terrain.ron");
        let t=Tuning::from_ron(text).unwrap();
        assert!(t.biped.max_step_cm<t.multiped.max_step_cm && t.multiped.max_step_cm<t.spider.max_step_cm);
        assert!(t.biped.min_up_pct>t.multiped.min_up_pct && t.multiped.min_up_pct>t.spider.min_up_pct);
        for (valid,bad) in [("max_step_cm: 60","max_step_cm: 999"),("min_up_pct: 82","min_up_pct: 0"),
            ("leg_reach_pct: 40","leg_reach_pct: 100"),("approach_pct: 65","approach_pct: 999")] {
            assert!(Tuning::from_ron(&text.replace(valid,bad)).is_err());
        }
        for (valid,bad) in [("sole_ready_pct: 33","sole_ready_pct: 0"),("sole_ready_pct: 33","sole_ready_pct: 101"),
            ("sole_gap_mm: 50","sole_gap_mm: 0"),("sole_gap_mm: 50","sole_gap_mm: 101")] {
            assert!(Tuning::from_ron(&text.replace(valid,bad)).is_err());
        }
    }
    #[test]
    fn support_share_is_bounded_and_older_profiles_keep_their_default() {
        let text=include_str!("../../../data/walking-terrain.ron");
        assert!(Tuning::from_ron(&text.replace("min_support_pct: 50","min_support_pct: 91")).is_err());
        assert!(Tuning::from_ron(&text.replace("min_support_pct: 50","min_support_pct: -1")).is_err());
        let old=Tuning::from_ron(&text.replace(", min_support_pct: 50", "")).unwrap();
        assert_eq!(old.spider.min_support_pct,0);
        assert_eq!(old.multiped.min_support_pct,0);
    }
}
