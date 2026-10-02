//! Opt-in observations of the real native tick, never simulation input.
//! Boundaries share one tick epoch. Phase/detail lanes overlap; their durations
//! must not be added together. Worker parents may include nested child jobs.
use super::{BlastRange, Instant, Write};
use std::{cell::RefCell, sync::Mutex};

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

struct Mark {
    lane: &'static str,
    event: &'static str,
    unit: Option<usize>,
    ns: u128,
    cpu: Option<Cpu>,
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
}
impl Tick {
    pub(super) fn record(sim: &mut BlastRange, generation: u64, profile_epoch: Instant) -> Self {
        // Allocate only in the diagnostic mode, before starting the tick clock.
        // No common worker lock: each actor owns its event buffer.
        let workers: Vec<_> = (0..sim.units.len())
            .map(|_| Mutex::new(Vec::with_capacity(24)))
            .collect();
        let marks = RefCell::new(Vec::with_capacity(128 + sim.units.len() * 14));
        let epoch = Instant::now();
        let start_ns = epoch.duration_since(profile_epoch).as_nanos();
        marks
            .borrow_mut()
            .push(Mark::new(epoch, "tick", "begin", None));
        sim.step_observed_tasks(
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
                workers[unit].lock().unwrap().push(mark);
            },
        );
        let mut marks = marks.into_inner();
        marks.push(Mark::new(epoch, "tick", "end", None));
        for events in workers {
            marks.extend(events.into_inner().unwrap());
        }
        Self {
            generation,
            tick: sim.tick,
            start_ns,
            marks,
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
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

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
