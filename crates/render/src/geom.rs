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
}

impl MeshBuf {
    pub fn clear(&mut self) {
        self.pos.clear();
        self.nrm.clear();
        self.idx.clear();
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

    /// **Пустой чанк даёт пустой буфер**, а не треугольник в нуле.
    #[test]
    fn no_quads_no_geometry() {
        let mut buf = MeshBuf::default();
        quads_to_mesh(&[], [0.0, 0.0, 0.0], &mut buf);
        assert!(buf.is_empty());
    }
}
