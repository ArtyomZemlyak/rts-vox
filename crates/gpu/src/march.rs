//! Марш луча по пирамиде занятости на видеокарте (Q-18, ADR-063, ADR-086).
//!
//! `21-render-bench.md` посчитал **итерации** — величину, которая
//! переносится с процессора на видеокарту. Здесь считаются миллисекунды,
//! которые не переносятся и которые единственные отвечают на вопрос
//! «влезает ли в кадр».
//!
//! Ядро **рендерное** (`docs/13-gpu.md`): результат в симуляцию
//! не возвращается, поэтому арифметика плавающая — как и будет
//! в настоящем шейдере.
//!
//! **Раскладка уровней плоская и мировая, а не поблочная.** Пирамида
//! в `harness` разложена по блокам 64³ ради компактности на процессоре;
//! здесь три уровня лежат тремя линейными битовыми массивами по мировой
//! сетке. Это то же самое содержимое и та же выровненность, но индекс
//! считается арифметикой без обращения к таблице блоков — на видеокарте
//! это дешевле, а главное проще проверяется.

use crate::Gpu;

pub const SHADER: &str = r#"
struct Dims {
    // Размер мира в вокселях и по каждому уровню, в словах по оси X.
    nx: u32, ny: u32, nz: u32, wx0: u32,
    bx: u32, by: u32, bz: u32, wx1: u32,
    tx: u32, ty: u32, tz: u32, wx2: u32,
};

struct Job {
    origin: vec4<f32>,   // xyz начало, w максимальная дальность
    dir: vec4<f32>,      // xyz направление, w не используется
};

struct Out {
    hit: u32,
    iters: u32,
};

@group(0) @binding(0) var<storage, read> l0: array<u32>;
@group(0) @binding(1) var<storage, read> l1: array<u32>;
@group(0) @binding(2) var<storage, read> l2: array<u32>;
@group(0) @binding(3) var<storage, read> jobs: array<Job>;
@group(0) @binding(4) var<storage, read_write> outs: array<Out>;
@group(0) @binding(5) var<uniform> d: Dims;
@group(0) @binding(6) var<uniform> count: vec4<u32>;

fn bit_at(lvl: u32, x: u32, y: u32, z: u32) -> bool {
    if (lvl == 0u) {
        let w = (y * d.nz + z) * d.wx0 + (x >> 5u);
        return (l0[w] & (1u << (x & 31u))) != 0u;
    }
    if (lvl == 1u) {
        let w = (y * d.bz + z) * d.wx1 + (x >> 5u);
        return (l1[w] & (1u << (x & 31u))) != 0u;
    }
    let w = (y * d.tz + z) * d.wx2 + (x >> 5u);
    return (l2[w] & (1u << (x & 31u))) != 0u;
}

// Параметр выхода из выровненной ячейки размера `size`, в которой луч
// находится в момент `t`.
fn exit_of(o: vec3<f32>, dir: vec3<f32>, inv: vec3<f32>, t: f32, size: f32) -> f32 {
    var best = 1e30;
    for (var a = 0u; a < 3u; a = a + 1u) {
        if (dir[a] == 0.0) { continue; }
        let pos = o[a] + dir[a] * t;
        let base = floor(pos / size) * size;
        var plane = base;
        if (dir[a] > 0.0) { plane = base + size; }
        let tt = (plane - o[a]) * inv[a];
        if (tt > t && tt < best) { best = tt; }
    }
    return best;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= count.x) { return; }
    let j = jobs[g.x];
    let o = j.origin.xyz;
    let dir = j.dir.xyz;
    let maxd = j.origin.w;
    var inv = vec3<f32>(1e30, 1e30, 1e30);
    if (dir.x != 0.0) { inv.x = 1.0 / dir.x; }
    if (dir.y != 0.0) { inv.y = 1.0 / dir.y; }
    if (dir.z != 0.0) { inv.z = 1.0 / dir.z; }

    var t = 0.0;
    var iters = 0u;
    var hit = 0u;
    loop {
        if (t >= maxd) { break; }
        iters = iters + 1u;
        let p = o + dir * t;
        let fx = floor(p.x);
        let fy = floor(p.y);
        let fz = floor(p.z);
        if (fx < 0.0 || fy < 0.0 || fz < 0.0) { break; }
        let x = u32(fx);
        let y = u32(fy);
        let z = u32(fz);
        if (x >= d.nx || y >= d.ny || z >= d.nz) { break; }

        var step_size = 1.0;
        if (!bit_at(2u, x >> 4u, y >> 4u, z >> 4u)) {
            step_size = 16.0;
        } else if (!bit_at(1u, x >> 2u, y >> 2u, z >> 2u)) {
            step_size = 4.0;
        } else if (bit_at(0u, x, y, z)) {
            hit = 1u;
            break;
        }
        let nt = exit_of(o, dir, inv, t, step_size);
        if (nt > 1e29) { break; }
        t = nt + 1e-4;
        // Страховка от зацикливания на вырожденном направлении: без неё
        // один плохой луч вешает всю рабочую группу.
        if (iters > 4096u) { break; }
    }
    outs[g.x].hit = hit;
    outs[g.x].iters = iters;
}
"#;

