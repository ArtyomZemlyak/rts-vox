//! Походка шагохода и плавность хода на экране (ADR-234).
//!
//! **Что здесь, а что в симуляции.** Симуляция знает про ногу ровно
//! столько, сколько нужно подвеске: где опора и на земле ли она.
//! Фаза шага — состояние симуляции (`units.gait`), потому что от неё
//! зависит, какая опора поднята, а значит и наклон корпуса. Всё,
//! что видно, — углы бедра и голени, дуга переноса, ступня на полу, —
//! считается здесь, в плавающей точке, каждый кадр, и в хеш не входит
//! (unsynced, ADR-020).
//!
//! **Звено ставится по цели ступни, а не по кривой углов.** Ступня
//! в опоре стоит на полу навигации в той точке мира, где встала, и
//! не скользит: положение вдоль хода выводится из фазы, а фаза
//! в симуляции растёт пропорционально пройденному пути. Колено
//! находится двухзвенной обратной кинематикой — окружность бедра
//! пересекается с окружностью голени, и из двух точек берётся та,
//! что в сторону сгиба, наружу от корпуса. Поворот звена берётся
//! **от рамки покоя**: кратчайший поворот, переводящий ось звена
//! из положения покоя в новое, домножается на поворот покоя.
//! Так в покое походка совпадает с тем, что нарисовано в теле,
//! и брусок не закручивается вокруг своей оси.
//!
//! **Между тиками.** Тик идёт тридцать раз в секунду, кадры — как
//! получится; корпус, переставляемый раз в тик, на ста шестидесяти
//! кадрах стоит пять кадров и прыгает на шестом. Поза кадра —
//! интерполяция между прошлым и нынешним тиком по доле недотиканного
//! времени; курс интерполируется по кратчайшей дуге, фаза шага —
//! по кольцу.

use bevy::prelude::{Quat, Vec3};
use voxel::unit::{Leg, UnitBody};

use crate::model::{root_centre, SegFrame};

/// Числа походки в мировых единицах — из данных ходовой.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GaitParams {
    /// Путь корпуса за полный цикл шага.
    pub stride: f32,
    /// Доля цикла, которую ступня стоит на земле.
    pub duty: f32,
    /// Высота переноса ступни.
    pub lift: f32,
    /// Сдвиг фазы по опорам, доли цикла.
    pub offset: [f32; 4],
}

impl GaitParams {
    /// Из данных: сантиметры в мировые единицы через ребро мирового
    /// воксела, проценты — в доли.
    pub fn of(g: &modding::tuning::Gait, per_cm: f32) -> GaitParams {
        GaitParams {
            stride: g.stride_cm as f32 * per_cm,
            duty: (g.duty_pct as f32 / 100.0).clamp(0.05, 1.0),
            lift: g.lift_cm as f32 * per_cm,
            offset: g.offset_pct.map(|p| p as f32 / 100.0),
        }
    }
}

/// Нога, посчитанная один раз при загрузке тела: шарниры покоя,
/// длины звеньев, направление сгиба и номера сегментов.
#[derive(Clone, Debug)]
pub struct LegRig {
    /// Шарниры покоя в системе корня, мировые единицы.
    pub hip: Vec3,
    pub knee: Vec3,
    pub ankle: Vec3,
    pub l1: f32,
    pub l2: f32,
    /// Куда сгибается колено: наружу от корпуса, в горизонтали.
    pub bend: Vec3,
    /// Рамки покоя звеньев — от них считается поворот.
    pub thigh_rest: SegFrame,
    pub shin_rest: SegFrame,
    pub thigh: usize,
    pub shin: usize,
    pub knee_ball: Option<usize>,
    pub ankle_ball: Option<usize>,
    pub foot: Option<usize>,
    /// Какой опоре подвески отвечает нога — по ней берётся сдвиг фазы.
    pub support: usize,
}

/// Поза одной ноги на кадр: рамки звеньев и того, что едет с ними.
#[derive(Clone, Copy, Debug)]
pub struct LegPose {
    pub thigh: SegFrame,
    pub shin: SegFrame,
    pub knee: SegFrame,
    pub ankle: SegFrame,
    pub foot: SegFrame,
}

