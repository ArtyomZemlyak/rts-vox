//! Ход машины: скорость и курс — состояние, а не мгновенный вектор
//! (ADR-220).
//!
//! До этого юнит получал желаемую скорость и **немедленно** ею
//! двигался: направление менялось на прямой угол за тик, скорость —
//! от нуля до предельной и обратно за тик. Поле направлений
//! четырёхстороннее, поэтому диагональный путь выглядел лестницей,
//! а не ездой.
//!
//! Здесь машина: у неё есть курс, который доворачивается с предельной
//! угловой скоростью, и скорость вдоль курса, которая растёт
//! с предельным ускорением и падает с предельным торможением.
//! Если цель дальше `pivot` от курса, машина **сначала
//! разворачивается**, а уже потом едет.
//!
//! Модуль чистый нарочно: ни мира, ни навигации, ни соседей. Всё, что
//! он умеет, проверяется headless одной функцией.

use fx::angle;
use fx::Fx;
use modding::Chassis;

/// «Ехать некуда»: приказа на ход нет, машина тормозит и стоит.
///
/// Не `Option<i32>`, потому что это значение живёт в буфере фазы
/// движения по слоту на юнита, а `Option` там стоил бы выравнивания
/// на ровном месте.
pub const NO_WAY: i32 = i32::MIN;

/// Что фаза движения посчитала одному юниту за тик.
///
/// Две разные вещи, и разделены они нарочно (ADR-220). **Тяга** —
/// это `way`, желаемый курс: она идёт через трансмиссию, то есть
/// через предельный доворот и предельное ускорение. **Толчок** —
/// это расталкивание соседей: оно приложено к телу напрямую, потому
/// что контакт тел не тяга, и машина, которую подпирают в бок,
/// сдвигается вбок, а не разворачивается.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Want {
    /// Желаемый курс, двоичный угол, или [`NO_WAY`].
    pub way: i32,
    /// Толчок соседей, мировых единиц в секунду по осям.
    pub push_x: i32,
    pub push_z: i32,
}

impl Default for Want {
    /// Умолчание — «ехать некуда», а не «на восток». Ноль здесь
    /// значил бы, что свежий слот буфера трогается сам.
    fn default() -> Want {
        Want { way: NO_WAY, push_x: 0, push_z: 0 }
    }
}

/// Состояние хода одного юнита.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Drive {
    /// Курс, двоичный угол `fx::angle`.
    pub head: i32,
    /// Скорость вдоль курса, мировых единиц в секунду.
    pub vel: Fx,
}

/// Пределы хода, уже пересчитанные **на тик**.
///
/// Считаются один раз на фазу по каждому виду ходовой, а не по юниту:
/// это деления в `i128`, и повторять их пять тысяч раз незачем.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Прибавка скорости за тик.
    pub accel: Fx,
    /// Убавка скорости за тик.
    pub brake: Fx,
    /// Доворот за тик, двоичных единиц угла.
    pub turn: i32,
    /// За этим углом машина сначала разворачивается, двоичных единиц.
    pub pivot: i32,
}

impl Limits {
    /// Перевести пределы из данных в пределы за тик.
    ///
    /// Деление целое: в данных живут целые, а за тик выходит дробь
    /// (ADR-173). Доворот меньше одной двоичной единицы за тик
    /// означал бы машину, которая не поворачивается вовсе, поэтому
    /// он снизу ограничен единицей.
    pub fn of(c: &Chassis, ticks_per_second: i32) -> Limits {
        Limits {
            accel: Fx::from_ratio(c.accel, ticks_per_second),
            brake: Fx::from_ratio(c.brake, ticks_per_second),
            turn: ((c.turn as i64 * angle::FULL as i64) / (360 * ticks_per_second as i64)).max(1)
                as i32,
            pivot: ((c.pivot as i64 * angle::FULL as i64) / 360) as i32,
        }
    }
}

/// Шаг за тик по осям, мировых единиц.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    pub dx: Fx,
    pub dz: Fx,
}

