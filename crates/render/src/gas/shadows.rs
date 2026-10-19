//! Reuse Bevy's rendered mesh shadows on the raymarched terrain.
//! The pinned 0.19.1 light layout is mirrored in mesh_shadows.wgsl; array
//! lengths come from the same public constants as the host buffer.
use super::*;
use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder};

pub(super) fn source(text: &str) -> String {
    text.replace("$CASCADES", &bevy::pbr::MAX_CASCADES_PER_LIGHT.to_string())
        .replace(
            "$DIRECTIONAL",
            &bevy::pbr::MAX_DIRECTIONAL_LIGHTS.to_string(),
        )
        .replace("$RECT", &bevy::pbr::MAX_RECT_LIGHTS.to_string())
}

fn shadow_distance(far: f32) -> f32 {
    // Quantise the coverage, not the camera. Avoid changing cascade splits
    // for tiny camera movements; never round the visible far plane down.
    (far.max(650.0) / 64.0).ceil() * 64.0
}

pub(super) fn fit(
    cameras: Query<&Projection, With<GasCamera>>,
    mut lights: Query<&mut CascadeShadowConfig, With<DirectionalLight>>,
) {
    let far = cameras
        .iter()
        .filter_map(|p| match p {
            Projection::Orthographic(p) => Some(p.far),
            Projection::Perspective(p) => Some(p.far),
            Projection::Custom(_) => None,
        })
        .filter(|v| v.is_finite() && *v > 0.0)
        .reduce(f32::max);
    let Some(far) = far else { return };
    let maximum = shadow_distance(far);
    let count = crate::look::scene()
        .shadow_cascades
        .min(bevy::pbr::MAX_CASCADES_PER_LIGHT);
    for mut light in &mut lights {
        if light.bounds.len() == count && light.bounds.last() == Some(&maximum) {
            continue;
        }
        *light = CascadeShadowConfigBuilder {
            num_cascades: count,
            minimum_distance: 1.0,
            first_cascade_far_bound: maximum * crate::look::scene().shadow_first_frac,
            maximum_distance: maximum,
            overlap_proportion: 0.2,
        }
        .build();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cascades_cover_close_and_map_scale_far_planes_without_growing_maps() {
        for far in [100.0, 650.0, 1200.0, 6500.0, 24000.0] {
            let cover = shadow_distance(far);
            assert!(cover >= far && cover >= 650.0);
            assert!(cover - far.max(650.0) < 64.0);
        }
        assert_eq!(shadow_distance(1201.0), shadow_distance(1202.0));
    }
    #[test]
    fn light_uniform_matches_the_pinned_bevy_layout_and_public_array_sizes() {
        assert_eq!(bevy::pbr::GpuLights::min_size().get(), 4576);
        let text = source(include_str!("../mesh_shadows.wgsl"));
        assert!(!text.contains('$'));
        assert!(text.contains("array<MeshCascade, 4>"));
        assert!(text.contains("array<MeshDirectionalLight, 10>"));
        assert!(text.contains("array<MeshRectLight, 8>"));
    }
    #[test]
    fn live_zoom_updates_shadow_coverage_with_the_same_number_of_cascades() {
        let mut app = App::new();
        app.add_systems(Update, fit);
        let camera = app
            .world_mut()
            .spawn((
                GasCamera,
                Projection::Orthographic(OrthographicProjection {
                    far: 1200.0,
                    ..OrthographicProjection::default_3d()
                }),
            ))
            .id();
        let sun = app
            .world_mut()
            .spawn((
                DirectionalLight::default(),
                CascadeShadowConfigBuilder {
                    maximum_distance: 650.0,
                    ..default()
                }
                .build(),
            ))
            .id();
        for far in [1200.0, 16000.0, 1400.0] {
            if let Projection::Orthographic(p) =
                &mut *app.world_mut().get_mut::<Projection>(camera).unwrap()
            {
                p.far = far;
            }
            app.update();
            let light = app.world().get::<CascadeShadowConfig>(sun).unwrap();
            assert_eq!(light.bounds.len(), crate::look::scene().shadow_cascades);
            assert!(*light.bounds.last().unwrap() >= far);
        }
    }
}
