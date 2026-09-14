//! Direct capture of the final game render, with wall-clock frame timestamps.
//! PNG conversion runs off the frame thread; a full queue skips a sample,
//! never delays or changes the simulation clock.
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::tasks::IoTaskPool;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

const FPS: f64 = 30.0;
const MAX_PENDING: usize = 4;

#[derive(Default)]
struct Writes {
    pending: AtomicUsize,
    failed: AtomicBool,
}
struct Frame {
    wall: f64,
    tick: u64,
    sim: f64,
}
struct Session {
    dir: PathBuf,
    seconds: f64,
    warm: Option<f64>,
    start: Option<f64>,
    initial_tick: u64,
    initial_time: f64,
    last_slot: u64,
    stop: Option<f64>,
    end_tick: u64,
    end_sim: f64,
    frames: Vec<Frame>,
    writes: Arc<Writes>,
    finished: bool,
}

#[derive(Resource, Default)]
pub struct Recording(Option<Session>);

impl Recording {
    pub fn new(dir: Option<String>, seconds: f64) -> io::Result<Self> {
        let Some(dir) = dir else {
            return Ok(Self::default());
        };
        if !seconds.is_finite() || !(0.1..=3600.0).contains(&seconds) {
            return Err(io::Error::other("record-seconds must be in 0.1..=3600"));
        }
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir)?;
        // Reusing an old sequence could silently encode stale frames.
        if std::fs::read_dir(&dir)?.next().is_some() {
            return Err(io::Error::other("record directory must be empty"));
        }
        Ok(Self(Some(Session {
            dir,
            seconds,
            warm: None,
            start: None,
            initial_tick: 0,
            initial_time: 0.0,
            last_slot: 0,
            stop: None,
            end_tick: 0,
            end_sim: 0.0,
            frames: Vec::new(),
            writes: Arc::new(Writes::default()),
            finished: false,
        })))
    }

    /// Warm the renderer and capture the initial pose before starting the clock.
    /// Drain the final PNG jobs with the simulation frozen at its end state.
    pub fn running(&self) -> bool {
        self.0
            .as_ref()
            .is_none_or(|s| s.start.is_some() && s.stop.is_none())
    }

    pub fn capture(
        &mut self,
        commands: &mut Commands,
        now: f64,
        tick: u64,
        sim: f64,
    ) -> Option<AppExit> {
        let s = self.0.as_mut()?;
        if s.finished {
            return None;
        }
        let warm = *s.warm.get_or_insert(now);
        if s.writes.failed.load(Ordering::Acquire) {
            s.finished = true;
            return Some(AppExit::error());
        }
        if s.start.is_none() {
            if now - warm > 30.0 {
                eprintln!("record: initial screenshot did not finish within 30 seconds");
                s.finished = true;
                return Some(AppExit::error());
            }
            if now - warm < 2.0 {
                return None;
            }
            if s.frames.is_empty() {
                s.initial_tick = tick;
                s.initial_time = sim;
                s.request(commands, 0.0, tick, 0.0);
            } else if s.writes.pending.load(Ordering::Acquire) == 0 {
                s.start = Some(now);
                println!(
                    "RECORD started: tick={tick} realtime=1x seconds={}",
                    s.seconds
                );
            }
            return None;
        }
        let elapsed = now - s.start.unwrap();
        if elapsed >= s.seconds && s.stop.is_none() {
            s.stop = Some(now);
            s.end_tick = tick;
            s.end_sim = sim - s.initial_time;
        }
        if let Some(stop) = s.stop {
            if s.writes.pending.load(Ordering::Acquire) == 0 {
                s.finished = true;
                return Some(match s.finish(stop - s.start.unwrap()) {
                    Ok(()) => AppExit::Success,
                    Err(e) => {
                        eprintln!("record manifest: {e}");
                        AppExit::error()
                    }
                });
            }
            if now - stop > 30.0 {
                s.finished = true;
                eprintln!("record: pending screenshot/write timeout");
                return Some(AppExit::error());
            }
            return None;
        }
        if let Some(slot) = due_slot(
            elapsed,
            s.seconds,
            s.last_slot,
            s.writes.pending.load(Ordering::Acquire),
        ) {
            s.last_slot = slot;
            s.request(commands, elapsed, tick, sim - s.initial_time);
        }
        None
    }
}

