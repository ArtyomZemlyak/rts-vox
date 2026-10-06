//! Набор тел: от лёгкого танка до экспериментальной машины.
//!
//! **Зачем.** Пока в проекте было одно тело — танк шесть метров, — весь
//! масштаб держался на одном числе, и проверить его было не с чем.
//! Крупная машина ломает сразу три вещи: ширину колонки тома, плотность
//! деталей и пересчёт вокселей в мировые единицы. Набор нужен, чтобы
//! каждая из трёх ломалась в тесте, а не в игре.
//!
//! **Размеры выбраны нами**, а не списаны из чужой игры: воспроизводится
//! разброс — обычная машина в единицы метров, экспериментальная
//! в десятки, — а не чей-то конкретный чертёж. Разброс и есть то, что
//! нагружает движок.
//!
//! **Воксель задаётся классом** (ADR-219). Обычная боевая машина —
//! 0.1 м: только в такую сетку ложатся каток, сустав, люк и решётка,
//! а без них машина остаётся коробкой, сколько её ни дроби. Колонка
//! тела держит не больше 255 вокселей (ADR-179), то есть при 0.1 м
//! машина не длиннее 25.5 м, — поэтому экспериментальный класс
//! и постройки остаются на 0.25 м. Это не «гиганты из кубиков
//! покрупнее», от которых уходил ADR-179: там воксель подгоняли
//! под 64-битную колонку, здесь предел назван и померен.
//!
//! **Набор сгруппирован: сторона, сторона, база.** [`tracked`] низкая
//! и на сплошной опоре, [`walking`] высокая и на точечной — стороны
//! разведены силуэтом, а не оттенком (ADR-203). [`buildings`] — то же
//! `UnitBody`, только неподвижное, и силуэт постройки обязан читаться
//! **сверху**, с высоты боевой камеры (ADR-202). [`all`] — склейка
//! трёх списков: тело мимо одного из них пройдёт мимо всех проверок.

use crate::shape::{
    carve, crate_box, cyl_f, disc, exhaust, fill, fill_f, hatch, headlight, joint, lamp, leg, lid,
    link, link_pivot, mark, mark_cyl, mount, pack, plate_ring, plated_turret, rail, ram, ring,
    rivets, rod, running_gear, seat, skirt, slab, span,
};
use crate::topo::unit::UnitVol;
use crate::unit::{
    Attach, Part, UnitBody, BODY_MM, FINISH_GUN, FINISH_OPTIC, FINISH_PLATE, FINISH_RUBBER,
    FINISH_STEEL, TURN,
};

/// Ребро воксела обычной боевой машины — десять сантиметров (ADR-219).
const FINE: u16 = BODY_MM;

/// Ребро воксела экспериментальной машины и постройки — канонические
/// 0.25 м (ADR-094).
///
/// Не выбор вкуса: при 100 мм колонка тела держит 25.5 м, а паук
/// и крепость длиннее. Постройки остаются здесь же по цене — завод
/// при 100 мм стоит 1.24 мс связности на попадание (ADR-208).
const COARSE: u16 = 250;

/// Стопа шагающей машины: подошва под голеностопом, пятка назад,
/// два пальца вперёд с просветом между ними. Плоская плита не
/// говорит, куда машина идёт; пальцы говорят. `c` — узел голеностопа
/// по `x`/`z`, `half` — полуширина подошвы, `len` — вынос пальца.
fn foot_volume(b: &mut UnitBody, c: (i32, i32), half: i32, len: i32, h: usize, p: Part) {
    let s = |v: i32| v.max(0) as usize;
    fill(
        b,
        (s(c.0 - half), s(c.0 + half)),
        (0, h),
        (s(c.1 - half), s(c.1 + half)),
        p,
    );
    fill(
        b,
        (s(c.0 - half - len / 2), s(c.0 - half)),
        (0, h - 1),
        (s(c.1 - half / 2), s(c.1 + half / 2)),
        p,
    );
    let t = (half / 2).max(1);
    fill(
        b,
        (s(c.0 + half), s(c.0 + half + len)),
        (0, h - 1),
        (s(c.1 - half), s(c.1 - half + t)),
        p,
    );
    fill(
        b,
        (s(c.0 + half), s(c.0 + half + len)),
        (0, h - 1),
        (s(c.1 + half - t), s(c.1 + half)),
        p,
    );
}

/// Attach the complete sole and toes to the last leg's ankle.
fn foot(b: &mut UnitBody, c: (i32, i32), half: i32, len: i32, h: usize, p: Part) {
    let ankle = b.legs.last().expect("foot without leg").ankle;
    let cx = half + len / 2;
    let mut part = UnitBody::new_scaled(
        (2 * half + len + len / 2) as u8,
        h as u8,
        (2 * half) as u8,
        1,
        b.mm,
    );
    foot_volume(&mut part, (cx, half), half, len, h, p);
    let index = b.segments.len() as u8;
    b.push_segment(
        Attach::flat(ankle, (cx as u8, ankle.1 as u8, half as u8)),
        part,
    );
    b.legs.last_mut().unwrap().foot = Some(index);
    debug_assert_eq!((ankle.0 as i32, ankle.2 as i32), c);
}

/// Ствол под углом: маска в корне, звено-сегмент со скрытой костью
/// и дульный тормоз потомком на его конце. `a` — узел у маски,
/// `e` — дульный срез, радиусы в полувокселях. Тормоз толще ствола
/// на два полувоксела и длиной в четверть своего диаметра.
#[allow(clippy::too_many_arguments)]
fn gun(
    b: &mut UnitBody,
    a: (i32, i32, i32),
    e: (i32, i32, i32),
    r2: i32,
    brake: bool,
    turret: bool,
) {
    let bone = if r2 >= 4 { 0 } else { -1 };
    let k = rod(b, a, e, r2, r2, bone, Part::Weapon, FINISH_GUN, turret);
    if brake {
        let len = span(a, e);
        let r = link_pivot(r2, r2);
        let br = r2 + 2;
        let tube = link(
            (br / 2).max(2) as u8,
            br,
            br,
            b.mm,
            Part::Weapon,
            FINISH_GUN,
        );
        let pivot = link_pivot(br, br);
        b.push_segment(
            Attach {
                parent: k,
                turret,
                ..Attach::flat(
                    (len as i16 - (br / 4).max(1) as i16, r as i16, r as i16),
                    (0, pivot, pivot),
                )
            },
            tube,
        );
    }
}

/// Стойки антенн — стальной прут назад и вверх от узла `a`.
fn antenna(b: &mut UnitBody, a: (i32, i32, i32), len: i32, turret: bool) {
    let s = |v: i32| v.max(0) as usize;
    fill_f(
        b,
        (s(a.0 - 1), s(a.0 + 1)),
        (s(a.1 - 1), s(a.1 + 1)),
        (s(a.2 - 1), s(a.2 + 1)),
        Part::Armour,
        FINISH_STEEL,
    );
    rod(
        b,
        a,
        (a.0 - len / 3, a.1 + len, a.2),
        2,
        2,
        -1,
        Part::Armour,
        FINISH_STEEL,
        turret,
    );
}

// ---------------------------------------------------------------------
// Лёгкий класс.
// ---------------------------------------------------------------------

/// **Штурмовой бот.** 4.0 × 7.0 × 4.0 м, том 40×70×40.
///
/// Двуногий, вдвое выше танка и вчетверо уже: силуэт обязан читаться
/// с той же высоты камеры, что и танк, иначе класс не различить.
/// Ноги обратного излома — бедро и голень сегментами на стальных
/// шарах, гидроцилиндры снаружи излома; грудь и забрало — наклонные
/// листы; орудия — стволы под небольшим склонением с плечевых шаров.
pub fn assault_bot() -> UnitBody {
    let mut b = UnitBody::new_scaled(40, 70, 40, 5, FINE);
    let a = Part::Armour;
    for &hz in &[10i32, 30] {
        let s = if hz < 20 { -1 } else { 1 };
        leg(
            &mut b,
            (20, 33, hz),
            (12, 19, hz + s * 2),
            (21, 6, hz),
            10,
            8,
            a,
        );
        foot(&mut b, (21, hz), 4, 5, 3, a);
        // Щека над тазобедренным шаром: из-под неё нога и выходит.
        skirt(
            &mut b,
            14,
            12,
            36,
            if s < 0 { 12 } else { 28 },
            s,
            TURN / 10,
            7,
            2,
            false,
            a,
            FINISH_PLATE,
        );
    }
    // Таз и торс; грудь — лист, откинутый назад кверху.
    fill(&mut b, (14, 26), (30, 36), (12, 28), a);
    fill(&mut b, (9, 30), (36, 52), (12, 28), a);
    let chest = slab(17, 3, 16, FINE, a, FINISH_PLATE);
    mount(
        &mut b,
        Attach::aimed((30, 52, 20), (0, 3, 8), (4, -16, 0)),
        chest,
        10,
        0,
        a,
    );
    // Плечи шире торса, наплечники — наклонные листы на концах.
    fill(&mut b, (11, 29), (46, 54), (5, 35), a);
    skirt(
        &mut b,
        12,
        16,
        54,
        5,
        -1,
        TURN / 12,
        7,
        2,
        false,
        a,
        FINISH_PLATE,
    );
    skirt(
        &mut b,
        12,
        16,
        54,
        35,
        1,
        TURN / 12,
        7,
        2,
        false,
        a,
        FINISH_PLATE,
    );
    // Руки-орудия: кожух с плечевого шара под небольшим склонением,
    // ствол из кожуха, казённик в корне.
    for &cz in &[8i32, 32] {
        joint(&mut b, (20, 50, cz), 6, a, FINISH_STEEL);
        rod(
            &mut b,
            (21, 50, cz),
            (35, 48, cz),
            10,
            8,
            2,
            a,
            FINISH_PLATE,
            false,
        );
        fill(
            &mut b,
            (20, 30),
            (47, 51),
            ((cz - 2) as usize, (cz + 2) as usize),
            Part::Weapon,
        );
        gun(&mut b, (33, 48, cz), (40, 47, cz), 4, true, false);
    }
    // Двигатель — ранец на спине с двумя соплами назад.
    fill(&mut b, (5, 11), (38, 50), (14, 26), Part::Engine);
    exhaust(&mut b, (1, 6), (42, 16), 2, Part::Engine);
    exhaust(&mut b, (1, 6), (42, 24), 2, Part::Engine);
    fill(&mut b, (16, 24), (40, 48), (16, 24), Part::Core);
    // Шея-шар, голова с забралом-листом и приборы на макушке.
    joint(&mut b, (20, 55, 20), 6, a, FINISH_STEEL);
    fill(&mut b, (14, 26), (56, 66), (14, 26), a);
    fill(&mut b, (24, 26), (58, 64), (15, 25), Part::Sensor);
    let visor = slab(9, 2, 12, FINE, Part::Sensor, FINISH_OPTIC);
    mount(
        &mut b,
        Attach::aimed((26, 66, 20), (0, 2, 6), (3, -8, 0)),
        visor,
        6,
        0,
        Part::Sensor,
    );
    disc(&mut b, (20, 20), 4, (66, 68), Part::Sensor);
    antenna(&mut b, (15, 66, 16), 6, false);
    antenna(&mut b, (15, 66, 24), 5, false);
    seat(b, "штурмовой бот")
}

