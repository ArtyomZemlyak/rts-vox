//! Квады мешинга — в вершины и индексы.
//!
//! Это **логика, а не рисование**, и потому проверяется headless: окно
//! из терминала не запустить, а перепутанный обход углов виден тестом
//! на векторное произведение (`CLAUDE.md`: тесты до рендера).
//!
//! Ошибиться здесь легко и незаметно: неверный обход даёт треугольник,
//! отвёрнутый от камеры, и чанк просто пропадает — ни ошибки,
//! ни предупреждения.

use meshing::{Quad, CS};

/// Буферы одного чанка. Переиспользуются: выделение на чанк мерило бы
/// аллокатор, а чанков тысячи.
#[derive(Default)]
pub struct MeshBuf {
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[f32; 3]>,
    pub idx: Vec<u32>,
    /// Цвет вершины. Пуст у рельефа — он одноцветный — и заполнен
    /// у юнитов, где цвет несёт роль воксела (ADR-172).
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

/// Развернуть квады чанка в треугольники.
///
/// `origin` — начало чанка в вокселях мира. Координаты квада приходят
/// в паддинговой системе `1..=CS`, поэтому единица вычитается.
pub fn quads_to_mesh(quads: &[Quad], origin: [f32; 3], out: &mut MeshBuf) {
    for q in quads {
        let (axis, positive) = axis_dir(q.face);
        let (la, lb, ls) = (
            q.a as f32 - 1.0,
            q.b as f32 - 1.0,
            q.slice as f32 - 1.0,
        );
        // Грань лежит между вокселем и его соседом: у положительной
        // стороны это дальний край воксела, у отрицательной — ближний.
        let plane = ls + if positive { 1.0 } else { 0.0 };
        let (w, h) = (q.w as f32, q.h as f32);

        // Начало квада и два направления в плоскости. Пара выбрана так,
        // что их векторное произведение даёт **положительную** ось:
        // отсюда и обход углов.
        let (p0, u, v) = match axis {
            0 => ([plane, lb, la], [0.0, h, 0.0], [0.0, 0.0, w]),
            1 => ([la, plane, lb], [0.0, 0.0, h], [w, 0.0, 0.0]),
            _ => ([lb, la, plane], [h, 0.0, 0.0], [0.0, w, 0.0]),
        };
        let add = |a: [f32; 3], b: [f32; 3]| [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
        let at = |p: [f32; 3]| {
            [
                p[0] + origin[0],
                p[1] + origin[1],
                p[2] + origin[2],
            ]
        };
        let c0 = at(p0);
        let c1 = at(add(p0, u));
        let c2 = at(add(add(p0, u), v));
        let c3 = at(add(p0, v));

        let mut n = [0.0f32; 3];
        n[axis] = if positive { 1.0 } else { -1.0 };

        let base = out.pos.len() as u32;
        out.pos.extend_from_slice(&[c0, c1, c2, c3]);
        out.nrm.extend_from_slice(&[n, n, n, n]);
        if positive {
            out.idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        } else {
            // Та же четвёрка, обход наоборот: иначе грань смотрит внутрь.
            out.idx.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
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

/// Развернуть воксельный том юнита в треугольники.
///
/// Грань ставится там, где соседа нет: внутренние грани не видны
/// и стоят ровно столько же, сколько видимые.
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
    out: &mut MeshBuf,
) {
    let (nx, ny, nz) = (
        b.vol.nx as usize,
        b.vol.ny as usize,
        b.vol.nz as usize,
    );
    // Том ставится серединой в начало координат: юнит вращается вокруг
    // себя, а не вокруг угла.
    let half = [
        nx as f32 * 0.5,
        0.0,
        nz as f32 * 0.5,
    ];
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                if !b.vol.get(x, y, z) {
                    continue;
                }
                let c = part_color(b.part_at(x, y, z), tint);
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

/// Цвет по роли воксела.
fn part_color(p: voxel::unit::Part, tint: [f32; 3]) -> [f32; 4] {
    use voxel::unit::Part;
    match p {
        // Броня — цвет стороны: по нему юнит и узнают.
        Part::Armour => [tint[0], tint[1], tint[2], 1.0],
        // Орудие тёмное: ствол обязан читаться на фоне корпуса.
        Part::Weapon => [0.16, 0.16, 0.18, 1.0],
        Part::Engine => [0.30, 0.24, 0.16, 1.0],
        Part::Sensor => [0.85, 0.90, 0.95, 1.0],
        // Ядро внутри и снаружи не видно; цвет на случай, когда
        // броню вокруг него снесли.
        Part::Core => [1.0, 0.85, 0.25, 1.0],
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
    let axis = if d[0] != 0 { 0 } else if d[1] != 0 { 1 } else { 2 };
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
    }

    /// **Том юнита разворачивается, и каждая грань смотрит наружу.**
    ///
    /// Та же проверка, что у рельефа, и по той же причине: перепутанный
    /// обход углов не падает — юнит просто исчезает с экрана.
    #[test]
    fn a_unit_volume_faces_outwards() {
        let tank = voxel::unit::tank();
        let mut buf = MeshBuf::default();
        volume_to_mesh(&tank, [0.3, 0.5, 0.9], 0.5, &mut buf);
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
        volume_to_mesh(&b, [1.0, 1.0, 1.0], 1.0, &mut buf);
        assert_eq!(buf.idx.len(), 324, "лишние грани внутри куба");
    }

    /// **Роль красит воксел.** Ствол обязан отличаться от брони,
    /// иначе силуэт не читается.
    #[test]
    fn the_barrel_is_not_the_colour_of_the_hull() {
        let tank = voxel::unit::tank();
        let tint = [0.3, 0.5, 0.9];
        let mut buf = MeshBuf::default();
        volume_to_mesh(&tank, tint, 0.5, &mut buf);
        let hull = [tint[0], tint[1], tint[2], 1.0];
        let others = buf.col.iter().filter(|&&c| c != hull).count();
        assert!(others > 0, "весь танк одного цвета — роли не видно");
    }

    /// **Пустой чанк даёт пустой буфер**, а не треугольник в нуле.
    #[test]
    fn no_quads_no_geometry() {
        let mut buf = MeshBuf::default();
        quads_to_mesh(&[], [0.0, 0.0, 0.0], &mut buf);
        assert!(buf.is_empty());
    }
}
