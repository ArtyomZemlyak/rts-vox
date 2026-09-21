//! Bounded stream and automatic weapon tuning for the proving ground.
use serde::Deserialize;
#[derive(Clone, Debug, Deserialize)]
pub struct Stream {
    pub burst_ticks: u32,
    pub interval_ticks: u32,
    pub range: i32,
    pub spread_milli: i32,
    pub work: u32,
    pub heat: u32,
    pub heat_radius: i32,
    pub fracture_j: u32,
    pub reload_ticks: u32,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Tuning {
    pub flame: Stream,
    pub machine_gun: Stream,
    pub smoke_engine_milli: i32,
    pub smoke_core_milli: i32,
    pub smoke_loss_milli: u32,
    pub wreck_fire_ticks: u32,
}
impl Tuning {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        for s in [&t.flame, &t.machine_gun] {
            if !(1..=300).contains(&s.burst_ticks)
                || !(1..=30).contains(&s.interval_ticks)
                || !(1..=3000).contains(&s.range)
                || !(0..=150).contains(&s.spread_milli)
                || !(1..=10000).contains(&s.work)
                || s.heat > 100000
                || !(0..=4).contains(&s.heat_radius)
                || !(1..=1000000).contains(&s.fracture_j)
                || !(1..=300).contains(&s.reload_ticks)
            {
                return Err("range weapon tuning outside bounded limits".into());
            }
        }
        if !(0..=1000).contains(&t.smoke_engine_milli)
            || !(0..=1000).contains(&t.smoke_core_milli)
            || !(1..=1000).contains(&t.smoke_loss_milli)
            || !(30..=18000).contains(&t.wreck_fire_ticks)
        {
            return Err("unit fire tuning outside bounded limits".into());
        }
        Ok(t)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stock_streams_and_invalid_budget() {
        let text = include_str!("../../../data/range-weapons.ron");
        let t = Tuning::from_ron(text).unwrap();
        assert!(t.machine_gun.range > t.flame.range);
        assert!(Tuning::from_ron(&text.replace("interval_ticks: 2", "interval_ticks: 0")).is_err());
    }
}
