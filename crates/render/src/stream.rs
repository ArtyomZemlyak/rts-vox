//! Потоковая подгрузка меша: кольцо чанков вокруг камеры.
//!
//! **Зачем.** Окно боя строило весь меш-слой разом. На карте
//! 1920×960×1920 это 15 376 чанков и двенадцать минут ожидания;
//! километр при ребре 10 см — это 162×81×162 = 2 125 764 чанка,
//! и построить их все нельзя ни при каком железе. Кольцо снимает
//! зависимость от размера карты целиком: работы столько, сколько
//! видно, а видно всегда одинаково.
//!
//! **Устройство.** Три множества и три потолка.
//!
//! - Чанк **в кольце**, если его параллелепипед ближе `build_radius`
//!   по горизонтали и `build_height` по вертикали к точке, на которую
//!   смотрит камера. Такой чанк обязан быть построен.
//! - Чанк **удерживается**, пока он в пределах `keep_radius`
//!   и `keep_height`. Разрыв между парами — гистерезис: без него чанк
//!   на самой границе строился бы и освобождался каждый кадр.
//! - **Потолки за кадр** — отдельно на осмотр, на сборку
//!   и на освобождение, потому что стоят они врозь: осмотр
//!   однородного чанка это 4 096 сдвигов по битовому объёму мира,
//!   сборка — мешинг, раскраска и отдача буфера. Не влезшее
//!   **откладывается на следующий кадр**, а не бросается
//!   (`CLAUDE.md`), и очередь FIFO, чтобы один дорогой чанк не запирал
//!   за собой остальные.
//!
//! **Кольцо цилиндр, а не шар** (`within`): вниз от камеры идёт столб
//! породы до дна мира, источенной пещерами, и шар тянул бы его в сборку
//! целиком — 5 437 невидимых чанков на карте 1920³.
//!
//! Числа живут в `data/render.ron`, а не здесь: радиус кольца
//! подбирают под машину и под сцену, а пересборка крейта стоит
//! две с половиной минуты.
//!
//! **Порядок сборки — ближние первыми.** Ничьи разрешаются линейным
//! индексом чанка, а не порядком обхода: два прогона одной сцены
//! обязаны строить одно и то же в одном и том же порядке, иначе
//! замеры «до» и «после» сравнивать нечем.
//!
//! Здесь нет ни Bevy, ни мира: только сетка чанков и точка. Поэтому
//! всё проверяется headless (`CLAUDE.md`, «тесты до рендера»).

use crate::{ChunkGrid, V3};

/// Числа потокового слоя. Все — из `data/render.ron`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stream {
    /// Радиус кольца сборки **по горизонтали**, мировые воксели.
    pub build_radius: u32,
    /// Радиус удержания по горизонтали. Строго больше радиуса сборки.
    pub keep_radius: u32,
    /// Полувысота кольца сборки, мировые воксели.
    pub build_height: u32,
    /// Полувысота удержания. Строго больше полувысоты сборки.
    pub keep_height: u32,
    /// Сколько чанков **осматривается** за кадр.
    pub probe_per_frame: u32,
    /// Сколько чанков **строится** за кадр.
    pub build_per_frame: u32,
    /// Сколько чанков **освобождается** за кадр.
    pub free_per_frame: u32,
}

