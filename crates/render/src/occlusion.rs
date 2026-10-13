//! Щелевое затенение по вокселям — постпроцесс по глубине и нормалям
//! (ADR-235).
//!
//! Щелевое затенение Bevy до воксельных щелей не достаёт: радиус
//! у него зашит в шейдере — семь сантиметров при ребре мира 10 см, —
//! а расширить его без shader defs нельзя, и они запрещены
//! (`CLAUDE.md`). Воронка, подножие обрыва и щель между машиной
//! и землёй лежат на масштабе в несколько вокселей; этот проход
//! берёт радиус из `look` и затемняет ровно их.
//!
//! Устройство то же, что у обводки (`render::edge`): полноэкранный
//! проход по образцу `bevy_core_pipeline::fullscreen_material`,
//! с глубиной и нормалями префасса в наборе привязок. Отличие одно:
//! матрицы вида приходят не из главного мира, а из `ExtractedView`,
//! поэтому юниформ собирается в рендер-мире своей системой.
//!
//! Идёт **до** тонмаппинга и до свечения: затенение — это свет,
//! а не краска, и умножать им надо линейный кадр.
//!
//! **Числа живут в `look`, а не здесь** (ADR-175).

use bevy::asset::Assets;
use bevy::core_pipeline::prepass::{DepthPrepass, NormalPrepass, ViewPrepassTextures};
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader};
use bevy::post_process::bloom::bloom;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, texture_depth_2d, uniform_buffer,
};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId,
    Canonical, ColorTargetState, ColorWrites, DynamicUniformBuffer, FragmentState, Operations,
    PipelineCache, RenderPassColorAttachment, RenderPassDescriptor, RenderPipeline,
    RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages,
    ShaderType, Specializer, SpecializerKey, TextureFormat, TextureSampleType, Variants,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

/// Настройки затенения на камере. Без компонента проход не выполняется.
#[derive(Component, Clone, Copy, ExtractComponent)]
#[require(DepthPrepass, NormalPrepass)]
pub struct Occlusion {
    /// Радиус полусферы выборок, мировые единицы (воксели мира).
    pub radius: f32,
    /// Сила затемнения, 0..1.
    pub strength: f32,
    /// Смещение по глубине против самозатенения, мировые единицы.
    pub bias: f32,
    /// Степень кривой: больше — тень собирается в самой щели.
    pub power: f32,
}

/// Юниформ прохода. Порядок полей — порядок в `occlusion.wgsl`;
/// матрицы первыми, потому что выравниваются на шестнадцать байт.
#[derive(ShaderType, Clone, Copy)]
struct OcclusionUniform {
    view_from_clip: Mat4,
    clip_from_view: Mat4,
    view_from_world: Mat4,
    radius: f32,
    strength: f32,
    bias: f32,
    power: f32,
}

/// Юниформы всех видов кадра одним буфером, по смещению на вид.
#[derive(Resource, Default)]
struct OcclusionUniforms(DynamicUniformBuffer<OcclusionUniform>);

/// Смещение вида в буфере юниформов.
#[derive(Component)]
struct OcclusionOffset(u32);

/// Проход затенения. Ставится окном рядом с остальными плагинами.
pub struct OcclusionPlugin;

/// Шейдер прохода. Встроен в двоичный файл: каталога ассетов
/// у окон проекта нет.
#[derive(Resource)]
struct OcclusionShader(Handle<Shader>);

impl Plugin for OcclusionPlugin {
    fn build(&self, app: &mut App) {
        let shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wgsl(
                include_str!("occlusion.wgsl"),
                "render/src/occlusion.wgsl",
            ));
        app.add_plugins(ExtractComponentPlugin::<Occlusion>::default());
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(OcclusionShader(shader))
            .init_resource::<OcclusionUniforms>()
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(
                Render,
                (
                    prepare_pipeline.in_set(RenderSystems::Prepare),
                    prepare_uniforms.in_set(RenderSystems::PrepareResources),
                ),
            )
            // **До свечения и тонмаппинга**: затенение умножает линейный
            // свет, а не готовую картинку.
            .add_systems(
                Core3d,
                draw_occlusion
                    .in_set(Core3dSystems::PostProcess)
                    .before(bloom),
            );
    }
}

#[derive(Resource)]
struct OcclusionPipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    variants: Variants<RenderPipeline, OcclusionSpecializer>,
}

