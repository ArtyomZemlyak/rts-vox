//! Heavy eight-legged range unit. Balance is authored in data/spider.ron.
use serde::Deserialize;
#[derive(Clone, Debug, Deserialize)]
pub struct Tuning {
    pub chassis: crate::tuning::Chassis,
    pub speed: i32,
    pub step: i32,
    pub laser: crate::range_weapon::Stream,
    pub charge_ticks: u32,
    #[serde(default)]
    pub laser_sweep: Option<LaserSweep>,
}
/// Opening pass of the siege beam; omitted settings preserve direct aim.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct LaserSweep {
    pub ticks: u32,
    pub lead_units: i32,
}
impl Tuning {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(1..=60).contains(&t.speed)
            || !(1..=32).contains(&t.step)
            || t.chassis.gait.is_none()
            || t.chassis.mass <= 0
            || t.chassis.density <= 0
            || !(1..=300).contains(&t.charge_ticks)
            || !(1..=300).contains(&t.laser.burst_ticks)
            || !(1..=30).contains(&t.laser.interval_ticks)
            || !(1..=3000).contains(&t.laser.range)
            || !(100..=950).contains(&t.laser.standoff_milli)
            || !(0..=150).contains(&t.laser.spread_milli)
            || !(1..=10000).contains(&t.laser.work)
            || !(1..=1000000).contains(&t.laser.fracture_j)
            || !(1..=100000).contains(&t.laser.heat)
            || !(0..=4).contains(&t.laser.heat_radius)
            || !(1..=300).contains(&t.laser.reload_ticks)
            || t.laser_sweep.is_some_and(|s| {
                !(1..=300).contains(&s.ticks)
                    || s.ticks > t.laser.burst_ticks.saturating_sub(1) * t.laser.interval_ticks
                    || !(1..=t.laser.range).contains(&s.lead_units)
            })
        {
            return Err("invalid heavy spider tuning".into());
        }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opening_laser_sweep_has_legacy_default_and_finite_reachable_limits() {
        let source = include_str!("../../../data/spider.ron");
        let direct = source
            .lines()
            .filter(|l| !l.contains("laser_sweep:"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(Tuning::from_ron(&direct).unwrap().laser_sweep.is_none());
        let sweep = Tuning::from_ron(source).unwrap().laser_sweep.unwrap();
        assert_eq!((sweep.ticks, sweep.lead_units), (18, 120));
        for (from, to) in [
            ("ticks: 18", "ticks: 0"),
            ("ticks: 18", "ticks: 120"),
            ("ticks: 18", "ticks: 301"),
            ("lead_units: 120", "lead_units: 0"),
            ("lead_units: 120", "lead_units: 2801"),
            ("burst_ticks: 120", "burst_ticks: 1"),
        ] {
            assert!(Tuning::from_ron(&source.replace(from, to)).is_err(), "{to}");
        }
    }
    #[test]
    fn spider_laser_and_gait_have_finite_budgets() {
        let source = include_str!("../../../data/spider.ron");
        let tuning = Tuning::from_ron(source).unwrap();
        assert_eq!(tuning.laser.interval_ticks, 1);
        let gait = tuning.chassis.gait.unwrap();
        assert!(gait.stride_cm > 0 && gait.duty_pct > 0 && gait.duty_pct < 100);
        assert!(gait.offset_pct.iter().all(|p| (0..100).contains(p)));
        for (from, to) in [
            ("range: 2800", "range: 99999"),
            ("interval_ticks: 1", "interval_ticks: 0"),
            ("heat_radius: 4", "heat_radius: 8"),
        ] {
            assert_ne!(source, source.replace(from, to));
            assert!(Tuning::from_ron(&source.replace(from, to)).is_err());
        }
    }
}
