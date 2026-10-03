//! Explicit render-compute experiments; IO/async and simulation pools stay separate.
use bevy::app::{TaskPoolOptions, TaskPoolPlugin};

pub fn plugin(workers: Option<usize>) -> TaskPoolPlugin {
    let mut options = TaskPoolOptions::default();
    if let Some(workers) = workers {
        assert!((1..=16).contains(&workers), "--compute-workers 1..=16");
        options.compute.min_threads = workers;
        options.compute.max_threads = workers;
    }
    TaskPoolPlugin {
        task_pool_options: options,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn override_is_fixed_and_does_not_reassign_other_pools() {
        let default = plugin(None).task_pool_options;
        for workers in [1, 4, 8, 16] {
            let options = plugin(Some(workers)).task_pool_options;
            assert_eq!(options.compute.min_threads, workers);
            assert_eq!(options.compute.max_threads, workers);
            assert_eq!(options.compute.percent, default.compute.percent);
            assert_eq!(format!("{:?}", options.io), format!("{:?}", default.io));
            assert_eq!(
                format!("{:?}", options.async_compute),
                format!("{:?}", default.async_compute)
            );
            assert_eq!(options.min_total_threads, default.min_total_threads);
            assert_eq!(options.max_total_threads, default.max_total_threads);
        }
    }
    #[test]
    #[should_panic(expected = "--compute-workers 1..=16")]
    fn empty_pool_is_rejected() {
        plugin(Some(0));
    }
    #[test]
    #[should_panic(expected = "--compute-workers 1..=16")]
    fn excessive_pool_is_rejected() {
        plugin(Some(17));
    }
}
