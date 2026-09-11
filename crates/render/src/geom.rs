//! Квады мешинга — в вершины и индексы.
//!
//! Это **логика, а не рисование**, и потому проверяется headless: окно
//! из терминала не запустить, а перепутанный обход углов виден тестом
//! на векторное произведение (`CLAUDE.md`: тесты до рендера).
//!
//! Ошибиться здесь легко и незаметно: неверный обход даёт треугольник,
//! отвёрнутый от камеры, и чанк просто пропадает — ни ошибки,
//! ни предупреждения.

use crate::look::{self, Class};
use meshing::{Quad, CS};
use voxel::store::Mat;

/// Буферы одного чанка. Переиспользуются: выделение на чанк мерило бы
/// аллокатор, а чанков тысячи.
#[derive(Default)]
pub struct MeshBuf {
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[f32; 3]>,
    pub idx: Vec<u32>,
    /// Цвет вершины. У юнитов несёт роль воксела (ADR-172), у рельефа —
    /// материал (ADR-175). Пуст только у неокрашенного разворота.
    pub col: Vec<[f32; 4]>,
}

impl MeshBuf {
    pub fn clear(&mut self) {
        self.pos.clear();
        self.nrm.clear();
        self.idx.clear();
        self.col.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.idx.is_empty()
    }
}

/// Ось грани и её знак: `face = ось * 2 + направление`, где ноль —
/// положительная сторона (`meshing`).
#[inline]
fn axis_dir(face: u8) -> (usize, bool) {
    ((face / 2) as usize, face % 2 == 0)
}

/// Воксель, которому принадлежит грань, в паддинговой системе чанка
/// `1..=CS`.
///
/// Повторяет `meshing::owner`, который приватен. Повторение проверено
/// не глазами: `painted_colours_match_the_world` красит настоящий мир
/// и сверяет цвет каждой вершины с материалом под ней — перепутанные
/// оси разъезжаются на первом же неоднородном чанке.
#[inline]
fn owner(axis: usize, s: usize, a: usize, b: usize) -> (usize, usize, usize) {
    match axis {
        0 => (s, b, a), // колонка по X индексирована (z, y)
        1 => (a, s, b), // по Y — (x, z)
        _ => (b, a, s), // по Z — (y, x)
    }
}

