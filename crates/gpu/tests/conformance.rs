//! Conformance-вектор для сверяемых ядер (ADR-092, `docs/13-gpu.md`).
//!
//! Смысл вектора — не в том, чтобы поймать ошибку здесь. Он в том, чтобы
//! **его можно было прогнать на чужой машине**, не имея её под рукой:
//! файл `tests/golden/eikonal-v1.txt` фиксирует вход и выход, и любое
//! расхождение называет сектор и ячейку.
//!
//! Ровно эту форму использует ProgPoW: спецификация на беззнаковых
//! 32-битных целых плюс опубликованные кросс-реализационные векторы
//! (`docs/analysis/09-gpu-compute.md`, 11.3).
//!
//! Три проверки, и порядок между ними важен:
//!
//! 1. **Эталон воспроизводит вектор.** Если нет — изменилась
//!    процессорная реализация, и это осознанная смена версии вектора,
//!    а не «поправить файл».
//! 2. **Видеокарта воспроизводит вектор.** Если нет — портируемое
//!    подмножество на этой машине не работает, и сверяемый путь
//!    выключается целиком.
//! 3. Вектор перегенерируется только руками, переменной окружения.
//!
//! Обновить: `REGEN_GOLDEN=1 cargo test --release -p gpu --test conformance`

use std::path::PathBuf;

use gpu::eikonal::{cost_words, cpu_sector, Solver, N};
use gpu::Gpu;

/// Секторов в векторе. Восемь хватает: расхождение подмножества
/// не бывает выборочным, а файл остаётся читаемым глазом.
const SECTORS: u32 = 8;
const ITERS: u32 = 48;

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/eikonal-v1.txt")
}

/// Текстом, а не байтами: расхождение обязано читаться в диффе.
fn render(fields: &[Vec<u16>]) -> String {
    let mut s = String::new();
    s.push_str("# conformance-вектор: эйконал, портируемое подмножество\n");
    s.push_str("# ADR-092, docs/13-gpu.md. Перегенерация — REGEN_GOLDEN=1.\n");
    s.push_str(&format!(
        "сектор 16x16, секторов {SECTORS}, итераций {ITERS}\n"
    ));
    s.push_str("стоимости: cost_words(), детерминированы от индекса\n");
    s.push_str("поле: 16 бит на клетку, строка = ряд сектора, hex\n");
    for (k, f) in fields.iter().enumerate() {
        s.push_str(&format!("\n[сектор {k}]\n"));
        for row in f.chunks(16) {
            let line: Vec<String> = row.iter().map(|v| format!("{v:04x}")).collect();
            s.push_str(&line.join(" "));
            s.push('\n');
        }
    }
    s
}

fn reference() -> Vec<Vec<u16>> {
    let cw = cost_words(SECTORS as usize * N);
    (0..SECTORS as usize)
        .map(|sec| {
            let base = sec * N;
            let bytes: Vec<u8> = (0..N)
                .map(|i| ((cw[(base + i) >> 2] >> (((base + i) & 3) * 8)) & 255) as u8)
                .collect();
            cpu_sector(&bytes, ITERS)
        })
        .collect()
}

#[test]
fn cpu_reference_reproduces_the_vector() {
    let want = render(&reference());
    let p = golden_path();
    if std::env::var("REGEN_GOLDEN").is_ok() || !p.exists() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, &want).unwrap();
        if std::env::var("REGEN_GOLDEN").is_ok() {
            eprintln!("вектор перезаписан: {}", p.display());
            return;
        }
    }
    let have = std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n");
    if have != want {
        let (a, b) = (
            have.lines().collect::<Vec<_>>(),
            want.lines().collect::<Vec<_>>(),
        );
        let at = a.iter().zip(&b).position(|(x, y)| x != y);
        panic!(
            "эталон разошёлся с вектором на строке {:?}:\n  вектор: {:?}\n  эталон: {:?}\n\
             это смена версии вектора, а не правка файла",
            at.map(|i| i + 1),
            at.and_then(|i| a.get(i)),
            at.and_then(|i| b.get(i))
        );
    }
}

#[test]
fn gpu_reproduces_the_vector() {
    let Some(g) = Gpu::new(wgpu::Backends::VULKAN) else {
        eprintln!("видеокарты нет — сверяемый путь не проверяется");
        return;
    };
    let s = Solver::new(&g, SECTORS);
    s.set_params(&g, SECTORS, ITERS);
    let mut e = g.device.create_command_encoder(&Default::default());
    s.encode(&mut e, SECTORS);
    g.queue.submit([e.finish()]);
    g.wait();
    let words = g.read_u32(&s.field, SECTORS as usize * N / 2);
    let got: Vec<Vec<u16>> = (0..SECTORS as usize)
        .map(|k| Solver::sector_field(&words, k))
        .collect();

    let want = reference();
    for sec in 0..SECTORS as usize {
        for i in 0..N {
            assert_eq!(
                got[sec][i],
                want[sec][i],
                "сектор {sec}, ячейка {i} ({}, {}): видеокарта {}, вектор {} — \
                 портируемое подмножество на этой машине не работает, \
                 сверяемый путь выключается целиком",
                i % 16,
                i / 16,
                got[sec][i],
                want[sec][i]
            );
        }
    }
}
