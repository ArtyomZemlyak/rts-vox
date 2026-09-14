//! Bevy adapter for the established `look` palette and camera effects.
//! Voxel ray passes can participate by publishing depth and normal prepasses.
use crate::{look, occlusion::Occlusion};
use bevy::anti_alias::smaa::Smaa;
use bevy::camera::Exposure;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::atmosphere::ScatteringMedium;
use bevy::light::{Atmosphere, AtmosphereEnvironmentMapLight, CascadeShadowConfigBuilder, SunDisk};
use bevy::pbr::{
    AtmosphereSettings, ScreenSpaceAmbientOcclusion, ScreenSpaceAmbientOcclusionQualityLevel,
};
use bevy::post_process::bloom::Bloom;
use bevy::post_process::dof::{DepthOfField, DepthOfFieldMode};
use bevy::post_process::effect_stack::Vignette;
use bevy::prelude::*;
use bevy::render::view::{ColorGrading, ColorGradingGlobal, ColorGradingSection};

fn rgb(c: [f32; 3]) -> Color {
    Color::srgb(c[0], c[1], c[2])
}
fn section(s: look::Section) -> ColorGradingSection {
    ColorGradingSection {
        saturation: s.saturation,
        contrast: s.contrast,
        gamma: s.gamma,
        gain: s.gain,
        lift: s.lift,
    }
}
pub fn camera(commands: &mut Commands, entity: Entity, focus: f32) {
    let sc = look::scene();
    let g = sc.grade;
    let v = sc.vignette;
    let o = sc.occlusion;
    let tone = match sc.tonemap {
        look::Tonemap::Aces => Tonemapping::AcesFitted,
        look::Tonemap::AgX => Tonemapping::AgX,
        look::Tonemap::Tony => Tonemapping::TonyMcMapface,
        look::Tonemap::Filmic => Tonemapping::BlenderFilmic,
    };
    commands.entity(entity).insert((
        tone,
        Exposure { ev100: sc.ev100 },
        ColorGrading {
            global: ColorGradingGlobal {
                exposure: g.exposure,
                temperature: g.temperature,
                tint: g.tint,
                hue: 0.0,
                post_saturation: g.post_saturation,
                midtones_range: g.midtones_range.0..g.midtones_range.1,
            },
            shadows: section(g.shadows),
            midtones: section(g.midtones),
            highlights: section(g.highlights),
        },
        Bloom {
            intensity: sc.bloom,
            ..Bloom::NATURAL
        },
        ScreenSpaceAmbientOcclusion {
            quality_level: ScreenSpaceAmbientOcclusionQualityLevel::High,
            constant_object_thickness: sc.ssao_thickness,
        },
        Occlusion {
            radius: o.radius,
            strength: o.strength,
            bias: o.bias,
            power: o.power,
        },
        Smaa::default(),
        Vignette {
            intensity: v.intensity,
            radius: v.radius,
            smoothness: v.smoothness,
            roundness: v.roundness,
            color: rgb(v.srgb),
            ..default()
        },
        AtmosphereSettings {
            aerial_view_lut_max_distance: sc.haze_range_m,
            ..default()
        },
        AtmosphereEnvironmentMapLight {
            intensity: sc.sky_ibl,
            size: UVec2::splat(sc.sky_ibl_size),
            ..default()
        },
    ));
    if sc.dof_f_stops > 0.0 {
        commands.entity(entity).insert(DepthOfField {
            mode: DepthOfFieldMode::Gaussian,
            focal_distance: focus,
            aperture_f_stops: sc.dof_f_stops,
            max_depth: 1000.0,
            ..default()
        });
    }
}
pub fn daylight(commands: &mut Commands, mediums: &mut Assets<ScatteringMedium>, ground: f32) {
    daylight_scaled(
        commands,
        mediums,
        ground,
        look::scene().haze_metres_per_voxel,
        650.0,
    );
}

/// The game uses physical metres; compact showcase scenes keep their art scale.
pub fn daylight_scaled(
    commands: &mut Commands,
    mediums: &mut Assets<ScatteringMedium>,
    ground: f32,
    metres: f32,
    shadow_range: f32,
) {
    let sc = look::scene();
    let k = 1.0 / metres;
    let atmo = commands
        .spawn(Atmosphere::earth(
            mediums.add(ScatteringMedium::earth(256, 256)),
        ))
        .id();
    commands.entity(atmo).insert(GlobalTransform::from(
        Transform::from_scale(Vec3::splat(k)).with_translation(Vec3::new(
            0.0,
            ground - 6_360_000.0 * k,
            0.0,
        )),
    ));
    commands.spawn((
        DirectionalLight {
            color: rgb(sc.sun_srgb),
            illuminance: sc.sun_lux,
            shadow_maps_enabled: true,
            shadow_depth_bias: 0.06,
            shadow_normal_bias: 1.0,
            ..default()
        },
        SunDisk {
            angular_size: sc.sun_disk_size,
            intensity: sc.sun_disk_intensity,
        },
        Transform::default().looking_to(Vec3::from(look::sun_dir()), Vec3::Y),
        CascadeShadowConfigBuilder {
            num_cascades: 4,
            minimum_distance: 1.0,
            first_cascade_far_bound: 100.0,
            maximum_distance: shadow_range,
            overlap_proportion: 0.2,
        }
        .build(),
    ));
}
