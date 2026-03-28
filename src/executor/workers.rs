use std::sync::Arc;

use std::cell::RefCell;

use crossbeam::deque::{Injector, Stealer, Worker};

use crate::executor::task::Task;

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

#[cfg(test)]
mod test {

    use super::*;

    use std::{
        future::Future,
        pin::Pin,
        result,
        sync::{
            Mutex,
            atomic::{AtomicBool, AtomicU8, Ordering},
        },
        task::{Context, Poll},
    };

    struct OurFuture {
        poll_count: Arc<AtomicU8>,
        is_completed: Arc<AtomicBool>,
    }

    impl Future for OurFuture {
        type Output = ();
        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            self.poll_count.fetch_add(1, Ordering::SeqCst);

            if self.is_completed.load(Ordering::SeqCst) {
                return Poll::Ready(());
            }

            Poll::Pending
        }
    }

    fn our_task() -> (Arc<Task>, Arc<AtomicU8>, Arc<AtomicBool>) {
        let poll_count = Arc::new(AtomicU8::new(0));
        let is_completed = Arc::new(AtomicBool::new(false));
        let future = OurFuture {
            poll_count: poll_count.clone(),
            is_completed: is_completed.clone(),
        };

        let task = Arc::new(Task {
            future: Mutex::new(Box::pin(future)),
            queue: Mutex::new(None),
        });

        (task, poll_count, is_completed)
    }

    #[test]
    fn test_find_task_returns_none_when_all_queues_empty() {
        let local: Worker<Arc<Task>> = Worker::new_lifo();
        let global = Injector::new();
        let stealers: Vec<Stealer<Arc<Task>>> = vec![];

        let result = find_task(&local, &global, &stealers);

        assert!(result.is_none())
    }

    #[test]
    fn test_find_task_prefers_local_over_global() {
        let local: Worker<Arc<Task>> = Worker::new_lifo();
        let global = Injector::new();
        let stealers: Vec<Stealer<Arc<Task>>> = vec![];

        let (local_task, _, _) = our_task();
        let (global_task, _, _) = our_task();

        local.push(local_task.clone());
        global.push(global_task.clone());

        let result = find_task(&local, &global, &stealers).unwrap();

        assert!(
            Arc::ptr_eq(&local_task, &result),
            "should have picked the local task"
        )
    }

    #[test]
    fn test_find_worker_steals_from_peer_when_local_empty() {
        let local: Worker<Arc<Task>> = Worker::new_lifo();
        let global = Injector::new();

        let peer: Worker<Arc<Task>> = Worker::new_lifo();
        let stealers: Vec<Stealer<Arc<Task>>> = vec![peer.stealer()];

        let (task, _, _) = our_task();

        peer.push(task.clone());

        let result = find_task(&local, &global, &stealers).unwrap();

        assert!(Arc::ptr_eq(&task, &result), "should have stolen from peer")
    }

    #[test]
    fn test_find_task_falls_back_to_global_when_local_empty() {
        let local: Worker<Arc<Task>> = Worker::new_lifo();
        let global = Injector::new();
        let stealers: Vec<Stealer<Arc<Task>>> = vec![];

        let (global_task, _, _) = our_task();

        global.push(global_task.clone());

        let result = find_task(&local, &global, &stealers).unwrap();

        assert!(Arc::ptr_eq(&global_task, &result))
    }

    #[test]
    fn test_worker_loop_processes_tasks_in_lifo_order() {
        let local: Worker<Arc<Task>> = Worker::new_lifo();
        let global = Injector::new();
        let stealers = vec![];

        let (first, _, _) = our_task();
        let (second, _, _) = our_task();

        local.push(first.clone());
        local.push(second.clone()); // pushed last, should come out first

        let result = find_task(&local, &global, &stealers).unwrap();
        assert!(
            Arc::ptr_eq(&result, &second),
            "LIFO: second pushed should be first out"
        );

        let result = find_task(&local, &global, &stealers).unwrap();
        assert!(Arc::ptr_eq(&result, &first));
    }
}