/// Три уровня по мировой сетке, каждый — линейный битовый массив.
pub struct Levels {
    pub l0: Vec<u32>,
    pub l1: Vec<u32>,
    pub l2: Vec<u32>,
    pub dims: [u32; 12],
}

/// Собрать уровни из произвольного предиката занятости.
pub fn build_levels(nx: usize, ny: usize, nz: usize, solid: impl Fn(usize, usize, usize) -> bool) -> Levels {
    let lvl = |sx: usize, sy: usize, sz: usize| (sx.div_ceil(32), sx.div_ceil(32) * sy * sz);
    let (wx0, n0) = lvl(nx, ny, nz);
    let (bx, by, bz) = (nx.div_ceil(4), ny.div_ceil(4), nz.div_ceil(4));
    let (wx1, n1) = lvl(bx, by, bz);
    let (tx, ty, tz) = (nx.div_ceil(16), ny.div_ceil(16), nz.div_ceil(16));
    let (wx2, n2) = lvl(tx, ty, tz);

    let mut l0 = vec![0u32; n0];
    let mut l1 = vec![0u32; n1];
    let mut l2 = vec![0u32; n2];
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                if !solid(x, y, z) {
                    continue;
                }
                l0[(y * nz + z) * wx0 + (x >> 5)] |= 1 << (x & 31);
                l1[((y / 4) * bz + z / 4) * wx1 + ((x / 4) >> 5)] |= 1 << ((x / 4) & 31);
                l2[((y / 16) * tz + z / 16) * wx2 + ((x / 16) >> 5)] |= 1 << ((x / 16) & 31);
            }
        }
    }
    Levels {
        l0,
        l1,
        l2,
        dims: [
            nx as u32, ny as u32, nz as u32, wx0 as u32,
            bx as u32, by as u32, bz as u32, wx1 as u32,
            tx as u32, ty as u32, tz as u32, wx2 as u32,
        ],
    }
}

/// Тот же марш на процессоре — эталон для проверки шейдера.
pub fn march_cpu(lv: &Levels, o: [f32; 3], dir: [f32; 3], maxd: f32) -> (bool, u32) {
    let d = &lv.dims;
    let bit = |lvl: usize, x: u32, y: u32, z: u32| -> bool {
        let (arr, nz, wx) = match lvl {
            0 => (&lv.l0, d[2], d[3]),
            1 => (&lv.l1, d[6], d[7]),
            _ => (&lv.l2, d[10], d[11]),
        };
        arr[((y * nz + z) * wx + (x >> 5)) as usize] & (1 << (x & 31)) != 0
    };
    let inv = [
        if dir[0] != 0.0 { 1.0 / dir[0] } else { 1e30 },
        if dir[1] != 0.0 { 1.0 / dir[1] } else { 1e30 },
        if dir[2] != 0.0 { 1.0 / dir[2] } else { 1e30 },
    ];
    let exit_of = |t: f32, size: f32| -> f32 {
        let mut best = 1e30f32;
        for a in 0..3 {
            if dir[a] == 0.0 {
                continue;
            }
            let pos = o[a] + dir[a] * t;
            let base = (pos / size).floor() * size;
            let plane = if dir[a] > 0.0 { base + size } else { base };
            let tt = (plane - o[a]) * inv[a];
            if tt > t && tt < best {
                best = tt;
            }
        }
        best
    };
    let (mut t, mut iters) = (0.0f32, 0u32);
    while t < maxd {
        iters += 1;
        let (px, py, pz) = (o[0] + dir[0] * t, o[1] + dir[1] * t, o[2] + dir[2] * t);
        let (fx, fy, fz) = (px.floor(), py.floor(), pz.floor());
        if fx < 0.0 || fy < 0.0 || fz < 0.0 {
            return (false, iters);
        }
        let (x, y, z) = (fx as u32, fy as u32, fz as u32);
        if x >= d[0] || y >= d[1] || z >= d[2] {
            return (false, iters);
        }
        let step = if !bit(2, x >> 4, y >> 4, z >> 4) {
            16.0
        } else if !bit(1, x >> 2, y >> 2, z >> 2) {
            4.0
        } else if bit(0, x, y, z) {
            return (true, iters);
        } else {
            1.0
        };
        let nt = exit_of(t, step);
        if nt > 1e29 {
            return (false, iters);
        }
        t = nt + 1e-4;
        if iters > 4096 {
            return (false, iters);
        }
    }
    (false, iters)
}

pub struct Marcher {
    pub pipe: wgpu::ComputePipeline,
    pub jobs: wgpu::Buffer,
    pub outs: wgpu::Buffer,
    pub count: wgpu::Buffer,
    pub bind: wgpu::BindGroup,
    pub cap: usize,
}

