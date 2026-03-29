use std::sync::{
    Arc,
    Mutex, MutexGuard, Once, OnceLock,
    atomic::{AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use mirokio::runtime::mirokio as Runtime;
use mirokio::spawn::spawn;
use mirokio::timer::sleep;

static INTEGRATION_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static RUNTIME_INIT: Once = Once::new();

fn integration_test_guard() -> MutexGuard<'static, ()> {
    INTEGRATION_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn ensure_runtime() {
    RUNTIME_INIT.call_once(|| {
        let runtime = Runtime::new(2);
        Box::leak(Box::new(runtime));
    });
}

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

// Integration
#[test]
fn test_spawn_sleep_and_resume_completes() {
    let _guard = integration_test_guard();
    ensure_runtime();

    let completed = std::sync::Arc::new(AtomicUsize::new(0));
    let completed_clone = completed.clone();

    spawn(async move {
        sleep(Duration::from_millis(10)).await;
        completed_clone.fetch_add(1, Ordering::SeqCst);
    });

    assert!(
        wait_until(Duration::from_millis(1000), || {
            completed.load(Ordering::SeqCst) == 1
        }),
        "spawned task should resume after sleep and complete"
    );
}

#[test]
fn test_two_tasks_sleeping_concurrently_both_wake() {
    let _guard = integration_test_guard();
    ensure_runtime();

    let completed = std::sync::Arc::new(AtomicUsize::new(0));

    for delay in [10_u64, 15_u64] {
        let completed_clone = completed.clone();
        spawn(async move {
            sleep(Duration::from_millis(delay)).await;
            completed_clone.fetch_add(1, Ordering::SeqCst);
        });
    }

    assert!(
        wait_until(Duration::from_millis(1000), || {
            completed.load(Ordering::SeqCst) == 2
        }),
        "both sleeping tasks should wake and complete"
    );
}

#[test]
fn test_task_spawning_more_tasks_works() {
    let _guard = integration_test_guard();
    ensure_runtime();

    let completed = std::sync::Arc::new(AtomicUsize::new(0));
    let completed_clone = completed.clone();

    spawn(async move {
        let nested = completed_clone.clone();
        spawn(async move {
            nested.fetch_add(1, Ordering::SeqCst);
        });

        completed_clone.fetch_add(1, Ordering::SeqCst);
    });

    assert!(
        wait_until(Duration::from_millis(1000), || {
            completed.load(Ordering::SeqCst) == 2
        }),
        "outer task and nested task should both complete"
    );
}

#[test]
fn test_multiple_spawned_tasks_all_complete() {
    let _guard = integration_test_guard();
    ensure_runtime();

    let completed = std::sync::Arc::new(AtomicUsize::new(0));
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
        "all spawned tasks should eventually complete"
    );
}

#[test]
fn test_high_contention_many_tasks_many_threads() {
    let _guard = integration_test_guard();
    ensure_runtime();

    let completed = Arc::new(AtomicUsize::new(0));
    let task_count = 128;

    for i in 0..task_count {
        let completed_clone = completed.clone();
        spawn(async move {
            sleep(Duration::from_millis((i % 5) as u64)).await;
            completed_clone.fetch_add(1, Ordering::SeqCst);
        });
    }

    assert!(
        wait_until(Duration::from_millis(3000), || {
            completed.load(Ordering::SeqCst) == task_count
        }),
        "high-contention task burst should still allow every task to complete"
    );
}
