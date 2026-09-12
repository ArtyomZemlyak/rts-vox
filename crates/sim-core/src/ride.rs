//! Ход корпуса: наклон машины — физика, а не поворот по нормали клетки.
//!
//! **Зачем отдельная фаза.** Машина, поставленная на рельеф «плашмя»,
//! читается наклейкой: она мгновенно ложится на новый уклон, не
//! качается при разгоне и не проваливается носом на краю уступа.
//! Здесь корпус — твёрдое тело на подвеске (крейт `phys`): силы
//! в точках опоры дают тангаж, крен и раскачку, а не поворот
//! по одной нормали.
//!
//! **Высоту фаза не двигает.** Ею заведует ход и падение (ADR-135,
//! ADR-174), и переносить их сюда значило бы переписывать навигацию
//! ради наклона. Физике высота подаётся как есть, и она считает
//! **только вращение** — то, чего в симуляции не было вовсе.
//!
//! Всё состояние фазы — нормаль корпуса и угловая скорость —
//! переживает тик, поэтому входит в хеш (ADR-116).

use fx::angle;
use fx::Fx;
use modding::tuning::Chassis;
use phys::{Chassis as Hull, Ride, Support, V3};

/// Сантиметры в мировые единицы: ребро воксела мира — полметра.
///
/// Числа опоры и клиренса задаются в данных сантиметрами, потому что
/// это размеры машины, а машина меряется в метрах, а не в вокселях
/// (`CLAUDE.md`: мировые единицы, не воксели).
const CM_PER_UNIT: i32 = 50;

#[inline]
fn cm(v: i32) -> Fx {
    Fx::from_ratio(v, CM_PER_UNIT)
}

/// Числа подвески вида ходовой — в вид, понятный `phys`.
pub fn ride_of(c: &Chassis) -> Ride {
    Ride {
        stiffness: Fx::from_int(c.stiffness),
        damping: Fx::from_int(c.damping),
        travel: cm(c.travel_cm),
        clearance: cm(c.clearance_cm),
        inertia: Fx::from_int(c.inertia.max(1)),
        // Тяготение — общее число тика, а не свойство ходовой.
        gravity: Fx::ZERO,
        angular_damping: Fx::from_int(c.ang_damp),
        min_up: Fx::from_ratio(c.max_tilt_pct.clamp(0, 100), 100),
    }
}

/// Четыре опоры по углам машины: нос и корма, левый борт и правый.
///
/// Четыре, а не восемь: тангаж и крен — это ровно две степени свободы,
/// и добавлять опоры значит платить за уточнение формы, которого
/// не видно. Замер цены — ADR-223: двенадцать опор стоят вдвое дороже
/// двух, а не вшестеро, но и вдвое тоже за что-то.
pub fn supports_of(c: &Chassis) -> [Support; 4] {
    let (a, b) = (cm(c.support_x_cm), cm(c.support_z_cm));
    [
        Support::new(a, b),
        Support::new(a, b.neg()),
        Support::new(a.neg(), b),
        Support::new(a.neg(), b.neg()),
    ]
}

/// Единичный вектор курса из двоичного угла.
#[inline]
pub fn heading_of(head: i32) -> (Fx, Fx) {
    (angle::cos(head), angle::sin(head))
}