impl LegPose {
    /// Рамка по роли звена.
    pub fn part(&self, role: LegRole) -> &SegFrame {
        match role {
            LegRole::Thigh => &self.thigh,
            LegRole::Shin => &self.shin,
            LegRole::Knee => &self.knee,
            LegRole::Ankle => &self.ankle,
            LegRole::Foot => &self.foot,
        }
    }
}

/// Роль звена в ноге — что у сущности сегмента подменяется позой.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegRole {
    Thigh,
    Shin,
    Knee,
    Ankle,
    Foot,
}

/// Собрать оснастку ног тела по его рамкам сегментов.
///
/// `per_mm` — мировых единиц в миллиметре; `frames` — рамки покоя
/// всех сегментов (`model::segment_frames`).
pub fn rigs_of(b: &UnitBody, per_mm: f32, frames: &[SegFrame]) -> Vec<LegRig> {
    let c = root_centre(b);
    let k = b.mm as f32 * per_mm;
    let at = |p: (i16, i16, i16)| {
        Vec3::new(
            (p.0 as f32 - c[0]) * k,
            (p.1 as f32 - c[1]) * k,
            (p.2 as f32 - c[2]) * k,
        )
    };
    b.legs
        .iter()
        .map(|l: &Leg| {
            let (hip, knee, ankle) = (at(l.hip), at(l.knee), at(l.ankle));
            // Сгиб — куда колено вынесено от прямой бедро–ступня,
            // в горизонтали. У шагохода это наружу по диагонали.
            let along = (ankle - hip).normalize_or_zero();
            let off = knee - hip;
            let side = off - along * off.dot(along);
            let flat = Vec3::new(side.x, 0.0, side.z);
            let bend = flat.try_normalize().unwrap_or_else(|| {
                let out = Vec3::new(hip.x, 0.0, hip.z);
                out.try_normalize().unwrap_or(Vec3::X)
            });
            let seg = |i: u8| frames.get(i as usize).copied().unwrap_or(SegFrame {
                pos: Vec3::ZERO,
                rot: Quat::IDENTITY,
                turret: false,
            });
            LegRig {
                hip,
                knee,
                ankle,
                l1: (knee - hip).length().max(1e-3),
                l2: (ankle - knee).length().max(1e-3),
                bend,
                thigh_rest: seg(l.thigh),
                shin_rest: seg(l.shin),
                thigh: l.thigh as usize,
                shin: l.shin as usize,
                knee_ball: l.knee_ball.map(|v| v as usize),
                ankle_ball: l.ankle_ball.map(|v| v as usize),
                foot: l.foot.map(|v| v as usize),
                support: l.support as usize,
            }
        })
        .collect()
}

/// Смещение ступни вдоль хода и её подъём по местной фазе `p` ∈ [0, 1).
///
/// В опоре ступня едет назад относительно корпуса ровно с той
/// скоростью, с какой корпус едет вперёд: за опору она проходит
/// `duty · stride`, а фаза за это время — `duty`. В переносе ступня
/// возвращается вперёд по синусной дуге.
pub fn foot_curve(p: f32, g: &GaitParams) -> (f32, f32) {
    let p = p.rem_euclid(1.0);
    let reach = g.duty * g.stride;
    if p < g.duty {
        (reach * 0.5 - p * g.stride, 0.0)
    } else {
        let q = (p - g.duty) / (1.0 - g.duty).max(1e-3);
        (reach * (q - 0.5), g.lift * (std::f32::consts::PI * q).sin())
    }
}

/// Корпус на кадре: всё, что нужно ноге, чтобы встать.
pub struct Stance<'a> {
    /// Середина основания корпуса в мире.
    pub base: Vec3,
    /// Поворот корпуса: курс и наклон.
    pub rot: Quat,
    /// Фаза шага, доли цикла.
    pub phase: f32,
    /// Доля предельного хода: у стоящей машины перенос без подъёма.
    pub lift_scale: f32,
    pub params: &'a GaitParams,
    /// Высота пола под точкой мира `(x, z)` — из навигации симуляции.
    pub floor: &'a dyn Fn(f32, f32) -> f32,
}