/// Геометрия прямоугольной грани: четыре вершины и два треугольника.
///
/// Координаты квада приходят в паддинговой системе `1..=CS`, поэтому
/// единица вычитается.
#[allow(clippy::too_many_arguments)]
fn push_quad(
    axis: usize,
    positive: bool,
    slice: usize,
    a: usize,
    b: usize,
    w: usize,
    h: usize,
    origin: [f32; 3],
    col: Option<[f32; 4]>,
    out: &mut MeshBuf,
) {
    let (la, lb, ls) = (a as f32 - 1.0, b as f32 - 1.0, slice as f32 - 1.0);
    // Грань лежит между вокселем и его соседом: у положительной
    // стороны это дальний край воксела, у отрицательной — ближний.
    let plane = ls + if positive { 1.0 } else { 0.0 };
    let (w, h) = (w as f32, h as f32);

    // Начало квада и два направления в плоскости. Пара выбрана так,
    // что их векторное произведение даёт **положительную** ось:
    // отсюда и обход углов.
    let (p0, u, v) = match axis {
        0 => ([plane, lb, la], [0.0, h, 0.0], [0.0, 0.0, w]),
        1 => ([la, plane, lb], [0.0, 0.0, h], [w, 0.0, 0.0]),
        _ => ([lb, la, plane], [h, 0.0, 0.0], [0.0, w, 0.0]),
    };
    let add = |a: [f32; 3], b: [f32; 3]| [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
    let at = |p: [f32; 3]| [p[0] + origin[0], p[1] + origin[1], p[2] + origin[2]];
    let c0 = at(p0);
    let c1 = at(add(p0, u));
    let c2 = at(add(add(p0, u), v));
    let c3 = at(add(p0, v));

    let mut n = [0.0f32; 3];
    n[axis] = if positive { 1.0 } else { -1.0 };

    let base = out.pos.len() as u32;
    out.pos.extend_from_slice(&[c0, c1, c2, c3]);
    out.nrm.extend_from_slice(&[n, n, n, n]);
    if let Some(c) = col {
        out.col.extend_from_slice(&[c, c, c, c]);
    }
    if positive {
        out.idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    } else {
        // Та же четвёрка, обход наоборот: иначе грань смотрит внутрь.
        out.idx.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
    }
}

/// Развернуть квады чанка в треугольники, без цвета.
///
/// `origin` — начало чанка в вокселях мира.
pub fn quads_to_mesh(quads: &[Quad], origin: [f32; 3], out: &mut MeshBuf) {
    for q in quads {
        let (axis, positive) = axis_dir(q.face);
        push_quad(
            axis,
            positive,
            q.slice as usize,
            q.a as usize,
            q.b as usize,
            q.w as usize,
            q.h as usize,
            origin,
            None,
            out,
        );
    }
}

/// Буферы раскраски. Как и `MeshBuf`, заводятся один раз: квадов
/// в чанке тысячи, и выделение на квад мерило бы аллокатор.
#[derive(Default)]
pub struct Painter {
    mats: Vec<Mat>,
    used: Vec<bool>,
}

/// Развернуть квады чанка, покрасив их материалом мира.
///
/// Зачем отдельный проход. Слияние в мешере идёт по **битовому объёму**
/// (ADR-068): занят воксель или нет. Материал ему не виден вовсе —
/// инкрементальный мешер кладёт в `Quad::mat` единицу всегда
/// (`meshing/src/incr.rs`), а разделение по материалам там не
/// поддержано намеренно. Значит один квад может лежать и на почве,
/// и на руде, и покрасить его целиком — соврать про то, где копать.
///
/// Поэтому квад досматривается здесь: материалы его вокселей читаются
/// из мира и склеиваются обратно тем же жадным правилом, что у мешера.
/// Однородный квад — а их подавляющее большинство — проходит быстрым
/// путём и остаётся одним квадом; разрезаются только те, что лежат
/// на границе материалов.
///
/// `origin` сдвигает геометрию, `at` — начало чанка **в вокселях
/// мира**: геометрия чанка локальная, а материал ищется в мире.
/// Выход разложен по классам поверхности (`look::Class`): у них разная
/// шероховатость, а её вершина нести не может.
pub fn quads_to_mesh_painted<F: Fn(u32, u32, u32) -> Mat>(
    quads: &[Quad],
    origin: [f32; 3],
    at: [u32; 3],
    mat_at: F,
    p: &mut Painter,
    out: &mut [MeshBuf; look::CLASSES],
) {
    for q in quads {
        let (axis, positive) = axis_dir(q.face);
        let (s, qa, qb) = (q.slice as usize, q.a as usize, q.b as usize);
        let (w, h) = (q.w as usize, q.h as usize);

        // Материалы всех вокселей квада — `w` строк по `h` штук.
        //
        // Каждый воксель спрашивается **ровно один раз**: обращение
        // к миру здесь самое дорогое, что есть (в нетронутом чанке это
        // вызов генератора), и лишняя сверка с первым элементом внутри
        // цикла стоила заметно дороже отдельного прохода по готовому
        // массиву, который весь лежит в кэше.
        p.mats.clear();
        p.mats.reserve(w * h);
        for r in 0..w {
            for c in 0..h {
                let (px, py, pz) = owner(axis, s, qa + r, qb + c);
                p.mats.push(mat_at(
                    at[0] + px as u32 - 1,
                    at[1] + py as u32 - 1,
                    at[2] + pz as u32 - 1,
                ));
            }
        }
        let first = p.mats[0];
        let uniform = p.mats.iter().all(|&m| m == first);

        let mut emit = |a: usize, b: usize, w: usize, h: usize, m: Mat| {
            let buf = &mut out[look::class(m) as usize];
            push_quad(axis, positive, s, a, b, w, h, origin, Some(look::colour(m)), buf);
        };

        if uniform {
            emit(qa, qb, w, h, first);
            continue;
        }

        // Неоднородный квад: то же жадное слияние, что у мешера, но
        // внутри одного квада и с материалом вместо бита.
        p.used.clear();
        p.used.resize(w * h, false);
        for r in 0..w {
            let mut c = 0usize;
            while c < h {
                if p.used[r * h + c] {
                    c += 1;
                    continue;
                }
                let m = p.mats[r * h + c];
                let mut run = 1usize;
                while c + run < h && !p.used[r * h + c + run] && p.mats[r * h + c + run] == m {
                    run += 1;
                }
                let mut span = 1usize;
                'down: while r + span < w {
                    for k in 0..run {
                        let i = (r + span) * h + c + k;
                        if p.used[i] || p.mats[i] != m {
                            break 'down;
                        }
                    }
                    span += 1;
                }
                for rr in r..r + span {
                    for k in 0..run {
                        p.used[rr * h + c + k] = true;
                    }
                }
                emit(qa + r, qb + c, span, run, m);
                c += run;
            }
        }
    }
}

