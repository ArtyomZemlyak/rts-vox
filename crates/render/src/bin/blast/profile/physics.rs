//! Bounded physics trace: complete counters, longest exact scope per thread,
//! family and caller stage. Short scopes remain in totals, not raw CSV rows.
use super::Mark;
use std::collections::BTreeMap;

pub(super) struct Total {
    pub thread: Option<u32>,
    pub stage: Option<usize>,
    pub family: &'static str,
    pub jobs: u64,
    pub wall_ns: u128,
    pub max_ns: u128,
    pub cpu_ns: u128,
    pub cpu_samples: u64,
    pub cycles: u128,
    pub cycle_samples: u64,
}
struct Group {
    total: Total,
    longest: (usize, usize),
}
/// Malformed scopes/counters retain the original trace so validation cannot
/// silently turn an incomplete observation into a valid sampled profile.
pub(super) fn compact(events: Vec<Mark>) -> (Vec<Mark>, Vec<Total>) {
    // Never merge unidentified workers into one invented thread. This also
    // preserves raw observations on platforms without an OS counter backend.
    if events.iter().any(|mark| {
        mark.event.starts_with("physics_") && mark.cpu.is_none_or(|cpu| cpu.thread == 0)
    }) {
        return (events, Vec::new());
    }
    type Key = (Option<u32>, Option<usize>, &'static str);
    let mut pending: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
    let mut groups: BTreeMap<Key, Group> = BTreeMap::new();
    let mut keep = Vec::new();
    for (index, mark) in events.iter().enumerate() {
        if !mark.event.starts_with("physics_") {
            keep.push(index);
            continue;
        }
        let thread = mark.cpu.map(|c| c.thread);
        if let Some(family) = mark.event.strip_suffix("_begin") {
            pending
                .entry((thread, mark.unit, family))
                .or_default()
                .push(index);
            continue;
        }
        let Some(family) = mark.event.strip_suffix("_end") else {
            return (events, Vec::new());
        };
        let key = (thread, mark.unit, family);
        let Some(start) = pending.get_mut(&key).and_then(Vec::pop) else {
            return (events, Vec::new());
        };
        let begin = &events[start];
        let Some(wall_ns) = mark.ns.checked_sub(begin.ns) else {
            return (events, Vec::new());
        };
        let cpu_pair = begin.cpu.zip(mark.cpu);
        let cpu_before = cpu_pair.and_then(|(a, b)| a.time_ns.zip(b.time_ns));
        let cycles_before = cpu_pair.and_then(|(a, b)| a.cycles.zip(b.cycles));
        let cpu = cpu_before.and_then(|(a, b)| b.checked_sub(a));
        let cycles = cycles_before.and_then(|(a, b)| b.checked_sub(a));
        if (cpu_before.is_some() && cpu.is_none()) || (cycles_before.is_some() && cycles.is_none())
        {
            return (events, Vec::new());
        }
        let group = groups.entry(key).or_insert_with(|| Group {
            total: Total {
                thread,
                stage: mark.unit,
                family,
                jobs: 0,
                wall_ns: 0,
                max_ns: 0,
                cpu_ns: 0,
                cpu_samples: 0,
                cycles: 0,
                cycle_samples: 0,
            },
            longest: (start, index),
        });
        group.total.jobs += 1;
        group.total.wall_ns += wall_ns;
        if wall_ns > group.total.max_ns {
            group.longest = (start, index);
            group.total.max_ns = wall_ns;
        }
        if let Some(cpu) = cpu {
            group.total.cpu_ns += u128::from(cpu);
            group.total.cpu_samples += 1;
        }
        if let Some(cycles) = cycles {
            group.total.cycles += u128::from(cycles);
            group.total.cycle_samples += 1;
        }
    }
    if pending.values().any(|indices| !indices.is_empty()) {
        return (events, Vec::new());
    }
    let mut totals = Vec::with_capacity(groups.len());
    for group in groups.into_values() {
        keep.extend([group.longest.0, group.longest.1]);
        totals.push(group.total);
    }
    keep.sort_unstable();
    (keep.into_iter().map(|i| events[i]).collect(), totals)
}

#[cfg(test)]
mod tests {
    use super::super::Cpu;
    use super::*;

    fn scope(ns: u128, event: &'static str, stage: usize) -> Mark {
        Mark {
            lane: "task",
            event,
            unit: Some(stage),
            ns,
            cpu: Some(Cpu {
                thread: 7,
                cycles: Some(ns as u64 * 3),
                time_ns: Some(ns as u64),
            }),
        }
    }
    #[test]
    fn totals_cover_all_scopes_but_keep_only_the_longest_in_each_stage() {
        let events = [(0, 3, 0), (5, 12, 0), (20, 22, 0), (25, 26, 1)]
            .into_iter()
            .flat_map(|(a, b, stage)| {
                [
                    scope(a, "physics_wake_begin", stage),
                    scope(b, "physics_wake_end", stage),
                ]
            })
            .collect();
        let (kept, totals) = compact(events);
        assert_eq!(
            kept.iter().map(|m| m.ns).collect::<Vec<_>>(),
            [5, 12, 25, 26]
        );
        assert_eq!(totals.len(), 2);
        let t = &totals[0];
        assert_eq!(
            (
                t.jobs,
                t.wall_ns,
                t.max_ns,
                t.cpu_ns,
                t.cpu_samples,
                t.cycles
            ),
            (3, 12, 7, 12, 3, 36)
        );
    }
    #[test]
    fn malformed_scopes_and_regressed_counters_are_not_discarded() {
        let mut regressed = vec![
            scope(0, "physics_wake_begin", 0),
            scope(1, "physics_wake_end", 0),
        ];
        regressed[0].cpu.as_mut().unwrap().time_ns = Some(20);
        regressed[1].cpu.as_mut().unwrap().time_ns = Some(10);
        let unknown: Vec<_> = [0, 1, 2, 3]
            .into_iter()
            .map(|ns| {
                let mut mark = scope(
                    ns,
                    if ns % 2 == 0 {
                        "physics_wake_begin"
                    } else {
                        "physics_wake_end"
                    },
                    0,
                );
                mark.cpu = None;
                mark
            })
            .collect();
        for events in [
            vec![scope(0, "physics_wake_begin", 0)],
            vec![
                scope(0, "physics_wake_begin", 0),
                scope(1, "physics_wake_end", 1),
            ],
            vec![
                scope(2, "physics_wake_begin", 0),
                scope(1, "physics_wake_end", 0),
            ],
            regressed,
            unknown,
        ] {
            let length = events.len();
            let (kept, totals) = compact(events);
            assert_eq!(kept.len(), length);
            assert!(totals.is_empty());
        }
    }
}
