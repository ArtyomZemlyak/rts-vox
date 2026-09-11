//! Снаряды — **сущности симуляции**, а не эффект рендера.
//!
//! До этого бой был мгновенным: каждый взявший цель снимал с неё воксель
//! в тот же тик, сквозь горы и на любом расстоянии внутри радиуса. Снаряд
//! летит, тратит на дорогу тики, упирается в породу и может промахнуться
//! по цели, которая ушла (ADR-164).
//!
//! **Раскладка плотная, удаление — через свободный список.** `swap_and_pop`
//! переставляет соседей и молча меняет исход ничьей «первый снаряд в клетке»
//! (`CLAUDE.md`).
//!
//! Положение хранится так же, как у юнитов: целая клетка плюс дробная часть
//! в `Fx`. Иначе снаряд и юнит считали бы расстояние по-разному, и попадание
//! зависело бы от того, чьими глазами смотреть.

use fx::Fx;

/// Снаряды в полёте.
#[derive(Default)]
pub struct Shots {
    /// Клетка мира.
    pub x: Vec<i32>,
    pub y: Vec<i32>,
    pub z: Vec<i32>,
    /// Дробная часть положения внутри клетки.
    pub fx: Vec<Fx>,
    pub fy: Vec<Fx>,
    pub fz: Vec<Fx>,
    /// Скорость, клеток **за тик**. Делится на тики один раз, при
    /// выстреле: деление на каждый тик полёта — это деление на каждый
    /// снаряд каждый тик, а их тысячи.
    pub vx: Vec<Fx>,
    pub vy: Vec<Fx>,
    pub vz: Vec<Fx>,
    pub owner: Vec<u8>,
    pub damage: Vec<u16>,
    /// Тиков жизни осталось. Снаряд, не попавший ни во что, обязан
    /// исчезнуть: без этого их число растёт без предела, а потолок
    /// на живые снаряды запирал бы стрельбу навсегда.
    pub life: Vec<u16>,
    pub alive: Vec<bool>,
    /// Свободные места. Удаление возвращает место сюда, а не двигает
    /// соседей.
    free: Vec<u32>,
    /// Живые по возрастанию номера. Порядок обхода обязан быть
    /// устойчивым, поэтому список пересобирается, а не правится.
    pub live_ids: Vec<u32>,
    /// Сколько снарядов в воздухе. Считается отдельно от `live_ids`:
    /// тот — снимок начала фазы, а потолок надо проверять на каждом
    /// выстреле.
    alive_n: usize,
}

impl Shots {
    pub fn new() -> Shots {
        Shots::default()
    }

    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    pub fn live(&self) -> &[u32] {
        &self.live_ids
    }

    /// Сколько снарядов сейчас в воздухе.
    pub fn alive_count(&self) -> usize {
        self.alive_n
    }

    /// Выпустить снаряд. Возвращает номер места.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        &mut self,
        at: (i32, i32, i32),
        frac: (Fx, Fx, Fx),
        vel: (Fx, Fx, Fx),
        owner: u8,
        damage: u16,
        life: u16,
    ) -> u32 {
        self.alive_n += 1;
        if let Some(i) = self.free.pop() {
            let k = i as usize;
            self.x[k] = at.0;
            self.y[k] = at.1;
            self.z[k] = at.2;
            self.fx[k] = frac.0;
            self.fy[k] = frac.1;
            self.fz[k] = frac.2;
            self.vx[k] = vel.0;
            self.vy[k] = vel.1;
            self.vz[k] = vel.2;
            self.owner[k] = owner;
            self.damage[k] = damage;
            self.life[k] = life;
            self.alive[k] = true;
            return i;
        }
        self.x.push(at.0);
        self.y.push(at.1);
        self.z.push(at.2);
        self.fx.push(frac.0);
        self.fy.push(frac.1);
        self.fz.push(frac.2);
        self.vx.push(vel.0);
        self.vy.push(vel.1);
        self.vz.push(vel.2);
        self.owner.push(owner);
        self.damage.push(damage);
        self.life.push(life);
        self.alive.push(true);
        (self.x.len() - 1) as u32
    }

    /// Убрать снаряд: место уходит в свободный список.
    pub fn kill(&mut self, i: u32) {
        let k = i as usize;
        if !self.alive[k] {
            return;
        }
        self.alive[k] = false;
        self.alive_n -= 1;
        self.free.push(i);
    }

    /// Пересобрать список живых. Свободный список при этом
    /// **упорядочивается**: иначе номер выданного места зависел бы
    /// от истории смертей, а не от состояния.
    pub fn refresh_live(&mut self) {
        self.live_ids.clear();
        for i in 0..self.x.len() {
            if self.alive[i] {
                self.live_ids.push(i as u32);
            }
        }
        self.free.sort_unstable();
        self.free.reverse();
    }
}