impl LegRig {
    /// Поза ноги на кадре.
    pub fn pose(&self, s: &Stance<'_>) -> LegPose {
        let g = s.params;
        let (off, lift) = foot_curve(s.phase - g.offset[self.support.min(3)], g);
        let lift = lift * s.lift_scale.clamp(0.0, 1.0);
        // Цель ступни: покой плюс ход вдоль носа корпуса, в мире.
        let want = self.ankle + Vec3::X * off;
        let w = s.base + s.rot * want;
        let floor = (s.floor)(w.x, w.z);
        // Пол под ступнёй, а не под корпусом: на склоне ступни стоят
        // на разной высоте, и ровно это отличает шаг от скольжения.
        let target_w = Vec3::new(w.x, floor + self.ankle.y + lift, w.z);
        let target = s.rot.inverse() * (target_w - s.base);
        self.solve(target, s.rot)
    }

    /// Двухзвенная обратная кинематика в системе корня: колено там,
    /// где окружность бедра встречает окружность голени, со стороны
    /// сгиба.
    pub fn solve(&self, target: Vec3, hull: Quat) -> LegPose {
        let d = target - self.hip;
        let dist = d.length().clamp(1e-3, (self.l1 + self.l2) * 0.999);
        let u = d / d.length().max(1e-6);
        let a = (self.l1 * self.l1 - self.l2 * self.l2 + dist * dist) / (2.0 * dist);
        let h = (self.l1 * self.l1 - a * a).max(0.0).sqrt();
        let n = (self.bend - u * self.bend.dot(u)).try_normalize().unwrap_or(Vec3::Y);
        let knee = self.hip + u * a + n * h;
        let ankle = self.hip + u * dist;
        let turn = |rest: &SegFrame, from: Vec3, to: Vec3, at: Vec3| -> SegFrame {
            let (from, to) = (from.normalize_or_zero(), to.normalize_or_zero());
            let arc = if from.length() > 0.5 && to.length() > 0.5 {
                Quat::from_rotation_arc(from, to)
            } else {
                Quat::IDENTITY
            };
            SegFrame { pos: at, rot: arc * rest.rot, turret: rest.turret }
        };
        let thigh = turn(&self.thigh_rest, self.knee - self.hip, knee - self.hip, self.hip);
        let shin = turn(&self.shin_rest, self.ankle - self.knee, ankle - knee, knee);
        // Шары суставов — куда пришли суставы; ступня — плоско в мире,
        // с одним лишь курсом корпуса.
        let ball = |at: Vec3| SegFrame { pos: at, rot: Quat::IDENTITY, turret: false };
        let (fwd, _, _) = hull.to_euler(bevy::math::EulerRot::YXZ);
        let foot = SegFrame {
            pos: ankle,
            rot: hull.inverse() * Quat::from_rotation_y(fwd),
            turret: false,
        };
        LegPose { thigh, shin, knee: ball(knee), ankle: ball(ankle), foot }
    }
}

// ---------------------------------------------------------------------
// Между тиками.
// ---------------------------------------------------------------------

/// Поза юнита на тике — то, что рендер интерполирует между тиками.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub pos: Vec3,
    /// Курс, радианы (`render::yaw_of`).
    pub yaw: f32,
    /// Нормаль корпуса.
    pub up: Vec3,
    /// Фаза шага, доли цикла.
    pub phase: f32,
    pub alive: bool,
}

impl Default for Pose {
    fn default() -> Pose {
        Pose { pos: Vec3::ZERO, yaw: 0.0, up: Vec3::Y, phase: 0.0, alive: false }
    }
}

/// Кратчайшая разность углов, радианы, `(-π, π]`.
pub fn angle_diff(a: f32, b: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (a - b + PI).rem_euclid(TAU) - PI
}

