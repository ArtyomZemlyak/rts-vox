//! Finite continuation of a penetrator and its assigned lateral fracture.
use super::*;
use crate::damage::Frame;
use std::collections::VecDeque;

type Cell = (usize, usize, [usize; 3]);
type Dose = (usize, usize, [usize; 3], u32);
const RAY_CELLS: usize = 64;
const SPALL_CELLS: usize = RAY_CELLS * 129;

#[path = "range_kinetic_save.rs"]
mod save;
#[cfg(test)]
#[path = "range_kinetic_tests.rs"]
mod tests;

#[derive(Default)]
pub(super) struct Pending {
    pub jobs: VecDeque<Job>,
    // Diagnostics, deliberately absent from save/hash.
    pub stats: [u64; 3],        // completed jobs, resumed cells, blocked attempts
    pub direct_stats: [u64; 3], // completed, retry attempts, blocked (including initial)
}

struct Pose {
    id: usize,
    root: Frame,
    segments: Vec<Frame>,
    delta: i32,
    pitch: i32,
}

enum Direct {
    Body(Cell),
    Ground(u16),
}

pub(super) struct Job {
    direct: Option<Direct>,
    config: ExplosionConfig,
    at: [i32; 3],
    ray: Vec<[i32; 3]>,
    cursor: usize,
    work: u32,
    lateral: Option<u32>,
    seeds: Vec<Cell>,
    spall: Option<Vec<Dose>>,
    spall_cursor: usize,
    remaining_mj: u64,
    // Allocated only when an immediate impact actually has to wait.
    poses: Option<Vec<Pose>>,
    ground: Vec<u16>,
}

impl Job {
    pub fn remaining_energy(&self) -> u64 {
        self.remaining_mj
    }
    fn freeze(&mut self, units: &[RangeUnit], world: &World) {
        if self.poses.is_some() {
            return;
        }
        let remaining = &self.ray[self.cursor..];
        self.poses = Some(
            units
                .iter()
                .enumerate()
                .filter_map(|(id, u)| {
                    let (lo, hi) = u.bounds();
                    remaining
                        .iter()
                        .any(|p| (0..3).all(|a| p[a] >= lo[a] && p[a] <= hi[a]))
                        .then(|| Pose {
                            id,
                            root: u.pose,
                            segments: u.seg_frames.clone(),
                            delta: u.turret_delta,
                            pitch: u.turret_pitch,
                        })
                })
                .collect(),
        );
        self.ground = self.ray.iter().map(|&p| material(world, p)).collect();
    }

    fn hit(&self, units: &[RangeUnit], p: [i32; 3]) -> Option<(usize, usize, [usize; 3], Frame)> {
        if let Some(poses) = &self.poses {
            for pose in poses {
                let u = &units[pose.id];
                if let Some((part, cell)) =
                    u.hit_with_turret(p, &pose.root, &pose.segments, pose.delta, pose.pitch)
                {
                    let frame = if part > 0 {
                        pose.segments[part - 1]
                    } else if cell[1] >= usize::from(u.body.turret_y) {
                        u.aimed_frame(&pose.root, pose.delta, pose.pitch)
                    } else {
                        pose.root
                    };
                    return Some((pose.id, part, cell, frame));
                }
            }
            None
        } else {
            units.iter().enumerate().find_map(|(id, u)| {
                u.hit(p).map(|(part, cell)| {
                    let frame = if part == 0 {
                        u.root_frame(cell[1])
                    } else {
                        u.seg_frames[part - 1]
                    };
                    (id, part, cell, frame)
                })
            })
        }
    }
}

impl BlastRange {
    // Flight reserves continuation capacity before any complete salvo is fired.
    pub(super) fn solid_flight_room(&self, rounds: &[battle::Round], count: usize) -> bool {
        self.kinetic.jobs.len()
            + self.streams.rounds.len()
            + rounds
                .iter()
                .filter(|r| {
                    matches!(
                        r.weapon,
                        battle::Weapon::Kinetic
                            | battle::Weapon::Canister
                            | battle::Weapon::MachineGun
                    )
                })
                .count()
            + count
            <= self.streams.tuning.kinetic.max_jobs
    }