/// Один тик хода.
///
/// `want` — куда машина хочет ехать (двоичный угол) или [`NO_WAY`].
/// `vmax` — предельная скорость **с учётом уклона**: её считает тот,
/// кто знает рельеф, а не этот модуль.
/// `inv_tick` — единица, делённая на частоту тика; передаётся готовой,
/// чтобы не делить на каждом юните.
///
/// Возвращает новое состояние хода и шаг за этот тик.
pub fn advance(d: Drive, want: i32, vmax: Fx, l: &Limits, inv_tick: Fx) -> (Drive, Move) {
    let mut head = d.head;
    // Цель по скорости. Курс не сбрасывается, когда ехать некуда:
    // машина, которой велели стоять, стоит **носом туда, куда
    // стояла**, а не в произвольную сторону.
    let aim = if want == NO_WAY {
        Fx::ZERO
    } else {
        let err = angle::diff(want, head);
        let turn = err.clamp(-l.turn, l.turn);
        head = angle::norm(head.wrapping_add(turn));
        // Остаток доворота — то, чего машина за этот тик не добрала.
        // Пока он больше порога, тяги нет: разворот сначала.
        if err.wrapping_sub(turn).abs() > l.pivot {
            Fx::ZERO
        } else {
            vmax
        }
    };
    // Разгон и торможение — разные числа, и это не украшение:
    // машина, тормозящая с ускорением разгона, вкатывается в цель.
    let vel = if aim.raw() > d.vel.raw() {
        Fx::min(aim, d.vel.add(l.accel))
    } else {
        Fx::max(aim, d.vel.sub(l.brake))
    };
    // Клэмп сверху обязателен отдельно: предельная скорость падает
    // от уклона, и машина, въехавшая в гору на полном ходу, обязана
    // сбросить до горного предела, а не ползти с прежней.
    let vel = Fx::max(Fx::ZERO, Fx::min(vel, vmax));
    let step = vel.mul(inv_tick);
    (
        Drive { head, vel },
        Move { dx: step.mul(angle::cos(head)), dz: step.mul(angle::sin(head)) },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TICKS_PER_SECOND;

    fn tracked() -> Chassis {
        crate::test_tuning().tracked
    }

    fn limits() -> Limits {
        Limits::of(&tracked(), TICKS_PER_SECOND)
    }

    fn inv_tick() -> Fx {
        Fx::from_ratio(1, TICKS_PER_SECOND)
    }

    /// **Машина разгоняется, а не прыгает.** Первый тик не даёт
    /// предельной скорости, и разгон занимает столько, сколько
    /// назначено данными.
    #[test]
    fn a_machine_takes_time_to_reach_its_speed() {
        let l = limits();
        let vmax = Fx::from_int(6);
        let mut d = Drive::default();
        let (first, _) = advance(d, 0, vmax, &l, inv_tick());
        assert!(first.vel.raw() > 0, "машина не тронулась вовсе");
        assert!(first.vel.raw() < vmax.raw(), "первый же тик дал предельную скорость");
        let mut ticks = 0;
        while d.vel.raw() < vmax.raw() && ticks < 1000 {
            let (n, _) = advance(d, 0, vmax, &l, inv_tick());
            d = n;
            ticks += 1;
        }
        // Шесть единиц при ускорении двенадцать — полсекунды,
        // то есть пятнадцать тиков. Плюс один: прибавка за тик это
        // 12/30, а двоичной дроби такое число не равно, и последнюю
        // тысячную предела машина добирает отдельным тиком.
        let want = 6 * TICKS_PER_SECOND / tracked().accel;
        assert!(
            ticks == want || ticks == want + 1,
            "разгон занял {ticks} тиков вместо {want}"
        );
    }

    /// **Разворот на месте.** Цель позади — машина не едет назад
    /// и не едет боком: она стоит и доворачивает, и только потом
    /// трогается.
    #[test]
    fn a_machine_turns_before_it_drives() {
        let l = limits();
        let vmax = Fx::from_int(6);
        let mut d = Drive { head: 0, vel: vmax };
        let back = angle::HALF;
        let mut moved_back = false;
        let mut ticks = 0;
        while ticks < 200 {
            let (n, m) = advance(d, back, vmax, &l, inv_tick());
            d = n;
            ticks += 1;
            if d.vel.raw() > 0 && m.dx.raw() < 0 {
                moved_back = true;
                break;
            }
            assert!(
                m.dx.raw() >= 0,
                "на тике {ticks} машина поехала назад, не развернувшись"
            );
        }
        assert!(moved_back, "машина так и не развернулась за двести тиков");
        // И разворот занял время: мгновенный был бы на первом тике.
        assert!(ticks > 10, "разворот на пол-оборота занял {ticks} тиков");
    }

    /// **Доворот ограничен.** За тик курс не меняется больше, чем
    /// позволено данными, каким бы ни был промах.
    #[test]
    fn the_turn_rate_is_a_limit() {
        let l = limits();
        for want in [1, 1000, angle::QUARTER, angle::HALF, angle::FULL - 1] {
            let d = Drive { head: 0, vel: Fx::ZERO };
            let (n, _) = advance(d, want, Fx::from_int(6), &l, inv_tick());
            assert!(
                angle::diff(n.head, 0).abs() <= l.turn,
                "курс уехал на {} при пределе {}",
                angle::diff(n.head, 0),
                l.turn
            );
        }
    }

    /// **Ехать некуда — машина встаёт, а не встаёт как вкопанная.**
    /// Торможение тоже имеет предел, и курс при этом не сбивается.
    #[test]
    fn with_nowhere_to_go_the_machine_brakes_and_keeps_its_heading() {
        let l = limits();
        let head = 12345;
        let mut d = Drive { head, vel: Fx::from_int(6) };
        let (first, _) = advance(d, NO_WAY, Fx::from_int(6), &l, inv_tick());
        assert!(first.vel.raw() > 0, "машина встала за один тик");
        let mut ticks = 0;
        while d.vel.raw() > 0 && ticks < 1000 {
            let (n, _) = advance(d, NO_WAY, Fx::from_int(6), &l, inv_tick());
            d = n;
            ticks += 1;
        }
        assert_eq!(d.vel, Fx::ZERO);
        assert_eq!(d.head, head, "стоящая машина развернулась сама");
        let want = 6 * TICKS_PER_SECOND / tracked().brake;
        assert!(
            ticks == want || ticks == want + 1,
            "торможение заняло {ticks} тиков вместо {want}"
        );
    }

    /// **Шаг идёт по курсу.** Не по желаемому направлению: машина
    /// едет туда, куда смотрит.
    #[test]
    fn the_step_follows_the_heading_not_the_wish() {
        let l = limits();
        let vmax = Fx::from_int(6);
        // Курс на юг, цель на восток, но за один тик машина
        // доворачивает лишь малость — значит и едет почти на юг.
        let d = Drive { head: angle::QUARTER, vel: vmax };
        let (n, m) = advance(d, 0, vmax, &l, inv_tick());
        assert!(m.dz.raw() > 0, "шаг не по курсу");
        assert!(
            m.dz.raw() > m.dx.abs().raw() * 4,
            "за тик машину развернуло почти целиком: dx={:?} dz={:?}",
            m.dx,
            m.dz
        );
        assert!(angle::diff(n.head, angle::QUARTER) < 0, "доворот пошёл не в ту сторону");
    }

    /// **Предельная скорость соблюдается.** Уклон её опускает,
    /// и машина обязана сбросить, а не ехать с прежней.
    #[test]
    fn the_speed_limit_holds_when_the_slope_lowers_it() {
        let l = limits();
        let mut d = Drive { head: 0, vel: Fx::from_int(6) };
        let uphill = Fx::from_int(6).mul(Fx::from_ratio(60, 100));
        for _ in 0..60 {
            let (n, m) = advance(d, 0, uphill, &l, inv_tick());
            d = n;
            assert!(d.vel.raw() <= uphill.raw(), "скорость выше предела на подъёме");
            assert!(m.dx.raw() <= uphill.mul(inv_tick()).raw() + 1);
        }
        assert_eq!(d.vel, uphill, "машина не вышла на горный предел");
    }

    /// **Виды ходовой отличаются не на словах.** Шагающая
    /// доворачивает быстрее гусеничной.
    #[test]
    fn the_walking_chassis_turns_faster_than_the_tracked_one() {
        let t = crate::test_tuning();
        let (a, b) = (
            Limits::of(&t.tracked, TICKS_PER_SECOND),
            Limits::of(&t.walking, TICKS_PER_SECOND),
        );
        assert!(b.turn > a.turn, "шагающая доворачивает не быстрее: {} против {}", b.turn, a.turn);
        assert!(a.accel.raw() > b.accel.raw(), "гусеничная разгоняется не быстрее");
    }
}
