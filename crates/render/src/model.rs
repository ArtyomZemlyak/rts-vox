//! Разворот тела юнита **с фаской**: воксели перестают быть кубиками.
//!
//! **Почему геометрией, а не шейдером.** ADR-052 решал фаску для
//! рельефа: там миллионы вокселей и инкрементальная пересборка, и
//! лишняя геометрия на каждом ребре разорительна. У тела юнита всё
//! иначе — тысячи вокселей, и меш строится **один раз на тип**,
//! а не на машину. Там, где цена платится единожды, геометрия честнее
//! шейдерного обмана: скругляется силуэт, а не только освещение.
//!
//! Второй довод — правило `CLAUDE.md`: «шейдеры самодостаточны, без
//! `#import` из модулей Bevy». Материал со своим фрагментным шейдером
//! пришлось бы писать вместе со **своим** освещением, то есть выпасть
//! из общего PBR, тонмаппинга и SSAO. Фаска в геометрии остаётся
//! в `StandardMaterial` и получает всё это даром (ADR-206).
//!
//! **Скругляются только настоящие углы.** Ребро получает фаску тогда
//! и только тогда, когда сосед за ним пуст. Внутри плоской стенки
//! соседи есть, фаски нет, грань остаётся ровной — иначе по всей
//! броне пошли бы борозды, а не скругления.
//!
//! **Обход выводится из геометрии, а не назначается.** У фаски
//! двенадцать случаев на воксель, и знак обхода в каждом зависит от
//! того, как легли оси; выписанный руками, он врёт в половине —
//! проверено, 520 отвёрнутых треугольников на танке. Поэтому `quad`
//! смотрит на векторное произведение своих же углов и разворачивает
//! себя сам. Заодно он выбрасывает вырожденные: при нулевой фаске
//! её четырёхугольники схлопываются в отрезок.

use voxel::unit::{Part, UnitBody};

use crate::geom::MeshBuf;

/// Шесть направлений: смещение соседа, ось и сторона.
const SIDES: [([i32; 3], usize, bool); 6] = [
    ([1, 0, 0], 0, true),
    ([-1, 0, 0], 0, false),
    ([0, 1, 0], 1, true),
    ([0, -1, 0], 1, false),
    ([0, 0, 1], 2, true),
    ([0, 0, -1], 2, false),
];

/// Насколько ребро съедается фаской, в долях воксела.
///
/// Четверть — предел: фаска ест с обеих сторон, и на нём грань
/// исчезает совсем.
///
/// **Значение выбрано по снимкам, а не по вкусу.** При 0.12 фаска
/// на воксель в 0.25 м — это три сантиметра на шестиметровом танке,
/// и на дистанции боя её не видно вовсе. При 0.20 ребро читается,
/// а плоскости остаётся три пятых воксела — довольно, чтобы материал
/// был виден плоскостью, а не только скосом (ADR-206).
pub const BEVEL: f32 = 0.20;

