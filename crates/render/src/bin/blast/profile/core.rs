//! Opt-in observations of the real native tick, never simulation input.
//! Boundaries share one tick epoch. Phase/detail lanes overlap; their durations
//! must not be added together. Worker parents may include nested child jobs.
use super::{BlastRange, Instant, Write};
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicU32, Ordering},
        Mutex,
    },
};
#[path = "physics.rs"]
mod physics;

#[derive(Clone, Copy)]
struct Cpu {
    thread: u32,
    cycles: Option<u64>,
    time_ns: Option<u64>,
}

#[cfg(windows)]
fn cpu() -> Option<Cpu> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut std::ffi::c_void;
        fn GetCurrentThreadId() -> u32;
        fn QueryThreadCycleTime(thread: *mut std::ffi::c_void, cycles: *mut u64) -> i32;
        fn GetThreadTimes(
            thread: *mut std::ffi::c_void,
            created: *mut FileTime,
            exited: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    impl FileTime {
        fn value(&self) -> u64 {
            (u64::from(self.high) << 32) | u64::from(self.low)
        }
    }
    let mut cycles = 0;
    let (mut created, mut exited, mut kernel, mut user) = (
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
    );
    // SAFETY: the pseudo handle belongs to the caller and needs no close;
    // the output points to a live, aligned stack u64.
    unsafe {
        let handle = GetCurrentThread();
        let cycles = (QueryThreadCycleTime(handle, &mut cycles) != 0).then_some(cycles);
        let time_ns = (GetThreadTimes(handle, &mut created, &mut exited, &mut kernel, &mut user)
            != 0)
            .then(|| kernel.value().checked_add(user.value())?.checked_mul(100))
            .flatten();
        Some(Cpu {
            thread: GetCurrentThreadId(),
            cycles,
            time_ns,
        })
    }
}

#[cfg(not(windows))]
fn cpu() -> Option<Cpu> {
    None
}

#[cfg(windows)]
fn cpu_identity() -> Option<Cpu> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    // SAFETY: this call returns the executing thread ID without an output buffer.
    Some(Cpu {
        thread: unsafe { GetCurrentThreadId() },
        cycles: None,
        time_ns: None,
    })
}
#[cfg(not(windows))]
fn cpu_identity() -> Option<Cpu> {
    None
}

#[derive(Clone, Copy)]
struct Mark {
    lane: &'static str,
    event: &'static str,
    unit: Option<usize>,
    ns: u128,
    cpu: Option<Cpu>,
}

/// Diagnostic buffers owned by OS thread, including physics jobs without an
/// actor ID. Registration uses atomics; unrelated workers never share the
/// append lock. Unknown thread IDs retain events in the fallback buffer and
/// remain explicitly unattributable in the analyzer.
struct WorkerMarks {
    slots: Vec<(AtomicU32, Mutex<Vec<Mark>>)>,
    unknown: Mutex<Vec<Mark>>,
}
impl WorkerMarks {
    fn new() -> Self {
        Self {
            // Profile runner allows 128 physics + 8 navigation workers.
            slots: (0..256)
                .map(|_| (AtomicU32::new(0), Mutex::new(Vec::new())))
                .collect(),
            unknown: Mutex::new(Vec::new()),
        }
    }
    fn push(&self, mark: Mark) {
        if let Some(id) = mark.cpu.map(|c| c.thread).filter(|&id| id != 0) {
            for (owner, marks) in &self.slots {
                let current = owner.load(Ordering::Relaxed);
                if current == id
                    || (current == 0
                        && owner
                            .compare_exchange(0, id, Ordering::Relaxed, Ordering::Relaxed)
                            .is_ok())
                {
                    marks.lock().unwrap().push(mark);
                    return;
                }
            }
            // Never silently drop a trace at an unexpected thread ceiling.
        }
        self.unknown.lock().unwrap().push(mark);
    }
    fn append_to(self, marks: &mut Vec<Mark>) -> Vec<physics::Total> {
        let mut totals = Vec::new();
        for (_, events) in self.slots {
            let (events, summary) = physics::compact(events.into_inner().unwrap());
            marks.extend(events);
            totals.extend(summary);
        }
        let (events, summary) = physics::compact(self.unknown.into_inner().unwrap());
        marks.extend(events);
        totals.extend(summary);
        totals
    }
}
impl Mark {
    fn phase(epoch: Instant, lane: &'static str, event: &'static str) -> Self {
        Self {
            lane,
            event,
            unit: None,
            ns: epoch.elapsed().as_nanos(),
            cpu: cpu_identity(),
        }
    }
    fn new(epoch: Instant, lane: &'static str, event: &'static str, unit: Option<usize>) -> Self {
        let ns = epoch.elapsed().as_nanos();
        Self {
            lane,
            event,
            unit,
            ns,
            cpu: cpu(),
        }
    }
}

