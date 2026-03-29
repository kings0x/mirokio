use std::sync::Arc;

use crossbeam::deque::Worker;
use mirokio::executor::workers::{CURRENT_WORKER, WorkerHandle};
use mirokio::runtime::GLOBAL_INJECTOR;
use mirokio::spawn::spawn;

fn drain_global_injector() {
    let global = GLOBAL_INJECTOR
        .get_or_init(|| Arc::new(crossbeam::deque::Injector::new()))
        .clone();
    let worker = Worker::new_lifo();

    loop {
        if global.steal_batch_and_pop(&worker).success().is_none() {
            break;
        }
    }
}

#[test]
fn test_spawn_on_main_thread_goes_to_global_queue() {
    drain_global_injector();
    CURRENT_WORKER.with(|slot| *slot.borrow_mut() = None);

    spawn(async {});

    let global = GLOBAL_INJECTOR
        .get_or_init(|| Arc::new(crossbeam::deque::Injector::new()))
        .clone();
    let worker = Worker::new_lifo();

    assert!(
        global.steal_batch_and_pop(&worker).success().is_some(),
        "spawning on a non-worker thread should enqueue onto the global injector"
    );
}

#[test]
fn test_spawn_on_worker_thread_goes_to_local_queue() {
    let worker = Arc::new(Worker::new_lifo());
    let handle = WorkerHandle {
        queue: worker.clone(),
    };

    CURRENT_WORKER.with(|slot| *slot.borrow_mut() = Some(handle));

    spawn(async {});

    let queued = worker.pop();

    CURRENT_WORKER.with(|slot| *slot.borrow_mut() = None);

    assert!(
        queued.is_some(),
        "spawning from a worker thread should enqueue onto that worker's local queue"
    );
}
