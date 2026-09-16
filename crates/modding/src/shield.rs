//! Energy shield tuning for the range stand (ADR-275). The balance lives
//! in `data/shield.ron`; the dome and its predicate are `sim_core::shield`.
use serde::Deserialize;

/// Dome radii the layout accepts, world units.
pub const SHIELD_RADIUS: std::ops::RangeInclusive<i32> = 8..=96;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct ShieldTuning {
    /// Energy a dome holds, in the wave units weapons declare as `energy`.
    pub capacity: u32,
    /// Energy restored per tick while the dome stands.
    pub recharge_per_tick: u32,
    /// Ticks a dome stays down after its charge is spent.
    pub overload_ticks: u32,
    /// Radius of a dome the editor places, world units.
    pub default_radius: i32,
}
impl ShieldTuning {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(1..=1_000_000).contains(&t.capacity)
            || t.recharge_per_tick > t.capacity
            || !(1..=3600).contains(&t.overload_ticks)
            || !SHIELD_RADIUS.contains(&t.default_radius)
        {
            return Err("invalid shield limits".into());
        }
        Ok(t)
    }
}
