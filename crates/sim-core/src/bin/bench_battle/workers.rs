//! Opt-in wall-time and CPU-cycle observations, owned by the bench.
use std::sync::Mutex;
use std::time::Instant;

const PHASES: [&str; 12] = [
    "routes",
    "ground",
    "local_clone",
    "local_advance",
    "terrain_advance",
    "route_forget",
    "route_query",
    "sweep_0",
    "sweep_1",
    "sweep_2",
    "sweep_3",
    "feet",
];

#[derive(Clone, Copy)]
struct CpuCycles {
    thread: u32,
    cycles: u64,
}

#[cfg(windows)]
fn cpu_cycles() -> Option<CpuCycles> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut std::ffi::c_void;
        fn GetCurrentThreadId() -> u32;
        fn QueryThreadCycleTime(thread: *mut std::ffi::c_void, cycles: *mut u64) -> i32;
    }
    let mut cycles = 0;
    // SAFETY: the pseudo handle belongs to this calling thread and needs no
    // close; the output pointer names a live, aligned u64 on its stack.
    unsafe {
        (QueryThreadCycleTime(GetCurrentThread(), &mut cycles) != 0).then(|| CpuCycles {
            thread: GetCurrentThreadId(),
            cycles,
        })
    }
}

#[cfg(not(windows))]
fn cpu_cycles() -> Option<CpuCycles> {
    None
}

struct Completed {
    worker: usize,
    duration: u128,
    end: u128,
    cpu: Option<CpuCycles>,
}

#[derive(Default)]
struct Task {
    start: Option<(Instant, usize, Option<CpuCycles>)>,
    completed: Option<Completed>,
}

pub(super) struct Profile {
    epoch: Instant,
    tasks: Vec<[Mutex<Task>; PHASES.len()]>,
}

#[derive(Default)]
pub(super) struct Window {
    work: [[u128; PHASES.len()]; 8],
    cycles: [[u128; PHASES.len()]; 8],
    cpu_samples: [[usize; PHASES.len()]; 8],
    os_threads: [Option<u32>; 8],
    jobs: [[usize; PHASES.len()]; 8],
    last: [[usize; PHASES.len()]; 8],
    span: [u128; PHASES.len()],
    longest: [(u128, usize); PHASES.len()],
    units: Vec<[u128; PHASES.len()]>,
    units_cycles: Vec<[u128; PHASES.len()]>,
    tail_units: Vec<[usize; PHASES.len()]>,
}