/// **Осадный шагоход.** 9.0 × 12.0 × 8.0 м, том 90×120×80.
///
/// Четыре опоры и одно тяжёлое орудие: машина прорыва, вдвое выше
/// бота и вчетверо тяжелее танка. Нос свисает над передними ногами
/// наклонным листом, башня — короб в листах, орудие поднято.
pub fn siege_walker() -> UnitBody {
    let mut b = UnitBody::new_scaled(90, 120, 80, 8, FINE);
    let a = Part::Armour;
    for &hx in &[24i32, 66] {
        let ox = if hx < 45 { -9 } else { 9 };
        for &hz in &[22i32, 58] {
            let s = if hz < 40 { -1 } else { 1 };
            let (kx, kz) = (hx + ox, hz + s * 7);
            leg(&mut b, (hx, 58, hz), (kx, 34, kz), (hx, 9, hz), 20, 16, a);
            foot(&mut b, (hx, hz), 9, 8, 8, a);
            // Щека над тазом: лист, из-под которого выходит нога.
            skirt(
                &mut b,
                hx - 12,
                24,
                58,
                if s < 0 { 14 } else { 66 },
                s,
                TURN / 12,
                10,
                3,
                false,
                a,
                FINISH_PLATE,
            );
        }
    }
    // Корпус и нос, свисающий над передними ногами: лист от кромки
    // крыши вниз и назад, короб под ним.
    fill(&mut b, (10, 74), (58, 84), (14, 66), a);
    fill(&mut b, (10, 87), (80, 84), (14, 66), a);
    let prow = slab(29, 3, 52, FINE, a, FINISH_PLATE);
    let prow = mount(
        &mut b,
        Attach::aimed((87, 84, 40), (0, 0, 26), (-13, -26, 0)),
        prow,
        0,
        14,
        a,
    );
    // Фары на носовом листе — потомки листа, смотрят по его нормали.
    for z in [6i16, 44] {
        b.push_segment(
            Attach {
                parent: prow,
                ..Attach::aimed((6, 0, z), (0, 1, 1), (0, -5, 0))
            },
            lamp(FINE, a),
        );
    }
    // Двигатель под кормовой палубой, жалюзи прорезями, выхлоп назад.
    fill(&mut b, (14, 36), (62, 82), (20, 60), Part::Engine);
    let mut z = 20;
    while z + 3 <= 60 {
        carve(&mut b, (16, 34), (83, 84), (z, z + 3));
        z += 6;
    }
    exhaust(&mut b, (2, 14), (72, 24), 4, Part::Engine);
    exhaust(&mut b, (2, 14), (72, 56), 4, Part::Engine);
    fill(&mut b, (42, 52), (64, 74), (35, 45), Part::Core);
    // Поручни по кромкам крыши, трап на корме.
    rail(&mut b, (12, 84, 15), 0, 60, a);
    rail(&mut b, (12, 84, 64), 0, 60, a);
    let mut y = 60;
    while y < 82 {
        fill_f(&mut b, (9, 10), (y, y + 1), (36, 44), a, FINISH_STEEL);
        y += 3;
    }
    // Погон и башня: короб в наклонных листах, маска и поднятый ствол.
    disc(&mut b, (46, 40), 22, (84, 86), a);
    mark_cyl(&mut b, 1, (84, 86), (46, 40), 22, FINISH_STEEL);
    plated_turret(
        &mut b,
        (28, 74),
        (86, 104),
        (24, 56),
        (10, 6),
        TURN / 16,
        19,
        3,
        false,
        a,
        FINISH_PLATE,
    );
    cyl_f(&mut b, 0, (72, 80), (95, 40), 8, 0, a, FINISH_GUN);
    fill(&mut b, (60, 72), (90, 100), (34, 46), Part::Weapon);
    gun(&mut b, (78, 95, 40), (90, 97, 40), 14, true, false);
    hatch(&mut b, (40, 30), 4, 104, a);
    lid(&mut b, (40, 50), 4, 104, a);
    // Мачта наблюдения на корме башни, тарелка на ней и антенны.
    fill(&mut b, (30, 40), (104, 112), (34, 46), a);
    disc(&mut b, (35, 40), 7, (112, 117), Part::Sensor);
    let dish = slab(13, 1, 13, FINE, Part::Sensor, FINISH_OPTIC);
    mount(
        &mut b,
        Attach {
            pitch: TURN / 10,
            ..Attach::flat((35, 117, 40), (6, 0, 6))
        },
        dish,
        -1,
        -1,
        Part::Sensor,
    );
    antenna(&mut b, (30, 104, 26), 10, false);
    antenna(&mut b, (30, 104, 54), 8, false);
    seat(b, "осадный шагоход")
}

/// **Командная машина.** 8.0 × 11.0 × 7.0 м, том 80×110×70.
///
/// Не самая крупная и не самая сильная — самая дорогая. Силуэт
/// человекоподобный нарочно: командира надо узнавать в толпе,
/// и узнают его по несимметричным рукам — орудие с одной стороны,
/// строительный манипулятор с клешнёй с другой.
pub fn commander() -> UnitBody {
    let mut b = UnitBody::new_scaled(80, 110, 70, 7, FINE);
    let a = Part::Armour;
    for &hz in &[24i32, 46] {
        let s = if hz < 35 { -1 } else { 1 };
        leg(
            &mut b,
            (38, 50, hz),
            (28, 30, hz + s * 3),
            (39, 8, hz),
            18,
            14,
            a,
        );
        foot(&mut b, (39, hz), 8, 8, 7, a);
        skirt(
            &mut b,
            28,
            20,
            56,
            if s < 0 { 18 } else { 52 },
            s,
            TURN / 10,
            9,
            3,
            false,
            a,
            FINISH_PLATE,
        );
    }
    // Таз, торс, грудной лист.
    fill(&mut b, (30, 46), (48, 56), (18, 52), a);
    fill(&mut b, (14, 58), (56, 84), (18, 52), a);
    let chest = slab(27, 3, 34, FINE, a, FINISH_PLATE);
    mount(
        &mut b,
        Attach::aimed((58, 84, 35), (0, 3, 17), (6, -26, 0)),
        chest,
        12,
        0,
        a,
    );
    // Плечи с наплечниками, ранец-двигатель с соплами.
    fill(&mut b, (18, 58), (72, 84), (6, 64), a);
    skirt(
        &mut b,
        20,
        36,
        84,
        6,
        -1,
        TURN / 12,
        10,
        3,
        false,
        a,
        FINISH_PLATE,
    );
    skirt(
        &mut b,
        20,
        36,
        84,
        64,
        1,
        TURN / 12,
        10,
        3,
        false,
        a,
        FINISH_PLATE,
    );
    fill(&mut b, (8, 18), (58, 80), (22, 48), Part::Engine);
    exhaust(&mut b, (2, 10), (68, 26), 3, Part::Engine);
    exhaust(&mut b, (2, 10), (68, 44), 3, Part::Engine);
    fill(&mut b, (32, 44), (60, 72), (29, 41), Part::Core);
    // Правая рука — орудие: кожух с плечевого шара, ствол, тормоз.
    joint(&mut b, (38, 74, 13), 9, a, FINISH_STEEL);
    rod(
        &mut b,
        (40, 72, 13),
        (56, 68, 13),
        16,
        12,
        4,
        a,
        FINISH_PLATE,
        false,
    );
    fill(&mut b, (38, 52), (66, 72), (10, 16), Part::Weapon);
    gun(&mut b, (54, 68, 13), (76, 66, 13), 6, true, false);
    // Левая рука — манипулятор: плечо, локоть, предплечье и клешня
    // из двух пальцев врозь. Ни ствола, ни симметрии.
    joint(&mut b, (38, 74, 57), 9, a, FINISH_STEEL);
    rod(
        &mut b,
        (38, 72, 57),
        (48, 54, 58),
        16,
        12,
        4,
        a,
        FINISH_PLATE,
        false,
    );
    joint(&mut b, (48, 54, 58), 8, a, FINISH_STEEL);
    rod(
        &mut b,
        (48, 54, 58),
        (64, 46, 58),
        12,
        10,
        3,
        a,
        FINISH_PLATE,
        false,
    );
    joint(&mut b, (64, 46, 58), 6, a, FINISH_STEEL);
    rod(
        &mut b,
        (64, 46, 58),
        (76, 44, 52),
        6,
        4,
        -1,
        a,
        FINISH_STEEL,
        false,
    );
    rod(
        &mut b,
        (64, 46, 58),
        (76, 44, 64),
        6,
        4,
        -1,
        a,
        FINISH_STEEL,
        false,
    );
    ram(&mut b, (44, 70, 62), (54, 56, 62), 4, a);
    // Голова с забралом, купол приборов, антенны.
    joint(&mut b, (38, 84, 35), 8, a, FINISH_STEEL);
    fill(&mut b, (26, 48), (86, 100), (24, 46), a);
    fill(&mut b, (46, 48), (90, 98), (26, 44), Part::Sensor);
    let visor = slab(13, 2, 20, FINE, Part::Sensor, FINISH_OPTIC);
    mount(
        &mut b,
        Attach::aimed((48, 100, 35), (0, 2, 10), (4, -12, 0)),
        visor,
        8,
        0,
        Part::Sensor,
    );
    disc(&mut b, (36, 35), 6, (100, 104), Part::Sensor);
    antenna(&mut b, (28, 100, 28), 8, false);
    antenna(&mut b, (28, 100, 42), 7, false);
    pack(&mut b, (40, 41), (52, 53), (55, 56));
    seat(b, "командная машина")
}

// ---------------------------------------------------------------------
// Экспериментальный класс. Крупнее — и воксель крупнее.
// ---------------------------------------------------------------------

