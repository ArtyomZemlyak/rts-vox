//! Кисти, которыми рисуются тела.
//!
//! **Зачем отдельный модуль.** Тела живут в двух местах: два ванильных
//! в [`crate::unit`], весь набор в [`crate::roster`]. Пока кисти лежали
//! в наборе, ванильные тела рисовались голыми циклами и отставали от
//! набора по дробности. Одна кисть, две мастерских.
//!
//! **Кисть описывает деталь, а не диапазон.** При вокселе 100 мм
//! у машины впятеро больше вокселей по каждой оси, чем при 250 мм,
//! и разница обязана уйти в детали — каток, шарнир, люк, решётку, —
//! а не в те же бруски помельче (ADR-219). Кисть тем и отличается от
//! `fill`, что называет деталь: `running_gear` рисует ходовую, а не
//! параллелепипед под машиной.
//!
//! **Наклонное — сегментом, а не лесенкой** (ADR-231, ADR-224). Кисти
//! второго рода строят **повёрнутые части**: [`slab`] и [`link`] дают
//! осевой том сегмента, [`mount`] и [`rod`] сажают его на тело и тем
//! же вызовом кладут в корневой том **скрытый объём** под ним
//! ([`shadow`]) — ступени под листом, кость внутри звена. Симуляция
//! видит только корневой том, и объём под попадание обязан лежать там.
//!
//! Арифметика целая, сравниваются **квадраты** — корень в симуляции
//! последнее средство (`CLAUDE.md`, ADR-071). Корень берётся ровно
//! в двух местах, оба при сборке шаблона: длина звена в [`rod`] и
//! наведение крепления в `Attach::aimed`.

use crate::unit::{
    Attach, Part, UnitBody, FINISH_BY_PART, FINISH_PLATE, FINISH_RUBBER, FINISH_STEEL,
};
use fx::Fx;

/// Назначить отделку всем твёрдым вокселям параллелепипеда.
///
/// Кисть формы и кисть отделки разведены нарочно: форма — это роль,
/// её видит симуляция; отделка — только вид, и назначается поверх
/// уже нарисованного. Воздух отделки не получает (`set_finish`).
pub(crate) fn mark(
    b: &mut UnitBody,
    xs: (usize, usize),
    ys: (usize, usize),
    zs: (usize, usize),
    code: u8,
) {
    let (nx, ny, nz) = (b.vol.nx as usize, b.vol.ny as usize, b.vol.nz as usize);
    for z in zs.0..zs.1.min(nz) {
        for y in ys.0..ys.1.min(ny) {
            for x in xs.0..xs.1.min(nx) {
                b.set_finish(x, y, z, code);
            }
        }
    }
}

/// Назначить отделку твёрдым вокселям шара — тем же условием, каким
/// шар рисуется, чтобы сустав красился сталью целиком и только он.
pub(crate) fn mark_ball(b: &mut UnitBody, c: (i32, i32, i32), r: i32, code: u8) {
    ball_h(
        b,
        (2 * c.0 + 1, 2 * c.1 + 1, 2 * c.2 + 1),
        2 * r + 1,
        |b, x, y, z| b.set_finish(x, y, z, code),
    );
}

/// Отделка цилиндру — тем же кругом, каким он рисуется.
pub(crate) fn mark_cyl(
    b: &mut UnitBody,
    axis: usize,
    span: (usize, usize),
    c: (i32, i32),
    r: i32,
    code: u8,
) {
    cyl_h(
        b,
        axis,
        span,
        (2 * c.0 + 1, 2 * c.1 + 1),
        2 * r + 1,
        0,
        |b, x, y, z| b.set_finish(x, y, z, code),
    );
}

/// Заполнить параллелепипед. Диапазоны полуоткрытые, выход за габарит
/// обрезается: формы строятся от руки, и промах на воксель не должен
/// ронять сборку.
pub(crate) fn fill(
    b: &mut UnitBody,
    xs: (usize, usize),
    ys: (usize, usize),
    zs: (usize, usize),
    p: Part,
) {
    let (nx, ny, nz) = (b.vol.nx as usize, b.vol.ny as usize, b.vol.nz as usize);
    for z in zs.0..zs.1.min(nz) {
        for y in ys.0..ys.1.min(ny) {
            for x in xs.0..xs.1.min(nx) {
                b.set(x, y, z, p);
            }
        }
    }
}

/// Заполнить и сразу отделать: одна кисть там, где деталь целиком
/// одного материала — стальной поручень, стеклянная фара.
pub(crate) fn fill_f(
    b: &mut UnitBody,
    xs: (usize, usize),
    ys: (usize, usize),
    zs: (usize, usize),
    p: Part,
    code: u8,
) {
    fill(b, xs, ys, zs, p);
    mark(b, xs, ys, zs, code);
}

/// Снять параллелепипед — паз, проём, смотровая щель.
///
/// Кисть, которая **убирает**: рельеф читается по тому, где брони нет,
/// не хуже, чем по тому, где она есть, а фаска мешера скругляет
/// открывшееся ребро так же, как выступающее. Резать безопасно только
/// **наружный слой**: вырез вглубь отрезает воксели от ядра, и это
/// поймает `every_body_is_whole_and_armed` уже после сборки.
pub(crate) fn carve(b: &mut UnitBody, xs: (usize, usize), ys: (usize, usize), zs: (usize, usize)) {
    let (nx, ny, nz) = (b.vol.nx as usize, b.vol.ny as usize, b.vol.nz as usize);
    for z in zs.0..zs.1.min(nz) {
        for y in ys.0..ys.1.min(ny) {
            for x in xs.0..xs.1.min(nx) {
                b.remove(x, y, z);
            }
        }
    }
}

/// Две другие оси при данной. Порядок фиксирован: у оси `Y` это
/// `(x, z)`, то есть привычная пара «вдоль, поперёк».
#[inline]
fn others(axis: usize) -> (usize, usize) {
    match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    }
}

/// **Круг в полувокселях.** Центр `c2` и радиус `r2` заданы половинами
/// воксела, поэтому центр может стоять и в середине воксела (нечётный
/// диаметр — каток, ствол), и на его углу (чётный — звено ноги, чья
/// ось обязана совпасть с шарниром сегмента, а шарнир лежит на узле
/// решётки).
///
/// Условие `D² ≤ r2² − 1` — это **цифровой круг** (ADR-224): при центре
/// в середине воксела оно равно `d² ≤ r² + r`, то есть кругу радиуса
/// на полвоксела больше. Строгий порог `d² ≤ r²` оставлял на каждой
/// оси одиночный воксель: при `r = 4` ряды шли ширинами 4, 3, 3, 2, 0,
/// фаска мешера превращала одиночку в ромбик, и каток читался цветком.
/// Здесь ряды идут 4, 4, 4, 3, 2 при любом `r ≥ 1`.
///
/// Внутренняя граница (`h2`) **строгая**: два вложенных кольца обязаны
/// касаться, а сдвинутая на полвоксела дырка разводила их на `d² = 42`,
/// которого на целой решётке нет, — решётка антенны рассыпалась
/// на кольца в воздухе.
#[inline]
fn in_disc(du2: i32, dv2: i32, r2: i32, h2: i32) -> bool {
    let d = du2 * du2 + dv2 * dv2;
    d < r2 * r2 && d >= h2 * h2
}

/// Обход круга в полувокселях вдоль оси `axis`; что делать с вокселем,
/// решает `f` — так одна геометрия служит и форме, и отделке.
fn cyl_h(
    b: &mut UnitBody,
    axis: usize,
    span: (usize, usize),
    c2: (i32, i32),
    r2: i32,
    h2: i32,
    mut f: impl FnMut(&mut UnitBody, usize, usize, usize),
) {
    let n = [b.vol.nx as i32, b.vol.ny as i32, b.vol.nz as i32];
    let (u, v) = others(axis);
    // Воксели, чья середина ближе `r2` полувокселей к центру.
    let (u0, u1) = ((c2.0 - r2 - 1).div_euclid(2), (c2.0 + r2 + 1).div_euclid(2));
    let (v0, v1) = ((c2.1 - r2 - 1).div_euclid(2), (c2.1 + r2 + 1).div_euclid(2));
    for a in span.0..span.1.min(n[axis] as usize) {
        for cv in v0.max(0)..=v1.min(n[v] - 1) {
            for cu in u0.max(0)..=u1.min(n[u] - 1) {
                if !in_disc(2 * cu + 1 - c2.0, 2 * cv + 1 - c2.1, r2, h2) {
                    continue;
                }
                let mut q = [0usize; 3];
                q[axis] = a;
                q[u] = cu as usize;
                q[v] = cv as usize;
                f(b, q[0], q[1], q[2]);
            }
        }
    }
}

/// **Цилиндр вдоль оси** `axis`: круг радиуса `r` без середины
/// радиуса `hole`, протянутый по оси на `span`. Центр — в середине
/// воксела `c`.
///
/// Одна кисть на три направления, потому что круг нужен во всех трёх:
/// башня и люк — по `Y`, ствол и выхлоп — по `X`, опорный каток и
/// шарнир ноги — по `Z`. Три отдельные копии разъехались бы по
/// правилам обрезки, и разъехались бы молча.
pub(crate) fn cyl(
    b: &mut UnitBody,
    axis: usize,
    span: (usize, usize),
    c: (i32, i32),
    r: i32,
    hole: i32,
    p: Part,
) {
    cyl_h(
        b,
        axis,
        span,
        (2 * c.0 + 1, 2 * c.1 + 1),
        2 * r + 1,
        2 * hole,
        |b, x, y, z| b.set(x, y, z, p),
    );
}

