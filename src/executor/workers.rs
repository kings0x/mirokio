use std::sync::Arc;

use std::cell::RefCell;

use crossbeam::deque::{Injector, Stealer, Worker};

use crate::executor::task::*;

thread_local! {
    pub static CURRENT_WORKER: RefCell<Option<WorkerHandle>> = RefCell::new(None);
}

#[derive(Clone)]
pub struct WorkerHandle {
    pub queue: Arc<Worker<Arc<Task>>>,
}

fn find_task(
    local: &Worker<Arc<Task>>,
    global: &Injector<Arc<Task>>,
    stealers: &[Stealer<Arc<Task>>],
) -> Option<Arc<Task>> {
    local
        .pop()
        .or_else(|| global.steal_batch_and_pop(local).success())
        .or_else(|| {
            stealers
                .iter()
                .map(|s| s.steal())
                .find(|s| s.is_success())
                .and_then(|s| s.success())
        })
}

pub fn worker_loop(
    global: Arc<Injector<Arc<Task>>>,
    local: Worker<Arc<Task>>,
    stealers: Arc<Vec<Stealer<Arc<Task>>>>,
) {
    let handle = WorkerHandle {
        queue: Arc::new(local),
    };

    CURRENT_WORKER.with(|slot| *slot.borrow_mut() = Some(handle.clone()));

    loop {
        if let Some(task) = find_task(&handle.queue, &global, &stealers) {
            task.poll();
        } else {
            std::thread::yield_now();
        }
    }
}