/// **Экспериментальный паук.** 40 × 22 × 40 м, том 160×88×160.
///
/// Шесть с лишним танков в длину. Восемь ног из трёх звеньев —
/// все звенья сегментами на стальных шарах, кости внутри; купол —
/// венец наклонных листов, а не ступени; лучевое орудие носом вперёд.
pub fn spider() -> UnitBody {
    let mut b = UnitBody::new_scaled(160, 88, 160, 8, COARSE);
    let a = Part::Armour;
    let c = (80usize, 80usize);
    let ci = (80i32, 80i32);

    // Восемь направлений в десятых долях — чтобы диагональ шла
    // короче по осям и ноги ложились кругом, а не квадратом.
    const DIRS: [(i32, i32); 8] = [
        (10, 0),
        (7, 7),
        (0, 10),
        (-7, 7),
        (-10, 0),
        (-7, -7),
        (0, -10),
        (7, -7),
    ];
    for (dx, dz) in DIRS {
        let at = |r: i32, y: i32| (ci.0 + dx * r / 10, y, ci.1 + dz * r / 10);
        let (hip, knee, ankle, toe) = (at(30, 44), at(52, 58), at(74, 66), at(70, 6));
        joint(&mut b, hip, 8, a, FINISH_STEEL);
        rod(&mut b, hip, knee, 14, 12, 4, a, FINISH_PLATE, false);
        joint(&mut b, knee, 7, a, FINISH_STEEL);
        rod(&mut b, knee, ankle, 12, 10, 3, a, FINISH_PLATE, false);
        joint(&mut b, ankle, 6, a, FINISH_STEEL);
        rod(&mut b, ankle, toe, 10, 6, 1, a, FINISH_PLATE, false);
        // Гидроцилиндр поверх бедра — от таза к колену.
        let up = |p: (i32, i32, i32), d: i32| (p.0, p.1 + d, p.2);
        ram(&mut b, up(hip, 6), up(knee, 4), 4, a);
        let f = at(70, 2);
        fill(
            &mut b,
            ((f.0 - 6).max(0) as usize, (f.0 + 6).max(0) as usize),
            (0, 6),
            ((f.2 - 6).max(0) as usize, (f.2 + 6).max(0) as usize),
            a,
        );
        fill(
            &mut b,
            ((f.0 - 4).max(0) as usize, (f.0 + 4).max(0) as usize),
            (6, 9),
            ((f.2 - 4).max(0) as usize, (f.2 + 4).max(0) as usize),
            a,
        );
    }

    // Корпус: два цилиндрических яруса с поясами брони, над ними —
    // венец наклонных листов до верхней палубы и второй, круче,
    // до купола. Пояс шире своего яруса — без поясов купол читается
    // гладким конусом, а не бронированной машиной.
    disc(&mut b, c, 40, (16, 26), a);
    ring(&mut b, c, 43, 38, (16, 19), a);
    disc(&mut b, c, 48, (26, 42), a);
    ring(&mut b, c, 51, 46, (26, 29), a);
    disc(&mut b, c, 30, (42, 64), a);
    plate_ring(&mut b, ci, 64, 32, 42, 50, 8, 3, 14, false, a, FINISH_PLATE);
    disc(&mut b, c, 18, (64, 76), a);
    plate_ring(&mut b, ci, 76, 18, 64, 32, 8, 2, 10, false, a, FINISH_PLATE);
    // Двигатель — внутри нижнего яруса, выхлопные стаканы на корме.
    disc(&mut b, c, 22, (20, 36), Part::Engine);
    for &z in &[70usize, 90] {
        cyl_f(
            &mut b,
            1,
            (42, 50),
            (36, z as i32),
            3,
            0,
            Part::Engine,
            FINISH_GUN,
        );
        carve(&mut b, (36, 37), (49, 50), (z, z + 1));
    }
    // Люки на верхней палубе.
    hatch(&mut b, (92, 68), 3, 76, a);
    hatch(&mut b, (92, 92), 3, 76, a);

    // Сенсорная надстройка на макушке: купол, четыре пилона, антенны.
    disc(&mut b, c, 12, (76, 84), Part::Sensor);
    for (dx, dz) in [(1i32, 1i32), (1, -1), (-1, 1), (-1, -1)] {
        let (x, z) = (ci.0 + dx * 14, ci.1 + dz * 14);
        fill(
            &mut b,
            ((x - 2) as usize, (x + 2) as usize),
            (74, 84),
            ((z - 2) as usize, (z + 2) as usize),
            Part::Sensor,
        );
        antenna(&mut b, (x, 84, z), 4, false);
    }

    // Лучевое орудие носом вперёд: кожух, ствол чуть склонён,
    // дульное кольцо потомком, казённик в корне.
    fill(&mut b, (100, 124), (42, 62), (66, 94), a);
    fill(&mut b, (104, 122), (46, 58), (70, 90), Part::Weapon);
    gun(&mut b, (120, 52, 80), (152, 50, 80), 18, true, false);

    fill(&mut b, (76, 84), (40, 48), (76, 84), Part::Core);
    seat(b, "экспериментальный паук")
}

/// **Мобильная крепость.** 56 × 16 × 26 м, том 224×64×104.
///
/// Девять танков в длину. Три башни на надстройке: у машины этого
/// класса одно орудие смотрелось бы как у танка, а разница классов
/// обязана читаться силуэтом. Нос и корма — наклонные листы, борта
/// надстройки — скосы, стволы подняты.
pub fn fortress() -> UnitBody {
    let mut b = UnitBody::new_scaled(224, 64, 104, 8, COARSE);
    let a = Part::Armour;

    // Ходовая: замкнутые ленты четыре метра высотой.
    running_gear(&mut b, (10, 214), 16, (0, 22), a);
    running_gear(&mut b, (10, 214), 16, (82, 104), a);

    // Корпус между лентами, палуба во всю ширину, нос и корма — листы.
    fill(&mut b, (8, 216), (2, 18), (22, 82), a);
    fill(&mut b, (8, 216), (17, 36), (14, 90), a);
    let bow = slab(22, 3, 76, COARSE, a, FINISH_PLATE);
    mount(
        &mut b,
        Attach::aimed((216, 36, 52), (0, 3, 38), (8, -20, 0)),
        bow,
        10,
        0,
        a,
    );
    let stern = slab(22, 3, 68, COARSE, a, FINISH_PLATE);
    mount(
        &mut b,
        Attach::aimed((8, 36, 52), (0, 3, 34), (-8, -20, 0)),
        stern,
        10,
        0,
        a,
    );
    // Боковые спонсоны и ящики на палубе.
    fill(&mut b, (76, 148), (22, 34), (6, 16), a);
    fill(&mut b, (76, 148), (22, 34), (88, 98), a);
    for &x in &[30usize, 194] {
        crate_box(&mut b, (x, x + 10), (36, 41), (16, 22), a);
        crate_box(&mut b, (x, x + 10), (36, 41), (82, 88), a);
    }

    // Надстройка со скошенными бортами, моторный отсек с жалюзи
    // и выхлопными трубами вверх.
    fill(&mut b, (40, 188), (36, 48), (26, 78), a);
    skirt(
        &mut b,
        40,
        148,
        48,
        78,
        1,
        TURN / 14,
        13,
        2,
        false,
        a,
        FINISH_PLATE,
    );
    skirt(
        &mut b,
        40,
        148,
        48,
        26,
        -1,
        TURN / 14,
        13,
        2,
        false,
        a,
        FINISH_PLATE,
    );
    fill(&mut b, (150, 186), (36, 47), (30, 74), Part::Engine);
    let mut z = 32;
    while z < 72 {
        carve(&mut b, (152, 184), (47, 48), (z, z + 3));
        z += 6;
    }
    for &z in &[34i32, 70] {
        cyl_f(
            &mut b,
            1,
            (48, 58),
            (170, z),
            2,
            0,
            Part::Engine,
            FINISH_GUN,
        );
        carve(&mut b, (170, 171), (57, 58), (z as usize, z as usize + 1));
    }
    // Поручни по кромкам надстройки и палубы.
    rail(&mut b, (44, 48, 28), 0, 100, a);
    rail(&mut b, (44, 48, 75), 0, 100, a);
    rail(&mut b, (12, 36, 15), 0, 26, a);
    rail(&mut b, (12, 36, 88), 0, 26, a);

    // Три башни: погон, короб в листах, маска, поднятый ствол.
    for x0 in [58i32, 106, 154] {
        let cx = x0 + 14;
        disc(&mut b, (cx as usize, 52), 16, (48, 50), a);
        mark_cyl(&mut b, 1, (48, 50), (cx, 52), 16, FINISH_STEEL);
        plated_turret(
            &mut b,
            ((cx - 12) as usize, (cx + 12) as usize),
            (50, 62),
            (40, 64),
            (8, 5),
            TURN / 16,
            13,
            2,
            false,
            a,
            FINISH_PLATE,
        );
        cyl_f(
            &mut b,
            0,
            ((cx + 10) as usize, (cx + 16) as usize),
            (56, 52),
            5,
            0,
            a,
            FINISH_GUN,
        );
        fill(
            &mut b,
            ((cx + 2) as usize, (cx + 12) as usize),
            (52, 60),
            (47, 57),
            Part::Weapon,
        );
        gun(
            &mut b,
            (cx + 14, 56, 52),
            (cx + 42, 59, 52),
            10,
            true,
            false,
        );
        hatch(&mut b, ((cx - 4) as usize, 46), 2, 62, a);
    }

    // Мачта на носу: ферма из четырёх стоек, площадка, тарелка.
    fill(&mut b, (16, 28), (36, 40), (44, 60), a);
    // Стойки — с костью: без неё площадка с тарелкой держится
    // на одних сегментах, а связность считает корневой том, и сенсор
    // выходит отрезанным от ядра (ADR-231).
    for (x, z) in [(17i32, 45i32), (17, 59), (27, 45), (27, 59)] {
        rod(
            &mut b,
            (x, 40, z),
            (22, 54, 52),
            3,
            3,
            0,
            a,
            FINISH_STEEL,
            false,
        );
    }
    fill(&mut b, (18, 26), (54, 57), (48, 56), a);
    disc(&mut b, (22, 52), 5, (57, 60), Part::Sensor);
    let dish = slab(11, 1, 11, COARSE, Part::Sensor, FINISH_OPTIC);
    mount(
        &mut b,
        Attach {
            pitch: TURN / 8,
            ..Attach::flat((22, 60, 52), (5, 0, 5))
        },
        dish,
        -1,
        -1,
        Part::Sensor,
    );
    antenna(&mut b, (44, 48, 30), 12, false);
    antenna(&mut b, (44, 48, 74), 12, false);

    fill(&mut b, (104, 112), (20, 28), (48, 56), Part::Core);
    // Fill closed roof pockets under the rear turret; external vents remain.
    pack(&mut b, (152, 173), (47, 48), (50, 53));
    pack(&mut b, (175, 184), (47, 48), (50, 53));
    pack(&mut b, (175, 184), (47, 48), (56, 59));
    seat(b, "мобильная крепость")
}