/// Цилиндр с отделкой: форма и материал одним вызовом.
///
/// Восемь аргументов — ось, диапазон, центр, радиус, отверстие,
/// роль и отделка: у цилиндра их столько и есть, и связывать их
/// в запись значило бы заводить тип ради одного вызова.
#[allow(clippy::too_many_arguments)]
pub(crate) fn cyl_f(
    b: &mut UnitBody,
    axis: usize,
    span: (usize, usize),
    c: (i32, i32),
    r: i32,
    hole: i32,
    p: Part,
    code: u8,
) {
    cyl_h(
        b,
        axis,
        span,
        (2 * c.0 + 1, 2 * c.1 + 1),
        2 * r + 1,
        2 * hole,
        |b, x, y, z| {
            b.set(x, y, z, p);
            b.set_finish(x, y, z, code);
        },
    );
}

/// **Восьмигранник** вдоль оси `axis` с осью на узле `c`: квадрат
/// полуширины `r2` полувокселей с углами, срезанными на треть.
///
/// Сечение звена, а не круг, и это выбор по снимку: у цифрового круга
/// шире пяти вокселей контур идёт ступенями, и фаска мешера
/// превращает каждую ступень в бусину — ствол читался нанизанными
/// шайбами. У восьмигранника только два наклона, ноль и сорок пять
/// градусов, и оба фаска сливает в гладкую грань (ADR-224). Сечения
/// до четырёх вокселей и так квадраты: их восьмигранник — это фаска.
pub(crate) fn oct_at(
    b: &mut UnitBody,
    axis: usize,
    span: (usize, usize),
    c: (i32, i32),
    r2: i32,
    p: Part,
    code: u8,
) {
    let n = [b.vol.nx as i32, b.vol.ny as i32, b.vol.nz as i32];
    let (u, v) = others(axis);
    let c2 = (2 * c.0, 2 * c.1);
    // Середины вокселей стоят на нечётных полувокселях от узла;
    // `m` — крайняя из них внутри радиуса. Угол срезается на треть
    // полуширины, а сечения до четырёх вокселей остаются квадратами.
    let m = if r2 % 2 == 0 { r2 - 1 } else { r2 - 2 }.max(1);
    let cut = 2 * m - if m <= 3 { 0 } else { 2 * (m / 3) };
    let (u0, u1) = ((c2.0 - m - 1).div_euclid(2), (c2.0 + m + 1).div_euclid(2));
    let (v0, v1) = ((c2.1 - m - 1).div_euclid(2), (c2.1 + m + 1).div_euclid(2));
    for a in span.0..span.1.min(n[axis] as usize) {
        for cv in v0.max(0)..=v1.min(n[v] - 1) {
            for cu in u0.max(0)..=u1.min(n[u] - 1) {
                let (du, dv) = ((2 * cu + 1 - c2.0).abs(), (2 * cv + 1 - c2.1).abs());
                if du > m || dv > m || du + dv > cut {
                    continue;
                }
                let mut q = [0usize; 3];
                q[axis] = a;
                q[u] = cu as usize;
                q[v] = cv as usize;
                b.set(q[0], q[1], q[2], p);
                if code != FINISH_BY_PART {
                    b.set_finish(q[0], q[1], q[2], code);
                }
            }
        }
    }
}

/// Слой-диск: круг радиуса `r` вокруг `(cx, cz)`, высотой `ys`.
pub(crate) fn disc(b: &mut UnitBody, c: (usize, usize), r: i32, ys: (usize, usize), p: Part) {
    cyl(b, 1, ys, (c.0 as i32, c.1 as i32), r, 0, p);
}

/// Кольцо: диск радиуса `r` без середины радиуса `hole`.
///
/// Нужно на пояса брони и дульные насадки — сплошной диск там
/// читается как пробка.
pub(crate) fn ring(
    b: &mut UnitBody,
    c: (usize, usize),
    r: i32,
    hole: i32,
    ys: (usize, usize),
    p: Part,
) {
    cyl(b, 1, ys, (c.0 as i32, c.1 as i32), r, hole, p);
}

/// Обход шара в полувокселях.
fn ball_h(
    b: &mut UnitBody,
    c2: (i32, i32, i32),
    r2: i32,
    mut f: impl FnMut(&mut UnitBody, usize, usize, usize),
) {
    let n = [b.vol.nx as i32, b.vol.ny as i32, b.vol.nz as i32];
    let lo = |c: i32| ((c - r2 - 1).div_euclid(2)).max(0);
    let hi = |c: i32, k: usize| ((c + r2 + 1).div_euclid(2)).min(n[k] - 1);
    let lim = r2 * r2 - 1;
    for z in lo(c2.2)..=hi(c2.2, 2) {
        let dz = 2 * z + 1 - c2.2;
        for y in lo(c2.1)..=hi(c2.1, 1) {
            let dy = 2 * y + 1 - c2.1;
            for x in lo(c2.0)..=hi(c2.0, 0) {
                let dx = 2 * x + 1 - c2.0;
                if dx * dx + dy * dy + dz * dz <= lim {
                    f(b, x as usize, y as usize, z as usize);
                }
            }
        }
    }
}

/// **Шар** радиуса `r` вокруг середины воксела `c`. Это шарнир:
/// колено, плечо, погон башенки.
///
/// Нога из двух брусков читается как труба с изломом; тот же излом
/// с шаром в нём читается как сустав, и разница видна с боевой камеры
/// (ADR-219).
pub(crate) fn ball(b: &mut UnitBody, c: (i32, i32, i32), r: i32, p: Part) {
    ball_h(
        b,
        (2 * c.0 + 1, 2 * c.1 + 1, 2 * c.2 + 1),
        2 * r + 1,
        |b, x, y, z| b.set(x, y, z, p),
    );
}

/// Шар с центром **на узле решётки** `c` и отделкой — сустав,
/// в который упирается звено-сегмент: узел и есть точка крепления.
pub(crate) fn joint(b: &mut UnitBody, c: (i32, i32, i32), r: i32, p: Part, code: u8) {
    ball_h(b, (2 * c.0, 2 * c.1, 2 * c.2), 2 * r, |b, x, y, z| {
        b.set(x, y, z, p);
        b.set_finish(x, y, z, code);
    });
}

/// **Звено конечности с шарниром на дальнем конце** — в корневом томе.
///
/// Звено круглое, а не квадратное: при вокселе 100 мм бедро толщиной
/// семь вокселей — это 70 см, и квадратное сечение на таком размере
/// читается как балка. Шар на конце и делает излом суставом.
///
/// **Считается расстояние до отрезка, а не цепочка шаров.** Первая
/// редакция ставила шар на каждый воксель длины, и на косом звене
/// центры ложились по решётке через один: нога выходила бугристой,
/// как бечёвка с узлами. Дробить шаг бесполезно — центры всё равно
/// целые. Честный конус получается только от расстояния до оси.
///
/// Косое звено **на виду** этой кистью больше не рисуется: любой
/// промежуточный наклон в осевой сетке — лестница (ADR-224), и звено
/// стало сегментом ([`rod`]). Здесь остаётся кость под сегментом
/// и осевые детали: ствол, мачта, кран.
///
/// Арифметика в `i64` и без деления: ближайшая точка держится
/// умноженной на квадрат длины, и сравниваются квадраты в том же
/// масштабе (`CLAUDE.md` — корня в симуляции нет).
pub(crate) fn limb(
    b: &mut UnitBody,
    a: (i32, i32, i32),
    e: (i32, i32, i32),
    r0: i32,
    r1: i32,
    joint: i32,
    p: Part,
) {
    let d = [(e.0 - a.0) as i64, (e.1 - a.1) as i64, (e.2 - a.2) as i64];
    let den = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    let rmax = r0.max(r1).max(joint);
    if den == 0 {
        ball(b, a, rmax, p);
        return;
    }
    let n = [b.vol.nx as i32, b.vol.ny as i32, b.vol.nz as i32];
    let lo = [
        (a.0.min(e.0) - rmax).max(0),
        (a.1.min(e.1) - rmax).max(0),
        (a.2.min(e.2) - rmax).max(0),
    ];
    let hi = [
        (a.0.max(e.0) + rmax).min(n[0] - 1),
        (a.1.max(e.1) + rmax).min(n[1] - 1),
        (a.2.max(e.2) + rmax).min(n[2] - 1),
    ];
    let s = [a.0 as i64, a.1 as i64, a.2 as i64];
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                let q = [x as i64, y as i64, z as i64];
                let w = [q[0] - s[0], q[1] - s[1], q[2] - s[2]];
                let t = (w[0] * d[0] + w[1] * d[1] + w[2] * d[2]).clamp(0, den);
                let mut dist2 = 0i64;
                for k in 0..3 {
                    let g = q[k] * den - (s[k] * den + d[k] * t);
                    dist2 += g * g;
                }
                let r = r0 as i64 * den + (r1 - r0) as i64 * t;
                // Тот же цифровой круг, что в `in_disc`, в масштабе `den`.
                if dist2 <= r * r + r * den {
                    b.set(x as usize, y as usize, z as usize, p);
                }
            }
        }
    }
    if joint > 0 {
        ball(b, e, joint, p);
    }
}

