//! Повёрнутые части тела и отделка по вокселю (ADR-231).
//!
//! Что здесь сторожится: сегмент переживает смену сетки вместе
//! с креплением; отделка едет с ролью; воздух отделки не получает;
//! родитель обязан существовать. Знаки углов и постановка сегмента
//! в теле — на стороне рендера, там же, где и поворот.

use voxel::unit::{Attach, Part, UnitBody, FINISH_BY_PART, FINISH_RUBBER, FINISH_STEEL, TURN};

fn slab(nx: u8, ny: u8, nz: u8) -> UnitBody {
    let mut b = UnitBody::new(nx, ny, nz, 1);
    for z in 0..nz as usize {
        for y in 0..ny as usize {
            for x in 0..nx as usize {
                b.set(x, y, z, Part::Armour);
            }
        }
    }
    b
}

/// **Смена сетки пересчитывает крепление тем же отношением, что
/// и форму.** Точка посадки и шарнир меряются в вокселях, углы —
/// нет: лист, наклонённый на восьмую оборота при 100 мм, наклонён
/// на неё же при 200.
#[test]
fn a_segment_keeps_its_attachment_through_resampling() {
    let tank = voxel::unit::tank();
    assert!(!tank.segments.is_empty(), "у танка нет наклонных листов");
    let coarse = tank.resampled(200).expect("танк при 200 мм");
    assert_eq!(coarse.segments.len(), tank.segments.len(), "сегменты потерялись");
    for (a, b) in tank.segments.iter().zip(&coarse.segments) {
        let (fa, fb) = (a.attach, b.attach);
        assert_eq!((fa.pitch, fa.yaw, fa.roll), (fb.pitch, fb.yaw, fb.roll), "угол зависит от сетки");
        assert_eq!(fa.parent, fb.parent);
        assert_eq!(fa.turret, fb.turret);
        assert_eq!(fb.at.0 as i32, fa.at.0 as i32 / 2, "посадка не пересчитана");
        assert_eq!(fb.pivot.2 as i32, fa.pivot.2 as i32 / 2, "шарнир не пересчитан");
        // Габарит сегмента в метрах тот же: вокселей вдвое меньше,
        // с округлением вверх.
        assert_eq!(b.body.vol.nx as u32, (a.body.vol.nx as u32).div_ceil(2));
        assert_eq!(b.body.mm, 200);
    }
}

/// **Отделка едет с ролью через смену сетки.** Трак остаётся резиной,
/// каток — сталью, при любом ребре.
#[test]
fn finish_survives_resampling() {
    let tank = voxel::unit::tank();
    let count = |b: &UnitBody, code: u8| -> usize {
        let mut n = 0;
        for z in 0..b.vol.nz as usize {
            for y in 0..b.vol.ny as usize {
                for x in 0..b.vol.nx as usize {
                    if b.vol.get(x, y, z) && b.finish_at(x, y, z) == code {
                        n += 1;
                    }
                }
            }
        }
        n
    };
    assert!(count(&tank, FINISH_RUBBER) > 0, "траки не резина");
    assert!(count(&tank, FINISH_STEEL) > 0, "катки не сталь");
    let coarse = tank.resampled(200).expect("танк при 200 мм");
    assert!(count(&coarse, FINISH_RUBBER) > 0, "резина пропала при 200 мм");
    assert!(count(&coarse, FINISH_STEEL) > 0, "сталь пропала при 200 мм");
}

/// **Воздух отделки не получает.** Пометка пустого воксела — это
/// пятно не того цвета после ремонта, и ловить его по снимку поздно.
#[test]
fn air_takes_no_finish() {
    let mut b = UnitBody::new(3, 3, 3, 1);
    b.set(1, 1, 1, Part::Armour);
    b.set_finish(0, 0, 0, FINISH_RUBBER);
    b.set_finish(1, 1, 1, FINISH_RUBBER);
    assert_eq!(b.finish_at(0, 0, 0), FINISH_BY_PART, "воздух получил отделку");
    assert_eq!(b.finish_at(1, 1, 1), FINISH_RUBBER);
    // Снятый и поставленный заново воксель помнит отделку: ремонт
    // возвращает машине её вид, а не красит трак краской корпуса.
    assert!(b.remove(1, 1, 1));
    b.set(1, 1, 1, Part::Armour);
    assert_eq!(b.finish_at(1, 1, 1), FINISH_RUBBER, "ремонт потерял отделку");
}

