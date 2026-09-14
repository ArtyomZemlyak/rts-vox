//! Angular actuator data, independent of weapon flight or visual frame rate.
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Turret {
    pub speed_deg_s: i32,
    pub accel_deg_s2: i32,
    pub aim_millidegrees: i32,
}
impl Turret {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(1..=360).contains(&t.speed_deg_s)
            || !(1..=3600).contains(&t.accel_deg_s2)
            || !(1..=10000).contains(&t.aim_millidegrees)
        {
            return Err("invalid turret actuator limits".into());
        }
        Ok(t)
    }
}