impl Marcher {
    pub fn new(g: &Gpu, lv: &Levels, cap: usize) -> Marcher {
        let pipe = g.pipeline(SHADER, "main", "march");
        let mk = |v: &[u32], label: &str| {
            let b = g.storage((v.len() * 4).max(4) as u64, label);
            g.queue.write_buffer(&b, 0, crate::as_bytes(v));
            b
        };
        let l0 = mk(&lv.l0, "l0");
        let l1 = mk(&lv.l1, "l1");
        let l2 = mk(&lv.l2, "l2");
        let jobs = g.storage((cap * 32) as u64, "jobs");
        let outs = g.storage((cap * 8) as u64, "outs");
        let uni = |v: &[u32], label: &str| {
            let b = g.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (v.len() * 4) as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            g.queue.write_buffer(&b, 0, crate::as_bytes(v));
            b
        };
        let dims = uni(&lv.dims, "dims");
        let count = uni(&[0u32; 4], "count");
        let bind = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipe.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: l0.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: l1.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: l2.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: jobs.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: outs.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: dims.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: count.as_entire_binding() },
            ],
        });
        // Буферы уровней держим живыми через замыкание bind group.
        std::mem::forget((l0, l1, l2, dims));
        Marcher { pipe, jobs, outs, count, bind, cap }
    }

    /// `jobs` — по восемь `f32` на луч: начало и дальность, направление.
    pub fn upload(&self, g: &Gpu, jobs: &[f32]) {
        assert!(jobs.len() / 8 <= self.cap);
        let bytes = unsafe {
            core::slice::from_raw_parts(jobs.as_ptr().cast::<u8>(), core::mem::size_of_val(jobs))
        };
        g.queue.write_buffer(&self.jobs, 0, bytes);
        g.queue
            .write_buffer(&self.count, 0, crate::as_bytes(&[(jobs.len() / 8) as u32, 0, 0, 0]));
    }

    pub fn encode(&self, e: &mut wgpu::CommandEncoder, rays: usize) {
        let mut p = e.begin_compute_pass(&Default::default());
        p.set_pipeline(&self.pipe);
        p.set_bind_group(0, &self.bind, &[]);
        p.dispatch_workgroups(rays.div_ceil(64) as u32, 1, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> Levels {
        // Рельеф с нависанием: холм плюс плита сверху, чтобы луч мог
        // и промахнуться, и попасть, и проскочить под плитой.
        build_levels(128, 64, 128, |x, y, z| {
            let h = 20
                + ((x as f32 * 0.07).sin() * 6.0) as i32
                + ((z as f32 * 0.05).cos() * 5.0) as i32;
            (y as i32) < h || (y == 40 && (x / 8 + z / 8) % 2 == 0)
        })
    }

    /// Шейдер обязан давать тот же вердикт и то же число итераций,
    /// что процессорный эталон. Расхождение по вердикту — луч сквозь
    /// стену; расхождение по итерациям — другая иерархия.
    #[test]
    fn gpu_march_matches_cpu() {
        let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
            eprintln!("видеокарты нет — проверка пропущена");
            return;
        };
        let lv = scene();
        let n = 4096usize;
        let m = Marcher::new(&g, &lv, n);

        let mut r = fx::rng::Rng::new(3, 0x9A7);
        let mut jobs = Vec::with_capacity(n * 8);
        let mut want = Vec::with_capacity(n);
        for _ in 0..n {
            let o = [
                r.below(128) as f32 + 0.5,
                r.range(2, 60) as f32 + 0.5,
                r.below(128) as f32 + 0.5,
            ];
            let (a, b, c) = (
                r.range(-100, 100) as f32 / 100.0,
                r.range(-100, 100) as f32 / 100.0,
                r.range(-100, 100) as f32 / 100.0,
            );
            let len = (a * a + b * b + c * c).sqrt().max(0.2);
            let d = [a / len, b / len, c / len];
            jobs.extend_from_slice(&[o[0], o[1], o[2], 200.0, d[0], d[1], d[2], 0.0]);
            want.push(march_cpu(&lv, o, d, 200.0));
        }
        m.upload(&g, &jobs);
        let mut e = g.device.create_command_encoder(&Default::default());
        m.encode(&mut e, n);
        g.queue.submit([e.finish()]);
        g.wait();

        let got = g.read_u32(&m.outs, n * 2);
        let (mut bad_hit, mut bad_it, mut hits) = (0usize, 0usize, 0usize);
        for i in 0..n {
            let (h, it) = (got[i * 2] == 1, got[i * 2 + 1]);
            hits += want[i].0 as usize;
            if h != want[i].0 {
                bad_hit += 1;
            }
            if it != want[i].1 {
                bad_it += 1;
            }
        }
        assert!(hits > n / 10, "почти ничего не попало — сцена подозрительна");
        assert_eq!(bad_hit, 0, "вердикт разошёлся на {bad_hit} лучах из {n}");
        assert_eq!(bad_it, 0, "число итераций разошлось на {bad_it} лучах из {n}");
    }
}