// ---------------------------------------------------------------------
// Гусеничная сторона. Низкий силуэт, сплошная опора, лента с траками.
// ---------------------------------------------------------------------

/// **Гусеничный разведчик.** 5.0 × 2.0 × 3.0 м, том 50×20×30.
///
/// Самая мелкая машина набора и единственная ниже танка. Пушка стоит
/// на открытой тумбе: башни на такой машине нет, и это видно. Лоб —
/// крутой лист между лентами, ствол поднят.
pub fn scout_tracked() -> UnitBody {
    let mut b = UnitBody::new_scaled(50, 20, 30, 3, FINE);
    let a = Part::Armour;
    running_gear(&mut b, (2, 48), 10, (1, 7), a);
    running_gear(&mut b, (2, 48), 10, (23, 29), a);
    // Корпус между лентами, полки над ними, лоб — лист.
    fill(&mut b, (4, 44), (2, 14), (7, 23), a);
    fill(&mut b, (2, 48), (11, 13), (0, 30), a);
    let glacis = slab(13, 2, 16, FINE, a, FINISH_PLATE);
    mount(
        &mut b,
        Attach::aimed((44, 14, 15), (0, 2, 8), (5, -12, 0)),
        glacis,
        8,
        0,
        a,
    );
    // Ящики и фары на полках, выхлоп назад, антенна на корме.
    crate_box(&mut b, (6, 14), (13, 16), (1, 6), a);
    crate_box(&mut b, (30, 38), (13, 16), (24, 29), a);
    headlight(&mut b, (44, 13, 1), 4, a);
    headlight(&mut b, (44, 13, 25), 4, a);
    fill(&mut b, (5, 16), (4, 12), (9, 21), Part::Engine);
    let mut z = 9;
    while z + 2 <= 21 {
        carve(&mut b, (6, 15), (13, 14), (z, z + 2));
        z += 4;
    }
    exhaust(&mut b, (0, 4), (11, 4), 1, Part::Engine);
    antenna(&mut b, (5, 14, 25), 5, false);
    fill(&mut b, (20, 26), (4, 10), (12, 18), Part::Core);
    // Открытая тумба: стальной погон, качающаяся часть, поднятый
    // ствол, прибор наблюдения сбоку.
    ring(&mut b, (24, 15), 7, 5, (14, 15), a);
    mark_cyl(&mut b, 1, (14, 15), (24, 15), 7, FINISH_STEEL);
    disc(&mut b, (24, 15), 4, (14, 17), a);
    fill(&mut b, (22, 28), (16, 19), (12, 18), Part::Weapon);
    gun(&mut b, (28, 18, 15), (44, 20, 15), 3, false, true);
    fill_f(
        &mut b,
        (18, 22),
        (16, 19),
        (11, 15),
        Part::Sensor,
        FINISH_OPTIC,
    );
    let mut b = seat(b, "гусеничный разведчик");
    b.turret_y = 14;
    b
}

/// **Гусеничная САУ.** 10.0 × 4.0 × 4.0 м, том 100×40×40.
///
/// Осадная машина гусеничной стороны: рубка вместо башни, ствол
/// поднят и длиннее корпуса на треть, сошники сзади. Ни один из трёх
/// признаков не встречается у шагающей стороны — по ним класс и узнают.
pub fn spg_tracked() -> UnitBody {
    let mut b = UnitBody::new_scaled(100, 40, 40, 5, FINE);
    let a = Part::Armour;
    running_gear(&mut b, (4, 96), 14, (2, 10), a);
    running_gear(&mut b, (4, 96), 14, (30, 38), a);
    // Корпус между лентами, полки, верхний корпус, лобовые листы.
    fill(&mut b, (7, 92), (2, 15), (10, 30), a);
    fill(&mut b, (4, 96), (15, 17), (0, 40), a);
    fill(&mut b, (7, 80), (17, 22), (3, 37), a);
    let glacis = slab(8, 3, 34, FINE, a, FINISH_PLATE);
    mount(
        &mut b,
        Attach::aimed((80, 22, 20), (0, 3, 17), (6, -5, 0)),
        glacis,
        10,
        0,
        a,
    );
    let nose = slab(14, 2, 20, FINE, a, FINISH_PLATE);
    mount(
        &mut b,
        Attach::aimed((92, 2, 20), (0, 0, 10), (4, 13, 0)),
        nose,
        0,
        10,
        a,
    );
    // Сошники: два листа с кормы, поднятые на сорок пять градусов,
    // на стальных петлях. Без них машина этого класса — длинный танк.
    for &z in &[10i16, 24] {
        fill_f(
            &mut b,
            (5, 8),
            (4, 8),
            (z as usize, z as usize + 6),
            a,
            FINISH_STEEL,
        );
        let spade = slab(9, 2, 6, FINE, a, FINISH_PLATE);
        mount(
            &mut b,
            Attach::aimed((5, 6, z + 3), (0, 0, 3), (-6, 6, 0)),
            spade,
            -1,
            -1,
            a,
        );
    }
    fill(&mut b, (9, 24), (8, 20), (12, 28), Part::Engine);
    let mut z = 8;
    while z + 3 <= 32 {
        carve(&mut b, (10, 24), (21, 22), (z, z + 3));
        z += 6;
    }
    exhaust(&mut b, (2, 8), (19, 6), 2, Part::Engine);
    exhaust(&mut b, (2, 8), (19, 33), 2, Part::Engine);
    fill(&mut b, (40, 48), (9, 17), (16, 24), Part::Core);
    // Рубка в наклонных листах: скулы и борта, лоб под маску.
    plated_turret(
        &mut b,
        (26, 72),
        (22, 34),
        (8, 32),
        (6, 5),
        TURN / 18,
        13,
        2,
        false,
        a,
        FINISH_PLATE,
    );
    cyl_f(&mut b, 0, (70, 78), (28, 20), 5, 0, a, FINISH_GUN);
    fill(&mut b, (56, 70), (24, 32), (15, 25), Part::Weapon);
    gun(&mut b, (78, 28, 20), (100, 32, 20), 10, true, false);
    // Крыша рубки: люки, прибор командира, антенны; ящики на полках,
    // фары, поручни по бортам рубки.
    hatch(&mut b, (40, 14), 3, 34, a);
    hatch(&mut b, (52, 26), 3, 34, a);
    fill_f(
        &mut b,
        (34, 40),
        (34, 37),
        (22, 28),
        Part::Sensor,
        FINISH_OPTIC,
    );
    antenna(&mut b, (30, 34, 12), 6, false);
    antenna(&mut b, (30, 34, 28), 5, false);
    crate_box(&mut b, (30, 44), (17, 21), (0, 3), a);
    crate_box(&mut b, (48, 60), (17, 21), (37, 40), a);
    cyl_f(&mut b, 0, (30, 46), (18, 38), 1, 0, a, FINISH_STEEL);
    headlight(&mut b, (82, 17, 0), 3, a);
    headlight(&mut b, (82, 17, 37), 3, a);
    // Поручни — по краю палубы, а не рядом с ней: наz = 2 и 37
    // палубы нет, и стойки висели бы в воздухе полусотней вокселей,
    // отрезанных от ядра.
    rail(&mut b, (30, 22, 3), 0, 20, a);
    rail(&mut b, (30, 22, 36), 0, 20, a);
    seat(b, "гусеничная САУ")
}

/// **Гусеничная командная машина.** 9.5 × 4.5 × 5.0 м, том 95×45×50.
///
/// Командира надо узнавать в толпе, и у гусеничной стороны узнают его
/// по антенной решётке: круг на прямоугольнике — единственная такая
/// пара силуэтов в наборе, и видна она сверху.
pub fn commander_tracked() -> UnitBody {
    let mut b = UnitBody::new_scaled(95, 45, 50, 5, FINE);
    let a = Part::Armour;
    running_gear(&mut b, (4, 91), 14, (3, 12), a);
    running_gear(&mut b, (4, 91), 14, (38, 47), a);
    fill(&mut b, (6, 86), (2, 15), (12, 38), a);
    fill(&mut b, (4, 91), (15, 17), (0, 50), a);
    fill(&mut b, (6, 76), (17, 22), (3, 47), a);
    let glacis = slab(8, 3, 44, FINE, a, FINISH_PLATE);
    mount(
        &mut b,
        Attach::aimed((76, 22, 25), (0, 3, 22), (6, -5, 0)),
        glacis,
        10,
        0,
        a,
    );
    let nose = slab(14, 2, 26, FINE, a, FINISH_PLATE);
    mount(
        &mut b,
        Attach::aimed((86, 2, 25), (0, 0, 13), (4, 13, 0)),
        nose,
        0,
        10,
        a,
    );
    fill(&mut b, (8, 24), (8, 20), (15, 35), Part::Engine);
    let mut z = 12;
    while z + 3 <= 38 {
        carve(&mut b, (9, 23), (21, 22), (z, z + 3));
        z += 6;
    }
    exhaust(&mut b, (2, 8), (19, 7), 2, Part::Engine);
    fill(&mut b, (40, 48), (9, 17), (21, 29), Part::Core);
    // Рубка штаба в наклонных листах, бронедверь в корме с поручнем.
    plated_turret(
        &mut b,
        (20, 76),
        (22, 34),
        (8, 42),
        (6, 5),
        TURN / 20,
        13,
        2,
        false,
        a,
        FINISH_PLATE,
    );
    fill(&mut b, (16, 20), (22, 32), (16, 34), a);
    // Утопленная дверь: выемка начинается **за** полосой, на которой
    // держится поручень, иначе он остаётся висеть отдельно от корпуса.
    carve(&mut b, (16, 17), (24, 31), (22, 30));
    fill_f(&mut b, (15, 16), (26, 29), (21, 22), a, FINISH_STEEL);
    // Мачта и решётка радара: чаша кольцами, а не сплошным диском —
    // сплошной читается как крышка, а не как антенна.
    fill(&mut b, (34, 42), (34, 38), (21, 29), a);
    for k in 0..5i32 {
        ring(
            &mut b,
            (38, 25),
            6 + 2 * k,
            4 + 2 * k,
            ((38 + k) as usize, (39 + k) as usize),
            Part::Sensor,
        );
    }
    // Дистанционная точка на носу рубки, ствол поднят.
    disc(&mut b, (64, 25), 6, (34, 37), a);
    mark_cyl(&mut b, 1, (34, 35), (64, 25), 6, FINISH_STEEL);
    fill(&mut b, (60, 70), (36, 41), (21, 29), Part::Weapon);
    gun(&mut b, (70, 39, 25), (84, 41, 25), 3, false, false);
    // Люки, штыревые антенны по углам рубки, ящики, фары, поручни.
    hatch(&mut b, (28, 14), 3, 34, a);
    hatch(&mut b, (28, 36), 3, 34, a);
    antenna(&mut b, (22, 34, 10), 9, false);
    antenna(&mut b, (22, 34, 40), 9, false);
    antenna(&mut b, (72, 34, 10), 6, false);
    crate_box(&mut b, (30, 44), (17, 21), (0, 3), a);
    crate_box(&mut b, (50, 62), (17, 21), (47, 50), a);
    headlight(&mut b, (78, 17, 0), 3, a);
    headlight(&mut b, (78, 17, 47), 3, a);
    // Поручни по краю палубы: она кончается на 46-м вокселе,
    // и стойка за ней висела бы в воздухе.
    rail(&mut b, (24, 22, 3), 0, 24, a);
    rail(&mut b, (24, 22, 46), 0, 24, a);
    seat(b, "гусеничная командная машина")
}

