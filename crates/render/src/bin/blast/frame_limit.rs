//! Presentation pacing only. The authoritative simulation keeps its tick rate.
use std::time::{Duration, Instant};

pub struct Limiter {
    period: Option<Duration>,
    previous: Option<Instant>,
}

impl Limiter {
    pub fn new(fps: u32) -> Self {
        Self {
            period: (fps > 0).then(|| Duration::from_secs_f64(1.0 / f64::from(fps))),
            previous: None,
        }
    }

    fn remaining(&self, now: Instant) -> Option<Duration> {
        self.previous?
            .checked_add(self.period?)?
            .checked_duration_since(now)
            .filter(|wait| !wait.is_zero())
    }

    pub fn wait(&mut self) {
        if let Some(remaining) = self.remaining(Instant::now()) {
            // Yield the CPU; no busy wait or catch-up burst after a slow frame.
            std::thread::sleep(remaining);
        }
        self.previous = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_frame_waits_only_for_the_remaining_five_milliseconds() {
        let start = Instant::now();
        let mut limiter = Limiter::new(200);
        assert_eq!(limiter.remaining(start), None);
        limiter.previous = Some(start);
        assert_eq!(
            limiter.remaining(start + Duration::from_millis(2)),
            Some(Duration::from_millis(3))
        );
        assert_eq!(limiter.remaining(start + Duration::from_millis(5)), None);
        assert_eq!(limiter.remaining(start + Duration::from_millis(50)), None);
        // After a slow frame the next deadline follows the actual new start.
        limiter.previous = Some(start + Duration::from_millis(50));
        assert_eq!(
            limiter.remaining(start + Duration::from_millis(51)),
            Some(Duration::from_millis(4))
        );
    }

    #[test]
    fn explicit_zero_disables_pacing() {
        let start = Instant::now();
        let mut limiter = Limiter::new(0);
        limiter.previous = Some(start);
        assert_eq!(limiter.remaining(start), None);
    }
}
