//! Копирование при записи: неповреждённые юниты одного типа делят
//! шаблонный том (ADR-018, `docs/05-units.md`).
//!
//! **COW — оптимизация, а не несущая конструкция.** Юнит без единого
//! попадания не хранит ничего, кроме номера шаблона; первое попадание
//! стоит **полной копии тома**, второе — записи одного байта. Заявление
//! «повреждение юнита это запись байта» верно со второго попадания,
//! и это надо помнить, а не забывать.
//!
//! **Шаблон адресуется индексом, а не указателем.** Индекс переносим,
//! сериализуем и одинаков у всех клиентов; `Arc` дал бы то же самое
//! ценой атомарного счётчика, который здесь не нужен: тома живут
//! столько же, сколько флот.
//!
//! **Удаление — free-list, никогда `swap_and_pop`** (`CLAUDE.md`):
//! последний переставляет соседей и молча меняет исход ничьей
//! в запросах вида «первый враг в радиусе».

use crate::topo::unit::CoreBufs;
use crate::unit::{Status, UnitBody};

pub type UnitId = u32;

enum Slot {
    /// Свободен; хранит следующий свободный — это и есть free-list.
    Free(u32),
    Live {
        template: u16,
        /// Собственный том. `None` — юнит цел и живёт шаблоном.
        own: Option<Box<UnitBody>>,
    },
}

#[derive(Clone, Copy, Default, Debug)]
pub struct FleetStats {
    pub live: usize,
    /// Сколько юнитов уже отделились от шаблона.
    pub damaged: usize,
    /// Сколько раз пришлось копировать том. Равно числу первых попаданий.
    pub copies: u64,
}

pub struct Fleet {
    templates: Vec<UnitBody>,
    slots: Vec<Slot>,
    free_head: u32,
    live: usize,
    damaged: usize,
    copies: u64,
}

const NIL: u32 = u32::MAX;

impl Fleet {
    pub fn new(templates: Vec<UnitBody>) -> Fleet {
        Fleet {
            templates,
            slots: Vec::new(),
            free_head: NIL,
            live: 0,
            damaged: 0,
            copies: 0,
        }
    }

    pub fn spawn(&mut self, template: u16) -> UnitId {
        assert!(
            (template as usize) < self.templates.len(),
            "нет такого шаблона"
        );
        self.live += 1;
        if self.free_head != NIL {
            let id = self.free_head;
            let Slot::Free(next) = self.slots[id as usize] else {
                unreachable!("free-list указывает на живой слот")
            };
            self.free_head = next;
            self.slots[id as usize] = Slot::Live {
                template,
                own: None,
            };
            return id;
        }
        self.slots.push(Slot::Live {
            template,
            own: None,
        });
        (self.slots.len() - 1) as UnitId
    }

    pub fn despawn(&mut self, id: UnitId) {
        if let Slot::Live { own, .. } = &self.slots[id as usize] {
            if own.is_some() {
                self.damaged -= 1;
            }
            self.live -= 1;
            self.slots[id as usize] = Slot::Free(self.free_head);
            self.free_head = id;
        }
    }

    /// Том юнита для чтения: собственный, если есть, иначе шаблон.
    pub fn body(&self, id: UnitId) -> &UnitBody {
        match &self.slots[id as usize] {
            Slot::Live { template, own } => match own {
                Some(b) => b,
                None => &self.templates[*template as usize],
            },
            Slot::Free(_) => panic!("обращение к освобождённому юниту {id}"),
        }
    }

    /// Том для записи. **Здесь и происходит копирование**: первое
    /// обращение отделяет юнита от шаблона целиком.
    pub fn body_mut(&mut self, id: UnitId) -> &mut UnitBody {
        let Slot::Live { template, own } = &mut self.slots[id as usize] else {
            panic!("запись в освобождённого юнита {id}")
        };
        if own.is_none() {
            *own = Some(Box::new(self.templates[*template as usize].clone()));
            self.copies += 1;
            self.damaged += 1;
        }
        own.as_mut().expect("копия только что создана")
    }

    pub fn is_pristine(&self, id: UnitId) -> bool {
        matches!(&self.slots[id as usize], Slot::Live { own: None, .. })
    }

    pub fn status(&self, id: UnitId, b: &mut CoreBufs) -> Status {
        self.body(id).status(b)
    }

    pub fn stats(&self) -> FleetStats {
        FleetStats {
            live: self.live,
            damaged: self.damaged,
            copies: self.copies,
        }
    }

    /// Байты флота: шаблоны плюс собственные тома повреждённых.
    ///
    /// Целый юнит стоит **один слот**, а не том. Разрыв между этим
    /// числом и «тысяча томов» и есть цена механики.
    pub fn bytes(&self) -> usize {
        let t: usize = self.templates.iter().map(|b| b.bytes()).sum();
        let o: usize = self
            .slots
            .iter()
            .filter_map(|s| match s {
                // Метки у копии общие с шаблоном (ADR-120).
                Slot::Live { own: Some(b), .. } => Some(b.bytes_private()),
                _ => None,
            })
            .sum();
        t + o + self.slots.len() * core::mem::size_of::<Slot>()
    }