/// **Кость**: шестисвязная цепочка вокселей от `a` до `e` и, при
/// `r > 0`, звено радиуса `r` вокруг неё.
///
/// Это скрытый объём под звеном-сегментом (ADR-231): симуляция видит
/// только корневой том, и кость держит связность ноги с ядром и массу
/// под попадание. Цепочка нужна отдельно от [`limb`]: конус нулевого
/// радиуса на косой линии рассыпается на отдельные воксели, а флуд
/// связности шестисвязный. Шаг делается по той оси, где накопленная
/// ошибка больше, — так цепочка отклоняется от прямой не дальше
/// воксела и прячется в звене радиуса два.
pub(crate) fn bone(b: &mut UnitBody, a: (i32, i32, i32), e: (i32, i32, i32), r: i32, p: Part) {
    let n = [b.vol.nx as i32, b.vol.ny as i32, b.vol.nz as i32];
    let d = [e.0 - a.0, e.1 - a.1, e.2 - a.2];
    let total = d[0].abs() + d[1].abs() + d[2].abs();
    let mut c = [a.0, a.1, a.2];
    let mut err = [0i32; 3];
    let mut chain = Vec::with_capacity(total as usize + 1);
    chain.push(c);
    for _ in 0..total {
        let mut best = 0;
        for k in 0..3 {
            err[k] += d[k].abs();
            if err[k] > err[best] {
                best = k;
            }
        }
        err[best] -= total;
        c[best] += d[best].signum();
        chain.push(c);
    }
    for c in &chain {
        if (0..3).all(|k| c[k] >= 0 && c[k] < n[k]) {
            b.set(c[0] as usize, c[1] as usize, c[2] as usize, p);
        }
    }
    // Утолщение короче цепочки на радиус с каждого конца: шапка
    // звена у самого узла выглянула бы за торец сегмента.
    let k = (r + 1) as usize;
    if r > 0 && chain.len() > 2 * k {
        let (s, f) = (chain[k], chain[chain.len() - 1 - k]);
        limb(b, (s[0], s[1], s[2]), (f[0], f[1], f[2]), r, r, 0, p);
    }
}

/// Коробка с полыми стенами: четыре стены, пол и крыша толщиной `t`.
///
/// Постройка — это прежде всего оболочка. Сплошной параллелепипед
/// того же габарита стоил бы вчетверо больше вокселей и читался бы
/// как брусок; оболочка даёт ту же форму и оставляет внутренность
/// под механизмы.
pub(crate) fn shell(
    b: &mut UnitBody,
    xs: (usize, usize),
    ys: (usize, usize),
    zs: (usize, usize),
    t: usize,
    p: Part,
) {
    fill(b, (xs.0, xs.0 + t), ys, zs, p);
    fill(b, (xs.1.saturating_sub(t), xs.1), ys, zs, p);
    fill(b, xs, ys, (zs.0, zs.0 + t), p);
    fill(b, xs, ys, (zs.1.saturating_sub(t), zs.1), p);
    fill(b, xs, (ys.0, ys.0 + t), zs, p);
    fill(b, xs, (ys.1.saturating_sub(t), ys.1), zs, p);
}

// ---------------------------------------------------------------------
// Сегменты: повёрнутые части тела и скрытый объём под ними.
// ---------------------------------------------------------------------

/// Матрица поворота крепления в `Fx`: крен, потом тангаж, потом
/// рыскание — тот же порядок и те же знаки, что у рендера
/// (`render::model::attach_rotation`), и это сторожится тестом
/// `shadow_follows_the_renderer` на стороне рендера.
fn rotation(a: &Attach) -> [[Fx; 3]; 3] {
    let (sy, cy) = (fx::angle::sin(a.yaw), fx::angle::cos(a.yaw));
    let (sp, cp) = (fx::angle::sin(a.pitch), fx::angle::cos(a.pitch));
    let (sr, cr) = (fx::angle::sin(a.roll), fx::angle::cos(a.roll));
    let (z, o) = (Fx::ZERO, Fx::ONE);
    let ry = [[cy, z, sy], [z, o, z], [sy.neg(), z, cy]];
    let rz = [[cp, sp.neg(), z], [sp, cp, z], [z, z, o]];
    let rx = [[o, z, z], [z, cr, sr.neg()], [z, sr, cr]];
    mat_mul(&ry, &mat_mul(&rz, &rx))
}

fn mat_mul(a: &[[Fx; 3]; 3], b: &[[Fx; 3]; 3]) -> [[Fx; 3]; 3] {
    let mut m = [[Fx::ZERO; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = a[i][0]
                .mul(b[0][j])
                .add(a[i][1].mul(b[1][j]))
                .add(a[i][2].mul(b[2][j]));
        }
    }
    m
}

fn mat_vec(m: &[[Fx; 3]; 3], v: [Fx; 3]) -> [Fx; 3] {
    let row = |r: &[Fx; 3]| r[0].mul(v[0]).add(r[1].mul(v[1])).add(r[2].mul(v[2]));
    [row(&m[0]), row(&m[1]), row(&m[2])]
}

/// Обратный поворот: матрица ортогональна, обратная — транспонированная.
fn transposed(m: &[[Fx; 3]; 3]) -> [[Fx; 3]; 3] {
    let mut t = [[Fx::ZERO; 3]; 3];
    for (i, row) in t.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = m[j][i];
        }
    }
    t
}

/// Рамка сегмента в вокселях **корня**: точка корня `p` для точки
/// сегмента `q` — это `t + R·q`. Цепочка родителей сложена; рёбра
/// вокселей у всех звеньев цепочки обязаны совпадать.
fn frame(b: &UnitBody, seg: usize) -> ([Fx; 3], [[Fx; 3]; 3]) {
    let s = &b.segments[seg];
    let a = &s.attach;
    assert_eq!(
        s.body.mm, b.mm,
        "сегмент {seg}: ребро воксела не как у корня"
    );
    let r = rotation(a);
    let pivot = [
        Fx::from_int(a.pivot.0 as i32),
        Fx::from_int(a.pivot.1 as i32),
        Fx::from_int(a.pivot.2 as i32),
    ];
    let at = [
        Fx::from_int(a.at.0 as i32),
        Fx::from_int(a.at.1 as i32),
        Fx::from_int(a.at.2 as i32),
    ];
    // Местное: `p_parent = at + R·(q − pivot)`.
    let rp = mat_vec(&r, pivot);
    let t_local = [at[0].sub(rp[0]), at[1].sub(rp[1]), at[2].sub(rp[2])];
    if a.parent == 0 {
        return (t_local, r);
    }
    let (tp, rpar) = frame(b, a.parent as usize - 1);
    let rt = mat_vec(&rpar, t_local);
    (
        [tp[0].add(rt[0]), tp[1].add(rt[1]), tp[2].add(rt[2])],
        mat_mul(&rpar, &r),
    )
}

/// **Скрытый объём под сегментом**: заполнить в корневом томе все
/// воксели, **целиком** лежащие внутри коробки `lo..hi`, заданной
/// в вокселях сегмента `seg` и повёрнутой вместе с ним.
///
/// Это ступени под наклонным листом и кость внутри звена (ADR-231):
/// симуляция видит только корневой том, и объём под попадание обязан
/// лежать там. Воксель берётся, когда все восемь его углов внутри
/// коробки — иначе он выглянул бы из-под листа лесенкой; коробку
/// поэтому обычно продлевают **внутрь** тела за пределы самого
/// сегмента (`below`/`above` у [`mount`]), чтобы ступени дошли
/// до корпуса.
///
/// Поворот считается таблицей `fx::angle` — целая арифметика, как всё
/// в теле. Допуск в одну шестьдесят четвёртую воксела принимает угол,
/// лежащий ровно на грани коробки: без него ступень под листом
/// на сорок пять градусов теряла каждый второй ряд.
pub(crate) fn shadow(
    b: &mut UnitBody,
    seg: usize,
    lo: (i32, i32, i32),
    hi: (i32, i32, i32),
    p: Part,
) {
    let (t, r) = frame(b, seg);
    let rt = transposed(&r);
    let n = [b.vol.nx as i32, b.vol.ny as i32, b.vol.nz as i32];
    let lo_f = [Fx::from_int(lo.0), Fx::from_int(lo.1), Fx::from_int(lo.2)];
    let hi_f = [Fx::from_int(hi.0), Fx::from_int(hi.1), Fx::from_int(hi.2)];
    // Габарит коробки в корне — по её восьми углам.
    let mut bmin = [i32::MAX; 3];
    let mut bmax = [i32::MIN; 3];
    for k in 0..8 {
        let q = [
            if k & 1 != 0 { hi_f[0] } else { lo_f[0] },
            if k & 2 != 0 { hi_f[1] } else { lo_f[1] },
            if k & 4 != 0 { hi_f[2] } else { lo_f[2] },
        ];
        let w = mat_vec(&r, q);
        for i in 0..3 {
            let v = t[i].add(w[i]);
            bmin[i] = bmin[i].min(v.to_int_trunc() - 1);
            bmax[i] = bmax[i].max(v.to_int_trunc() + 1);
        }
    }
    let eps = Fx::from_ratio(1, 64);
    for z in bmin[2].max(0)..=bmax[2].min(n[2] - 1) {
        for y in bmin[1].max(0)..=bmax[1].min(n[1] - 1) {
            for x in bmin[0].max(0)..=bmax[0].min(n[0] - 1) {
                let inside = (0..8).all(|k| {
                    let c = [
                        Fx::from_int(x + (k & 1)),
                        Fx::from_int(y + ((k >> 1) & 1)),
                        Fx::from_int(z + ((k >> 2) & 1)),
                    ];
                    let d = [c[0].sub(t[0]), c[1].sub(t[1]), c[2].sub(t[2])];
                    let q = mat_vec(&rt, d);
                    (0..3).all(|i| q[i] >= lo_f[i].sub(eps) && q[i] <= hi_f[i].add(eps))
                });
                if inside {
                    b.set(x as usize, y as usize, z as usize, p);
                }
            }
        }
    }
}

