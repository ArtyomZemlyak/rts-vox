//! Хеш-словарь, по которому **нельзя пройти в хеш-порядке** (`CLAUDE.md`:
//! никакого `HashMap` в итерируемых путях).
//!
//! Симуляции нужны быстрые словари: множество достигнутых клеток поиска
//! пути, маски запросов к осевшим обломкам, ключи слияния куч. Сами по себе
//! они безопасны — опасен обход: порядок хеш-таблицы зависит от хешера
//! и истории вставок, и цикл по ней молча меняет исход ничьей.
//!
//! Поэтому обёртка отдаёт только запросы — `get`, `insert`, `entry`,
//! `remove`, `contains_key` — и упорядоченные выборки, которые сортируют
//! ключи перед выдачей. Единственный неупорядоченный выход —
//! [`Lookup::drain_values_unordered`]: он для переработки буферов, чьё
//! содержимое тут же очищается, и порядок которых ни на что не влияет.
//!
//! Это единственный файл sim-крейтов, которому архитектурный тест
//! разрешает `HashMap` (анализ 239, P-74).

use std::collections::hash_map::{Entry, HashMap, RandomState};
use std::hash::{BuildHasher, Hash};

#[derive(Clone, Debug)]
pub struct Lookup<K, V, S = RandomState>(HashMap<K, V, S>);

impl<K, V, S: Default> Default for Lookup<K, V, S> {
    fn default() -> Self {
        Self(HashMap::with_hasher(S::default()))
    }
}

impl<K: Eq + Hash, V> Lookup<K, V, RandomState> {
    pub fn new() -> Self {
        Self(HashMap::new())
    }
}

impl<K: Eq + Hash, V, S: BuildHasher + Default> Lookup<K, V, S> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self(HashMap::with_capacity_and_hasher(capacity, S::default()))
    }
}

impl<K: Eq + Hash, V, S: BuildHasher> Lookup<K, V, S> {
    pub fn get(&self, key: &K) -> Option<&V> {
        self.0.get(key)
    }
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.0.get_mut(key)
    }
    pub fn contains_key(&self, key: &K) -> bool {
        self.0.contains_key(key)
    }
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.0.insert(key, value)
    }
    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.0.remove(key)
    }
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        self.0.entry(key)
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
    /// Ключи по возрастанию: обход, не зависящий от хешера.
    pub fn sorted_keys(&self) -> Vec<K>
    where
        K: Ord + Clone,
    {
        let mut keys: Vec<K> = self.0.keys().cloned().collect();
        keys.sort_unstable();
        keys
    }
    /// Пары по возрастанию ключа. Ключи уникальны, поэтому нестабильная
    /// сортировка здесь однозначна.
    pub fn sorted_entries(&self) -> Vec<(&K, &V)>
    where
        K: Ord,
    {
        let mut out: Vec<(&K, &V)> = self.0.iter().collect();
        out.sort_unstable_by(|a, b| a.0.cmp(b.0));
        out
    }
    /// Удалить записи, чей ключ удовлетворяет предикату. Предикат —
    /// `Fn` от одного ключа: он не может копить состояние между вызовами,
    /// поэтому порядок, в котором таблица их перебирает, не наблюдаем.
    pub fn remove_where(&mut self, stale: impl Fn(&K) -> bool) {
        self.0.retain(|k, _| !stale(k));
    }
    /// Забрать значения **в неопределённом порядке**, опустошив словарь.
    /// Только для переработки буферов, содержимое которых сразу
    /// очищается: порядок выдачи не должен влиять ни на что, кроме
    /// повторного использования памяти.
    pub fn drain_values_unordered(&mut self) -> impl Iterator<Item = V> + '_ {
        self.0.drain().map(|(_, v)| v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_views_do_not_depend_on_insertion_order() {
        let mut a: Lookup<u32, u32> = Lookup::new();
        let mut b: Lookup<u32, u32> = Lookup::with_capacity(64);
        for k in [9u32, 2, 7, 5, 1] {
            a.insert(k, k * 10);
        }
        for k in [1u32, 5, 7, 2, 9] {
            b.insert(k, k * 10);
        }
        assert_eq!(a.sorted_keys(), vec![1, 2, 5, 7, 9]);
        assert_eq!(a.sorted_keys(), b.sorted_keys());
        assert_eq!(a.sorted_entries(), b.sorted_entries());
        *a.entry(3).or_insert(0) += 4;
        assert_eq!(a.get(&3), Some(&4));
        assert_eq!(a.remove(&3), Some(4));
        let mut drained: Vec<u32> = a.drain_values_unordered().collect();
        drained.sort_unstable();
        assert_eq!(drained, vec![10, 20, 50, 70, 90]);
        assert!(a.is_empty());
    }
}
