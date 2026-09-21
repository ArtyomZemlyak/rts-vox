//! Conservative empty-space acceleration for the wrapped terrain window.
//! Only changed bricks are rebuilt; the GPU still tests each occupied voxel.
use super::*;
use std::ops::Range;

const MAX_BRICKS: usize = TERRAIN_WORDS / 64;

pub(super) struct Bricks {
    pub buffer: Buffer,
    indices: Buffer,
    config: UniformBuffer<UVec4>,
    group: Option<BindGroup>,
    layout: BindGroupLayoutDescriptor,
    build: CachedComputePipelineId,
    reduce: CachedComputePipelineId,
    pending: u32,
    dirty: Vec<u64>,
    full: bool,
    dims: [u32; 3],
}

impl Bricks {
    pub fn new(device: &RenderDevice, cache: &PipelineCache, shader: Handle<Shader>) -> Self {
        let layout = BindGroupLayoutDescriptor::new(
            "terrain brick build",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    uniform_buffer::<UVec4>(false),
                    storage_buffer_read_only::<Vec<u32>>(false),
                    storage_buffer_read_only::<Vec<u32>>(false),
                    storage_buffer::<Vec<u32>>(false),
                ),
            ),
        );
        let build = cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("terrain brick build".into()),
            layout: vec![layout.clone()],
            shader: shader.clone(),
            entry_point: Some("build".into()),
            ..default()
        });
        let reduce = cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("terrain coarse brick build".into()),
            layout: vec![layout.clone()],
            shader,
            entry_point: Some("reduce".into()),
            ..default()
        });
        let buffer = |label| {
            device.create_buffer(&BufferDescriptor {
                label: Some(label),
                size: ((MAX_BRICKS + MAX_BRICKS / 64) * 4) as u64,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            buffer: buffer("terrain occupied bricks"),
            indices: buffer("terrain dirty bricks"),
            config: UniformBuffer::from(UVec4::ZERO),
            group: None,
            layout,
            build,
            reduce,
            pending: 0,
            dirty: vec![],
            full: false,
            dims: [0; 3],
        }
    }

    pub fn prepare(
        &mut self,
        window: TerrainWindow,
        patches: &[Range<usize>],
        terrain: &Buffer,
        device: &RenderDevice,
        queue: &RenderQueue,
        cache: &PipelineCache,
    ) {
        // Misaligned or tiny diagnostic windows use the original voxel march.
        if window.dims.iter().any(|d| *d == 0 || d % 8 != 0)
            || window.origin.iter().any(|v| *v < 0 || v % 8 != 0)
        {
            self.dims = [0; 3];
            self.pending = 0;
            return;
        }
        let count = window.words() / 64;
        if self.dims != window.dims {
            self.dims = window.dims;
            self.dirty = vec![0; count.div_ceil(64)];
            self.full = true;
        }
        if patches.is_empty() && self.pending == 0 && !self.full {
            return;
        }
        if patches.iter().map(|r| r.len()).sum::<usize>() >= window.words() / 8 {
            self.full = true;
        }
        // Accumulate until submitted, even if a pipeline is still compiling.
        if !self.full {
            for span in patches {
                mark(&mut self.dirty, window.dims, span.clone());
            }
        }
        let indices: Vec<u32> = if self.full {
            vec![]
        } else {
            self.dirty
                .iter()
                .enumerate()
                .flat_map(|(i, &bits)| {
                    let mut b = bits;
                    std::iter::from_fn(move || {
                        if b == 0 {
                            return None;
                        }
                        let bit = b.trailing_zeros();
                        b &= b - 1;
                        Some(i as u32 * 64 + bit)
                    })
                })
                .collect()
        };
        self.pending = if self.full {
            count as u32
        } else {
            indices.len() as u32
        };
        if self.pending == 0 {
            return;
        }
        if !self.full {
            let bytes: Vec<u8> = indices.iter().flat_map(|v| v.to_le_bytes()).collect();
            queue.write_buffer(&self.indices, 0, &bytes);
        }
        self.config.set(UVec4::new(
            window.dims[0],
            window.dims[1],
            window.dims[2],
            self.pending | if self.full { 0x80000000 } else { 0 },
        ));
        self.config.write_buffer(device, queue);
        self.group = Some(device.create_bind_group(
            "terrain brick build",
            &cache.get_bind_group_layout(&self.layout),
            &BindGroupEntries::sequential((
                self.config.binding().unwrap(),
                terrain.as_entire_binding(),
                self.indices.as_entire_binding(),
                self.buffer.as_entire_binding(),
            )),
        ));
    }

    pub fn encode(&mut self, cache: &PipelineCache, encoder: &mut CommandEncoder) -> bool {
        if self.pending == 0 {
            return true;
        }
        let (Some(build), Some(reduce), Some(group)) = (
            cache.get_compute_pipeline(self.build),
            cache.get_compute_pipeline(self.reduce),
            &self.group,
        ) else {
            return false;
        };
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("terrain brick build"),
                timestamp_writes: None,
            });
            pass.set_pipeline(build);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(self.pending.min(65535), self.pending.div_ceil(65535), 1);
        }
        if self.dims.iter().all(|n| n % 32 == 0) {
            let count = self.dims.iter().map(|n| n / 32).product::<u32>();
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("terrain coarse brick build"),
                timestamp_writes: None,
            });
            pass.set_pipeline(reduce);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(count, 1, 1);
        }
        self.pending = 0;
        self.full = false;
        self.dirty.fill(0);
        true
    }
}

// A word is eight consecutive x voxels. Visit rows, not individual words;
// repeated y/z rows coalesce into a small bitset before any GPU upload.
fn mark(bits: &mut [u64], dims: [u32; 3], mut words: Range<usize>) {
    let nx = dims[0] as usize / 8;
    let ny = dims[1] as usize;
    while words.start < words.end {
        let row = words.start / nx;
        let x = words.start % nx;
        let n = (nx - x).min(words.end - words.start);
        let mut first = x + nx * ((row % ny) / 8 + ny / 8 * (row / ny / 8));
        let last = first + n;
        while first < last {
            let span = (64 - first % 64).min(last - first);
            bits[first / 64] |= (u64::MAX >> (64 - span)) << (first % 64);
            first += span;
        }
        words.start += n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dirty_rows_match_every_patched_voxel_including_wrap_and_partial_words() {
        for dims in [[24, 16, 40], [1280, 192, 1280]] {
            let count = dims.iter().product::<u32>() as usize / 512;
            let words = count * 64;
            let spans = [
                0..1,
                1..137,
                words / 2 - 13..words / 2 + 83,
                words - 191..words,
            ];
            let mut marked = vec![0; count.div_ceil(64)];
            let mut expected = marked.clone();
            for span in spans {
                mark(&mut marked, dims, span.clone());
                for word in span {
                    let i = word * 8;
                    let x = i % dims[0] as usize;
                    let y = i / dims[0] as usize % dims[1] as usize;
                    let z = i / (dims[0] * dims[1]) as usize;
                    let b = x / 8 + dims[0] as usize / 8 * (y / 8 + dims[1] as usize / 8 * (z / 8));
                    expected[b / 64] |= 1 << (b % 64);
                }
            }
            assert_eq!(marked, expected);
            mark(&mut marked, dims, 0..words);
            assert_eq!(
                marked.iter().map(|b| b.count_ones()).sum::<u32>() as usize,
                count
            );
        }
    }
}
