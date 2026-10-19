//! Opt-in inclusive wall spans for the rubble builder, outside the SIM.
//! Parent spans overlap: columns contains gather and merge. Do not sum them.
use std::sync::Mutex;
use std::time::Instant;

const FAMILIES: [&str; 9] = [
    "physics_rubble_prepare",
    "physics_rubble_classify",
    "physics_rubble_support_build",
    "physics_rubble_support_query",
    "physics_rubble_support_drop",
    "physics_rubble_gather_dispatch",
    "physics_rubble_merge",
    "physics_rubble_columns",
    "physics_rubble_publish",
];

#[derive(Default)]
struct Spans {
    started: [Option<Instant>; 9],
    nanos: [u128; 9],
    calls: [u32; 9],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_spans_overlap_without_losing_pairs_and_reset() {
        let profile = Profile::default();
        // Other stages and unselected leaf spans must not enter this table.
        profile.observe(1, "physics_rubble_prepare_begin");
        profile.observe(0, "physics_rubble_floor_begin");
        profile.observe(0, "physics_rubble_columns_begin");
        profile.observe(0, "physics_rubble_gather_dispatch_begin");
        profile.observe(0, "physics_rubble_gather_dispatch_end");
        profile.observe(0, "physics_rubble_merge_begin");
        profile.observe(0, "physics_rubble_merge_end");
        profile.observe(0, "physics_rubble_publish_begin");
        profile.observe(0, "physics_rubble_publish_end");
        profile.observe(0, "physics_rubble_columns_end");
        {
            let spans = profile.0.lock().unwrap();
            assert!(spans.started.iter().all(Option::is_none));
            assert_eq!(spans.calls, [0, 0, 0, 0, 0, 1, 1, 1, 1]);
            assert!(spans.nanos[7] >= spans.nanos[5] + spans.nanos[6] + spans.nanos[8]);
        }
        profile.finish_window(17, 17);
        let spans = profile.0.lock().unwrap();
        assert_eq!(spans.nanos, [0; 9]);
        assert_eq!(spans.calls, [0; 9]);
    }
}

#[derive(Default)]
pub(super) struct Profile(Mutex<Spans>);

impl Profile {
    pub(super) fn observe(&self, stage: usize, event: &'static str) {
        if stage != 0 {
            return;
        }
        let (family, begin) = if let Some(family) = event.strip_suffix("_begin") {
            (family, true)
        } else if let Some(family) = event.strip_suffix("_end") {
            (family, false)
        } else {
            return;
        };
        let Some(i) = FAMILIES.iter().position(|&name| name == family) else {
            return;
        };
        let now = Instant::now();
        let mut spans = self.0.lock().expect("rubble profile");
        if begin {
            assert!(spans.started[i].replace(now).is_none(), "nested {family}");
        } else {
            let start = spans.started[i].take().expect("rubble span begin");
            spans.nanos[i] += now.duration_since(start).as_nanos();
            spans.calls[i] += 1;
        }
    }

    pub(super) fn finish_window(&self, end: usize, ticks: usize) {
        let mut spans = self.0.lock().expect("rubble profile");
        assert!(spans.started.iter().all(Option::is_none));
        for (i, family) in FAMILIES.iter().enumerate() {
            println!(
                "rubble_span window={}..{end} family={family} inclusive_ns_per_tick={} calls={} diagnostic_only=true",
                end - ticks + 1,
                spans.nanos[i] / ticks as u128,
                spans.calls[i],
            );
        }
        *spans = Spans::default();
    }
}
