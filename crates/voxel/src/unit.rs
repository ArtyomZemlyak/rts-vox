//! Тело юнита: воксельный том с помеченными подсистемами.
//!
//! **Урон пространственный, броня работает как геометрия** — не число
//! (`docs/05-units.md`). У юнита есть ядро; попадание снимает воксели,
//! и что именно оно задело, решает геометрия, а не таблица.
//!
//! **Подсистем ровно три, и список закрыт** (ADR-095): двигатель,
//! орудие, сенсор. Метка — поле воксела, а не отдельная сущность:
//! никаких списков компонентов на юнита. Подсистема жива, пока жив
//! **хоть один её воксель, связанный с ядром**, — отрезанная от ядра
//! не работает, даже если цела.
//!
//! **Отказ булев, а не градиентный.** Двигатель либо есть, либо нет.
//! Частичная скорость от доли уцелевших вокселей потребовала бы
//! нормировки, то есть деления в `Fx` на каждом попадании, и это
//! отдельное решение, которого нет.

use crate::topo::unit::{core_connectivity, CoreBufs, UnitVol};

/// Метка воксела. Ровно то, что перечисляет ADR-095, плюс броня и ядро.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Part {
    Armour = 0,
    Core = 1,
    Engine = 2,
    Weapon = 3,
    Sensor = 4,
}

impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::Armour => "броня",
            Part::Core => "ядро",
            Part::Engine => "двигатель",
            Part::Weapon => "орудие",
            Part::Sensor => "сенсор",
        }
    }
}

/// Три подсистемы, у которых есть отказ. Броня и ядро не подсистемы:
/// броня это геометрия, ядро это жизнь.
pub const SUBSYSTEMS: [Part; 3] = [Part::Engine, Part::Weapon, Part::Sensor];

/// Состояние юнита после попадания.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Status {
    pub alive: bool,
    pub engine: bool,
    pub weapon: bool,
    pub sensor: bool,
    /// Уцелело вокселей, связанных с ядром.
    pub attached: u32,
    /// Отвалилось от ядра — материя для реклейма.
    pub detached: u32,
}

impl Status {
    pub fn get(&self, p: Part) -> bool {
        match p {
            Part::Engine => self.engine,
            Part::Weapon => self.weapon,
            Part::Sensor => self.sensor,
            Part::Core => self.alive,
            Part::Armour => true,
        }
    }
}

#[derive(Clone)]
pub struct UnitBody {
    pub vol: UnitVol,
    /// Метка на воксель. Плотный массив: том маленький, и косвенность
    /// здесь дороже самих данных.
    part: Vec<u8>,
    /// Маски вокселей каждой подсистемы по колонкам, как у `UnitVol`.
    /// Считаются один раз при сборке: попадание их только гасит.
    masks: [Vec<u32>; 3],
    pub core: (u8, u8, u8),
    pub core_size: u8,
}

impl UnitBody {
    /// Пустой том. Положение ядра здесь **не задаётся**: оно выводится
    /// из разметки вызовом `seat_core` после того, как воксели
    /// расставлены. Причина в D-8: пока положение задавалось отдельным
    /// числом, зашитый в код танк и он же, загруженный из данных,
    /// получали разные ядра — обе величины были верны, но разные, и
    /// расхождение не было видно ниоткуда.
    pub fn new(nx: u8, ny: u8, nz: u8, core_size: u8) -> UnitBody {
        let n = nx as usize * ny as usize * nz as usize;
        UnitBody {
            vol: UnitVol::new(nx, ny, nz),
            part: vec![Part::Armour as u8; n],
            masks: [
                vec![0; ny as usize * nz as usize],
                vec![0; ny as usize * nz as usize],
                vec![0; ny as usize * nz as usize],
            ],
            core: (0, 0, 0),
            core_size: core_size.max(1),
        }
    }

