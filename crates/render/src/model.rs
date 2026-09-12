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
//!
//! **Наклонная деталь — повёрнутый том, а не лесенка.** Тело состоит
//! из корневого тома и сегментов (`voxel::unit::Segment`): у каждого
//! свой осевой том, своя точка крепления и свой поворот. Мешер
//! разворачивает каждый том в своей системе координат — корень
//! вокруг середины основания, сегмент вокруг шарнира, — а место
//! в теле задаёт [`segment_frames`], складывая цепочку креплений.
//! Мировая сетка не пиксельная: тело поворачивается целиком на любой
//! угол, и сегмент внутри него — тоже (ADR-231).
//!
//! **Цвет и материал — по отделке воксела**, а не по роли: трак
//! и корпус оба броня, но один резина, другой краска.

use bevy::prelude::{Component, Quat, Transform, Vec3};
use voxel::unit::{Attach, UnitBody, TURN};

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
/// **Половина — это спиленный пополам куб**, и это умолчание. Фаска
/// ест с обеих сторон, поэтому на половине грань схлопывается в точку,
/// а одинокий воксель становится октаэдром. Внутри сплошной брони
/// рёбра не скругляются вовсе — там соседи, — так что плоскости
/// остаются плоскостями, а срезаются только настоящие углы.
///
/// Путь до этого значения стоит помнить. При 0.12 фаска на вокселе
/// в 0.25 м — три сантиметра на шестиметровом танке, и на дистанции
/// боя её нет вовсе. При 0.20 ребро читается вблизи и теряется
/// в бою. Только на половине форма меняется так, что видно
/// с игровой камеры (ADR-206).
pub const BEVEL: f32 = 0.5;

