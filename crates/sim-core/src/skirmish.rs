//! Validation of authored scenario footprints before spawning any unit.
pub fn validate_layout(
    layout: &modding::skirmish::Skirmish,
    kinds: &modding::kinds::Kinds,
) -> Result<(), String> {
    let roster = voxel::roster::all();
    let mut boxes = Vec::new();
    for (i, u) in layout.units.iter().enumerate() {
        let k = kinds
            .index_of(u.kind)
            .ok_or_else(|| format!("unknown kind {}", u.kind))?;
        let name = &kinds.at(k).body;
        let body = &roster
            .iter()
            .find(|(n, _)| n == name)
            .ok_or_else(|| format!("unknown body {name}"))?
            .1;
        let half_x = (body.vol.nx as i32 * body.mm as i32 + 2 * voxel::unit::WORLD_MM as i32 - 1)
            / (2 * voxel::unit::WORLD_MM as i32);
        let half_z = (body.vol.nz as i32 * body.mm as i32 + 2 * voxel::unit::WORLD_MM as i32 - 1)
            / (2 * voxel::unit::WORLD_MM as i32);
        let bounds = [
            u.at[0] - half_x,
            u.at[1] - half_z,
            u.at[0] + half_x,
            u.at[1] + half_z,
        ];
        if bounds[0] < 0
            || bounds[1] < 0
            || bounds[2] > layout.size[0] as i32
            || bounds[3] > layout.size[2] as i32
        {
            return Err(format!("unit {i} footprint crosses the map boundary"));
        }
        for (j, b) in boxes.iter().enumerate() {
            let b: &[i32; 4] = b;
            if bounds[0] < b[2] && bounds[2] > b[0] && bounds[1] < b[3] && bounds[3] > b[1] {
                return Err(format!("unit footprints {j} and {i} intersect"));
            }
        }
        boxes.push(bounds);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn battlefield_has_a_kilometre_and_nonoverlapping_footprints() {
        let layout =
            modding::skirmish::Skirmish::from_ron(include_str!("../../../data/battlefield.ron"))
                .unwrap();
        let kinds = modding::kinds::from_ron(include_str!("../../../data/kinds.ron"), 30).unwrap();
        assert_eq!(layout.size[0] * voxel::unit::WORLD_MM as u32, 1_000_000);
        validate_layout(&layout, &kinds).unwrap();
        let mut bad = layout.clone();
        bad.units[1].at = bad.units[0].at;
        assert!(validate_layout(&bad, &kinds).is_err());
        bad = layout.clone();
        bad.units[0].kind = 65000;
        assert!(validate_layout(&bad, &kinds).is_err());
        bad = layout;
        bad.units[0].at = [0, 0];
        assert!(validate_layout(&bad, &kinds).is_err());
    }
}