fn due_slot(elapsed: f64, seconds: f64, last: u64, pending: usize) -> Option<u64> {
    let slot = (elapsed * FPS).floor() as u64;
    (elapsed < seconds && slot > last && pending < MAX_PENDING).then_some(slot)
}

impl Session {
    fn request(&mut self, commands: &mut Commands, wall: f64, tick: u64, sim: f64) {
        let path = self.dir.join(format!("{:06}.png", self.frames.len()));
        self.frames.push(Frame { wall, tick, sim });
        self.writes.pending.fetch_add(1, Ordering::AcqRel);
        let writes = self.writes.clone();
        commands.spawn(Screenshot::primary_window()).observe(
            move |event: On<ScreenshotCaptured>| {
                let image = event.image.clone();
                let (path, writes) = (path.clone(), writes.clone());
                IoTaskPool::get()
                    .spawn(async move {
                        let result = image
                            .try_into_dynamic()
                            .map_err(|e| format!("{e}"))
                            // HDR alpha contains brightness, not transparency.
                            .and_then(|img| img.to_rgb8().save(&path).map_err(|e| e.to_string()));
                        if let Err(e) = result {
                            eprintln!("record {}: {e}", path.display());
                            writes.failed.store(true, Ordering::Release);
                        }
                        writes.pending.fetch_sub(1, Ordering::Release);
                    })
                    .detach();
            },
        );
    }

    fn finish(&self, wall: f64) -> io::Result<()> {
        let mut csv = io::BufWriter::new(std::fs::File::create(self.dir.join("frames.csv"))?);
        writeln!(csv, "file,wall_seconds,tick,simulation_seconds")?;
        for (i, f) in self.frames.iter().enumerate() {
            writeln!(csv, "{i:06}.png,{:.9},{},{:.9}", f.wall, f.tick, f.sim)?;
        }
        csv.flush()?;
        write_timeline(
            &self.dir.join("timeline.ffconcat"),
            &self.frames,
            self.seconds,
        )?;
        println!("RECORD complete: images={} wall={wall:.6}s sim={:.6}s ticks={}..{} video={:.3}s pending=0",
            self.frames.len(), self.end_sim, self.initial_tick, self.end_tick, self.seconds);
        Ok(())
    }
}

fn write_timeline(path: &Path, frames: &[Frame], duration: f64) -> io::Result<()> {
    let mut out = io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(out, "ffconcat version 1.0")?;
    for (i, f) in frames.iter().enumerate() {
        let next = frames.get(i + 1).map_or(duration, |n| n.wall);
        writeln!(
            out,
            "file '{i:06}.png'\noption framerate 1000\nduration {:.9}",
            next - f.wall
        )?;
    }
    // Concat needs another packet to preserve the last image's duration.
    if !frames.is_empty() {
        writeln!(
            out,
            "file '{:06}.png'\noption framerate 1000",
            frames.len() - 1
        )?;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_slow_writer_skips_samples_without_compressing_wall_time() {
        assert_eq!(due_slot(0.034, 1.0, 0, 0), Some(1));
        assert_eq!(due_slot(0.034, 1.0, 1, 0), None);
        assert_eq!(due_slot(0.19, 1.0, 1, MAX_PENDING), None);
        assert_eq!(due_slot(0.21, 1.0, 1, 0), Some(6));
        assert_eq!(due_slot(1.0, 1.0, 6, 0), None);
    }
    #[test]
    fn the_timeline_keeps_gaps_and_the_final_hold() {
        let frames = [
            Frame {
                wall: 0.0,
                tick: 0,
                sim: 0.0,
            },
            Frame {
                wall: 0.07,
                tick: 2,
                sim: 0.07,
            },
            Frame {
                wall: 0.3,
                tick: 9,
                sim: 0.3,
            },
        ];
        let path =
            std::env::temp_dir().join(format!("rts-record-test-{}.ffconcat", std::process::id()));
        write_timeline(&path, &frames, 0.5).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(text.contains("duration 0.070000000") && text.contains("duration 0.230000000"));
        assert!(text.contains("duration 0.200000000"));
        assert_eq!(text.matches("file '000002.png'").count(), 2);
    }
}