impl Profile {
    pub(super) fn new(units: usize) -> Self {
        Self {
            epoch: Instant::now(),
            tasks: (0..units)
                .map(|_| std::array::from_fn(|_| Mutex::default()))
                .collect(),
        }
    }
    pub(super) fn observe(&self, id: usize, event: &str) {
        let (phase, begin) = match event {
            "route_begin" => (0, true),
            "route_end" => (0, false),
            "ground_begin" => (1, true),
            "ground_end" => (1, false),
            "local_clone_begin" => (2, true),
            "local_clone_end" => (2, false),
            "local_advance_begin" => (3, true),
            "local_advance_end" => (3, false),
            "terrain_advance_begin" => (4, true),
            "terrain_advance_end" => (4, false),
            "route_forget_begin" => (5, true),
            "route_forget_end" => (5, false),
            "route_query_begin" => (6, true),
            "route_query_end" => (6, false),
            "sweep_0_begin" => (7, true),
            "sweep_0_end" => (7, false),
            "sweep_1_begin" => (8, true),
            "sweep_1_end" => (8, false),
            "sweep_2_begin" => (9, true),
            "sweep_2_end" => (9, false),
            "sweep_3_begin" => (10, true),
            "sweep_3_end" => (10, false),
            "feet_begin" => (11, true),
            "feet_end" => (11, false),
            _ => unreachable!("unknown navigation task event"),
        };
        let now = Instant::now();
        let cpu = cpu_cycles();
        // Locks are private to one unit/phase. Aggregation follows pool join.
        let mut task = self.tasks[id][phase].lock().unwrap();
        if begin {
            let worker = rayon::current_thread_index().expect("navigation worker");
            assert!(worker < 8, "navigation pool exceeds supported worker count");
            assert!(task.start.is_none() && task.completed.is_none());
            task.start = Some((now, worker, cpu));
        } else {
            let (start, worker, before) = task.start.take().expect("task begin before end");
            let cpu = before.zip(cpu).and_then(|(before, after)| {
                if before.thread != after.thread {
                    return None;
                }
                after
                    .cycles
                    .checked_sub(before.cycles)
                    .map(|cycles| CpuCycles {
                        thread: after.thread,
                        cycles,
                    })
            });
            task.completed = Some(Completed {
                worker,
                duration: now.duration_since(start).as_nanos(),
                end: now.duration_since(self.epoch).as_nanos(),
                cpu,
            });
        }
    }
    pub(super) fn collect(&self, window: &mut Window) {
        window.units.resize(self.tasks.len(), [0; PHASES.len()]);
        window
            .units_cycles
            .resize(self.tasks.len(), [0; PHASES.len()]);
        window
            .tail_units
            .resize(self.tasks.len(), [0; PHASES.len()]);
        let mut first = [u128::MAX; PHASES.len()];
        let mut last = [(0, None); PHASES.len()];
        for (unit, tasks) in self.tasks.iter().enumerate() {
            for (phase, task) in tasks.iter().enumerate() {
                let mut task = task.lock().unwrap();
                assert!(task.start.is_none(), "navigation task must have joined");
                let Some(Completed {
                    worker,
                    duration,
                    end,
                    cpu,
                }) = task.completed.take()
                else {
                    continue;
                };
                if let Some(cpu) = cpu {
                    if let Some(thread) = window.os_threads[worker] {
                        assert_eq!(thread, cpu.thread, "worker changed OS thread within window");
                    }
                    window.os_threads[worker] = Some(cpu.thread);
                    window.cycles[worker][phase] += u128::from(cpu.cycles);
                    window.units_cycles[unit][phase] += u128::from(cpu.cycles);
                    window.cpu_samples[worker][phase] += 1;
                }
                window.work[worker][phase] += duration;
                window.jobs[worker][phase] += 1;
                window.units[unit][phase] += duration;
                if duration > window.longest[phase].0 {
                    window.longest[phase] = (duration, unit);
                }
                first[phase] = first[phase].min(end - duration);
                if end > last[phase].0 {
                    last[phase] = (end, Some((worker, unit)));
                }
            }
        }
        for (phase, (end, finisher)) in last.into_iter().enumerate() {
            if let Some((worker, unit)) = finisher {
                window.last[worker][phase] += 1;
                window.tail_units[unit][phase] += 1;
                window.span[phase] += end - first[phase];
            }
        }
    }
}

impl Window {
    pub(super) fn report(&self, first: usize, last: usize) {
        let ticks = (last - first + 1) as u128;
        for (phase, name) in PHASES.iter().enumerate() {
            let work_us = self.work.map(|v| v[phase] / ticks / 1000);
            let cpu_cycles = self.cycles.map(|v| v[phase] / ticks);
            let cpu_samples = self.cpu_samples.map(|v| v[phase]);
            let jobs = self.jobs.map(|v| v[phase]);
            let last_finisher = self.last.map(|v| v[phase]);
            let mut units: Vec<_> = self
                .units
                .iter()
                .enumerate()
                .map(|(id, v)| (id, v[phase] / ticks / 1000))
                .collect();
            units.sort_unstable_by_key(|&(id, us)| (std::cmp::Reverse(us), id));
            units.truncate(8);
            let mut cpu_units: Vec<_> = self
                .units_cycles
                .iter()
                .enumerate()
                .map(|(id, v)| (id, v[phase] / ticks))
                .collect();
            cpu_units.sort_unstable_by_key(|&(id, cycles)| (std::cmp::Reverse(cycles), id));
            cpu_units.truncate(8);
            let mut tails: Vec<_> = self
                .tail_units
                .iter()
                .enumerate()
                .map(|(id, v)| (id, v[phase]))
                .collect();
            tails.sort_unstable_by_key(|&(id, count)| (std::cmp::Reverse(count), id));
            tails.truncate(8);
            println!("navigation_tasks_window={first}..{last} phase={name} job_wall_us_per_tick_by_worker={work_us:?} job_cpu_cycles_per_tick_by_worker={cpu_cycles:?} cpu_samples_by_worker={cpu_samples:?} os_threads_by_worker={:?} jobs_by_worker={jobs:?} last_finisher_by_worker={last_finisher:?} span_mean_us={} longest_job_us={} longest_job_unit={} top_units_us_per_tick={units:?} top_units_cpu_cycles_per_tick={cpu_units:?} last_finisher_units={tails:?}", self.os_threads, self.span[phase] / ticks / 1000, self.longest[phase].0 / 1000, self.longest[phase].1);
        }
    }
}