/// Шесть направлений соседа: `(dx, dy, dz)` и нормаль той же грани.
const SIDES: [([i32; 3], [f32; 3]); 6] = [
    ([1, 0, 0], [1.0, 0.0, 0.0]),
    ([-1, 0, 0], [-1.0, 0.0, 0.0]),
    ([0, 1, 0], [0.0, 1.0, 0.0]),
    ([0, -1, 0], [0.0, -1.0, 0.0]),
    ([0, 0, 1], [0.0, 0.0, 1.0]),
    ([0, 0, -1], [0.0, 0.0, -1.0]),
];

/// Какую часть тома разворачивать.
///
/// Корпус и башня едут одним телом, но смотрят в разные стороны:
/// корпус по движению, башня в цель (ADR-176). Разные повороты —
/// разные сущности, значит и меши разные.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slice {
    /// Весь том целиком.
    All,
    /// Ниже линии башни — корпус.
    Hull,
    /// От линии башни и выше — башня со стволом.
    Turret,
}

impl Slice {
    /// Входит ли слой `y` в эту часть.
    fn takes(self, y: usize, turret_y: usize) -> bool {
        match self {
            Slice::All => true,
            // Башни нет — весь том корпус. Иначе безбашенное тело
            // исчезло бы с экрана целиком.
            Slice::Hull => turret_y == 0 || y < turret_y,
            Slice::Turret => turret_y != 0 && y >= turret_y,
        }
    }
}

/// Развернуть воксельный том юнита в треугольники.
///
/// Грань ставится там, где соседа нет: внутренние грани не видны
/// и стоят ровно столько же, сколько видимые. Сосед считается **по
/// всему тому**, а не по своей части: иначе разрез корпуса и башни
/// оброс бы крышкой и днищем, которых никто никогда не увидит.
///
/// Цвет берётся у **роли воксела**, а не выдумывается: броня красится
/// стороной, орудие и двигатель — своим. Так силуэт читается, и видно,
/// куда попали (`docs/05-units.md`).
///
/// Слияния квадов здесь нет намеренно: том юнита — это тысячи вокселей,
/// а не миллионы, и строится он **один раз на тип**. Городить ради
/// него второй мешер значит держать два кода одной задачи.
pub fn volume_to_mesh(
    b: &voxel::unit::UnitBody,
    tint: [f32; 3],
    scale: f32,
    part: Slice,
    out: &mut MeshBuf,
) {
    let (nx, ny, nz) = (b.vol.nx as usize, b.vol.ny as usize, b.vol.nz as usize);
    let ty = b.turret_y as usize;
    // Том ставится серединой в начало координат: юнит вращается вокруг
    // себя, а не вокруг угла. Середина берётся у **всего** тома, не
    // у части: башня обязана вращаться вокруг оси корпуса.
    let half = [nx as f32 * 0.5, 0.0, nz as f32 * 0.5];
    for z in 0..nz {
        for y in 0..ny {
            if !part.takes(y, ty) {
                continue;
            }
            for x in 0..nx {
                if !b.vol.get(x, y, z) {
                    continue;
                }
                let c = look::to_linear(look::part_srgb(b.part_at(x, y, z), tint));
                for (d, n) in SIDES {
                    let (sx, sy, sz) = (x as i32 + d[0], y as i32 + d[1], z as i32 + d[2]);
                    let inside = sx >= 0
                        && sy >= 0
                        && sz >= 0
                        && (sx as usize) < nx
                        && (sy as usize) < ny
                        && (sz as usize) < nz;
                    if inside && b.vol.get(sx as usize, sy as usize, sz as usize) {
                        continue;
                    }
                    push_face([x as f32, y as f32, z as f32], half, scale, d, n, c, out);
                }
            }
        }
    }
}

