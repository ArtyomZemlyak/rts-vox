//! Ход корпуса: наклон машины — физика, а не поворот по нормали клетки.
//!
//! **Зачем отдельная фаза.** Машина, поставленная на рельеф «плашмя»,
//! читается наклейкой: она мгновенно ложится на новый уклон, не
//! качается при разгоне и не проваливается носом на краю уступа.
//! Здесь корпус — твёрдое тело на подвеске (крейт `phys`): силы
//! в точках опоры дают тангаж, крен и раскачку, а не поворот
//! по одной нормали.
//!
//! Навигационный уровень остаётся дискретным. У подвижной машины
//! фаза интегрирует смещение корпуса над ним и вертикальную скорость
//! (ADR-252). При переходе уступа сохраняется мировая высота.
//! Опора и свободный полёт используют один шаг; только движущаяся
//! платформа владеет отдельным перемещением (ADR-253).
//!
//! Всё состояние фазы — высота, скорость, нормаль и вращение —
//! переживает тик, поэтому входит в хеш (ADR-116).

use fx::angle;
use fx::Fx;
use modding::tuning::Chassis;
use phys::{Chassis as Hull, Ride, Support, V3};

/// Сантиметры в мировые единицы: ребро воксела мира — десять
/// сантиметров (ADR-227).
///
/// Числа опоры и клиренса задаются в данных сантиметрами, потому что
/// это размеры машины, а машина меряется в метрах, а не в вокселях
/// (`CLAUDE.md`: мировые единицы, не воксели). Выводится из ребра,
/// а не пишется числом: ребро уже менялось однажды, и переживший
/// смену литерал делал подвеску впятеро мельче машины.
const CM_PER_UNIT: i32 = voxel::unit::WORLD_MM as i32 / 10;

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
        // Натяг ставится вместе с тяготением: вес машины держит он,
        // а не жёсткость (ADR-234). Здесь тяготения ещё нет — значит
        // нет и натяга.
        preload: Fx::ZERO,
        angular_damping: Fx::from_int(c.ang_damp),
        min_up: Fx::from_ratio(c.max_tilt_pct.clamp(0, 100), 100),
        settle: cm(c.travel_cm).mul(Fx::from_ratio(c.settle_pct.clamp(1, 100), 100)),
        settle_tilt: Fx::from_ratio(c.settle_tilt_pct.clamp(0, 100), 100),
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

/// Полный оборот в радианах, `710/113`: ошибка `8.5e-8`, и дробь
/// точная в `Fx` — десятичная запись `6.2831…` точной не была бы.
const TAU: Fx = Fx::from_ratio(710, 113);

/// Угловая скорость вокруг мировой вертикали из шага курса за тик.
///
/// **Знак.** Курс `head` растёт от `+X` к `+Z`, а это поворот
/// **по часовой** вокруг `+Y` (правая тройка: `+X` к `+Z` есть `−Y`).
/// Поэтому правовинтовая скорость вокруг `+Y` — это минус шаг курса.
/// Знак не выведен, а проверен: тест `a_turn_rolls_the_hull_outwards`
/// в `phys` и `a_left_turn_rolls_right` здесь.
#[inline]
pub fn yaw_rate(step: i32) -> Fx {
    Fx::from_ratio(-step * crate::TICKS_PER_SECOND, angle::FULL).mul(TAU)
}

/// Горизонтальное ускорение корпуса в мировых осях.
///
/// Продольное — вдоль курса; центростремительное — `ω × v`, то есть
/// `ω·v·(sin, −cos)` при курсе `(cos, sin)`. Оба входят в один вектор,
/// и подвеска не знает, откуда что: ей всё равно, тормозит машина
/// или входит в поворот, плечо у силы инерции одно.
pub fn sway_of(accel: Fx, vel: Fx, yaw_step: i32, head: i32) -> V3 {
    let (hc, hs) = heading_of(head);
    let w = yaw_rate(yaw_step);
    let lat = w.mul(vel);
    V3::new(
        accel.mul(hc).add(lat.mul(hs)),
        Fx::ZERO,
        accel.mul(hs).sub(lat.mul(hc)),
    )
}