/// Осевой том **листа**: `len` вдоль `X`, `thick` по `Y`, `wide`
/// по `Z`, сплошной, с отделкой. Наружная грань — верхняя (`y = thick`).
pub(crate) fn slab(len: u8, thick: u8, wide: u8, mm: u16, p: Part, code: u8) -> UnitBody {
    let mut s = UnitBody::new_scaled(len.max(1), thick.max(1), wide.max(1), 1, mm);
    fill(
        &mut s,
        (0, len as usize),
        (0, thick as usize),
        (0, wide as usize),
        p,
    );
    mark(
        &mut s,
        (0, len as usize),
        (0, thick as usize),
        (0, wide as usize),
        code,
    );
    s
}

/// Посадить сегмент на тело и положить под него скрытый объём:
/// коробка сегмента, продлённая на `below` вокселей в его `−Y`
/// и на `above` в `+Y`. Возвращает номер сегмента — для потомков.
pub(crate) fn mount(
    b: &mut UnitBody,
    a: Attach,
    seg: UnitBody,
    below: i32,
    above: i32,
    p: Part,
) -> u8 {
    let (nx, ny, nz) = (seg.vol.nx as i32, seg.vol.ny as i32, seg.vol.nz as i32);
    b.push_segment(a, seg);
    let k = b.segments.len() - 1;
    if below >= 0 || above >= 0 {
        shadow(b, k, (0, -below, 0), (nx, ny + above, nz), p);
    }
    (k + 1) as u8
}

/// Осевой том **звена**: круглое, вдоль своей оси `X`, длиной `len`,
/// радиус в полувокселях от `r2a` у начала до `r2e` у конца, ось
/// на узле решётки `(r, r)` — там же, куда кладётся шарнир.
///
/// Звено сегмента рисуется **вдоль оси** и потому без единой ступени:
/// в этом и смысл сегмента (ADR-231). Толщина тает к дальнему концу —
/// нога, у которой голень той же толщины, что бедро, читается трубой.
pub(crate) fn link(len: u8, r2a: i32, r2e: i32, mm: u16, p: Part, code: u8) -> UnitBody {
    let r = (r2a.max(r2e) + 1) / 2;
    let w = (2 * r).max(1) as u8;
    let mut s = UnitBody::new_scaled(len.max(1), w, w, 1, mm);
    let last = (len as i32 - 1).max(1);
    for x in 0..len as usize {
        let r2 = r2a + (r2e - r2a) * x as i32 / last;
        oct_at(&mut s, 0, (x, x + 1), (r, r), r2.max(2), p, code);
    }
    s
}

/// Радиус тома звена — узел, на который ложится шарнир.
pub(crate) fn link_pivot(r2a: i32, r2e: i32) -> u8 {
    ((r2a.max(r2e) + 1) / 2) as u8
}

/// Целая длина отрезка между узлами — округлённая, а не усечённая.
/// Корень законен: один раз при сборке шаблона (ADR-071).
pub(crate) fn span(a: (i32, i32, i32), e: (i32, i32, i32)) -> u8 {
    let d = [(e.0 - a.0) as i64, (e.1 - a.1) as i64, (e.2 - a.2) as i64];
    let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    (((4 * d2).isqrt() + 1) / 2).clamp(1, 255) as u8
}

/// **Звено-сегмент от узла `a` к узлу `e`** корневого тома — с костью
/// внутри. Радиусы в полувокселях у начала и у конца; кость — радиуса
/// `bone_r` вокселей (ноль — одна цепочка). Ствол под углом, бедро,
/// голень, шток, распорка — всё это `rod`.
///
/// Кость тоньше звена на два воксела с лишним: цепочка отклоняется от
/// прямой на воксель, угол таблицы синуса — на две десятых градуса,
/// и оба запаса вместе обязаны прятаться под поверхностью звена.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rod(
    b: &mut UnitBody,
    a: (i32, i32, i32),
    e: (i32, i32, i32),
    r2a: i32,
    r2e: i32,
    bone_r: i32,
    p: Part,
    code: u8,
    turret: bool,
) -> u8 {
    let len = span(a, e);
    let seg = link(len, r2a, r2e, b.mm, p, code);
    let r = link_pivot(r2a, r2e);
    let at = (a.0 as i16, a.1 as i16, a.2 as i16);
    let d = (e.0 - a.0, e.1 - a.1, e.2 - a.2);
    let attach = Attach {
        turret,
        ..Attach::aimed(at, (0, r, r), d)
    };
    // Кость идёт по вокселям, лежащим **впереди** узла `a` и позади
    // узла `e` вдоль хода звена: узел — угол воксела, и воксель, чей
    // угол это узел, по каждой оси выбирается с той стороны, куда
    // звено идёт. Так цепочка не выглядывает за торцы звена.
    if bone_r >= 0 {
        let ahead = |v: i32, d: i32| if d < 0 { v - 1 } else { v };
        let behind = |v: i32, d: i32| if d > 0 { v - 1 } else { v };
        let va = (ahead(a.0, d.0), ahead(a.1, d.1), ahead(a.2, d.2));
        let ve = (behind(e.0, d.0), behind(e.1, d.1), behind(e.2, d.2));
        bone(b, va, ve, bone_r, p);
    }
    b.push_segment(attach, seg);
    b.segments.len() as u8
}

/// **Нога из двух звеньев** — бедро и голень сегментами, суставы
/// стальными шарами на узлах, кости внутри (ADR-231).
///
/// Узлы `hip`, `knee`, `ankle` — в вокселях корня. `rt`/`rs` —
/// радиусы бедра и голени у их начала, в полувокселях; к концу
/// звено тоньше на воксель. Стопа — дело вызывающего: у бота она
/// с пальцами, у паука — коготь.
pub(crate) fn leg(
    b: &mut UnitBody,
    hip: (i32, i32, i32),
    knee: (i32, i32, i32),
    ankle: (i32, i32, i32),
    rt: i32,
    rs: i32,
    p: Part,
) {
    // Moving pieces are segments, just like the original strider. No static
    // bone is left in the root to protrude when a joint bends (ADR-234).
    let (jt, jk, ja) = (rt / 2 + 1, (rt - 2).max(rs) / 2 + 1, (rs / 2).max(1));
    joint(b, hip, jt, p, FINISH_STEEL);
    let thigh = rod(b, hip, knee, rt, rt - 2, -1, p, FINISH_BY_PART, false) - 1;
    let shin = rod(b, knee, ankle, rs, rs - 2, -1, p, FINISH_BY_PART, false) - 1;
    let mut ball = |at: (i32, i32, i32), radius: i32| {
        let n = (radius * 2 + 1) as u8;
        let mut part = UnitBody::new_scaled(n, n, n, 1, b.mm);
        joint(&mut part, (radius, radius, radius), radius, p, FINISH_STEEL);
        let index = b.segments.len() as u8;
        b.push_segment(
            Attach::flat(
                (at.0 as i16, at.1 as i16, at.2 as i16),
                (radius as u8, radius as u8, radius as u8),
            ),
            part,
        );
        index
    };
    let knee_ball = ball(knee, jk);
    let ankle_ball = ball(ankle, ja);
    let point = |v: (i32, i32, i32)| (v.0 as i16, v.1 as i16, v.2 as i16);
    let support = u8::from(hip.0 < b.vol.nx as i32 / 2) * 2 + u8::from(hip.2 < b.vol.nz as i32 / 2);
    b.push_leg(crate::unit::Leg {
        hip: point(hip),
        knee: point(knee),
        ankle: point(ankle),
        thigh,
        shin,
        knee_ball: Some(knee_ball),
        ankle_ball: Some(ankle_ball),
        foot: None,
        support,
    });
}

