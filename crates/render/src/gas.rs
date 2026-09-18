//! Bounded GPU atmosphere regions. Nearby compatible sources share a field;
//! distant impacts retain independent fields and clocks. No field readback.

mod regions;
pub use regions::Atmosphere;
use regions::REGIONS;

use bevy::core_pipeline::prepass::{
    DepthPrepass, NormalPrepass, ViewPrepassTextures, NORMAL_PREPASS_FORMAT,
};
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader};
use bevy::post_process::bloom::bloom;
use bevy::prelude::*;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_resource::binding_types::*;
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::view::{ExtractedView, ViewDepthTexture, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub const GAS_N: usize = 48;
pub const GAS_CELLS: usize = GAS_N * GAS_N * GAS_N;
/// The largest window of terrain the buffer on the card holds (ADR-318).
/// The volume follows the hull rather than covering the map, so this is
/// the size of the window, not of the world.
pub const TERRAIN_WORDS: usize = 640 * 192 * 640 / 8;

#[derive(Clone, Copy, ShaderType)]
pub struct GasParams {
    pub world_from_clip: Mat4,
    pub camera: Vec4,
    pub origin: Vec4,
    pub source: Vec4,
    pub control: Vec4,
    pub chemistry: Vec4,
    pub charge: Vec4,
    pub terrain: Vec4,
    /// Where the window of terrain sits in the world, `xyz` (ADR-318).
    /// The volume follows the hull instead of covering the map, and its
    /// storage wraps: world voxel `q` lives at `q % dims`.
    pub terrain_origin: Vec4,
    pub clip_from_world: Mat4,
    pub sun: Vec4,
    pub sky: Vec4,
    pub energy: Vec4,
    pub beam: Vec4,
    pub weapon: Vec4,
    /// Picture knobs of the overlay (ADR-298): `x` scales the residual
    /// glow of heated ground after a shell.
    pub look: Vec4,
}
impl Default for GasParams {
    fn default() -> Self {
        Self {
            world_from_clip: Mat4::IDENTITY,
            camera: Vec4::ZERO,
            origin: Vec4::new(0.0, 0.0, 0.0, 5.0),
            source: Vec4::ZERO,
            control: Vec4::new(1.0 / 60.0, 0.0, 0.0, 0.0),
            chemistry: Vec4::ZERO,
            charge: Vec4::new(0.0, 0.0, 2.0, 0.0),
            terrain: Vec4::ZERO,
            terrain_origin: Vec4::ZERO,
            clip_from_world: Mat4::IDENTITY,
            sun: Vec4::new(-0.55, 0.6, -0.55, 1.5),
            sky: Vec4::new(0.12, 0.16, 0.21, 0.0012),
            energy: Vec4::ZERO,
            beam: Vec4::ZERO,
            weapon: Vec4::ZERO,
            look: Vec4::new(1.0, 0.0, 0.0, 0.0),
        }
    }
}

/// The window of terrain the picture keeps in memory (ADR-318).
///
/// The ray march reaches about a thousand voxels, so a window of that
/// order shows everything the shader can see; a volume over a whole
/// range of a kilometre would be 9.6 GB. Storage **wraps**: world voxel
/// `q` lives at `q % dims`, so moving the window rewrites only the slabs
/// that came into view and leaves the rest of the volume where it is.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct TerrainWindow {
    pub origin: [i32; 3],
    pub dims: [u32; 3],
}
impl TerrainWindow {
    /// A window of at most `want` voxels a side, centred on `focus` and
    /// held inside the world. Its corner moves in steps of `step`, so a
    /// hull creeping about does not rewrite the volume every metre.
    pub fn around(world: [u32; 3], focus: [i32; 3], want: [u32; 3], step: i32) -> Self {
        let dims = [0, 1, 2].map(|a| world[a].min(want[a]));
        let origin = [0, 1, 2].map(|a| {
            let free = (world[a] - dims[a]) as i32;
            let wanted = focus[a] - dims[a] as i32 / 2;
            (wanted.div_euclid(step.max(1)) * step.max(1)).clamp(0, free)
        });
        Self { origin, dims }
    }
    pub fn holds(&self, q: [i32; 3]) -> bool {
        (0..3).all(|a| q[a] >= self.origin[a] && q[a] < self.origin[a] + self.dims[a] as i32)
    }
    /// Where this voxel lives in the volume, if the window holds it.
    pub fn index(&self, q: [i32; 3]) -> Option<usize> {
        self.holds(q).then(|| {
            let m = [0, 1, 2].map(|a| q[a] as u32 % self.dims[a]);
            (m[0] + self.dims[0] * (m[1] + self.dims[1] * m[2])) as usize
        })
    }
    pub fn words(&self) -> usize {
        (self.dims[0] as usize * self.dims[1] as usize * self.dims[2] as usize) / 8
    }
    /// The stretch of this axis the window holds.
    pub fn span(&self, a: usize) -> std::ops::Range<i32> {
        self.origin[a]..self.origin[a] + self.dims[a] as i32
    }
}