/// Одна грань воксела: четыре вершины и два треугольника.
fn push_face(
    at: [f32; 3],
    half: [f32; 3],
    scale: f32,
    d: [i32; 3],
    n: [f32; 3],
    c: [f32; 4],
    out: &mut MeshBuf,
) {
    // Ось грани и её сторона.
    let axis = if d[0] != 0 {
        0
    } else if d[1] != 0 {
        1
    } else {
        2
    };
    let positive = d[axis] > 0;
    let mut p0 = [at[0], at[1], at[2]];
    if positive {
        p0[axis] += 1.0;
    }
    // Два направления в плоскости грани.
    let (u, v) = match axis {
        0 => ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        1 => ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        _ => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    };
    let at_world = |p: [f32; 3]| {
        [
            (p[0] - half[0]) * scale,
            (p[1] - half[1]) * scale,
            (p[2] - half[2]) * scale,
        ]
    };
    let add = |a: [f32; 3], b: [f32; 3]| [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
    let c0 = at_world(p0);
    let c1 = at_world(add(p0, u));
    let c2 = at_world(add(add(p0, u), v));
    let c3 = at_world(add(p0, v));
    let base = out.pos.len() as u32;
    out.pos.extend_from_slice(&[c0, c1, c2, c3]);
    out.nrm.extend_from_slice(&[n, n, n, n]);
    out.col.extend_from_slice(&[c, c, c, c]);
    if positive {
        out.idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    } else {
        out.idx.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
    }
}

/// Начало чанка в вокселях по его номеру в сетке.
pub fn chunk_origin(i: usize, nx: u32, ny: u32) -> [f32; 3] {
    let i = i as u32;
    let (cx, cy, cz) = (i % nx, (i / nx) % ny, i / (nx * ny));
    let s = CS as f32;
    [cx as f32 * s, cy as f32 * s, cz as f32 * s]
}

/// То же начало чанка, но в целых вокселях: материал ищется в мире
/// по целым индексам, и `f32` там значил бы округление.
pub fn chunk_at(i: usize, nx: u32, ny: u32) -> [u32; 3] {
    let i = i as u32;
    let (cx, cy, cz) = (i % nx, (i / nx) % ny, i / (nx * ny));
    let s = CS as u32;
    [cx * s, cy * s, cz * s]
}

/// Сколько классов поверхности — столько буферов.
pub fn class_bufs() -> [MeshBuf; look::CLASSES] {
    [MeshBuf::default(), MeshBuf::default()]
}

/// Индекс класса — чтобы вызывающий не гадал, где какой буфер.
pub const ROCK: usize = Class::Rock as usize;
pub const ORE: usize = Class::Ore as usize;

#[cfg(test)]
mod tests {
    use super::*;

    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }

    /// **Каждый треугольник смотрит наружу.**
    ///
    /// Векторное произведение сторон обязано совпасть по знаку
    /// с нормалью грани. Ошибка здесь не падает и не ругается — чанк
    /// просто исчезает с экрана, и найти это без теста нечем.
    #[test]
    fn every_triangle_faces_outwards() {
        let mut buf = MeshBuf::default();
        let quads: Vec<Quad> = (0..6)
            .map(|face| Quad { face, slice: 4, a: 3, b: 2, w: 2, h: 3, mat: 1 })
            .collect();
        quads_to_mesh(&quads, [0.0, 0.0, 0.0], &mut buf);
        assert_eq!(buf.idx.len(), 6 * 6, "на грань должно быть два треугольника");

        for t in buf.idx.chunks(3) {
            let (a, b, c) = (
                buf.pos[t[0] as usize],
                buf.pos[t[1] as usize],
                buf.pos[t[2] as usize],
            );
            let n = buf.nrm[t[0] as usize];
            let x = cross(sub(b, a), sub(c, a));
            let dot = x[0] * n[0] + x[1] * n[1] + x[2] * n[2];
            assert!(
                dot > 0.0,
                "треугольник отвёрнут от своей нормали: {x:?} против {n:?}"
            );
        }
    }

    /// **Грань лежит в своей плоскости** и занимает ровно свой
    /// прямоугольник: положительная сторона на дальнем краю воксела,
    /// отрицательная на ближнем.
    #[test]
    fn a_face_sits_in_its_own_plane() {
        let mut buf = MeshBuf::default();
        // +X у воксела со срезом 4: плоскость x = 4.
        quads_to_mesh(
            &[Quad { face: 0, slice: 4, a: 3, b: 2, w: 2, h: 3, mat: 1 }],
            [0.0, 0.0, 0.0],
            &mut buf,
        );
        assert!(buf.pos.iter().all(|p| p[0] == 4.0), "плоскость не та: {:?}", buf.pos);
        // −X того же среза: плоскость x = 3.
        buf.clear();
        quads_to_mesh(
            &[Quad { face: 1, slice: 4, a: 3, b: 2, w: 2, h: 3, mat: 1 }],
            [0.0, 0.0, 0.0],
            &mut buf,
        );
        assert!(buf.pos.iter().all(|p| p[0] == 3.0), "плоскость не та: {:?}", buf.pos);

        // Прямоугольник: по `a` ширина `w`, по `b` высота `h`.
        buf.clear();
        quads_to_mesh(
            &[Quad { face: 0, slice: 4, a: 3, b: 2, w: 2, h: 3, mat: 1 }],
            [0.0, 0.0, 0.0],
            &mut buf,
        );
        let zs: Vec<f32> = buf.pos.iter().map(|p| p[2]).collect();
        let ys: Vec<f32> = buf.pos.iter().map(|p| p[1]).collect();
        assert_eq!(zs.iter().cloned().fold(f32::MAX, f32::min), 2.0);
        assert_eq!(zs.iter().cloned().fold(f32::MIN, f32::max), 4.0);
        assert_eq!(ys.iter().cloned().fold(f32::MAX, f32::min), 1.0);
        assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 4.0);
    }

    /// **Начало чанка совпадает с его местом в сетке.**
    #[test]
    fn chunk_origins_tile_the_world() {
        let (nx, ny) = (3u32, 2u32);
        assert_eq!(chunk_origin(0, nx, ny), [0.0, 0.0, 0.0]);
        assert_eq!(chunk_origin(1, nx, ny), [CS as f32, 0.0, 0.0]);
        assert_eq!(chunk_origin(3, nx, ny), [0.0, CS as f32, 0.0]);
        assert_eq!(chunk_origin(6, nx, ny), [0.0, 0.0, CS as f32]);
        // Целый вариант обязан давать то же самое: две формулы одного
        // места расходятся молча.
        for i in 0..12 {
            let a = chunk_origin(i, nx, ny);
            let b = chunk_at(i, nx, ny);
            assert_eq!([a[0] as u32, a[1] as u32, a[2] as u32], b);
        }
    }

    /// **Том юнита разворачивается, и каждая грань смотрит наружу.**
    ///
    /// Та же проверка, что у рельефа, и по той же причине: перепутанный
    /// обход углов не падает — юнит просто исчезает с экрана.
    #[test]
    fn a_unit_volume_faces_outwards() {
        let tank = voxel::unit::tank();
        let mut buf = MeshBuf::default();
        volume_to_mesh(&tank, [0.3, 0.5, 0.9], 0.5, Slice::All, &mut buf);
        assert!(!buf.idx.is_empty(), "танк не дал ни одного треугольника");
        assert_eq!(buf.col.len(), buf.pos.len(), "цвет не у каждой вершины");
        for t in buf.idx.chunks(3) {
            let (a, b, c) = (
                buf.pos[t[0] as usize],
                buf.pos[t[1] as usize],
                buf.pos[t[2] as usize],
            );
            let n = buf.nrm[t[0] as usize];
            let x = cross(sub(b, a), sub(c, a));
            let dot = x[0] * n[0] + x[1] * n[1] + x[2] * n[2];
            assert!(dot > 0.0, "треугольник отвёрнут: {x:?} против {n:?}");
        }
    }

    /// **Внутренних граней нет.** Грань между двумя занятыми вокселями
    /// не видна никогда, а стоит столько же, сколько видимая.
    #[test]
    fn inner_faces_are_not_emitted() {
        // Сплошной куб 3×3×3: снаружи шесть сторон по девять вокселей,
        // то есть 54 грани. Считаются **индексы**, по шесть на грань:
        // 54 × 6 = 324. Внутренних граней быть не должно — без отсева
        // соседа вышло бы 27 × 6 × 6 = 972.
        let mut b = voxel::unit::UnitBody::new(3, 3, 3, 1);
        for z in 0..3 {
            for y in 0..3 {
                for x in 0..3 {
                    b.set(x, y, z, voxel::unit::Part::Armour);
                }
            }
        }
        let mut buf = MeshBuf::default();
        volume_to_mesh(&b, [1.0, 1.0, 1.0], 1.0, Slice::All, &mut buf);
        assert_eq!(buf.idx.len(), 324, "лишние грани внутри куба");
    }

    /// **Роль красит воксел.** Ствол обязан отличаться от брони,
    /// иначе силуэт не читается.
    #[test]
    fn the_barrel_is_not_the_colour_of_the_hull() {
        let tank = voxel::unit::tank();
        let tint = [0.3, 0.5, 0.9];
        let mut buf = MeshBuf::default();
        volume_to_mesh(&tank, tint, 0.5, Slice::All, &mut buf);
        let hull = look::to_linear(tint);
        let others = buf.col.iter().filter(|&&c| c != hull).count();
        assert!(others > 0, "весь танк одного цвета — роли не видно");
    }

    /// **Корпус и башня делят том без остатка и без нахлёста.**
    ///
    /// Проверка на сумму: разрез, потерявший слой, даёт дыру в юните,
    /// а разрез с нахлёстом — двойную геометрию, и то и другое молча.
    #[test]
    fn hull_and_turret_split_the_body_exactly() {
        for body in [voxel::unit::tank(), voxel::unit::strider()] {
            assert!(body.turret_y > 0, "у тела нет линии башни — делить нечего");
            let (mut all, mut hull, mut turret) =
                (MeshBuf::default(), MeshBuf::default(), MeshBuf::default());
            volume_to_mesh(&body, [0.5, 0.5, 0.5], 0.5, Slice::All, &mut all);
            volume_to_mesh(&body, [0.5, 0.5, 0.5], 0.5, Slice::Hull, &mut hull);
            volume_to_mesh(&body, [0.5, 0.5, 0.5], 0.5, Slice::Turret, &mut turret);
            assert!(!hull.is_empty() && !turret.is_empty(), "часть тела пуста");
            assert_eq!(
                all.idx.len(),
                hull.idx.len() + turret.idx.len(),
                "корпус и башня не складываются в целое тело"
            );
        }
    }

    /// **Ствол уехал в башню.** Иначе поворачивать нечего: орудие
    /// осталось бы на корпусе и смотрело по движению.
    #[test]
    fn the_barrel_belongs_to_the_turret() {
        let tank = voxel::unit::tank();
        let gun = look::to_linear(look::part_srgb(voxel::unit::Part::Weapon, [0.5, 0.5, 0.5]));
        let mut hull = MeshBuf::default();
        let mut turret = MeshBuf::default();
        volume_to_mesh(&tank, [0.5, 0.5, 0.5], 0.5, Slice::Hull, &mut hull);
        volume_to_mesh(&tank, [0.5, 0.5, 0.5], 0.5, Slice::Turret, &mut turret);
        assert_eq!(hull.col.iter().filter(|&&c| c == gun).count(), 0, "орудие в корпусе");
        assert!(turret.col.contains(&gun), "в башне нет орудия");
    }

    /// **Пустой чанк даёт пустой буфер**, а не треугольник в нуле.
    #[test]
    fn no_quads_no_geometry() {
        let mut buf = MeshBuf::default();
        quads_to_mesh(&[], [0.0, 0.0, 0.0], &mut buf);
        assert!(buf.is_empty());
    }

    /// Ступенчатый рельеф из трёх слоёв с вкраплениями руды.
    ///
    /// Ступени нужны не для красоты: на ровной плите почти все грани
    /// смотрят вверх, и проверка прошла бы, не тронув две оси из трёх.
    /// Вкрапления — чтобы граница материалов проходила **внутри**
    /// квада, а не по его краю.
    struct Layers;

    impl voxel::Generator for Layers {
        fn material(&self, x: u32, y: u32, z: u32) -> Mat {
            let h = 12 + x % 7 + (z % 5) * 2;
            if y >= h {
                return voxel::store::AIR;
            }
            if y + 2 >= h {
                return if (x / 3 + z / 2) % 3 == 0 { 17 } else { 1 };
            }
            if y >= 6 {
                2
            } else {
                3
            }
        }
    }

    /// **Цвет вершины равен материалу воксела под ней.**
    ///
    /// Главный тест раскраски и единственный, который ловит
    /// перепутанные оси в `owner`. Проверяется не «цвета есть», а
    /// **соответствие**: для каждой вершины считается воксель, которому
    /// принадлежит её грань, и его материал сверяется с цветом.
    #[test]
    fn painted_colours_match_the_world() {
        use meshing::layer::MeshLayer;
        use meshing::Opts;

        let w = voxel::World::new(128, 64, 128, 1 << 16, Box::new(Layers));
        let layer = MeshLayer::new(&w, Opts::default());
        let (nx, ny, _) = layer.dims();
        let mut p = Painter::default();
        let mut seen = [0usize; look::CLASSES];
        let mut checked = 0usize;
        for i in 0..layer.dims().0 as usize * layer.dims().1 as usize * layer.dims().2 as usize {
            let quads = layer.rebuilt_quads(&w, i);
            if quads.is_empty() {
                continue;
            }
            let mut bufs = class_bufs();
            let at = chunk_at(i, nx, ny);
            quads_to_mesh_painted(
                &quads,
                [0.0, 0.0, 0.0],
                at,
                |x, y, z| w.material(x, y, z),
                &mut p,
                &mut bufs,
            );
            for (k, b) in bufs.iter().enumerate() {
                assert_eq!(b.col.len(), b.pos.len(), "цвет не у каждой вершины");
                seen[k] += b.idx.len();
                // Вершина лежит на грани воксела; воксель — тот, чья
                // это грань. Берём точку внутри квада и сдвигаем её
                // на полшага против нормали.
                for t in b.idx.chunks(6) {
                    // Первая вершина четвёрки — всегда `t[0]`, но
                    // противоположный угол лежит через одну **по
                    // буферу**, а не по индексу: у отрицательных граней
                    // обход перевёрнут.
                    let base = t[0] as usize;
                    let (a, c) = (b.pos[base], b.pos[base + 2]);
                    let n = b.nrm[base];
                    let mid = [
                        (a[0] + c[0]) * 0.5 - n[0] * 0.5,
                        (a[1] + c[1]) * 0.5 - n[1] * 0.5,
                        (a[2] + c[2]) * 0.5 - n[2] * 0.5,
                    ];
                    let (vx, vy, vz) = (
                        at[0] + mid[0].floor() as u32,
                        at[1] + mid[1].floor() as u32,
                        at[2] + mid[2].floor() as u32,
                    );
                    let m = w.material(vx, vy, vz);
                    assert_ne!(m, voxel::store::AIR, "грань принадлежит воздуху");
                    assert_eq!(
                        b.col[base],
                        look::colour(m),
                        "воксель {vx},{vy},{vz} материала {m} покрашен не своим цветом"
                    );
                    assert_eq!(look::class(m) as usize, k, "материал {m} не в своём буфере");
                    checked += 1;
                }
            }
        }
        assert!(checked > 5_000, "проверка почти ничего не проверила: {checked}");
        assert!(seen[ROCK] > 0 && seen[ORE] > 0, "один из классов пуст: {seen:?}");
    }

    /// **Однородный квад не режется.** Быстрый путь — это не
    /// оптимизация ради оптимизации: без него сплошной пол мира
    /// распался бы на воксели, и вершин стало бы в тысячи раз больше.
    #[test]
    fn a_uniform_quad_stays_one_quad() {
        let mut p = Painter::default();
        let mut bufs = class_bufs();
        quads_to_mesh_painted(
            &[Quad { face: 2, slice: 4, a: 1, b: 1, w: 40, h: 40, mat: 1 }],
            [0.0, 0.0, 0.0],
            [0, 0, 0],
            |_, _, _| 2,
            &mut p,
            &mut bufs,
        );
        assert_eq!(bufs[ROCK].idx.len(), 6, "однородный квад развалился");
        assert_eq!(bufs[ORE].idx.len(), 0);
    }

    /// **Неоднородный квад режется ровно по границе.** Половина на
    /// половину даёт две грани, а не сорок: жадное слияние внутри
    /// квада обязано работать, иначе граница материалов стоит дороже
    /// всего остального вместе.
    #[test]
    fn a_split_quad_costs_two_quads_not_forty() {
        let mut p = Painter::default();
        let mut bufs = class_bufs();
        // Грань +Y: owner = (a, s, b), то есть `a` это x, `b` это z.
        quads_to_mesh_painted(
            &[Quad { face: 2, slice: 4, a: 1, b: 1, w: 40, h: 40, mat: 1 }],
            [0.0, 0.0, 0.0],
            [0, 0, 0],
            |x, _, _| if x < 20 { 2 } else { 17 },
            &mut p,
            &mut bufs,
        );
        assert_eq!(bufs[ROCK].idx.len(), 6, "порода развалилась на куски");
        assert_eq!(bufs[ORE].idx.len(), 6, "руда развалилась на куски");
    }
}
