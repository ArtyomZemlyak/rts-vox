//! Из чего складывается цена обращения к видеокарте.
//!
//! Запуск: `cargo run --release -p gpu --bin bench_submit`
//!
//! ADR-087 отверг перенос симуляции на видеокарту по одному числу:
//! «диспатч стоит 24–36 мкс, а половина фаз дешевле». Число взято из
//! чужой работы (arXiv 2608.08730) и в ней же оговорено, что «наивный
//! замер одиночного диспатча завышает результат примерно в двадцать раз».
//!
//! Если 24–36 мкс — цена **отправки**, а не цена диспатча, то вывод
//! неверен: отправку платят один раз за тик, а не за фазу. Тогда
//! осмысленны обе идеи — упаковать все фазы в один командный буфер
//! и считать не каждый тик, а пачкой.
//!
//! Здесь эти величины разделяются:
//!
//! - `F` — постоянная часть: кодирование пустого буфера, `submit`,
//!   ожидание готовности. Платится **за отправку**.
//! - `m` — предельная цена **ещё одного** диспатча в уже отправляемом
//!   буфере.
//!
//! Всё, что дальше, — арифметика по этим двум числам. Фаза каждый тик
//! стоит `F + m`; та же фаза пачкой раз в `N` тиков — `F/N + m`.

use std::time::Instant;

use gpu::Gpu;

/// Сколько разных операций изображаем. Столько же фаз в тике у нас
/// кандидатов на перенос.
const KINDS: usize = 8;
const REPS: usize = 51;

fn shader(k: usize) -> String {
    // Разные модули, а не один с константой: цель — измерить смену
    // конвейера и группы привязок, как при смене операции.
    format!(
        r#"
@group(0) @binding(0) var<storage, read_write> data: array<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {{
    let n = arrayLength(&data);
    if (g.x < n) {{
        data[g.x] = data[g.x] * 1664525u + {}u;
    }}
}}
"#,
        k * 7 + 1013904223
    )
}

/// Медиана и минимум, наносекунды.
fn stat(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[v.len() / 2], v[0])
}

fn section(t: &str) {
    println!("\n=== {t} ===");
}

struct Kit {
    pipes: Vec<wgpu::ComputePipeline>,
    binds: Vec<wgpu::BindGroup>,
    #[allow(dead_code)]
    bufs: Vec<wgpu::Buffer>,
}

impl Kit {
    fn new(g: &Gpu, elems: u64) -> Kit {
        let mut pipes = Vec::new();
        let mut binds = Vec::new();
        let mut bufs = Vec::new();
        for k in 0..KINDS {
            let p = g.pipeline(&shader(k), "main", &format!("kind{k}"));
            let b = g.storage(elems * 4, &format!("buf{k}"));
            let bg = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &p.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: b.as_entire_binding(),
                }],
            });
            pipes.push(p);
            binds.push(bg);
            bufs.push(b);
        }
        Kit { pipes, binds, bufs }
    }
}

/// Один командный буфер: `passes` проходов, в каждом `per_pass` диспатчей.
/// `distinct` — брать ли каждый раз новую операцию или повторять одну.
fn encode(
    g: &Gpu,
    kit: &Kit,
    passes: usize,
    per_pass: usize,
    groups: u32,
    distinct: bool,
) -> wgpu::CommandBuffer {
    let mut e = g.device.create_command_encoder(&Default::default());
    let mut n = 0usize;
    for _ in 0..passes {
        let mut p = e.begin_compute_pass(&Default::default());
        for _ in 0..per_pass {
            let k = if distinct { n % KINDS } else { 0 };
            p.set_pipeline(&kit.pipes[k]);
            p.set_bind_group(0, &kit.binds[k], &[]);
            p.dispatch_workgroups(groups, 1, 1);
            n += 1;
        }
    }
    e.finish()
}

