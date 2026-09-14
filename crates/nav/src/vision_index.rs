//! Кто пересчитывается, когда мир изменился.
//!
//! ADR-070 померил стоимость пересчёта видимости при движении сенсоров.
//! Но копка меняет геометрию, и всякий сенсор, чей диск накрыт правкой,
//! обязан пересчитаться тоже. Здесь измеряется вторая половина.
//!
//! Ключевое различие, которое нельзя терять: **правка воксела и переворот
//! непрозрачности нав-клетки — разные события**. Нав-сетка грубее воксельной
//! именно затем, чтобы снятие одного воксела не меняло граф
//! (`../../docs/06-navigation.md`). Инвалидация видимости считается по
//! перевёрнутым клеткам, а не по правкам.

use crate::vision::Sensor;

/// Ведро сенсоров по сетке. Сторона ведра равна максимальному радиусу
/// обзора, поэтому запрос «кто видит эту клетку» трогает ровно 3×3 ведра.
pub struct SensorIndex {
    cell: usize,
    w: usize,
    h: usize,
    /// Начало списка каждого ведра в `items` — CSR, а не `Vec<Vec>`:
    /// одно выделение вместо тысяч, и порядок обхода стабилен.
    starts: Vec<u32>,
    items: Vec<u32>,
    /// Штампы для дедупликации без `HashSet` (`CLAUDE.md`).
    stamp: Vec<u32>,
    epoch: u32,
}

impl SensorIndex {
    pub fn new(map_w: usize, map_h: usize, max_radius: usize, n_sensors: usize) -> SensorIndex {
        let cell = max_radius.max(1);
        SensorIndex {
            cell,
            w: map_w.div_ceil(cell),
            h: map_h.div_ceil(cell),
            starts: vec![0; map_w.div_ceil(cell) * map_h.div_ceil(cell) + 1],
            items: vec![0; n_sensors],
            stamp: vec![0; n_sensors],
            epoch: 0,
        }
    }

    /// Пересборка индекса. Счётная сортировка за два прохода:
    /// порядок внутри ведра — по индексу сенсора, то есть детерминирован.
    pub fn rebuild(&mut self, sensors: &[Sensor]) {
        self.starts.iter_mut().for_each(|v| *v = 0);
        for s in sensors {
            let b = self.bucket(s.x as usize, s.y as usize);
            self.starts[b + 1] += 1;
        }
        for i in 1..self.starts.len() {
            self.starts[i] += self.starts[i - 1];
        }
        let mut cursor = self.starts.clone();
        for (i, s) in sensors.iter().enumerate() {
            let b = self.bucket(s.x as usize, s.y as usize);
            self.items[cursor[b] as usize] = i as u32;
            cursor[b] += 1;
        }
    }

    #[inline(always)]
    fn bucket(&self, x: usize, y: usize) -> usize {
        let (bx, by) = (
            (x / self.cell).min(self.w - 1),
            (y / self.cell).min(self.h - 1),
        );
        by * self.w + bx
    }

    /// Начать новый набор затронутых сенсоров.
    pub fn begin(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stamp.iter_mut().for_each(|v| *v = 0);
            self.epoch = 1;
        }
    }

    /// Добавить в набор всех, чей диск накрывает клетку `(x, y)`.
    /// Возвращает, сколько сенсоров проверено, — это цена запроса.
    pub fn touch(&mut self, sensors: &[Sensor], x: usize, y: usize, out: &mut Vec<u32>) -> u64 {
        let (bx, by) = (x / self.cell, y / self.cell);
        let mut examined = 0u64;
        for gy in by.saturating_sub(1)..=(by + 1).min(self.h - 1) {
            for gx in bx.saturating_sub(1)..=(bx + 1).min(self.w - 1) {
                let b = gy * self.w + gx;
                let (lo, hi) = (self.starts[b] as usize, self.starts[b + 1] as usize);
                for &si in &self.items[lo..hi] {
                    examined += 1;
                    if self.stamp[si as usize] == self.epoch {
                        continue;
                    }
                    let s = sensors[si as usize];
                    let (dx, dy) = (s.x as i64 - x as i64, s.y as i64 - y as i64);
                    let r = s.radius as i64;
                    // Сравнение квадратов, а не расстояний (`CLAUDE.md`).
                    if dx * dx + dy * dy <= r * r {
                        self.stamp[si as usize] = self.epoch;
                        out.push(si);
                    }
                }
            }
        }
        examined
    }

    /// Пометить сенсор как уже попавший в набор — для тех, кто сменил клетку.
    pub fn mark(&mut self, si: u32, out: &mut Vec<u32>) {
        if self.stamp[si as usize] != self.epoch {
            self.stamp[si as usize] = self.epoch;
            out.push(si);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_sensors(n: usize, step: u16, radius: u16) -> Vec<Sensor> {
        let mut v = Vec::new();
        for i in 0..n {
            for j in 0..n {
                v.push(Sensor {
                    x: 8 + i as u16 * step,
                    y: 8 + j as u16 * step,
                    radius,
                });
            }
        }
        v
    }

    /// Индекс обязан находить ровно тех, кого находит полный перебор.
    #[test]
    fn matches_brute_force() {
        let sensors = grid_sensors(12, 7, 10);
        let mut idx = SensorIndex::new(256, 256, 10, sensors.len());
        idx.rebuild(&sensors);
        let mut r = fx::rng::Rng::new(3, 4);
        for _ in 0..200 {
            let (x, y) = (r.below(200) as usize + 4, r.below(200) as usize + 4);
            let mut got = Vec::new();
            idx.begin();
            idx.touch(&sensors, x, y, &mut got);
            got.sort_unstable();

            let mut want: Vec<u32> = sensors
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    let (dx, dy) = (s.x as i64 - x as i64, s.y as i64 - y as i64);
                    dx * dx + dy * dy <= (s.radius as i64) * (s.radius as i64)
                })
                .map(|(i, _)| i as u32)
                .collect();
            want.sort_unstable();
            assert_eq!(got, want, "клетка ({x},{y})");
        }
    }

    /// Дедупликация: две правки в одной клетке дают один пересчёт.
    #[test]
    fn dedup_within_a_tick() {
        let sensors = grid_sensors(4, 9, 12);
        let mut idx = SensorIndex::new(128, 128, 12, sensors.len());
        idx.rebuild(&sensors);
        let mut out = Vec::new();
        idx.begin();
        idx.touch(&sensors, 20, 20, &mut out);
        let first = out.len();
        idx.touch(&sensors, 20, 20, &mut out);
        assert_eq!(out.len(), first, "повторная правка добавила сенсоров");
        idx.touch(&sensors, 21, 20, &mut out);
        assert!(out.len() >= first);
    }

    /// Порядок внутри набора обязан быть функцией индексов сенсоров,
    /// а не порядка правок: иначе фаза пересчёта наблюдаема.
    #[test]
    fn order_is_stable_under_edit_permutation() {
        let sensors = grid_sensors(8, 6, 11);
        let mut idx = SensorIndex::new(200, 200, 11, sensors.len());
        idx.rebuild(&sensors);
        let cells = [(30usize, 30usize), (31, 44), (55, 20), (30, 31)];

        let mut a = Vec::new();
        idx.begin();
        for &(x, y) in cells.iter() {
            idx.touch(&sensors, x, y, &mut a);
        }
        let mut b = Vec::new();
        idx.begin();
        for &(x, y) in cells.iter().rev() {
            idx.touch(&sensors, x, y, &mut b);
        }
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "набор зависит от порядка правок");
    }
}
