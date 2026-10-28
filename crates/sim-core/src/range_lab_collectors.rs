//! Laboratory salvage crawler. Real debris -> finite hauler -> refining depot.
//! The vehicle planner, material ledger and fragment retirement are shared.
use super::BlastRange;
use crate::matter::Kind;
use fx::Fx;
use modding::lab_tools::{Collector, Device, DeviceKind, Tuning};
use std::collections::{BTreeMap, BTreeSet};

fn square(a: [i32; 3], b: [i32; 3]) -> i64 {
    (0..3)
        .map(|k| {
            let d = i64::from(a[k]) - i64::from(b[k]);
            d * d
        })
        .sum()
}
fn point(f: &crate::burst::Fragment) -> [i32; 3] {
    [f.position.x, f.position.y, f.position.z].map(Fx::to_int_trunc)
}
fn forget(c: &mut Collector) {
    c.target = None;
    c.aim = None;
    c.spent = 0;
    c.route.clear();
    c.waypoint = 0;
}
fn shape(t: Tuning) -> nav::vehicle::Shape {
    nav::vehicle::Shape {
        half: [t.collector_half; 2],
        support: [t.collector_half - 1; 2],
        clearance: Fx::ZERO,
        step: t.collector_step,
        min_up: Fx::from_ratio(3, 4),
        height: t.collector_height,
        span: t.collector_half,
        ramp: Fx::from_ratio(1, 2),
    }
}
fn travel(
    world: &voxel::World,
    units: &[super::RangeUnit],
    at: &mut [i32; 3],
    turns: &mut u8,
    c: &mut Collector,
    goal: [i32; 3],
    tick: u32,
    t: Tuning,
    plans: &mut usize,
) -> bool {
    if tick % t.collector_move_ticks != 0 {
        return true;
    }
    let from = [at[0], at[2]];
    let to = [goal[0], goal[2]];
    if from == to {
        return true;
    }
    let clear = |p: [Fx; 2], _: i32| {
        let p = p.map(Fx::to_int_trunc);
        !units.iter().any(|u| {
            let (lo, hi) = u.bounds();
            u.status.alive
                && p[0] + t.collector_half >= lo[0]
                && p[0] - t.collector_half <= hi[0]
                && p[1] + t.collector_half >= lo[2]
                && p[1] - t.collector_half <= hi[2]
        })
    };
    while c.route.get(c.waypoint) == Some(&from) {
        c.waypoint += 1;
        c.leg_start = from;
        c.leg_step = 0;
    }
    if c.waypoint >= c.route.len() {
        // Open ground needs no A*. A single checked local step also lets a
        // vehicle leave an awkward graph-cell boundary beside a fresh crater.
        let next = [
            from[0] + (to[0] - from[0]).signum(),
            from[1] + (to[1] - from[1]).signum(),
        ];
        let p = next.map(Fx::from_int);
        let pose =
            nav::vehicle::Probe::around(world, &[], p, t.collector_half * 2).pose(p, 0, shape(t));
        let floor = pose.height.to_int_trunc();
        if !pose.refused() && (floor - at[1]).abs() <= t.collector_step && clear(p, 0) {
            let dx = next[0] - at[0];
            let dz = next[1] - at[2];
            *at = [next[0], floor, next[1]];
            *turns = if dx != 0 {
                if dx > 0 {
                    0
                } else {
                    2
                }
            } else if dz > 0 {
                1
            } else {
                3
            };
            return true;
        }
        if *plans == 0 || tick < c.plan_tick {
            return true;
        }
        *plans -= 1;
        let cfg = nav::vehicle::Search {
            cell: (t.collector_half as u32).next_power_of_two() as i32,
            min: [t.collector_half + 1; 2],
            max: [
                world.nx as i32 - t.collector_half - 1,
                world.nz as i32 - t.collector_half - 1,
            ],
            nodes: t.collector_nodes,
            crush_cost: 0,
            crush_cap: 0,
            margin: 0,
            heuristic_pct: 150,
        };
        let p = from.map(Fx::from_int);
        let answer = nav::vehicle::plan(
            nav::vehicle::Probe::around(world, &[], p, t.collector_radius),
            shape(t),
            p,
            0,
            to,
            cfg,
            clear,
        );
        c.route = answer.path;
        c.waypoint = 0;
        c.leg_start = from;
        c.leg_step = 0;
        c.plan_tick = tick.saturating_add(t.collector_retry_ticks);
        while c.route.get(c.waypoint) == Some(&from) {
            c.waypoint += 1;
        }
        if c.waypoint >= c.route.len() {
            return false;
        }
    }
    let next = c.route[c.waypoint];
    // One world-cell move, checked against today's terrain, not a cached path.
    let dx = next[0] - c.leg_start[0];
    let dz = next[1] - c.leg_start[1];
    let den = dx.abs().max(dz.abs()).max(1);
    c.leg_step = (c.leg_step + 1).min(den as u32);
    let offset = |d: i32| (i64::from(d) * i64::from(c.leg_step) / i64::from(den)) as i32;
    let mut proposed = *at;
    proposed[0] = c.leg_start[0] + offset(dx);
    proposed[2] = c.leg_start[1] + offset(dz);
    let p = [proposed[0], proposed[2]].map(Fx::from_int);
    let mut probe = nav::vehicle::Probe::around(world, &[], p, t.collector_half * 2);
    let pose = probe.pose(p, 0, shape(t));
    let floor = pose.height.to_int_trunc();
    if pose.refused() || (floor - at[1]).abs() > t.collector_step || !clear(p, 0) {
        c.route.clear();
        c.waypoint = 0;
        return false;
    }
    proposed[1] = floor;
    *at = proposed;
    *turns = if dx.abs() >= dz.abs() {
        if dx > 0 {
            0
        } else {
            2
        }
    } else if dz > 0 {
        1
    } else {
        3
    };
    true
}
impl BlastRange {
    pub(super) fn step_lab_collectors(&mut self) {
        let Some(layout) = self.layout.as_ref().filter(|l| l.laboratory) else {
            return;
        };
        if !layout
            .lab_devices
            .iter()
            .any(|d| d.kind == DeviceKind::Collector)
        {
            return;
        }
        let mut claimed: BTreeMap<u64, usize> = layout
            .lab_devices
            .iter()
            .enumerate()
            .filter_map(|(i, d)| d.collector.as_ref()?.target.map(|id| (id, i)))
            .collect();
        let mined: BTreeSet<_> = self
            .mining()
            .and_then(|m| m.reclaim())
            .into_iter()
            .flat_map(|r| (0..self.units.len()).filter_map(|i| r.piece_work(i).map(|p| p.0)))
            .collect();
        let t = self.lab_tools_tuning;
        // One shared ID lookup; keep fragment slots stable until all collectors finish.
        let mut ids: Vec<_> = self
            .matter
            .debris
            .fragments
            .iter()
            .enumerate()
            .map(|(i, f)| (f.id, i))
            .collect();
        ids.sort_unstable_by_key(|v| v.0);
        let locate = |id| {
            ids.binary_search_by_key(&id, |v| v.0)
                .ok()
                .map(|i| ids[i].1)
        };
        let mut taken = BTreeSet::new();
        let mut scans = t.collector_scans;
        let mut plans = t.collector_plans;
        let count = layout.lab_devices.len();
        let devices = &mut self.layout.as_mut().unwrap().lab_devices;
        for n in 0..count {
            let i = (self.tick as usize + n) % count;
            let Device {
                kind,
                at,
                turns,
                container,
                collector,
                dug,
                ..
            } = &mut devices[i];
            if *kind != DeviceKind::Collector {
                continue;
            }
            let Some(c) = collector else {
                continue;
            };
            let e = &mut self.matter.economy;
            let tank =
                *container.get_or_insert_with(|| e.ledger.create(Kind::Hauler, 0, c.capacity));
            let depot = *c
                .depot
                .get_or_insert_with(|| e.ledger.create(Kind::Depot, 0, t.collector_depot));
            let loaded = e.ledger.held(tank);
            let room = e.ledger.room(tank);
            if room == 0 {
                c.returning = true;
            }
            if c.returning {
                if let Some(id) = c.target {
                    if claimed.get(&id) == Some(&i) {
                        claimed.remove(&id);
                    }
                }
                if c.target.is_some() {
                    forget(c);
                }
                if square(*at, c.dock) <= i64::from(t.collector_reach).pow(2) {
                    // Size the input from destination room: a full depot cannot destroy cargo.
                    let most = (u128::from(e.ledger.room(depot))
                        * u128::from(t.collector_refine_den)
                        / u128::from(t.collector_refine_num))
                    .min(u128::from(u64::MAX)) as u64;
                    let want = loaded.min(most).min(t.collector_unload);
                    if want > 0
                        && (want == loaded
                            || want.saturating_mul(t.collector_refine_num)
                                >= t.collector_refine_den)
                    {
                        c.delivered += e.ledger.refine(
                            tank,
                            depot,
                            want,
                            t.collector_refine_num,
                            t.collector_refine_den,
                        );
                    }
                    if e.ledger.held(tank) == 0 {
                        c.returning = false;
                        c.trips += 1;
                        c.searched = 0;
                        c.route.clear();
                        c.waypoint = 0;
                    }
                } else {
                    travel(
                        &self.world,
                        &self.units,
                        at,
                        turns,
                        c,
                        c.dock,
                        self.tick,
                        t,
                        &mut plans,
                    );
                }
                continue;
            }
            if let Some(id) = c.target {
                let valid = locate(id).is_some_and(|slot| {
                    let f = &self.matter.debris.fragments[slot];
                    f.resting && e.ledger.held(f.held) > 0
                }) && !mined.contains(&id)
                    && claimed.get(&id) == Some(&i);
                if !valid {
                    if claimed.get(&id) == Some(&i) {
                        claimed.remove(&id);
                    }
                    forget(c);
                }
            }
            if c.target.is_none() && scans > 0 {
                let fragments = &self.matter.debris.fragments;
                let len = fragments.len();
                let mut best = None;
                for _ in 0..len.min(scans) {
                    let f = &fragments[c.cursor % len];
                    c.cursor = (c.cursor % len + 1) % len;
                    c.searched = c.searched.saturating_add(1);
                    scans -= 1;
                    let p = point(f);
                    let radius = (i64::from(p[0]) - i64::from(c.area[0])).pow(2)
                        + (i64::from(p[2]) - i64::from(c.area[2])).pow(2);
                    let amount = e.ledger.held(f.held);
                    if !f.resting
                        || amount == 0
                        || amount > c.capacity
                        || radius > i64::from(t.collector_radius).pow(2)
                        || claimed.contains_key(&f.id)
                        || mined.contains(&f.id)
                        || c.rejected
                            .is_some_and(|(id, until)| id == f.id && self.tick < until)
                    {
                        continue;
                    }
                    let candidate = (square(*at, p), f.id);
                    if best.is_none_or(|old| candidate < old) {
                        best = Some(candidate);
                    }
                }
                if let Some((_, id)) = best {
                    c.target = Some(id);
                    c.spent = 0;
                    c.searched = 0;
                    c.route.clear();
                    c.waypoint = 0;
                    c.plan_tick = self.tick;
                    claimed.insert(id, i);
                } else if len == 0 || c.searched >= len {
                    c.searched = 0;
                    if loaded > 0 {
                        c.returning = true;
                        c.route.clear();
                        c.waypoint = 0;
                    }
                }
            }
            let Some(id) = c.target else {
                continue;
            };
            let Some(slot) = locate(id) else {
                forget(c);
                continue;
            };
            let f = &self.matter.debris.fragments[slot];
            let p = point(f);
            c.aim = Some(p);
            let amount = e.ledger.held(f.held);
            if amount > room {
                c.returning = loaded > 0;
                claimed.remove(&id);
                forget(c);
                continue;
            }
            if square(*at, p) <= i64::from(t.collector_reach).pow(2) {
                let strength = e.mats.get(f.material).map_or(u32::MAX, |m| m.strength);
                let need = u64::from(f.count.max(1)) * u64::from(strength.max(1));
                c.spent = c
                    .spent
                    .saturating_add(u64::from(t.collector_work))
                    .min(need);
                if c.spent >= need {
                    let held = f.held;
                    let cells = f.count;
                    let moved = e.ledger.transfer(held, tank, amount);
                    assert_eq!(
                        moved, amount,
                        "whole-piece reclaim must fit the finite tank"
                    );
                    c.picked += moved;
                    c.pieces += 1;
                    *dug += u64::from(cells);
                    taken.insert(held);
                    claimed.remove(&id);
                    forget(c);
                    c.searched = 0;
                }
            } else {
                let mut approach = p;
                let axis = if (p[0] - at[0]).abs() >= (p[2] - at[2]).abs() {
                    0
                } else {
                    2
                };
                approach[axis] -= (p[axis] - at[axis]).signum() * (t.collector_reach - 2).max(0);
                if ([at[0], at[2]] == [approach[0], approach[2]]
                    && (at[1] - p[1]).abs() > t.collector_reach)
                    || !travel(
                        &self.world,
                        &self.units,
                        at,
                        turns,
                        c,
                        approach,
                        self.tick,
                        t,
                        &mut plans,
                    )
                {
                    c.rejected = Some((id, self.tick.saturating_add(t.collector_retry_ticks)));
                    claimed.remove(&id);
                    forget(c);
                }
            }
        }
        if !taken.is_empty() {
            self.matter
                .debris
                .retire_reclaimed(&mut self.matter.economy, &taken);
        }
    }
}

#[cfg(test)]
#[path = "range_lab_collector_tests.rs"]
mod tests;
