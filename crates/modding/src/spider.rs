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
    #[serde(default)]
    pub laser_tracking: Option<LaserTracking>,
}
/// A continuously firing siege head has its own finite angular servos.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct LaserTracking {
    pub yaw: crate::turret::Turret,
    pub elevation: crate::turret::Turret,
    pub fire_millidegrees: i32,
    pub elevation_limit_degrees: i32,
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
            || !(1..=12000000).contains(&t.laser.work)
            || !(1..=1000000).contains(&t.laser.fracture_j)
            || !(1..=320000000).contains(&t.laser.heat)
            || !(0..=4).contains(&t.laser.heat_radius)
            || !(1..=300).contains(&t.laser.reload_ticks)
            || t.laser_sweep.is_some_and(|s| {
                !(1..=300).contains(&s.ticks)
                    || s.ticks > t.laser.burst_ticks.saturating_sub(1) * t.laser.interval_ticks
                    || !(1..=t.laser.range).contains(&s.lead_units)
            })
            || t.laser_tracking.is_some_and(|s| {
                [s.yaw, s.elevation].into_iter().any(|a| {
                    !(1..=360).contains(&a.speed_deg_s)
                        || !(1..=3600).contains(&a.accel_deg_s2)
                        || !(1..=10000).contains(&a.aim_millidegrees)
                }) || !(s.yaw.aim_millidegrees..=180000).contains(&s.fire_millidegrees)
                    || !(1..=89).contains(&s.elevation_limit_degrees)
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
            ("ticks: 18", "ticks: 240"),
            ("ticks: 18", "ticks: 301"),
            ("lead_units: 120", "lead_units: 0"),
            ("lead_units: 120", "lead_units: 2801"),
            ("burst_ticks: 240", "burst_ticks: 1"),
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
            ("range: \"spider_laser\"", "range: 99999"),
            ("interval_ticks: 1", "interval_ticks: 0"),
            ("heat_radius: 4", "heat_radius: 8"),
        ] {
            assert_ne!(source, source.replace(from, to));
            assert!(Tuning::from_ron(&source.replace(from, to)).is_err());
        }
    }
    #[test]
    fn spider_laser_has_ten_times_the_previous_contact_power() {
        let source=include_str!("../../../data/spider.ron");
        let t=Tuning::from_ron(source).unwrap();
        assert_eq!((t.laser.work,t.laser.heat,t.laser.fracture_j),(1200000*10,32000000*10,20000));
        assert_eq!((t.laser.heat_radius,t.laser.interval_ticks),(4,1));
        for (from,to) in [("work: 12000000","work: 12000001"),("heat: 320000000","heat: 320000001")] {
            assert!(Tuning::from_ron(&source.replace(from,to)).is_err());
        }
        let common=crate::range_weapon::Tuning::from_ron(include_str!("../../../data/range-weapons.ron")).unwrap();
        assert_eq!((common.laser.work,common.laser.heat,common.laser.fracture_j),(120,400,1));
    }
}

#[cfg(test)]
mod tracking_tests {
    use super::*;
    #[test]
    fn tracking_is_optional_and_all_servos_and_early_fire_cone_are_bounded() {
        let source = include_str!("../../../data/spider.ron");
        let t = Tuning::from_ron(source).unwrap();
        assert!(t.laser_tracking.is_some());
        let begin = source.find("    laser_tracking:").unwrap();
        let end = begin + source[begin..].find("    )),").unwrap() + "    )),".len();
        let legacy = format!("{}{}", &source[..begin], &source[end..]);
        assert!(Tuning::from_ron(&legacy).unwrap().laser_tracking.is_none());
        for (from, to) in [
            ("speed_deg_s: 18", "speed_deg_s: 0"),
            ("accel_deg_s2: 36", "accel_deg_s2: 3601"),
            ("fire_millidegrees: 60000", "fire_millidegrees: 0"),
            ("fire_millidegrees: 60000", "fire_millidegrees: 180001"),
            ("elevation_limit_degrees: 75", "elevation_limit_degrees: 90"),
        ] {
            assert_ne!(source, source.replace(from, to));
            assert!(Tuning::from_ron(&source.replace(from, to)).is_err());
        }
    }
}