/// Полный оборот: закодировать, отправить, дождаться. Возвращает
/// (кодирование, отправка, ожидание) в наносекундах.
fn round(
    g: &Gpu,
    kit: &Kit,
    passes: usize,
    per_pass: usize,
    groups: u32,
    distinct: bool,
) -> [f64; 3] {
    let t0 = Instant::now();
    let cb = encode(g, kit, passes, per_pass, groups, distinct);
    let t1 = Instant::now();
    g.queue.submit([cb]);
    let t2 = Instant::now();
    g.wait();
    let t3 = Instant::now();
    [
        (t1 - t0).as_nanos() as f64,
        (t2 - t1).as_nanos() as f64,
        (t3 - t2).as_nanos() as f64,
    ]
}

fn measure(
    g: &Gpu,
    kit: &Kit,
    passes: usize,
    per_pass: usize,
    groups: u32,
    distinct: bool,
) -> [f64; 4] {
    for _ in 0..5 {
        round(g, kit, passes, per_pass, groups, distinct);
    }
    let (mut e, mut s, mut w, mut t) = (vec![], vec![], vec![], vec![]);
    for _ in 0..REPS {
        let r = round(g, kit, passes, per_pass, groups, distinct);
        e.push(r[0]);
        s.push(r[1]);
        w.push(r[2]);
        t.push(r[0] + r[1] + r[2]);
    }
    [stat(e).0, stat(s).0, stat(w).0, stat(t).0]
}