// ---------------------------------------------------------------------
// Шагающая сторона. Высокий силуэт, точечная опора, корпус над землёй.
// ---------------------------------------------------------------------

/// **Шагающий разведчик.** 3.0 × 6.0 × 3.0 м, том 30×60×30.
///
/// Вдвое выше своей длины — предельный случай шагающего силуэта
/// и самая узкая машина набора. Нога обратного излома: колено
/// выносится назад, ступня возвращается под корпус. Корпус висит
/// между опорами в наклонных листах, на мачте — тарелка.
pub fn scout_walking() -> UnitBody {
    let mut b = UnitBody::new_scaled(30, 60, 30, 4, FINE);
    let a = Part::Armour;
    for &hz in &[9i32, 21] {
        leg(&mut b, (15, 30, hz), (8, 18, hz), (16, 5, hz), 8, 6, a);
        foot(&mut b, (16, hz), 3, 4, 3, a);
    }
    // Таз с юбками над шарами, корпус, грудной и кормовой листы.
    fill(&mut b, (9, 21), (28, 33), (7, 23), a);
    skirt(
        &mut b,
        9,
        12,
        33,
        7,
        -1,
        TURN / 12,
        6,
        2,
        false,
        a,
        FINISH_PLATE,
    );
    skirt(
        &mut b,
        9,
        12,
        33,
        23,
        1,
        TURN / 12,
        6,
        2,
        false,
        a,
        FINISH_PLATE,
    );
    fill(&mut b, (7, 23), (30, 42), (8, 22), a);
    let chest = slab(13, 2, 14, FINE, a, FINISH_PLATE);
    mount(
        &mut b,
        Attach::aimed((23, 42, 15), (0, 2, 7), (3, -12, 0)),
        chest,
        8,
        0,
        a,
    );
    let back = slab(13, 2, 14, FINE, a, FINISH_PLATE);
    mount(
        &mut b,
        Attach::aimed((7, 42, 15), (0, 2, 7), (-3, -12, 0)),
        back,
        8,
        0,
        a,
    );
    fill(&mut b, (8, 14), (31, 39), (10, 20), Part::Engine);
    cyl_f(
        &mut b,
        1,
        (42, 46),
        (10, 15),
        1,
        0,
        Part::Engine,
        FINISH_GUN,
    );
    carve(&mut b, (10, 11), (45, 46), (15, 16));
    fill(&mut b, (12, 18), (31, 37), (12, 18), Part::Core);
    // Шея-шар, голова с забралом и орудием, мачта с тарелкой.
    joint(&mut b, (15, 42, 15), 4, a, FINISH_STEEL);
    fill(&mut b, (10, 20), (44, 52), (10, 20), a);
    fill(&mut b, (18, 20), (46, 50), (11, 19), Part::Sensor);
    let visor = slab(7, 2, 10, FINE, Part::Sensor, FINISH_OPTIC);
    mount(
        &mut b,
        Attach::aimed((20, 52, 15), (0, 2, 5), (2, -6, 0)),
        visor,
        5,
        0,
        Part::Sensor,
    );
    fill(&mut b, (17, 22), (45, 49), (13, 17), Part::Weapon);
    gun(&mut b, (22, 47, 15), (30, 46, 15), 3, false, true);
    fill_f(&mut b, (14, 16), (52, 56), (14, 16), a, FINISH_STEEL);
    disc(&mut b, (15, 15), 3, (56, 58), Part::Sensor);
    let dish = slab(7, 1, 7, FINE, Part::Sensor, FINISH_OPTIC);
    mount(
        &mut b,
        Attach {
            pitch: TURN / 12,
            turret: true,
            ..Attach::flat((15, 58, 15), (3, 0, 3))
        },
        dish,
        -1,
        -1,
        Part::Sensor,
    );
    let mut b = seat(b, "шагающий разведчик");
    b.turret_y = 42;
    b
}

// ---------------------------------------------------------------------
// Постройки. То же `UnitBody`, только неподвижное: ядро, броня, три
// подсистемы. Отдельного типа у них нет — см. ADR-202.
// ---------------------------------------------------------------------

/// **Завод.** 24.0 × 16.0 × 20.0 м, том 96×64×80.
///
/// Ангар с воротами в торце, площадкой перед ними и портальным краном
/// над площадкой. Крыша — пилообразная: три ската листами и три
/// вертикальных стеклянных фонаря. Сверху это прямоугольник
/// с вырезом и двумя рельсами поперёк выреза — форма, которой больше
/// нет ни у одной постройки.
pub fn factory() -> UnitBody {
    let mut b = UnitBody::new_scaled(96, 64, 80, 4, COARSE);
    let a = Part::Armour;

    // Ангар: стены, пол и крыша. Ворота вырезаются тем, что передняя
    // стена кладётся не целиком, а косяками и перемычкой; одна
    // створка задвинута наполовину.
    fill(&mut b, (0, 3), (0, 48), (0, 80), a);
    fill(&mut b, (0, 64), (0, 48), (0, 3), a);
    fill(&mut b, (0, 64), (0, 48), (77, 80), a);
    fill(&mut b, (0, 64), (0, 3), (0, 80), a);
    fill(&mut b, (0, 64), (45, 48), (0, 80), a);
    fill(&mut b, (61, 64), (0, 48), (0, 20), a);
    fill(&mut b, (61, 64), (0, 48), (60, 80), a);
    fill(&mut b, (61, 64), (44, 48), (20, 60), a);
    fill_f(&mut b, (62, 63), (3, 44), (20, 40), a, FINISH_STEEL);
    rivets(&mut b, (62, 42, 21), 2, 18, 3, a);
    // Пилообразная крыша: скат — лист, фонарь — стекло во всю длину.
    for k in 0..3usize {
        let x0 = 4 + 20 * k;
        let ramp = slab(16, 2, 72, COARSE, a, FINISH_PLATE);
        mount(
            &mut b,
            Attach::aimed((x0 as i16, 48, 40), (0, 2, 36), (14, 8, 0)),
            ramp,
            8,
            0,
            a,
        );
        fill_f(
            &mut b,
            (x0 + 14, x0 + 16),
            (48, 56),
            (5, 75),
            a,
            FINISH_OPTIC,
        );
        fill(&mut b, (x0 + 14, x0 + 16), (48, 56), (4, 5), a);
        fill(&mut b, (x0 + 14, x0 + 16), (48, 56), (75, 76), a);
    }
    // Труба у задней стены и трубопровод вдоль неё.
    cyl_f(&mut b, 1, (48, 62), (8, 72), 3, 0, Part::Engine, FINISH_GUN);
    cyl_f(
        &mut b,
        1,
        (56, 58),
        (8, 72),
        4,
        3,
        Part::Engine,
        FINISH_STEEL,
    );
    carve(&mut b, (7, 10), (61, 62), (71, 74));
    fill_f(&mut b, (3, 5), (40, 42), (10, 70), a, FINISH_STEEL);

    // Площадка выката и портальный кран над ней: тележка и крюк.
    fill(&mut b, (64, 96), (0, 3), (24, 56), a);
    for &x0 in &[68usize, 86] {
        for &z0 in &[20usize, 56] {
            fill(&mut b, (x0, x0 + 6), (0, 44), (z0, z0 + 4), a);
        }
        fill(&mut b, (x0, x0 + 6), (40, 44), (20, 60), a);
    }
    fill(&mut b, (74, 86), (42, 46), (20, 60), a);
    fill_f(&mut b, (76, 84), (38, 42), (32, 40), a, FINISH_STEEL);
    rod(
        &mut b,
        (80, 38, 36),
        (80, 26, 36),
        2,
        2,
        -1,
        a,
        FINISH_STEEL,
        false,
    );

    // Реактор внутри ангара и постамент ядра рядом с ним.
    disc(&mut b, (20, 40), 11, (3, 40), Part::Engine);
    fill(&mut b, (36, 48), (3, 20), (34, 46), a);
    fill(&mut b, (40, 44), (8, 12), (38, 42), Part::Core);

    // Диспетчерская вышка над крышей — остеклённая кабина, тарелка —
    // и точка ПВО на площадке.
    fill(&mut b, (8, 20), (48, 58), (8, 20), a);
    fill(&mut b, (6, 22), (56, 62), (6, 22), Part::Sensor);
    fill(&mut b, (6, 22), (62, 64), (6, 22), a);
    // Постамент тарелки — сенсор в **корневом** томе, а не только
    // в сегменте: камера смотрит на крышу, а сегментов проекция
    // сверху не видит, и роль постройки читалась бы по подписи.
    fill_f(
        &mut b,
        (12, 17),
        (62, 64),
        (12, 17),
        Part::Sensor,
        FINISH_OPTIC,
    );
    let dish = slab(9, 1, 9, COARSE, Part::Sensor, FINISH_OPTIC);
    mount(
        &mut b,
        Attach {
            pitch: TURN / 8,
            ..Attach::flat((14, 64, 14), (4, 0, 4))
        },
        dish,
        -1,
        -1,
        Part::Sensor,
    );
    // Дорожка от площадки выката к точке ПВО: без неё тумба стоит
    // на воздухе, и орудие оказывается отрезанным от ядра.
    fill(&mut b, (76, 84), (0, 3), (6, 26), a);
    disc(&mut b, (80, 12), 6, (3, 8), a);
    mark_cyl(&mut b, 1, (7, 8), (80, 12), 6, FINISH_STEEL);
    fill(&mut b, (76, 84), (8, 14), (8, 16), Part::Weapon);
    gun(&mut b, (84, 12, 12), (94, 17, 12), 4, false, false);

    seat(b, "завод")
}