    pub(super) fn defer_stream_contact(
        &mut self,
        target: Option<Cell>,
        p: [i32; 3],
        at: [i32; 3],
        tuning: &modding::range_weapon::Stream,
        config: &ExplosionConfig,
    ) {
        assert!(self.pending_machine_gun_contacts() < self.streams.tuning.kinetic.max_jobs);
        let mut config = *config;
        config.energy = tuning.work;
        config.material_energy_j = tuning.fracture_j;
        config.fragment_box = 1;
        config.spall = None;
        self.kinetic.jobs.push_back(Job {
            direct: Some(
                target.map_or_else(|| Direct::Ground(material(&self.world, p)), Direct::Body),
            ),
            config,
            at,
            ray: vec![p],
            cursor: 0,
            work: tuning.work,
            lateral: None,
            seeds: Vec::new(),
            spall: None,
            spall_cursor: 0,
            remaining_mj: self.matter.remaining_mj,
            poses: Some(Vec::new()),
            ground: vec![material(&self.world, p)],
        });
        // The finite allocation belongs to the pending contact now.
        self.matter.remaining_mj = 0;
        self.kinetic.stats[2] += 1;
        self.kinetic.direct_stats[2] += 1;
    }

    pub fn pending_kinetic_impacts(&self) -> usize {
        self.kinetic.jobs.len()
    }
    pub fn kinetic_progress(&self) -> [u64; 3] {
        self.kinetic.stats
    }
    pub fn machine_gun_continuation_progress(&self) -> [u64; 3] {
        self.kinetic.direct_stats
    }
    pub fn pending_machine_gun_contacts(&self) -> usize {
        self.kinetic
            .jobs
            .iter()
            .filter(|j| j.direct.is_some())
            .count()
    }

    pub(super) fn kinetic_hit_at(
        &mut self,
        at: [i32; 3],
        origin: [Fx; 3],
        velocity: [Fx; 3],
        config: &ExplosionConfig,
    ) {
        let length = velocity
            .iter()
            .fold(Fx::ZERO, |n, v| n.add(v.mul(*v)))
            .sqrt();
        if length <= Fx::ZERO {
            return;
        }
        let direction = velocity.map(|v| v.div(length));
        let horizontal_sq = velocity[0]
            .mul(velocity[0])
            .add(velocity[2].mul(velocity[2]));
        let launch_sq = Fx::from_int(config.shell_speed).mul(Fx::from_int(config.shell_speed));
        let remaining = horizontal_sq.div(launch_sq).min(Fx::ONE);
        let work =
            (u64::from(config.energy) * remaining.raw() as u64 / Fx::ONE.raw() as u64) as u32;
        if work == 0 {
            return;
        }
        // Producers count pending jobs together with flying solid rounds.
        // They wait before firing, never discard an already accepted hit.
        assert!(
            self.kinetic.jobs.len() - self.pending_machine_gun_contacts()
                < self.streams.tuning.kinetic.max_jobs
        );
        let mut ray = Vec::with_capacity(RAY_CELLS);
        for i in 0..RAY_CELLS {
            let distance = Fx::from_ratio(i as i32, 2);
            let p =
                std::array::from_fn(|a| origin[a].add(direction[a].mul(distance)).to_int_trunc());
            if ray.last() != Some(&p) {
                ray.push(p);
            }
        }
        let previous = (self.matter.remaining_mj, self.matter.fragment_box());
        self.matter
            .begin(config.material_energy_j, config.fragment_box);
        let remaining_mj = self.matter.remaining_mj;
        self.matter.remaining_mj = previous.0;
        self.matter.fragment_size(u32::from(previous.1));
        let mut job = Job {
            direct: None,
            config: *config,
            at,
            ray,
            cursor: 0,
            work,
            lateral: None,
            seeds: Vec::new(),
            spall: None,
            spall_cursor: 0,
            remaining_mj,
            poses: None,
            ground: Vec::new(),
        };
        // Preserve synchronous impact behaviour when capacity is sufficient.
        let mut budget = usize::MAX;
        if self.run_kinetic_job(&mut job, &mut budget, None) {
            self.kinetic.stats[0] += 1;
        } else {
            self.kinetic.jobs.push_back(job);
        }
    }

    pub(super) fn step_kinetic_jobs(&mut self) {
        if self.kinetic.jobs.is_empty() {
            return;
        }
        let mut budget = self.streams.tuning.kinetic.cells_per_tick;
        let count = self.kinetic.jobs.len();
        // No other material producer runs between these atomic batches.
        let mut room = self.matter.claim_room();
        for _ in 0..count {
            if budget == 0 {
                break;
            }
            let mut job = self.kinetic.jobs.pop_front().unwrap();
            let before = budget;
            let direct = job.direct.is_some();
            if direct {
                self.kinetic.direct_stats[1] += 1;
            }
            if self.run_kinetic_job(&mut job, &mut budget, Some(&mut room)) {
                self.kinetic.stats[0] += 1;
                if direct {
                    self.kinetic.direct_stats[0] += 1;
                }
            } else {
                self.kinetic.jobs.push_back(job);
            }
            self.kinetic.stats[1] += (before - budget) as u64;
        }
    }

