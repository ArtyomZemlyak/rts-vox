//! Обводка силуэта и ступени яркости — постпроцесс по глубине
//! и нормалям (ADR-214).
//!
//! Направление «графика» держится на линии по силуэту, а линию честно
//! можно взять только из префасса: обводка по яркости кадра ставит её
//! на границе цвета, то есть по узору породы, а не по краю машины.
//!
//! Устройство скопировано с `bevy_core_pipeline::fullscreen_material`
//! (он сам — оформленный в плагин пример движка) и отличается ровно
//! одним: в набор привязок добавлены глубина и нормали префасса,
//! которых у `FullscreenMaterial` нет.
//!
//! **Числа живут в `look`, а не здесь** (ADR-175): этот модуль знает,
//! как нарисовать линию, и не знает, какой она должна быть.

use bevy::asset::Assets;
use bevy::core_pipeline::prepass::{DepthPrepass, NormalPrepass, ViewPrepassTextures};
use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader};
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_component::{
    ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
    UniformComponentPlugin,
};
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, texture_depth_2d, uniform_buffer,
};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId,
    Canonical, ColorTargetState, ColorWrites, FragmentState, Operations, PipelineCache,
    RenderPassColorAttachment, RenderPassDescriptor, RenderPipeline, RenderPipelineDescriptor,
    Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages, ShaderType, Specializer,
    SpecializerKey, TextureFormat, TextureSampleType, Variants,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

/// Настройки обводки на камере. Ставится только тем направлением,
/// которому она нужна: без компонента проход не выполняется вовсе.
///
/// Порядок полей — порядок в `edge.wgsl`, и цвет идёт первым не для
/// красоты: `vec4` выравнивается на шестнадцать байт, и поставленный
/// после шести `f32` он уехал бы на восемь байт вперёд относительно
/// наивного чтения.
#[derive(Component, Clone, Copy, ExtractComponent, ShaderType)]
#[require(DepthPrepass, NormalPrepass)]
pub struct Edges {
    /// Цвет линии, **линейный**.
    pub colour: Vec4,
    pub width: f32,
    pub depth: f32,
    pub normal: f32,
    pub strength: f32,
    pub steps: f32,
    pub posterise: f32,
    pub pad0: f32,
    pub pad1: f32,
}

/// Проход обводки. Ставится окном рядом с остальными плагинами.
pub struct EdgePlugin;

/// Шейдер прохода. Встроен в двоичный файл: каталога ассетов
/// у окон проекта нет, а заводить его ради одного файла значит
/// заводить его ради одного файла.
#[derive(Resource)]
struct EdgeShader(Handle<Shader>);

impl Plugin for EdgePlugin {
    fn build(&self, app: &mut App) {
        let shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wgsl(
                include_str!("edge.wgsl"),
                "render/src/edge.wgsl",
            ));
        app.add_plugins((
            ExtractComponentPlugin::<Edges>::default(),
            UniformComponentPlugin::<Edges>::default(),
        ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(EdgeShader(shader))
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare_pipeline.in_set(RenderSystems::Prepare))
            // **После тонмаппинга**: линия обязана быть той яркости,
            // которую задали, а не той, в которую её переведёт кривая.
            // Ступени яркости по той же причине: в линейном свете
            // ровные ступени ступенями не выглядят.
            .add_systems(
                Core3d,
                draw_edges
                    .in_set(Core3dSystems::PostProcess)
                    .after(tonemapping),
            );
    }
}

#[derive(Resource)]
struct EdgePipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    variants: Variants<RenderPipeline, EdgeSpecializer>,
}

struct EdgeSpecializer;

/// Формат цели вида — единственное, чем отличаются варианты конвейера.
#[derive(PartialEq, Eq, Hash, Clone, Copy, SpecializerKey)]
struct EdgeKey {
    target_format: TextureFormat,
}

impl Specializer<RenderPipeline> for EdgeSpecializer {
    type Key = EdgeKey;

