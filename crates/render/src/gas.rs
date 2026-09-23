//! Bounded GPU atmosphere regions. Nearby compatible sources share a field;
//! distant impacts retain independent fields and clocks. No field readback.

mod bricks;
mod display;
mod regions;
pub use regions::Atmosphere;
use regions::REGIONS;

use bevy::core_pipeline::core_3d::main_opaque_pass_3d;
use bevy::core_pipeline::deferred::node::late_deferred_prepass;
use bevy::core_pipeline::prepass::{
    DepthPrepass, NormalPrepass, ViewPrepassTextures, NORMAL_PREPASS_FORMAT,
};
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader};
use bevy::post_process::bloom::bloom;
use bevy::prelude::*;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_resource::binding_types::*;
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::view::{ExtractedView, ViewDepthTexture, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

pub const GAS_N: usize = 48;
pub const GAS_CELLS: usize = GAS_N * GAS_N * GAS_N;
/// The largest window of terrain the buffer on the card holds (ADR-318).
/// The volume follows the hull rather than covering the map, so this is
/// the size of the window, not of the world.
pub const TERRAIN_WORDS: usize = 1280 * 192 * 1280 / 8;

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
    /// World position represented by a wrapped storage index.
    pub fn position(&self, index: usize) -> [i32; 3] {
        let d = self.dims.map(|v| v as usize);
        let m = [index % d[0], index / d[0] % d[1], index / d[0] / d[1]];
        [0, 1, 2].map(|a| {
            self.origin[a] + ((m[a] + d[a] - self.origin[a] as usize % d[a]) % d[a]) as i32
        })
    }
    /// The stretch of this axis the window holds.
    pub fn span(&self, a: usize) -> std::ops::Range<i32> {
        self.origin[a]..self.origin[a] + self.dims[a] as i32
    }
}

