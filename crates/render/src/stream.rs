//! Потоковая подгрузка меша: кольцо столбов чанков вокруг камеры.
//!
//! **Зачем.** Окно боя строило весь меш-слой разом. На карте
//! 1920×960×1920 это 15 376 чанков и 11.5 секунды до первого кадра;
//! километр при ребре 10 см — это 162×81×162 = 2 125 764 чанка,
//! и построить их все нельзя ни при каком железе. Кольцо снимает
//! зависимость от размера карты целиком: работы столько, сколько
//! видно, а видно всегда одинаково.
//!
//! **Устройство.** Единица очереди — **столб чанков**, а не чанк:
//! глубину кольцо ограничивает не расстоянием, а породой (см. [`ring`]).
//!
//! - Столб **в кольце**, если он ближе `build_radius` по горизонтали
//!   к точке, на которую смотрит камера. Осмотр идёт сверху вниз
//!   и **кончается на первом сплошном чанке**: ниже всё закрыто
//!   породой.
//! - Столб **удерживается**, пока он ближе `keep_radius`. Разрыв
//!   между радиусами — гистерезис: без него столб на самой границе
//!   строился бы и освобождался каждый кадр.
//! - **Потолки за кадр** — отдельно на осмотр, на сборку
//!   и на освобождение, потому что стоят они врозь: осмотр чанка —
//!   4 096 сдвигов по битовому объёму мира (7–25 мкс), сборка —
//!   мешинг, раскраска и отдача буфера (1.6 мс). Не влезшее
//!   **откладывается на следующий кадр**, а не бросается
//!   (`CLAUDE.md`), и очередь FIFO, чтобы один дорогой столб
//!   не запирал за собой остальные.
//!
//! Числа живут в `data/render.ron`, а не здесь: радиус кольца
//! подбирают под машину и под сцену, а пересборка крейта стоит
//! десять минут.
//!
//! **Порядок сборки — ближние первыми.** Ничьи разрешаются номером
//! столба, а не порядком обхода: два прогона одной сцены обязаны
//! строить одно и то же в одном и том же порядке, иначе замеры
//! «до» и «после» сравнивать нечем.
//!
//! Здесь нет ни Bevy, ни мира: только сетка чанков и точка. Поэтому
//! всё проверяется headless (`CLAUDE.md`, «тесты до рендера»).

use crate::{ChunkGrid, V3};

/// Числа потокового слоя. Все — из `data/render.ron`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stream {
    /// Радиус кольца сборки **по горизонтали**, мировые воксели.
    pub build_radius: u32,
    /// Радиус удержания. Строго больше радиуса сборки.
    pub keep_radius: u32,
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
    /// Радиусы перепутаны местами: удержание обязано быть не меньше
    /// сборки, иначе гистерезиса нет, а есть дрожание.
    Radii(u32, u32),
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamError::Io(e) => write!(f, "{e}"),
            StreamError::Missing(k) => write!(f, "в файле нет поля {k}"),
            StreamError::Bad(k) => write!(f, "поле {k} — не целое число"),
            StreamError::Radii(b, k) => {
                write!(f, "радиус удержания {k} меньше радиуса сборки {b}")
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
            probe_per_frame: field(text, "probe_per_frame")?,
            build_per_frame: field(text, "build_per_frame")?,
            free_per_frame: field(text, "free_per_frame")?,
        };
        if s.keep_radius < s.build_radius {
            return Err(StreamError::Radii(s.build_radius, s.keep_radius));
        }
        Ok(s)
    }

    pub fn load(path: &std::path::Path) -> Result<Stream, StreamError> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| StreamError::Io(format!("{}: {e}", path.display())))?;
        Stream::from_ron(&s)
    }
}

/// Квадрат **горизонтального** расстояния от точки до столба чанков.
///
/// Квадрат, а не расстояние: корень не нужен ни для сравнения
/// с радиусом, ни для порядка сборки, а стоит он дороже всего
/// остального вместе (`CLAUDE.md`, ADR-071). Плавающая точка здесь
/// разрешена — это рендер.
///
/// Горизонтальное, а не полное: **глубину кольцо ограничивает
/// не расстоянием, а породой** — см. [`ring`].
pub fn column_dist2(g: &ChunkGrid, cx: u32, cz: u32, p: V3) -> f32 {
    let s = g.cs as f32;
    let d = |v: f32, a: f32| {
        let b = a + s;
        if v < a {
            a - v
        } else if v > b {
            v - b
        } else {
            0.0
        }
    };
    let (dx, dz) = (d(p.x, cx as f32 * s), d(p.z, cz as f32 * s));
    dx * dx + dz * dz
}

/// Номер столба чанков: `cz * nx + cx`.
#[inline]
pub fn column(g: &ChunkGrid, cx: u32, cz: u32) -> u32 {
    cz * g.nx + cx
}

/// Столб по номеру.
#[inline]
pub fn column_at(g: &ChunkGrid, c: u32) -> (u32, u32) {
    (c % g.nx, c / g.nx)
}