    /// Посадить затравку связности на разметку.
    ///
    /// Правило одно и живёт здесь: затравка — куб `core_size` в
    /// **минимальном углу габарита вокселей, помеченных `Core`**, и
    /// весь этот куб обязан быть ядром. Возвращает `false`, если ядра
    /// нет вовсе или куб в него не влезает: юнит без ядра неубиваем,
    /// и молча пропустить это нельзя.
    pub fn seat_core(&mut self) -> bool {
        let (nx, ny, nz) = (
            self.vol.nx as usize,
            self.vol.ny as usize,
            self.vol.nz as usize,
        );
        let mut lo = (usize::MAX, usize::MAX, usize::MAX);
        let mut any = false;
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    if self.vol.get(x, y, z) && self.part_at(x, y, z) == Part::Core {
                        lo = (lo.0.min(x), lo.1.min(y), lo.2.min(z));
                        any = true;
                    }
                }
            }
        }
        if !any {
            return false;
        }
        let cs = self.core_size as usize;
        if lo.0 + cs > nx || lo.1 + cs > ny || lo.2 + cs > nz {
            return false;
        }
        for dz in 0..cs {
            for dy in 0..cs {
                for dx in 0..cs {
                    let (x, y, z) = (lo.0 + dx, lo.1 + dy, lo.2 + dz);
                    if !self.vol.get(x, y, z) || self.part_at(x, y, z) != Part::Core {
                        return false;
                    }
                }
            }
        }
        self.core = (lo.0 as u8, lo.1 as u8, lo.2 as u8);
        true
    }

    #[inline]
    fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.vol.ny as usize + y) * self.vol.nx as usize + x
    }

    #[inline]
    fn col(&self, y: usize, z: usize) -> usize {
        z * self.vol.ny as usize + y
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, p: Part) {
        self.vol.set(x, y, z, true);
        let i = self.idx(x, y, z);
        self.part[i] = p as u8;
        if let Some(k) = subsystem_slot(p) {
            let c = self.col(y, z);
            self.masks[k][c] |= 1u32 << x;
        }
    }

    pub fn part_at(&self, x: usize, y: usize, z: usize) -> Part {
        match self.part[self.idx(x, y, z)] {
            1 => Part::Core,
            2 => Part::Engine,
            3 => Part::Weapon,
            4 => Part::Sensor,
            _ => Part::Armour,
        }
    }

    /// Снять воксель. Возвращает `true`, если он там был.
    pub fn remove(&mut self, x: usize, y: usize, z: usize) -> bool {
        if !self.vol.get(x, y, z) {
            return false;
        }
        self.vol.set(x, y, z, false);
        if let Some(k) = subsystem_slot(self.part_at(x, y, z)) {
            let c = self.col(y, z);
            self.masks[k][c] &= !(1u32 << x);
        }
        true
    }

    /// Состояние после урона: что связано с ядром и какие подсистемы живы.
    ///
    /// Один флуд от ядра на весь запрос, а не по флуду на подсистему:
    /// маска прикреплённого считается один раз, а подсистемы читаются
    /// её пересечением со своими масками.
    pub fn status(&self, b: &mut CoreBufs) -> Status {
        let rep = core_connectivity(&self.vol, b, self.core, self.core_size);
        let reached = b.reached_cols();
        let live = |k: usize| -> bool {
            self.masks[k]
                .iter()
                .zip(reached)
                .any(|(m, r)| m & r != 0)
        };
        Status {
            alive: rep.core_alive,
            engine: rep.core_alive && live(0),
            weapon: rep.core_alive && live(1),
            sensor: rep.core_alive && live(2),
            attached: rep.attached,
            detached: rep.detached,
        }
    }

    pub fn solid_count(&self) -> u32 {
        self.vol.solid_count()
    }

    /// Байты тома. Метка воксела — байт, и это самая дорогая часть:
    /// занятость стоит бит.
    pub fn bytes(&self) -> usize {
        self.vol.cols.len() * 4 + self.part.len() + self.masks.iter().map(|m| m.len() * 4).sum::<usize>()
    }
}