pub(super) struct Tick {
    generation: u64,
    tick: u32,
    start_ns: u128,
    marks: Vec<Mark>,
    physics: Vec<physics::Total>,
}
impl Tick {
    pub(super) fn record_phases(
        sim: &mut BlastRange,
        generation: u64,
        profile_epoch: Instant,
    ) -> Self {
        let marks = RefCell::new(Vec::with_capacity(64));
        let epoch = Instant::now();
        let start_ns = epoch.duration_since(profile_epoch).as_nanos();
        marks
            .borrow_mut()
            .push(Mark::new(epoch, "tick", "begin", None));
        sim.step_observed_detail(
            |event| marks.borrow_mut().push(Mark::phase(epoch, "phase", event)),
            |event| marks.borrow_mut().push(Mark::phase(epoch, "detail", event)),
            |_, _| {},
        );
        let mut marks = marks.into_inner();
        marks.push(Mark::new(epoch, "tick", "end", None));
        Self {
            generation,
            tick: sim.tick,
            start_ns,
            marks,
            physics: Vec::new(),
        }
    }
    fn wall_ns(&self) -> u128 {
        let begin = self
            .marks
            .iter()
            .find(|m| m.lane == "tick" && m.event == "begin")
            .unwrap()
            .ns;
        let end = self
            .marks
            .iter()
            .find(|m| m.lane == "tick" && m.event == "end")
            .unwrap()
            .ns;
        end - begin
    }
    pub(super) fn record(sim: &mut BlastRange, generation: u64, profile_epoch: Instant) -> Self {
        // Allocate only in the diagnostic mode, before starting the tick clock.
        // No common worker lock: each known OS thread owns its event buffer.
        let workers = WorkerMarks::new();
        let marks = RefCell::new(Vec::with_capacity(128 + sim.units.len() * 14));
        let epoch = Instant::now();
        let start_ns = epoch.duration_since(profile_epoch).as_nanos();
        marks
            .borrow_mut()
            .push(Mark::new(epoch, "tick", "begin", None));
        sim.step_observed_all_tasks(
            |event| {
                marks
                    .borrow_mut()
                    .push(Mark::new(epoch, "phase", event, None))
            },
            |event| {
                marks
                    .borrow_mut()
                    .push(Mark::new(epoch, "detail", event, None))
            },
            |unit, event| {
                marks
                    .borrow_mut()
                    .push(Mark::new(epoch, "unit", event, Some(unit)))
            },
            |unit, event| {
                let mark = Mark::new(epoch, "task", event, Some(unit));
                workers.push(mark);
            },
            |slot, event| {
                workers.push(Mark::new(epoch, "task", event, Some(slot)));
            },
        );
        let mut marks = marks.into_inner();
        marks.push(Mark::new(epoch, "tick", "end", None));
        let physics = workers.append_to(&mut marks);
        Self {
            generation,
            tick: sim.tick,
            start_ns,
            marks,
            physics,
        }
    }
}

fn save_marks(output: &str, rows: &[Tick], extension: &str) -> std::io::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let path = std::path::Path::new(output).with_extension(extension);
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(
        out,
        "generation,tick,lane,event,unit,offset_ns,os_thread,cpu_cycles,thread_cpu_ns,profile_offset_ns"
    )?;
    for row in rows {
        for mark in &row.marks {
            write!(
                out,
                "{},{},{},{},",
                row.generation, row.tick, mark.lane, mark.event
            )?;
            if let Some(unit) = mark.unit {
                write!(out, "{unit}")?;
            }
            write!(
                out,
                ",{},{},",
                mark.ns,
                mark.cpu.map_or(String::new(), |c| c.thread.to_string())
            )?;
            if let Some(cycles) = mark.cpu.and_then(|c| c.cycles) {
                write!(out, "{cycles}")?;
            }
            write!(out, ",")?;
            if let Some(cpu_ns) = mark.cpu.and_then(|c| c.time_ns) {
                write!(out, "{cpu_ns}")?;
            }
            writeln!(out, ",{}", row.start_ns + mark.ns)?;
        }
    }
    out.flush()
}

pub(super) fn save_stalls(output: &str, rows: &[Tick]) -> std::io::Result<()> {
    save_marks(output, rows, "stall.csv")
}