#[derive(Resource, Clone)]
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
    pub timings: Arc<GasTimings>,
    uploaded: Arc<AtomicU64>,
    patches: Vec<std::ops::Range<usize>>,
    patch_words: Vec<u32>,
}
/// Optional extraction and render preparation CPU counters for the native profiler.
/// These are not GPU timestamps and never enter the simulation.
#[derive(Default)]
pub struct GasTimings {
    pub enabled: AtomicBool,
    pub extract_ns: AtomicU64,
    pub extract_peak_ns: AtomicU64,
    pub prepare_ns: AtomicU64,
    pub allocation_ns: AtomicU64,
    pub prepare_peak_ns: AtomicU64,
    pub allocation_peak_ns: AtomicU64,
    pub active_regions: AtomicU64,
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
            timings: Arc::new(GasTimings::default()),
            uploaded: Arc::new(AtomicU64::new(0)),
            patches: Vec::new(),
            patch_words: Vec::new(),
        }
    }
}
impl ExtractResource for GasInput {
    type Source = Self;
    fn extract_resource(source: &Self) -> Self {
        let started = source
            .timings
            .enabled
            .load(Ordering::Relaxed)
            .then(std::time::Instant::now);
        let uploaded = source.uploaded.load(Ordering::Acquire);
        let mut spans = if uploaded == source.terrain_revision {
            Vec::new()
        } else if uploaded == 0 {
            std::iter::once(0..source.terrain_words.len()).collect()
        } else {
            source.terrain_dirty.clone()
        };
        spans.sort_unstable_by_key(|r| r.start);
        let mut used = 0;
        for i in 0..spans.len() {
            let r = spans[i].clone();
            if used > 0 && r.start <= spans[used - 1].end {
                spans[used - 1].end = spans[used - 1].end.max(r.end);
            } else {
                spans[used] = r;
                used += 1;
            }
        }
        spans.truncate(used);
        let merged = spans;
        let mut patch_words = Vec::with_capacity(merged.iter().map(|r| r.len()).sum());
        for r in &merged {
            patch_words.extend_from_slice(&source.terrain_words[r.clone()]);
        }
        let result = Self {
            params: source.params,
            atmosphere: source.atmosphere.clone(),
            // Never share the full mutable terrain with the render world:
            // doing so makes Arc::make_mut copy 157 MB on each moving tick.
            terrain_words: Arc::new(Vec::new()),
            // One contiguous payload, even for 245760 disjoint rows of a
            // streamed x slab. A Vec allocation per row stalled extraction.
            patches: merged,
            patch_words,
            terrain_revision: source.terrain_revision,
            terrain_dirty: Vec::new(),
            window: source.window,
            ready: source.ready.clone(),
            timings: source.timings.clone(),
            uploaded: source.uploaded.clone(),
        };
        if let Some(started) = started {
            let ns = started.elapsed().as_nanos() as u64;
            source.timings.extract_ns.store(ns, Ordering::Relaxed);
            source
                .timings
                .extract_peak_ns
                .fetch_max(ns, Ordering::Relaxed);
        }
        result
    }
}
impl GasInput {
    /// Preserve patches until the renderer acknowledges them, including frames
    /// that only pan a paused camera and never execute a simulation tick.
    pub fn begin_frame(&mut self) {
        if self.uploaded.load(Ordering::Acquire) == self.terrain_revision {
            self.terrain_dirty.clear();
        }
    }
    fn patches(&self) -> impl Iterator<Item = (usize, &[u32])> {
        self.patches.iter().scan(0, |cursor, span| {
            let end = *cursor + span.len();
            let words = &self.patch_words[*cursor..end];
            *cursor = end;
            Some((span.start, words))
        })
    }
    /// Presentation only; existing plumes keep their clocks, fields and scale
    /// relative to the weapon (small burning surfaces use half-size cubes).
    pub fn set_display_edge(&mut self, edge: f32) {
        self.atmosphere
            .resize_display(self.params.charge.z.max(0.01), edge);
        self.params.charge.z = edge;
    }
    /// Move a surface emitter toward the incident air, keeping the whole gas
    /// cell clear even when it joins a region with a different grid origin.
    pub fn surface_source(&self, point: Vec3, outward: Vec3) -> Vec3 {
        regions::surface_source(
            &self.terrain_words,
            self.window,
            point,
            outward,
            self.params.origin.w as i32,
        )
    }
    pub fn advance(&mut self, step: u32) {
        self.atmosphere
            .advance_gpu(step, &self.terrain_words, self.window, &self.terrain_dirty);
    }
}

#[derive(Component, Clone, Copy, ExtractComponent)]
#[require(DepthPrepass, NormalPrepass)]
pub struct GasCamera;

pub struct GasPlugin;

