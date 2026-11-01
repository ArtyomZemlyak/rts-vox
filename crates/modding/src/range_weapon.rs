//! Bounded stream and automatic weapon tuning for the proving ground.
use serde::Deserialize;
#[derive(Clone, Debug, Deserialize)]
pub struct Stream {
    /// Duration in authored 30 Hz samples, independent of simulation Hz.
    pub burst_ticks: u32,
    /// Sample interval; fractional simulation deadlines retain salvo count.
    pub interval_ticks: u32,
    #[serde(deserialize_with = "crate::attack_ranges::deserialize")]
    pub range: i32,
    pub standoff_milli: i32,
    pub spread_milli: i32,
    pub work: u32,
    pub heat: u32,
    pub heat_radius: i32,
    pub fracture_j: u32,
    /// Cooldown in simulation ticks.
    pub reload_ticks: u32,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Tuning {
    #[serde(default)]
    pub kinetic: Kinetic,
    pub flame: Stream,
    pub machine_gun: Stream,
    pub laser: Stream,
    #[serde(default)]
    pub lightning: Option<Stream>,
    pub dust: Dust,
    pub smoke_engine_milli: i32,
    pub smoke_core_milli: i32,
    pub smoke_loss_milli: u32,
}
/// Bounded continuation storage and material contacts per retry phase.
#[derive(Clone, Debug, Deserialize)]
pub struct Kinetic {
    pub max_jobs: usize,
    pub cells_per_tick: usize,
}
impl Default for Kinetic {
    fn default() -> Self {
        Self {
            max_jobs: 128,
            cells_per_tick: 512,
        }
    }
}
/// Presentation-only track dust. The range simulation never reads it.
#[derive(Clone, Debug, Deserialize)]
pub struct Dust {
    pub interval_ticks: u32,
    pub min_speed: i32,
    pub full_speed: i32,
    pub min_turn_rate: i32,
    pub full_turn_rate: i32,
    pub max_particles: usize,
    pub edge_milli: u32,
    pub life_milli: u32,
    pub rise_milli: u32,
    pub trail_milli: u32,
    pub rear_milli: i32,
    pub lateral_milli: i32,
    pub floor_probe: i32,
}
impl Tuning {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(1..=128).contains(&t.kinetic.max_jobs)
            || !(1..=8192).contains(&t.kinetic.cells_per_tick)
        {
            return Err("kinetic continuation limits outside bounded limits".into());
        }
        for s in [&t.flame, &t.machine_gun, &t.laser].into_iter().chain(t.lightning.iter()) {
            if !(1..=300).contains(&s.burst_ticks)
                || !(1..=30).contains(&s.interval_ticks)
                || !(1..=3000).contains(&s.range)
                || !(100..=950).contains(&s.standoff_milli)
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
        {
            return Err("unit fire tuning outside bounded limits".into());
        }
        let d = &t.dust;
        if !(1..=30).contains(&d.interval_ticks)
            || !(0..=100).contains(&d.min_speed)
            || !(1..=100).contains(&d.full_speed)
            || d.full_speed <= d.min_speed
            || !(0..=200).contains(&d.min_turn_rate)
            || !(1..=200).contains(&d.full_turn_rate)
            || d.full_turn_rate <= d.min_turn_rate
            || !(1..=256).contains(&d.max_particles)
            || !(500..=10000).contains(&d.edge_milli)
            || !(100..=3000).contains(&d.life_milli)
            || !(0..=10000).contains(&d.rise_milli)
            || !(0..=20000).contains(&d.trail_milli)
            || !(0..=1000).contains(&d.rear_milli)
            || !(0..=500).contains(&d.lateral_milli)
            || !(1..=12).contains(&d.floor_probe)
        {
            return Err("range dust tuning outside bounded limits".into());
        }
        Ok(t)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kinetic_continuation_limits_are_bounded_and_optional_in_legacy_data() {
        let text = include_str!("../../../data/range-weapons.ron");
        let t = Tuning::from_ron(text).unwrap();
        assert_eq!(t.kinetic.max_jobs, 128);
        assert_eq!(t.kinetic.cells_per_tick, 512);
        let legacy = text
            .lines()
            .filter(|l| !l.contains("kinetic:"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(Tuning::from_ron(&legacy).unwrap().kinetic.max_jobs, 128);
        for (from, to) in [
            ("max_jobs: 128", "max_jobs: 0"),
            ("max_jobs: 128", "max_jobs: 129"),
            ("cells_per_tick: 512", "cells_per_tick: 0"),
            ("cells_per_tick: 512", "cells_per_tick: 8193"),
        ] {
            assert_ne!(text.replace(from, to), text);
            assert!(Tuning::from_ron(&text.replace(from, to)).is_err());
        }
    }
    #[test]
    fn stock_streams_and_invalid_budget() {
        let text = include_str!("../../../data/range-weapons.ron");
        let t = Tuning::from_ron(text).unwrap();
        assert!(t.machine_gun.range > t.flame.range);
        assert!(t.laser.range > t.flame.range);
        assert_eq!(t.laser.interval_ticks, 1);
        assert!((100..=950).contains(&t.flame.standoff_milli));
        assert!(Tuning::from_ron(&text.replace("interval_ticks: 2", "interval_ticks: 0")).is_err());
        assert!(
            Tuning::from_ron(&text.replace("max_particles: 128", "max_particles: 999")).is_err()
        );
        assert!(
            Tuning::from_ron(&text.replace("standoff_milli: 750", "standoff_milli: 0")).is_err()
        );
    }
}
