//! Замеры видеокарты. **Крейт вне графа симуляции** — от него не зависит
//! ни `sim`, ни `fx`, ни что-либо, считающее golden-хеши. Граница слоёв
//! из `CLAUDE.md` («sim-core не знает про wgpu») здесь не нарушается:
//! зависимость направлена в другую сторону, и никто из симуляции сюда
//! не смотрит.
//!
//! Что здесь меряется — цена **обращения** к видеокарте, а не цена
//! арифметики на ней. ADR-087 построен на числе «диспатч стоит 24–36 мкс»,
//! взятом из чужих замеров. Если это на самом деле цена **submit**, а не
//! цена dispatch, то вывод меняется: диспатчи можно упаковать в один
//! submit и платить фиксированную часть один раз.

pub mod eikonal;

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

/// Минимальный блокирующий исполнитель: одна зависимость (`wgpu`) лучше двух.
/// `park_timeout` вместо `park` — чтобы не зависнуть, если бэкенд разбудит
/// не через waker.
pub fn block_on<F: Future>(f: F) -> F::Output {
    struct T(std::thread::Thread);
    impl Wake for T {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }
    let w = Waker::from(Arc::new(T(std::thread::current())));
    let mut cx = Context::from_waker(&w);
    let mut f = std::pin::pin!(f);
    loop {
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park_timeout(Duration::from_millis(1)),
        }
    }
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
    pub features: wgpu::Features,
    pub limits: wgpu::Limits,
    /// Наносекунд на такт таймстемпа. `None`, если `TIMESTAMP_QUERY` нет.
    pub ts_period: Option<f32>,
}

/// Фичи, которые мы просим, если адаптер их даёт. Ничего обязательного:
/// замер должен идти и на голом ядре WebGPU.
fn wanted(a: wgpu::Features) -> wgpu::Features {
    let mut f = wgpu::Features::empty();
    for w in [
        wgpu::Features::TIMESTAMP_QUERY,
        wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
        wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES,
        wgpu::Features::PIPELINE_STATISTICS_QUERY,
        wgpu::Features::MAPPABLE_PRIMARY_BUFFERS,
        wgpu::Features::SHADER_INT64,
        wgpu::Features::SUBGROUP,
    ] {
        if a.contains(w) {
            f |= w;
        }
    }
    f
}

impl Gpu {
    pub fn new(backends: wgpu::Backends) -> Option<Gpu> {
        let inst = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: Default::default(),
            display: None,
        });
        let adapter = block_on(inst.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .ok()?;
        let info = adapter.get_info();
        let features = wanted(adapter.features());
        let limits = adapter.limits();
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("bench"),
            required_features: features,
            required_limits: limits.clone(),
            ..Default::default()
        }))
        .ok()?;
        let ts_period = features
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| queue.get_timestamp_period());
        Some(Gpu { device, queue, info, features, limits, ts_period })
    }

    /// Блокирует, пока видеокарта не доработает всё отправленное.
    pub fn wait(&self) {
        self.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    }

    pub fn storage(&self, bytes: u64, label: &str) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    }

    pub fn readback_buf(&self, bytes: u64) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn pipeline(&self, wgsl: &str, entry: &str, label: &str) -> wgpu::ComputePipeline {
        let m = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(wgsl.into()),
        });
        self.device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: None,
                module: &m,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions {
                    // Явно, а не умолчанием: у wgpu оно `true`, у Bevy через
                    // `#[derive(Default)]` — `false`, и чтение `var<workgroup>`
                    // до записи вернуло бы мусор (`docs/13-gpu.md`).
                    zero_initialize_workgroup_memory: true,
                    ..Default::default()
                },
                cache: None,
            })
    }

    /// Скачать буфер и отдать как `u32`. Блокирующе — это замер, не кадр.
    pub fn read_u32(&self, src: &wgpu::Buffer, n: usize) -> Vec<u32> {
        let bytes = (n * 4) as u64;
        let dst = self.readback_buf(bytes);
        let mut e = self.device.create_command_encoder(&Default::default());
        e.copy_buffer_to_buffer(src, 0, &dst, 0, bytes);
        self.queue.submit([e.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        dst.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        self.wait();
        rx.recv().expect("map").expect("map ok");
        let v = dst.get_mapped_range(..);
        let out = v
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        drop(v);
        dst.unmap();
        out
    }
}

/// `&[u32]` как байты без `bytemuck`: выравнивание u32 строже байтового,
/// длина точная, время жизни привязано к входу.
pub fn as_bytes(v: &[u32]) -> &[u8] {
    unsafe { core::slice::from_raw_parts(v.as_ptr().cast::<u8>(), core::mem::size_of_val(v)) }
}