#[inline]
fn subsystem_slot(p: Part) -> Option<usize> {
    match p {
        Part::Engine => Some(0),
        Part::Weapon => Some(1),
        Part::Sensor => Some(2),
        _ => None,
    }
}

/// Танк по ADR-094: 6×3×3 м при вокселе тел 0.25 м — 24×12×12.
///
/// Броня по краям, ядро в центре, двигатель сзади, орудие спереди
/// сверху, сенсоры по бокам башни. Форма условная: настоящие тома
/// приходят `.vox`-моделями из данных (ADR-008), а эта нужна, чтобы
/// сцена имела что разбирать.
pub fn tank() -> UnitBody {
    let (nx, ny, nz) = (24u8, 12u8, 12u8);
    let mut u = UnitBody::new(nx, ny, nz, 2);
    for z in 0..nz as usize {
        for y in 0..ny as usize {
            for x in 0..nx as usize {
                // Корпус: нижние две трети по высоте.
                let hull = y < 8;
                // Башня: середина сверху.
                let turret = y >= 8 && (6..18).contains(&x) && (3..9).contains(&z);
                // Ствол торчит вперёд из башни — он **вне** её габарита,
                // поэтому в геометрию входит отдельно. Первая редакция
                // этого не сделала, и орудия в танке не оказалось вовсе.
                let barrel = (17..24).contains(&x) && (9..11).contains(&y) && (5..7).contains(&z);
                if !hull && !turret && !barrel {
                    continue;
                }
                let p = if (10..14).contains(&x) && (4..7).contains(&y) && (5..8).contains(&z) {
                    Part::Core
                } else if x < 5 && (2..6).contains(&y) && (3..9).contains(&z) {
                    Part::Engine
                } else if barrel || (turret && x >= 14 && y >= 9 && (4..8).contains(&z)) {
                    Part::Weapon
                } else if turret && y >= 10 && !(4..8).contains(&z) {
                    Part::Sensor
                } else {
                    Part::Armour
                };
                u.set(x, y, z, p);
            }
        }
    }
    assert!(u.seat_core(), "у танка обязано быть ядро");
    u
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tank_has_every_part_and_the_right_size() {
        let t = tank();
        // 6×3×3 м при вокселе 0.25 м — том 24×12×12 (ADR-094).
        assert_eq!((t.vol.nx, t.vol.ny, t.vol.nz), (24, 12, 12));
        let mut seen = [0u32; 5];
        for z in 0..12 {
            for y in 0..12 {
                for x in 0..24 {
                    if t.vol.get(x, y, z) {
                        seen[t.part_at(x, y, z) as usize] += 1;
                    }
                }
            }
        }
        for p in [Part::Armour, Part::Core, Part::Engine, Part::Weapon, Part::Sensor] {
            assert!(seen[p as usize] > 0, "{} отсутствует", p.name());
        }
        assert!(t.solid_count() > 1000, "танк подозрительно пуст");
    }

    /// Целый танк — все подсистемы живы. Иначе всё остальное
    /// бессмысленно.
    #[test]
    fn an_intact_tank_works() {
        let t = tank();
        let mut b = CoreBufs::new();
        let s = t.status(&mut b);
        assert!(s.alive && s.engine && s.weapon && s.sensor, "{s:?}");
        assert_eq!(s.detached, 0);
    }

    /// **Отстрел подсистемы отключает способность, не убивая юнита.**
    /// Это то, ради чего выбраны помеченные подсистемы (ADR-095).
    #[test]
    fn shooting_out_the_engine_stops_the_tank_but_does_not_kill_it() {
        let mut t = tank();
        let mut b = CoreBufs::new();
        for z in 0..12 {
            for y in 0..12 {
                for x in 0..24 {
                    if t.vol.get(x, y, z) && t.part_at(x, y, z) == Part::Engine {
                        t.remove(x, y, z);
                    }
                }
            }
        }
        let s = t.status(&mut b);
        assert!(s.alive, "танк умер от потери двигателя");
        assert!(!s.engine, "двигатель жив после отстрела");
        assert!(s.weapon && s.sensor, "пострадали чужие подсистемы: {s:?}");
    }

    /// **Подсистема, отрезанная от ядра, не работает, даже если цела.**
    /// Способность считается по связности (ADR-095, пункт 4).
    #[test]
    fn a_subsystem_severed_from_the_core_stops_working() {
        let mut t = tank();
        let mut b = CoreBufs::new();
        assert!(t.status(&mut b).engine);
        // Срезаем корпус между двигателем и ядром: сам двигатель цел.
        for z in 0..12 {
            for y in 0..12 {
                for x in 5..9 {
                    t.remove(x, y, z);
                }
            }
        }
        let mut engine_left = 0;
        for z in 0..12 {
            for y in 0..12 {
                for x in 0..24 {
                    if t.vol.get(x, y, z) && t.part_at(x, y, z) == Part::Engine {
                        engine_left += 1;
                    }
                }
            }
        }
        assert!(engine_left > 0, "двигатель случайно снесли — тест ни о чём");
        let s = t.status(&mut b);
        assert!(s.alive, "ядро должно быть цело");
        assert!(!s.engine, "отрезанный двигатель работает");
        assert!(s.detached > 0, "отрезанное не посчитано");
    }

    /// Пробитие к ядру убивает.
    #[test]
    fn destroying_the_core_kills() {
        let mut t = tank();
        let mut b = CoreBufs::new();
        for z in 0..12 {
            for y in 0..12 {
                for x in 0..24 {
                    if t.vol.get(x, y, z) && t.part_at(x, y, z) == Part::Core {
                        t.remove(x, y, z);
                    }
                }
            }
        }
        let s = t.status(&mut b);
        assert!(!s.alive, "танк жив без ядра");
    }

    /// Урон пространственный: попадание в нос не трогает двигатель
    /// в корме. Броня работает как геометрия.
    #[test]
    fn damage_is_positional() {
        let mut t = tank();
        let mut b = CoreBufs::new();
        for z in 0..12 {
            for y in 0..8 {
                for x in 20..24 {
                    t.remove(x, y, z);
                }
            }
        }
        let s = t.status(&mut b);
        assert!(s.alive && s.engine, "нос снесли, а двигатель встал: {s:?}");
    }
}