/// Поза между прошлым и нынешним тиком, `t` ∈ [0, 1].
///
/// Юнит, которого не было на прошлом тике, не интерполируется:
/// подкрепление, вставшее в слот убитого, иначе ехало бы через
/// полкарты из его могилы.
pub fn lerp(prev: &Pose, cur: &Pose, t: f32) -> Pose {
    if !prev.alive || !cur.alive {
        return *cur;
    }
    let t = t.clamp(0.0, 1.0);
    // Разность фаз по кольцу, `(-0.5, 0.5]`.
    let dphase = (cur.phase - prev.phase + 0.5).rem_euclid(1.0) - 0.5;
    Pose {
        pos: prev.pos.lerp(cur.pos, t),
        yaw: prev.yaw + angle_diff(cur.yaw, prev.yaw) * t,
        up: prev.up.lerp(cur.up, t).try_normalize().unwrap_or(cur.up),
        phase: (prev.phase + dphase * t).rem_euclid(1.0),
        alive: true,
    }
}

/// Фаза шага из двоичной фазы симуляции — доли цикла.
pub fn phase_of(gait: i32) -> f32 {
    (gait as u32 & (fx::angle::FULL as u32 - 1)) as f32 / fx::angle::FULL as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> GaitParams {
        GaitParams { stride: 1.6, duty: 0.75, lift: 0.4, offset: [0.25, 0.75, 0.0, 0.5] }
    }

    /// Нога как у шагохода: бедро высоко, колено наружу, ступня под
    /// корпусом.
    fn rig() -> LegRig {
        let hip = Vec3::new(-0.9, 2.7, -0.9);
        let knee = Vec3::new(-1.3, 1.5, -1.3);
        let ankle = Vec3::new(-1.1, 0.4, -1.1);
        let rest = |from: Vec3| SegFrame {
            pos: Vec3::ZERO,
            rot: Quat::from_rotation_arc(Vec3::X, from.normalize()),
            turret: false,
        };
        LegRig {
            hip,
            knee,
            ankle,
            l1: (knee - hip).length(),
            l2: (ankle - knee).length(),
            bend: Vec3::new(-1.0, 0.0, -1.0).normalize(),
            thigh_rest: rest(knee - hip),
            shin_rest: rest(ankle - knee),
            thigh: 0,
            shin: 1,
            knee_ball: None,
            ankle_ball: None,
            foot: None,
            support: 3,
        }
    }

    fn near(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-3
    }

    /// **Звенья не растягиваются.** Обратная кинематика двигает
    /// колено, а длины бедра и голени — свойство тела.
    #[test]
    fn the_links_keep_their_lengths() {
        let r = rig();
        for k in 0..40 {
            let t = r.ankle + Vec3::new(0.05 * k as f32 - 1.0, 0.02 * k as f32, 0.0);
            let p = r.solve(t, Quat::IDENTITY);
            let knee = p.shin.pos;
            let ankle = p.thigh.pos + p.thigh.rot * Vec3::X * r.l1;
            assert!(near(ankle, knee), "бедро не доходит до колена: {ankle:?} против {knee:?}");
            let l1 = (knee - p.thigh.pos).length();
            assert!((l1 - r.l1).abs() < 1e-3, "бедро стало {l1} вместо {}", r.l1);
            let tip = knee + p.shin.rot * Vec3::X * r.l2;
            assert!((tip - knee).length() - r.l2 < 1e-3);
        }
    }

    /// **В покое поза совпадает с рамками тела.** Цель — ступня покоя,
    /// значит колено обязано встать туда, где нарисовано, а повороты
    /// звеньев — остаться поворотами покоя.
    #[test]
    fn the_rest_target_reproduces_the_rest_frames() {
        let r = rig();
        let p = r.solve(r.ankle, Quat::IDENTITY);
        assert!(near(p.shin.pos, r.knee), "колено ушло из покоя: {:?}", p.shin.pos);
        assert!(near(p.thigh.rot * Vec3::X, r.thigh_rest.rot * Vec3::X));
        assert!(near(p.shin.rot * Vec3::X, r.shin_rest.rot * Vec3::X));
    }

    /// **Колено гнётся наружу, а не внутрь.** Из двух пересечений
    /// окружностей берётся то, что в сторону сгиба.
    #[test]
    fn the_knee_bends_outwards() {
        let r = rig();
        let p = r.solve(r.ankle + Vec3::Y * 0.3, Quat::IDENTITY);
        let knee = p.shin.pos;
        let along = (r.ankle - r.hip).normalize();
        let off = knee - r.hip;
        let side = off - along * off.dot(along);
        assert!(side.dot(r.bend) > 0.0, "колено ушло внутрь: {knee:?}");
    }

    /// **В опоре ступня не скользит.** Корпус прошёл долю шага —
    /// ступня относительно корпуса отъехала назад ровно на столько же.
    #[test]
    fn a_planted_foot_stays_where_it_is_in_the_world() {
        let g = params();
        // Опора: фаза от 0 до duty. Два момента в опоре.
        let (a, _) = foot_curve(0.10, &g);
        let (b, _) = foot_curve(0.40, &g);
        let body_moved = (0.40 - 0.10) * g.stride;
        assert!(((a - b) - body_moved).abs() < 1e-5, "ступня проскользнула: {a} → {b}");
    }

    /// **Перенос поднимает ступню и возвращает её вперёд.**
    #[test]
    fn the_swing_lifts_the_foot_and_brings_it_forward() {
        let g = params();
        let (start, l0) = foot_curve(g.duty, &g);
        let (mid, l1) = foot_curve(g.duty + (1.0 - g.duty) * 0.5, &g);
        let (end, _) = foot_curve(0.999, &g);
        assert_eq!(l0, 0.0);
        assert!((l1 - g.lift).abs() < 1e-3, "на середине переноса подъём {l1}");
        assert!(end > mid && mid > start, "ступня не идёт вперёд: {start} {mid} {end}");
        let (back, _) = foot_curve(0.0, &g);
        assert!((back - end).abs() < 0.02, "цикл не замкнулся: {end} против {back}");
    }

    /// **Ступня стоит на полу под собой, а не под корпусом.** На склоне
    /// пол под ступнёй ниже — и цель ступни ниже ровно на столько.
    #[test]
    fn the_foot_target_follows_the_floor_under_it() {
        let r = rig();
        let g = params();
        let run = |drop: f32| -> Vec3 {
            let floor = move |_x: f32, _z: f32| -drop;
            let s = Stance {
                base: Vec3::ZERO,
                rot: Quat::IDENTITY,
                phase: 0.6,
                lift_scale: 1.0,
                params: &g,
                floor: &floor,
            };
            r.pose(&s).ankle.pos
        };
        let flat = run(0.0);
        let low = run(0.3);
        assert!((flat.y - low.y - 0.3).abs() < 1e-3, "ступня не опустилась к полу: {flat:?} {low:?}");
    }

    /// **Курс интерполируется по кратчайшей дуге**, а не через ноль.
    #[test]
    fn yaw_is_interpolated_the_short_way_round() {
        use std::f32::consts::PI;
        let a = Pose { yaw: PI - 0.1, alive: true, ..Pose::default() };
        let b = Pose { yaw: -PI + 0.1, alive: true, ..Pose::default() };
        let m = lerp(&a, &b, 0.5);
        assert!(angle_diff(m.yaw, PI).abs() < 1e-4, "середина не у π: {}", m.yaw);
        // Фаза шага — по кольцу.
        let a = Pose { phase: 0.9, alive: true, ..Pose::default() };
        let b = Pose { phase: 0.1, alive: true, ..Pose::default() };
        let m = lerp(&a, &b, 0.5);
        assert!(m.phase < 0.01 || m.phase > 0.99, "фаза пошла через полцикла: {}", m.phase);
        // Мёртвый на прошлом тике — без интерполяции.
        let dead = Pose { alive: false, ..Pose::default() };
        let cur = Pose { pos: Vec3::new(5.0, 0.0, 5.0), alive: true, ..Pose::default() };
        assert_eq!(lerp(&dead, &cur, 0.5), cur);
    }
}