#[cfg(test)]
mod window_tests {
    use super::*;
    #[test]
    fn paused_frames_retain_unacknowledged_terrain_edits() {
        let mut source = GasInput {
            terrain_revision: 4,
            terrain_dirty: std::iter::once(2..5).collect(),
            ..default()
        };
        source.uploaded.store(3, Ordering::Release);
        source.begin_frame();
        assert_eq!(source.terrain_dirty.len(), 1);
        assert_eq!(source.terrain_dirty[0], 2..5);
        source.uploaded.store(4, Ordering::Release);
        source.begin_frame();
        assert!(source.terrain_dirty.is_empty());
    }
    #[test]
    #[ignore = "manual CPU benchmark of one streamed x slab, includes resource drop"]
    fn bench_sparse_slab_extraction() {
        let window = TerrainWindow {
            origin: [8, 0, 0],
            dims: [1280, 192, 1280],
        };
        let source = GasInput {
            window,
            terrain_words: Arc::new(vec![0x77777777; window.words()]),
            terrain_revision: 2,
            terrain_dirty: (0..192 * 1280)
                .map(|row| row * 160..row * 160 + 1)
                .collect(),
            ..default()
        };
        source.uploaded.store(1, Ordering::Release);
        for trial in 0..5 {
            let start = std::time::Instant::now();
            let extracted = GasInput::extract_resource(&source);
            std::hint::black_box(&extracted);
            drop(extracted);
            println!(
                "SLAB trial={trial} ms={:.3}",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
    #[test]
    fn extraction_copies_only_unacknowledged_patches_without_sharing_the_world() {
        let mut source = GasInput {
            terrain_words: Arc::new(vec![7; 1000]),
            terrain_revision: 1,
            ..default()
        };
        let first = GasInput::extract_resource(&source);
        assert_eq!(Arc::strong_count(&source.terrain_words), 1);
        assert!(first.terrain_words.is_empty());
        assert_eq!(first.patches, std::iter::once(0..1000).collect::<Vec<_>>());
        assert_eq!(first.patch_words, vec![7; 1000]);
        first.uploaded.store(1, Ordering::Release);
        assert!(GasInput::extract_resource(&source).patches.is_empty());
        Arc::make_mut(&mut source.terrain_words)[13] = 9;
        source.terrain_revision = 3; // Two simulation ticks in one frame.
        source.terrain_dirty = vec![12..15, 10..14, 99..100];
        let next = GasInput::extract_resource(&source);
        assert_eq!(next.patches, vec![10..15, 99..100]);
        assert_eq!(next.patch_words, vec![7, 7, 7, 9, 7, 7]);
        assert_eq!(next.patches().nth(1), Some((99, &[7][..])));
        assert_eq!(first.patch_words[13], 7);
        assert_eq!(Arc::strong_count(&source.terrain_words), 1);
    }
    #[test]
    fn packed_patches_preserve_addresses_across_gaps_overlaps_and_unequal_rows() {
        let source = GasInput {
            terrain_words: Arc::new((0..1000).map(|i| i * 713).collect()),
            terrain_revision: 2,
            terrain_dirty: vec![991..1000, 700..703, 8..13, 13..40, 25..30, 6..8, 1..2],
            ..default()
        };
        source.uploaded.store(1, Ordering::Release);
        let extracted = GasInput::extract_resource(&source);
        assert_eq!(extracted.patches, vec![1..2, 6..40, 700..703, 991..1000]);
        assert_eq!(extracted.patch_words.len(), 47);
        for (at, words) in extracted.patches() {
            for (i, word) in words.iter().enumerate() {
                assert_eq!(*word, (at + i) as u32 * 713);
            }
        }
    }
    #[test]
    fn wrapped_storage_recovers_world_positions_after_window_moves() {
        for origin in [[0, 0, 0], [64, 0, 128], [8768, 0, 8640]] {
            let w = TerrainWindow {
                origin,
                dims: [1280, 192, 1280],
            };
            for x in [0, 1, 639, 640, 1279] {
                for z in [0, 127, 640, 1279] {
                    for y in [0, 63, 191] {
                        let q = [origin[0] + x, y, origin[2] + z];
                        assert_eq!(w.position(w.index(q).unwrap()), q);
                    }
                }
            }
        }
    }
}
#[derive(Resource)]
struct Shaders {
    bricks: Handle<Shader>,
    display: Handle<Shader>,
    mask: Handle<Shader>,
    upload: Handle<Shader>,
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
        let mask = assets.add(Shader::from_wgsl(
            include_str!("gas_mask.wgsl"),
            "render/src/gas_mask.wgsl",
        ));
        let bricks = assets.add(Shader::from_wgsl(
            source("terrain_bricks.wgsl", include_str!("terrain_bricks.wgsl")),
            "render/src/terrain_bricks.wgsl",
        ));
        let upload = assets.add(Shader::from_wgsl(
            include_str!("terrain_upload.wgsl"),
            "render/src/terrain_upload.wgsl",
        ));
        let compute = assets.add(Shader::from_wgsl(
            source("gas.wgsl", include_str!("gas.wgsl")),
            "render/src/gas.wgsl",
        ));
        let sampling = source("gas_sampling.wgsl", include_str!("gas_sampling.wgsl"));
        let display = assets.add(Shader::from_wgsl(
            format!(
                "{common}\n{sampling}\n{}",
                source("gas_display.wgsl", include_str!("gas_display.wgsl"))
            ),
            "render/src/gas_display.wgsl",
        ));
        let draw = assets.add(Shader::from_wgsl(
            format!(
                "{}\n{}\n{}\n{}",
                common,
                sampling,
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
            bricks,
            display,
            mask,
            upload,
            compute,
            draw,
            terrain,
        })
        .add_systems(RenderStartup, init)
        .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups))
        .add_systems(
            Core3d,
            terrain_prepass
                .in_set(Core3dSystems::Prepass)
                .after(late_deferred_prepass),
        )
        .add_systems(
            Core3d,
            shade_terrain
                .in_set(Core3dSystems::MainPass)
                .before(main_opaque_pass_3d),
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
    mask_layout: BindGroupLayoutDescriptor,
    mask_build: CachedComputePipelineId,
    upload_layout: BindGroupLayoutDescriptor,
    upload: CachedComputePipelineId,
    compute_layout: BindGroupLayoutDescriptor,
    draw_layout: BindGroupLayoutDescriptor,
    colour_layout: BindGroupLayoutDescriptor,
    init: CachedComputePipelineId,
    step: CachedComputePipelineId,
    draws: Variants<RenderPipeline, GasSpecializer>,
    colours: Variants<RenderPipeline, GasSpecializer>,
    sampler: Sampler,
    uniform: UniformBuffer<GasParams>,
    empty: Buffer,
    regions: [Option<GpuRegion>; REGIONS],
    terrain: Buffer,
    terrain_revision: u64,
    bricks: bricks::Bricks,
    display: display::DisplayPipeline,
    ready_frames: u32,
    terrain_layout: BindGroupLayoutDescriptor,
    terrain_prepass: CachedRenderPipelineId,
}
struct GpuRegion {
    id: u64,
    display: Option<display::Display>,
    buffers: [Buffer; 2],
    mask: Buffer,
    mask_revision: u64,
    mask_dirty: bool,
    mask_uniform: UniformBuffer<MaskParams>,
    mask_group: BindGroup,
    uniform: UniformBuffer<GasParams>,
    groups: [BindGroup; 2],
    deposits: Vec<GpuDeposit>,
    read: usize,
    simulated: u32,
    applied: u64,
}
#[derive(Clone, Copy, ShaderType)]
struct MaskParams {
    origin: IVec4,
    window: IVec4,
    dims: UVec4,
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
struct DrawId {
    gas: CachedRenderPipelineId,
    terrain: CachedRenderPipelineId,
}
fn init(
    mut commands: Commands,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    shaders: Res<Shaders>,
    fullscreen: Res<FullscreenShader>,
) {
    let mask_layout = BindGroupLayoutDescriptor::new(
        "gas occupancy build",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<MaskParams>(false),
                storage_buffer_read_only::<Vec<u32>>(false),
                storage_buffer::<Vec<u32>>(false),
            ),
        ),
    );
    let mask_build = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("gas occupancy build".into()),
        layout: vec![mask_layout.clone()],
        shader: shaders.mask.clone(),
        entry_point: Some("build".into()),
        ..default()
    });
    let upload_layout = BindGroupLayoutDescriptor::new(
        "terrain sparse upload",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                storage_buffer_read_only::<Vec<u32>>(false),
                storage_buffer_read_only::<Vec<u32>>(false),
                storage_buffer::<Vec<u32>>(false),
            ),
        ),
    );
    let upload = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("terrain sparse upload".into()),
        layout: vec![upload_layout.clone()],
        shader: shaders.upload.clone(),
        entry_point: Some("apply".into()),
        ..default()
    });
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
                storage_buffer_read_only::<Vec<u32>>(false),
                storage_buffer_read_only::<Vec<GpuCell>>(false),
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
    // Terrain is an opaque surface. It writes directly into the main colour
    // attachment, without sampling or switching the postprocess targets.
    let colour_layout = BindGroupLayoutDescriptor::new(
        "terrain colour",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            (
                (2, texture_depth_2d()),
                (3, uniform_buffer::<GasParams>(false)),
                (5, storage_buffer_read_only::<Vec<u32>>(false)),
                (6, texture_2d(TextureSampleType::Float { filterable: true })),
                (7, storage_buffer_read_only::<Vec<u32>>(false)),
            ),
        ),
    );
    let mut colour_desc = desc.clone();
    colour_desc.label = Some("voxel world colour".into());
    colour_desc.layout = vec![colour_layout.clone()];
    colour_desc.fragment.as_mut().unwrap().entry_point = Some("terrain_fragment".into());
    // The shared shader has two fragment entry points.
    let mut desc = desc;
    desc.fragment.as_mut().unwrap().entry_point = Some("fragment".into());
    let terrain_layout = BindGroupLayoutDescriptor::new(
        "terrain prepass",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            (
                (3, uniform_buffer::<GasParams>(false)),
                (5, storage_buffer_read_only::<Vec<u32>>(false)),
                (7, storage_buffer_read_only::<Vec<u32>>(false)),
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
        mask_layout,
        mask_build,
        upload_layout,
        upload,
        compute_layout,
        draw_layout,
        colour_layout,
        init,
        step,
        draws: Variants::new(GasSpecializer, desc),
        colours: Variants::new(GasSpecializer, colour_desc),
        sampler: device.create_sampler(&SamplerDescriptor::default()),
        uniform: UniformBuffer::from(GasParams::default()),
        empty: state_buffer(&device),
        regions: std::array::from_fn(|_| None),
        bricks: bricks::Bricks::new(&device, &cache, shaders.bricks.clone()),
        display: display::DisplayPipeline::new(&cache, shaders.display.clone()),
        terrain,
        terrain_revision: 0,
        ready_frames: 0,
        terrain_layout,
        terrain_prepass,
    });
}