/// Развернуть тело в треугольники со скруглёнными рёбрами.
///
/// `layers` — какие слои по высоте брать, полуоткрыто: башню и корпус
/// надо рисовать порознь, они вращаются в разные стороны.
/// `centre` — точка тома в вокселях, которая встаёт в начало
/// координат меша: у корня это [`root_centre`], у сегмента —
/// шарнир ([`pivot_centre`]), потому что вокруг него сегмент
/// и поворачивается.
/// Треугольники раскладываются **по отделкам**, а не в один буфер:
/// сталь, стекло и резина отвечают на свет по-разному, и ответ этот
/// задаётся материалом, а вершина его нести не может (ADR-211).
/// Буферов ровно [`crate::look::FINISHES`]; пустые вызывающий
/// пропускает.
pub fn body_to_mesh(
    b: &UnitBody,
    tint: [f32; 3],
    scale: f32,
    layers: (usize, usize),
    bevel: f32,
    centre: [f32; 3],
    outs: &mut [MeshBuf; crate::look::FINISHES],
) {
    let (nx, ny, nz) = (b.vol.nx as usize, b.vol.ny as usize, b.vol.nz as usize);
    let bevel = bevel.clamp(0.0, 0.5);
    let half = centre;
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
                // Отделка воксела решает и цвет, и буфер: код тела,
                // а без него — роль (ADR-231).
                let f = crate::look::finish_of(b.finish_at(x, y, z), b.part_at(x, y, z));
                let c = crate::look::to_linear(crate::look::finish_srgb(f, tint));
                let out = &mut outs[f as usize];
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

/// Середина основания корневого тома: юнит вращается вокруг себя,
/// а не вокруг угла, и стоит низом на земле.
pub fn root_centre(b: &UnitBody) -> [f32; 3] {
    [b.vol.nx as f32 * 0.5, 0.0, b.vol.nz as f32 * 0.5]
}

/// Шарнир сегмента — точка его тома, вокруг которой он повёрнут
/// и которой посажен на родителя.
pub fn pivot_centre(a: &Attach) -> [f32; 3] {
    [a.pivot.0 as f32, a.pivot.1 as f32, a.pivot.2 as f32]
}

/// Поворот крепления: крен, потом тангаж, потом рыскание.
///
/// Порядок — часть контракта с телом, и знаки пришпилены тестом:
/// тангаж положительный поднимает нос, рыскание положительное
/// уводит его к `-Z` (тем же знаком, что курс машины), крен
/// положительный кладёт верх к `+Z`.
pub fn attach_rotation(a: &Attach) -> Quat {
    let k = std::f32::consts::TAU / TURN as f32;
    Quat::from_rotation_y(a.yaw as f32 * k)
        * Quat::from_rotation_z(a.pitch as f32 * k)
        * Quat::from_rotation_x(a.roll as f32 * k)
}

/// Место сегмента в системе координат тела: смещение и поворот
/// относительно корня, уже сложенные по цепочке родителей.
///
/// Сущность сегмента несёт это как компонент и каждый кадр ставится
/// в `base + rot * pos` с поворотом `rot * rot_seg`, где `rot` —
/// поворот корпуса (или башни, если `turret`). Иерархии сущностей
/// нет нарочно: положение читается из симуляции каждый кадр, и
/// одна формула на сущность дешевле распространения преобразований
/// по дереву.
#[derive(Component, Clone, Copy, Debug)]
pub struct SegFrame {
    pub pos: Vec3,
    pub rot: Quat,
    /// Едет с вращающейся частью тела: сам сегмент или любой из его
    /// предков прикреплён к башне.
    pub turret: bool,
}

/// Рамки всех сегментов тела по порядку. `per_mm` — мировых единиц
/// в миллиметре: у каждого тома своё ребро, и смещение крепления
/// меряется в вокселях **родителя**.
pub fn segment_frames(b: &UnitBody, per_mm: f32) -> Vec<SegFrame> {
    let mut frames: Vec<SegFrame> = Vec::with_capacity(b.segments.len());
    for s in &b.segments {
        let a = &s.attach;
        let (pos, rot, centre, scale, turret) = if a.parent == 0 {
            (Vec3::ZERO, Quat::IDENTITY, root_centre(b), b.mm as f32 * per_mm, false)
        } else {
            // Родитель добавлен раньше — это сторожит `push_segment`.
            let k = a.parent as usize - 1;
            let p = &frames[k];
            let ps = &b.segments[k];
            (p.pos, p.rot, pivot_centre(&ps.attach), ps.body.mm as f32 * per_mm, p.turret)
        };
        let off = Vec3::new(
            (a.at.0 as f32 - centre[0]) * scale,
            (a.at.1 as f32 - centre[1]) * scale,
            (a.at.2 as f32 - centre[2]) * scale,
        );
        frames.push(SegFrame {
            pos: pos + rot * off,
            rot: rot * attach_rotation(a),
            turret: a.turret || turret,
        });
    }
    frames
}

/// Поставить сущность тела: корень — в `base` с поворотом `rot`,
/// сегмент — на своё место в теле, повёрнутом тем же `rot`.
pub fn place(tr: &mut Transform, base: Vec3, rot: Quat, seg: Option<&SegFrame>) {
    match seg {
        Some(s) => {
            tr.translation = base + rot * s.pos;
            tr.rotation = rot * s.rot;
        }
        None => {
            tr.translation = base;
            tr.rotation = rot;
        }
    }
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
    use voxel::unit::Part;

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

    fn split(b: &UnitBody, bevel: f32) -> [MeshBuf; crate::look::FINISHES] {
        let mut bufs: [MeshBuf; crate::look::FINISHES] = std::array::from_fn(|_| MeshBuf::default());
        body_to_mesh(b, [0.3, 0.5, 0.9], 1.0, (0, 255), bevel, root_centre(b), &mut bufs);
        bufs
    }

    fn near(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 2e-3
    }

    /// **Знаки углов крепления пришпилены.** Тангаж положительный
    /// поднимает нос, рыскание положительное уводит его к `-Z`
    /// (как курс машины), крен положительный кладёт верх к `+Z`.
    /// Тело пишет углы в этих знаках; разойдись они — лобовой лист
    /// лёг бы на моторный отсек.
    #[test]
    fn attachment_angles_turn_the_way_the_body_says() {
        let flat = Attach::flat((0, 0, 0), (0, 0, 0));
        let x = |a: Attach| attach_rotation(&a) * Vec3::X;
        assert!(near(x(Attach { pitch: TURN / 4, ..flat }), Vec3::Y), "тангаж не поднимает нос");
        assert!(near(x(Attach { yaw: TURN / 4, ..flat }), -Vec3::Z), "рыскание не туда");
        assert!(
            near(attach_rotation(&Attach { roll: TURN / 4, ..flat }) * Vec3::Y, Vec3::Z),
            "крен не туда"
        );
        // Наведённое крепление смотрит туда, куда просили, и в
        // плоскости, и с наклоном.
        let d = x(Attach::aimed((0, 0, 0), (0, 0, 0), (3, -4, 0)));
        assert!(near(d, Vec3::new(0.6, -0.8, 0.0)), "наведение в плоскости: {d:?}");
        let want = Vec3::new(-2.0, -2.0, -1.0).normalize();
        let d = x(Attach::aimed((0, 0, 0), (0, 0, 0), (-2, -2, -1)));
        assert!(near(d, want), "наведение с рысканием: {d:?} вместо {want:?}");
    }

    /// **Сегмент стоит там, где сказано крепление, и едет
    /// за родителем.** Меш сегмента строится вокруг шарнира,
    /// а рамка кладёт шарнир в точку крепления родителя.
    #[test]
    fn a_segment_rides_where_its_attachment_says() {
        let mut root = cube(4);
        // Лист на переднем нижнем ребре, носом вверх на сорок пять.
        root.push_segment(
            Attach { pitch: TURN / 8, ..Attach::flat((4, 0, 2), (0, 0, 1)) },
            cube(2),
        );
        // Потомок на дальнем конце листа, башенный.
        root.push_segment(
            Attach { parent: 1, turret: true, ..Attach::flat((2, 0, 1), (0, 0, 1)) },
            cube(1),
        );
        let f = segment_frames(&root, 1.0 / root.mm as f32);
        assert_eq!(f.len(), 2);
        // Корень серединой в начале координат: (4, 0, 2) − (2, 0, 2).
        assert!(near(f[0].pos, Vec3::new(2.0, 0.0, 0.0)), "{:?}", f[0].pos);
        assert!(!f[0].turret);
        // Потомок: два воксела вдоль листа, поднятого на сорок пять.
        let r = std::f32::consts::FRAC_1_SQRT_2 * 2.0;
        assert!(near(f[1].pos, Vec3::new(2.0 + r, r, 0.0)), "{:?}", f[1].pos);
        assert!(f[1].turret, "потомок башенного сегмента едет с башней");
        assert!(f[1].rot.dot(f[0].rot).abs() > 0.9999, "потомок без своего поворота повёрнут иначе");

        // Меш сегмента — вокруг шарнира: у листа 2×2×2 с шарниром
        // (0, 0, 1) координаты лежат в [0, 2]×[0, 2]×[−1, 1].
        let seg = &root.segments[0];
        let mut bufs: [MeshBuf; crate::look::FINISHES] = std::array::from_fn(|_| MeshBuf::default());
        body_to_mesh(&seg.body, [0.5; 3], 1.0, (0, 255), 0.0, pivot_centre(&seg.attach), &mut bufs);
        let pos = &bufs[crate::look::Finish::Plate as usize].pos;
        assert!(!pos.is_empty());
        for p in pos {
            assert!((0.0..=2.0).contains(&p[0]) && (0.0..=2.0).contains(&p[1]));
            assert!((-1.0..=1.0).contains(&p[2]), "сегмент не вокруг шарнира: {p:?}");
        }

        // Постановка: корень в базе, сегмент — в базе плюс повёрнутое
        // смещение.
        let mut tr = Transform::IDENTITY;
        let rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        place(&mut tr, Vec3::new(10.0, 0.0, 0.0), rot, Some(&f[0]));
        assert!(near(tr.translation, Vec3::new(10.0, 0.0, -2.0)), "{:?}", tr.translation);
        place(&mut tr, Vec3::new(10.0, 0.0, 0.0), rot, None);
        assert!(near(tr.translation, Vec3::new(10.0, 0.0, 0.0)));
    }

    /// **Отделка красит воксель, а не роль.** Трак танка — броня,
    /// но резина; без кода он лёг бы в буфер краски.
    #[test]
    fn finish_codes_route_the_voxel_to_their_own_buffer() {
        let tank = voxel::unit::tank();
        let bufs = split(&tank, 0.0);
        assert!(
            !bufs[crate::look::Finish::Rubber as usize].idx.is_empty(),
            "траки не резина"
        );
        assert!(!bufs[crate::look::Finish::Steel as usize].idx.is_empty(), "катки не сталь");
        assert!(!bufs[crate::look::Finish::Gun as usize].idx.is_empty(), "ствол не воронёный");
        // Сегменты в корневой меш не попадают: у них свой.
        assert!(!tank.segments.is_empty(), "у танка нет наклонных листов");
    }

    /// Все отделки в один буфер — для проверок, которым отделка
    /// безразлична: геометрия от разделения не меняется.
    fn build(b: &UnitBody, bevel: f32) -> MeshBuf {
        let mut out = MeshBuf::default();
        for buf in split(b, bevel) {
            let base = out.pos.len() as u32;
            out.pos.extend_from_slice(&buf.pos);
            out.nrm.extend_from_slice(&buf.nrm);
            out.col.extend_from_slice(&buf.col);
            out.idx.extend(buf.idx.iter().map(|i| i + base));
        }
        out
    }

    /// **Тело расходится по отделкам, и ничего при этом не теряется.**
    ///
    /// Разделение на буферы — чисто раскладка: сумма частей обязана
    /// совпасть с целым до треугольника. Промах здесь выглядит как
    /// дыра в корпусе, и заметить его на снимке трудно: не хватать
    /// будет ровно тех граней, которые и так внутри строя.
    #[test]
    fn the_body_splits_by_finish_and_loses_nothing() {
        let tank = voxel::unit::tank();
        let bufs = split(&tank, BEVEL);
        let whole = build(&tank, BEVEL);
        let parts: usize = bufs.iter().map(|b| b.idx.len()).sum();
        assert_eq!(parts, whole.idx.len(), "при раскладке пропали треугольники");
        // Не меньше трёх непустых: броня, ствол, сенсор. Одна —
        // значит отделка не различается вовсе.
        let used = bufs.iter().filter(|b| !b.idx.is_empty()).count();
        assert!(used >= 3, "танк лёг всего в {used} отделок");
        // Ядро внутри брони, наружу оно граней не даёт.
        assert!(
            bufs[crate::look::Finish::Core as usize].idx.is_empty(),
            "ядро видно снаружи целого танка"
        );
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
    /// Умеренная фаска даёт шесть граней, двенадцать скосов и восемь
    /// уголков. Без уголков на месте вершины остаётся треугольная
    /// дыра, и видно её только на просвет.
    #[test]
    fn a_lone_voxel_is_rounded_on_every_edge_and_corner() {
        let buf = build(&cube(1), 0.2);
        assert_eq!(buf.idx.len(), (6 + 12) * 6 + 8 * 3);
    }

    /// **На половине воксель — октаэдр: куб спилен целиком.**
    ///
    /// Фаска ест с обеих сторон, поэтому на половине и грани,
    /// и скосы схлопываются в отрезки, а остаются только восемь
    /// уголков — то есть ровно восемь треугольных граней.
    ///
    /// Это не крайний случай, который «как-нибудь отработает»:
    /// половина — умолчание (`BEVEL`), и форма при нём обязана
    /// оставаться замкнутой, а не рассыпаться на дыры.
    #[test]
    fn at_half_a_voxel_the_cube_is_cut_to_an_octahedron() {
        let buf = build(&cube(1), 0.5);
        assert_eq!(buf.idx.len(), 8 * 3, "не восемь граней — это не октаэдр");
        // Замкнутость: у октаэдра шесть вершин, и каждая принадлежит
        // четырём граням. Дыра сломала бы это соотношение.
        let mut corners: Vec<[i32; 3]> = buf
            .pos
            .iter()
            .map(|p| [(p[0] * 64.0) as i32, (p[1] * 64.0) as i32, (p[2] * 64.0) as i32])
            .collect();
        corners.sort_unstable();
        corners.dedup();
        assert_eq!(corners.len(), 6, "вершин не шесть: {corners:?}");
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