/// **Склад.** 16.0 × 8.0 × 16.0 м, том 64×32×64.
///
/// Каре из ящиков вокруг открытого двора, в середине двора — башня
/// с наклонными конвейерами на четыре стороны и стрелой крана.
/// Сверху это рамка с крестом внутри, и двор в ней настоящий: он
/// проверяется тестом как дыра в проекции, а не на глаз.
pub fn depot() -> UnitBody {
    let mut b = UnitBody::new_scaled(64, 32, 64, 4, COARSE);
    let a = Part::Armour;

    // Ящики по периметру. Между ними промежутки — сплошная стена
    // читалась бы как бункер, а не как склад.
    for &(x0, z0) in &[
        (4usize, 0usize),
        (26, 0),
        (44, 0),
        (4, 52),
        (26, 52),
        (44, 52),
        (0, 4),
        (0, 26),
        (0, 44),
        (52, 4),
        (52, 26),
        (52, 44),
    ] {
        // Packed cargo boxes; the courtyard between boxes remains open.
        fill(&mut b, (x0, x0 + 16), (0, 20), (z0, z0 + 12), a);
        rivets(&mut b, (x0 + 1, 19, z0 + 1), 0, 14, 3, a);
    }
    // Обходной мостик по верху с поручнем по наружной кромке: он
    // и связывает ящики между собой.
    fill(&mut b, (0, 64), (18, 20), (0, 4), a);
    fill(&mut b, (0, 64), (18, 20), (60, 64), a);
    fill(&mut b, (0, 4), (18, 20), (0, 64), a);
    fill(&mut b, (60, 64), (18, 20), (0, 64), a);
    rail(&mut b, (0, 20, 0), 0, 64, a);
    rail(&mut b, (0, 20, 63), 0, 64, a);
    rail(&mut b, (0, 20, 0), 2, 64, a);
    rail(&mut b, (63, 20, 0), 2, 64, a);

    // Башня в середине двора, наклонные конвейеры к каре — лента
    // резиной поверх станины, стойки под ними в корне.
    fill(&mut b, (26, 38), (0, 28), (26, 38), a);
    fill(&mut b, (28, 36), (0, 4), (28, 36), Part::Engine);
    let belt = |len: u8| {
        let mut s = slab(len, 2, 4, COARSE, a, FINISH_PLATE);
        mark(&mut s, (0, len as usize), (1, 2), (0, 4), FINISH_RUBBER);
        s
    };
    // Станина под лентой ложится **в корень** и на шесть вокселей
    // ниже ленты: конвейер и есть то, чем башня со двора держится
    // за каре, а связность считает корневой том, не сегменты
    // (ADR-231). Тонкая станина не годится: наклонный лист
    // в два воксела задевает центры вокселей через один, и путь
    // рвётся — башня с ядром оказывалась отрезана от орудия.
    mount(
        &mut b,
        Attach::aimed((26, 18, 30), (0, 2, 0), (-16, -6, 0)),
        belt(17),
        6,
        0,
        a,
    );
    mount(
        &mut b,
        Attach::aimed((38, 18, 30), (0, 2, 0), (16, -6, 0)),
        belt(17),
        6,
        0,
        a,
    );
    mount(
        &mut b,
        Attach::aimed((30, 18, 26), (0, 2, 0), (0, -6, -16)),
        belt(17),
        6,
        0,
        a,
    );
    mount(
        &mut b,
        Attach::aimed((30, 18, 38), (0, 2, 0), (0, -6, 16)),
        belt(17),
        6,
        0,
        a,
    );
    for &(x, z) in &[(16usize, 30usize), (46, 30), (30, 16), (30, 46)] {
        fill_f(&mut b, (x, x + 4), (0, 14), (z, z + 4), a, FINISH_STEEL);
    }
    // Стрела крана с башни — вверх и в сторону, крюк на тросе.
    rod(
        &mut b,
        (32, 26, 32),
        (46, 31, 32),
        4,
        3,
        -1,
        a,
        FINISH_STEEL,
        false,
    );
    rod(
        &mut b,
        (44, 31, 32),
        (44, 24, 32),
        2,
        2,
        -1,
        a,
        FINISH_STEEL,
        false,
    );

    // Наблюдательный колпак на башне, антенна, пулемётная точка
    // на углу каре с поднятым стволом.
    disc(&mut b, (32, 32), 8, (28, 32), Part::Sensor);
    antenna(&mut b, (28, 28, 28), 4, false);
    fill(&mut b, (4, 12), (20, 27), (2, 10), Part::Weapon);
    gun(&mut b, (12, 25, 6), (20, 27, 6), 3, false, false);
    fill(&mut b, (29, 33), (4, 8), (29, 33), Part::Core);

    seat(b, "склад")
}

/// **Турель.** 6.0 × 9.0 × 6.0 м, том 24×36×24.
///
/// Круглое основание, тумба, голова в наклонных листах с двумя
/// поднятыми стволами. Единственная постройка набора выше своей
/// ширины: оборону видно поверх всего остального.
pub fn turret() -> UnitBody {
    let mut b = UnitBody::new_scaled(24, 36, 24, 3, COARSE);
    let a = Part::Armour;
    // Основание — круг заметно уже габарита: сверху турель обязана
    // читаться кругом, а не квадратом, иначе на сетке силуэтов
    // она сходится с заводским ангаром.
    disc(&mut b, (12, 12), 9, (0, 8), a);
    mark_cyl(&mut b, 1, (7, 8), (12, 12), 9, FINISH_STEEL);
    disc(&mut b, (12, 12), 5, (2, 7), Part::Engine);
    // Погон уже основания — уступ и делает основание основанием.
    disc(&mut b, (12, 12), 8, (8, 12), a);
    disc(&mut b, (12, 12), 6, (12, 20), a);
    mark_cyl(&mut b, 1, (12, 20), (12, 12), 6, FINISH_STEEL);
    plated_turret(
        &mut b,
        (4, 16),
        (20, 30),
        (6, 18),
        (5, 3),
        TURN / 14,
        11,
        2,
        true,
        a,
        FINISH_PLATE,
    );
    fill(&mut b, (10, 16), (22, 28), (8, 16), Part::Weapon);
    for &z in &[10i32, 14] {
        // Пенёк ствола у маски — **орудие**, а не корпус: ствол
        // дальше уходит сегментом, а проекция сверху читает только
        // корневой том, и турель без него смотрелась бы коробкой
        // без оружия.
        cyl_f(&mut b, 0, (16, 18), (25, z), 2, 0, Part::Weapon, FINISH_GUN);
        gun(&mut b, (18, 25, z), (24, 26, z), 3, false, true);
    }
    fill(&mut b, (8, 16), (30, 33), (8, 16), a);
    disc(&mut b, (12, 12), 4, (33, 36), Part::Sensor);
    antenna(&mut b, (6, 30, 8), 4, true);
    fill(&mut b, (10, 13), (13, 16), (10, 13), Part::Core);
    let mut b = seat(b, "турель");
    // Вращается всё выше погона; основание стоит.
    b.turret_y = 12;
    b
}

/// **Буровая.** 12.0 × 14.0 × 12.0 м, том 48×56×48.
///
/// Четыре опоры-сегмента, сходящиеся к кронблоку, колонна по оси,
/// наклонный жёлоб и бункер сбоку, вентилятор с крученными
/// лопастями на силовом блоке. Сверху это квадрат из четырёх пятен
/// с кругом в середине и отростком в одну сторону — ни на что
/// в наборе не похоже.
///
/// Буровая головка помечена `Weapon` намеренно: подсистем ровно три
/// и список закрыт (ADR-095), а роль «излучатель, снимающий воксели»
/// у бура ровно та же, что у орудия. Отрежут головку — установка
/// перестанет копать, и это ровно тот отказ, который `Weapon` и
/// описывает.
pub fn drill_rig() -> UnitBody {
    let mut b = UnitBody::new_scaled(48, 56, 48, 4, COARSE);
    let a = Part::Armour;

    // Опорные пятна и сходящиеся к кронблоку ноги — звенья-сегменты
    // с костью внутри.
    for &(px, pz) in &[(2usize, 2usize), (2, 34), (34, 2), (34, 34)] {
        fill(&mut b, (px, px + 12), (0, 3), (pz, pz + 12), a);
    }
    for &(fx, fz, tx, tz) in &[
        (8i32, 8i32, 20i32, 20i32),
        (8, 40, 20, 28),
        (40, 8, 28, 20),
        (40, 40, 28, 28),
    ] {
        fill_f(
            &mut b,
            ((fx - 2) as usize, (fx + 2) as usize),
            (3, 5),
            ((fz - 2) as usize, (fz + 2) as usize),
            a,
            FINISH_STEEL,
        );
        rod(
            &mut b,
            (fx, 4, fz),
            (tx, 38, tz),
            8,
            6,
            1,
            a,
            FINISH_PLATE,
            false,
        );
    }
    // Пояса жёсткости. Радиусы взяты по тому, где в этой высоте
    // реально проходят ноги, — пояс мимо ног был бы кольцом в воздухе.
    ring(&mut b, (24, 24), 16, 13, (17, 19), a);
    ring(&mut b, (24, 24), 12, 8, (27, 29), a);

    // Колонна целиком и головка под ней — один излучатель: отрежут
    // колонну, и копать установка перестанет ровно так же, как без
    // головки.
    disc(&mut b, (24, 24), 5, (6, 40), Part::Weapon);
    disc(&mut b, (24, 24), 7, (0, 6), Part::Weapon);

    // Кронблок — рама с проёмом. Сплошной он накрыл бы колонну, и
    // сверху установка стала бы ящиком на четырёх ногах: чем она
    // занята, было бы видно только сбоку.
    fill(&mut b, (16, 32), (38, 48), (16, 20), a);
    fill(&mut b, (16, 32), (38, 48), (28, 32), a);
    fill(&mut b, (16, 20), (38, 48), (20, 28), a);
    fill(&mut b, (28, 32), (38, 48), (20, 28), a);
    rail(&mut b, (16, 48, 16), 0, 16, a);
    rail(&mut b, (16, 48, 31), 0, 16, a);

    // Наклонный жёлоб к бункеру отвала и сам бункер.
    let chute = slab(17, 2, 6, COARSE, a, FINISH_PLATE);
    mount(
        &mut b,
        Attach::aimed((30, 34, 24), (0, 2, 3), (14, -8, 0)),
        chute,
        -1,
        -1,
        a,
    );
    fill_f(&mut b, (30, 34), (26, 34), (22, 26), a, FINISH_STEEL);
    fill(&mut b, (36, 48), (6, 26), (16, 32), a);
    carve(&mut b, (37, 47), (25, 26), (17, 31));

    // Силовой блок на раме вдоль одной из сторон, на нём вентилятор:
    // ступица и четыре лопасти, каждая повёрнута вокруг своей оси —
    // крен даёт закрутку, рыскание разводит их по кругу.
    fill(&mut b, (4, 12), (3, 6), (12, 36), a);
    fill(&mut b, (2, 14), (6, 16), (18, 30), Part::Engine);
    cyl_f(
        &mut b,
        1,
        (16, 19),
        (8, 24),
        2,
        0,
        Part::Engine,
        FINISH_STEEL,
    );
    for k in 0..4i32 {
        let blade = slab(5, 1, 2, COARSE, Part::Engine, FINISH_STEEL);
        b.push_segment(
            Attach {
                yaw: k * TURN / 4,
                roll: TURN / 10,
                ..Attach::flat((8, 18, 24), (0, 0, 1))
            },
            blade,
        );
    }

    // Мачта наблюдения — на углу рамы, мимо проёма; антенна.
    fill(&mut b, (16, 22), (48, 52), (16, 22), a);
    disc(&mut b, (19, 19), 5, (52, 56), Part::Sensor);
    antenna(&mut b, (30, 48, 30), 6, false);
    fill(&mut b, (22, 26), (10, 14), (22, 26), Part::Core);

    pack(&mut b, (27, 28), (37, 38), (19, 20));
    pack(&mut b, (20, 21), (37, 38), (28, 29));
    seat(b, "буровая")
}

