use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Placement {
    pub kind: u16,
    pub owner: u8,
    pub at: [i32; 2],
    pub move_to: Option<[i32; 2]>,
    pub attack_move: Option<[i32; 2]>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Skirmish {
    pub size: [u32; 3],
    pub seed: u64,
    pub focus: [i32; 2],
    pub units: Vec<Placement>,
}

impl Skirmish {
    pub fn from_ron(s: &str) -> Result<Self, String> {
        let v: Self = ron::from_str(s).map_err(|e| e.to_string())?;
        let [x, y, z] = v.size;
        if !(128..=10000).contains(&x)
            || z != x
            || !(64..=512).contains(&y)
            || v.units.len() > 256
            || v.units.is_empty()
        {
            return Err(
                "scenario requires a square 128..10000 world, height 64..512 and 1..256 units"
                    .into(),
            );
        }
        let inside = |p: [i32; 2]| p[0] >= 0 && p[1] >= 0 && p[0] < x as i32 && p[1] < z as i32;
        if !inside(v.focus)
            || v.units.iter().any(|u| {
                u.owner > 1
                    || !inside(u.at)
                    || u.move_to.is_some_and(|p| !inside(p))
                    || u.attack_move.is_some_and(|p| !inside(p))
                    || (u.move_to.is_some() && u.attack_move.is_some())
            })
        {
            return Err("scenario position or owner out of bounds".into());
        }
        Ok(v)
    }
}
