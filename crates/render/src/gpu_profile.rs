//! Bounded export of published GPU diagnostics, never CPU/GPU frame correlation.
//! Bevy publishes the latest completed batch asynchronously; its timestamp is
//! availability on the main thread, not the GPU execution time or frame ID.
use bevy::diagnostic::DiagnosticsStore;
use std::{collections::BTreeMap, io::Write, time::Instant};

const LIMIT: usize = 1_000_000;
#[derive(Clone, Copy)]
struct Sample {
    row: usize,
    available_ns: u128,
    ms: f64,
}
#[derive(Default)]
struct Pass {
    last: Option<Instant>,
    samples: Vec<Sample>,
}
pub struct Capture {
    passes: BTreeMap<String, Pass>,
    limit: usize,
    seen: usize,
    retained: usize,
    invalid: usize,
}
impl Default for Capture {
    fn default() -> Self {
        Self {
            passes: BTreeMap::new(),
            limit: LIMIT,
            seen: 0,
            retained: 0,
            invalid: 0,
        }
    }
}
impl Capture {
    pub fn observe(&mut self, diagnostics: &DiagnosticsStore, epoch: Instant, row: usize) {
        for d in diagnostics.iter() {
            let path = d.path().to_string();
            if path.ends_with("/elapsed_gpu") {
                if let Some(m) = d.measurement() {
                    self.record(&path, m.time, m.value, epoch, row);
                }
            }
        }
    }
    fn record(&mut self, path: &str, time: Instant, ms: f64, epoch: Instant, row: usize) {
        let Some(available) = time.checked_duration_since(epoch) else {
            return; // Do not relabel warmup measurements as measured work.
        };
        let pass = self.passes.entry(path.to_owned()).or_default();
        if pass.last.is_some_and(|last| last >= time) {
            return; // An unchanged or stale publication is not a new GPU pass.
        }
        pass.last = Some(time);
        self.seen += 1;
        if !ms.is_finite() || ms < 0.0 {
            self.invalid += 1;
        } else if self.retained < self.limit {
            pass.samples.push(Sample {
                row,
                available_ns: available.as_nanos(),
                ms,
            });
            self.retained += 1;
        }
    }
    pub fn save(&self, out: &mut impl Write) -> std::io::Result<()> {
        writeln!(out, "observation_row,available_ns,path,elapsed_gpu_ms")?;
        for (path, pass) in &self.passes {
            let escaped = path.replace('"', "\"\"");
            for sample in &pass.samples {
                writeln!(
                    out,
                    "{},{},\"{}\",{:.9}",
                    sample.row, sample.available_ns, escaped, sample.ms
                )?;
            }
        }
        Ok(())
    }
    pub fn summary(&self, out: &mut impl Write) -> std::io::Result<()> {
        writeln!(out, "PROFILE gpu_capture_schema=1 gpu_sample_limit={} gpu_samples_seen={} gpu_samples_retained={} gpu_samples_dropped={} gpu_samples_invalid={} gpu_clock=publication_without_source_frame", self.limit, self.seen, self.retained, self.seen - self.retained - self.invalid, self.invalid)?;
        for (path, pass) in &self.passes {
            let mut v: Vec<_> = pass.samples.iter().map(|s| s.ms).collect();
            v.sort_by(f64::total_cmp);
            if !v.is_empty() {
                writeln!(out, "GPU {path} n={} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} max_ms={:.3} mean_ms={:.6}", v.len(), v[v.len()/2], v[v.len()*95/100], v[v.len()*99/100], v[v.len()-1], v.iter().sum::<f64>() / v.len() as f64)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn publications_keep_batch_identity_and_exclude_stale_and_warmup_values() {
        let start = Instant::now();
        let epoch = start + Duration::from_millis(1);
        let time = epoch + Duration::from_millis(2);
        let mut capture = Capture::default();
        capture.record("a", start, 999.0, epoch, 0);
        capture.record("a", time, 2.0, epoch, 3);
        capture.record("a", time, 3.0, epoch, 4);
        capture.record("a", epoch, 4.0, epoch, 5);
        capture.record("b", time, 0.0, epoch, 3);
        assert_eq!((capture.seen, capture.retained), (2, 2));
        assert_eq!(capture.passes["a"].samples[0].ms, 2.0);
        for pass in capture.passes.values() {
            assert_eq!(pass.samples[0].available_ns, 2_000_000);
            assert_eq!(pass.samples[0].row, 3);
        }
    }
    #[test]
    fn overflow_invalid_values_and_csv_names_are_explicit() {
        let epoch = Instant::now();
        let mut capture = Capture {
            limit: 1,
            ..Default::default()
        };
        capture.record("a,\"b", epoch, 1.25, epoch, 0);
        capture.record("c", epoch, 2.5, epoch, 0);
        capture.record("d", epoch, f64::NAN, epoch, 0);
        capture.record("e", epoch, -1.0, epoch, 0);
        let mut out = Vec::new();
        capture.save(&mut out).unwrap();
        assert!(String::from_utf8(out)
            .unwrap()
            .contains("\"a,\"\"b\",1.250000000"));
        let mut out = Vec::new();
        capture.summary(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(
            "gpu_samples_seen=4 gpu_samples_retained=1 gpu_samples_dropped=1 gpu_samples_invalid=2"
        ));
        assert!(!text.contains("NaN"));
    }
}
