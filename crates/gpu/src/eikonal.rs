//! Эйконал на видеокарте: единственная фаза, которой отставание разрешено.
//!
//! `CLAUDE.md` разрешает отставание ровно двум вещам — рендеру
//! и **интегрированию flow field**. ADR-048 фиксирует то же расщепление:
//! «nav_update — проходимость немедленно, интегрирование по очереди».
//!
//! Схема повторяет CPU-замер (ADR-066): грязные секторы 16×16, стоимость
//! 8 бит, поле 16 бит, **целая арифметика**. Одна рабочая группа на сектор,
//! 256 потоков, Якоби в разделяемой памяти с барьером.
//!
//! **Почему Якоби, а не FIM.** Якоби читает соседей из предыдущей
//! итерации, поэтому порядок потоков на результат не влияет. Это и есть
//! посылка теоремы Cousot, на которой стоит ADR-066: атомиков нет,
//! редукций нет, арифметика целая — значит, результат не зависит
//! ни от расписания варпов, ни от их числа.
//!
//! **Корень побитовый, а не `sqrt(f32(x))`.** Плавающий корень
//! на видеокарте не воспроизводим между вендорами, и всё, ради чего эта
//! фаза целая, сломалось бы в одной строке.

use crate::Gpu;

/// Сторона сектора в нав-клетках. Как в CPU-замере.
pub const SEC: u32 = 16;
/// Ячеек в секторе.
pub const N: usize = (SEC * SEC) as usize;
/// «Недостижимо». Поле 16-битное, поэтому это же и потолок.
pub const INF: u32 = 65535;

pub const SHADER: &str = r#"
struct Params {
    sectors: u32,
    iters: u32,
};

@group(0) @binding(0) var<storage, read> cost: array<u32>;        // 4 стоимости на слово
@group(0) @binding(1) var<storage, read_write> field: array<u32>; // 2 значения на слово
@group(0) @binding(2) var<uniform> p: Params;

const SEC: u32 = 16u;
const N: u32 = 256u;
const INF: u32 = 65535u;

var<workgroup> a: array<u32, 256>;
var<workgroup> b: array<u32, 256>;

// Побитовый целочисленный корень. Дискриминант тут не больше 2*255^2,
// то есть 17 бит, поэтому стартовая позиция 1<<16 достаточна и точна:
// это наибольшая степень четырёх, не превосходящая аргумент.
fn isqrt(n: u32) -> u32 {
    var x = n;
    var c = 0u;
    var d = 1u << 16u;
    loop {
        if (d == 0u) { break; }
        if (x >= c + d) {
            x = x - (c + d);
            c = (c >> 1u) + d;
        } else {
            c = c >> 1u;
        }
        d = d >> 2u;
    }
    return c;
}

// Годуновское обновление в разностной форме: T = lo + h(f, d).
fn godunov(lo: u32, hi: u32, f: u32) -> u32 {
    if (lo >= INF) { return INF; }
    if (hi >= INF || hi - lo >= f) { return lo + f; }
    let d = hi - lo;
    return lo + (d + isqrt(2u * f * f - d * d)) / 2u;
}