    fn run_kinetic_job(
        &mut self,
        job: &mut Job,
        budget: &mut usize,
        room: Option<&mut u32>,
    ) -> bool {
        let before: Vec<_> = self
            .units
            .iter()
            .map(|u| (u.removed.len(), u.seg_removed.len()))
            .collect();
        let previous = (self.matter.remaining_mj, self.matter.fragment_box());
        self.matter.remaining_mj = job.remaining_mj;
        self.matter.fragment_size(job.config.fragment_box);
        if let Some(room) = &room {
            self.matter.begin_body_batch_with_room(**room);
        } else {
            self.matter.begin_body_batch();
        }
        let finished = if job.direct.is_some() {
            self.direct_contact(job, budget)
        } else {
            self.kinetic_channel(job, budget) && self.kinetic_lateral(job, budget)
        };
        let remaining = self.matter.finish_body_batch();
        if let Some(room) = room {
            *room = remaining;
        }
        job.remaining_mj = self.matter.remaining_mj;
        self.matter.remaining_mj = previous.0;
        self.matter.fragment_size(u32::from(previous.1));
        for (id, (u, old)) in self.units.iter_mut().zip(before).enumerate() {
            if (u.removed.len(), u.seg_removed.len()) != old {
                u.refresh_damage(id);
            }
        }
        if !finished {
            job.freeze(&self.units, &self.world);
        }
        finished
    }

    fn direct_contact(&mut self, job: &Job, budget: &mut usize) -> bool {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        let result = match job.direct.as_ref().unwrap() {
            Direct::Body(target) => {
                let (id, part, cell) = *target;
                let u = &mut self.units[id];
                let body = if part == 0 {
                    &u.body
                } else {
                    &u.body.segments[part - 1].body
                };
                if !body.vol.get(cell[0], cell[1], cell[2])
                    || u.section_holds(part, cell.map(|v| v as u8))
                    || (part > 0 && u.seg_gone[part - 1])
                {
                    return true;
                }
                let need = u.hit_resistance(Some((part, cell)), &job.config);
                if job.work >= need {
                    let frame = if part == 0 {
                        u.root_frame(cell[1])
                    } else {
                        u.seg_frames[part - 1]
                    };
                    let result = if part == 0 {
                        self.matter.body(id, u, cell, &frame, job.at)
                    } else {
                        self.matter
                            .body_segment(id, u, part - 1, cell, &frame, job.at)
                    };
                    if !matches!(result, matter::ClaimResult::Applied) {
                        if matches!(result, matter::ClaimResult::Deferred) {
                            self.kinetic.stats[2] += 1;
                            self.kinetic.direct_stats[2] += 1;
                        }
                        return !matches!(result, matter::ClaimResult::Deferred);
                    }
                    if part == 0 {
                        u.detached += 1;
                    } else {
                        u.seg_detached += 1;
                    }
                }
                if part == 0 {
                    u.receive_cell(cell, job.work, &job.config);
                } else {
                    u.receive_seg_cell(part - 1, cell, job.work, &job.config);
                }
                return true;
            }
            Direct::Ground(mat) => {
                let p = job.ray[0];
                if material(&self.world, p) != *mat {
                    return true;
                }
                let total = self
                    .surface_work
                    .get(&p)
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(job.work);
                if total < material_work(*mat, &job.config) {
                    self.surface_work.insert(p, total);
                    self.surface_changed.push(p);
                    return true;
                }
                let result = self.matter.ground(&mut self.world, p, job.at);
                if matches!(result, matter::ClaimResult::Applied) {
                    self.surface_work.remove(&p);
                    self.surface_changed.push(p);
                }
                result
            }
        };
        if matches!(result, matter::ClaimResult::Deferred) {
            self.kinetic.stats[2] += 1;
            self.kinetic.direct_stats[2] += 1;
        }
        !matches!(result, matter::ClaimResult::Deferred)
    }

