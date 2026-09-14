//! Доставка изменений мира рендеру: поток правок против выемки чанка (Q-42).
//!
//! Вопрос архитектурный, но решается числом. Рендеру нужен битовый объём
//! (ADR-061) и материал видимых вокселей. Взять их можно двумя способами:
//!
//! - **уведомление «чанк изменился»** — рендер вытягивает чанк целиком
//!   и пересобирает из него всё, что ему нужно;
//! - **поток команд правок** — рендер проигрывает список правок тика
//!   в свою копию.
//!
//! Второй способ означает, что у рендера **своя производная копия мира**,
//! то есть ровно та проблема двух расходящихся представлений, ради
//! которой ADR-020 запретил меши в симуляции. Поэтому цена должна быть
//! не «лучше», а «несопоставимо лучше», иначе берётся первый.
//!
//! Ядро **рендерное, не сверяемое** (`docs/13-gpu.md`): его результат
//! в симуляцию не возвращается. Контракт при этом соблюдён и так —
//! `atomicOr`/`atomicAnd` по `u32` ассоциативны, значение в памяти
//! детерминировано, возврат не используется.

use crate::Gpu;

pub const APPLY_SHADER: &str = r#"
@group(0) @binding(0) var<storage, read_write> bits: array<atomic<u32>>;
@group(0) @binding(1) var<storage, read> edits: array<u32>;
@group(0) @binding(2) var<uniform> count: vec4<u32>;

// Правка упакована в слово: старшие 31 бит — линейный индекс воксела,
// младший — операция (1 поставить, 0 снять).
//
// **Граница берётся из `count`, а не из `arrayLength`.** Буфер правок
// выделен под потолок тика и почти всегда заполнен частично; `arrayLength`
// вернул бы его полный размер, и хвост применился бы мусором — нулевое
// слово это «снять воксель 0». Правило 13 из `docs/13-gpu.md`: индексы
// клампятся явно.
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= count.x) { return; }
    let e = edits[g.x];
    let idx = e >> 1u;
    let word = idx >> 5u;
    let mask = 1u << (idx & 31u);
    if ((e & 1u) == 1u) {
        atomicOr(&bits[word], mask);
    } else {
        atomicAnd(&bits[word], ~mask);
    }
}
"#;

pub struct EditApply {
    pub pipe: wgpu::ComputePipeline,
    pub bits: wgpu::Buffer,
    pub edits: wgpu::Buffer,
    pub count: wgpu::Buffer,
    pub bind: wgpu::BindGroup,
}

impl EditApply {
    /// `voxels` — сколько вокселей в объёме, `max_edits` — потолок правок
    /// за тик.
    pub fn new(g: &Gpu, voxels: u64, max_edits: u64) -> EditApply {
        let pipe = g.pipeline(APPLY_SHADER, "main", "apply_edits");
        let bits = g.storage(voxels / 8, "bits");
        let edits = g.storage(max_edits * 4, "edits");
        let count = g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("count"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipe.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: bits.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: edits.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: count.as_entire_binding(),
                },
            ],
        });
        EditApply {
            pipe,
            bits,
            edits,
            count,
            bind,
        }
    }

    /// Загрузить список и закодировать применение. Загрузка и диспатч
    /// живут в одном командном буфере — «все правки тика одним проходом»
    /// (`analysis/04-optimization.md`).
    pub fn encode(&self, g: &Gpu, e: &mut wgpu::CommandEncoder, list: &[u32]) {
        g.queue.write_buffer(&self.edits, 0, crate::as_bytes(list));
        g.queue.write_buffer(
            &self.count,
            0,
            crate::as_bytes(&[list.len() as u32, 0, 0, 0]),
        );
        let mut p = e.begin_compute_pass(&Default::default());
        p.set_pipeline(&self.pipe);
        p.set_bind_group(0, &self.bind, &[]);
        p.dispatch_workgroups(list.len().div_ceil(64) as u32, 1, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Применение списка правок обязано давать тот же битовый объём,
    /// что и прямая запись. Расхождение здесь — это дыра в стене
    /// у рендера при целой стене у симуляции.
    #[test]
    fn applied_edits_match_direct_writes() {
        let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
            eprintln!("видеокарты нет — проверка пропущена");
            return;
        };
        const VOX: u64 = 1 << 20;
        let a = EditApply::new(&g, VOX, 1 << 16);

        // Заполняем единицами, потом гасим и ставим вперемешку.
        let full = vec![u32::MAX; (VOX / 32) as usize];
        g.queue.write_buffer(&a.bits, 0, crate::as_bytes(&full));

        // Повторяющийся индекс в списке — это две правки одного воксела
        // в одном тике, и порядок их применения на видеокарте не определён.
        // Такого списка симуляция не отдаёт (ADR-030: у юнита не более
        // одной правки в полёте), поэтому и вход строится без повторов.
        let mut r = fx::rng::Rng::new(11, 0xED1);
        let mut seen = std::collections::BTreeSet::new();
        let mut list = Vec::new();
        for _ in 0..20000 {
            let idx = r.below(VOX as u32);
            if seen.insert(idx) {
                list.push((idx << 1) | (r.below(2) == 1) as u32);
            }
        }
        let mut want = full.clone();
        for e in &list {
            let (idx, set) = (e >> 1, e & 1 == 1);
            let (w, m) = ((idx >> 5) as usize, 1u32 << (idx & 31));
            if set {
                want[w] |= m;
            } else {
                want[w] &= !m;
            }
        }

        let mut enc = g.device.create_command_encoder(&Default::default());
        a.encode(&g, &mut enc, &list);
        g.queue.submit([enc.finish()]);
        g.wait();

        let got = g.read_u32(&a.bits, (VOX / 32) as usize);
        let bad = got.iter().zip(&want).filter(|(x, y)| x != y).count();
        assert_eq!(bad, 0, "разошлось слов: {bad} из {}", want.len());
    }
}