/// Почему файл не прочитался. Пропущенное поле — ошибка, а не ноль:
/// умолчание означало бы, что число всё-таки живёт в коде.
#[derive(Debug)]
pub enum StreamError {
    Io(String),
    Missing(&'static str),
    Bad(&'static str),
    /// Размеры перепутаны местами: удержание обязано быть не меньше
    /// сборки, иначе гистерезиса нет, а есть дрожание.
    Radii(&'static str, u32, u32),
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamError::Io(e) => write!(f, "{e}"),
            StreamError::Missing(k) => write!(f, "в файле нет поля {k}"),
            StreamError::Bad(k) => write!(f, "поле {k} — не целое число"),
            StreamError::Radii(what, b, k) => {
                write!(f, "{what}: удержание {k} меньше сборки {b}")
            }
        }
    }
}

/// Прочитать целое поле вида `имя: 1234,`.
///
/// Разбор нарочно примитивный: пять целых чисел не стоят зависимости
/// от разборщика RON в рендере. Ровно тем же приёмом `look` сверяет
/// свою палитру с `data/materials.ron`.
fn field(text: &str, key: &'static str) -> Result<u32, StreamError> {
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("//") {
            continue;
        }
        let Some(rest) = t.strip_prefix(key) else { continue };
        let Some(rest) = rest.trim_start().strip_prefix(':') else { continue };
        let num: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        return num.parse::<u32>().map_err(|_| StreamError::Bad(key));
    }
    Err(StreamError::Missing(key))
}

impl Stream {
    pub fn from_ron(text: &str) -> Result<Stream, StreamError> {
        let s = Stream {
            build_radius: field(text, "build_radius")?,
            keep_radius: field(text, "keep_radius")?,
            build_height: field(text, "build_height")?,
            keep_height: field(text, "keep_height")?,
            probe_per_frame: field(text, "probe_per_frame")?,
            build_per_frame: field(text, "build_per_frame")?,
            free_per_frame: field(text, "free_per_frame")?,
        };
        if s.keep_radius < s.build_radius {
            return Err(StreamError::Radii("радиус", s.build_radius, s.keep_radius));
        }
        if s.keep_height < s.build_height {
            return Err(StreamError::Radii("высота", s.build_height, s.keep_height));
        }
        Ok(s)
    }

    pub fn load(path: &std::path::Path) -> Result<Stream, StreamError> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| StreamError::Io(format!("{}: {e}", path.display())))?;
        Stream::from_ron(&s)
    }
}

/// Расстояние от точки до параллелепипеда чанка: **по горизонтали
/// квадрат, по вертикали сама величина**.
///
/// Квадрат, а не расстояние: корень не нужен ни для сравнения
/// с радиусом, ни для порядка сборки, а стоит он дороже всего
/// остального вместе (`CLAUDE.md`, ADR-071). Плавающая точка здесь
/// разрешена — это рендер.
pub fn chunk_span(g: &ChunkGrid, i: usize, p: V3) -> (f32, f32) {
    let (lo, hi) = g.aabb(i);
    let d = |v: f32, a: f32, b: f32| {
        if v < a {
            a - v
        } else if v > b {
            v - b
        } else {
            0.0
        }
    };
    let (dx, dy, dz) = (d(p.x, lo.x, hi.x), d(p.y, lo.y, hi.y), d(p.z, lo.z, hi.z));
    (dx * dx + dz * dz, dy)
}

/// Квадрат полного расстояния — только для порядка сборки.
pub fn chunk_dist2(g: &ChunkGrid, i: usize, p: V3) -> f32 {
    let (h2, dy) = chunk_span(g, i, p);
    h2 + dy * dy
}

/// Внутри ли чанк кольца: ближе `radius` по горизонтали **и** ближе
/// `height` по вертикали.
///
/// **Кольцо — цилиндр, а не шар, и это не мелочь.** Радиус в тысячу
/// вокселей вокруг камеры захватывает вниз весь столб породы до дна
/// мира, а порода источена пещерами: чанк там неоднороден, значит
/// имеет грани, значит попадает в очередь сборки — и не виден
/// никогда, потому что закрыт породой. На карте 1920³ таких чанков
/// набралось 5 437, и очередь сборки от них не пустела вовсе.
///
/// Плата — предел: яма глубже `height` под точкой взгляда покажет
/// не все свои стены. Камера RTS смотрит на поверхность, и предел
/// задаётся данными.
pub fn within(g: &ChunkGrid, i: usize, p: V3, radius: f32, height: f32) -> bool {
    let (h2, dy) = chunk_span(g, i, p);
    h2 <= radius * radius && dy <= height
}