struct OcclusionSpecializer;

/// Формат цели вида — единственное, чем отличаются варианты конвейера.
#[derive(PartialEq, Eq, Hash, Clone, Copy, SpecializerKey)]
struct OcclusionKey {
    target_format: TextureFormat,
}

impl Specializer<RenderPipeline> for OcclusionSpecializer {
    type Key = OcclusionKey;

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
    shader: Res<OcclusionShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "occlusion_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_depth_2d(),
                texture_2d(TextureSampleType::Float { filterable: true }),
                uniform_buffer::<OcclusionUniform>(true),
            ),
        ),
    );
    let desc = RenderPipelineDescriptor {
        label: Some("occlusion_pipeline".into()),
        layout: vec![layout.clone()],
        vertex: fullscreen_shader.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: shader.0.clone(),
            targets: vec![Some(ColorTargetState {
                format: TextureFormat::Rgba16Float,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        ..default()
    };
    commands.insert_resource(OcclusionPipeline {
        layout,
        sampler: render_device.create_sampler(&SamplerDescriptor::default()),
        variants: Variants::new(OcclusionSpecializer, desc),
    });
}

/// Номер собранного конвейера для вида.
#[derive(Component)]
struct OcclusionPipelineId(CachedRenderPipelineId);

fn prepare_pipeline(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    pipeline: Option<ResMut<OcclusionPipeline>>,
    views: Query<(Entity, &ExtractedView, Option<&Occlusion>), With<ExtractedCamera>>,
) -> Result<(), BevyError> {
    let Some(mut pipeline) = pipeline else {
        return Ok(());
    };
    for (entity, view, occlusion) in &views {
        if occlusion.is_none() {
            commands.entity(entity).remove::<OcclusionPipelineId>();
            continue;
        }
        let id = pipeline.variants.specialize(
            &pipeline_cache,
            OcclusionKey {
                target_format: view.target_format,
            },
        )?;
        commands.entity(entity).insert(OcclusionPipelineId(id));
    }
    Ok(())
}

/// Юниформ на вид: матрицы из `ExtractedView` и числа с камеры.
fn prepare_uniforms(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut uniforms: ResMut<OcclusionUniforms>,
    views: Query<(Entity, &ExtractedView, &Occlusion)>,
) {
    uniforms.0.clear();
    for (entity, view, o) in &views {
        let clip_from_view = view.clip_from_view;
        let u = OcclusionUniform {
            view_from_clip: clip_from_view.inverse(),
            clip_from_view,
            view_from_world: view.world_from_view.to_matrix().inverse(),
            radius: o.radius,
            strength: o.strength,
            bias: o.bias,
            power: o.power,
        };
        let offset = uniforms.0.push(&u);
        commands.entity(entity).insert(OcclusionOffset(offset));
    }
    uniforms.0.write_buffer(&device, &queue);
}

/// Сам проход. Набор привязок собирается здесь: `post_process_write`
/// меняет источник с приёмником местами, и раньше не узнать, какая
/// из двух текстур сейчас источник.
fn draw_occlusion(
    view: ViewQuery<(
        &ViewTarget,
        &ViewPrepassTextures,
        &OcclusionOffset,
        &OcclusionPipelineId,
    )>,
    pipeline: Option<Res<OcclusionPipeline>>,
    pipeline_cache: Res<PipelineCache>,
    uniforms: Res<OcclusionUniforms>,
    mut ctx: RenderContext,
) {
    let Some(pipeline) = pipeline else {
        return;
    };
    let (target, prepass, offset, id) = view.into_inner();
    let Some(render_pipeline) = pipeline_cache.get_render_pipeline(id.0) else {
        return;
    };
    let Some(binding) = uniforms.0.binding() else {
        return;
    };
    let (Some(depth), Some(normal)) = (prepass.depth_view(), prepass.normal_view()) else {
        return;
    };
    let depth = depth.clone();
    let normal = normal.clone();

    let post = target.post_process_write();
    let bind_group = ctx.render_device().create_bind_group(
        "occlusion_bind_group",
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
    let span = diagnostics.time_span(ctx.command_encoder(), "voxel-occlusion");
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("occlusion_pass"),
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
    pass.set_bind_group(0, &bind_group, &[offset.0]);
    pass.draw(0..3, 0..1);
    drop(pass);
    span.end(ctx.command_encoder());
}
