//! Static broad phase for authored range geometry. Candidate order is the
//! original data order, so overlapping solids and air cutouts keep last-writer semantics.
use super::ROAD_GRADE_SHOULDER;
use modding::range::RangeLayout;
const TILE: usize = 128;
#[derive(Clone, Default)]
pub(super) struct Bin {
    pub foundations: Vec<usize>,
    pub blocks: Vec<usize>,
    pub zones: Vec<usize>,
    pub ramps: Vec<usize>,
    /// Road and segment indices, both in authored order. A nearby segment
    /// must not bring the rest of a kilometre-long road into every query.
    pub roads: Vec<(usize, usize)>,
}
pub(super) struct Index {
    pub foundations: Vec<modding::range::RangeBlock>,
    nx: usize,
    bins: Vec<Bin>,
    empty: Bin,
}
impl Index {
    pub fn new(layout: Option<&RangeLayout>) -> Self {
        let size = layout.map_or(modding::range::stock_size(), |r| r.size);
        let nx = (size[0] as usize).div_ceil(TILE);
        let nz = (size[2] as usize).div_ceil(TILE);
        let mut out = Self {
            foundations: layout.map_or_else(Vec::new, super::foundation::blocks),
            nx,
            bins: vec![Bin::default(); nx * nz],
            empty: Bin::default(),
        };
        for i in 0..out.foundations.len() {
            let f = &out.foundations[i];
            out.insert([f.min[0], f.min[2]], [f.max[0], f.max[2]], |bin| {
                bin.foundations.push(i)
            });
        }
        if let Some(r) = layout {
            for (i, b) in r.blocks.iter().enumerate() {
                out.insert([b.min[0], b.min[2]], [b.max[0], b.max[2]], |bin| {
                    bin.blocks.push(i)
                });
            }
            for (i, z) in r.zones.iter().enumerate() {
                out.insert(z.min, z.max, |bin| bin.zones.push(i));
            }
            for (i, road) in r.roads.iter().enumerate() {
                for (segment_id, segment) in road.points.windows(2).enumerate() {
                    let pad = road.width / 2 + ROAD_GRADE_SHOULDER;
                    let lo = [
                        segment[0][0].min(segment[1][0]) - pad,
                        segment[0][1].min(segment[1][1]) - pad,
                    ];
                    let hi = [
                        segment[0][0].max(segment[1][0]) + pad + 1,
                        segment[0][1].max(segment[1][1]) + pad + 1,
                    ];
                    out.insert(lo, hi, |bin| bin.roads.push((i, segment_id)));
                }
            }
            if let Some(c) = &r.course {
                for (i, b) in c.ramps.iter().enumerate() {
                    out.insert([b.min[0], b.min[2]], [b.max[0], b.max[2]], |bin| {
                        bin.ramps.push(i)
                    });
                }
            }
        }
        out
    }
    fn insert(&mut self, lo: [i32; 2], hi: [i32; 2], mut put: impl FnMut(&mut Bin)) {
        // Layout validation precedes construction. Clamp defensively for reference tests.
        for z in lo[1].max(0) as usize / TILE..=(hi[1] - 1).max(0) as usize / TILE {
            for x in lo[0].max(0) as usize / TILE..=(hi[0] - 1).max(0) as usize / TILE {
                if x < self.nx {
                    if let Some(bin) = self.bins.get_mut(z * self.nx + x) {
                        put(bin);
                    }
                }
            }
        }
    }
    pub fn at(&self, x: u32, z: u32) -> &Bin {
        let x = x as usize / TILE;
        if x >= self.nx {
            return &self.empty;
        }
        self.bins
            .get(z as usize / TILE * self.nx + x)
            .unwrap_or(&self.empty)
    }
}