fn main() {
    let g = match Gpu::new(wgpu::Backends::VULKAN) {
        Some(g) => g,
        None => {
            println!("адаптера нет");
            return;
        }
    };
    println!(
        "{} | {:?} | драйвер {}",
        g.info.name, g.info.backend, g.info.driver_info
    );
    println!("медиана из {REPS} прогонов, наносекунды\n");

    // 4096 элементов: 64 рабочие группы по 64 — работы почти нет,
    // мерится обращение, а не арифметика.
    let kit = Kit::new(&g, 4096);

    section("1. Пустой оборот: сколько стоит просто сходить на видеокарту");
    println!("один диспатч на одну рабочую группу — работы там нет вовсе.\n");
    let r = measure(&g, &kit, 1, 1, 1, false);
    println!("кодирование {:>9.0} нс", r[0]);
    println!("submit      {:>9.0} нс", r[1]);
    println!("ожидание    {:>9.0} нс", r[2]);
    println!("итого       {:>9.0} нс = {:.1} мкс", r[3], r[3] / 1000.0);

    section("2. Сколько стоит следующий диспатч в том же буфере");
    println!("один проход, N диспатчей, одна отправка, одно ожидание.\n");
    println!(
        "{:>5} {:>12} {:>12} {:>12} {:>12} {:>14}",
        "N", "кодир.", "submit", "ожидание", "итого", "на диспатч"
    );
    let mut same = Vec::new();
    for n in [1usize, 2, 4, 8, 16, 32, 64, 128] {
        let r = measure(&g, &kit, 1, n, 1, false);
        same.push((n, r));
        println!(
            "{n:>5} {:>12.0} {:>12.0} {:>12.0} {:>12.0} {:>14.0}",
            r[0],
            r[1],
            r[2],
            r[3],
            r[3] / n as f64
        );
    }
    let (n1, r1) = same[0];
    let (n2, r2) = *same.last().unwrap();
    let m = (r2[3] - r1[3]) / (n2 - n1) as f64;
    let f = r1[3] - m;
    println!();
    println!("линейная подгонка по краям: цена = F + m·N");
    println!("  F (постоянная часть отправки) = {:.1} мкс", f / 1000.0);
    println!("  m (предельная цена диспатча)  = {:.0} нс", m);

    section("3. Разные операции против одной и той же");
    println!("восемь разных конвейеров и групп привязок в одном буфере.\n");
    println!(
        "{:>5} {:>14} {:>14} {:>10}",
        "N", "одна и та же", "разные", "дороже"
    );
    for n in [8usize, 16, 32, 64, 128] {
        let a = measure(&g, &kit, 1, n, 1, false);
        let b = measure(&g, &kit, 1, n, 1, true);
        println!("{n:>5} {:>14.0} {:>14.0} {:>9.2}x", a[3], b[3], b[3] / a[3]);
    }

    section("4. Барьеры: один проход против прохода на фазу");
    println!("зависимые фазы обязаны разделяться проходами — это барьер.\n");
    println!(
        "{:>5} {:>14} {:>14} {:>10}",
        "фаз", "в одном проходе", "проход на фазу", "дороже"
    );
    for n in [2usize, 4, 8, 16, 32] {
        let a = measure(&g, &kit, 1, n, 1, true);
        let b = measure(&g, &kit, n, 1, 1, true);
        println!("{n:>5} {:>14.0} {:>14.0} {:>9.2}x", a[3], b[3], b[3] / a[3]);
    }

    section("5. Отдельная отправка на фазу против одной общей");
    println!("«каждая фаза сама ходит на видеокарту» против «все вместе».\n");
    let mut warm = 0;
    for n in [1usize, 2, 4, 8, 16] {
        // Все вместе: один буфер, одна отправка, одно ожидание.
        let together = measure(&g, &kit, n, 1, 1, true);
        // Каждая сама: свой буфер, своя отправка, своё ожидание.
        let mut v = Vec::new();
        for _ in 0..(REPS + 5) {
            let t0 = Instant::now();
            for i in 0..n {
                let cb = encode(&g, &kit, 1, 1, 1, i % KINDS != 0);
                g.queue.submit([cb]);
                g.wait();
            }
            v.push(t0.elapsed().as_nanos() as f64);
        }
        v.drain(0..5);
        let apart = stat(v).0;
        if warm == 0 {
            println!(
                "{:>5} {:>16} {:>16} {:>10}",
                "фаз", "одной отправкой", "по отдельности", "разница"
            );
            warm = 1;
        }
        println!(
            "{n:>5} {:>16.0} {:>16.0} {:>9.2}x",
            together[3],
            apart,
            apart / together[3]
        );
    }

    section("6. Когда работа перевешивает обращение");
    println!("тот же один диспатч, но с растущим числом рабочих групп.\n");
    let big = Kit::new(&g, 64 * 1024 * 1024);
    println!(
        "{:>12} {:>14} {:>14} {:>14}",
        "рабочих групп", "элементов", "итого, нс", "ожидание, нс"
    );
    for gcount in [1u32, 64, 1024, 16384, 65535] {
        let r = measure(&g, &big, 1, 1, gcount, false);
        println!(
            "{gcount:>12} {:>14} {:>14.0} {:>14.0}",
            gcount as u64 * 64,
            r[3],
            r[2]
        );
    }

    section("7. Ждать или крутиться");
    println!("`poll(Wait)` усыпляет поток. Это может быть не задержка");
    println!("видеокарты, а задержка планировщика Windows.\n");
    let mut v = Vec::new();
    for _ in 0..(REPS + 5) {
        let cb = encode(&g, &kit, 1, 1, 1, false);
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let d2 = done.clone();
        let t0 = Instant::now();
        g.queue.submit([cb]);
        g.queue
            .on_submitted_work_done(move || d2.store(true, std::sync::atomic::Ordering::Release));
        // `PollType::Poll` всегда возвращает `Poll`, а не «очередь пуста»:
        // единственный честный признак готовности — обратный вызов.
        while !done.load(std::sync::atomic::Ordering::Acquire) {
            let _ = g.device.poll(wgpu::PollType::Poll);
            std::hint::spin_loop();
        }
        v.push(t0.elapsed().as_nanos() as f64);
    }
    v.drain(0..5);
    let spin = stat(v);
    let blk = measure(&g, &kit, 1, 1, 1, false);
    println!("усыпить поток (`Wait`):  {:>9.0} нс", blk[1] + blk[2]);
    println!(
        "крутиться (`Poll`):      {:>9.0} нс (мин {:.0})",
        spin.0, spin.1
    );

    section("8. Забрать результат обратно");
    println!("здесь и живёт «readback стоит два-три кадра» из ADR-087.\n");
    let out = g.storage(4096 * 4, "out");
    // 8а. Блокирующий оборот: отправить, дождаться, отобразить, прочитать.
    let mut v = Vec::new();
    for _ in 0..(REPS + 5) {
        let t0 = Instant::now();
        let cb = encode(&g, &kit, 1, 1, 1, false);
        g.queue.submit([cb]);
        let _ = g.read_u32(&out, 64);
        v.push(t0.elapsed().as_nanos() as f64);
    }
    v.drain(0..5);
    println!("блокирующий оборот с чтением: {:>9.0} нс", stat(v).0);

    // 8б. Конвейер: на каждом тике отправляем работу и читаем результат
    // тика, отправленного `depth` тиков назад. Никогда не блокируемся
    // на текущем.
    for depth in [1usize, 2, 3] {
        let bufs: Vec<_> = (0..depth + 1).map(|_| g.readback_buf(256)).collect();
        type MapResult = Result<(), wgpu::BufferAsyncError>;
        let mut pending: Vec<Option<std::sync::mpsc::Receiver<MapResult>>> =
            (0..depth + 1).map(|_| None).collect();
        let mut v = Vec::new();
        for tick in 0..(REPS + 10) {
            let t0 = Instant::now();
            let slot = tick % (depth + 1);
            // Забираем то, что отправляли `depth` тиков назад.
            if let Some(rx) = pending[slot].take() {
                // Крутим `poll`, пока отображение не завершится: без пульса
                // обратный вызов не сработает, а `recv` встанет намертво.
                loop {
                    match rx.try_recv() {
                        Ok(r) => {
                            r.expect("ok");
                            break;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            let _ = g.device.poll(wgpu::PollType::Poll);
                        }
                        Err(e) => panic!("{e:?}"),
                    }
                }
                let view = bufs[slot].get_mapped_range(..);
                std::hint::black_box(view[0]);
                drop(view);
                bufs[slot].unmap();
            }
            let mut e = g.device.create_command_encoder(&Default::default());
            {
                let mut p = e.begin_compute_pass(&Default::default());
                p.set_pipeline(&kit.pipes[0]);
                p.set_bind_group(0, &kit.binds[0], &[]);
                p.dispatch_workgroups(64, 1, 1);
            }
            e.copy_buffer_to_buffer(&out, 0, &bufs[slot], 0, 256);
            g.queue.submit([e.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            bufs[slot].map_async(wgpu::MapMode::Read, .., move |r| {
                let _ = tx.send(r);
            });
            pending[slot] = Some(rx);
            // Пульс: без него отображение никогда не завершится.
            let _ = g.device.poll(wgpu::PollType::Poll);
            v.push(t0.elapsed().as_nanos() as f64);
        }
        v.drain(0..10);
        println!(
            "конвейер с отставанием {depth}:   {:>9.0} нс на тик",
            stat(v).0
        );
    }
    g.wait();

    section("9. Загрузка правок в буфер");
    println!("список правок тика: индекс и материал, восемь байт на воксель.\n");
    let up = g.storage(64 * 1024 * 1024, "upload");
    println!(
        "{:>10} {:>12} {:>14} {:>14}",
        "правок", "байт", "нс", "ГиБ/с"
    );
    for edits in [256usize, 1024, 4096, 16384, 65536, 262144] {
        let data: Vec<u32> = (0..edits * 2).map(|i| i as u32).collect();
        let bytes = gpu::as_bytes(&data);
        let mut v = Vec::new();
        for _ in 0..(REPS + 5) {
            let t0 = Instant::now();
            g.queue.write_buffer(&up, 0, bytes);
            g.queue.submit([]);
            v.push(t0.elapsed().as_nanos() as f64);
        }
        v.drain(0..5);
        g.wait();
        let ns = stat(v).0;
        println!(
            "{edits:>10} {:>12} {ns:>14.0} {:>14.1}",
            bytes.len(),
            bytes.len() as f64 / ns
        );
    }
}