/// **Трапециевидный лист**: верхняя кромка `top` вдоль `X`, нижняя
/// `bot` (шире или уже), высота `slant` по `Z`, толщина `thick`.
/// Возвращает том и `x` узла, с которого начинается верхняя кромка:
/// шарнир ставится на неё, а лист ниже кромки расходится в обе
/// стороны поровну.
pub(crate) fn trapezoid(
    top: u8,
    bot: u8,
    thick: u8,
    slant: u8,
    mm: u16,
    p: Part,
    code: u8,
) -> (UnitBody, u8) {
    let w = top.max(bot);
    let off = (w - top) / 2;
    let mut s = UnitBody::new_scaled(w.max(1), thick.max(1), slant.max(1), 1, mm);
    let (top, bot, w, slant) = (top as i32, bot as i32, w as i32, slant.max(1) as i32);
    for z in 0..slant {
        // Ширина ряда — линейно от верхней кромки к нижней; ряд
        // центрируется на середине тома.
        let width = top + (bot - top) * (2 * z + 1) / (2 * slant);
        let x0 = (w - width) / 2;
        fill(
            &mut s,
            (x0.max(0) as usize, (x0 + width).max(0) as usize),
            (0, thick as usize),
            (z as usize, z as usize + 1),
            p,
        );
    }
    mark(
        &mut s,
        (0, w as usize),
        (0, thick as usize),
        (0, slant as usize),
        code,
    );
    (s, off)
}

/// **Венец листов**: `n` наклонных листов вокруг оси `Y` от верхнего
/// многоугольника радиуса `r_top` на высоте `y_top` к нижнему
/// радиуса `r_bot` на `y_bot`. Усечённый конус из листов, а не
/// ступенями: купол паука, голова турели, надстройка крепости.
///
/// Радиус многоугольника задаёт **вершины**, лист идёт по ребру.
/// Обход по часовой стрелке при взгляде сверху (`x` вправо,
/// `z` вверх): наружу у листа смотрит местная `+Z` до рыскания.
/// Под каждым листом ложится тень до глубины `deep` — она и есть
/// скрытый объём венца; середину заполняет вызывающий.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plate_ring(
    b: &mut UnitBody,
    c: (i32, i32),
    y_top: i32,
    r_top: i32,
    y_bot: i32,
    r_bot: i32,
    n: usize,
    thick: u8,
    deep: i32,
    turret: bool,
    p: Part,
    code: u8,
) {
    let n = n.max(3);
    let step = (crate::unit::TURN as i64 / n as i64) as i32;
    let half = Fx::from_ratio(1, 2);
    // Вершина `k` верхнего многоугольника. Угол растёт по часовой
    // стрелке сверху: положительное рыскание уводит `+X` к `−Z`.
    let vertex = |k: usize| -> (i32, i32) {
        let a = (k as i32) * step;
        let (s, cs) = (fx::angle::sin(a), fx::angle::cos(a));
        (
            c.0 + cs.mul(Fx::from_int(r_top)).add(half).to_int_trunc(),
            c.1 - s.mul(Fx::from_int(r_top)).add(half).to_int_trunc(),
        )
    };
    // Длина ребра и апофема — от радиуса, одни на все листы.
    let edge = |r: i32| {
        fx::angle::sin(step / 2)
            .mul(Fx::from_int(2 * r))
            .add(half)
            .to_int_trunc()
    };
    let apo = |r: i32| {
        fx::angle::cos(step / 2)
            .mul(Fx::from_int(r))
            .add(half)
            .to_int_trunc()
    };
    let (top, bot) = (edge(r_top).max(1), edge(r_bot).max(1));
    let (h, da) = (y_top - y_bot, apo(r_bot) - apo(r_top));
    let slant = span((0, 0, 0), (da, h, 0)) as i32;
    // Лист вертикален при равных апофемах, дальше отклоняется наружу.
    let roll = crate::unit::TURN / 4 - fx::angle::diff(fx::angle::atan2(da as i64, h as i64), 0);
    let mm = b.mm;
    for k in 0..n {
        let (x0, z0) = vertex(k);
        let (x1, z1) = vertex(k + 1);
        let (seg, off) = trapezoid(top as u8, bot as u8, thick, slant as u8, mm, p, code);
        let attach = Attach {
            roll,
            turret,
            ..Attach::aimed(
                (x0 as i16, y_top as i16, z0 as i16),
                (off, thick, 0),
                (x1 - x0, 0, z1 - z0),
            )
        };
        mount(b, attach, seg, deep, 0, p);
    }
}

/// **Башня из листов**: крыша-многоугольник со скулами у лба,
/// скрытый короб под ней и наклонные листы по рёбрам — два борта,
/// две скулы; лоб остаётся под маску, корма — вертикальная.
///
/// `xs`/`zs` — короб по крыше, `ys` — высота; `cheek` — насколько
/// скула отступает по `x` и по `z`; `lean` — наклон листов от
/// вертикали; `slant` — длина листа вниз по наклону.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plated_turret(
    b: &mut UnitBody,
    xs: (usize, usize),
    ys: (usize, usize),
    zs: (usize, usize),
    cheek: (usize, usize),
    lean: i32,
    slant: u8,
    thick: u8,
    turret: bool,
    p: Part,
    code: u8,
) -> (u8, u8) {
    let (x0, x1) = (xs.0 as i32, xs.1 as i32);
    let (z0, z1) = (zs.0 as i32, zs.1 as i32);
    let (cx, cz) = (cheek.0 as i32, cheek.1.max(1) as i32);
    // Короб: ряд `z` — до ближней к нему точки ребра скулы.
    for z in z0..z1 {
        let xr = if z < z0 + cz {
            x1 - cx + (z - z0) * cx / cz
        } else if z >= z1 - cz {
            x1 - cx + (z1 - 1 - z) * cx / cz
        } else {
            x1
        };
        fill(
            b,
            (x0 as usize, xr.max(x0) as usize),
            ys,
            (z as usize, z as usize + 1),
            p,
        );
    }
    let (y, mm) = (ys.1 as i16, b.mm);
    let hang = |at: (i16, i16, i16), dir: (i32, i32, i32)| Attach {
        roll: crate::unit::TURN / 4 - lean,
        turret,
        ..Attach::aimed(at, (0, thick, 0), dir)
    };
    let side = |len: i32| slab(len.max(1) as u8, thick, slant, mm, p, code);
    let deep = slant as i32;
    // Борта: `+Z` от кормы к скуле, `−Z` от скулы к корме.
    let sl = x1 - cx - x0;
    mount(
        b,
        hang((x0 as i16, y, z1 as i16), (sl, 0, 0)),
        side(sl),
        deep,
        0,
        p,
    );
    mount(
        b,
        hang(((x1 - cx) as i16, y, z0 as i16), (-sl, 0, 0)),
        side(sl),
        deep,
        0,
        p,
    );
    // Скулы: `−Z` от лба к борту, `+Z` от борта ко лбу. Номера
    // возвращаются: на скулы вешают потомков — гранатомёты, фары.
    let cl = span((0, 0, 0), (cx, 0, cz)) as i32;
    let l = mount(
        b,
        hang((x1 as i16, y, (z0 + cz) as i16), (-cx, 0, -cz)),
        side(cl),
        deep,
        0,
        p,
    );
    let r = mount(
        b,
        hang(((x1 - cx) as i16, y, z1 as i16), (cx, 0, -cz)),
        side(cl),
        deep,
        0,
        p,
    );
    (l, r)
}

/// **Юбка**: наклонный лист вдоль борта от кромки `(x0, y, z)` длиной
/// `len` по `X`, свисающий наружу на сторону `side` (`−1` — к `−Z`,
/// `+1` — к `+Z`) с наклоном `lean` от вертикали. Бортовой экран,
/// щека над тазобедренным шаром, скос надстройки.
#[allow(clippy::too_many_arguments)]
pub(crate) fn skirt(
    b: &mut UnitBody,
    x0: i32,
    len: i32,
    y: i32,
    z: i32,
    side: i32,
    lean: i32,
    slant: u8,
    thick: u8,
    turret: bool,
    p: Part,
    code: u8,
) -> u8 {
    let seg = slab(len.max(1) as u8, thick, slant, b.mm, p, code);
    // Наружу смотрит местная `+Z` до рыскания: у борта `+Z` лист
    // идёт вдоль `+X`, у борта `−Z` — обратно.
    let (at, dir) = if side > 0 {
        ((x0 as i16, y as i16, z as i16), (len, 0, 0))
    } else {
        (((x0 + len) as i16, y as i16, z as i16), (-len, 0, 0))
    };
    let attach = Attach {
        roll: crate::unit::TURN / 4 - lean,
        turret,
        ..Attach::aimed(at, (0, thick, 0), dir)
    };
    mount(b, attach, seg, slant as i32, 0, p)
}

/// **Фара-потомок**: куб в три воксела со стеклом на дальней грани
/// по `X`. Вешается на лист: `Attach::aimed` от точки на наружной
/// грани листа по его нормали, шарнир — середина основания.
pub(crate) fn lamp(mm: u16, p: Part) -> UnitBody {
    let mut s = UnitBody::new_scaled(3, 3, 3, 1, mm);
    fill_f(&mut s, (0, 3), (0, 3), (0, 3), p, FINISH_STEEL);
    s.set_finish(2, 1, 1, crate::unit::FINISH_OPTIC);
    s
}