fn read_cost(base: u32, i: u32) -> u32 {
    let w = cost[(base + i) >> 2u];
    return (w >> (((base + i) & 3u) * 8u)) & 255u;
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wid: vec3<u32>,
        @builtin(local_invocation_id) lid: vec3<u32>) {
    let sector = wid.x;
    // Ветвление по `workgroup_id` однородно внутри группы, поэтому
    // барьеры ниже законны.
    if (sector >= p.sectors) { return; }
    let i = lid.x;
    let x = i % SEC;
    let y = i / SEC;
    let base = sector * N;

    // Цель — угол сектора. В настоящей задаче это фронт из соседнего
    // сектора; для объёма работы важно лишь, что фронт один и пересекает
    // весь сектор.
    let seed = select(INF, 0u, i == 0u);
    a[i] = seed;
    b[i] = seed;
    workgroupBarrier();

    let f = max(read_cost(base, i), 1u);
    var iter = 0u;
    loop {
        if (iter >= p.iters) { break; }
        var hx = INF;
        if (x > 0u)       { hx = min(hx, a[i - 1u]); }
        if (x < SEC - 1u) { hx = min(hx, a[i + 1u]); }
        var hy = INF;
        if (y > 0u)       { hy = min(hy, a[i - SEC]); }
        if (y < SEC - 1u) { hy = min(hy, a[i + SEC]); }

        var v = godunov(min(hx, hy), max(hx, hy), f);
        v = min(v, a[i]);
        if (i == 0u) { v = 0u; }
        b[i] = v;
        workgroupBarrier();
        a[i] = b[i];
        workgroupBarrier();
        iter = iter + 1u;
    }

    // Два значения по 16 бит в слово. Слово трогает только чётный поток
    // и берёт половину соседа из разделяемой памяти — иначе два потока
    // писали бы в одно слово.
    if ((i & 1u) == 0u) {
        field[(base + i) >> 1u] = a[i] | (a[i + 1u] << 16u);
    }
}
"#;

/// Тот же Якоби на процессоре, той же целой арифметикой.
///
/// Существует ради одной проверки: **совпадает ли результат побитово**.
/// Если нет — переносить фазу нельзя ни при какой скорости, потому что
/// поле навигации читает симуляция, а она в lockstep.
pub fn cpu_sector(cost: &[u8], iters: u32) -> Vec<u16> {
    let mut a = vec![INF; N];
    a[0] = 0;
    let mut b = a.clone();
    let god = |lo: u32, hi: u32, f: u32| -> u32 {
        if lo >= INF {
            return INF;
        }
        if hi >= INF || hi - lo >= f {
            return lo + f;
        }
        let d = hi - lo;
        lo + (d + (2 * f * f - d * d).isqrt()) / 2
    };
    for _ in 0..iters {
        for i in 0..N {
            let (x, y) = (i as u32 % SEC, i as u32 / SEC);
            let mut hx = INF;
            if x > 0 {
                hx = hx.min(a[i - 1]);
            }
            if x < SEC - 1 {
                hx = hx.min(a[i + 1]);
            }
            let mut hy = INF;
            if y > 0 {
                hy = hy.min(a[i - SEC as usize]);
            }
            if y < SEC - 1 {
                hy = hy.min(a[i + SEC as usize]);
            }
            let f = (cost[i] as u32).max(1);
            let mut v = god(hx.min(hy), hx.max(hy), f);
            v = v.min(a[i]);
            if i == 0 {
                v = 0;
            }
            b[i] = v;
        }
        a.copy_from_slice(&b);
    }
    a.iter().map(|&v| v as u16).collect()
}

/// Стоимости: 4 на слово, значения 1..=200, детерминированно от индекса.
pub fn cost_words(cells: usize) -> Vec<u32> {
    (0..cells / 4)
        .map(|i| {
            let i = i as u64;
            let b = |k: u64| ((i.wrapping_mul(2654435761).wrapping_add(k) % 200) + 1) as u32;
            b(0) | (b(1) << 8) | (b(2) << 16) | (b(3) << 24)
        })
        .collect()
}

/// Развёрнутый на видеокарте решатель: буферы живут между прогонами,
/// как и положено — выделение памяти на тик мерило бы аллокатор.
pub struct Solver {
    pub pipe: wgpu::ComputePipeline,
    pub cost: wgpu::Buffer,
    pub field: wgpu::Buffer,
    pub params: wgpu::Buffer,
    pub bind: wgpu::BindGroup,
    pub max_sectors: u32,
    pub cost_words: Vec<u32>,
}

