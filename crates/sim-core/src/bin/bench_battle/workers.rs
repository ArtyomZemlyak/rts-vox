//! Opt-in wall-time observations of navigation jobs, owned by the bench.
use std::sync::Mutex;
use std::time::Instant;

#[derive(Default)]
struct Task {
    start: Option<(Instant, usize)>,
    completed: Option<(usize, u128, u128)>,
}

pub(super) struct Profile {
    epoch: Instant,
    tasks: Vec<[Mutex<Task>; 2]>,
}

#[derive(Default)]
pub(super) struct Window {
    work: [[u128; 2]; 8],
    jobs: [[usize; 2]; 8],
    last: [[usize; 2]; 8],
    span: [u128; 2],
    longest: [(u128, usize); 2],
    units: Vec<[u128; 2]>,
    tail_units: Vec<[usize; 2]>,
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
            _ => unreachable!("unknown navigation task event"),
        };
        let now = Instant::now();
        // Locks are private to one unit/phase. Aggregation follows pool join.
        let mut task = self.tasks[id][phase].lock().unwrap();
        if begin {
            let worker = rayon::current_thread_index().expect("navigation worker");
            assert!(worker < 8, "navigation pool exceeds supported worker count");
            assert!(task.start.is_none() && task.completed.is_none());
            task.start = Some((now, worker));
        } else {
            let (start, worker) = task.start.take().expect("task begin before end");
            task.completed = Some((
                worker,
                now.duration_since(start).as_nanos(),
                now.duration_since(self.epoch).as_nanos(),
            ));
        }
    }
    pub(super) fn collect(&self, window: &mut Window) {
        window.units.resize(self.tasks.len(), [0; 2]);
        window.tail_units.resize(self.tasks.len(), [0; 2]);
        let mut first = [u128::MAX; 2];
        let mut last = [(0, None); 2];
        for (unit, tasks) in self.tasks.iter().enumerate() {
            for (phase, task) in tasks.iter().enumerate() {
                let mut task = task.lock().unwrap();
                assert!(task.start.is_none(), "navigation task must have joined");
                let Some((worker, duration, end)) = task.completed.take() else {
                    continue;
                };
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
        for (phase, name) in ["routes", "ground"].iter().enumerate() {
            let work_us = self.work.map(|v| v[phase] / ticks / 1000);
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
            let mut tails: Vec<_> = self
                .tail_units
                .iter()
                .enumerate()
                .map(|(id, v)| (id, v[phase]))
                .collect();
            tails.sort_unstable_by_key(|&(id, count)| (std::cmp::Reverse(count), id));
            tails.truncate(8);
            println!("navigation_tasks_window={first}..{last} phase={name} job_wall_us_per_tick_by_worker={work_us:?} jobs_by_worker={jobs:?} last_finisher_by_worker={last_finisher:?} span_mean_us={} longest_job_us={} longest_job_unit={} top_units_us_per_tick={units:?} last_finisher_units={tails:?}", self.span[phase] / ticks / 1000, self.longest[phase].0 / 1000, self.longest[phase].1);
        }
    }
}
