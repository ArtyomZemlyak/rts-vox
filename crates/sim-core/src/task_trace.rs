//! Test-only validation of observer pairing on the actual worker threads.
use std::{collections::HashMap, sync::Mutex, thread::ThreadId};

#[derive(Default)]
pub(crate) struct Trace(Mutex<Vec<(ThreadId, usize, &'static str)>>);
impl Trace {
    pub(crate) fn record(&self, stage: usize, event: &'static str) {
        self.0
            .lock()
            .unwrap()
            .push((std::thread::current().id(), stage, event));
    }
    pub(crate) fn jobs(&self, begin: &str) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, _, event)| *event == begin)
            .count()
    }
    pub(crate) fn balanced(&self, expected: &[&str]) {
        let marks = self.0.lock().unwrap();
        let mut pending = HashMap::new();
        let mut seen = HashMap::new();
        for &(thread, stage, event) in marks.iter() {
            if let Some(family) = event.strip_suffix("_begin") {
                *pending.entry((thread, stage, family)).or_insert(0usize) += 1;
                *seen.entry(family).or_insert(0usize) += 1;
            } else if let Some(family) = event.strip_suffix("_end") {
                let count = pending
                    .get_mut(&(thread, stage, family))
                    .expect("unmatched end");
                assert!(*count > 0, "duplicate end: {family}");
                *count -= 1;
            } else {
                panic!("unknown observer event: {event}");
            }
        }
        assert!(pending.values().all(|&n| n == 0), "unclosed task");
        for family in expected {
            assert!(seen.contains_key(family), "missing family: {family}");
        }
    }
}