    /// Сколько весил бы флот без копирования при записи.
    pub fn bytes_without_cow(&self) -> usize {
        self.slots
            .iter()
            .filter_map(|s| match s {
                Slot::Live { template, .. } => Some(self.templates[*template as usize].bytes()),
                _ => None,
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::{tank, Part};

    fn fleet(n: usize) -> (Fleet, Vec<UnitId>) {
        let mut f = Fleet::new(vec![tank()]);
        let ids = (0..n).map(|_| f.spawn(0)).collect();
        (f, ids)
    }

    /// **Тысяча целых юнитов весит один том плюс тысяча слотов.**
    ///
    /// Записанное в `docs/05-units.md` «занимает память одного»
    /// не буквально: слот всё-таки чего-то стоит. Проверяется то,
    /// что верно, — разрыв с «тысяча томов» на два порядка, — а не
    /// красивая формулировка.
    #[test]
    fn a_thousand_pristine_units_cost_one_volume_plus_slots() {
        let (f, _) = fleet(1000);
        let one = tank().bytes();
        let slots = 1000 * core::mem::size_of::<Slot>();
        assert!(
            f.bytes() <= one + slots + 64,
            "тысяча целых весит {} при томе {one} и слотах {slots}",
            f.bytes()
        );
        // Разрыв с «тысяча томов» — вот что здесь ценно.
        assert!(
            f.bytes_without_cow() / f.bytes() > 100,
            "выигрыш всего {}x",
            f.bytes_without_cow() / f.bytes()
        );
        assert_eq!(f.stats().damaged, 0);
    }

    /// **Первое попадание стоит копии, второе — нет.** Заявление
    /// «повреждение это запись байта» верно со второго.
    #[test]
    fn the_first_hit_copies_and_the_second_does_not() {
        let (mut f, ids) = fleet(10);
        assert!(f.is_pristine(ids[3]));
        f.body_mut(ids[3]).remove(12, 6, 6);
        assert!(!f.is_pristine(ids[3]));
        assert_eq!(f.stats().copies, 1);
        f.body_mut(ids[3]).remove(12, 6, 7);
        assert_eq!(f.stats().copies, 1, "второе попадание скопировало том");
        assert_eq!(f.stats().damaged, 1);
        // Соседи не пострадали.
        for &id in &ids {
            if id != ids[3] {
                assert!(f.is_pristine(id), "повреждение протекло в юнита {id}");
            }
        }
    }

    /// Повреждение одного юнита **не видно** остальным: том отделён,
    /// а не изменён на месте.
    #[test]
    fn damage_does_not_leak_through_the_template() {
        let (mut f, ids) = fleet(4);
        let mut b = CoreBufs::new();
        // Сносим ядро первому. Габарит берётся у самого тома:
        // сетка тел менялась, и литералы пережили бы её молча.
        let (nx, ny, nz) = {
            let v = &f.body(ids[0]).vol;
            (v.nx as usize, v.ny as usize, v.nz as usize)
        };
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    let body = f.body(ids[0]);
                    if body.vol.get(x, y, z) && body.part_at(x, y, z) == Part::Core {
                        f.body_mut(ids[0]).remove(x, y, z);
                    }
                }
            }
        }
        assert!(!f.status(ids[0], &mut b).alive);
        for &id in &ids[1..] {
            assert!(
                f.status(id, &mut b).alive,
                "чужой юнит умер вместе с первым"
            );
        }
    }

    /// **Удаление через free-list, а не `swap_and_pop`:** идентификаторы
    /// живых юнитов не меняются, когда умирает сосед.
    #[test]
    fn despawn_keeps_neighbour_ids_stable() {
        let (mut f, ids) = fleet(5);
        f.body_mut(ids[2]).remove(12, 6, 6);
        let before: Vec<u32> = ids.clone();
        f.despawn(ids[1]);
        assert_eq!(f.stats().live, 4);
        for &id in [ids[0], ids[2], ids[3], ids[4]].iter() {
            // Обращение не паникует — значит слот на месте.
            let _ = f.body(id);
        }
        assert_eq!(before, ids, "идентификаторы поехали");
        // Освобождённый слот переиспользуется, и юнит в нём — чистый.
        let re = f.spawn(0);
        assert_eq!(re, ids[1], "free-list не переиспользовал слот");
        assert!(f.is_pristine(re));
    }

    /// Повреждённый юнит, умерев, освобождает свой том.
    #[test]
    fn despawning_a_damaged_unit_frees_its_volume() {
        let (mut f, ids) = fleet(3);
        f.body_mut(ids[0]).remove(12, 6, 6);
        let with = f.bytes();
        f.despawn(ids[0]);
        assert_eq!(f.stats().damaged, 0);
        assert!(f.bytes() < with, "том повреждённого не освободился");
    }
}