// Силуэт сверху. Одна реализация на отчёт и на тесты: величина,
// посчитанная дважды, расходится молча.
// ---------------------------------------------------------------------

/// Проекция тела сверху: роль **верхнего** занятого воксела каждой
/// клетки, `None` — сквозь клетку видно землю.
///
/// Это и есть то, что видит игрок с высоты боевой камеры, и потому
/// единственная честная мера утверждения «силуэт читается сверху».
pub fn top_view(body: &UnitBody) -> Vec<Option<Part>> {
    let (nx, ny, nz) = (
        body.vol.nx as usize,
        body.vol.ny as usize,
        body.vol.nz as usize,
    );
    let mut v = vec![None; nx * nz];
    for z in 0..nz {
        for x in 0..nx {
            for y in (0..ny).rev() {
                if body.vol.get(x, y, z) {
                    v[z * nx + x] = Some(body.part_at(x, y, z));
                    break;
                }
            }
        }
    }
    v
}

/// След: занята ли клетка `(x, z)` хоть на какой высоте.
///
/// Меры силуэта берут `UnitVol`, а не `UnitBody`: силуэт — свойство
/// занятости, и обломок, у которого меток нет вовсе, обязан меряться
/// тем же кодом.
fn covered(vol: &UnitVol) -> Vec<bool> {
    let (nx, ny, nz) = (vol.nx as usize, vol.ny as usize, vol.nz as usize);
    let mut f = vec![false; nx * nz];
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                if vol.get(x, y, z) {
                    f[z * nx + x] = true;
                }
            }
        }
    }
    f
}

/// Заполненность следа в **промилле**: доля занятых клеток проекции.
///
/// Тысяча — сплошной прямоугольник, то есть сверху постройка выглядит
/// плитой, чем бы она ни была сбоку.
pub fn footprint_permille(vol: &UnitVol) -> u32 {
    let v = covered(vol);
    (v.iter().filter(|c| **c).count() * 1000 / v.len().max(1)) as u32
}

/// Дворов в проекции сверху: связных пятен пустоты, до которых
/// не добраться снаружи.
///
/// Каре склада отличается от бруска того же габарита ровно этим,
/// и отличие видно только сверху.
pub fn courtyards(vol: &UnitVol) -> u32 {
    let (nx, nz) = (vol.nx as usize, vol.nz as usize);
    let v = covered(vol);
    // Достижимое снаружи — заливка от рамки. Оставшаяся пустота
    // и есть дворы.
    let mut open = vec![false; nx * nz];
    let mut st: Vec<usize> = Vec::new();
    let seed = |i: usize, open: &mut Vec<bool>, st: &mut Vec<usize>| {
        if !v[i] && !open[i] {
            open[i] = true;
            st.push(i);
        }
    };
    for x in 0..nx {
        seed(x, &mut open, &mut st);
        seed((nz - 1) * nx + x, &mut open, &mut st);
    }
    for z in 0..nz {
        seed(z * nx, &mut open, &mut st);
        seed(z * nx + nx - 1, &mut open, &mut st);
    }
    let flood = |st: &mut Vec<usize>, mark: &mut Vec<bool>| {
        while let Some(i) = st.pop() {
            let (x, z) = (i % nx, i / nx);
            let mut go = |k: usize, st: &mut Vec<usize>| {
                if !v[k] && !mark[k] {
                    mark[k] = true;
                    st.push(k);
                }
            };
            if x > 0 {
                go(i - 1, st);
            }
            if x + 1 < nx {
                go(i + 1, st);
            }
            if z > 0 {
                go(i - nx, st);
            }
            if z + 1 < nz {
                go(i + nx, st);
            }
        }
    };
    flood(&mut st, &mut open);

    let mut mark = open.clone();
    let mut yards = 0;
    for i in 0..nx * nz {
        if v[i] || mark[i] {
            continue;
        }
        yards += 1;
        mark[i] = true;
        st.push(i);
        flood(&mut st, &mut mark);
    }
    yards
}

/// Силуэт сверху, приведённый к сетке `n`×`n`: клетка занята, если
/// занята хоть одна клетка соответствующего блока.
///
/// Приведение нужно, чтобы силуэты **разных габаритов** сравнивались
/// как формы. Размер и так различает постройки; вопрос в том,
/// различает ли их форма, и ответ на него даёт только общая сетка.
pub fn silhouette(vol: &UnitVol, n: usize) -> Vec<bool> {
    let (nx, nz) = (vol.nx as usize, vol.nz as usize);
    let v = covered(vol);
    let mut s = vec![false; n * n];
    for z in 0..nz {
        for x in 0..nx {
            if v[z * nx + x] {
                s[(z * n / nz) * n + x * n / nx] = true;
            }
        }
    }
    s
}

/// Насколько два силуэта не совпадают, в **клетках общей сетки**.
pub fn silhouette_distance(a: &[bool], b: &[bool]) -> u32 {
    a.iter().zip(b).filter(|(x, y)| x != y).count() as u32
}

/// **Гусеничная сторона.** Низкая, на сплошной опоре.
pub fn tracked() -> Vec<(&'static str, UnitBody)> {
    vec![
        ("гусеничный разведчик", scout_tracked()),
        ("танк", crate::unit::tank()),
        ("гусеничная САУ", spg_tracked()),
        ("гусеничная командная машина", commander_tracked()),
        ("мобильная крепость", fortress()),
    ]
}

/// **Шагающая сторона.** Высокая, на точечной опоре.
pub fn walking() -> Vec<(&'static str, UnitBody)> {
    vec![
        ("шагающий разведчик", scout_walking()),
        ("штурмовой бот", assault_bot()),
        ("осадный шагоход", siege_walker()),
        ("командная машина", commander()),
        ("экспериментальный паук", spider()),
    ]
}

/// **База.** Постройки общие для обеих сторон: силуэтом разведены
/// армии, а не базы (ADR-203).
pub fn buildings() -> Vec<(&'static str, UnitBody)> {
    vec![
        ("завод", factory()),
        ("склад", depot()),
        ("турель", turret()),
        ("буровая", drill_rig()),
    ]
}

