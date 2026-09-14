//! Oriented vehicle clearance and routes against the current voxel surface.
//! The same footprint query serves planning and accepted movement. This
//! surface adapter does not replace the multi-level navigation graph.
use fx::{angle, Fx};
use std::{cmp::Reverse, collections::BinaryHeap};
use voxel::World;

#[derive(Clone, Copy)]
pub struct Shape {
    pub half: [i32; 2],
    pub support: [i32; 2],
    pub clearance: Fx,
    pub step: i32,
    pub min_up: Fx,
}
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub height: Fx,
    pub floors: [Fx; 4],
    pub blocked: usize,
}
/// Query-local height cache. No cached result survives a world edit.
pub struct Probe<'a> {
    world: &'a World,
    heights: Vec<u16>,
}
impl<'a> Probe<'a> {
    pub fn new(world: &'a World) -> Self {
        Self {
            world,
            heights: vec![u16::MAX; (world.nx * world.nz) as usize],
        }
    }
    pub fn floor(&mut self, x: i32, z: i32) -> i32 {
        if x < 0 || z < 0 || x >= self.world.nx as i32 || z >= self.world.nz as i32 {
            return self.world.ny as i32;
        }
        let i = (x as u32 + self.world.nx * z as u32) as usize;
        if self.heights[i] == u16::MAX {
            self.heights[i] = (0..self.world.ny)
                .rev()
                .find(|y| self.world.solid(x as u32, *y, z as u32))
                .map_or(0, |y| y as u16 + 1);
        }
        self.heights[i] as i32
    }
    fn surface(&mut self, p: [Fx; 2]) -> Fx {
        // A support has a finite contact patch. Blend the four neighbouring
        // columns instead of snapping its load at each integer boundary.
        // Hull obstacle checks below still use the exact voxel surface.
        let x = p[0].to_int_trunc();
        let z = p[1].to_int_trunc();
        let u = p[0].sub(Fx::from_int(x));
        let v = p[1].sub(Fx::from_int(z));
        let h00 = Fx::from_int(self.floor(x, z));
        let h10 = Fx::from_int(self.floor(x + 1, z));
        let h01 = Fx::from_int(self.floor(x, z + 1));
        let h11 = Fx::from_int(self.floor(x + 1, z + 1));
        let a = h00.add(h10.sub(h00).mul(u));
        let b = h01.add(h11.sub(h01).mul(u));
        a.add(b.sub(a).mul(v))
    }
    fn contact(&mut self, p: [Fx; 2], radius: Fx) -> Fx {
        let mut height = self.surface(p);
        for (x, z) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
            height = height.max(self.surface([
                p[0].add(radius.mul(Fx::from_int(x))),
                p[1].add(radius.mul(Fx::from_int(z))),
            ]));
        }
        height
    }
    pub fn pose(&mut self, pos: [Fx; 2], head: i32, s: Shape) -> Pose {
        let (c, sin) = (angle::cos(head), angle::sin(head));
        let point = |x: i32, z: i32| {
            [
                pos[0]
                    .add(c.mul(Fx::from_int(x)))
                    .sub(sin.mul(Fx::from_int(z))),
                pos[1]
                    .add(sin.mul(Fx::from_int(x)))
                    .add(c.mul(Fx::from_int(z))),
            ]
        };
        let radius = Fx::from_int(
            (s.half[0] - s.support[0])
                .min(s.half[1] - s.support[1])
                .min(s.step)
                .max(1),
        );
        let floors = [(-1, -1), (1, -1), (-1, 1), (1, 1)].map(|(x, z)| {
            let p = point(x * s.support[0], z * s.support[1]);
            self.contact(p, radius)
        });
        let height = floors
            .iter()
            .fold(Fx::ZERO, |a, b| a.add(*b))
            .div(Fx::from_int(4));
        let pitch = floors[1]
            .add(floors[3])
            .sub(floors[0])
            .sub(floors[2])
            .div(Fx::from_int(4 * s.support[0].max(1)));
        let roll = floors[2]
            .add(floors[3])
            .sub(floors[0])
            .sub(floors[1])
            .div(Fx::from_int(4 * s.support[1].max(1)));
        let mut blocked = if s
            .min_up
            .mul(s.min_up)
            .mul(Fx::ONE.add(pitch.mul(pitch)).add(roll.mul(roll)))
            > Fx::ONE
        {
            1_000_000
        } else {
            0
        };
        let ext = [
            c.abs()
                .mul(Fx::from_int(s.half[0]))
                .add(sin.abs().mul(Fx::from_int(s.half[1]))),
            sin.abs()
                .mul(Fx::from_int(s.half[0]))
                .add(c.abs().mul(Fx::from_int(s.half[1]))),
        ];
        let lo = [
            pos[0].sub(ext[0]).to_int_trunc() - 1,
            pos[1].sub(ext[1]).to_int_trunc() - 1,
        ];
        let hi = [
            pos[0].add(ext[0]).to_int_trunc() + 1,
            pos[1].add(ext[1]).to_int_trunc() + 1,
        ];
        for z in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                let dx = Fx::from_int(x).add(Fx::from_ratio(1, 2)).sub(pos[0]);
                let dz = Fx::from_int(z).add(Fx::from_ratio(1, 2)).sub(pos[1]);
                let u = dx.mul(c).add(dz.mul(sin));
                let v = dz.mul(c).sub(dx.mul(sin));
                // Include a half-cell halo so a one-voxel rock cannot hide between
                // the four nominal supports or between diagonally sampled cells.
                if u.abs() > Fx::from_int(s.half[0]).add(Fx::from_ratio(1, 2))
                    || v.abs() > Fx::from_int(s.half[1]).add(Fx::from_ratio(1, 2))
                {
                    continue;
                }
                let ground = Fx::from_int(self.floor(x, z));
                let belly = height
                    .add(pitch.mul(u))
                    .add(roll.mul(v))
                    .add(s.clearance.max(Fx::from_int(s.step)));
                if ground > belly {
                    blocked += 1;
                }
            }
        }
        Pose {
            height,
            floors,
            blocked,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Search {
    pub cell: i32,
    pub min: [i32; 2],
    pub max: [i32; 2],
    pub nodes: usize,
}
const DIR: [[i32; 2]; 8] = [
    [1, 0],
    [1, 1],
    [0, 1],
    [-1, 1],
    [-1, 0],
    [-1, -1],
    [0, -1],
    [1, -1],
];
fn connects(
    ground: &mut Probe<'_>,
    shape: Shape,
    from: [Fx; 2],
    head: i32,
    to: [Fx; 2],
    clear: &impl Fn([Fx; 2], i32) -> bool,
) -> bool {
    let delta = [to[0].sub(from[0]), to[1].sub(from[1])];
    let yaw = if delta == [Fx::ZERO; 2] {
        head
    } else {
        angle::atan2(delta[1].raw(), delta[0].raw())
    };
    let turn = angle::diff(yaw, head);
    let turns = (turn.abs() / (angle::FULL / 16) + 1).max(1);
    let mut old = ground.pose(from, head, shape);
    for i in 1..=turns {
        let h = angle::norm(head + turn * i / turns);
        let next = ground.pose(from, h, shape);
        if next.blocked != 0
            || next.height.sub(old.height).abs() > Fx::from_int(shape.step)
            || !clear(from, h)
        {
            return false;
        }
        old = next;
    }
    let steps = (delta[0].abs().max(delta[1].abs()).to_int_trunc() + 1).max(1);
    for i in 1..=steps {
        let f = Fx::from_ratio(i, steps);
        let p = [from[0].add(delta[0].mul(f)), from[1].add(delta[1].mul(f))];
        let next = ground.pose(p, yaw, shape);
        if next.blocked != 0
            || next.height.sub(old.height).abs() > Fx::from_int(shape.step)
            || !clear(p, yaw)
        {
            return false;
        }
        old = next;
    }
    true
}
/// An oriented A*: translations keep heading, turns reserve their swept
/// footprint. It cannot choose a narrow corridor and then rotate through it.
pub fn route(
    mut ground: Probe<'_>,
    shape: Shape,
    from: [Fx; 2],
    head: i32,
    to: [i32; 2],
    cfg: Search,
    clear: impl Fn([Fx; 2], i32) -> bool,
) -> Vec<[i32; 2]> {
    if cfg.cell <= 0
        || cfg.nodes == 0
        || (0..2).any(|a| {
            to[a] < cfg.min[a]
                || to[a] >= cfg.max[a]
                || from[a] < Fx::ZERO
                || from[a] >= Fx::from_int([ground.world.nx, ground.world.nz][a] as i32)
        })
        || ground.world.nx as i32 % cfg.cell != 0
        || ground.world.nz as i32 % cfg.cell != 0
    {
        return vec![];
    }
    if connects(&mut ground, shape, from, head, to.map(Fx::from_int), &clear) {
        return vec![to];
    }
    let n = (ground.world.nx as i32).div_euclid(cfg.cell) as usize;
    let rows = (ground.world.nz as i32).div_euclid(cfg.cell) as usize;
    let start = from[0].to_int_trunc().div_euclid(cfg.cell) as usize
        + n * from[1].to_int_trunc().div_euclid(cfg.cell) as usize;
    let anchor = [
        from[0].to_int_trunc().rem_euclid(cfg.cell),
        from[1].to_int_trunc().rem_euclid(cfg.cell),
    ];
    let end = (to[0] - anchor[0]).div_euclid(cfg.cell) as usize
        + n * (to[1] - anchor[1]).div_euclid(cfg.cell) as usize;
    let point = |i: usize| {
        [
            Fx::from_int((i % n) as i32 * cfg.cell + anchor[0]),
            Fx::from_int((i / n) as i32 * cfg.cell + anchor[1]),
        ]
    };
    let start_dir = (angle::norm(head + angle::FULL / 16) / (angle::FULL / 8)) as usize % 8;
    let initial = start * 8 + start_dir;
    let mut distance = vec![u32::MAX; n * rows * 8];
    let mut previous = vec![usize::MAX; distance.len()];
    let mut poses = vec![None; distance.len()];
    let mut open = BinaryHeap::new();
    distance[initial] = 0;
    open.push(Reverse((0, 0, initial)));
    let mut found = None;
    for _ in 0..cfg.nodes {
        let Some(Reverse((_, cost, i))) = open.pop() else {
            break;
        };
        if cost != distance[i] {
            continue;
        }
        let at = point(i / 8);
        let direction = i % 8;
        let yaw = direction as i32 * angle::FULL / 8;
        let pose = *poses[i].get_or_insert_with(|| ground.pose(at, yaw, shape));
        if ((at[0].to_int_trunc() - to[0])
            .abs()
            .max((at[1].to_int_trunc() - to[1]).abs())
            <= cfg.cell * 4
            || i / 8 == end)
            && pose.blocked == 0
            && clear(at, yaw)
            && connects(&mut ground, shape, at, yaw, to.map(Fx::from_int), &clear)
        {
            found = Some(i);
            break;
        }
        for turn in [-1, 0, 1] {
            let d = (direction as i32 + turn).rem_euclid(8) as usize;
            let next_yaw = d as i32 * angle::FULL / 8;
            let cell = if turn == 0 {
                let x = (i / 8 % n) as i32 + DIR[d][0];
                let z = (i / 8 / n) as i32 + DIR[d][1];
                if x < 0 || z < 0 || x >= n as i32 || z >= rows as i32 {
                    continue;
                }
                x as usize + n * z as usize
            } else {
                i / 8
            };
            let q = point(cell);
            if (0..2).any(|a| q[a] < Fx::from_int(cfg.min[a]) || q[a] >= Fx::from_int(cfg.max[a])) {
                continue;
            }
            let j = cell * 8 + d;
            let next_cost = cost
                + if turn == 0 {
                    if d.is_multiple_of(2) {
                        10
                    } else {
                        14
                    }
                } else {
                    6
                };
            if next_cost >= distance[j] {
                continue;
            }
            let next = *poses[j].get_or_insert_with(|| ground.pose(q, next_yaw, shape));
            if next.blocked != 0
                || !clear(q, next_yaw)
                || next.height.sub(pose.height).abs() > Fx::from_int(shape.step)
            {
                continue;
            }
            let mid = [
                at[0].add(q[0]).div(Fx::from_int(2)),
                at[1].add(q[1]).div(Fx::from_int(2)),
            ];
            let mid_yaw = angle::norm(yaw + angle::diff(next_yaw, yaw) / 2);
            if ground.pose(mid, mid_yaw, shape).blocked != 0 || !clear(mid, mid_yaw) {
                continue;
            }
            distance[j] = next_cost;
            previous[j] = i;
            let heuristic = q[0]
                .sub(Fx::from_int(to[0]))
                .abs()
                .max(q[1].sub(Fx::from_int(to[1])).abs())
                .to_int_trunc()
                / cfg.cell
                * 10;
            open.push(Reverse((next_cost + heuristic as u32, next_cost, j)));
        }
    }
    let Some(mut i) = found else {
        return vec![];
    };
    let mut all = vec![];
    while i != initial {
        let p = point(i / 8).map(Fx::to_int_trunc);
        if all.last() != Some(&p) {
            all.push(p);
        }
        i = previous[i];
    }
    all.reverse();
    if all.last() != Some(&to) {
        all.push(to);
    }
    // Remove collinear grid points while retaining the exact turn centres.
    let mut out = vec![];
    let mut before = from.map(Fx::to_int_trunc);
    for (i, p) in all.iter().enumerate() {
        let keep = i + 1 == all.len() || {
            let q = all[i + 1];
            (p[0] - before[0]) * (q[1] - p[1]) != (p[1] - before[1]) * (q[0] - p[0])
        };
        if keep {
            out.push(*p);
        }
        before = *p;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestGround {
        rock: bool,
        corridor: bool,
        hill: bool,
    }
    impl voxel::Generator for TestGround {
        fn material(&self, x: u32, y: u32, z: u32) -> u16 {
            let height = if self.hill {
                16 + (12 - (x as i32 - 128).abs() / 6).max(0) as u32
            } else {
                16
            };
            if y < height
                || (self.rock && x == 90 && z == 72 && y < 30)
                || (self.corridor && (58..=82).contains(&x) && !(57..84).contains(&z) && y < 55)
            {
                1
            } else {
                0
            }
        }
    }
    fn world(rock: bool, corridor: bool, hill: bool) -> World {
        World::new(
            256,
            64,
            256,
            100,
            Box::new(TestGround {
                rock,
                corridor,
                hill,
            }),
        )
    }
    fn tank() -> Shape {
        Shape {
            half: [30, 12],
            support: [25, 10],
            clearance: Fx::from_ratio(45, 10),
            step: 4,
            min_up: Fx::from_ratio(91, 100),
        }
    }
    #[test]
    fn narrow_opening_uses_width_and_forbids_a_turn_inside_it() {
        let w = world(false, true, false);
        let mut p = Probe::new(&w);
        let at = [Fx::from_int(70); 2];
        assert_eq!(p.pose(at, 0, tank()).blocked, 0);
        assert!(p.pose(at, angle::FULL / 8, tank()).blocked > 0);
        let route = route(
            Probe::new(&w),
            tank(),
            [Fx::from_int(40), Fx::from_int(70)],
            0,
            [200, 70],
            Search {
                cell: 8,
                min: [32; 2],
                max: [224; 2],
                nodes: 6400,
            },
            |_, _| true,
        );
        assert!(!route.is_empty());
        assert_eq!(route.last(), Some(&[200, 70]));
    }
    #[test]
    fn a_single_voxel_obstacle_between_supports_blocks_the_full_hull() {
        let mut w = world(true, false, false);
        let mut p = Probe::new(&w);
        let at = [Fx::from_int(80), Fx::from_int(70)];
        let pose = p.pose(at, 0, tank());
        assert_eq!(pose.floors, [Fx::from_int(16); 4]);
        assert!(pose.blocked > 0);
        for y in 16..30 {
            w.queue_dig(90, y, 72);
        }
        w.flush();
        assert_eq!(Probe::new(&w).pose(at, 0, tank()).blocked, 0);
    }
    #[test]
    fn rounded_crest_and_gradual_slopes_fit_the_vehicle_clearance() {
        let w = world(false, false, true);
        let mut p = Probe::new(&w);
        for x in 90..168 {
            let at = [Fx::from_int(x), Fx::from_int(128)];
            assert_eq!(p.pose(at, 0, tank()).blocked, 0, "crest at {x}");
            assert_eq!(
                p.pose(at, angle::FULL / 2, tank()).blocked,
                0,
                "reverse crest at {x}"
            );
        }
    }
}