/// Чанки кольца вокруг точки `p`, **ближние первыми**.
///
/// Обходится только ящик вокруг точки, а не вся сетка: на километре
/// в ней два миллиона чанков, и полный обход каждый кадр стоил бы
/// больше самой сборки.
pub fn ring(g: &ChunkGrid, p: V3, radius: f32, height: f32, out: &mut Vec<u32>) {
    out.clear();
    if radius <= 0.0 || height < 0.0 || g.is_empty() {
        return;
    }
    let cs = g.cs as f32;
    let span = |v: f32, n: u32, r: f32| -> (u32, u32) {
        let cut = |k: f32| -> u32 {
            if k <= 0.0 {
                0
            } else {
                (k as u32).min(n.saturating_sub(1))
            }
        };
        (cut(((v - r) / cs).floor()), cut(((v + r) / cs).floor()))
    };
    let (x0, x1) = span(p.x, g.nx, radius);
    let (y0, y1) = span(p.y, g.ny, height);
    let (z0, z1) = span(p.z, g.nz, radius);
    // Порядок обхода растровый, и ничья разрешается линейным индексом,
    // а не им: сортировка устойчивая, и равные расстояния лягут
    // по возрастанию индекса.
    let mut keyed: Vec<(f32, u32)> = Vec::new();
    for cz in z0..=z1 {
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                let i = g.index(cx, cy, cz);
                if within(g, i, p, radius, height) {
                    keyed.push((chunk_dist2(g, i, p), i as u32));
                }
            }
        }
    }
    keyed.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(core::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    out.extend(keyed.into_iter().map(|(_, i)| i));
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = include_str!("../../../data/render.ron");

    fn grid() -> ChunkGrid {
        ChunkGrid { nx: 32, ny: 16, nz: 32, cs: 62 }
    }

    /// **Файл в репозитории читается.** Разбор, который не проверен
    /// на настоящем файле, проверяет только сам себя.
    #[test]
    fn the_shipped_file_parses() {
        let s = Stream::from_ron(FILE).expect("data/render.ron не разобрался");
        assert!(s.build_radius > 0, "радиус сборки нулевой");
        assert!(s.build_height > 0, "высота сборки нулевая");
        assert!(s.keep_radius >= s.build_radius);
        assert!(s.keep_height >= s.build_height);
        assert!(s.probe_per_frame > 0 && s.build_per_frame > 0 && s.free_per_frame > 0);
    }

    /// **Пропущенное поле — ошибка, а не ноль.** Умолчание означало бы,
    /// что радиус кольца всё-таки живёт в коде.
    #[test]
    fn a_missing_field_is_an_error() {
        for key in [
            "build_radius",
            "keep_radius",
            "build_height",
            "keep_height",
            "probe_per_frame",
            "build_per_frame",
            "free_per_frame",
        ] {
            let cut = FILE.replace(&format!("{key}:"), &format!("// вырезано {key}="));
            assert!(cut != FILE, "в файле нет поля {key} — тест ничего не режет");
            assert!(
                Stream::from_ron(&cut).is_err(),
                "файл без {key} разобрался: у поля есть умолчание"
            );
        }
    }

    /// **Радиус удержания меньше радиуса сборки — ошибка.** Иначе чанк
    /// на границе строится и освобождается каждый кадр, и потолок
    /// работы уходит целиком на дрожание.
    #[test]
    fn swapped_radii_are_refused() {
        for (a, b) in [("build_radius", "keep_radius"), ("build_height", "keep_height")] {
            let bad = FILE
                .replace(&format!("{a}:"), "x_swap:")
                .replace(&format!("{b}:"), &format!("{a}:"))
                .replace("x_swap:", &format!("{b}:"));
            match Stream::from_ron(&bad) {
                Err(StreamError::Radii(..)) => {}
                other => panic!("перепутанные {a} и {b} приняты: {other:?}"),
            }
        }
    }

    /// **Кольцо — цилиндр, а не шар.** Под камерой лежит столб породы
    /// до дна мира, источенный пещерами: шар тянет его в очередь сборки
    /// целиком, и виден он не бывает никогда. Разница не в аккуратности,
    /// а в том, пустеет очередь или нет.
    #[test]
    fn the_ring_is_a_cylinder_not_a_sphere() {
        let g = ChunkGrid { nx: 32, ny: 16, nz: 32, cs: 62 };
        let p = V3::new(992.0, 496.0, 992.0);
        let (mut tall, mut flat) = (Vec::new(), Vec::new());
        ring(&g, p, 400.0, 400.0, &mut tall);
        ring(&g, p, 400.0, 100.0, &mut flat);
        assert!(
            flat.len() * 2 < tall.len(),
            "низкое кольцо не уже высокого: {} против {}",
            flat.len(),
            tall.len()
        );
        for &i in flat.iter() {
            assert!(tall.contains(&i), "чанк {i} есть в низком кольце и нет в высоком");
        }
        // Чанк прямо под камерой, ниже полувысоты, в кольцо не входит.
        let deep = g.index(16, 0, 16);
        assert!(within(&g, deep, p, 400.0, 500.0), "низ не достаётся и высоким кольцом");
        assert!(!within(&g, deep, p, 400.0, 100.0), "низ попал в низкое кольцо");
    }

    /// **Расстояние считается до ящика, а не до его центра.** Разница
    /// в полчанка, и на ней кольцо перестаёт быть кольцом: дальний угол
    /// чанка попадает в кадр раньше, чем его середина — в радиус.
    #[test]
    fn distance_is_measured_to_the_box() {
        let g = grid();
        let i = g.index(4, 2, 4);
        let (lo, hi) = g.aabb(i);
        // Точка внутри ящика — ноль.
        let mid = V3::new(
            (lo.x + hi.x) * 0.5,
            (lo.y + hi.y) * 0.5,
            (lo.z + hi.z) * 0.5,
        );
        assert_eq!(chunk_dist2(&g, i, mid), 0.0);
        // Точка в десяти вокселях от грани — сто.
        let out = V3::new(hi.x + 10.0, mid.y, mid.z);
        assert!((chunk_dist2(&g, i, out) - 100.0).abs() < 1e-3);
        // Горизонталь и вертикаль считаются врозь: кольцо цилиндр.
        let up = V3::new(mid.x, hi.y + 7.0, mid.z);
        let (h2, dy) = chunk_span(&g, i, up);
        assert_eq!(h2, 0.0);
        assert!((dy - 7.0).abs() < 1e-3);
    }

    /// **Кольцо содержит ровно то, что ближе радиуса.** Проверка против
    /// прямого перебора всей сетки: обход ящика — оптимизация, и она
    /// обязана давать то же множество.
    #[test]
    fn the_ring_is_exactly_what_is_near() {
        let g = grid();
        let mut got = Vec::new();
        for p in [
            V3::new(500.0, 300.0, 500.0),
            V3::new(0.0, 0.0, 0.0),
            V3::new(1983.0, 991.0, 1983.0),
            V3::new(-200.0, 100.0, 900.0),
        ] {
            for (r, h) in [(40.0f32, 40.0f32), (130.0, 300.0), (400.0, 90.0)] {
                ring(&g, p, r, h, &mut got);
                let mut want: Vec<u32> = (0..g.len())
                    .filter(|&i| within(&g, i, p, r, h))
                    .map(|i| i as u32)
                    .collect();
                assert_eq!(got.len(), want.len(), "точка {p:?}, радиус {r}: не тот размер");
                let mut have = got.clone();
                have.sort_unstable();
                want.sort_unstable();
                assert_eq!(have, want, "точка {p:?}, радиус {r}: не то множество");
            }
        }
    }

    /// **Ближние строятся первыми, и ничья разрешается индексом.**
    /// Порядок — не вкус: без него два прогона одной сцены строят разное
    /// и сравнивать их нечем.
    #[test]
    fn the_ring_is_ordered_near_first() {
        let g = grid();
        let p = V3::new(497.0, 311.0, 503.0);
        let mut got = Vec::new();
        ring(&g, p, 300.0, 300.0, &mut got);
        assert!(got.len() > 20, "кольцо почти пустое: {}", got.len());
        let mut prev = (-1.0f32, 0u32);
        for &i in got.iter() {
            let d2 = chunk_dist2(&g, i as usize, p);
            assert!(d2 >= prev.0 - 1e-3, "порядок сбит: {d2} после {}", prev.0);
            if (d2 - prev.0).abs() < 1e-6 {
                assert!(i > prev.1, "ничья разрешена не индексом: {i} после {}", prev.1);
            }
            prev = (d2, i);
        }
    }

    /// **Кольцо не выходит за сетку.** Точка у самого края карты —
    /// обычное дело, а выход за край здесь означал бы индекс чанка,
    /// которого нет.
    #[test]
    fn the_ring_stays_inside_the_grid() {
        let g = grid();
        let mut got = Vec::new();
        for p in [
            V3::new(-5000.0, -5000.0, -5000.0),
            V3::new(9000.0, 9000.0, 9000.0),
            V3::new(0.0, 0.0, 0.0),
        ] {
            ring(&g, p, 5000.0, 5000.0, &mut got);
            for &i in got.iter() {
                assert!((i as usize) < g.len(), "чанк {i} вне сетки из {}", g.len());
            }
        }
        // Огромный радиус вокруг середины накрывает всё.
        ring(&g, V3::new(992.0, 496.0, 992.0), 100_000.0, 100_000.0, &mut got);
        assert_eq!(got.len(), g.len(), "огромное кольцо накрыло не всю сетку");
    }

    /// **Кольцо не зависит от размера карты.** Ровно это делает километр
    /// возможным: работа считается от радиуса, а не от сетки.
    ///
    /// Сравниваются **координаты** чанков, а не их номера: линейный
    /// индекс зависит от размеров сетки, и совпасть у карты в 192 метра
    /// и у карты в километр он не может.
    #[test]
    fn the_ring_does_not_grow_with_the_map() {
        let small = ChunkGrid { nx: 32, ny: 16, nz: 32, cs: 62 };
        let huge = ChunkGrid { nx: 162, ny: 81, nz: 162, cs: 62 };
        let p = V3::new(600.0, 400.0, 600.0);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        ring(&small, p, 250.0, 250.0, &mut a);
        ring(&huge, p, 250.0, 250.0, &mut b);
        let at = |g: &ChunkGrid, v: &[u32]| -> Vec<(u32, u32, u32)> {
            v.iter().map(|&i| g.coords(i as usize)).collect()
        };
        assert!(a.len() > 30, "кольцо почти пустое: {}", a.len());
        assert_eq!(
            at(&small, &a),
            at(&huge, &b),
            "то же кольцо на карте побольше вышло другим"
        );
    }

    /// **Гистерезис шире сборки.** Чанк, только что построенный
    /// на границе кольца, обязан удерживаться — иначе он освободится
    /// в том же кадре.
    #[test]
    fn the_keep_ring_contains_the_build_ring() {
        let g = grid();
        let s = Stream::from_ron(FILE).expect("файл");
        let p = V3::new(700.0, 400.0, 700.0);
        let mut got = Vec::new();
        ring(&g, p, s.build_radius as f32, s.build_height as f32, &mut got);
        assert!(!got.is_empty(), "кольцо пустое — проверять нечего");
        for &i in got.iter() {
            assert!(
                within(&g, i as usize, p, s.keep_radius as f32, s.keep_height as f32),
                "чанк {i} строится и тут же освобождается"
            );
        }
    }
}