pub(super) fn save(output: &str, rows: &[Tick]) -> std::io::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    save_marks(output, rows, "core.csv")?;
    let path = std::path::Path::new(output).with_extension("physics.csv");
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(out, "generation,tick,os_thread,stage,family,jobs,inclusive_wall_ns,max_wall_ns,cpu_ns,cpu_samples,cycles,cycle_samples")?;
    for row in rows {
        for total in &row.physics {
            writeln!(
                out,
                "{},{},{},{},{},{},{},{},{},{},{},{}",
                row.generation,
                row.tick,
                total.thread.map_or(String::new(), |id| id.to_string()),
                total.stage.map_or(String::new(), |id| id.to_string()),
                total.family,
                total.jobs,
                total.wall_ns,
                total.max_ns,
                total.cpu_ns,
                total.cpu_samples,
                total.cycles,
                total.cycle_samples
            )?;
        }
    }
    out.flush()
}

/// A diagnostic flight recorder. No file IO, worker marks or locks during a
/// tick. Retain bounded windows, plus the last quiet history at shutdown.
pub(super) struct Trigger {
    pub(super) threshold_ms: u32,
    history: std::collections::VecDeque<Tick>,
    retained: Vec<Tick>,
    post_left: usize,
    pub(super) windows: usize,
    pub(super) slow_ticks: u64,
    pub(super) unarmed_slow_ticks: u64,
}
impl Trigger {
    pub(super) const PRE: usize = 16;
    pub(super) const POST: usize = 16;
    pub(super) const MAX_WINDOWS: usize = 4;
    pub(super) fn new(threshold_ms: u32) -> Self {
        assert!((1..=10000).contains(&threshold_ms));
        Self {
            threshold_ms,
            history: std::collections::VecDeque::with_capacity(Self::PRE),
            retained: Vec::new(),
            post_left: 0,
            windows: 0,
            slow_ticks: 0,
            unarmed_slow_ticks: 0,
        }
    }
    pub(super) fn push(&mut self, tick: Tick) {
        // A reset must not attach old-generation context to a new incident.
        let previous = self.history.back().or_else(|| self.retained.last());
        if previous.is_some_and(|was| was.generation != tick.generation) {
            self.history.clear();
            self.post_left = 0;
        }
        let slow = tick.wall_ns() >= u128::from(self.threshold_ms) * 1_000_000;
        self.slow_ticks += u64::from(slow);
        if self.post_left > 0 {
            self.retained.push(tick);
            self.post_left -= 1;
        } else if slow && self.windows < Self::MAX_WINDOWS {
            self.retained.extend(self.history.drain(..));
            self.retained.push(tick);
            self.windows += 1;
            self.post_left = Self::POST;
        } else {
            self.unarmed_slow_ticks += u64::from(slow);
            if self.history.len() == Self::PRE {
                self.history.pop_front();
            }
            self.history.push_back(tick);
        }
    }
    pub(super) fn finish(&mut self) -> Vec<Tick> {
        self.retained.extend(self.history.drain(..));
        std::mem::take(&mut self.retained)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_buffers_keep_concurrent_physics_batches_and_overflow() {
        let buffers = WorkerMarks::new();
        std::thread::scope(|scope| {
            for id in 1..=32 {
                let buffers = &buffers;
                scope.spawn(move || {
                    for event in ["physics_translate_begin", "physics_translate_end"] {
                        buffers.push(Mark {
                            lane: "task",
                            event,
                            unit: Some(0),
                            ns: id as u128,
                            cpu: Some(Cpu {
                                thread: id,
                                cycles: None,
                                time_ns: None,
                            }),
                        });
                    }
                });
            }
        });
        // Exhaust registration and retain both overflow and unknown-ID marks.
        for id in 33..=257 {
            buffers.push(Mark {
                lane: "task",
                event: "physics_cells_begin",
                unit: Some(0),
                ns: 0,
                cpu: Some(Cpu {
                    thread: id,
                    cycles: None,
                    time_ns: None,
                }),
            });
        }
        buffers.push(Mark {
            lane: "task",
            event: "unknown",
            unit: Some(0),
            ns: 0,
            cpu: None,
        });
        let mut marks = Vec::new();
        let _ = buffers.append_to(&mut marks);
        assert_eq!(marks.len(), 290);
        for id in 1..=32 {
            let events: Vec<_> = marks
                .iter()
                .filter(|m| m.cpu.is_some_and(|c| c.thread == id))
                .map(|m| m.event)
                .collect();
            assert_eq!(events, ["physics_translate_begin", "physics_translate_end"]);
        }
        assert!(marks.iter().any(|m| m.cpu.is_some_and(|c| c.thread == 257)));
        assert_eq!(marks.iter().filter(|m| m.cpu.is_none()).count(), 1);
    }

    #[test]
    fn observation_does_not_change_tick_or_snapshot() {
        let mut sim = BlastRange::new(super::super::weapon_config(
            false,
            false,
            1000,
            modding::explosion::Delivery::Blast,
        ));
        let mut observed = BlastRange::new(sim.config.clone());
        let mut phases = BlastRange::new(sim.config.clone());
        for _ in 0..12 {
            sim.step();
            let trace = Tick::record(&mut observed, 7, Instant::now());
            assert_eq!(trace.tick, sim.tick);
            assert_eq!(trace.generation, 7);
            assert_eq!(trace.marks.first().unwrap().event, "begin");
            let end = trace
                .marks
                .iter()
                .find(|m| m.lane == "tick" && m.event == "end")
                .unwrap();
            assert!(trace.marks.iter().all(|m| m.ns <= end.ns));
            assert_eq!(sim.snapshot(), observed.snapshot());
            let light = Tick::record_phases(&mut phases, 7, Instant::now());
            assert_eq!(sim.snapshot(), phases.snapshot());
            assert!(light.physics.is_empty());
            assert!(light.marks.iter().filter(|m| m.lane != "tick").all(|m| m
                .cpu
                .is_none_or(|cpu| cpu.time_ns.is_none() && cpu.cycles.is_none())));
            assert!(light
                .marks
                .iter()
                .all(|m| m.lane != "task" && m.lane != "unit"));
        }
    }
    fn sample(tick: u32, generation: u64, ms: u128) -> Tick {
        Tick {
            generation,
            tick,
            start_ns: u128::from(tick) * 100_000_000,
            marks: vec![
                Mark {
                    lane: "tick",
                    event: "begin",
                    unit: None,
                    ns: 0,
                    cpu: None,
                },
                Mark {
                    lane: "tick",
                    event: "end",
                    unit: None,
                    ns: ms * 1_000_000,
                    cpu: None,
                },
            ],
            physics: Vec::new(),
        }
    }
    #[test]
    fn trigger_keeps_context_and_post_window_without_duplicates() {
        let mut ring = Trigger::new(50);
        for tick in 1..=100 {
            ring.push(sample(tick, 0, if tick == 30 { 50 } else { 1 }));
        }
        let rows = ring.finish();
        let expected: Vec<_> = (14..=46).chain(85..=100).collect();
        assert_eq!(rows.iter().map(|r| r.tick).collect::<Vec<_>>(), expected);
        assert_eq!(ring.windows, 1);
        assert_eq!(ring.slow_ticks, 1);
    }
    #[test]
    fn trigger_bounds_repeated_stalls_and_counts_unarmed_crossings() {
        let mut ring = Trigger::new(50);
        for tick in 1..=1000 {
            ring.push(sample(tick, 0, 51));
        }
        let rows = ring.finish();
        assert_eq!(ring.windows, Trigger::MAX_WINDOWS);
        assert_eq!(ring.slow_ticks, 1000);
        assert!(ring.unarmed_slow_ticks > 0);
        assert!(
            rows.len() <= Trigger::MAX_WINDOWS * (Trigger::PRE + 1 + Trigger::POST) + Trigger::PRE
        );
        assert!(rows.windows(2).all(|w| w[0].tick < w[1].tick));
    }
    #[test]
    fn trigger_reset_cuts_history_and_unfinished_post_capture() {
        let mut ring = Trigger::new(50);
        for tick in 1..=20 {
            ring.push(sample(tick, 0, if tick == 20 { 51 } else { 1 }));
        }
        ring.push(sample(1, 1, 1));
        ring.push(sample(2, 1, 51));
        let rows = ring.finish();
        assert_eq!(ring.windows, 2);
        assert_eq!(
            rows.iter()
                .filter(|r| r.generation == 1)
                .map(|r| r.tick)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }
    #[test]
    fn trigger_without_incident_retains_only_quiet_tail() {
        let mut ring = Trigger::new(50);
        for tick in 1..=100 {
            ring.push(sample(tick, 0, 1));
        }
        assert_eq!(ring.windows, 0);
        assert_eq!(ring.finish().len(), Trigger::PRE);
        assert!(ring.finish().is_empty());
    }
}