/// Шаг подвески одной машины.
///
/// `floor` — высота пола под точкой опоры, мировые единицы; её даёт
/// навигация, потому что пол — свойство нав-клетки, а не воксела
/// (ADR-121). `y` — высота корпуса, и физика её **не меняет**:
/// после шага она возвращается на место, а наружу уходит только
/// вращение.
#[allow(clippy::too_many_arguments)]
pub fn step_hull(
    up: &mut V3,
    w: &mut V3,
    load: &mut Fx,
    floors: &[Fx; 4],
    head: i32,
    r: &Ride,
    supports: &[Support; 4],
    dt: Fx,
) {
    // **Корпус висит на среднем поле опор.** Подать сюда высоту юнита
    // нельзя: она не связана со средним, и тогда одна сторона сжата
    // всегда — момент не обнуляется ни при каком наклоне, и корпус
    // крутится без остановки. На среднем равновесие есть: наклон
    // выравнивает сжатия, момент гаснет.
    let mean = floors
        .iter()
        .fold(Fx::ZERO, |a, f| a.add(*f))
        .mul(Fx::from_ratio(1, floors.len() as i32));
    let mut h = Hull { y: mean.add(r.clearance), vy: Fx::ZERO, up: *up, w: *w, load: *load };
    phys::step(&mut h, supports, floors, heading_of(head), r, dt);
    *up = h.up;
    *w = h.w;
    *load = h.load;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chassis() -> Chassis {
        Chassis {
            accel: 12,
            brake: 24,
            turn: 180,
            pivot: 105,
            climb: 60,
            descend: 125,
            stiffness: 40,
            damping: 8,
            travel_cm: 25,
            clearance_cm: 25,
            inertia: 4,
            ang_damp: 3,
            support_x_cm: 250,
            support_z_cm: 120,
            max_tilt_pct: 91,
        }
    }

    const DT: Fx = Fx::from_ratio(1, 30);

    /// **На ровном полу корпус стоит ровно.**
    #[test]
    fn a_level_floor_leaves_the_hull_level() {
        let c = chassis();
        let (r, s) = (ride_of(&c), supports_of(&c));
        let (mut up, mut w, mut load) = (V3::UP, V3::ZERO, Fx::ZERO);
        let floors = [Fx::ZERO; 4];
        for _ in 0..200 {
            step_hull(&mut up, &mut w, &mut load, &floors, 0, &r, &s, DT);
        }
        assert!(up.x.abs() < Fx::from_ratio(1, 200), "завалился по тангажу: {:?}", up.x);
        assert!(up.z.abs() < Fx::from_ratio(1, 200), "завалился по крену: {:?}", up.z);
        assert!(up.y > Fx::from_ratio(9, 10));
    }

    /// **Нос выше кормы — корпус задирается.**
    ///
    /// Это и есть разница между физикой и наклейкой: наклон приходит
    /// из разной высоты пола под носом и кормой, а не из нормали,
    /// снятой в одной точке.
    #[test]
    fn a_step_under_the_nose_pitches_the_hull() {
        let c = chassis();
        let (r, s) = (ride_of(&c), supports_of(&c));
        let (mut up, mut w, mut load) = (V3::UP, V3::ZERO, Fx::ZERO);
        // Курс нулевой: нос смотрит по +X, значит опоры с `x > 0`
        // это нос. Под ним пол выше на полметра.
        let half = Fx::from_ratio(1, 2);
        let floors = [half, half, Fx::ZERO, Fx::ZERO];
        for _ in 0..400 {
            step_hull(&mut up, &mut w, &mut load, &floors, 0, &r, &s, DT);
        }
        assert!(up.x < Fx::from_ratio(-1, 20), "нос не задрался: {:?}", up.x);
        assert!(up.z.abs() < Fx::from_ratio(1, 50), "появился крен на ровном борте");
    }

    /// **Развернули машину — тот же уступ даёт крен, а не тангаж.**
    ///
    /// Перепутанный знак в повороте опор проходит проверку выше
    /// и валится здесь.
    #[test]
    fn turning_the_machine_turns_the_tilt() {
        let c = chassis();
        let (r, s) = (ride_of(&c), supports_of(&c));
        let (mut up, mut w, mut load) = (V3::UP, V3::ZERO, Fx::ZERO);
        let half = Fx::from_ratio(1, 2);
        let floors = [half, half, Fx::ZERO, Fx::ZERO];
        // Четверть оборота: нос смотрит по +Z.
        let quarter = angle::FULL / 4;
        for _ in 0..400 {
            step_hull(&mut up, &mut w, &mut load, &floors, quarter, &r, &s, DT);
        }
        assert!(up.z.abs() > Fx::from_ratio(1, 20), "уступ не дал крена: {:?}", up.z);
    }

    /// **Один и тот же ввод даёт то же состояние до бита.**
    #[test]
    fn the_same_input_gives_the_same_tilt() {
        let c = chassis();
        let (r, s) = (ride_of(&c), supports_of(&c));
        let run = || {
            let (mut up, mut w, mut load) = (V3::UP, V3::ZERO, Fx::ZERO);
            let floors = [Fx::from_ratio(1, 3), Fx::ZERO, Fx::from_ratio(1, 5), Fx::ZERO];
            for _ in 0..300 {
                step_hull(&mut up, &mut w, &mut load, &floors, 9000, &r, &s, DT);
            }
            (up, w, load)
        };
        assert_eq!(run(), run());
    }
}
