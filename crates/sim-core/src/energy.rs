//! Сеть энергии (ADR-022): «энергия — поток, её нельзя отобрать, можно
//! только перерезать снабжение».
//!
//! Отсюда устройство: узлы (производители, потребители, проводники), связи,
//! и **связность решает всё** — потребитель получает энергию, только пока
//! связан с производителем. Значит обновление сети это компоненты связности
//! плюс две суммы на компоненту.
//!
//! Всё целочисленно. Суммы — `wrapping_add`: единственная операция,
//! безопасная для параллельной редукции (`CLAUDE.md`).

/// Узел сети. Производство и потребление в условных единицах за тик.
#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub produce: i32,
    pub demand: i32,
}

pub struct Network {
    pub nodes: Vec<Node>,
    /// Связи как пары индексов. Перерезанные не удаляются, а помечаются:
    /// удаление из плотного массива меняло бы соседей (`CLAUDE.md`).
    pub links: Vec<(u32, u32)>,
    pub live: Vec<bool>,

    parent: Vec<u32>,
    /// Метка компоненты на узел — канонична: корень это минимальный индекс.
    pub label: Vec<u32>,
    pub components: usize,
    /// Производство и спрос по компонентам.
    pub prod: Vec<i64>,
    pub want: Vec<i64>,
    /// Сколько досталось каждому узлу.
    pub delivered: Vec<i32>,
}

impl Network {
    pub fn new(nodes: Vec<Node>, links: Vec<(u32, u32)>) -> Network {
        let n = nodes.len();
        let e = links.len();
        Network {
            nodes,
            links,
            live: vec![true; e],
            parent: (0..n as u32).collect(),
            label: vec![0; n],
            components: 0,
            prod: Vec::new(),
            want: Vec::new(),
            delivered: vec![0; n],
        }
    }

    #[inline(always)]
    fn find(&mut self, mut a: u32) -> u32 {
        while self.parent[a as usize] != a {
            let g = self.parent[self.parent[a as usize] as usize];
            self.parent[a as usize] = g;
            a = g;
        }
        a
    }

    #[inline(always)]
    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        // Корнем становится меньший индекс: метка — функция графа,
        // а не порядка объединения (ловушка union by rank, ADR-055).
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        self.parent[hi as usize] = lo;
    }

    /// Полное обновление: компоненты, суммы, раздача.
    pub fn update(&mut self) {
        let n = self.nodes.len();
        for (i, p) in self.parent.iter_mut().enumerate() {
            *p = i as u32;
        }
        for k in 0..self.links.len() {
            if !self.live[k] {
                continue;
            }
            let (a, b) = self.links[k];
            self.union(a, b);
        }
        // Канонизация: корни перенумерованы по возрастанию.
        self.label.clear();
        self.label.resize(n, u32::MAX);
        let mut next = 0u32;
        let mut roots: Vec<u32> = vec![u32::MAX; n];
        for i in 0..n {
            let r = self.find(i as u32);
            if roots[r as usize] == u32::MAX {
                roots[r as usize] = next;
                next += 1;
            }
            self.label[i] = roots[r as usize];
        }
        self.components = next as usize;

        self.prod.clear();
        self.prod.resize(self.components, 0);
        self.want.clear();
        self.want.resize(self.components, 0);
        for i in 0..n {
            let c = self.label[i] as usize;
            self.prod[c] = self.prod[c].wrapping_add(self.nodes[i].produce as i64);
            self.want[c] = self.want[c].wrapping_add(self.nodes[i].demand as i64);
        }

        // Раздача: доля от спроса, целочисленно и независимо на узел.
        // Порядок обхода не наблюдаем — каждый узел считается из сумм
        // своей компоненты, а не из остатка.
        for i in 0..n {
            let c = self.label[i] as usize;
            let d = self.nodes[i].demand as i64;
            self.delivered[i] = if d == 0 {
                0
            } else if self.prod[c] >= self.want[c] {
                d as i32
            } else {
                ((d * self.prod[c]) / self.want[c].max(1)) as i32
            };
        }
    }

    /// Перерезать связь. Это и есть событие «перерезали снабжение».
    pub fn cut(&mut self, k: usize) {
        self.live[k] = false;
    }

    pub fn restore(&mut self, k: usize) {
        self.live[k] = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx::rng::Rng;

    fn chain(n: usize) -> Network {
        let nodes: Vec<Node> = (0..n)
            .map(|i| {
                if i == 0 {
                    Node { produce: 1000, demand: 0 }
                } else {
                    Node { produce: 0, demand: 10 }
                }
            })
            .collect();
        let links: Vec<(u32, u32)> = (1..n).map(|i| ((i - 1) as u32, i as u32)).collect();
        Network::new(nodes, links)
    }

    /// Пока цепь цела, всем хватает.
    #[test]
    fn intact_chain_feeds_everyone() {
        let mut net = chain(50);
        net.update();
        assert_eq!(net.components, 1);
        for i in 1..50 {
            assert_eq!(net.delivered[i], 10, "узел {i} голодает");
        }
    }

    /// **Перерезали снабжение — и половина осталась без энергии.**
    /// Это ровно то, ради чего в ADR-022 энергия сделана потоком.
    #[test]
    fn cutting_a_link_starves_the_far_half() {
        let mut net = chain(50);
        net.cut(24);
        net.update();
        assert_eq!(net.components, 2);
        for i in 1..25 {
            assert_eq!(net.delivered[i], 10, "ближняя половина голодает: {i}");
        }
        for i in 25..50 {
            assert_eq!(net.delivered[i], 0, "дальняя половина всё ещё питается: {i}");
        }
        net.restore(24);
        net.update();
        assert_eq!(net.components, 1);
        assert_eq!(net.delivered[49], 10);
    }

    /// Нехватка делится пропорционально спросу, а не по порядку обхода.
    #[test]
    fn shortage_is_shared_by_demand() {
        let nodes = vec![
            Node { produce: 100, demand: 0 },
            Node { produce: 0, demand: 100 },
            Node { produce: 0, demand: 300 },
        ];
        let mut net = Network::new(nodes, vec![(0, 1), (1, 2)]);
        net.update();
        // Спрос 400, производство 100: каждому четверть.
        assert_eq!(net.delivered[1], 25);
        assert_eq!(net.delivered[2], 75);
    }

    /// Метка компоненты канонична: она не зависит от порядка связей.
    #[test]
    fn labels_do_not_depend_on_link_order() {
        let mut r = Rng::new(6, 6);
        let n = 200usize;
        let nodes: Vec<Node> = (0..n).map(|_| Node { produce: 1, demand: 1 }).collect();
        let mut links: Vec<(u32, u32)> = (0..400)
            .map(|_| (r.below(n as u32), r.below(n as u32)))
            .collect();
        let mut a = Network::new(nodes.clone(), links.clone());
        a.update();
        links.reverse();
        let mut b = Network::new(nodes, links);
        b.update();
        assert_eq!(a.label, b.label, "метка зависит от порядка связей");
        assert_eq!(a.components, b.components);
    }
}
