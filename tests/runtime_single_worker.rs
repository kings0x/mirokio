use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use mirokio::runtime::mirokio as Runtime;
use mirokio::spawn::spawn;

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if predicate() {
            return true;
        }

        thread::sleep(Duration::from_millis(1));
    }

    predicate()
}

#[test]
fn test_runtime_single_worker_still_makes_progress() {
    let runtime = Runtime::new(1);
    Box::leak(Box::new(runtime));

    let completed = Arc::new(AtomicUsize::new(0));
    let task_count = 8;

    for _ in 0..task_count {
        let completed_clone = completed.clone();
        spawn(async move {
            completed_clone.fetch_add(1, Ordering::SeqCst);
        });
    }

    assert!(
        wait_until(Duration::from_millis(1000), || {
            completed.load(Ordering::SeqCst) == task_count
        }),
        "a single-worker runtime should still make progress on spawned tasks"
    );
}
