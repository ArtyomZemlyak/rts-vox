//! Wall-clock evidence stays in the CLI. It never changes the simulation.
use sim_core::{World, PHASES};
use std::{
    io::{self, Write},
    path::Path,
    time::Instant,
};

const SUB: [&str; 11] = [
    "move_requests",
    "move_drive",
    "move_suspension",
    "nav_topology",
    "nav_terrain",
    "nav_fields",
    "nav_indices",
    "combat_projectiles",
    "combat_targeting",
    "combat_fire",
    "combat_materials",
];
fn sub_times(w: &World) -> [u64; 11] {
    std::array::from_fn(|i| {
        if i < 3 {
            w.movement_ns[i]
        } else if i < 7 {
            w.navigation_ns[i - 3]
        } else {
            w.combat_ns[i - 7]
        }
    })
}
pub struct Stamp {
    at: Instant,
    phases: [u64; 16],
    sub: [u64; 11],
}
struct Sample {
    tick: u64,
    wall: u64,
    phases: [u64; 16],
    sub: [u64; 11],
    edits: usize,
    detached: usize,
    queued: usize,
}
#[derive(Default)]
pub struct Timings {
    samples: Vec<Sample>,
}

pub fn start(w: &World) -> Stamp {
    Stamp {
        phases: w.phase_ns,
        sub: sub_times(w),
        at: Instant::now(),
    }
}

fn rank(sorted: &[u64], percentile: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[(sorted.len() * percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(sorted.len() - 1)]
}

impl Timings {
    pub fn record(&mut self, w: &World, stamp: Stamp) {
        let wall = stamp.at.elapsed().as_nanos() as u64;
        let phases = std::array::from_fn(|i| w.phase_ns[i] - stamp.phases[i]);
        let now = sub_times(w);
        let sub = std::array::from_fn(|i| now[i] - stamp.sub[i]);
        let (edits, detached, queued) = w.ground().map_or((0, 0, 0), |g| {
            (g.stats.edits, g.stats.collapse.detached, g.stats.queued)
        });
        self.samples.push(Sample {
            tick: w.tick,
            wall,
            phases,
            sub,
            edits,
            detached,
            queued,
        });
    }

    pub fn report(&self, path: Option<&Path>) -> io::Result<()> {
        if self.samples.is_empty() {
            return Ok(());
        }
        let mut values: Vec<_> = self.samples.iter().map(|s| s.wall).collect();
        values.sort_unstable();
        let mean = values.iter().map(|&v| v as u128).sum::<u128>() / values.len() as u128;
        println!("tick wall us: mean={} p50={} p95={} p99={} max={} over10ms={}/{} over_tick_interval={}",
            mean / 1000, rank(&values, 50) / 1000, rank(&values, 95) / 1000,
            rank(&values, 99) / 1000, values.last().copied().unwrap_or(0) / 1000,
            values.iter().filter(|&&v| v > 10_000_000).count(), values.len(),
            values.iter().filter(|&&v| v > 1_000_000_000 / sim_core::TICKS_PER_SECOND as u64).count());
        for (i, phase) in PHASES.iter().enumerate() {
            for (v, s) in values.iter_mut().zip(&self.samples) {
                *v = s.phases[i];
            }
            values.sort_unstable();
            println!(
                "phase tail {:<14} p95_us={} max_us={}",
                phase.name(),
                rank(&values, 95) / 1000,
                values.last().copied().unwrap_or(0) / 1000
            );
        }
        if let Some(path) = path {
            let mut out = io::BufWriter::new(std::fs::File::create(path)?);
            write!(out, "tick,wall_ns")?;
            for p in PHASES {
                write!(out, ",{}_ns", p.name())?;
            }
            for name in SUB {
                write!(out, ",{name}_ns")?;
            }
            writeln!(out, ",edits,detached,queued")?;
            for s in &self.samples {
                write!(out, "{},{}", s.tick, s.wall)?;
                for value in s.phases {
                    write!(out, ",{value}")?;
                }
                for value in s.sub {
                    write!(out, ",{value}")?;
                }
                writeln!(out, ",{},{},{}", s.edits, s.detached, s.queued)?;
            }
            out.flush()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn nearest_rank_includes_the_upper_tail_and_handles_short_runs() {
        assert_eq!(super::rank(&[], 99), 0);
        assert_eq!(super::rank(&[7], 95), 7);
        let v: Vec<_> = (1..=1000).collect();
        assert_eq!(super::rank(&v, 50), 500);
        assert_eq!(super::rank(&v, 95), 950);
        assert_eq!(super::rank(&v, 99), 990);
        assert_eq!(super::rank(&v, 100), 1000);
    }
}