/// Развернуть тело в треугольники со скруглёнными рёбрами.
///
/// `layers` — какие слои по высоте брать, полуоткрыто: башню и корпус
/// надо рисовать порознь, они вращаются в разные стороны.
pub fn body_to_mesh(
    b: &UnitBody,
    tint: [f32; 3],
    scale: f32,
    layers: (usize, usize),
    bevel: f32,
    out: &mut MeshBuf,
) {
    let (nx, ny, nz) = (b.vol.nx as usize, b.vol.ny as usize, b.vol.nz as usize);
    let bevel = bevel.clamp(0.0, 0.24);
    // Том ставится серединой в начало координат по горизонтали и низом
    // по высоте: юнит вращается вокруг себя, а не вокруг угла.
    let half = [nx as f32 * 0.5, 0.0, nz as f32 * 0.5];
    let solid = |p: [i32; 3]| -> bool {
        p[0] >= 0
            && p[1] >= 0
            && p[2] >= 0
            && (p[0] as usize) < nx
            && (p[1] as usize) < ny
            && (p[2] as usize) < nz
            && b.vol.get(p[0] as usize, p[1] as usize, p[2] as usize)
    };

    for z in 0..nz {
        for y in layers.0..layers.1.min(ny) {
            for x in 0..nx {
                if !b.vol.get(x, y, z) {
                    continue;
                }
                let c = colour(b.part_at(x, y, z), tint);
                let at = [x as i32, y as i32, z as i32];
                // Местная точка воксела → мировая.
                let put = |p: [f32; 3]| -> [f32; 3] {
                    [
                        (at[0] as f32 + p[0] - half[0]) * scale,
                        (at[1] as f32 + p[1] - half[1]) * scale,
                        (at[2] as f32 + p[2] - half[2]) * scale,
                    ]
                };
                let free = |k: usize, s: i32| -> bool {
                    let mut p = at;
                    p[k] += s;
                    !solid(p)
                };
                for (d, axis, positive) in SIDES {
                    if solid([at[0] + d[0], at[1] + d[1], at[2] + d[2]]) {
                        continue;
                    }
                    face(axis, positive, bevel, c, &free, &put, out);
                }
                corners(bevel, c, &free, &put, out);
            }
        }
    }
}

/// Цвет по роли воксела — тот же, что у плоского разворота.
fn colour(p: Part, tint: [f32; 3]) -> [f32; 4] {
    crate::look::to_linear(crate::look::part_srgb(p, tint))
}

/// Две другие оси при данной.
#[inline]
fn others(axis: usize) -> (usize, usize) {
    match axis {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    }
}

/// Координата по оси `k`: поджатая внутрь, если за ребром пусто.
#[inline]
fn edge(hi: bool, cut: bool, bevel: f32) -> f32 {
    match (hi, cut) {
        (true, true) => 1.0 - bevel,
        (true, false) => 1.0,
        (false, true) => bevel,
        (false, false) => 0.0,
    }
}

/// Открытая грань и фаски по её открытым рёбрам.
fn face(
    axis: usize,
    positive: bool,
    bevel: f32,
    c: [f32; 4],
    free: &impl Fn(usize, i32) -> bool,
    put: &impl Fn([f32; 3]) -> [f32; 3],
    out: &mut MeshBuf,
) {
    let (u, v) = others(axis);
    let (cu0, cu1) = (free(u, -1), free(u, 1));
    let (cv0, cv1) = (free(v, -1), free(v, 1));
    let (u0, u1) = (edge(false, cu0, bevel), edge(true, cu1, bevel));
    let (v0, v1) = (edge(false, cv0, bevel), edge(true, cv1, bevel));
    let a = if positive { 1.0 } else { 0.0 };

    let pt = |cu: f32, cv: f32, ca: f32| -> [f32; 3] {
        let mut p = [0.0f32; 3];
        p[axis] = ca;
        p[u] = cu;
        p[v] = cv;
        put(p)
    };
    let mut n = [0.0f32; 3];
    n[axis] = if positive { 1.0 } else { -1.0 };
    quad(
        [
            pt(u0, v0, a),
            pt(u1, v0, a),
            pt(u1, v1, a),
            pt(u0, v1, a),
        ],
        n,
        c,
        out,
    );

    // Фаска по каждому открытому ребру. Ребро открыто, когда сосед
    // за ним пуст: тогда у того же воксела открыта и грань в ту
    // сторону, и фаска соединяет две поджатые грани.
    //
    // Ребро принадлежит **двум** открытым граням сразу, и обе хотят
    // поставить на него фаску. Ставит та, у которой ось младше:
    // условие `k > axis` отсекает вторую. Без этого каждая фаска
    // легла бы вдвое, и просвечивала бы сама через себя.
    let sa = if positive { 1.0 - bevel } else { bevel };
    for (k, hi) in [(u, false), (u, true), (v, false), (v, true)] {
        let sgn = if hi { 1 } else { -1 };
        if !free(k, sgn) {
            continue;
        }
        if k > axis {
            continue;
        }
        let (o0, o1) = if k == u { (v0, v1) } else { (u0, u1) };
        let ke = edge(hi, true, bevel);
        let kf = if hi { 1.0 } else { 0.0 };
        let mut fnorm = [0.0f32; 3];
        let inv = std::f32::consts::FRAC_1_SQRT_2;
        fnorm[axis] = if positive { inv } else { -inv };
        fnorm[k] = sgn as f32 * inv;

        let corner = |kc: f32, oc: f32, ac: f32| -> [f32; 3] {
            let mut p = [0.0f32; 3];
            p[axis] = ac;
            p[k] = kc;
            p[if k == u { v } else { u }] = oc;
            put(p)
        };
        quad(
            [
                corner(ke, o0, a),
                corner(kf, o0, sa),
                corner(kf, o1, sa),
                corner(ke, o1, a),
            ],
            fnorm,
            c,
            out,
        );
    }
}