/// **Родитель обязан существовать.** Сегмент, прикреплённый
/// к несуществующему, повис бы в начале координат молча.
#[test]
#[should_panic(expected = "родитель")]
fn a_parent_must_exist() {
    let mut b = slab(2, 2, 2);
    b.push_segment(Attach { parent: 3, ..Attach::flat((0, 0, 0), (0, 0, 0)) }, slab(1, 1, 1));
}

/// Цепочка: сегмент на сегменте разрешён, если родитель добавлен
/// раньше.
#[test]
fn a_segment_may_hang_on_a_segment() {
    let mut b = slab(2, 2, 2);
    b.push_segment(Attach { pitch: TURN / 8, ..Attach::flat((2, 0, 1), (0, 0, 0)) }, slab(3, 1, 1));
    b.push_segment(Attach { parent: 1, ..Attach::flat((3, 0, 0), (0, 0, 0)) }, slab(1, 1, 1));
    assert_eq!(b.segments.len(), 2);
    assert_eq!(b.segments[1].attach.parent, 1);
}

/// **Наведённое крепление смотрит по вектору.** Целочисленный
/// `atan2` даёт тот же угол, что и курс машины: вниз — отрицательный
/// тангаж, к `-Z` — положительное рыскание.
#[test]
fn an_aimed_attachment_faces_its_direction() {
    let down = Attach::aimed((0, 0, 0), (0, 0, 0), (0, -5, 0));
    assert_eq!(down.pitch, -TURN / 4, "вниз — четверть оборота вниз");
    let left = Attach::aimed((0, 0, 0), (0, 0, 0), (0, 0, -5));
    assert_eq!(left.yaw, TURN / 4, "к -Z — четверть оборота влево");
    let fwd = Attach::aimed((0, 0, 0), (0, 0, 0), (7, 0, 0));
    assert_eq!((fwd.yaw, fwd.pitch), (0, 0));
    // Ноги шагохода наведены вниз и наружу: тангаж отрицательный
    // у звеньев, рыскание — по четырём четвертям.
    let s = voxel::unit::strider();
    assert_eq!(s.legs.len(), 4, "у шагохода не четыре ноги");
    let mut quads = [false; 4];
    for l in &s.legs {
        for k in [l.thigh, l.shin] {
            let seg = &s.segments[k as usize];
            assert!(seg.attach.pitch < 0, "звено ноги смотрит вверх");
            let y = seg.attach.yaw.rem_euclid(TURN);
            quads[(y * 4 / TURN) as usize] = true;
        }
    }
    assert!(quads.iter().all(|&q| q), "ноги не разведены по четырём сторонам: {quads:?}");
}

/// **Нога описана цепочкой, и цепочка сходится.** Бедро наведено
/// от бедренного шарнира к колену, голень — от колена к лодыжке;
/// колено, лодыжка и ступня посажены в свои шарниры. Разойдись
/// шарниры ноги с креплениями звеньев — походка гнула бы одно,
/// а рисовалось бы другое (ADR-234).
#[test]
fn a_leg_is_a_chain_from_hip_to_foot() {
    let s = voxel::unit::strider();
    let mut supports = [false; 4];
    for l in &s.legs {
        let thigh = &s.segments[l.thigh as usize].attach;
        let shin = &s.segments[l.shin as usize].attach;
        assert_eq!(thigh.at, l.hip, "бедро не в бедренном шарнире");
        assert_eq!(shin.at, l.knee, "голень не в колене");
        for (name, part, at) in [
            ("колено", l.knee_ball, l.knee),
            ("лодыжка", l.ankle_ball, l.ankle),
            ("ступня", l.foot, l.ankle),
        ] {
            let k = part.unwrap_or_else(|| panic!("у ноги нет части: {name}"));
            assert_eq!(s.segments[k as usize].attach.at, at, "{name} не в своём шарнире");
        }
        assert!(l.knee.1 < l.hip.1 && l.ankle.1 < l.knee.1, "нога идёт не вниз: {l:?}");
        supports[l.support as usize] = true;
    }
    assert!(supports.iter().all(|&s| s), "ноги не разобраны по опорам: {supports:?}");
    // Смена сетки пересчитывает шарниры тем же отношением, что
    // и крепления, а номера звеньев не трогает.
    let coarse = s.resampled(200).expect("шагоход при 200 мм");
    assert_eq!(coarse.legs.len(), 4);
    for (a, b) in s.legs.iter().zip(&coarse.legs) {
        assert_eq!(b.hip.1 as i32, a.hip.1 as i32 / 2, "шарнир не пересчитан");
        assert_eq!((a.thigh, a.shin, a.support), (b.thigh, b.shin, b.support));
        assert_eq!(coarse.segments[b.thigh as usize].attach.at, b.hip);
    }
}