impl Solver {
    pub fn new(g: &Gpu, max_sectors: u32) -> Solver {
        let pipe = g.pipeline(SHADER, "main", "eikonal");
        let cells = max_sectors as usize * N;
        let cost = g.storage(cells as u64, "cost");
        let field = g.storage(cells as u64 * 2, "field");
        let params = g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params"),
            size: 8,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cw = cost_words(cells);
        g.queue.write_buffer(&cost, 0, crate::as_bytes(&cw));
        let bind = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipe.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: cost.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: field.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: params.as_entire_binding() },
            ],
        });
        Solver { pipe, cost, field, params, bind, max_sectors, cost_words: cw }
    }

    /// Закодировать один диспатч в переданный кодировщик.
    pub fn encode(&self, e: &mut wgpu::CommandEncoder, sectors: u32) {
        let mut p = e.begin_compute_pass(&Default::default());
        p.set_pipeline(&self.pipe);
        p.set_bind_group(0, &self.bind, &[]);
        p.dispatch_workgroups(sectors, 1, 1);
    }

    pub fn set_params(&self, g: &Gpu, sectors: u32, iters: u32) {
        g.queue
            .write_buffer(&self.params, 0, crate::as_bytes(&[sectors, iters]));
    }

    /// Стоимости сектора в байтах — то, что подаётся эталону.
    pub fn sector_cost(&self, sector: usize) -> Vec<u8> {
        let base = sector * N;
        (0..N)
            .map(|i| {
                let w = self.cost_words[(base + i) >> 2];
                ((w >> (((base + i) & 3) * 8)) & 255) as u8
            })
            .collect()
    }

    /// Распаковать поле сектора из скачанных слов.
    pub fn sector_field(words: &[u32], sector: usize) -> Vec<u16> {
        let base = sector * N;
        (0..N)
            .map(|i| {
                let w = words[(base + i) >> 1];
                (if (i & 1) == 0 { w & 0xFFFF } else { w >> 16 }) as u16
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Самая нагруженная проверка этого крейта.** Шейдер и эталон
    /// на процессоре обязаны дать побитово одно и то же. Расхождение
    /// означает, что переносить фазу нельзя ни при какой скорости.
    ///
    /// Тест молча проходит, если видеокарты нет: `sim-cli` и CI обязаны
    /// собираться и на машине без неё.
    #[test]
    fn gpu_eikonal_matches_cpu_bit_for_bit() {
        let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
            eprintln!("видеокарты нет — проверка пропущена");
            return;
        };
        let sectors = 64u32;
        let iters = 48u32;
        let s = Solver::new(&g, sectors);
        s.set_params(&g, sectors, iters);
        let mut e = g.device.create_command_encoder(&Default::default());
        s.encode(&mut e, sectors);
        g.queue.submit([e.finish()]);
        g.wait();

        let words = g.read_u32(&s.field, sectors as usize * N / 2);
        let mut checked = 0usize;
        for sec in 0..sectors as usize {
            let want = cpu_sector(&s.sector_cost(sec), iters);
            let got = Solver::sector_field(&words, sec);
            for i in 0..N {
                assert_eq!(
                    got[i], want[i],
                    "сектор {sec}, ячейка {i}: видеокарта {}, процессор {}",
                    got[i], want[i]
                );
                checked += 1;
            }
        }
        assert_eq!(checked, sectors as usize * N);
    }

    /// Побитовый корень обязан совпадать с `u32::isqrt` на всём диапазоне
    /// дискриминанта, а не «примерно»: расхождение на единицу — это другое
    /// поле навигации.
    #[test]
    fn bitwise_isqrt_covers_the_discriminant_range() {
        // Максимум дискриминанта: 2*255^2 при d = 0.
        for x in 0..=2 * 255u32 * 255 {
            let mut v = x;
            let mut c = 0u32;
            let mut d = 1u32 << 16;
            while d != 0 {
                if v >= c + d {
                    v -= c + d;
                    c = (c >> 1) + d;
                } else {
                    c >>= 1;
                }
                d >>= 2;
            }
            assert_eq!(c, x.isqrt(), "x = {x}");
        }
    }
}