/// **Гидроцилиндр** вдоль ноги: гильза на две трети длины и шток
/// на треть, оба сегментами, оба стальные. Ставится снаружи излома —
/// там, где звенья расходятся. Кости под ним нет: он тоньше кости
/// и держится за оба своих конца, как настоящий.
pub(crate) fn ram(b: &mut UnitBody, a: (i32, i32, i32), e: (i32, i32, i32), r2: i32, p: Part) {
    let m = (
        a.0 + (e.0 - a.0) * 2 / 3,
        a.1 + (e.1 - a.1) * 2 / 3,
        a.2 + (e.2 - a.2) * 2 / 3,
    );
    rod(b, a, m, r2, r2, -1, p, FINISH_STEEL, false);
    rod(
        b,
        m,
        e,
        (r2 - 2).max(2),
        (r2 - 2).max(2),
        -1,
        p,
        FINISH_RUBBER,
        false,
    );
}

// ---------------------------------------------------------------------
// Ходовая часть.
// ---------------------------------------------------------------------

/// Гусеничная лента высотой `h` с траками — **крупная сетка**.
///
/// Остаётся для лент, в которые каток не помещается: при высоте меньше
/// десяти вокселей внутри петли нет места ни ему, ни просвету.
pub(crate) fn tracks(b: &mut UnitBody, xs: (usize, usize), h: usize, zs: (usize, usize), p: Part) {
    fill(b, xs, (0, h), zs, p);
    let mut x = xs.0 + 1;
    while x + 2 <= xs.1 {
        fill(b, (x, x + 2), (0, h + 1), zs, p);
        x += 4;
    }
    mark(b, xs, (0, h + 1), zs, FINISH_RUBBER);
}

/// Номер сектора, в который смотрит `(du, dv)`, из `n` штук.
///
/// Обход по периметру **квадрата**, а не по углу: арктангенса в целой
/// арифметике здесь не нужно, а зубу венца и траку ленты равномерности
/// квадрата хватает — у диагоналей сектор шире на четверть, и на венце
/// из восьми зубьев это не читается.
fn sector(du: i32, dv: i32, n: i32) -> i32 {
    let m = du.abs().max(dv.abs()).max(1);
    let s = if du == m {
        dv + m
    } else if dv == m {
        3 * m - du
    } else if du == -m {
        5 * m - dv
    } else {
        7 * m + du
    };
    (s * n / (8 * m)).clamp(0, n - 1)
}

/// **Гусеничная лента**: замкнутая петля, огибающая ходовую.
///
/// Лента — **оболочка капсулы** вокруг отрезка между осями ведущего
/// колеса и ленивца: снаружи радиус `r`, внутри `r − t`, и в торцах
/// петля сама заворачивается вокруг колёс. Две полосы, положенные
/// сверху и снизу, огибать нечего, и ходовая читалась аркадой
/// с проёмами, а не механизмом (ADR-224).
///
/// Траки — выступы наружу через `step`: по прямым ветвям шаг считается
/// вдоль оси, в торцах — по сектору вокруг колеса, потому что проекция
/// на закруглении стоит на месте и гребёнка сгустилась бы в комок.
/// Трак ставится **шагом наружу от наружной шкурки**, а не своим
/// кольцом радиусов: кольцо давало на закруглении одиночные воксели
/// без соседа внутрь, и трак висел в воздухе.
///
/// Арифметика в `i64` и без деления на длину — тот же приём, что
/// в [`limb`]. Вся лента — резина.
#[allow(clippy::too_many_arguments)]
fn belt(
    b: &mut UnitBody,
    zs: (usize, usize),
    a: (i32, i32),
    e: (i32, i32),
    r: i32,
    t: i32,
    step: i32,
    p: Part,
) {
    let (nx, ny, nz) = (b.vol.nx as i32, b.vol.ny as i32, b.vol.nz as i32);
    let d = [(e.0 - a.0) as i64, (e.1 - a.1) as i64];
    let den = d[0] * d[0] + d[1] * d[1];
    if den == 0 || step <= 0 {
        return;
    }
    let run = d[0].abs().max(d[1].abs()).max(1);
    let (ro, ri, rg) = (
        r as i64 * den,
        (r - t).max(0) as i64 * den,
        (r - 1).max(0) as i64 * den,
    );
    let disc2 = |rr: i64| rr * rr + rr * den;
    let lo = [(a.0.min(e.0) - r - 1).max(0), (a.1.min(e.1) - r - 1).max(0)];
    let hi = [
        (a.0.max(e.0) + r + 1).min(nx - 1),
        (a.1.max(e.1) + r + 1).min(ny - 1),
    ];
    let zz = (zs.0, zs.1.min(nz as usize));
    for y in lo[1]..=hi[1] {
        for x in lo[0]..=hi[0] {
            let w = [(x - a.0) as i64, (y - a.1) as i64];
            let tt = (w[0] * d[0] + w[1] * d[1]).clamp(0, den);
            let g = [
                x as i64 * den - (a.0 as i64 * den + d[0] * tt),
                y as i64 * den - (a.1 as i64 * den + d[1] * tt),
            ];
            let dist2 = g[0] * g[0] + g[1] * g[1];
            if dist2 > disc2(ro) || dist2 < ri * ri {
                continue;
            }
            for z in zz.0..zz.1 {
                b.set(x as usize, y as usize, z, p);
                b.set_finish(x as usize, y as usize, z, FINISH_RUBBER);
            }
            let skin = dist2 > disc2(rg);
            let on = if tt == 0 || tt == den {
                let c = if tt == 0 { a } else { e };
                sector(x - c.0, y - c.1, 4 * r) % step == 0
            } else {
                (tt / run).rem_euclid(step as i64) == 0
            };
            if !skin || !on {
                continue;
            }
            let (sx, sy) = if g[0].abs() >= g[1].abs() {
                (g[0].signum() as i32, 0)
            } else {
                (0, g[1].signum() as i32)
            };
            let (gx, gy) = (x + sx, y + sy);
            if gx < 0 || gy < 0 || gx >= nx || gy >= ny {
                continue;
            }
            for z in zz.0..zz.1 {
                b.set(gx as usize, gy as usize, z, p);
                b.set_finish(gx as usize, gy as usize, z, FINISH_RUBBER);
            }
        }
    }
}

/// **Ведущее колесо**: стальной диск радиуса `r` и зубчатый венец
/// до `out`, во всю ширину ленты `zs`, диск — в утопленной `zw`.
///
/// Зуб — это то, чем колесо отличается от катка, и единственная
/// деталь, по которой видно, какой конец ходовой ведущий. Венец —
/// тонкое кольцо `(out − 2, out]`, а не сектор от диска наружу:
/// сплошной клин делал ведущее вдвое крупнее катка. Зуб занимает
/// половину сектора и за ширину ленты по `Z` не выходит: слой,
/// в котором, кроме зуба, нет ничего, фаска превращала в ромбики.
#[allow(clippy::too_many_arguments)]
fn sprocket(
    b: &mut UnitBody,
    zs: (usize, usize),
    zw: (usize, usize),
    c: (i32, i32),
    r: i32,
    out: i32,
    teeth: i32,
    p: Part,
) {
    cyl_f(b, 2, zw, c, r, 0, p, FINISH_STEEL);
    let (nx, ny, nz) = (b.vol.nx as i32, b.vol.ny as i32, b.vol.nz as i32);
    let rin = (out - 2).max(r);
    let (r2, o2) = (rin * rin + rin, out * out + out);
    for z in zs.0..zs.1.min(nz as usize) {
        for dv in -out..=out {
            for du in -out..=out {
                let d2 = du * du + dv * dv;
                if d2 <= r2 || d2 > o2 || sector(du, dv, 2 * teeth) % 2 != 0 {
                    continue;
                }
                let (x, y) = (c.0 + du, c.1 + dv);
                if x < 0 || y < 0 || x >= nx || y >= ny {
                    continue;
                }
                b.set(x as usize, y as usize, z, p);
                b.set_finish(x as usize, y as usize, z, FINISH_STEEL);
            }
        }
    }
}

/// **Ступица**: стальной колпак радиуса `r`, выступающий из колеса
/// на воксель с каждой стороны — вровень с наружной гранью ленты,
/// дальше нельзя: в плоскости, где нет ничего, кроме него, фаска
/// делает из колпака пуговицу.
fn hub(b: &mut UnitBody, zw: (usize, usize), c: (i32, i32), r: i32, p: Part) {
    cyl_f(
        b,
        2,
        (zw.0.saturating_sub(1), zw.1 + 1),
        c,
        r,
        0,
        p,
        FINISH_STEEL,
    );
}

/// Опустить кусок ветви на воксель: каждый твёрдый воксель окна
/// повторяется строкой ниже вместе с отделкой, а **верхний твёрдый
/// воксель столбца** снимается. Сдвиг, а не утолщение: у столбца
/// с траком верх — трак, у соседнего — пусто, и снятие строки окна
/// делало бы ветвь толще вместо того, чтобы опустить.
fn drop_row(b: &mut UnitBody, xs: (usize, usize), ys: (usize, usize), zs: (usize, usize), p: Part) {
    let (nx, ny, nz) = (b.vol.nx as usize, b.vol.ny as usize, b.vol.nz as usize);
    let (y0, y1) = (ys.0.max(1), ys.1.min(ny));
    for z in zs.0..zs.1.min(nz) {
        for x in xs.0..xs.1.min(nx) {
            let top = (y0..y1).rev().find(|&y| b.vol.get(x, y, z));
            for y in y0..y1 {
                if b.vol.get(x, y, z) {
                    let code = b.finish_at(x, y, z);
                    b.set(x, y - 1, z, p);
                    b.set_finish(x, y - 1, z, code);
                }
            }
            if let Some(y) = top {
                b.remove(x, y, z);
            }
        }
    }
}

