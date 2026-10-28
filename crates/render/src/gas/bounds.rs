//! GPU-only conservative support: rays missing it cannot see a gas cube.
use super::*;

pub(super) struct BoundsPipeline {
    layout: BindGroupLayoutDescriptor,
    pub build: CachedComputePipelineId,
}
impl BoundsPipeline {
    pub fn new(cache: &PipelineCache, shader: Handle<Shader>) -> Self {
        let layout = BindGroupLayoutDescriptor::new(
            "gas visible bounds",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::COMPUTE,
                (
                    (4, storage_buffer_read_only::<Vec<GpuCell>>(false)),
                    (12, storage_buffer::<Vec<u32>>(false)),
                ),
            ),
        );
        let build = cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("gas visible bounds".into()),
            layout: vec![layout.clone()],
            shader,
            entry_point: Some("build".into()),
            ..default()
        });
        Self { layout, build }
    }
}

pub(super) struct Bounds {
    pub buffer: Buffer,
    groups: [BindGroup; 2],
    key: Option<(u32, u64)>,
}
impl Bounds {
    pub fn new(
        buffers: &[Buffer; 2],
        pipeline: &BoundsPipeline,
        device: &RenderDevice,
        cache: &PipelineCache,
    ) -> Self {
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("gas visible bounds"),
            size: 24,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = cache.get_bind_group_layout(&pipeline.layout);
        let groups = std::array::from_fn(|i| {
            device.create_bind_group(
                "gas visible bounds",
                &layout,
                &BindGroupEntries::with_indices((
                    (4, buffers[i].as_entire_binding()),
                    (12, buffer.as_entire_binding()),
                )),
            )
        });
        Self {
            buffer,
            groups,
            key: None,
        }
    }
    pub fn encode(
        &mut self,
        simulated: u32,
        applied: u64,
        read: usize,
        pipeline: &ComputePipeline,
        encoder: &mut CommandEncoder,
    ) {
        let key = (simulated, applied);
        if self.key == Some(key) {
            return;
        }
        encoder.clear_buffer(&self.buffer, 0, None);
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("gas visible bounds"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.groups[read], &[]);
        pass.dispatch_workgroups((GAS_CELLS as u32).div_ceil(64), 1, 1);
        self.key = Some(key);
    }
}