    fn specialize(
        &self,
        key: Self::Key,
        descriptor: &mut RenderPipelineDescriptor,
    ) -> Result<Canonical<Self::Key>, BevyError> {
        descriptor.fragment_mut()?.set_target(
            0,
            ColorTargetState {
                format: key.target_format,
                blend: None,
                write_mask: ColorWrites::ALL,
            },
        );
        Ok(key)
    }
}

fn init_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    fullscreen_shader: Res<FullscreenShader>,
    shader: Res<EdgeShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "edge_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                // Глубина и нормали читаются `textureLoad`, поэтому
                // отдельного сэмплера им не нужно: выборка по целому
                // пикселю, а не интерполяция.
                texture_depth_2d(),
                texture_2d(TextureSampleType::Float { filterable: true }),
                uniform_buffer::<Edges>(true),
            ),
        ),
    );
    let desc = RenderPipelineDescriptor {
        label: Some("edge_pipeline".into()),
        layout: vec![layout.clone()],
        vertex: fullscreen_shader.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: shader.0.clone(),
            targets: vec![Some(ColorTargetState {
                format: TextureFormat::Rgba8UnormSrgb,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        ..default()
    };
    commands.insert_resource(EdgePipeline {
        layout,
        sampler: render_device.create_sampler(&SamplerDescriptor::default()),
        variants: Variants::new(EdgeSpecializer, desc),
    });
}

/// Номер собранного конвейера для вида.
#[derive(Component)]
struct EdgePipelineId(CachedRenderPipelineId);

fn prepare_pipeline(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    pipeline: Option<ResMut<EdgePipeline>>,
    views: Query<(Entity, &ExtractedView, Option<&Edges>), With<ExtractedCamera>>,
) -> Result<(), BevyError> {
    let Some(mut pipeline) = pipeline else {
        return Ok(());
    };
    for (entity, view, edges) in &views {
        if edges.is_none() {
            commands.entity(entity).remove::<EdgePipelineId>();
            continue;
        }
        let id = pipeline.variants.specialize(
            &pipeline_cache,
            EdgeKey {
                target_format: view.target_format,
            },
        )?;
        commands.entity(entity).insert(EdgePipelineId(id));
    }
    Ok(())
}

/// Сам проход.
///
/// Набор привязок собирается **здесь**, а не в фазе подготовки,
/// и это не лень: `post_process_write` меняет источник с приёмником
/// местами, и узнать, какая из двух текстур сейчас источник, можно
/// только внутри прохода.
fn draw_edges(
    view: ViewQuery<(
        &ViewTarget,
        &ViewPrepassTextures,
        &DynamicUniformIndex<Edges>,
        &EdgePipelineId,
    )>,
    pipeline: Option<Res<EdgePipeline>>,
    pipeline_cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<Edges>>,
    mut ctx: RenderContext,
) {
    let Some(pipeline) = pipeline else {
        return;
    };
    let (target, prepass, index, id) = view.into_inner();
    let Some(render_pipeline) = pipeline_cache.get_render_pipeline(id.0) else {
        return;
    };
    let Some(binding) = uniforms.uniforms().binding() else {
        return;
    };
    // Префасс есть не всегда: карта нормалей появляется вместе
    // с `NormalPrepass`, а до первого кадра её нет вовсе.
    let (Some(depth), Some(normal)) = (prepass.depth_view(), prepass.normal_view()) else {
        return;
    };
    let depth = depth.clone();
    let normal = normal.clone();

    let post = target.post_process_write();
    let bind_group = ctx.render_device().create_bind_group(
        "edge_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipeline.layout),
        &BindGroupEntries::sequential((
            post.source,
            &pipeline.sampler,
            &depth,
            &normal,
            binding.clone(),
        )),
    );

    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let span = diagnostics.time_span(ctx.command_encoder(), "voxel-edge");
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("edge_pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(render_pipeline);
    pass.set_bind_group(0, &bind_group, &[index.index()]);
    pass.draw(0..3, 0..1);
    drop(pass);
    span.end(ctx.command_encoder());
}
