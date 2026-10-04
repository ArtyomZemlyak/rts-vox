//! Wall-clock camera workload, independent of simulation progress and FPS.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Sample {
    pub segment: u32,
    pub focus: Vec2,
    pub height: f32,
}
pub(super) fn sample(seconds: f64, size: [u32; 3]) -> Sample {
    // Holds exercise jumps; interpolated legs exercise continuous toroidal pans.
    let stops = [
        (0.15, 0.12, 240.0),
        (0.22, 0.20, 240.0),
        (0.5, 0.5, 240.0),
        (0.85, 0.88, 240.0),
        (0.78, 0.80, 240.0),
        (0.5, 0.5, size[0].max(size[2]) as f32 * 1.7),
        (0.38, 0.64, 240.0),
        (0.64, 0.38, 240.0),
    ];
    let segment = (seconds.max(0.0) / 6.0).floor() as u32;
    let i = segment as usize % stops.len();
    let a = stops[i];
    let b = stops[(i + 1) % stops.len()];
    let t = if matches!(i, 0 | 3) {
        0.0 // The next segment starts with a camera jump.
    } else {
        ((seconds.rem_euclid(6.0) - 3.0) / 3.0).clamp(0.0, 1.0) as f32
    };
    let t = t * t * (3.0 - 2.0 * t);
    Sample {
        segment,
        focus: Vec2::new(
            (a.0 + (b.0 - a.0) * t) * size[0] as f32,
            (a.1 + (b.1 - a.1) * t) * size[2] as f32,
        ),
        height: a.2 + (b.2 - a.2) * t,
    }
}

#[derive(Clone, Copy)]
pub(super) struct Frame {
    pub seconds: f64,
    pub tick: u32,
    pub sample: Sample,
    pub origin: [i32; 3],
    pub progress: Option<u32>,
    pub blocked: bool,
    pub observe_ms: f64,
    pub stream_ms: f64,
}
pub(super) fn save(output: &str, frames: &[Frame]) -> std::io::Result<()> {
    let mut out = std::io::BufWriter::new(std::fs::File::create(
        std::path::Path::new(output).with_extension("camera.csv"),
    )?);
    writeln!(
        out,
        "seconds,tick,segment,focus_x,focus_z,height,origin_x,origin_z,progress,blocked,observe_ms,stream_ms"
    )?;
    for f in frames {
        writeln!(
            out,
            "{:.6},{},{},{:.3},{:.3},{:.3},{},{},{},{},{:.6},{:.6}",
            f.seconds,
            f.tick,
            f.sample.segment,
            f.sample.focus.x,
            f.sample.focus.y,
            f.sample.height,
            f.origin[0],
            f.origin[2],
            f.progress.map_or(String::new(), |v| v.to_string()),
            f.blocked,
            f.observe_ms,
            f.stream_ms
        )?;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tour_repeats_and_covers_both_bases_pan_and_map_zoom() {
        let size = [10_240, 192, 10_240];
        for seconds in [0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0] {
            let a = sample(seconds, size);
            let b = sample(seconds + 48.0, size);
            assert_eq!(a.focus, b.focus);
            assert_eq!(a.height, b.height);
            assert_eq!(a.segment + 8, b.segment);
        }
        assert!(sample(0.0, size).focus.x < 2000.0);
        assert!(sample(18.0, size).focus.x > 8000.0);
        assert!(sample(30.0, size).height > 15_000.0);
        assert_ne!(sample(9.0, size).focus, sample(10.0, size).focus);
    }
}