#[derive(Resource, Clone, ExtractResource)]
pub struct GasInput {
    pub params: GasParams,
    pub atmosphere: Atmosphere,
    pub terrain_words: Arc<Vec<u32>>,
    pub terrain_revision: u64,
    pub terrain_dirty: Vec<std::ops::Range<usize>>,
    /// Where the volume of terrain sits in the world (ADR-318).
    pub window: TerrainWindow,
    /// The scene can wait for actual shader readiness before advancing capture.
    pub ready: Arc<AtomicBool>,
}
impl Default for GasInput {
    fn default() -> Self {
        Self {
            params: GasParams::default(),
            atmosphere: Atmosphere::default(),
            terrain_words: Arc::new(vec![]),
            terrain_revision: 0,
            terrain_dirty: vec![],
            window: TerrainWindow::default(),
            ready: Arc::new(AtomicBool::new(false)),
        }
    }
}
impl GasInput {
    pub fn advance(&mut self, step: u32) {
        self.atmosphere.advance(
            step,
            &self.terrain_words,
            self.params.terrain.truncate().as_uvec3(),
            &self.terrain_dirty,
        );
    }
}

#[derive(Component, Clone, Copy, ExtractComponent)]
#[require(DepthPrepass, NormalPrepass)]
pub struct GasCamera;

pub struct GasPlugin;
#[derive(Resource)]
struct Shaders {
    compute: Handle<Shader>,
    draw: Handle<Shader>,
    terrain: Handle<Shader>,
}
impl Plugin for GasPlugin {
    fn build(&self, app: &mut App) {
        // Optional development source directory for visual iteration. Releases
        // use embedded shaders; no runtime files are needed to play a capture.
        let source = |name: &str, embedded: &str| -> String {
            if let Ok(dir) = std::env::var("RTS_VOX_SHADER_DIR") {
                std::fs::read_to_string(std::path::Path::new(&dir).join(name))
                    .expect("development shader")
            } else {
                embedded.to_owned()
            }
        };
        let common = source("terrain_common.wgsl", include_str!("terrain_common.wgsl"));
        let mut assets = app.world_mut().resource_mut::<Assets<Shader>>();
        let compute = assets.add(Shader::from_wgsl(
            source("gas.wgsl", include_str!("gas.wgsl")),
            "render/src/gas.wgsl",
        ));
        let draw = assets.add(Shader::from_wgsl(
            format!(
                "{}\n{}\n{}",
                common,
                source("energy.wgsl", include_str!("energy.wgsl")),
                source("gas_draw.wgsl", include_str!("gas_draw.wgsl"))
            ),
            "render/src/gas_draw.wgsl",
        ));
        let terrain = assets.add(Shader::from_wgsl(
            format!(
                "{}\n{}",
                common,
                source("terrain_prepass.wgsl", include_str!("terrain_prepass.wgsl"))
            ),
            "render/src/terrain_prepass.wgsl",
        ));
        app.init_resource::<GasInput>().add_plugins((
            ExtractResourcePlugin::<GasInput>::default(),
            ExtractComponentPlugin::<GasCamera>::default(),
        ));
        let Some(r) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        r.insert_resource(Shaders {
            compute,
            draw,
            terrain,
        })
        .add_systems(RenderStartup, init)
        .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups))
        .add_systems(
            Core3d,
            terrain_prepass
                .after(Core3dSystems::Prepass)
                .before(Core3dSystems::MainPass),
        )
        .add_systems(
            Core3d,
            draw_gas
                .after(Core3dSystems::MainPass)
                .before(Core3dSystems::EarlyPostProcess)
                .before(bloom)
                .before(bevy::core_pipeline::tonemapping::tonemapping),
        );
    }
}

