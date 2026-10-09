//! Derived rectangle bounds over the canonical sorted column slice.
//! No physical state, query approximation or ordering enters this tree.
use super::Rubble;

const LEAF: usize = 32;
const MIN_COLUMNS: usize = 512;

#[derive(Clone, Debug, Default)]
pub(super) struct Index(Vec<Node>);

#[derive(Clone, Debug, Default)]
struct Node {
    lo: [i32; 2],
    hi: [i32; 2],
    top: u16,
    start: usize,
    end: usize,
    children: Option<[usize; 2]>,
}
impl Node {
    fn outside(&self, lo: [i32; 2], hi: [i32; 2]) -> bool {
        (0..2).any(|a| self.hi[a] < lo[a] || self.lo[a] > hi[a])
    }
    fn inside(&self, lo: [i32; 2], hi: [i32; 2]) -> bool {
        (0..2).all(|a| self.lo[a] >= lo[a] && self.hi[a] <= hi[a])
    }
}
impl Index {
    pub(super) fn rebuild(&mut self, columns: &[([i32; 2], u16)]) {
        self.0.clear();
        if columns.len() > MIN_COLUMNS {
            self.build(columns, 0, columns.len());
        }
    }
    fn build(&mut self, columns: &[([i32; 2], u16)], start: usize, end: usize) -> usize {
        let index = self.0.len();
        self.0.push(Node::default());
        let mut node = Node {
            lo: [i32::MAX; 2],
            hi: [i32::MIN; 2],
            start,
            end,
            ..Node::default()
        };
        if end - start <= LEAF {
            for &(at, top) in &columns[start..end] {
                for a in 0..2 {
                    node.lo[a] = node.lo[a].min(at[a]);
                    node.hi[a] = node.hi[a].max(at[a]);
                }
                node.top = node.top.max(top);
            }
        } else {
            let middle = start + (end - start) / 2;
            let left = self.build(columns, start, middle);
            let right = self.build(columns, middle, end);
            for a in 0..2 {
                node.lo[a] = self.0[left].lo[a].min(self.0[right].lo[a]);
                node.hi[a] = self.0[left].hi[a].max(self.0[right].hi[a]);
            }
            node.top = self.0[left].top.max(self.0[right].top);
            node.children = Some([left, right]);
        }
        self.0[index] = node;
        index
    }
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub(super) fn upper(
        &self,
        columns: &[([i32; 2], u16)],
        lo: [i32; 2],
        hi: [i32; 2],
        examined: &mut impl FnMut(),
    ) -> u16 {
        let mut upper = 0;
        self.upper_node(0, columns, lo, hi, &mut upper, examined);
        upper
    }
    fn upper_node(
        &self,
        index: usize,
        columns: &[([i32; 2], u16)],
        lo: [i32; 2],
        hi: [i32; 2],
        upper: &mut u16,
        examined: &mut impl FnMut(),
    ) {
        let node = &self.0[index];
        if node.top <= *upper || node.outside(lo, hi) {
            return;
        }
        if node.inside(lo, hi) {
            *upper = node.top;
        } else if let Some([left, right]) = node.children {
            self.upper_node(left, columns, lo, hi, upper, examined);
            self.upper_node(right, columns, lo, hi, upper, examined);
        } else {
            for &(at, top) in &columns[node.start..node.end] {
                examined();
                if (0..2).all(|a| at[a] >= lo[a] && at[a] <= hi[a]) {
                    *upper = (*upper).max(top);
                }
            }
        }
    }
    pub(super) fn touches(&self, columns: &[([i32; 2], u16)], lo: [i32; 2], hi: [i32; 2]) -> bool {
        self.touches_node(0, columns, lo, hi)
    }
    fn touches_node(
        &self,
        index: usize,
        columns: &[([i32; 2], u16)],
        lo: [i32; 2],
        hi: [i32; 2],
    ) -> bool {
        let node = &self.0[index];
        if node.outside(lo, hi) {
            false
        } else if node.inside(lo, hi) {
            true
        } else if let Some([left, right]) = node.children {
            self.touches_node(left, columns, lo, hi) || self.touches_node(right, columns, lo, hi)
        } else {
            columns[node.start..node.end]
                .iter()
                .any(|(at, _)| (0..2).all(|a| at[a] >= lo[a] && at[a] <= hi[a]))
        }
    }
}

impl PartialEq for Rubble {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for Rubble {}