/// Внутри ли столб круга радиуса `radius` вокруг точки.
pub fn within(g: &ChunkGrid, c: u32, p: V3, radius: f32) -> bool {
    let (cx, cz) = column_at(g, c);
    column_dist2(g, cx, cz, p) <= radius * radius
}

/// Столбы чанков вокруг точки `p`, **ближние первыми**.
///
/// **Кольцо ограничивает глубину не расстоянием, а породой.** Вниз
/// от камеры идёт столб породы до дна мира, источенный пещерами:
/// чанк там неоднороден, значит имеет грани, значит попал бы в сборку
/// — и не виден никогда, потому что закрыт породой сверху. На карте
/// 1920³ таких чанков набиралось 5 437, и очередь сборки от них
/// не пустела вовсе.
///
/// Резать по вертикальному радиусу — значит привязать кольцо к тому,
/// где камера, и получить дыры в рельефе от всякой ошибки в её высоте.
/// Поэтому дно колонки — **первый сплошной чанк сверху**: всё под ним
/// закрыто породой, правило не знает ни о камере, ни о высоте мира,
/// а прокопанная колонка перестаёт быть сплошной сама, и дно едет
/// вниз вместе с забоем.
///
/// Отсюда и единица очереди: столб, а не чанк. Обходится только ящик
/// вокруг точки, а не вся сетка: на километре в ней два миллиона
/// чанков, и полный обход каждый кадр стоил бы больше самой сборки.
pub fn ring(g: &ChunkGrid, p: V3, radius: f32, out: &mut Vec<u32>) {
    out.clear();
    if radius <= 0.0 || g.is_empty() {
        return;
    }
    let cs = g.cs as f32;
    let span = |v: f32, n: u32| -> (u32, u32) {
        let cut = |k: f32| -> u32 {
            if k <= 0.0 {
                0
            } else {
                (k as u32).min(n.saturating_sub(1))
            }
        };
        (cut(((v - radius) / cs).floor()), cut(((v + radius) / cs).floor()))
    };
    let (x0, x1) = span(p.x, g.nx);
    let (z0, z1) = span(p.z, g.nz);
    let r2 = radius * radius;
    // Ничья разрешается номером столба, а не порядком обхода:
    // сортировка устойчивая, и равные расстояния лягут по возрастанию
    // номера.
    let mut keyed: Vec<(f32, u32)> = Vec::new();
    for cz in z0..=z1 {
        for cx in x0..=x1 {
            let d2 = column_dist2(g, cx, cz, p);
            if d2 <= r2 {
                keyed.push((d2, column(g, cx, cz)));
            }
        }
    }
    keyed.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(core::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    out.extend(keyed.into_iter().map(|(_, c)| c));
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
        assert!(s.keep_radius >= s.build_radius);
        assert!(s.probe_per_frame > 0 && s.build_per_frame > 0 && s.free_per_frame > 0);
    }

    /// **Пропущенное поле — ошибка, а не ноль.** Умолчание означало бы,
    /// что радиус кольца всё-таки живёт в коде.
    #[test]
    fn a_missing_field_is_an_error() {
        for key in [
            "build_radius",
            "keep_radius",
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
        let bad = FILE
            .replace("build_radius:", "x_swap:")
            .replace("keep_radius:", "build_radius:")
            .replace("x_swap:", "keep_radius:");
        match Stream::from_ron(&bad) {
            Err(StreamError::Radii(..)) => {}
            other => panic!("перепутанные радиусы приняты: {other:?}"),
        }
    }

    /// **Расстояние считается до столба, а не до его середины.**
    /// Разница в полчанка, и на ней кольцо перестаёт быть кольцом:
    /// дальний угол попадает в кадр раньше, чем середина — в радиус.
    ///
    /// И считается оно **по горизонтали**: высота столба в расстояние
    /// не входит вовсе.
    #[test]
    fn distance_is_measured_horizontally_to_the_column() {
        let g = grid();
        let s = g.cs as f32;
        // Точка внутри столба — ноль, на какой бы высоте она ни была.
        for y in [0.0f32, 500.0, -300.0] {
            let mid = V3::new(4.5 * s, y, 4.5 * s);
            assert_eq!(column_dist2(&g, 4, 4, mid), 0.0);
        }
        // Точка в десяти вокселях от грани — сто.
        let out = V3::new(5.0 * s + 10.0, 0.0, 4.5 * s);
        assert!((column_dist2(&g, 4, 4, out) - 100.0).abs() < 1e-3);
    }

    /// **Кольцо содержит ровно те столбы, что ближе радиуса.** Проверка
    /// против прямого перебора всей сетки: обход ящика — оптимизация,
    /// и она обязана давать то же множество.
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
            for r in [40.0f32, 130.0, 400.0] {
                ring(&g, p, r, &mut got);
                let mut want: Vec<u32> = (0..g.nx * g.nz)
                    .filter(|&c| within(&g, c, p, r))
                    .collect();
                assert_eq!(got.len(), want.len(), "точка {p:?}, радиус {r}: не тот размер");
                let mut have = got.clone();
                have.sort_unstable();
                want.sort_unstable();
                assert_eq!(have, want, "точка {p:?}, радиус {r}: не то множество");
            }
        }
    }

    /// **Ближние строятся первыми, и ничья разрешается номером.**
    /// Порядок — не вкус: без него два прогона одной сцены строят разное
    /// и сравнивать их нечем.
    #[test]
    fn the_ring_is_ordered_near_first() {
        let g = grid();
        let p = V3::new(497.0, 311.0, 503.0);
        let mut got = Vec::new();
        ring(&g, p, 300.0, &mut got);
        assert!(got.len() > 20, "кольцо почти пустое: {}", got.len());
        let mut prev = (-1.0f32, 0u32);
        for &c in got.iter() {
            let (cx, cz) = column_at(&g, c);
            let d2 = column_dist2(&g, cx, cz, p);
            assert!(d2 >= prev.0 - 1e-3, "порядок сбит: {d2} после {}", prev.0);
            if (d2 - prev.0).abs() < 1e-6 {
                assert!(c > prev.1, "ничья разрешена не номером: {c} после {}", prev.1);
            }
            prev = (d2, c);
        }
    }

    /// **Кольцо не выходит за сетку.** Точка у самого края карты —
    /// обычное дело, а выход за край здесь означал бы столб, которого
    /// нет.
    #[test]
    fn the_ring_stays_inside_the_grid() {
        let g = grid();
        let mut got = Vec::new();
        for p in [
            V3::new(-5000.0, -5000.0, -5000.0),
            V3::new(9000.0, 9000.0, 9000.0),
            V3::new(0.0, 0.0, 0.0),
        ] {
            ring(&g, p, 5000.0, &mut got);
            for &c in got.iter() {
                assert!(c < g.nx * g.nz, "столб {c} вне сетки из {}", g.nx * g.nz);
            }
        }
        // Огромный радиус вокруг середины накрывает все столбы.
        ring(&g, V3::new(992.0, 496.0, 992.0), 100_000.0, &mut got);
        assert_eq!(got.len(), (g.nx * g.nz) as usize, "накрыло не всю сетку");
    }

    /// **Высота мира на кольцо не влияет.** Глубину ограничивает
    /// порода, а не расстояние, и мир вдесятеро глубже даёт то же
    /// кольцо — иначе километр упирался бы в собственную толщину.
    #[test]
    fn the_ring_does_not_grow_with_depth() {
        let shallow = ChunkGrid { nx: 162, ny: 4, nz: 162, cs: 62 };
        let deep = ChunkGrid { nx: 162, ny: 81, nz: 162, cs: 62 };
        let p = V3::new(5000.0, 120.0, 5000.0);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        ring(&shallow, p, 1100.0, &mut a);
        ring(&deep, p, 1100.0, &mut b);
        assert!(a.len() > 500, "кольцо почти пустое: {}", a.len());
        assert_eq!(a, b, "мир глубже дал другое кольцо");
    }

    /// **Кольцо не зависит от размера карты.** Ровно это делает километр
    /// возможным: работа считается от радиуса, а не от сетки.
    ///
    /// Сравниваются **координаты** столбов, а не их номера: номер
    /// зависит от ширины сетки и совпасть не может.
    #[test]
    fn the_ring_does_not_grow_with_the_map() {
        let small = ChunkGrid { nx: 32, ny: 16, nz: 32, cs: 62 };
        let huge = ChunkGrid { nx: 162, ny: 81, nz: 162, cs: 62 };
        let p = V3::new(600.0, 400.0, 600.0);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        ring(&small, p, 250.0, &mut a);
        ring(&huge, p, 250.0, &mut b);
        let at = |g: &ChunkGrid, v: &[u32]| -> Vec<(u32, u32)> {
            v.iter().map(|&c| column_at(g, c)).collect()
        };
        assert!(a.len() > 30, "кольцо почти пустое: {}", a.len());
        assert_eq!(
            at(&small, &a),
            at(&huge, &b),
            "то же кольцо на карте побольше вышло другим"
        );
    }

    /// **Гистерезис шире сборки.** Столб, только что построенный
    /// на границе кольца, обязан удерживаться — иначе он освободится
    /// в том же кадре.
    #[test]
    fn the_keep_ring_contains_the_build_ring() {
        let g = grid();
        let s = Stream::from_ron(FILE).expect("файл");
        let p = V3::new(700.0, 400.0, 700.0);
        let mut got = Vec::new();
        ring(&g, p, s.build_radius as f32, &mut got);
        assert!(!got.is_empty(), "кольцо пустое — проверять нечего");
        for &c in got.iter() {
            assert!(
                within(&g, c, p, s.keep_radius as f32),
                "столб {c} строится и тут же освобождается"
            );
        }
    }
}
