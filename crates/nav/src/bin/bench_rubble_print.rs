//! Full publication versus exact page updates, including dense changes.
use nav::vehicle::{Changed, Rubble, RubblePrint};
use std::{hint::black_box, time::Instant};

fn main() {
    for side in [32, 128, 256] {
        let size = [side, side];
        let columns: Vec<_> = (0..side as i32)
            .flat_map(|x| (0..side as i32).map(move |z| ([x, z], 5)))
            .collect();
        let old = Rubble::from_sorted(columns.clone());
        for count in [0, 1, 64, columns.len()] {
            let mut columns = columns.clone();
            // Spread changes over distant pages, not just a single dense patch.
            for i in 0..count {
                let index = i * columns.len() / count;
                columns[index].1 = 6;
            }
            let next = Rubble::from_sorted(columns);
            let before = RubblePrint::of(&old, size);
            let after = RubblePrint::of(&next, size);
            let mut expected = Changed::default();
            before.changes(&after, &mut expected);
            let mut times = [Vec::new(), Vec::new()];
            let mut hashed = [0, 0];
            for round in 0..36 {
                // Alternate ordering; cloning setup stays outside the clock.
                for kind in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let mut print = before.clone();
                    let mut changed = Changed::default();
                    let begin = Instant::now();
                    let mut work = Default::default();
                    if black_box(&old) != black_box(&next) {
                        if kind == 0 {
                            let (full, counted) = RubblePrint::of_counted(black_box(&next), size);
                            work = counted;
                            print.changes(&full, &mut changed);
                            print = full;
                        } else {
                            work =
                                print.update(black_box(&old), black_box(&next), size, &mut changed);
                        }
                    }
                    black_box((&print, &changed, &work));
                    let ns = begin.elapsed().as_nanos();
                    assert_eq!(print, after);
                    assert_eq!(changed, expected);
                    hashed[kind] = work.hashed_columns;
                    if round >= 4 {
                        times[kind].push(ns);
                    }
                }
            }
            let mean = times
                .each_ref()
                .map(|t| t.iter().sum::<u128>() / t.len() as u128);
            println!(
                "columns={} changed={} rounds=32 full_mean_ns={} delta_mean_ns={} full_hashed={} delta_hashed={}",
                old.len(), count, mean[0], mean[1], hashed[0], hashed[1],
            );
        }
    }
}