/// Уголки: там, где сходятся три фаски, остаётся треугольная дыра.
fn corners(
    bevel: f32,
    c: [f32; 4],
    free: &impl Fn(usize, i32) -> bool,
    put: &impl Fn([f32; 3]) -> [f32; 3],
    out: &mut MeshBuf,
) {
    for hi in 0..8u8 {
        let s = [hi & 1 != 0, hi & 2 != 0, hi & 4 != 0];
        // Уголок есть только там, где все три соседа пусты.
        if !(0..3).all(|k| free(k, if s[k] { 1 } else { -1 })) {
            continue;
        }
        let inv = 1.0 / (3.0f32).sqrt();
        let n = [
            if s[0] { inv } else { -inv },
            if s[1] { inv } else { -inv },
            if s[2] { inv } else { -inv },
        ];
        // Три точки: каждая лежит на своей грани, поджатая по двум
        // другим осям.
        let mut p = [[0.0f32; 3]; 3];
        for (k, slot) in p.iter_mut().enumerate() {
            for a in 0..3 {
                slot[a] = if a == k {
                    if s[a] { 1.0 } else { 0.0 }
                } else {
                    edge(s[a], true, bevel)
                };
            }
        }
        tri([put(p[0]), put(p[1]), put(p[2])], n, c, out);
    }
}

/// Четырёхугольник двумя треугольниками. Обход выводится из углов.
fn quad(p: [[f32; 3]; 4], n: [f32; 3], c: [f32; 4], out: &mut MeshBuf) {
    tri([p[0], p[1], p[2]], n, c, out);
    tri([p[0], p[2], p[3]], n, c, out);
}