/// Шаг подвески одной машины.
///
/// `floor` — высота пола под точкой опоры, мировые единицы; её даёт
/// навигация, потому что пол — свойство нав-клетки, а не воксела
/// (ADR-121). `y` — высота корпуса, и физика её **не меняет**:
/// после шага она возвращается на место, а наружу уходит только
/// вращение.
///
/// `sway` — горизонтальное ускорение корпуса в мировых осях: с ним
/// торможение роняет носом, а поворот кренит наружу. Считает его
/// фаза хода из своих же приращений скорости и курса.
#[allow(clippy::too_many_arguments)]
pub fn step_hull(
    up: &mut V3,
    w: &mut V3,
    load: &mut Fx,
    floors: &[Fx; 4],
    head: i32,
    sway: V3,
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
    let mut h = Hull {
        y: mean.add(r.clearance),
        vy: Fx::ZERO,
        up: *up,
        w: *w,
        load: *load,
    };
    phys::step(&mut h, supports, floors, heading_of(head), sway, r, dt);
    *up = h.up;
    *w = h.w;
    *load = h.load;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airborne_history_changes_the_hash_before_the_landing() {
        let tuning = crate::Tuning::from_ron(include_str!("../../../data/sim.ron")).unwrap();
        let mut w = crate::World::new(2, 1, tuning);
        let i = w.units.spawn(10, 10, 0, 30, 300) as usize;
        w.units.refresh_live();
        w.units.fall_from[i] = 100;
        let before = w.phase_hash();
        w.units.fall_from[i] = 101;
        assert_ne!(
            before,
            w.phase_hash(),
            "landing energy history was invisible to the hash"
        );
        let before = w.phase_hash();
        w.units.fall_v[i] = Fx::ONE;
        assert_ne!(before, w.phase_hash());
        let before = w.phase_hash();
        w.units.fall_f[i] = Fx::from_ratio(1, 2);
        assert_ne!(before, w.phase_hash());
    }

    fn chassis() -> Chassis {
        Chassis {
            mass: 60,
            density: 2400,
            thrust: 180,
            brake: 240,
            turn: 180,
            pivot: 105,
            pivot_speed: 40,
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
            contact_cm: 120,
            approach_pct: 60,
            settle_pct: 35,
            settle_tilt_pct: 50,
            gait: None,
        }
    }

    /// Та же ходовая с подвеской из данных: мягкой, чтобы динамику
    /// было видно. На жёсткой (40) торможение даёт десятую долю градуса
    /// — знак верен, величины нет (ADR-225).
    fn soft_chassis() -> Chassis {
        Chassis {
            stiffness: 2,
            damping: 1,
            ..chassis()
        }
    }

    const DT: Fx = Fx::from_ratio(1, 30);

    #[test]
    fn tiny_spring_residuals_do_not_drop_legacy_rays_into_the_ground() {
        let mut units = crate::Units::new();
        let i = units.spawn(0, 0, 0, 30, 100) as usize;
        units.y[i] = 80;
        for residual in [
            Fx(-1),
            Fx(1),
            Fx::from_ratio(-49, 100),
            Fx::from_ratio(49, 100),
        ] {
            units.heave[i] = residual;
            assert_eq!(units.body_cell_y(i), 80);
        }
        units.heave[i] = Fx::from_ratio(-3, 4);
        assert_eq!(units.body_cell_y(i), 79);
        units.heave[i] = Fx::from_ratio(3, 4);
        assert_eq!(units.body_cell_y(i), 81);
    }

    /// **На ровном полу корпус стоит ровно.**
    #[test]
    fn a_level_floor_leaves_the_hull_level() {
        let c = chassis();
        let (r, s) = (ride_of(&c), supports_of(&c));
        let (mut up, mut w, mut load) = (V3::UP, V3::ZERO, Fx::ZERO);
        let floors = [Fx::ZERO; 4];
        for _ in 0..200 {
            step_hull(&mut up, &mut w, &mut load, &floors, 0, V3::ZERO, &r, &s, DT);
        }
        assert!(
            up.x.abs() < Fx::from_ratio(1, 200),
            "завалился по тангажу: {:?}",
            up.x
        );
        assert!(
            up.z.abs() < Fx::from_ratio(1, 200),
            "завалился по крену: {:?}",
            up.z
        );
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
            step_hull(&mut up, &mut w, &mut load, &floors, 0, V3::ZERO, &r, &s, DT);
        }
        assert!(up.x < Fx::from_ratio(-1, 20), "нос не задрался: {:?}", up.x);
        assert!(
            up.z.abs() < Fx::from_ratio(1, 50),
            "появился крен на ровном борте"
        );
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
            step_hull(
                &mut up,
                &mut w,
                &mut load,
                &floors,
                quarter,
                V3::ZERO,
                &r,
                &s,
                DT,
            );
        }
        assert!(
            up.z.abs() > Fx::from_ratio(1, 20),
            "уступ не дал крена: {:?}",
            up.z
        );
    }

    /// **Торможение роняет машину носом — и через фазу хода тоже.**
    ///
    /// `phys` это проверяет у себя; здесь проверяется, что фаза не
    /// теряет ускорение по дороге: подать его в мировых осях, а не
    /// в осях машины, и не перепутать при повороте курса.
    #[test]
    fn braking_dives_the_nose_through_the_phase() {
        let c = soft_chassis();
        let (r, s) = (ride_of(&c), supports_of(&c));
        let floors = [Fx::ZERO; 4];
        // Курс — четверть оборота, нос смотрит по +Z. Торможение —
        // ускорение против хода, то есть по −Z.
        let quarter = angle::FULL / 4;
        let (mut up, mut w, mut load) = (V3::UP, V3::ZERO, Fx::ZERO);
        let brake = V3::new(Fx::ZERO, Fx::ZERO, Fx::from_int(-6));
        for _ in 0..300 {
            step_hull(
                &mut up, &mut w, &mut load, &floors, quarter, brake, &r, &s, DT,
            );
        }
        // Верх уходит к носу, то есть к +Z; по X клевка нет.
        assert!(up.z > Fx::from_ratio(1, 40), "не клюёт по курсу: {up:?}");
        assert!(
            up.x.abs() < Fx::from_ratio(1, 100),
            "клевок ушёл не в ту ось: {up:?}"
        );
        assert!(
            load == Fx::ONE,
            "на ровном полу все опоры на земле, а доля {load:?}"
        );
    }

    /// **Поворот к `+Z` на ходу кренит корпус к `−Z` — наружу.**
    ///
    /// Знак угловой скорости выведен в `yaw_rate` из соглашения
    /// о курсе; здесь он проверяется, а не принимается на веру.
    /// Стоящая машина в повороте не кренится вовсе: центростремительное
    /// ускорение пропорционально скорости.
    #[test]
    fn a_left_turn_rolls_right() {
        let c = soft_chassis();
        let (r, s) = (ride_of(&c), supports_of(&c));
        let floors = [Fx::ZERO; 4];
        // Нос по +X, курс растёт к +Z на 1/64 оборота за тик, ход
        // шесть единиц в секунду: центр поворота слева, на +Z.
        let step = angle::FULL / 64;
        let run = |vel: Fx| -> V3 {
            let (mut up, mut w, mut load) = (V3::UP, V3::ZERO, Fx::ZERO);
            for _ in 0..300 {
                let sway = sway_of(Fx::ZERO, vel, step, 0);
                step_hull(&mut up, &mut w, &mut load, &floors, 0, sway, &r, &s, DT);
            }
            up
        };
        let moving = run(Fx::from_int(10));
        let still = run(Fx::ZERO);
        assert!(
            moving.z < Fx::from_ratio(-1, 40),
            "крен не наружу: {moving:?}"
        );
        assert!(
            still.z.abs() < Fx::from_ratio(1, 200),
            "стоя кренится: {still:?}"
        );
    }

    /// **Один и тот же ввод даёт то же состояние до бита.**
    #[test]
    fn the_same_input_gives_the_same_tilt() {
        let c = chassis();
        let (r, s) = (ride_of(&c), supports_of(&c));
        let run = || {
            let (mut up, mut w, mut load) = (V3::UP, V3::ZERO, Fx::ZERO);
            let floors = [
                Fx::from_ratio(1, 3),
                Fx::ZERO,
                Fx::from_ratio(1, 5),
                Fx::ZERO,
            ];
            for _ in 0..300 {
                step_hull(
                    &mut up,
                    &mut w,
                    &mut load,
                    &floors,
                    9000,
                    V3::ZERO,
                    &r,
                    &s,
                    DT,
                );
            }
            (up, w, load)
        };
        assert_eq!(run(), run());
    }
}