/// **Ходовая часть: замкнутая лента с траками, ведущее колесо
/// с венцом, ленивец, опорные катки со ступицами, поддерживающие
/// ролики и провис верхней ветви между ними.**
///
/// Это та кисть, ради которой воксель и уменьшался (ADR-219). Лента —
/// резина, всё, что крутится, — голая сталь (ADR-231). `h` — высота
/// ленты в вокселях; ниже десяти кисть вырождается в брус: катку
/// нужен радиус.
///
/// **Колёса утоплены от наружного борта на воксель.** Лента и колёса
/// в одну ширину сходились наружными гранями заподлицо, и борт
/// выходил сплошной плитой с проёмами. Утопленные видно сквозь
/// просвет между ветвями — это и есть разница между механизмом
/// и аркадой (ADR-224). Наружный борт — тот, что ближе к краю
/// габарита.
///
/// Каток — две трети внутренности ленты и стоит на внутренней грани
/// нижней ветви; шаг — диаметр плюс три воксела просвета; катки
/// разносятся по длине поровну, ничья при дележе — линейным индексом.
/// Роликов вдвое меньше, чем катков, и стоят они по своей разметке:
/// ролик в каждом просвете смыкал ходовую обратно в стенку.
pub(crate) fn running_gear(
    b: &mut UnitBody,
    xs: (usize, usize),
    h: usize,
    zs: (usize, usize),
    p: Part,
) {
    b.push_load_path(crate::unit::WeaponPath {
        min: [xs.0 as u8, 0, zs.0 as u8],
        max: [xs.1 as u8, h as u8, zs.1 as u8],
        axis: 0,
    });
    if h < 10 {
        tracks(b, xs, h, zs, p);
        return;
    }
    let nz = b.vol.nz as usize;
    let out_low = zs.0 <= nz.saturating_sub(zs.1);
    let zw = if out_low {
        (zs.0 + 1, zs.1)
    } else {
        (zs.0, zs.1 - 1)
    };
    // Наружный радиус ленты, её толщина, радиус катка. Ось ленты
    // поднята на воксель над днищем: трак растёт наружу, а у нижней
    // ветви наружу — вниз, и лежащей на нуле ленте расти было некуда.
    let r = ((h - 1) / 2) as i32;
    let t = ((r + 1) / 3).max(1);
    let rw = ((r - t) * 2 / 3).max(2);
    let (cy, cw) = (r + 1, t + 1 + rw);
    let (x0, x1) = (xs.0 as i32 + r, xs.1 as i32 - 1 - r);
    if x1 <= x0 {
        tracks(b, xs, h, zs, p);
        return;
    }

    belt(b, zs, (x0, cy), (x1, cy), r, t, 3, p);
    sprocket(b, zs, zw, (x0, cy), r - t, r + 1, 8, p);
    cyl_f(b, 2, zw, (x1, cy), r - t, 0, p, FINISH_STEEL);
    hub(b, zw, (x1, cy), 2, p);

    let (first, last) = (x0 + (r - t) + rw + 1, x1 - (r - t) - rw - 1);
    let step = 2 * rw + 3;
    let n = ((last - first) / step + 1).max(1);
    for i in 0..n {
        let cx = first + (last - first) * i / (n - 1).max(1);
        cyl_f(b, 2, zw, (cx, cw), rw, 0, p, FINISH_STEEL);
        hub(b, zw, (cx, cw), (rw / 2).max(1), p);
    }
    let ry = cy + (r - t);
    if ry - 1 > cw + rw {
        let nr = (n / 2 + 1).max(2);
        let (rf, rl) = (first + rw, last - rw);
        let at = |i: i32, m: i32| rf + (rl - rf) * i / m.max(1);
        for i in 0..nr {
            cyl_f(b, 2, zw, (at(i, nr - 1), ry), 1, 0, p, FINISH_STEEL);
        }
        for i in 0..nr - 1 {
            let cx = at(2 * i + 1, 2 * (nr - 1));
            drop_row(
                b,
                ((cx - 1).max(0) as usize, (cx + 2) as usize),
                ((cy + r - t) as usize, (cy + r + 2) as usize),
                zs,
                p,
            );
        }
    }
}

// ---------------------------------------------------------------------
// Мелочь, которая читается с игровой камеры.
// ---------------------------------------------------------------------

/// **Люк**: крышка на воксель выше крыши, с петлёй по краю.
///
/// Люк не украшение: это единственная деталь, по которой на крыше
/// видно масштаб машины. Плоская крыша в двадцать вокселей шириной
/// не говорит, два это метра или пять.
pub(crate) fn hatch(b: &mut UnitBody, c: (usize, usize), r: i32, y: usize, p: Part) {
    disc(b, c, r, (y, y + 1), p);
    ring(b, c, r + 1, r, (y, y + 1), p);
    // Ручка-скоба поперёк крышки — сталью, чтобы люк не сливался
    // с крышей в одно пятно краски.
    fill_f(
        b,
        (c.0 - 1, c.0 + 1),
        (y + 1, y + 2),
        (c.1 - r as usize / 2, c.1 + r as usize / 2 + 1),
        p,
        FINISH_STEEL,
    );
}

/// **Люк без скобы** — крышка вровень с петлёй, там, где над ним
/// нет места: крыша корпуса под самой линией башни.
pub(crate) fn lid(b: &mut UnitBody, c: (usize, usize), r: i32, y: usize, p: Part) {
    disc(b, c, r, (y, y + 1), p);
    ring(b, c, r + 1, r, (y, y + 1), p);
    mark_cyl(
        b,
        1,
        (y, y + 1),
        (c.0 as i32, c.1 as i32),
        r + 1,
        FINISH_STEEL,
    );
    mark_cyl(
        b,
        1,
        (y, y + 1),
        (c.0 as i32, c.1 as i32),
        r - 1,
        FINISH_PLATE,
    );
}

/// **Поручень** вдоль оси `axis` от `a` длиной `len`: стальной прут
/// на воксель над поверхностью и стойки к ней через четыре воксела.
/// Стойки — это связность: прут без них висит в воздухе, и это
/// ловит `every_body_is_whole_and_armed`.
pub(crate) fn rail(b: &mut UnitBody, a: (usize, usize, usize), axis: usize, len: usize, p: Part) {
    let mut q = [a.0, a.1, a.2];
    for i in 0..len {
        q[axis] = a_axis(a, axis) + i;
        b.set(q[0], q[1] + 1, q[2], p);
        b.set_finish(q[0], q[1] + 1, q[2], FINISH_STEEL);
        if i % 4 == 0 || i + 1 == len {
            b.set(q[0], q[1], q[2], p);
            b.set_finish(q[0], q[1], q[2], FINISH_STEEL);
        }
    }
}

#[inline]
fn a_axis(a: (usize, usize, usize), axis: usize) -> usize {
    match axis {
        0 => a.0,
        1 => a.1,
        _ => a.2,
    }
}

/// **Ящик ЗИП**: короб с утопленной крышкой и стальной скобой.
/// Полка в полтора метра без него — пустая плита.
pub(crate) fn crate_box(
    b: &mut UnitBody,
    xs: (usize, usize),
    ys: (usize, usize),
    zs: (usize, usize),
    p: Part,
) {
    fill(b, xs, ys, zs, p);
    if xs.1 > xs.0 + 2 && zs.1 > zs.0 + 2 && ys.1 > ys.0 + 1 {
        carve(
            b,
            (xs.0 + 1, xs.1 - 1),
            (ys.1 - 1, ys.1),
            (zs.0 + 1, zs.1 - 1),
        );
        let mx = (xs.0 + xs.1) / 2;
        fill_f(
            b,
            (mx, mx + 1),
            (ys.1 - 1, ys.1),
            (zs.0 + 1, zs.1 - 1),
            p,
            FINISH_STEEL,
        );
    }
}

/// **Фара**: стекло в стальной оправе на носовой грани, смотрит по `+X`.
/// `at` — нижний ближний угол оправы, `w` — ширина по `Z`.
pub(crate) fn headlight(b: &mut UnitBody, at: (usize, usize, usize), w: usize, p: Part) {
    fill_f(
        b,
        (at.0, at.0 + 2),
        (at.1, at.1 + 3),
        (at.2, at.2 + w),
        p,
        FINISH_STEEL,
    );
    fill_f(
        b,
        (at.0 + 1, at.0 + 2),
        (at.1 + 1, at.1 + 2),
        (at.2 + 1, at.2 + w - 1),
        p,
        crate::unit::FINISH_OPTIC,
    );
    // Козырёк над стеклом.
    fill(
        b,
        (at.0, at.0 + 3),
        (at.1 + 3, at.1 + 4),
        (at.2, at.2 + w),
        p,
    );
}

