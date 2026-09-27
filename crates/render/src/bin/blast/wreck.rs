//! Persistent wreck finish, derived from the saved authoritative alive flag.
use super::*;

pub fn materials(
    clean: &[Handle<StandardMaterial>; look::FINISHES],
    assets: &mut Assets<StandardMaterial>,
) -> [Handle<StandardMaterial>; look::FINISHES] {
    clean.clone().map(|handle| {
        let mut material = assets.get(&handle).unwrap().clone();
        material.base_color = Color::linear_rgb(0.45, 0.45, 0.45);
        material.metallic = 0.04;
        material.perceptual_roughness = 0.96;
        material.metallic_roughness_texture = None;
        material.emissive = LinearRgba::BLACK;
        material.emissive_texture = None;
        assets.add(material)
    })
}

/// Soot, ash and exposed heat-darkened edges retain the cubic form and maps.
/// No team paint or luminous sensor survives a destroyed hull's finish.
pub fn charred(buf: &mut MeshBuf) {
    for i in 0..buf.pos.len() {
        let p = Vec3::from(buf.pos[i]);
        let cell = p.floor();
        let grain = (cell.dot(Vec3::new(17.1, 7.7, 23.3)).sin() * 437.1)
            .fract()
            .abs();
        let patch = (p.dot(Vec3::new(0.17, 0.11, 0.23)).sin() * 0.5 + 0.5).powi(2);
        let n = Vec3::from(buf.nrm[i]).abs();
        let bevel = 1.0 - n.max_element();
        let c = 0.024 + patch * 0.065 + grain * 0.018 + bevel * 0.06;
        buf.col[i] = [c, c * 0.96, c * 0.90, 1.0];
    }
}