/// Весь набор с именами — для отчёта и для тестов, чтобы новое тело
/// нельзя было добавить мимо проверок.
///
/// Порядок фиксирован и сгруппирован: сторона, сторона, база.
pub fn all() -> Vec<(&'static str, UnitBody)> {
    let mut v = tracked();
    v.extend(walking());
    v.extend(buildings());
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topo::unit::{core_connectivity, CoreBufs};

    /// Габарит несвязанных вокселей — диагностика для теста ниже.
    ///
    /// Без неё тест сообщает одно число, и найти по нему висящую
    /// в воздухе мачту можно только перечитыванием разметки. Габарит
    /// указывает на неё сразу. Живёт в тесте, потому что нужна ровно
    /// там, где падает.
    type Cell = (usize, usize, usize);

    fn detached_bbox(body: &UnitBody, b: &mut CoreBufs) -> Option<(Cell, Cell)> {
        core_connectivity(&body.vol, b, body.core, body.core_size);
        let reached = b.reached_cols();
        let w = body.vol.words as usize;
        let (mut lo, mut hi) = ((usize::MAX, usize::MAX, usize::MAX), (0usize, 0, 0));
        let mut any = false;
        for z in 0..body.vol.nz as usize {
            for y in 0..body.vol.ny as usize {
                let base = (z * body.vol.ny as usize + y) * w;
                for k in 0..w {
                    let mut m = body.vol.cols[base + k] & !reached[base + k];
                    while m != 0 {
                        let x = k * 64 + m.trailing_zeros() as usize;
                        m &= m - 1;
                        lo = (lo.0.min(x), lo.1.min(y), lo.2.min(z));
                        hi = (hi.0.max(x), hi.1.max(y), hi.2.max(z));
                        any = true;
                    }
                }
            }
        }
        if any {
            Some((lo, hi))
        } else {
            None
        }
    }

    /// **У каждого тела есть ядро и все три подсистемы живы.**
    ///
    /// Это тот тест, ради которого набор и заведён: тело без ядра
    /// неубиваемо, тело с отрезанным от ядра орудием не стреляет,
    /// и оба случая молчат до самого боя. На первом прогоне он поймал
    /// две машины из шести — у обеих сенсорная мачта висела в воздухе.
    #[test]
    fn every_body_is_whole_and_armed() {
        let mut b = CoreBufs::default();
        for (name, body) in all() {
            let st = body.status(&mut b);
            assert!(st.alive, "{name}: ядро не живо");
            assert!(st.engine, "{name}: двигатель не связан с ядром");
            assert!(st.weapon, "{name}: орудие не связано с ядром");
            assert!(st.sensor, "{name}: сенсор не связан с ядром");
            assert_eq!(
                st.detached,
                0,
                "{name}: вокселей в воздухе {}, габарит {:?}",
                st.detached,
                detached_bbox(&body, &mut b)
            );
        }
    }

    /// **Постройку видно сверху, а не только сбоку.**
    ///
    /// Камера RTS смотрит на крышу. У постройки, чей след занят
    /// целиком, сверху ровный прямоугольник — плита, чем бы она
    /// ни была в профиль; у постройки, чьего орудия и сенсора сверху
    /// не видно, роль читается только по подписи. Оба случая
    /// собираются и рисуются, и оба видны только отсюда.
    #[test]
    fn every_building_reads_from_above() {
        let base = buildings();
        for (name, body) in &base {
            let fp = footprint_permille(&body.vol);
            assert!(fp < 900, "{name}: след занят на {fp}‰ — сверху это плита");
            let view = top_view(body);
            for p in [Part::Weapon, Part::Sensor] {
                assert!(
                    view.contains(&Some(p)),
                    "{name}: {} сверху не видно",
                    p.name()
                );
            }
        }
        // Проверка самой меры: сплошной короб занимает след целиком
        // и от себя не отличается. Без этого тест мог бы зеленеть
        // на мере, которая всегда мала.
        let mut slab = UnitBody::new_scaled(32, 16, 32, 2, COARSE);
        fill(&mut slab, (0, 32), (0, 16), (0, 32), Part::Armour);
        assert_eq!(
            footprint_permille(&slab.vol),
            1000,
            "мера следа не работает"
        );
        assert_eq!(courtyards(&slab.vol), 0, "двор нашёлся у сплошного короба");
        let s16 = silhouette(&slab.vol, 16);
        assert_eq!(
            silhouette_distance(&s16, &s16),
            0,
            "мера различия не работает"
        );

        // Сравниваются **формы**: размер и так различает постройки,
        // и сетка 16×16 его нарочно убирает.
        for i in 0..base.len() {
            for j in i + 1..base.len() {
                let d = silhouette_distance(
                    &silhouette(&base[i].1.vol, 16),
                    &silhouette(&base[j].1.vol, 16),
                );
                assert!(
                    d >= 48,
                    "{} и {} сверху одной формы: расходятся {d} клеток из 256",
                    base[i].0,
                    base[j].0
                );
            }
        }
    }

    /// **Стороны разведены силуэтом, а не оттенком.**
    ///
    /// Мера — высота против собственной длины: гусеничная машина
    /// вдвое длиннее своей высоты, шагающая нет. Порог один на обе
    /// стороны и строгий с одной из них, поэтому машина не может
    /// пройти обе проверки: перекрытия у классов нет.
    ///
    /// Оттенок на дистанции RTS съедает туман, а пропорцию нет —
    /// довод тот же, что у второго ванильного тела (`unit::strider`).
    #[test]
    fn the_two_sides_do_not_share_a_silhouette() {
        for (name, b) in tracked() {
            let (nx, ny) = (b.vol.nx as u32, b.vol.ny as u32);
            assert!(
                2 * ny <= nx,
                "{name}: высота {ny} при длине {nx} — для гусеничной стороны это высоко"
            );
        }
        for (name, b) in walking() {
            let (nx, ny) = (b.vol.nx as u32, b.vol.ny as u32);
            assert!(
                2 * ny > nx,
                "{name}: высота {ny} при длине {nx} — для шагающей стороны это низко"
            );
        }
        // Стороны симметричны по ролям: разведчик, линейный, осадный,
        // командир, титан. Численность — самая грубая проверка этого,
        // и она ловит забытую роль.
        assert_eq!(
            tracked().len(),
            walking().len(),
            "у сторон разное число классов"
        );
        assert_eq!(
            all().len(),
            tracked().len() + walking().len() + buildings().len(),
            "тело мимо набора: all() обязан быть склейкой трёх списков"
        );
    }

    /// **Обломок любой машины держит её материю и лежит на её следе.**
    ///
    /// Форма перекладывается из сетки тела в мировую, и это самое
    /// опасное место для инварианта сохранения: наивный пересчёт
    /// «вокселей стало вчетверо меньше» потерял бы семь восьмых
    /// материи разом и на всех машинах сразу. Проверяется весь набор,
    /// потому что плотность у машин разная, а округление ошибается
    /// на редких. Набор нужен и потому, что ребро обломка у классов
    /// разное: 0.1 м у боевой машины, 0.25 м у крупной (ADR-227).
    #[test]
    fn every_wreck_holds_its_machine() {
        for (name, body) in all() {
            let w = body.wreck(0);
            // Допуск — один воксель **обломка**: тело нечётной длины
            // в его клетку целиком не ложится, а клетка у классов
            // разная (ADR-227).
            let slack = w.mm as u32;
            assert_eq!(
                w.matter_mm3(),
                body.volume_mm3(),
                "{name}: материя не сошлась при огрублении сетки"
            );
            assert!(
                w.capacity_mm3() >= body.volume_mm3(),
                "{name}: форме обломка нечем держать материю машины"
            );
            let ((bx, by, bz), (wx, wy, wz)) = (body.extent_mm(), w.extent_mm());
            assert!(
                wx <= bx + slack && wy <= by + slack && wz <= bz + slack,
                "{name}: обломок крупнее машины: {wx}x{wy}x{wz} против {bx}x{by}x{bz}"
            );

            // Куча лежит на следе машины, а не рядом с ним.
            let k = (crate::unit::WORLD_MM / body.mm).max(1) as usize;
            let (bf, wf) = (covered(&body.vol), covered(&w.vol));
            let (nx, nz) = (body.vol.nx as usize, body.vol.nz as usize);
            let (mx, mz) = (w.vol.nx as usize, w.vol.nz as usize);
            for z in 0..mz {
                for x in 0..mx {
                    if !wf[z * mx + x] {
                        continue;
                    }
                    let mut on = false;
                    for dz in 0..k {
                        for dx in 0..k {
                            let (bx, bz) = (x * k + dx, z * k + dz);
                            on |= bx < nx && bz < nz && bf[bz * nx + bx];
                        }
                    }
                    assert!(on, "{name}: столб обломка ({x}, {z}) лёг мимо машины");
                }
            }
        }
    }

    /// **Разброс размеров действительно есть.**
    ///
    /// Набор нужен ради разброса; если он схлопнется до одного класса,
    /// проверять станет нечего, и заметить это надо здесь.
    #[test]
    fn the_roster_actually_spans_a_range() {
        let sizes: Vec<u32> = all().iter().map(|(_, b)| b.extent_mm().0).collect();
        let (lo, hi) = (
            *sizes.iter().min().expect("набор пуст"),
            *sizes.iter().max().expect("набор пуст"),
        );
        assert!(lo <= 6_000, "самая мелкая машина {lo} мм — крупнее танка");
        assert!(hi >= 40_000, "самая крупная машина всего {hi} мм");
        assert!(hi / lo >= 8, "разброс всего {}x", hi / lo);
    }

    /// **Ребро воксела следует классу, и класс его держит.**
    ///
    /// Первая редакция набора давала гигантам воксель в метр, чтобы
    /// они влезли в 64-битную колонку, — разрешение подгонялось
    /// под реализацию, и машина выходила «из кубиков покрупнее»
    /// (ADR-179). Теперь ребро названо классом (ADR-219), и у грубой
    /// сетки ровно две причины, обе проверяются здесь: машина длиннее
    /// того, что держит колонка, или это постройка, у которой цена
    /// связности измерена и запретительна (ADR-208).
    #[test]
    fn the_voxel_edge_follows_the_class() {
        let base: Vec<&str> = buildings().iter().map(|(n, _)| *n).collect();
        for (name, body) in all() {
            let (lx, ly, lz) = body.extent_mm();
            let long = lx.max(ly).max(lz);
            let coarse = base.contains(&name) || long > 255 * FINE as u32;
            let want = if coarse { COARSE } else { FINE };
            assert_eq!(
                body.mm, want,
                "{name}: воксель {} мм вместо {want}",
                body.mm
            );
            // Габарит обязан делиться на ребро без остатка: иначе
            // «шесть метров» и «шестьдесят вокселей» разъедутся.
            let per_m = body.vol.nx as usize * 1000 / lx as usize;
            assert_eq!(
                per_m,
                1000 / want as usize,
                "{name}: {per_m} вокселей на метр при ребре {want} мм"
            );
        }
        // Мелкая сетка — правило, а не исключение: если она останется
        // у одной машины, класс перестанет что-либо значить.
        let fine = all().iter().filter(|(_, b)| b.mm == FINE).count();
        assert!(fine >= 8, "мелким вокселем нарисовано всего {fine} тел");
    }

    /// **Масштаб воксела не меняет мирового объёма.**
    ///
    /// Главный инвариант ADR-178. Два одинаковых куба, собранные
    /// вокселем в 0.25 м и в 1 м, обязаны стоить экономике одинаково:
    /// «ресурс есть объём на плотность» (`CLAUDE.md`), а не число
    /// вокселей. Сломается — расхождение вылезет не сразу, а как
    /// перекос экономики между классами машин.
    #[test]
    fn the_same_block_costs_the_same_at_either_scale() {
        let mut fine = UnitBody::new_scaled(16, 16, 16, 2, 250);
        fill(&mut fine, (0, 16), (0, 16), (0, 16), Part::Armour);
        let mut coarse = UnitBody::new_scaled(4, 4, 4, 2, 1000);
        fill(&mut coarse, (0, 4), (0, 4), (0, 4), Part::Armour);
        assert_eq!(fine.extent_mm(), coarse.extent_mm(), "габариты разошлись");
        assert_eq!(
            fine.volume_mm3(),
            coarse.volume_mm3(),
            "тот же куб стоит разного: {} вокселей против {}",
            fine.solid_count(),
            coarse.solid_count()
        );
        assert_ne!(
            fine.solid_count(),
            coarse.solid_count(),
            "тест ничего не проверяет"
        );
    }

    /// **Крупное тело занимает больше одного слова колонки.**
    ///
    /// Тот самый предел, ради которого колонка стала многословной
    /// (ADR-179): на 64 битах крепость не собиралась вовсе.
    #[test]
    fn the_big_ones_need_several_words_per_column() {
        let wide = all().iter().filter(|(_, b)| b.vol.words > 1).count();
        assert!(
            wide >= 2,
            "многословных тел всего {wide} — предел не проверяется"
        );
    }
}