#[cfg(test)]
mod core_seat_tests {
    use super::*;

    /// Затравка обязана лежать внутри разметки ядра, иначе связность
    /// считается не от ядра.
    #[test]
    fn the_seed_cube_lies_inside_the_core_marking() {
        let t = tank();
        let cs = t.core_size as usize;
        let (cx, cy, cz) = (t.core.0 as usize, t.core.1 as usize, t.core.2 as usize);
        for dz in 0..cs {
            for dy in 0..cs {
                for dx in 0..cs {
                    assert_eq!(
                        t.part_at(cx + dx, cy + dy, cz + dz),
                        Part::Core,
                        "затравка вышла из ядра"
                    );
                }
            }
        }
    }

    /// Том без ядра обязан быть отвергнут, а не собран.
    #[test]
    fn a_body_without_a_core_is_refused() {
        let mut u = UnitBody::new(4, 4, 4, 2);
        for z in 0..4 {
            for y in 0..4 {
                for x in 0..4 {
                    u.set(x, y, z, Part::Armour);
                }
            }
        }
        assert!(!u.seat_core());
    }

    /// Ядро меньше затравки — тоже отказ: куб не влезает в разметку.
    #[test]
    fn a_core_smaller_than_the_seed_is_refused() {
        let mut u = UnitBody::new(4, 4, 4, 2);
        for z in 0..4 {
            for y in 0..4 {
                for x in 0..4 {
                    u.set(x, y, z, Part::Armour);
                }
            }
        }
        u.set(1, 1, 1, Part::Core);
        assert!(!u.seat_core());
    }
}
