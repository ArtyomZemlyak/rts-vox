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

pub(super) fn save(output: &str, rows: &[Tick]) -> std::io::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let path = std::path::Path::new(output).with_extension("core.csv");
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
    out.flush()?;
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
        }
    }
}