/// **Заклёпки** вдоль оси `axis` от `a`: стальные шляпки в воксель
/// через `step`, сидят **на** поверхности. Ряд, а не одиночки: одна
/// заклёпка на плите читается сором, ряд — швом.
pub(crate) fn rivets(
    b: &mut UnitBody,
    a: (usize, usize, usize),
    axis: usize,
    len: usize,
    step: usize,
    p: Part,
) {
    let mut q = [a.0, a.1, a.2];
    let mut i = 0;
    while i < len {
        q[axis] = a_axis(a, axis) + i;
        b.set(q[0], q[1], q[2], p);
        b.set_finish(q[0], q[1], q[2], FINISH_STEEL);
        i += step.max(1);
    }
}

/// **Выхлопная труба** назад по `−X` от `x1`: труба, кожух-кольцо
/// у корпуса и тёмный срез. Труба — воронёная, кожух — краска.
pub(crate) fn exhaust(b: &mut UnitBody, x: (usize, usize), c: (i32, i32), r: i32, p: Part) {
    cyl_f(b, 0, x, c, r, 0, p, crate::unit::FINISH_GUN);
    cyl_f(
        b,
        0,
        (x.1.saturating_sub(2), x.1),
        c,
        r + 1,
        r,
        p,
        crate::unit::FINISH_PLATE,
    );
    // Срез трубы полый: середина снята, кромка тёмная.
    carve(
        b,
        (x.0, x.0 + 1),
        (c.0 as usize, c.0 as usize + 1),
        (c.1 as usize, c.1 as usize + 1),
    );
    mark_cyl(b, 0, (x.0, x.0 + 1), c, r, FINISH_RUBBER);
}

/// Посадить ядро и убедиться, что оно село.
///
/// Тело без ядра неубиваемо, и молча выпустить такое — ошибка, которую
/// увидит только бой. Поэтому паника здесь, на сборке шаблона: это
/// не пользовательский ввод, а наша собственная разметка.
pub(crate) fn seat(mut b: UnitBody, name: &str) -> UnitBody {
    assert!(b.seat_core(), "{name}: ядро не село");
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::{BODY_MM, TURN};

    fn count(b: &UnitBody) -> u32 {
        b.solid_count()
    }

    /// **Цифровой круг без одиночек на осях.** Ряды круга радиуса
    /// четыре — 4, 4, 4, 3, 2, а не 4, 3, 3, 2, 0: ни одного ряда
    /// шириной в воксель.
    #[test]
    fn a_digital_disc_has_no_lone_voxels_on_its_axes() {
        let mut b = UnitBody::new_scaled(16, 1, 16, 1, BODY_MM);
        disc(&mut b, (8, 8), 4, (0, 1), Part::Armour);
        let width = |z: usize| (0..16).filter(|&x| b.vol.get(x, 0, z)).count();
        assert_eq!(width(8), 9, "ряд через центр");
        assert_eq!(width(4), 5, "макушка круга не столбик, а ряд из пяти");
        assert_eq!(width(3), 0, "круг вылез за радиус");
        // Чётный диаметр вокруг узла: симметричен относительно узла.
        // Это форма звена сегмента — на ней стоит шарнир, и несимметричное
        // сечение уводило бы ось звена на полвоксела (ADR-231).
        let mut e = UnitBody::new_scaled(16, 1, 16, 1, BODY_MM);
        oct_at(&mut e, 1, (0, 1), (8, 8), 6, Part::Armour, FINISH_BY_PART);
        for z in 0..16 {
            for x in 0..16 {
                assert_eq!(
                    e.vol.get(x, 0, z),
                    e.vol.get(15 - x, 0, 15 - z),
                    "круг несимметричен"
                );
            }
        }
        assert_eq!(count(&e), 32, "сечение звена радиуса три вокруг узла");
    }

    /// **Кость шестисвязна** на любой косой: флуд связности идёт по
    /// граням, и диагональная цепочка для него — рассыпанные воксели.
    #[test]
    fn a_bone_is_six_connected() {
        for e in [(11, 3, 7), (2, 9, 12), (12, 12, 12), (1, 1, 8)] {
            let mut b = UnitBody::new_scaled(13, 13, 13, 1, BODY_MM);
            bone(&mut b, (1, 1, 1), e, 0, Part::Armour);
            let n = count(&b);
            assert_eq!(
                n as i32,
                (e.0 - 1) + (e.1 - 1) + (e.2 - 1) + 1,
                "цепочка до {e:?} длиннее или короче шестисвязной"
            );
            // Каждый воксель, кроме первого, соседствует по грани
            // с предыдущим: иначе счётчик сошёлся бы случайно.
            let mut cells: Vec<(usize, usize, usize)> = Vec::new();
            for z in 0..13 {
                for y in 0..13 {
                    for x in 0..13 {
                        if b.vol.get(x, y, z) {
                            cells.push((x, y, z));
                        }
                    }
                }
            }
            for &(x, y, z) in &cells {
                if (x, y, z) == (1, 1, 1) {
                    continue;
                }
                let has = cells.iter().any(|&(a, c, d)| {
                    (a as i32 - x as i32).abs()
                        + (c as i32 - y as i32).abs()
                        + (d as i32 - z as i32).abs()
                        == 1
                });
                assert!(has, "воксель {x} {y} {z} без соседа по грани");
            }
        }
    }

    /// **Тень листа — ступени под его наружной плоскостью.** Лист
    /// на сорок пять градусов от кромки вниз-вперёд, коробка
    /// продлена внутрь: каждый воксель тени лежит под плоскостью
    /// `x + y ≤ 62` (та, что у танка до сегментов считалась руками),
    /// а воксели над ней тень не трогает.
    #[test]
    fn the_shadow_stays_under_the_plate() {
        let mut b = UnitBody::new_scaled(64, 24, 8, 1, BODY_MM);
        let s = slab(6, 3, 8, BODY_MM, Part::Armour, FINISH_BY_PART);
        mount(
            &mut b,
            Attach {
                pitch: -TURN / 8,
                ..Attach::flat((46, 18, 4), (0, 3, 4))
            },
            s,
            12,
            0,
            Part::Armour,
        );
        assert!(count(&b) > 0, "тень пуста");
        for z in 0..8 {
            for y in 0..24 {
                for x in 0..64 {
                    if b.vol.get(x, y, z) {
                        // Наружная плоскость листа проходит через
                        // узел (46, 18) под сорок пять градусов:
                        // `x + y = 64` в узлах решётки. Дальний
                        // верхний угол воксела обязан быть под ней.
                        assert!(x + y + 2 <= 64, "воксель {x} {y} выглянул из-под листа");
                        assert!(y < 18, "тень выше кромки: {x} {y}");
                    }
                }
            }
        }
        // Воксели, лежащие ровно на плоскости, приняты, а не
        // потеряны на допуске.
        assert!(b.vol.get(46, 16, 3), "ступень у кромки потеряна");
        assert!(b.vol.get(48, 14, 3), "ступень на плоскости потеряна");
        assert!(!b.vol.get(49, 14, 3), "ступень выглянула из-под листа");
    }

    /// **Звено накрывает свою кость.** Кость — в корне, звено —
    /// сегмент; каждый воксель кости обязан лежать внутри повёрнутого
    /// звена, иначе он выглядывает наружу заусенцем.
    #[test]
    fn the_bone_hides_inside_its_rod() {
        for (a, e) in [
            ((6, 25, 8), (20, 6, 21)),
            ((4, 4, 4), (26, 5, 25)),
            ((15, 28, 15), (15, 3, 16)),
        ] {
            let mut b = UnitBody::new_scaled(30, 30, 30, 1, BODY_MM);
            let (r2a, r2e) = (8, 6);
            rod(
                &mut b,
                a,
                e,
                r2a,
                r2e,
                1,
                Part::Armour,
                FINISH_BY_PART,
                false,
            );
            let k = b.segments.len() - 1;
            let (t, r) = frame(&b, k);
            let rt = transposed(&r);
            let len = b.segments[k].body.vol.nx as i32;
            let piv = Fx::from_int(link_pivot(r2a, r2e) as i32);
            let half = Fx::from_ratio(1, 2);
            let mut bones = 0;
            for z in 0..30 {
                for y in 0..30 {
                    for x in 0..30 {
                        if !b.vol.get(x, y, z) {
                            continue;
                        }
                        bones += 1;
                        // Середина воксела кости — в координатах звена.
                        let c = [
                            Fx::from_int(x as i32).add(half).sub(t[0]),
                            Fx::from_int(y as i32).add(half).sub(t[1]),
                            Fx::from_int(z as i32).add(half).sub(t[2]),
                        ];
                        let q = mat_vec(&rt, c);
                        let along = q[0].to_int_trunc();
                        assert!(
                            q[0] >= Fx::ZERO && along < len,
                            "кость {x} {y} {z} за торцом звена"
                        );
                        let r2 = r2a + (r2e - r2a) * along / (len - 1).max(1);
                        // Радиус звена плюс полвоксела цифрового круга.
                        let lim = Fx::from_ratio(r2, 2).add(half);
                        let (dy, dz) = (q[1].sub(piv), q[2].sub(piv));
                        let d2 = dy.mul(dy).add(dz.mul(dz));
                        assert!(
                            d2 <= lim.mul(lim),
                            "кость {x} {y} {z} торчит из звена {a:?}→{e:?}"
                        );
                    }
                }
            }
            assert!(bones > 10, "кости нет");
        }
    }
}