#[derive(ShaderType)]
struct GpuCell {
    flow: Vec4,
    matter: Vec4,
}
#[derive(Resource)]
struct Pipeline {
    compute_layout: BindGroupLayoutDescriptor,
    draw_layout: BindGroupLayoutDescriptor,
    init: CachedComputePipelineId,
    step: CachedComputePipelineId,
    draws: Variants<RenderPipeline, GasSpecializer>,
    sampler: Sampler,
    uniform: UniformBuffer<GasParams>,
    empty: Buffer,
    regions: [Option<GpuRegion>; REGIONS],
    terrain: Buffer,
    terrain_revision: u64,
    ready_frames: u32,
    terrain_layout: BindGroupLayoutDescriptor,
    terrain_prepass: CachedRenderPipelineId,
}
struct GpuRegion {
    id: u64,
    buffers: [Buffer; 2],
    mask: Buffer,
    mask_revision: u64,
    uniform: UniformBuffer<GasParams>,
    groups: [BindGroup; 2],
    deposits: Vec<GpuDeposit>,
    read: usize,
    simulated: u32,
    applied: u64,
}
struct GpuDeposit {
    serial: u64,
    step: u32,
    _uniform: UniformBuffer<GasParams>,
    groups: [BindGroup; 2],
}
fn state_buffer(device: &RenderDevice) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some("gas state"),
        size: (GAS_CELLS * 32) as u64,
        usage: BufferUsages::STORAGE,
        mapped_at_creation: false,
    })
}
fn compute_groups(
    device: &RenderDevice,
    layout: &BindGroupLayout,
    uniform: &UniformBuffer<GasParams>,
    buffers: &[Buffer; 2],
    mask: &Buffer,
) -> [BindGroup; 2] {
    std::array::from_fn(|i| {
        device.create_bind_group(
            "gas state ping pong",
            layout,
            &BindGroupEntries::sequential((
                uniform.binding().unwrap(),
                buffers[i].as_entire_binding(),
                buffers[1 - i].as_entire_binding(),
                mask.as_entire_binding(),
            )),
        )
    })
}
struct GasSpecializer;
#[derive(Clone, Copy, PartialEq, Eq, Hash, SpecializerKey)]
struct Key {
    format: TextureFormat,
}
impl Specializer<RenderPipeline> for GasSpecializer {
    type Key = Key;
    fn specialize(
        &self,
        key: Key,
        d: &mut RenderPipelineDescriptor,
    ) -> Result<Canonical<Key>, BevyError> {
        d.fragment_mut()?.set_target(
            0,
            ColorTargetState {
                format: key.format,
                blend: None,
                write_mask: ColorWrites::ALL,
            },
        );
        Ok(key)
    }
}
#[derive(Component)]
struct DrawId(CachedRenderPipelineId);
fn init(
    mut commands: Commands,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    shaders: Res<Shaders>,
    fullscreen: Res<FullscreenShader>,
) {
    let compute_layout = BindGroupLayoutDescriptor::new(
        "gas compute",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<GasParams>(false),
                storage_buffer_read_only::<Vec<GpuCell>>(false),
                storage_buffer::<Vec<GpuCell>>(false),
                storage_buffer_read_only::<Vec<u32>>(false),
            ),
        ),
    );
    let init = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("gas injection".into()),
        layout: vec![compute_layout.clone()],
        shader: shaders.compute.clone(),
        entry_point: Some("init".into()),
        ..default()
    });
    let step = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("gas transport combustion".into()),
        layout: vec![compute_layout.clone()],
        shader: shaders.compute.clone(),
        entry_point: Some("step".into()),
        ..default()
    });
    let draw_layout = BindGroupLayoutDescriptor::new(
        "gas composite",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_depth_2d(),
                uniform_buffer::<GasParams>(false),
                storage_buffer_read_only::<Vec<GpuCell>>(false),
                storage_buffer_read_only::<Vec<u32>>(false),
                texture_2d(TextureSampleType::Float { filterable: true }),
            ),
        ),
    );
    let desc = RenderPipelineDescriptor {
        label: Some("gas display voxels".into()),
        layout: vec![draw_layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: shaders.draw.clone(),
            targets: vec![Some(ColorTargetState {
                format: TextureFormat::Rgba16Float,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        ..default()
    };
    let terrain_layout = BindGroupLayoutDescriptor::new(
        "terrain prepass",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            (
                (3, uniform_buffer::<GasParams>(false)),
                (5, storage_buffer_read_only::<Vec<u32>>(false)),
            ),
        ),
    );
    let terrain_prepass = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("voxel world depth and normals".into()),
        layout: vec![terrain_layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: shaders.terrain.clone(),
            targets: vec![Some(ColorTargetState {
                format: NORMAL_PREPASS_FORMAT,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        depth_stencil: Some(DepthStencilState {
            format: TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(CompareFunction::GreaterEqual),
            stencil: default(),
            bias: default(),
        }),
        ..default()
    });
    let terrain = device.create_buffer(&BufferDescriptor {
        label: Some("packed terrain materials"),
        size: (TERRAIN_WORDS * 4) as u64,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    commands.insert_resource(Pipeline {
        compute_layout,
        draw_layout,
        init,
        step,
        draws: Variants::new(GasSpecializer, desc),
        sampler: device.create_sampler(&SamplerDescriptor::default()),
        uniform: UniformBuffer::from(GasParams::default()),
        empty: state_buffer(&device),
        regions: std::array::from_fn(|_| None),
        terrain,
        terrain_revision: 0,
        ready_frames: 0,
        terrain_layout,
        terrain_prepass,
    });
}

fn prepare(
    mut commands: Commands,
    mut pipeline: ResMut<Pipeline>,
    input: Res<GasInput>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
    views: Query<(Entity, &ExtractedView), With<GasCamera>>,
) -> Result<(), BevyError> {
    let mut params = input.params;
    for (entity, view) in &views {
        params.world_from_clip = view.world_from_view.to_matrix() * view.clip_from_view.inverse();
        params.clip_from_world = params.world_from_clip.inverse();
        params.camera = view.world_from_view.translation().extend(1.0);
        let id = pipeline.draws.specialize(
            &cache,
            Key {
                format: view.target_format,
            },
        )?;
        commands.entity(entity).insert(DrawId(id));
    }
    params.control.z = 0.0;
    params.control.w = -1.0; // base pass, without raymarching a plume
    pipeline.uniform.set(params);
    pipeline.uniform.write_buffer(&device, &queue);
    if pipeline.terrain_revision != input.terrain_revision {
        assert!(input.terrain_words.len() <= TERRAIN_WORDS);
        // First upload is dense; later uploads cover only changed word spans.
        let spans = if pipeline.terrain_revision == 0 {
            std::iter::once(0..input.terrain_words.len()).collect()
        } else {
            input.terrain_dirty.clone()
        };
        for r in spans {
            let bytes: Vec<u8> = input.terrain_words[r.clone()]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect();
            queue.write_buffer(&pipeline.terrain, r.start as u64 * 4, &bytes);
        }
        pipeline.terrain_revision = input.terrain_revision;
    }
    let layout = cache.get_bind_group_layout(&pipeline.compute_layout);
    for (slot, source) in pipeline.regions.iter_mut().zip(&input.atmosphere.regions) {
        let Some(source) = source else {
            *slot = None;
            continue;
        };
        let mut p = source.params;
        p.world_from_clip = params.world_from_clip;
        p.clip_from_world = params.clip_from_world;
        p.camera = params.camera;
        p.sun = params.sun;
        p.sky = params.sky;
        p.terrain = params.terrain;
        p.weapon.y = params.weapon.y;
        p.weapon.w = params.weapon.w;
        p.look = params.look;
        p.control.y = input.atmosphere.clock.saturating_sub(source.last) as f32;
        p.control.z = source.opacity(input.atmosphere.clock);
        p.control.w = 1.0; // overlay: the terrain and beam have already been drawn
        if slot.as_ref().is_none_or(|r| r.id != source.id) {
            let buffers = std::array::from_fn(|_| state_buffer(&device));
            let mask = device.create_buffer(&BufferDescriptor {
                label: Some("gas solid geometry"),
                size: (GAS_CELLS * 4) as u64,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut uniform = UniformBuffer::from(p);
            uniform.write_buffer(&device, &queue);
            let groups = compute_groups(&device, &layout, &uniform, &buffers, &mask);
            *slot = Some(GpuRegion {
                id: source.id,
                buffers,
                mask,
                mask_revision: 0,
                uniform,
                groups,
                deposits: vec![],
                read: 0,
                simulated: 0,
                applied: 0,
            });
        }
        let r = slot.as_mut().unwrap();
        r.uniform.set(p);
        r.uniform.write_buffer(&device, &queue);
        if r.mask_revision != source.mask_revision {
            let bytes: Vec<u8> = source.mask.iter().flat_map(|v| v.to_le_bytes()).collect();
            queue.write_buffer(&r.mask, 0, &bytes);
            r.mask_revision = source.mask_revision;
        }
        r.deposits.clear();
        for deposit in source.deposits.iter().filter(|d| d.serial > r.applied) {
            let mut p = deposit.params;
            p.control.z = 1.0;
            p.control.w = if deposit.serial == source.id {
                0.0
            } else {
                1.0
            };
            let mut uniform = UniformBuffer::from(p);
            uniform.write_buffer(&device, &queue);
            let groups = compute_groups(&device, &layout, &uniform, &r.buffers, &r.mask);
            r.deposits.push(GpuDeposit {
                serial: deposit.serial,
                step: deposit.step,
                _uniform: uniform,
                groups,
            });
        }
    }
    Ok(())
}

fn terrain_prepass(
    view: ViewQuery<(&ViewDepthTexture, &ViewPrepassTextures), With<GasCamera>>,
    pipeline: Res<Pipeline>,
    cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (depth, prepass) = view.into_inner();
    let (Some(draw), Some(normals), Some(copy)) = (
        cache.get_render_pipeline(pipeline.terrain_prepass),
        prepass.normal.as_ref(),
        prepass.depth.as_ref(),
    ) else {
        return;
    };
    let group = ctx.render_device().create_bind_group(
        "voxel prepass",
        &cache.get_bind_group_layout(&pipeline.terrain_layout),
        &BindGroupEntries::with_indices((
            (3, pipeline.uniform.binding().unwrap()),
            (5, pipeline.terrain.as_entire_binding()),
        )),
    );
    {
        let mut pass = ctx
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("voxel depth and normals"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &normals.texture.default_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Load,
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                    view: depth.view(),
                    depth_ops: Some(Operations {
                        load: LoadOp::Load,
                        store: StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        pass.set_pipeline(draw);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
    ctx.command_encoder().copy_texture_to_texture(
        depth.texture.as_image_copy(),
        copy.texture.texture.as_image_copy(),
        prepass.size,
    );
}

fn draw_gas(
    view: ViewQuery<(&ViewTarget, &ViewPrepassTextures, &DrawId), With<GasCamera>>,
    mut pipeline: ResMut<Pipeline>,
    input: Res<GasInput>,
    cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (target, prepass, id) = view.into_inner();
    let (Some(draw), Some(init), Some(step), Some(depth), Some(normals)) = (
        cache.get_render_pipeline(id.0),
        cache.get_compute_pipeline(pipeline.init),
        cache.get_compute_pipeline(pipeline.step),
        prepass.depth_view(),
        prepass.normal_view(),
    ) else {
        return;
    };
    // Gas may compile before PBR/depth/bloom. Starting capture then would
    // record the gun appearing halfway through the shot. Wait for the whole
    // scene's pipeline queue to settle, including late material variants.
    if !input.ready.load(Ordering::Relaxed) {
        pipeline.ready_frames = if cache.waiting_pipelines().next().is_none() {
            pipeline.ready_frames + 1
        } else {
            0
        };
        if pipeline.ready_frames >= 12 {
            input.ready.store(true, Ordering::Relaxed);
        }
    }
    for (slot, source) in pipeline.regions.iter_mut().zip(&input.atmosphere.regions) {
        let (Some(r), Some(source)) = (slot, source) else {
            continue;
        };
        let mut pass = ctx
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("gas physics"),
                timestamp_writes: None,
            });
        let wanted = input.atmosphere.clock.saturating_sub(source.born);
        // At most 16 substeps per region per frame, matching the scene's eight
        // 30 Hz ticks. Pending deposits stay ordered until the GPU catches up.
        let end = wanted.min(r.simulated + 16);
        loop {
            for d in &r.deposits {
                if d.serial > r.applied && d.step <= r.simulated {
                    pass.set_pipeline(init);
                    pass.set_bind_group(0, &d.groups[r.read], &[]);
                    pass.dispatch_workgroups(12, 12, 12);
                    r.read = 1 - r.read;
                    r.applied = d.serial;
                }
            }
            if r.simulated >= end {
                break;
            }
            pass.set_pipeline(step);
            pass.set_bind_group(0, &r.groups[r.read], &[]);
            pass.dispatch_workgroups(12, 12, 12);
            r.read = 1 - r.read;
            r.simulated += 1;
        }
        source.applied.store(r.applied, Ordering::Relaxed);
    }
    // Shade terrain/charging effects exactly once. Overlay plumes cannot
    // overwrite an earlier plume's colour with a fresh terrain calculation.
    let mut order: Vec<_> = input
        .atmosphere
        .regions
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            r.as_ref().map(|r| {
                let centre = r.params.origin.truncate()
                    + Vec3::splat(GAS_N as f32 * r.params.origin.w * 0.5);
                let distance = centre.distance_squared(pipeline.uniform.get().camera.truncate());
                (i, distance)
            })
        })
        .collect();
    order.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    for slot in std::iter::once(None).chain(order.into_iter().map(|(i, _)| Some(i))) {
        let (uniform, buffer) = if let Some(i) = slot {
            let r = pipeline.regions[i].as_ref().unwrap();
            (&r.uniform, &r.buffers[r.read])
        } else {
            (&pipeline.uniform, &pipeline.empty)
        };
        let post = target.post_process_write();
        let group = ctx.render_device().create_bind_group(
            "gas draw",
            &cache.get_bind_group_layout(&pipeline.draw_layout),
            &BindGroupEntries::sequential((
                post.source,
                &pipeline.sampler,
                depth,
                uniform.binding().unwrap(),
                buffer.as_entire_binding(),
                pipeline.terrain.as_entire_binding(),
                normals,
            )),
        );
        let mut pass = ctx
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("voxel gas composite"),
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
        pass.set_pipeline(draw);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}
