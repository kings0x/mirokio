use std::sync::{
    Arc, Mutex,
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

fn record_max(max_seen: &AtomicUsize, current: usize) {
    let mut observed = max_seen.load(Ordering::SeqCst);

    while current > observed {
        match max_seen.compare_exchange(observed, current, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => break,
            Err(actual) => observed = actual,
        }
    }
}

#[test]
fn test_runtime_tasks_complete_concurrently() {
    let runtime = Runtime::new(2);
    Box::leak(Box::new(runtime));

    let completed = Arc::new(AtomicUsize::new(0));
    let inflight = Arc::new(AtomicUsize::new(0));
    let max_inflight = Arc::new(AtomicUsize::new(0));
    let worker_threads = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let task_count = 8;

    for _ in 0..task_count {
        let completed_clone = completed.clone();
        let inflight_clone = inflight.clone();
        let max_inflight_clone = max_inflight.clone();
        let worker_threads_clone = worker_threads.clone();

        spawn(async move {
            let current = inflight_clone.fetch_add(1, Ordering::SeqCst) + 1;
            record_max(&max_inflight_clone, current);

            worker_threads_clone
                .lock()
                .unwrap()
                .insert(thread::current().id());

            thread::sleep(Duration::from_millis(40));

            inflight_clone.fetch_sub(1, Ordering::SeqCst);
            completed_clone.fetch_add(1, Ordering::SeqCst);
        });
    }

    assert!(
        wait_until(Duration::from_millis(1500), || {
            completed.load(Ordering::SeqCst) == task_count
        }),
        "all tasks should finish on a multi-worker runtime"
    );
    assert!(
        max_inflight.load(Ordering::SeqCst) > 1,
        "multi-worker runtime should make progress on more than one task at a time"
    );
    assert!(
        worker_threads.lock().unwrap().len() > 1,
        "concurrent runtime test should observe more than one worker thread"
    );
}