    fn kinetic_channel(&mut self, job: &mut Job, budget: &mut usize) -> bool {
        while job.cursor < job.ray.len() && job.work > 0 {
            if *budget == 0 {
                return false;
            }
            *budget -= 1;
            let p = job.ray[job.cursor];
            if let Some((id, part, cell, frame)) = job.hit(&self.units, p) {
                if job.lateral.is_none() {
                    if let Some(spall) = &job.config.spall {
                        let share =
                            (u64::from(job.work) * u64::from(spall.work_milli) / 1000) as u32;
                        job.lateral = Some(share);
                        job.work -= share;
                    }
                }
                let u = &mut self.units[id];
                if u.section_holds(part, cell.map(|v| v as u8)) {
                    break;
                }
                let need = u.hit_resistance(Some((part, cell)), &job.config).max(1);
                let spent = job.work.min(need);
                if spent == need {
                    let result = if part == 0 {
                        self.matter.body(id, u, cell, &frame, job.at)
                    } else {
                        self.matter
                            .body_segment(id, u, part - 1, cell, &frame, job.at)
                    };
                    match result {
                        matter::ClaimResult::Deferred => {
                            self.kinetic.stats[2] += 1;
                            return false;
                        }
                        matter::ClaimResult::Applied => {}
                        _ => break,
                    }
                    if part == 0 {
                        u.detached += 1;
                    } else {
                        u.seg_detached += 1;
                    }
                    job.seeds.push((id, part, cell));
                }
                if part == 0 {
                    u.receive_cell(cell, spent, &job.config);
                } else {
                    u.receive_seg_cell(part - 1, cell, spent, &job.config);
                }
                job.work -= spent;
            } else {
                let mat = material(&self.world, p);
                // Do not redirect an old contact into newly changed terrain.
                if mat == 0 || (!job.ground.is_empty() && mat != job.ground[job.cursor]) {
                    job.cursor += 1;
                    continue;
                }
                let old = self.surface_work.get(&p).copied().unwrap_or(0);
                let need = material_work(mat, &job.config).saturating_sub(old).max(1);
                let spent = job.work.min(need);
                if spent == need {
                    match self.matter.ground(&mut self.world, p, job.at) {
                        matter::ClaimResult::Deferred => {
                            self.kinetic.stats[2] += 1;
                            return false;
                        }
                        matter::ClaimResult::Applied => {}
                        _ => break,
                    }
                    self.surface_work.remove(&p);
                } else {
                    self.surface_work.insert(p, old.saturating_add(spent));
                }
                self.surface_changed.push(p);
                job.work -= spent;
            }
            job.cursor += 1;
        }
        job.cursor = job.ray.len();
        true
    }

    fn kinetic_lateral(&mut self, job: &mut Job, budget: &mut usize) -> bool {
        if job.spall.is_none() {
            job.spall = Some(match (&job.config.spall, job.lateral) {
                (Some(s), Some(work)) => self.kinetic_spall_doses(&job.seeds, work, s.radius),
                _ => Vec::new(),
            });
        }
        let doses = job.spall.as_ref().unwrap();
        while let Some(&(id, part, cell, dose)) = doses.get(job.spall_cursor) {
            if *budget == 0 {
                return false;
            }
            *budget -= 1;
            let u = &mut self.units[id];
            let body = if part == 0 {
                &u.body
            } else {
                &u.body.segments[part - 1].body
            };
            // Another physical event may have removed/transferred this source.
            if !body.vol.get(cell[0], cell[1], cell[2])
                || u.section_holds(part, cell.map(|v| v as u8))
                || (part > 0 && u.seg_gone[part - 1])
            {
                job.spall_cursor += 1;
                continue;
            }
            let need = u.hit_resistance(Some((part, cell)), &job.config);
            if dose >= need {
                // Once the source has moved, ordinary emission follows its
                // current frame. The assigned local cell and dose stay fixed.
                let frame = if part == 0 {
                    u.root_frame(cell[1])
                } else {
                    u.seg_frames[part - 1]
                };
                match if part == 0 {
                    self.matter.body(id, u, cell, &frame, job.at)
                } else {
                    self.matter
                        .body_segment(id, u, part - 1, cell, &frame, job.at)
                } {
                    matter::ClaimResult::Deferred => {
                        self.kinetic.stats[2] += 1;
                        return false;
                    }
                    matter::ClaimResult::Applied => {}
                    _ => {
                        job.spall_cursor += 1;
                        continue;
                    }
                }
                if part == 0 {
                    u.detached += 1;
                } else {
                    u.seg_detached += 1;
                }
            }
            if part == 0 {
                u.receive_cell(cell, dose, &job.config);
            } else {
                u.receive_seg_cell(part - 1, cell, dose, &job.config);
            }
            job.spall_cursor += 1;
        }
        true
    }
}