/// Треугольник. Вырожденный не ставится; отвёрнутый разворачивается.
fn tri(p: [[f32; 3]; 3], n: [f32; 3], c: [f32; 4], out: &mut MeshBuf) {
    let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
    let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
    let x = [
        e1[1] * e2[2] - e1[2] * e2[1],
        e1[2] * e2[0] - e1[0] * e2[2],
        e1[0] * e2[1] - e1[1] * e2[0],
    ];
    let dot = x[0] * n[0] + x[1] * n[1] + x[2] * n[2];
    // Порог, а не ноль: фаска нулевой ширины даёт отрезок, и его
    // треугольники не нужны никому.
    if dot.abs() < 1e-9 {
        return;
    }
    let base = out.pos.len() as u32;
    out.pos.extend_from_slice(&p);
    out.nrm.extend_from_slice(&[n, n, n]);
    out.col.extend_from_slice(&[c, c, c]);
    if dot > 0.0 {
        out.idx.extend_from_slice(&[base, base + 1, base + 2]);
    } else {
        out.idx.extend_from_slice(&[base, base + 2, base + 1]);
    }
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

    fn cube(n: usize) -> UnitBody {
        let mut b = UnitBody::new(n as u8, n as u8, n as u8, 1);
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    b.set(x, y, z, Part::Armour);
                }
            }
        }
        b
    }

    fn build(b: &UnitBody, bevel: f32) -> MeshBuf {
        let mut buf = MeshBuf::default();
        body_to_mesh(b, [0.3, 0.5, 0.9], 1.0, (0, 255), bevel, &mut buf);
        buf
    }

    /// **Каждый треугольник смотрит наружу.**
    ///
    /// На фаске эта ловушка злее, чем на плоском развороте: обходов
    /// втрое больше, и выписанные руками врали на 520 треугольниках
    /// танка. Теперь обход выводится из углов, и тест это сторожит.
    #[test]
    fn every_triangle_faces_outwards() {
        let buf = build(&voxel::unit::tank(), BEVEL);
        assert!(!buf.idx.is_empty(), "танк не дал ни одного треугольника");
        let mut bad = 0;
        for t in buf.idx.chunks(3) {
            let (a, b, c) = (
                buf.pos[t[0] as usize],
                buf.pos[t[1] as usize],
                buf.pos[t[2] as usize],
            );
            let n = buf.nrm[t[0] as usize];
            let x = cross(sub(b, a), sub(c, a));
            if x[0] * n[0] + x[1] * n[1] + x[2] * n[2] <= 0.0 {
                bad += 1;
            }
        }
        assert_eq!(bad, 0, "{bad} треугольников отвёрнуты");
    }

    /// **Нулевая фаска даёт ровно плоский разворот.**
    ///
    /// Заодно проверяет, что вырожденные четырёхугольники фаски
    /// выбрасываются, а не остаются отрезками в буфере.
    #[test]
    fn a_zero_bevel_is_the_plain_unfolding() {
        let buf = build(&cube(3), 0.0);
        assert_eq!(buf.idx.len(), 6 * 9 * 6, "не шесть граней по девять квадов");
    }

    /// **Одинокий воксель скруглён со всех сторон.**
    ///
    /// Шесть граней, двенадцать фасок и восемь уголков. Без уголков
    /// на месте вершины остаётся треугольная дыра, и видно её только
    /// на просвет.
    #[test]
    fn a_lone_voxel_is_rounded_on_every_edge_and_corner() {
        let buf = build(&cube(1), BEVEL);
        assert_eq!(buf.idx.len(), (6 + 12) * 6 + 8 * 3);
    }

    /// **Фаска ставится только по настоящим углам.**
    ///
    /// Внутри плоской плиты соседи есть, и рёбра между ними ровные.
    /// Если бы фаска шла по каждому ребру подряд, броня пошла бы
    /// бороздами, а геометрия выросла бы на порядок.
    #[test]
    fn only_real_corners_are_rounded() {
        let slab = {
            let mut b = UnitBody::new(5, 1, 5, 1);
            for z in 0..5 {
                for x in 0..5 {
                    b.set(x, 0, z, Part::Armour);
                }
            }
            b
        };
        let round = build(&slab, BEVEL);
        let plain = build(&slab, 0.0);
        let extra = (round.idx.len() - plain.idx.len()) / 3;
        assert!(extra > 0, "фасок нет вовсе");
        // Двадцать пять вокселей по двенадцать рёбер — триста; фасок
        // должно быть в разы меньше, потому что внутренние рёбра ровные.
        assert!(
            extra < 150,
            "треугольников фаски {extra}: они ставятся и между соседями"
        );
    }

    /// **Цена названа числом.**
    #[test]
    fn the_bevel_costs_what_it_costs() {
        let tank = voxel::unit::tank();
        let (plain, round) = (build(&tank, 0.0), build(&tank, BEVEL));
        let k = round.idx.len() as f32 / plain.idx.len() as f32;
        // Замерено: 1.20 раза на танке. Мало — и это хорошо: внутри
        // брони рёбра ровные, фаску получают только настоящие углы.
        // Верхняя граница сторожит обратное — фаску по каждому ребру,
        // которая утроила бы геометрию ни за что.
        assert!(
            (1.05..2.0).contains(&k),
            "фаска изменила геометрию в {k:.2} раза — это не похоже на фаску"
        );
    }
}
