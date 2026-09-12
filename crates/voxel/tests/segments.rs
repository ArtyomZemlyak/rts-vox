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
    // у всех, рыскание — по четырём четвертям.
    let s = voxel::unit::strider();
    assert!(s.segments.len() >= 8, "у шагохода не восемь звеньев ног");
    let mut quads = [false; 4];
    for seg in &s.segments {
        assert!(seg.attach.pitch < 0, "звено ноги смотрит вверх");
        let y = seg.attach.yaw.rem_euclid(TURN);
        quads[(y * 4 / TURN) as usize] = true;
    }
    assert!(quads.iter().all(|&q| q), "ноги не разведены по четырём сторонам: {quads:?}");
}
