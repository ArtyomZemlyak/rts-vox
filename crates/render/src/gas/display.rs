//! Cache camera-independent samples at the exact centres of display cubes.
//! Small display cubes use the direct shader path instead of unbounded storage.
use super::*;

pub(super) struct DisplayPipeline {
    layout: BindGroupLayoutDescriptor,
    pub build: CachedComputePipelineId,
}
impl DisplayPipeline {
    pub fn new(cache: &PipelineCache, shader: Handle<Shader>) -> Self {
        let layout = BindGroupLayoutDescriptor::new(
            "gas display samples",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::COMPUTE,
                (
                    (3, uniform_buffer::<GasParams>(false)),
                    (4, storage_buffer_read_only::<Vec<GpuCell>>(false)),
                    (8, storage_buffer::<Vec<GpuCell>>(false)),
                ),
            ),
        );
        let build = cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("gas display samples".into()),
            layout: vec![layout.clone()],
            shader,
            entry_point: Some("build".into()),
            ..default()
        });
        Self { layout, build }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct SampleKey {
    simulated: u32,
    applied: u64,
    edge: f32,
    field_edge: f32,
    sun: Vec4,
    time: f32,
}
impl SampleKey {
    fn new(p: &GasParams, simulated: u32, applied: u64) -> Self {
        Self {
            simulated,
            applied,
            edge: p.charge.z,
            field_edge: p.origin.w,
            sun: p.sun,
            time: p.weapon.y,
        }
    }
}
pub(super) struct Display {
    pub buffer: Buffer,
    side: u32,
    groups: [BindGroup; 2],
    key: Option<SampleKey>,
}
fn side(p: &GasParams) -> u32 {
    (GAS_N as f32 * p.origin.w / p.charge.z).ceil() as u32
}
impl Display {
    pub fn prepare(
        slot: &mut Option<Self>,
        p: &GasParams,
        uniform: &UniformBuffer<GasParams>,
        buffers: &[Buffer; 2],
        pipeline: &DisplayPipeline,
        device: &RenderDevice,
        cache: &PipelineCache,
    ) {
        let n = side(p);
        if n == 0 || n > 64 {
            *slot = None;
            return;
        }
        if slot.as_ref().is_some_and(|s| s.side == n) {
            return;
        }
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("cached gas display cells"),
            size: u64::from(n).pow(3) * 32,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let layout = cache.get_bind_group_layout(&pipeline.layout);
        let groups = std::array::from_fn(|i| {
            device.create_bind_group(
                "gas display samples",
                &layout,
                &BindGroupEntries::with_indices((
                    (3, uniform.binding().unwrap()),
                    (4, buffers[i].as_entire_binding()),
                    (8, buffer.as_entire_binding()),
                )),
            )
        });
        *slot = Some(Self {
            buffer,
            side: n,
            groups,
            key: None,
        });
    }
    pub fn encode(
        &mut self,
        p: &GasParams,
        simulated: u32,
        applied: u64,
        read: usize,
        pipeline: &ComputePipeline,
        encoder: &mut CommandEncoder,
    ) {
        let key = SampleKey::new(p, simulated, applied);
        if self.key == Some(key) {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("gas display samples"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.groups[read], &[]);
        pass.dispatch_workgroups(self.side.pow(3).div_ceil(64), 1, 1);
        self.key = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_cache_is_bounded_and_tracks_every_sample_input() {
        let mut p = GasParams::default();
        p.charge.z = 6.4;
        assert_eq!(side(&p), 38);
        p.charge.z = 3.2;
        assert!(side(&p) > 64); // exact direct fallback, no coarser smoke
        p.charge.z = 3.75;
        assert_eq!(side(&p), 64);
        let k = SampleKey::new(&p, 1, 2);
        assert!(k != SampleKey::new(&p, 2, 2));
        assert!(k != SampleKey::new(&p, 1, 3));
        for input in 0..4 {
            let mut changed = p;
            match input {
                0 => changed.charge.z += 0.1,
                1 => changed.origin.w += 0.1,
                2 => changed.sun.x += 0.1,
                _ => changed.weapon.y += 0.1,
            }
            assert!(k != SampleKey::new(&changed, 1, 2));
        }
        p.camera = Vec4::ONE;
        p.world_from_clip = Mat4::ZERO;
        p.control.z = 0.1; // compositing opacity, not a sample input
        assert!(k == SampleKey::new(&p, 1, 2));
    }
}