fn le_bytes(words: &[u32]) -> std::borrow::Cow<'_, [u8]> {
    #[cfg(target_endian = "little")]
    {
        std::borrow::Cow::Borrowed(bytemuck::cast_slice(words))
    }
    #[cfg(target_endian = "big")]
    {
        std::borrow::Cow::Owned(words.iter().flat_map(|v| v.to_le_bytes()).collect())
    }
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
    let started = input
        .timings
        .enabled
        .load(Ordering::Relaxed)
        .then(std::time::Instant::now);
    let mut allocated_ns = 0;
    let mut params = input.params;
    for (entity, view) in &views {
        params.world_from_clip = view.world_from_view.to_matrix() * view.clip_from_view.inverse();
        params.clip_from_world = params.world_from_clip.inverse();
        params.camera = view.world_from_view.translation().extend(1.0);
        let key = Key {
            format: view.target_format,
        };
        let gas = pipeline.draws.specialize(&cache, key)?;
        let terrain = pipeline.colours.specialize(&cache, key)?;
        commands.entity(entity).insert(DrawId { gas, terrain });
    }
    params.control.z = 0.0;
    params.control.w = -1.0; // base pass, without raymarching a plume
    pipeline.uniform.set(params);
    pipeline.uniform.write_buffer(&device, &queue);
    if pipeline.terrain_revision != input.terrain_revision {
        assert!(input.window.words() <= TERRAIN_WORDS);
        let count = input.patch_words.len();
        // Bound per-workgroup serial writes when a few ranges are much longer
        // than the usual short rows of a streamed window.
        const WORDS_PER_UPLOAD_SPAN: usize = 512;
        let descriptors = input
            .patches
            .iter()
            .map(|r| r.len().div_ceil(WORDS_PER_UPLOAD_SPAN))
            .sum::<usize>();
        if let Some(compute) = cache.get_compute_pipeline(pipeline.upload).filter(|_| {
            input.patches.len() > 128
                && count > 0
                && count * 4 <= device.limits().max_storage_buffer_binding_size as usize
                && descriptors * 12 <= device.limits().max_storage_buffer_binding_size as usize
        }) {
            let mut spans = Vec::with_capacity(descriptors * 3);
            let mut offset = 0u32;
            for (start, words) in input.patches() {
                for chunk in (0..words.len()).step_by(WORDS_PER_UPLOAD_SPAN) {
                    spans.extend_from_slice(&[
                        (start + chunk) as u32,
                        offset + chunk as u32,
                        (words.len() - chunk).min(WORDS_PER_UPLOAD_SPAN) as u32,
                    ]);
                }
                offset += words.len() as u32;
            }
            debug_assert_eq!(offset as usize, count);
            debug_assert_eq!(spans.len() / 3, descriptors);
            let descriptor_bytes = le_bytes(&spans);
            let value_bytes = le_bytes(&input.patch_words);
            let spans = device.create_buffer(&BufferDescriptor {
                label: Some("terrain patch spans"),
                size: descriptor_bytes.len() as u64,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let values = device.create_buffer(&BufferDescriptor {
                label: Some("terrain patch values"),
                size: value_bytes.len() as u64,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&spans, 0, descriptor_bytes.as_ref());
            queue.write_buffer(&values, 0, value_bytes.as_ref());
            let group = device.create_bind_group(
                "terrain sparse upload",
                &cache.get_bind_group_layout(&pipeline.upload_layout),
                &BindGroupEntries::sequential((
                    spans.as_entire_binding(),
                    values.as_entire_binding(),
                    pipeline.terrain.as_entire_binding(),
                )),
            );
            let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
                label: Some("terrain sparse upload"),
            });
            {
                let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                    label: Some("terrain sparse upload"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(compute);
                pass.set_bind_group(0, &group, &[]);
                let groups = (descriptors as u32).div_ceil(4);
                pass.dispatch_workgroups(groups.min(65535), groups.div_ceil(65535), 1);
            }
            // Submit before the frame's passes, making the new terrain visible
            // to depth, shadows and gas in this same frame.
            queue.submit([encoder.finish()]);
        } else {
            for (start, words) in input.patches() {
                let bytes = le_bytes(words);
                queue.write_buffer(&pipeline.terrain, start as u64 * 4, bytes.as_ref());
            }
        }
        pipeline.terrain_revision = input.terrain_revision;
        input
            .uploaded
            .store(input.terrain_revision, Ordering::Release);
    }
    let after_upload = started.map_or(0, |t| t.elapsed().as_nanos() as u64);
    let terrain_buffer = pipeline.terrain.clone();
    pipeline.bricks.prepare(
        input.window,
        &input.patches,
        &terrain_buffer,
        &device,
        &queue,
        &cache,
    );
    let after_bricks = started.map_or(0, |t| t.elapsed().as_nanos() as u64);
    let layout = cache.get_bind_group_layout(&pipeline.compute_layout);
    let mask_layout = cache.get_bind_group_layout(&pipeline.mask_layout);
    let Pipeline {
        regions, display, ..
    } = &mut *pipeline;
    for (slot, source) in regions.iter_mut().zip(&input.atmosphere.regions) {
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
        p.terrain_origin = params.terrain_origin;
        p.weapon.y = params.weapon.y;
        p.weapon.w = params.weapon.w;
        p.look = params.look;
        p.control.y = input.atmosphere.clock.saturating_sub(source.last) as f32;
        p.control.z = source.opacity(input.atmosphere.clock);
        p.control.w = 1.0; // overlay: the terrain and beam have already been drawn
        if slot.as_ref().is_none_or(|r| r.id != source.id) {
            let allocation = started.map(|_| std::time::Instant::now());
            let buffers = std::array::from_fn(|_| state_buffer(&device));
            let mask = device.create_buffer(&BufferDescriptor {
                label: Some("gas solid geometry"),
                size: (GAS_CELLS * 4) as u64,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut mask_uniform = UniformBuffer::from(MaskParams {
                origin: p.origin.as_ivec4(),
                window: p.terrain_origin.as_ivec4(),
                dims: p.terrain.as_uvec4(),
            });
            mask_uniform.write_buffer(&device, &queue);
            let mask_group = device.create_bind_group(
                "gas occupancy build",
                &mask_layout,
                &BindGroupEntries::sequential((
                    mask_uniform.binding().unwrap(),
                    terrain_buffer.as_entire_binding(),
                    mask.as_entire_binding(),
                )),
            );
            let mut uniform = UniformBuffer::from(p);
            uniform.write_buffer(&device, &queue);
            let groups = compute_groups(&device, &layout, &uniform, &buffers, &mask);
            *slot = Some(GpuRegion {
                id: source.id,
                display: None,
                buffers,
                mask,
                mask_revision: 0,
                mask_dirty: false,
                mask_uniform,
                mask_group,
                uniform,
                groups,
                deposits: vec![],
                read: 0,
                simulated: 0,
                applied: 0,
            });
            if let Some(allocation) = allocation {
                allocated_ns += allocation.elapsed().as_nanos() as u64;
            }
        }
        let r = slot.as_mut().unwrap();
        r.uniform.set(p);
        r.uniform.write_buffer(&device, &queue);
        display::Display::prepare(
            &mut r.display,
            &p,
            &r.uniform,
            &r.buffers,
            display,
            &device,
            &cache,
        );
        if r.mask_revision != source.mask_revision {
            if source.mask.is_empty() {
                r.mask_uniform.set(MaskParams {
                    origin: p.origin.as_ivec4(),
                    window: p.terrain_origin.as_ivec4(),
                    dims: p.terrain.as_uvec4(),
                });
                r.mask_uniform.write_buffer(&device, &queue);
                r.mask_dirty = true;
            } else {
                let bytes: Vec<u8> = source.mask.iter().flat_map(|v| v.to_le_bytes()).collect();
                queue.write_buffer(&r.mask, 0, &bytes);
            }
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
    if let Some(started) = started {
        let elapsed = started.elapsed().as_nanos() as u64;
        if elapsed >= 4_000_000 {
            eprintln!(
                "PROFILE_RENDER_PREPARE total_ms={:.3} upload_ms={:.3} bricks_ms={:.3} regions_ms={:.3} patches={} words={} regions={}",
                elapsed as f64 / 1_000_000.0,
                after_upload as f64 / 1_000_000.0,
                after_bricks.saturating_sub(after_upload) as f64 / 1_000_000.0,
                elapsed.saturating_sub(after_bricks) as f64 / 1_000_000.0,
                input.patches.len(),
                input.patch_words.len(),
                input.atmosphere.regions.iter().flatten().count()
            );
        }
        input
            .timings
            .prepare_peak_ns
            .fetch_max(elapsed, Ordering::Relaxed);
        input
            .timings
            .allocation_peak_ns
            .fetch_max(allocated_ns, Ordering::Relaxed);
        input.timings.prepare_ns.store(elapsed, Ordering::Relaxed);
        input
            .timings
            .allocation_ns
            .store(allocated_ns, Ordering::Relaxed);
        input.timings.active_regions.store(
            input.atmosphere.regions.iter().flatten().count() as u64,
            Ordering::Relaxed,
        );
    }
    Ok(())
}

fn terrain_prepass(
    view: ViewQuery<(&ViewDepthTexture, &ViewPrepassTextures), With<GasCamera>>,
    mut pipeline: ResMut<Pipeline>,
    cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (depth, prepass) = view.into_inner();
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let span = diagnostics.time_span(ctx.command_encoder(), "terrain-bricks");
    let ready = pipeline.bricks.encode(&cache, ctx.command_encoder());
    span.end(ctx.command_encoder());
    if !ready {
        return;
    }
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
            (7, pipeline.bricks.buffer.as_entire_binding()),
        )),
    );
    let span = diagnostics.time_span(ctx.command_encoder(), "terrain-prepass");
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
    span.end(ctx.command_encoder());
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
    let (
        Some(_draw),
        Some(init),
        Some(step),
        Some(display_build),
        Some(mask_build),
        Some(_depth),
        Some(_normals),
    ) = (
        cache.get_render_pipeline(id.gas),
        cache.get_compute_pipeline(pipeline.init),
        cache.get_compute_pipeline(pipeline.step),
        cache.get_compute_pipeline(pipeline.display.build),
        cache.get_compute_pipeline(pipeline.mask_build),
        prepass.depth_view(),
        prepass.normal_view(),
    )
    else {
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
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let mask_span = diagnostics.time_span(ctx.command_encoder(), "gas-mask");
    {
        let mut pass = ctx
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("gas occupancy"),
                timestamp_writes: None,
            });
        pass.set_pipeline(mask_build);
        for region in pipeline.regions.iter_mut().flatten() {
            if region.mask_dirty {
                pass.set_bind_group(0, &region.mask_group, &[]);
                pass.dispatch_workgroups(12, 12, 12);
                region.mask_dirty = false;
            }
        }
    }
    mask_span.end(ctx.command_encoder());
    let simulation_span = diagnostics.time_span(ctx.command_encoder(), "gas-simulation");
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
    simulation_span.end(ctx.command_encoder());
    let display_span = diagnostics.time_span(ctx.command_encoder(), "gas-display-samples");
    for r in pipeline.regions.iter_mut().flatten() {
        if let Some(display) = &mut r.display {
            display.encode(
                r.uniform.get(),
                r.simulated,
                r.applied,
                r.read,
                display_build,
                ctx.command_encoder(),
            );
        }
    }
    display_span.end(ctx.command_encoder());
    // Projectile/refraction effects follow the completed opaque, sky and
    // transparent passes. Only here may we swap the postprocess targets.
    composite(&mut ctx, &pipeline, &cache, target, prepass, id, None);
    // Overlay plumes cannot overwrite an earlier plume's colour.
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
    for (slot, _) in order {
        composite(&mut ctx, &pipeline, &cache, target, prepass, id, Some(slot));
    }
}

/// Write opaque terrain before PBR and sky, with the normal attachment clear
/// semantics. A post_process_write between opaque and transparent passes races
/// the atmosphere's in-place pass, causing black sky and intermittent unit haze.
fn shade_terrain(
    view: ViewQuery<(&ViewTarget, &ViewPrepassTextures, &DrawId), With<GasCamera>>,
    pipeline: Res<Pipeline>,
    cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (target, prepass, id) = view.into_inner();
    let (Some(draw), Some(depth), Some(normals)) = (
        cache.get_render_pipeline(id.terrain),
        prepass.depth_view(),
        prepass.normal_view(),
    ) else {
        return;
    };
    let group = ctx.render_device().create_bind_group(
        "terrain colour",
        &cache.get_bind_group_layout(&pipeline.colour_layout),
        &BindGroupEntries::with_indices((
            (2, depth),
            (3, pipeline.uniform.binding().unwrap()),
            (5, pipeline.terrain.as_entire_binding()),
            (6, normals),
            (7, pipeline.bricks.buffer.as_entire_binding()),
        )),
    );
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let span = diagnostics.time_span(ctx.command_encoder(), "terrain-colour");
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("voxel terrain colour"),
            color_attachments: &[Some(target.get_color_attachment())],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(draw);
    pass.set_bind_group(0, &group, &[]);
    pass.draw(0..3, 0..1);
    drop(pass);
    span.end(ctx.command_encoder());
}
fn composite(
    ctx: &mut RenderContext,
    pipeline: &Pipeline,
    cache: &PipelineCache,
    target: &ViewTarget,
    prepass: &ViewPrepassTextures,
    id: &DrawId,
    slot: Option<usize>,
) {
    let (Some(draw), Some(depth), Some(normals)) = (
        cache.get_render_pipeline(id.gas),
        prepass.depth_view(),
        prepass.normal_view(),
    ) else {
        return;
    };
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let (uniform, buffer, display) = if let Some(i) = slot {
        let r = pipeline.regions[i].as_ref().unwrap();
        (
            &r.uniform,
            &r.buffers[r.read],
            r.display.as_ref().map_or(&pipeline.empty, |d| &d.buffer),
        )
    } else {
        (&pipeline.uniform, &pipeline.empty, &pipeline.empty)
    };
    let span = diagnostics.time_span(
        ctx.command_encoder(),
        slot.map_or_else(|| "gas-base".to_string(), |i| format!("gas-region-{i}")),
    );
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
            pipeline.bricks.buffer.as_entire_binding(),
            display.as_entire_binding(),
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
    drop(pass);
    span.end(ctx.command_encoder());
}
